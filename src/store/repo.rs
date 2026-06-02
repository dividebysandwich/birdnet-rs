//! Repository helpers — the queries the pipeline, server functions, and the
//! retention task need.

use chrono::Utc;
use sea_orm::ActiveValue::Set;
use sea_orm::{
    ActiveModelTrait, ColumnTrait, Condition, DatabaseConnection, EntityTrait, IntoActiveModel,
    ModelTrait, PaginatorTrait, QueryFilter, QueryOrder, QuerySelect, TransactionTrait,
};
use serde::Serialize;

use super::entities::{image_cache, note, note_review, result};
use crate::Detection;

/// A detection paired with its review status (if any).
pub type ReviewedNote = (note::Model, Option<note_review::Model>);

/// A detection with its raw top-k results, as returned to the media routes.
#[derive(Debug, Serialize)]
pub struct DetectionRecord {
    #[serde(flatten)]
    pub note: note::Model,
    pub results: Vec<result::Model>,
}

/// Persist a detection and its results in one transaction. Returns the new id.
pub async fn save_detection(db: &DatabaseConnection, det: &Detection) -> anyhow::Result<i32> {
    let ts = det.timestamp;
    let utc = ts.with_timezone(&Utc);

    let txn = db.begin().await?;

    let note = note::ActiveModel {
        date: Set(ts.format("%Y-%m-%d").to_string()),
        time: Set(ts.format("%H:%M:%S").to_string()),
        timestamp: Set(utc),
        scientific_name: Set(det.scientific_name.clone()),
        common_name: Set(det.common_name.clone()),
        species_code: Set(det.species_code.clone()),
        confidence: Set(det.confidence as f64),
        latitude: Set(0.0),
        longitude: Set(0.0),
        source: Set(det.source.clone()),
        clip_name: Set(det.clip_name.clone()),
        created_at: Set(Utc::now()),
        ..Default::default()
    };
    let note = note.insert(&txn).await?;

    for r in &det.results {
        let row = result::ActiveModel {
            note_id: Set(note.id),
            species: Set(format!("{}_{}", r.scientific_name, r.common_name)),
            confidence: Set(r.confidence as f64),
            ..Default::default()
        };
        row.insert(&txn).await?;
    }

    txn.commit().await?;
    Ok(note.id)
}

/// Most recent detections (with review status), newest first, paginated.
pub async fn recent(
    db: &DatabaseConnection,
    limit: u64,
    offset: u64,
) -> anyhow::Result<Vec<ReviewedNote>> {
    let (rows, _) = query(db, None, None, None, limit, offset).await?;
    Ok(rows)
}

/// Search detections by common name, scientific name, or species code substring.
pub async fn search(
    db: &DatabaseConnection,
    text: &str,
    limit: u64,
) -> anyhow::Result<Vec<ReviewedNote>> {
    let (rows, _) = query(db, Some(text), None, None, limit, 0).await?;
    Ok(rows)
}

/// Paginated query with optional text search and timestamp window. Returns the
/// page of `(note, review)` rows plus the total matching count.
pub async fn query(
    db: &DatabaseConnection,
    search: Option<&str>,
    since: Option<chrono::DateTime<Utc>>,
    until: Option<chrono::DateTime<Utc>>,
    limit: u64,
    offset: u64,
) -> anyhow::Result<(Vec<ReviewedNote>, u64)> {
    let mut cond = Condition::all();
    if let Some(q) = search.filter(|s| !s.is_empty()) {
        cond = cond.add(
            Condition::any()
                .add(note::Column::CommonName.contains(q))
                .add(note::Column::ScientificName.contains(q))
                .add(note::Column::SpeciesCode.contains(q)),
        );
    }
    if let Some(s) = since {
        cond = cond.add(note::Column::Timestamp.gte(s));
    }
    if let Some(u) = until {
        cond = cond.add(note::Column::Timestamp.lte(u));
    }

    let total = note::Entity::find().filter(cond.clone()).count(db).await?;
    let rows = note::Entity::find()
        .filter(cond)
        .find_also_related(note_review::Entity)
        .order_by_desc(note::Column::Timestamp)
        .limit(limit.max(1))
        .offset(offset)
        .all(db)
        .await?;
    Ok((rows, total))
}

