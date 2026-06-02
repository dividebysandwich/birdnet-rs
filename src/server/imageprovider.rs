//! Species image provider (≈ birdnet-go's `internal/imageprovider`). Resolves a
//! scientific name → a Wikipedia thumbnail, downloads it to a local cache dir,
//! records it in `image_caches`, and nudges the dashboard to refetch so the
//! thumbnail appears. Wikimedia requires a descriptive User-Agent.

use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::Mutex;

use chrono::{Duration, Utc};
use sea_orm::DatabaseConnection;

use super::sse::SseManager;
use crate::config::ImageProviderSettings;
use crate::store::repo;

const WIKI_API: &str = "https://en.wikipedia.org/w/api.php";

pub struct ImageService {
    http: reqwest::Client,
    db: DatabaseConnection,
    sse: SseManager,
    cache_dir: PathBuf,
    ttl: Duration,
    user_agent: String,
    inflight: Mutex<HashSet<String>>,
}

impl ImageService {
    pub fn new(
        http: reqwest::Client,
        db: DatabaseConnection,
        sse: SseManager,
        cfg: &ImageProviderSettings,
    ) -> ImageService {
        ImageService {
            http,
            db,
            sse,
            cache_dir: cfg.cache_dir.clone(),
            ttl: Duration::days(cfg.ttl_days.max(1)),
            user_agent: cfg.user_agent.clone(),
            inflight: Mutex::new(HashSet::new()),
        }
    }

    /// Ensure a cached image exists (and is fresh) for `scientific_name`.
    /// Best-effort: logs and moves on. On a *new* image, triggers a UI refresh.
    pub async fn ensure(&self, scientific_name: &str) {
        // Skip non-species labels (Noise/Dog/Human/…): real names are binomial.
        if !scientific_name.contains(' ') {
            return;
        }

        match repo::image_get(&self.db, scientific_name).await {
            Ok(Some(row)) if Utc::now() - row.cached_at < self.ttl => return, // fresh
            Ok(_) => {}
            Err(e) => {
                tracing::warn!("image cache lookup failed: {e}");
                return;
            }
        }

        // Avoid duplicate concurrent fetches of the same species.
        {
            let mut guard = self.inflight.lock().unwrap();
            if !guard.insert(scientific_name.to_string()) {
                return;
            }
        }
        let result = self.fetch_and_cache(scientific_name).await;
        self.inflight.lock().unwrap().remove(scientific_name);

        match result {
            Ok(true) => self.sse.publish_refresh(),
            Ok(false) => {}
            Err(e) => tracing::warn!("image fetch for '{scientific_name}' failed: {e}"),
        }
    }

    /// Returns Ok(true) if a new image file was downloaded.
    async fn fetch_and_cache(&self, sci: &str) -> anyhow::Result<bool> {
        let Some((thumb_url, file_title)) = self.lookup_thumbnail(sci).await? else {
            // Negative cache so we don't hammer the API.
            repo::image_upsert(&self.db, sci, "wikipedia", "", None, "", "", "").await?;
            return Ok(false);
        };
        let (license, license_url, author) = self.lookup_license(&file_title).await.unwrap_or_default();

        let bytes = self
            .http
            .get(&thumb_url)
            .header(reqwest::header::USER_AGENT, &self.user_agent)
            .send()
            .await?
            .error_for_status()?
            .bytes()
            .await?;

        let ext = extension_of(&thumb_url);
        let filename = format!("{}.{ext}", sanitize(sci));
        tokio::fs::create_dir_all(&self.cache_dir).await?;
        tokio::fs::write(self.cache_dir.join(&filename), &bytes).await?;

        repo::image_upsert(
            &self.db,
            sci,
            "wikipedia",
            &thumb_url,
            Some(filename),
            &license,
            &license_url,
            &author,
        )
        .await?;
        tracing::info!("cached image for {sci}");
        Ok(true)
    }

    /// Query the Wikipedia `pageimages` API → `(thumbnail_url, file_title)`.
    async fn lookup_thumbnail(&self, sci: &str) -> anyhow::Result<Option<(String, String)>> {
        let body: serde_json::Value = self
            .http
            .get(WIKI_API)
            .header(reqwest::header::USER_AGENT, &self.user_agent)
            .query(&[
                ("action", "query"),
                ("format", "json"),
                ("formatversion", "2"),
                ("prop", "pageimages"),
                ("piprop", "thumbnail|name"),
                ("pilicense", "free"),
                ("pithumbsize", "400"),
                ("titles", sci),
                ("redirects", "1"),
            ])
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?;

        let page = &body["query"]["pages"][0];
        let thumb = page["thumbnail"]["source"].as_str();
        let title = page["pageimage"].as_str();
        Ok(match (thumb, title) {
            (Some(t), Some(f)) => Some((t.to_string(), format!("File:{f}"))),
            _ => None,
        })
    }

    /// Query `imageinfo`/`extmetadata` → `(license, license_url, author)`.
    async fn lookup_license(&self, file_title: &str) -> anyhow::Result<(String, String, String)> {
        let body: serde_json::Value = self
            .http
            .get(WIKI_API)
            .header(reqwest::header::USER_AGENT, &self.user_agent)
            .query(&[
                ("action", "query"),
                ("format", "json"),
                ("formatversion", "2"),
                ("prop", "imageinfo"),
                ("iiprop", "extmetadata"),
                ("titles", file_title),
                ("redirects", "1"),
            ])
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?;

        let meta = &body["query"]["pages"][0]["imageinfo"][0]["extmetadata"];
        let get = |k: &str| meta[k]["value"].as_str().map(strip_html).unwrap_or_default();
        Ok((get("LicenseShortName"), get("LicenseUrl"), get("Artist")))
    }
}

fn sanitize(name: &str) -> String {
    name.chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
        .collect()
}

fn extension_of(url: &str) -> String {
    let tail = url.rsplit('/').next().unwrap_or("");
    match tail.rsplit('.').next().map(|e| e.to_ascii_lowercase()) {
        Some(e) if matches!(e.as_str(), "jpg" | "jpeg" | "png" | "gif" | "webp" | "svg") => e,
        _ => "jpg".to_string(),
    }
}

/// Crude HTML-tag/entity stripping for the Wikimedia `Artist` field.
fn strip_html(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut in_tag = false;
    for c in s.chars() {
        match c {
            '<' => in_tag = true,
            '>' => in_tag = false,
            _ if !in_tag => out.push(c),
            _ => {}
        }
    }
    out.trim().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitize_and_ext() {
        assert_eq!(sanitize("Cyanocitta cristata"), "Cyanocitta_cristata");
        assert_eq!(extension_of("https://x/y/Foo.JPG"), "jpg");
        assert_eq!(extension_of("https://x/y/Foo"), "jpg");
        assert_eq!(extension_of("https://x/y/Foo.png"), "png");
    }

    #[test]
    fn strips_html() {
        assert_eq!(strip_html("<a href=\"x\">Jane Doe</a>"), "Jane Doe");
    }
}
