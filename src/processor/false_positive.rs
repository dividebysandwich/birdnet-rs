//! False-positive confirmation filter (simplified port of birdnet-go's
//! `false_positive_filter.go`).
//!
//! A single-window hit is easy to trigger on noise. To confirm a species we
//! require it to be detected at least `min_detections` times within a short
//! reference window (default 6 s — about two overlapping 3 s windows). The
//! strictness is selected by a level 0..5; level 0 disables the filter.

use std::collections::HashMap;
use std::collections::VecDeque;

use chrono::{DateTime, Duration, Utc};

/// Reference window over which confirmations are counted.
const REFERENCE_WINDOW: Duration = Duration::milliseconds(6_000);
/// Cooldown after a confirmation before the same species can confirm again,
/// to avoid emitting a detection on every overlapping window.
const COOLDOWN: Duration = Duration::milliseconds(6_000);

/// Minimum in-window detections required at each level (index = level).
const MIN_DETECTIONS: [u32; 6] = [1, 1, 2, 3, 4, 5];

pub struct ConfirmationFilter {
    level: usize,
    recent: HashMap<String, VecDeque<DateTime<Utc>>>,
    last_confirmed: HashMap<String, DateTime<Utc>>,
}

impl ConfirmationFilter {
    pub fn new(level: usize) -> ConfirmationFilter {
        ConfirmationFilter {
            level: level.min(MIN_DETECTIONS.len() - 1),
            recent: HashMap::new(),
            last_confirmed: HashMap::new(),
        }
    }

    /// Register a candidate detection of `species` at `now`. Returns `true` if
    /// the species is now confirmed (and not in cooldown), meaning a detection
    /// should be emitted.
    pub fn confirm(&mut self, species: &str, now: DateTime<Utc>) -> bool {
        let needed = MIN_DETECTIONS[self.level];

        let hits = self.recent.entry(species.to_string()).or_default();
        hits.push_back(now);
        while let Some(&front) = hits.front() {
            if now - front > REFERENCE_WINDOW {
                hits.pop_front();
            } else {
                break;
            }
        }
        let count = hits.len() as u32;

        if count < needed {
            return false;
        }
        // Respect cooldown so overlapping windows don't double-emit.
        if let Some(&last) = self.last_confirmed.get(species)
            && now - last < COOLDOWN
        {
            return false;
        }
        self.last_confirmed.insert(species.to_string(), now);
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn t(ms: i64) -> DateTime<Utc> {
        DateTime::<Utc>::from_timestamp_millis(1_700_000_000_000 + ms).unwrap()
    }

    #[test]
    fn level_zero_confirms_immediately() {
        let mut f = ConfirmationFilter::new(0);
        assert!(f.confirm("A", t(0)));
    }

    #[test]
    fn higher_level_needs_multiple_hits() {
        let mut f = ConfirmationFilter::new(2); // needs 2
        assert!(!f.confirm("A", t(0)));
        assert!(f.confirm("A", t(1500)));
    }

    #[test]
    fn cooldown_suppresses_immediate_reconfirm() {
        let mut f = ConfirmationFilter::new(0);
        assert!(f.confirm("A", t(0)));
        assert!(!f.confirm("A", t(100)));
        assert!(f.confirm("A", t(7000))); // past cooldown
    }

    #[test]
    fn stale_hits_drop_out_of_window() {
        let mut f = ConfirmationFilter::new(2); // needs 2 within 6 s
        assert!(!f.confirm("A", t(0)));
        // Second hit arrives after the window — first has expired, still 1.
        assert!(!f.confirm("A", t(7000)));
    }
}