/// A single detection with its results, by id.
pub async fn get(db: &DatabaseConnection, id: i32) -> anyhow::Result<Option<DetectionRecord>> {
    let Some(note) = note::Entity::find_by_id(id).one(db).await? else {
        return Ok(None);
    };
    let results = note.find_related(result::Entity).all(db).await?;
    Ok(Some(DetectionRecord { note, results }))
}

/// Set (or replace) the human review status for a detection.
pub async fn set_review(db: &DatabaseConnection, note_id: i32, verified: &str) -> anyhow::Result<()> {
    let existing = note_review::Entity::find()
        .filter(note_review::Column::NoteId.eq(note_id))
        .one(db)
        .await?;
    match existing {
        Some(row) => {
            let mut m = row.into_active_model();
            m.verified = Set(verified.to_string());
            m.update(db).await?;
        }
        None => {
            note_review::ActiveModel {
                note_id: Set(note_id),
                verified: Set(verified.to_string()),
                ..Default::default()
            }
            .insert(db)
            .await?;
        }
    }
    Ok(())
}

/// Total number of stored detections.
pub async fn count(db: &DatabaseConnection) -> anyhow::Result<u64> {
    Ok(note::Entity::find().count(db).await?)
}

/// `(id, clip_name)` for detections whose clip is older than `cutoff`.
pub async fn clips_older_than(
    db: &DatabaseConnection,
    cutoff: chrono::DateTime<Utc>,
) -> anyhow::Result<Vec<(i32, String)>> {
    let rows = note::Entity::find()
        .filter(note::Column::ClipName.is_not_null())
        .filter(note::Column::Timestamp.lt(cutoff))
        .all(db)
        .await?;
    Ok(rows
        .into_iter()
        .filter_map(|n| n.clip_name.map(|c| (n.id, c)))
        .collect())
}

/// Clear a detection's clip reference (after its file has been deleted).
pub async fn clear_clip(db: &DatabaseConnection, id: i32) -> anyhow::Result<()> {
    let m = note::ActiveModel { id: Set(id), clip_name: Set(None), ..Default::default() };
    m.update(db).await?;
    Ok(())
}

/// The cached image entry for a species, if any.
pub async fn image_get(
    db: &DatabaseConnection,
    scientific_name: &str,
) -> anyhow::Result<Option<image_cache::Model>> {
    Ok(image_cache::Entity::find()
        .filter(image_cache::Column::ScientificName.eq(scientific_name))
        .one(db)
        .await?)
}

/// Insert or replace the cached image entry for a species.
#[allow(clippy::too_many_arguments)]
pub async fn image_upsert(
    db: &DatabaseConnection,
    scientific_name: &str,
    provider: &str,
    remote_url: &str,
    local_path: Option<String>,
    license_name: &str,
    license_url: &str,
    author_name: &str,
) -> anyhow::Result<()> {
    let existing = image_get(db, scientific_name).await?;
    let mut m = match existing {
        Some(row) => row.into_active_model(),
        None => image_cache::ActiveModel {
            scientific_name: Set(scientific_name.to_string()),
            ..Default::default()
        },
    };
    m.provider = Set(provider.to_string());
    m.remote_url = Set(remote_url.to_string());
    m.local_path = Set(local_path);
    m.license_name = Set(license_name.to_string());
    m.license_url = Set(license_url.to_string());
    m.author_name = Set(author_name.to_string());
    m.cached_at = Set(Utc::now());
    m.save(db).await?;
    Ok(())
}

/// Of the given scientific names, which have a cached local image file.
pub async fn species_with_images(
    db: &DatabaseConnection,
    names: &[String],
) -> anyhow::Result<std::collections::HashSet<String>> {
    if names.is_empty() {
        return Ok(std::collections::HashSet::new());
    }
    let rows = image_cache::Entity::find()
        .filter(image_cache::Column::ScientificName.is_in(names.iter().map(String::as_str)))
        .filter(image_cache::Column::LocalPath.is_not_null())
        .all(db)
        .await?;
    Ok(rows.into_iter().map(|r| r.scientific_name).collect())
}
