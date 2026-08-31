//! Scoring: speed is everything. You only score by actually clicking Waldo;
//! the faster you find him, the more points. Wrong clicks cost a penalty so
//! carpet-clicking the planet is a losing strategy.
//! Computed by the server (authoritative) from the same code the client uses.

pub const MAX_SCORE: u32 = 5000;
pub const MISS_PENALTY: u32 = 150;

/// How close (in world units) a click must land to Waldo to count as finding
/// him. Generous on purpose — he is ~1 unit tall.
pub const WALDO_HIT_RADIUS: f32 = 1.2;

/// `time_left_frac`: fraction of the round remaining when Waldo was clicked
/// (1.0 = instant, 0.0 = at the buzzer). `misses`: wrong clicks this round.
/// Returns 0 if Waldo was never found.
pub fn score_find(found: bool, time_left_frac: f32, misses: u32) -> u32 {
    if !found || !time_left_frac.is_finite() {
        return 0;
    }
    let base = MAX_SCORE as f32 * time_left_frac.clamp(0.0, 1.0);
    let penalty = (MISS_PENALTY * misses) as f32;
    (base - penalty).max(0.0).round() as u32
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn instant_find_scores_max() {
        assert_eq!(score_find(true, 1.0, 0), MAX_SCORE);
    }

    #[test]
    fn buzzer_beater_scores_zero_ish() {
        assert_eq!(score_find(true, 0.0, 0), 0);
        assert!(score_find(true, 0.01, 0) > 0);
    }

    #[test]
    fn faster_is_better() {
        let mut prev = MAX_SCORE + 1;
        for i in 0..=10 {
            let s = score_find(true, 1.0 - i as f32 / 10.0, 0);
            assert!(s < prev);
            prev = s;
        }
    }

    #[test]
    fn misses_cost_points() {
        assert_eq!(score_find(true, 1.0, 1), MAX_SCORE - MISS_PENALTY);
        assert!(score_find(true, 0.5, 3) < score_find(true, 0.5, 0));
        // spam-clicking bottoms out at zero, never negative
        assert_eq!(score_find(true, 0.1, 100), 0);
    }

    #[test]
    fn not_found_scores_zero() {
        assert_eq!(score_find(false, 1.0, 0), 0);
        assert_eq!(score_find(false, 0.5, 2), 0);
    }

    #[test]
    fn degenerate_inputs() {
        assert_eq!(score_find(true, f32::NAN, 0), 0);
        assert_eq!(score_find(true, 2.0, 0), MAX_SCORE); // clamped
    }
}
