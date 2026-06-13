//! Repository helpers — the queries the pipeline, server functions, and the
//! retention task need.

use chrono::Utc;
use sea_orm::ActiveValue::Set;
use sea_orm::sea_query::{Expr, Order};
use sea_orm::{
    ActiveModelTrait, ColumnTrait, Condition, DatabaseConnection, EntityTrait, FromQueryResult,
    IntoActiveModel, ModelTrait, PaginatorTrait, QueryFilter, QueryOrder, QuerySelect,
    TransactionTrait,
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

// ---------------------------------------------------------------------------
// Aggregation queries for the calendar + statistics pages. All day/month/year
// bucketing keys off the denormalized `note.date` TEXT column (`YYYY-MM-DD`),
// so they are pure string operations — no timezone math in SQL.
// ---------------------------------------------------------------------------

/// One species and how many times it was detected.
#[derive(Debug, Clone, FromQueryResult)]
pub struct SpeciesCount {
    pub common_name: String,
    pub scientific_name: String,
    pub count: i64,
}

/// One calendar day and its detection count.
#[derive(Debug, Clone, FromQueryResult)]
pub struct DayCount {
    pub day: String,
    pub count: i64,
}

/// One time bucket (`YYYY-MM-DD` | `YYYY-MM` | `YYYY`) and its detection count.
#[derive(Debug, Clone, FromQueryResult)]
pub struct BucketCount {
    pub bucket: String,
    pub count: i64,
}

/// Top species by detection count within an optional timestamp window,
/// most-detected first. `limit` caps the result.
pub async fn species_counts(
    db: &DatabaseConnection,
    since: Option<chrono::DateTime<Utc>>,
    until: Option<chrono::DateTime<Utc>>,
    limit: u64,
) -> anyhow::Result<Vec<SpeciesCount>> {
    let mut cond = Condition::all();
    if let Some(s) = since {
        cond = cond.add(note::Column::Timestamp.gte(s));
    }
    if let Some(u) = until {
        cond = cond.add(note::Column::Timestamp.lte(u));
    }
    let rows = note::Entity::find()
        .select_only()
        .column(note::Column::CommonName)
        .column(note::Column::ScientificName)
        .column_as(note::Column::Id.count(), "count")
        .filter(cond)
        .group_by(note::Column::ScientificName)
        .group_by(note::Column::CommonName)
        .order_by(note::Column::Id.count(), Order::Desc)
        .limit(limit.max(1))
        .into_model::<SpeciesCount>()
        .all(db)
        .await?;
    Ok(rows)
}

/// Detections grouped by local calendar day, between two inclusive
/// `YYYY-MM-DD` date strings (lexical comparison is correct for this format).
pub async fn detections_per_day(
    db: &DatabaseConnection,
    from_date: &str,
    to_date: &str,
) -> anyhow::Result<Vec<DayCount>> {
    let rows = note::Entity::find()
        .select_only()
        .column_as(note::Column::Date, "day")
        .column_as(note::Column::Id.count(), "count")
        .filter(note::Column::Date.gte(from_date))
        .filter(note::Column::Date.lte(to_date))
        .group_by(note::Column::Date)
        .order_by_asc(note::Column::Date)
        .into_model::<DayCount>()
        .all(db)
        .await?;
    Ok(rows)
}

/// Detections grouped into time buckets between two inclusive `YYYY-MM-DD`
/// dates. `granularity` is `"day"` (uses `date` verbatim), `"month"`
/// (`substr(date,1,7)`), or `"year"` (`substr(date,1,4)`). An optional
/// `scientific_name` restricts the count to a single species.
pub async fn detections_by_bucket(
    db: &DatabaseConnection,
    granularity: &str,
    from_date: &str,
    to_date: &str,
    scientific_name: Option<&str>,
) -> anyhow::Result<Vec<BucketCount>> {
    let bucket = match granularity {
        "year" => Expr::cust("substr(date, 1, 4)"),
        "month" => Expr::cust("substr(date, 1, 7)"),
        _ => Expr::cust("date"),
    };
    let mut cond = Condition::all()
        .add(note::Column::Date.gte(from_date))
        .add(note::Column::Date.lte(to_date));
    if let Some(name) = scientific_name {
        cond = cond.add(note::Column::ScientificName.eq(name));
    }
    let rows = note::Entity::find()
        .select_only()
        .column_as(bucket.clone(), "bucket")
        .column_as(note::Column::Id.count(), "count")
        .filter(cond)
        .group_by(bucket.clone())
        .order_by(bucket, Order::Asc)
        .into_model::<BucketCount>()
        .all(db)
        .await?;
    Ok(rows)
}

/// Number of distinct species ever detected.
pub async fn distinct_species_count(db: &DatabaseConnection) -> anyhow::Result<u64> {
    #[derive(FromQueryResult)]
    struct Scalar {
        v: i64,
    }
    let row = note::Entity::find()
        .select_only()
        .column_as(Expr::cust("COUNT(DISTINCT scientific_name)"), "v")
        .into_model::<Scalar>()
        .one(db)
        .await?;
    Ok(row.map(|r| r.v.max(0) as u64).unwrap_or(0))
}

/// Earliest and latest detection dates (`YYYY-MM-DD`), if any detections exist.
pub async fn date_bounds(db: &DatabaseConnection) -> anyhow::Result<Option<(String, String)>> {
    #[derive(FromQueryResult)]
    struct Bounds {
        min_date: Option<String>,
        max_date: Option<String>,
    }
    let row = note::Entity::find()
        .select_only()
        .column_as(note::Column::Date.min(), "min_date")
        .column_as(note::Column::Date.max(), "max_date")
        .into_model::<Bounds>()
        .one(db)
        .await?;
    Ok(row.and_then(|b| match (b.min_date, b.max_date) {
        (Some(a), Some(z)) => Some((a, z)),
        _ => None,
    }))
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
