//! Foxkanes support — autonomous-parameter formulas.
//!
//! Every function here is pure: same inputs → same outputs, no I/O, no state.
//! The game contract reads its observable state (population counts, recent
//! hunts, FIRE bonding capacity, etc.) and calls into these functions to
//! get the *current effective parameter values*. No setter ever overrides
//! a result of these functions.
//!
//! Naming convention: `compute_<parameter>(<inputs>)` where inputs are
//! exactly the on-chain readable state that determines the parameter.

#![no_std]

use foxkanes_constants::*;

// ============================================================================
// Tax homeostasis
// ============================================================================

/// Effective safe-claim tax rate (bps), given current fox/farmer ratio.
///
/// Formula: `BASE_TAX_BPS × TARGET_FOX_RATIO / current_fox_ratio`,
/// clamped to [MIN_TAX_BPS, MAX_TAX_BPS].
///
/// Intuition: at target ratio, tax is BASE. Too many foxes → tax drops
/// (each fox earns less, role becomes unattractive, attrition self-corrects).
/// Too few foxes → tax rises (hunting/role-conversion incentive grows).
pub fn compute_tax_bps(current_fox_count: u128, current_farmer_count: u128) -> u128 {
    let total = current_fox_count.saturating_add(current_farmer_count);
    if total == 0 {
        return BASE_TAX_BPS;
    }
    let current_ratio_bps = current_fox_count
        .saturating_mul(BPS)
        .checked_div(total)
        .unwrap_or(0);
    if current_ratio_bps == 0 {
        // No foxes — tax goes to max (maximally attractive to become a fox)
        return MAX_TAX_BPS;
    }
    let raw = BASE_TAX_BPS
        .saturating_mul(TARGET_FOX_RATIO_BPS)
        .checked_div(current_ratio_bps)
        .unwrap_or(BASE_TAX_BPS);
    clamp(raw, MIN_TAX_BPS, MAX_TAX_BPS)
}

// ============================================================================
// Hunt success probability
// ============================================================================

/// Per-hunter base success probability (bps), scaling with target fox's
/// ripeness. `unclaimed` is the target fox's accumulated unclaimed taxes;
/// `moving_max` is the all-time max observed across the protocol.
///
/// Empty fox (unclaimed = 0): HUNT_BASE_PROB_BPS (5%).
/// Maximally ripe fox: HUNT_BASE_PROB_BPS + HUNT_RIPE_BONUS_BPS (20%).
pub fn compute_per_hunter_prob_bps(unclaimed: u128, moving_max: u128) -> u128 {
    if moving_max == 0 {
        return HUNT_BASE_PROB_BPS;
    }
    let bonus = HUNT_RIPE_BONUS_BPS
        .saturating_mul(unclaimed)
        .checked_div(moving_max)
        .unwrap_or(0);
    HUNT_BASE_PROB_BPS.saturating_add(bonus)
}

/// Party success probability (bps), given per-hunter probability and
/// party size. Equivalent to `1 - (1 - p)^N` in floating-point.
///
/// Computed iteratively in fixed-point to avoid floating math in WASM.
pub fn compute_party_success_bps(per_hunter_bps: u128, party_size: u128) -> u128 {
    // Probability of *all* hunters failing = (1 - p)^N. Party succeeds
    // if at least one hunter succeeds. So party success = 1 - (1-p)^N.
    let fail_per_hunter = BPS.saturating_sub(per_hunter_bps);
    let mut all_fail = BPS; // 1.0 in bps
    let mut i: u128 = 0;
    while i < party_size {
        all_fail = all_fail
            .saturating_mul(fail_per_hunter)
            .checked_div(BPS)
            .unwrap_or(0);
        i = i.saturating_add(1);
    }
    BPS.saturating_sub(all_fail)
}

// ============================================================================
// Hunt aging cost (failure penalty)
// ============================================================================

/// Aging cost (in blocks) levied on each failed hunting-party member.
///
/// Linear interpolation from BASE to MAX based on recent failed-hunt rate.
/// `failed_recent` is the count of failed hunts in the last RECENT_WINDOW
/// blocks; `recent_hunts_total` is the total hunts in that window.
///
/// All hunts fail (failed_recent == recent_hunts_total): MAX aging.
/// No hunts fail: BASE aging.
pub fn compute_aging_blocks(failed_recent: u128, recent_hunts_total: u128) -> u64 {
    if recent_hunts_total == 0 {
        return HUNT_BASE_AGING_BLOCKS;
    }
    let fail_rate_bps = failed_recent
        .saturating_mul(BPS)
        .checked_div(recent_hunts_total)
        .unwrap_or(0);
    // Linear: aging = BASE + (MAX - BASE) × fail_rate
    let span = HUNT_MAX_AGING_BLOCKS.saturating_sub(HUNT_BASE_AGING_BLOCKS) as u128;
    let bonus = span
        .saturating_mul(fail_rate_bps)
        .checked_div(BPS)
        .unwrap_or(0) as u64;
    HUNT_BASE_AGING_BLOCKS.saturating_add(bonus)
}

// ============================================================================
// Lifespan
// ============================================================================

/// Animal lifespan (in blocks), given the current effective daily mint rate.
///
/// `lifespan = TARGET_POPULATION / mints_per_day × DAY`
///
/// Steady-state: with N mints/day and L-day lifespan, population is
/// approximately N × L. Setting L = TARGET_POP / N keeps population near
/// target without any governance.
pub fn compute_lifespan_blocks(mints_per_day: u128) -> u64 {
    if mints_per_day == 0 {
        // No mints happening; assign max lifespan as fallback.
        return (TARGET_POPULATION as u64).saturating_mul(DAY);
    }
    let days = TARGET_POPULATION
        .checked_div(mints_per_day)
        .unwrap_or(TARGET_POPULATION) as u64;
    days.saturating_mul(DAY)
}

