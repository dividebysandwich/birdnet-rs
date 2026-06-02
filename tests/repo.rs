//! Integration tests for the new repository queries: review, search, retention.

use std::sync::Arc;

use birdnet_rs::Detection;
use birdnet_rs::inference::Prediction;
use birdnet_rs::store::{connect, repo};
use chrono::{Duration, Local, Utc};

async fn db() -> (sea_orm::DatabaseConnection, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    let db = connect(&dir.path().join("t.db")).await.unwrap();
    (db, dir)
}

fn detection(common: &str, clip: Option<&str>, age_days: i64) -> Detection {
    Detection {
        id: None,
        timestamp: Local::now() - Duration::days(age_days),
        scientific_name: "Cyanocitta cristata".into(),
        common_name: common.into(),
        species_code: "blujay".into(),
        confidence: 0.9,
        source: "test".into(),
        clip_name: clip.map(str::to_string),
        results: vec![Prediction {
            scientific_name: "Cyanocitta cristata".into(),
            common_name: common.into(),
            confidence: 0.9,
        }],
        pcm: Arc::new(vec![0.0; 10]),
    }
}

#[tokio::test]
async fn review_round_trips_through_recent() {
    let (db, _d) = db().await;
    let id = repo::save_detection(&db, &detection("Blue Jay", Some("a.wav"), 0)).await.unwrap();

    let rows = repo::recent(&db, 10, 0).await.unwrap();
    assert_eq!(rows.len(), 1);
    assert!(rows[0].1.is_none(), "unreviewed → no review row");

    repo::set_review(&db, id, "correct").await.unwrap();
    let rows = repo::recent(&db, 10, 0).await.unwrap();
    assert_eq!(rows[0].1.as_ref().unwrap().verified, "correct");

    // set_review replaces, not duplicates.
    repo::set_review(&db, id, "false_positive").await.unwrap();
    let rows = repo::recent(&db, 10, 0).await.unwrap();
    assert_eq!(rows[0].1.as_ref().unwrap().verified, "false_positive");
}

#[tokio::test]
async fn search_matches_name_and_code() {
    let (db, _d) = db().await;
    repo::save_detection(&db, &detection("Blue Jay", None, 0)).await.unwrap();
    repo::save_detection(&db, &detection("American Crow", None, 0)).await.unwrap();

    assert_eq!(repo::search(&db, "Blue", 10).await.unwrap().len(), 1);
    assert_eq!(repo::search(&db, "blujay", 10).await.unwrap().len(), 2); // species_code
    assert_eq!(repo::search(&db, "nope", 10).await.unwrap().len(), 0);
}

#[tokio::test]
async fn retention_finds_and_clears_old_clips() {
    let (db, _d) = db().await;
    let old = repo::save_detection(&db, &detection("Old", Some("old.wav"), 40)).await.unwrap();
    repo::save_detection(&db, &detection("New", Some("new.wav"), 1)).await.unwrap();

    let cutoff = Utc::now() - Duration::days(30);
    let stale = repo::clips_older_than(&db, cutoff).await.unwrap();
    assert_eq!(stale.len(), 1);
    assert_eq!(stale[0], (old, "old.wav".to_string()));

    repo::clear_clip(&db, old).await.unwrap();
    let after = repo::clips_older_than(&db, cutoff).await.unwrap();
    assert!(after.is_empty(), "clip reference cleared");
    // The detection record itself is preserved.
    assert_eq!(repo::count(&db).await.unwrap(), 2);
}
