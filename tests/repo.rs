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
async fn query_filters_by_time_and_paginates() {
    let (db, _d) = db().await;
    // Ages 0..4 days old.
    for i in 0..5 {
        repo::save_detection(&db, &detection(&format!("Bird{i}"), None, i)).await.unwrap();
    }

    // Window: last ~2.5 days → 3 detections (ages 0, 1, 2).
    let since = Utc::now() - Duration::days(2) - Duration::hours(12);
    let (page0, total) = repo::query(&db, None, Some(since), None, 2, 0).await.unwrap();
    assert_eq!(total, 3);
    assert_eq!(page0.len(), 2, "first page of size 2");
    let (page1, _) = repo::query(&db, None, Some(since), None, 2, 2).await.unwrap();
    assert_eq!(page1.len(), 1, "second page has the remainder");

    // Search narrows the total.
    let (_, named) = repo::query(&db, Some("Bird0"), None, None, 50, 0).await.unwrap();
    assert_eq!(named, 1);
}

#[tokio::test]
async fn image_cache_upsert_get_and_filter() {
    let (db, _d) = db().await;
    // No image yet.
    assert!(repo::image_get(&db, "Cyanocitta cristata").await.unwrap().is_none());

    // A species with a downloaded file, and one with only a negative entry.
    repo::image_upsert(&db, "Cyanocitta cristata", "wikipedia", "http://x/jay.jpg",
        Some("Cyanocitta_cristata.jpg".into()), "CC BY-SA", "http://lic", "Jane")
        .await.unwrap();
    repo::image_upsert(&db, "Corvus corax", "wikipedia", "", None, "", "", "")
        .await.unwrap();

    let row = repo::image_get(&db, "Cyanocitta cristata").await.unwrap().unwrap();
    assert_eq!(row.local_path.as_deref(), Some("Cyanocitta_cristata.jpg"));
    assert_eq!(row.author_name, "Jane");

    // Upsert replaces (no duplicate row, updated fields).
    repo::image_upsert(&db, "Cyanocitta cristata", "wikipedia", "http://x/jay2.jpg",
        Some("new.jpg".into()), "CC0", "", "Bob").await.unwrap();
    let row = repo::image_get(&db, "Cyanocitta cristata").await.unwrap().unwrap();
    assert_eq!(row.local_path.as_deref(), Some("new.jpg"));
    assert_eq!(row.author_name, "Bob");

    // Only species with a local file are returned.
    let names = vec!["Cyanocitta cristata".to_string(), "Corvus corax".to_string()];
    let with = repo::species_with_images(&db, &names).await.unwrap();
    assert!(with.contains("Cyanocitta cristata"));
    assert!(!with.contains("Corvus corax")); // negative entry (no local_path)
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
