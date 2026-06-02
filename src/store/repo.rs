//! Repository helpers — the queries the pipeline and web API need.

use chrono::Utc;
use sea_orm::ActiveValue::Set;
use sea_orm::{
    ActiveModelTrait, ColumnTrait, DatabaseConnection, EntityTrait, ModelTrait, PaginatorTrait,
    QueryFilter, QueryOrder, QuerySelect, TransactionTrait,
};
use serde::Serialize;

use super::entities::{note, result};
use crate::Detection;

/// A detection with its raw top-k results, as returned to the API.
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
        species_code: Set(String::new()),
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

/// Most recent detections, newest first, paginated.
pub async fn recent(
    db: &DatabaseConnection,
    limit: u64,
    offset: u64,
) -> anyhow::Result<Vec<note::Model>> {
    let rows = note::Entity::find()
        .order_by_desc(note::Column::Timestamp)
        .paginate(db, limit.max(1))
        .fetch_page(offset / limit.max(1))
        .await?;
    Ok(rows)
}

/// A single detection with its results, by id.
pub async fn get(db: &DatabaseConnection, id: i32) -> anyhow::Result<Option<DetectionRecord>> {
    let Some(note) = note::Entity::find_by_id(id).one(db).await? else {
        return Ok(None);
    };
    let results = note.find_related(result::Entity).all(db).await?;
    Ok(Some(DetectionRecord { note, results }))
}

/// Filter detections by species substring (case-insensitive on common name).
pub async fn by_species(
    db: &DatabaseConnection,
    species: &str,
    limit: u64,
) -> anyhow::Result<Vec<note::Model>> {
    let rows = note::Entity::find()
        .filter(note::Column::CommonName.contains(species))
        .order_by_desc(note::Column::Timestamp)
        .limit(limit)
        .all(db)
        .await?;
    Ok(rows)
}

/// Total number of stored detections.
pub async fn count(db: &DatabaseConnection) -> anyhow::Result<u64> {
    Ok(note::Entity::find().count(db).await?)
}
