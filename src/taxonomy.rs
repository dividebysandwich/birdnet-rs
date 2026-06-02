//! eBird taxonomy — maps BirdNET `Scientific_Common` labels to 6-letter eBird
//! species codes (≈ birdnet-go's embedded `eBird_taxonomy_codes` data).
//!
//! The source JSON is a flat object containing entries in both directions
//! (`"code": "Scientific_Common"` and `"Scientific_Common": "code"`); we keep
//! the label→code direction for lookups.

use std::collections::HashMap;
use std::path::Path;

/// Scientific_Common → eBird code lookup.
pub struct Taxonomy {
    map: HashMap<String, String>,
}

impl Taxonomy {
    /// Load from the eBird taxonomy codes JSON.
    pub fn load(path: &Path) -> anyhow::Result<Taxonomy> {
        let text = std::fs::read_to_string(path)
            .map_err(|e| anyhow::anyhow!("reading taxonomy {}: {e}", path.display()))?;
        let raw: HashMap<String, String> = serde_json::from_str(&text)
            .map_err(|e| anyhow::anyhow!("parsing taxonomy {}: {e}", path.display()))?;
        // Keep only the label→code direction (label keys contain '_').
        let map: HashMap<String, String> =
            raw.into_iter().filter(|(k, _)| k.contains('_')).collect();
        Ok(Taxonomy { map })
    }

    /// eBird code for a `(scientific, common)` pair, if known.
    pub fn code(&self, scientific: &str, common: &str) -> Option<&str> {
        self.map
            .get(&format!("{scientific}_{common}"))
            .map(String::as_str)
    }

    pub fn len(&self) -> usize {
        self.map.len()
    }

    pub fn is_empty(&self) -> bool {
        self.map.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn loads_and_looks_up_codes() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("tax.json");
        // Both directions present, as in the real file.
        std::fs::write(
            &path,
            r#"{
                "ostric2": "Struthio camelus_Common Ostrich",
                "Struthio camelus_Common Ostrich": "ostric2",
                "blujay": "Cyanocitta cristata_Blue Jay",
                "Cyanocitta cristata_Blue Jay": "blujay"
            }"#,
        )
        .unwrap();
        let tax = Taxonomy::load(&path).unwrap();
        assert_eq!(tax.len(), 2); // only label→code kept
        assert_eq!(tax.code("Cyanocitta cristata", "Blue Jay"), Some("blujay"));
        assert_eq!(tax.code("Nonexistent species", "Nothing"), None);
    }
}
