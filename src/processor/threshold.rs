//! Dynamic per-species thresholds (port of the core idea in birdnet-go's
//! `dynamic_threshold.go`).
//!
//! After repeated high-confidence detections of a species, its required
//! threshold is lowered in tiers so subsequent quieter calls of the same bird
//! still register. Tiers (fraction of the base threshold):
//!
//! | level | factor |
//! |-------|--------|
//! | 0     | 1.00   |
//! | 1     | 0.75   |
//! | 2     | 0.50   |
//! | 3     | 0.25   |
//!
//! Levels expire after `valid_hours` with no new high-confidence detection.

use std::collections::HashMap;

use chrono::{DateTime, Duration, Utc};

const FACTORS: [f32; 4] = [1.0, 0.75, 0.5, 0.25];

#[derive(Debug, Clone)]
struct SpeciesState {
    level: usize,
    high_conf_count: u32,
    last_learned: DateTime<Utc>,
}

/// Tracks dynamic threshold state per species.
pub struct DynamicThresholds {
    enabled: bool,
    base: f32,
    /// Confidence at/above which a detection counts as "high confidence".
    trigger: f32,
    /// High-confidence detections needed to advance one level.
    count_per_level: u32,
    valid: Duration,
    states: HashMap<String, SpeciesState>,
}

impl DynamicThresholds {
    pub fn new(enabled: bool, base: f32) -> DynamicThresholds {
        DynamicThresholds {
            enabled,
            base,
            trigger: (base + 0.4).min(0.9),
            count_per_level: 2,
            valid: Duration::hours(24),
            states: HashMap::new(),
        }
    }

    /// The effective threshold for `species` at time `now`, after applying (and
    /// expiring) any learned level.
    pub fn effective(&mut self, species: &str, now: DateTime<Utc>) -> f32 {
        if !self.enabled {
            return self.base;
        }
        let level = match self.states.get(species) {
            Some(s) if now - s.last_learned <= self.valid => s.level,
            Some(_) => {
                self.states.remove(species);
                0
            }
            None => 0,
        };
        self.base * FACTORS[level]
    }

    /// Record a confirmed detection so the species can learn a lower threshold.
    pub fn record(&mut self, species: &str, confidence: f32, now: DateTime<Utc>) {
        if !self.enabled || confidence < self.trigger {
            return;
        }
        let state = self
            .states
            .entry(species.to_string())
            .or_insert(SpeciesState { level: 0, high_conf_count: 0, last_learned: now });
        state.last_learned = now;
        state.high_conf_count += 1;
        if state.high_conf_count >= self.count_per_level && state.level < FACTORS.len() - 1 {
            state.level += 1;
            state.high_conf_count = 0;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn t(secs: i64) -> DateTime<Utc> {
        DateTime::<Utc>::from_timestamp(1_700_000_000 + secs, 0).unwrap()
    }

    #[test]
    fn disabled_always_returns_base() {
        let mut d = DynamicThresholds::new(false, 0.3);
        d.record("X", 0.99, t(0));
        assert_eq!(d.effective("X", t(0)), 0.3);
    }

    #[test]
    fn learns_lower_threshold_after_high_conf() {
        let mut d = DynamicThresholds::new(true, 0.3);
        assert_eq!(d.effective("Owl", t(0)), 0.3);
        d.record("Owl", 0.99, t(0));
        d.record("Owl", 0.99, t(1)); // 2 high-conf → level 1
        assert!((d.effective("Owl", t(2)) - 0.3 * 0.75).abs() < 1e-6);
    }

    #[test]
    fn level_expires_after_valid_window() {
        let mut d = DynamicThresholds::new(true, 0.3);
        d.record("Owl", 0.99, t(0));
        d.record("Owl", 0.99, t(1));
        // 25 hours later the level has expired back to base.
        assert_eq!(d.effective("Owl", t(25 * 3600)), 0.3);
    }
}
