//! Detection datastore — SeaORM over SQLite (≈ birdnet-go `internal/datastore`).
//!
//! Phase 1 targets SQLite; SeaORM keeps MySQL/Postgres open for later. Schema
//! is created on startup via [`migration::Migrator`].

pub mod entities;
pub mod migration;
pub mod repo;

use std::path::Path;

use sea_orm::{ConnectOptions, Database, DatabaseConnection};
use sea_orm_migration::MigratorTrait;

/// Open (creating if needed) the SQLite database at `path` and run migrations.
pub async fn connect(path: &Path) -> anyhow::Result<DatabaseConnection> {
    // `mode=rwc` creates the file if it does not exist.
    let url = format!("sqlite://{}?mode=rwc", path.display());
    let mut opts = ConnectOptions::new(url);
    opts.sqlx_logging(false);

    let db = Database::connect(opts).await?;
    migration::Migrator::up(&db, None).await?;
    tracing::info!("datastore ready at {}", path.display());
    Ok(db)
}
