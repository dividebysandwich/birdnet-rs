//! Clip retention (≈ birdnet-go's `internal/diskmanager`). A background task
//! periodically deletes clip files older than `max_age_days` and clears their
//! reference in the database; the detection record itself is kept.

use std::path::PathBuf;
use std::time::Duration;

use chrono::Utc;
use sea_orm::DatabaseConnection;

use crate::store::repo;

/// How often to run a retention sweep.
const SWEEP_INTERVAL: Duration = Duration::from_secs(3600);

/// Spawn the periodic retention task (runs an initial sweep immediately).
pub fn spawn(db: DatabaseConnection, export_path: PathBuf, max_age_days: u32) {
    tokio::spawn(async move {
        loop {
            if let Err(e) = sweep(&db, &export_path, max_age_days).await {
                tracing::warn!("retention sweep failed: {e}");
            }
            tokio::time::sleep(SWEEP_INTERVAL).await;
        }
    });
    tracing::info!("clip retention enabled: deleting clips older than {max_age_days} days");
}

/// Delete clips older than the cutoff and clear their DB references.
async fn sweep(
    db: &DatabaseConnection,
    export_path: &std::path::Path,
    max_age_days: u32,
) -> anyhow::Result<()> {
    let cutoff = Utc::now() - chrono::Duration::days(max_age_days as i64);
    let old = repo::clips_older_than(db, cutoff).await?;

    let mut removed = 0usize;
    for (id, clip) in old {
        let path = export_path.join(&clip);
        match tokio::fs::remove_file(&path).await {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {} // already gone
            Err(e) => {
                tracing::warn!("retention: could not delete {}: {e}", path.display());
                continue; // leave the DB reference so we retry next sweep
            }
        }
        repo::clear_clip(db, id).await?;
        removed += 1;
    }

    if removed > 0 {
        tracing::info!("retention: removed {removed} clip(s) older than {max_age_days} day(s)");
    }
    Ok(())
}
