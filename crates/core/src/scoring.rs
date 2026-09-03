//! Rank-based round scoring: finding Waldo earns points by finish order,
//! the supporting cast earns flat bonuses, Odlaw and wrong clicks cost you.

pub const MISS_PENALTY: u32 = 25;
pub const ODLAW_PENALTY: u32 = 150;
pub const BONUS_WENDA: u32 = 100;
pub const BONUS_WIZARD: u32 = 75;
pub const BONUS_WOOF: u32 = 150; // just a tail in a bush: hardest to spot

/// Max distance from a character for a click to count as finding them.
pub const WALDO_HIT_RADIUS: f32 = 1.2;

/// Points for finding Waldo Nth (1-based). 0 means not found.
pub fn rank_points(rank: u32) -> u32 {
    match rank {
        0 => 0,
        1 => 1000,
        2 => 700,
        3 => 550,
        4 => 450,
        5 => 400,
        r => 400u32.saturating_sub(25 * (r - 5)).max(250),
    }
}

/// Net bonus points for the round (cast finds minus Odlaw), may be negative.
pub fn bonus_points(wenda: bool, woof: bool, wizard: bool, odlaw: bool) -> i32 {
    (wenda as i32) * BONUS_WENDA as i32
        + (woof as i32) * BONUS_WOOF as i32
        + (wizard as i32) * BONUS_WIZARD as i32
        - (odlaw as i32) * ODLAW_PENALTY as i32
}

/// Total round score, floored at zero.
pub fn score_round(
    found_rank: u32,
    misses: u32,
    wenda: bool,
    woof: bool,
    wizard: bool,
    odlaw: bool,
) -> u32 {
    let pts = rank_points(found_rank) as i64 + bonus_points(wenda, woof, wizard, odlaw) as i64
        - (MISS_PENALTY * misses) as i64;
    pts.max(0) as u32
}

pub const MAX_SCORE: u32 = 1000 + BONUS_WENDA + BONUS_WIZARD + BONUS_WOOF;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rank_order_pays() {
        let mut prev = u32::MAX;
        for r in 1..=12 {
            let p = rank_points(r);
            assert!(p <= prev, "rank {r} should not pay more than rank {}", r - 1);
            assert!(p >= 250);
            if prev > 250 && prev != u32::MAX {
                assert!(p < prev, "ranks above the floor must strictly decrease");
            }
            prev = p;
        }
        assert_eq!(rank_points(1), 1000);
        assert_eq!(rank_points(0), 0);
    }

    #[test]
    fn perfect_round() {
        assert_eq!(score_round(1, 0, true, true, true, false), MAX_SCORE);
    }

    #[test]
    fn misses_and_odlaw_cost() {
        assert_eq!(score_round(1, 2, false, false, false, false), 1000 - 50);
        assert_eq!(score_round(1, 0, false, false, false, true), 1000 - 150);
        assert!(score_round(2, 0, false, false, false, false) < score_round(1, 0, false, false, false, false));
    }

    #[test]
    fn not_found_can_still_bonus() {
        assert_eq!(score_round(0, 0, true, false, false, false), BONUS_WENDA);
    }

    #[test]
    fn never_negative() {
        assert_eq!(score_round(0, 50, false, false, false, true), 0);
    }
}
