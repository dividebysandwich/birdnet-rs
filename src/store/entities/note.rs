//! `notes` table — one row per stored detection (≈ birdnet-go `datastore.Note`).

use sea_orm::entity::prelude::*;
use serde::Serialize;

#[derive(Clone, Debug, PartialEq, DeriveEntityModel, Serialize)]
#[sea_orm(table_name = "notes")]
pub struct Model {
    #[sea_orm(primary_key)]
    #[serde(skip_deserializing)]
    pub id: i32,
    /// Local date `YYYY-MM-DD` (denormalized for cheap day grouping).
    pub date: String,
    /// Local time `HH:MM:SS`.
    pub time: String,
    /// Full UTC timestamp of the analyzed window start.
    pub timestamp: DateTimeUtc,
    pub scientific_name: String,
    pub common_name: String,
    /// eBird/species code; empty in Phase 1 (no taxonomy yet).
    pub species_code: String,
    pub confidence: f64,
    pub latitude: f64,
    pub longitude: f64,
    pub source: String,
    pub clip_name: Option<String>,
    pub created_at: DateTimeUtc,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {
    #[sea_orm(has_many = "super::result::Entity")]
    Result,
    #[sea_orm(has_one = "super::note_review::Entity")]
    NoteReview,
}

impl Related<super::result::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::Result.def()
    }
}

impl Related<super::note_review::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::NoteReview.def()
    }
}

impl ActiveModelBehavior for ActiveModel {}