// ============================================================================
// Lottery weight (quadratic-discounted)
// ============================================================================

/// Player's lottery weight, given LP committed.
///
/// Returns `sqrt(lp_committed)`. Integer square root via Newton's method.
/// This bends the curve from "whale takes all" (linear) toward "many small
/// players compete" (sqrt). Same trick Gitcoin quadratic-funding uses.
pub fn compute_lottery_weight(lp_committed: u128) -> u128 {
    integer_sqrt(lp_committed)
}

/// Integer square root (Newton's method). Pure, no float, WASM-safe.
pub fn integer_sqrt(n: u128) -> u128 {
    if n == 0 {
        return 0;
    }
    let mut x = n;
    let mut y = x.saturating_add(1) / 2;
    while y < x {
        x = y;
        y = (x.saturating_add(n / x)) / 2;
    }
    x
}

// ============================================================================
// Lottery resolution
// ============================================================================

/// Returns true iff this commitment wins, given:
///   - the commitment's weight
///   - the day's total weight across all commitments
///   - the random seed derived from the reveal block hash + commitment id
///
/// Per-commitment win probability is calibrated to produce ~mints_per_day
/// winners in expectation: `p = min(1, mints_per_day × weight / total_weight)`.
pub fn lottery_check(
    commitment_weight: u128,
    total_weight: u128,
    mints_per_day: u128,
    seed_u128: u128,
) -> bool {
    if total_weight == 0 || commitment_weight == 0 || mints_per_day == 0 {
        return false;
    }
    // Numerator capped at total to keep probability ≤ 1.
    let num = mints_per_day
        .saturating_mul(commitment_weight)
        .min(total_weight);
    // Sample a uniform [0, total_weight) value from the seed.
    let sample = seed_u128 % total_weight;
    sample < num
}

/// Role assignment: returns true for fox (10% probability) on win.
/// Uses a separate region of the seed to avoid correlation with the
/// win/lose roll.
pub fn role_is_fox(seed_u128: u128) -> bool {
    // Use the high 64 bits of the seed for role to decorrelate from win roll
    // (which uses low bits via modulo).
    let role_roll = (seed_u128 >> 64) % (BPS as u128);
    role_roll < TARGET_FOX_RATIO_BPS
}

// ============================================================================
// Utility
// ============================================================================

#[inline]
pub fn clamp(v: u128, lo: u128, hi: u128) -> u128 {
    if v < lo {
        lo
    } else if v > hi {
        hi
    } else {
        v
    }
}

// ============================================================================
// Unit tests for the pure formulas. Run with `cargo test -p foxkanes-support`.
// (no_std but these tests run on host, so std is available in test cfg)
// ============================================================================

#[cfg(test)]
mod tests {
    extern crate std;
    use super::*;

    #[test]
    fn tax_at_target_equals_base() {
        // 900 farmers + 100 foxes = 10% fox ratio = target
        assert_eq!(compute_tax_bps(100, 900), BASE_TAX_BPS);
    }

    #[test]
    fn tax_too_many_foxes_drops() {
        // 200 foxes / 1000 = 20% > target → tax should drop
        let t = compute_tax_bps(200, 800);
        assert!(t < BASE_TAX_BPS);
        assert!(t >= MIN_TAX_BPS);
    }

    #[test]
    fn tax_too_few_foxes_rises() {
        // 50 foxes / 1000 = 5% < target → tax should rise
        let t = compute_tax_bps(50, 950);
        assert!(t > BASE_TAX_BPS);
        assert!(t <= MAX_TAX_BPS);
    }

    #[test]
    fn tax_no_foxes_is_max() {
        assert_eq!(compute_tax_bps(0, 100), MAX_TAX_BPS);
    }

    #[test]
    fn empty_fox_hunt_prob() {
        assert_eq!(compute_per_hunter_prob_bps(0, 1000), HUNT_BASE_PROB_BPS);
    }

    #[test]
    fn ripe_fox_hunt_prob() {
        assert_eq!(
            compute_per_hunter_prob_bps(1000, 1000),
            HUNT_BASE_PROB_BPS + HUNT_RIPE_BONUS_BPS
        );
    }

    #[test]
    fn party_success_monotone_in_size() {
        let p = HUNT_BASE_PROB_BPS;
        let s1 = compute_party_success_bps(p, 1);
        let s5 = compute_party_success_bps(p, 5);
        let s10 = compute_party_success_bps(p, 10);
        assert!(s1 <= s5);
        assert!(s5 <= s10);
        assert!(s10 < BPS);
    }

    #[test]
    fn aging_with_no_hunts_is_base() {
        assert_eq!(compute_aging_blocks(0, 0), HUNT_BASE_AGING_BLOCKS);
    }

    #[test]
    fn aging_with_all_failures_is_max() {
        assert_eq!(compute_aging_blocks(100, 100), HUNT_MAX_AGING_BLOCKS);
    }

    #[test]
    fn lifespan_at_max_rate() {
        // 100 mints/day, 9000 target → 90-day lifespan
        let l = compute_lifespan_blocks(100);
        assert_eq!(l, 90 * DAY);
    }

    #[test]
    fn sqrt_basics() {
        assert_eq!(integer_sqrt(0), 0);
        assert_eq!(integer_sqrt(1), 1);
        assert_eq!(integer_sqrt(4), 2);
        assert_eq!(integer_sqrt(100), 10);
        assert_eq!(integer_sqrt(99), 9);
        assert_eq!(integer_sqrt(101), 10);
    }
}
