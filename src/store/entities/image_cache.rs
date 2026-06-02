//! `image_caches` table — one cached species photo (≈ birdnet-go's `ImageCache`).

use sea_orm::entity::prelude::*;
use serde::Serialize;

#[derive(Clone, Debug, PartialEq, DeriveEntityModel, Serialize)]
#[sea_orm(table_name = "image_caches")]
pub struct Model {
    #[sea_orm(primary_key)]
    #[serde(skip_deserializing)]
    pub id: i32,
    /// Scientific name the image represents (unique lookup key).
    #[sea_orm(unique, indexed)]
    pub scientific_name: String,
    /// Source provider, e.g. `wikipedia`.
    pub provider: String,
    /// Remote image URL the file was downloaded from (empty if none found).
    pub remote_url: String,
    /// On-disk path of the cached image, relative to the cache dir (None = not found).
    pub local_path: Option<String>,
    pub license_name: String,
    pub license_url: String,
    pub author_name: String,
    /// When this entry was (re)fetched.
    pub cached_at: DateTimeUtc,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}
