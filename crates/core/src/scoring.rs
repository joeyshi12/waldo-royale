//! Speed scoring: points for clicking Waldo, scaled by time remaining,
//! with a penalty per wrong click.

pub const MAX_SCORE: u32 = 5000;
pub const MISS_PENALTY: u32 = 150;

/// Max distance from Waldo for a click to count as finding him.
pub const WALDO_HIT_RADIUS: f32 = 1.2;

/// `time_left_frac` is the fraction of round time remaining at the find.
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
