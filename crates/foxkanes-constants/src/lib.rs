//! Foxkanes Constants
//!
//! Every value in this file is hardcoded forever. The protocol has no admin
//! and no governance. Changing any of these values requires a new deploy
//! producing a distinct contract (i.e., a community fork), not an upgrade
//! to the existing one.
//!
//! Three categories live here:
//!   1. Block-duration constants, aligned with FIRE / Bitcoin halving cycle
//!   2. Template IDs for the four Foxkanes contracts (game, animal,
//!      commitment, zap). Chosen above FIRE's 0x100..0x104 range to avoid
//!      any chance of collision; see also AMM_POOL_TEMPLATE etc. which sit
//!      at 0xffef/0xffed on alkanes runtime.
//!   3. Game-economic constants. These set the autonomous-parameter
//!      formulas' inputs. Each carries a comment explaining what changes
//!      mechanically if the value is wrong — useful for the inevitable
//!      "should this be 5% or 10%?" debate that happens once and never
//!      again because nobody can change it.

#![no_std]

// ============================================================================
// Block Duration Constants — match FIRE for sanity
// 1 block ≈ 10 min on Bitcoin mainnet
// ============================================================================

/// Bitcoin halving interval
pub const BITCOIN_HALVING: u64 = 210_000;
/// FIRE halving interval (half of Bitcoin halving)
pub const HALVING_INTERVAL: u64 = BITCOIN_HALVING / 2; // 105,000
/// One year in blocks
pub const YEAR: u64 = HALVING_INTERVAL / 2; // 52,500
/// Six months in blocks
pub const SIX_MONTHS: u64 = HALVING_INTERVAL / 4; // 26,250
/// Three months in blocks
pub const THREE_MONTHS: u64 = HALVING_INTERVAL / 8; // 13,125
/// One month in blocks
pub const MONTH: u64 = HALVING_INTERVAL / 24; // 4,375
/// One week in blocks
pub const WEEK: u64 = HALVING_INTERVAL / 100; // 1,050
/// One day in blocks (~144 blocks at 10 min)
pub const DAY: u64 = 144;

// ============================================================================
// Template IDs
// Chosen above FIRE's 0x100..0x104 range. Each contract is deployed once at
// `(block: 3, tx: TEMPLATE_ID)`; live AlkaneIds become `(block: 4, tx: ...)`
// after the deploy is recorded. See FIRE's `deploy_target` / `deployed_id`
// pattern in fire-constants.
// ============================================================================

/// foxkanes-game (the factory + game loop)
pub const FOXKANES_GAME: u128 = 0x200;
/// foxkanes-animal (per-NFT receipt template; spawned via block-6 factory)
pub const FOXKANES_ANIMAL: u128 = 0x201;
/// foxkanes-commitment (per-lottery-entry receipt template)
pub const FOXKANES_COMMITMENT: u128 = 0x202;
/// foxkanes-zap (peripheral router; replaceable without touching game state)
pub const FOXKANES_ZAP: u128 = 0x203;

/// Helper: convert a template id to its deploy target.
pub const fn deploy_target(id: u128) -> (u128, u128) {
    (3, id)
}
/// Helper: convert a template id to its live deployed AlkaneId (post-deploy).
pub const fn deployed_id(id: u128) -> (u128, u128) {
    (4, id)
}

// FIRE references (already deployed). We bond into these on player's behalf.
// Block: 4 because these are live AlkaneIds, not templates. Source:
// fire-constants.
pub const FIRE_TOKEN_BLOCK: u128 = 4;
pub const FIRE_TOKEN_TX: u128 = 0x100;
pub const FIRE_BONDING_BLOCK: u128 = 4;
pub const FIRE_BONDING_TX: u128 = 0x103;
pub const FIRE_REDEMPTION_BLOCK: u128 = 4;
pub const FIRE_REDEMPTION_TX: u128 = 0x104;
pub const FIRE_TREASURY_BLOCK: u128 = 4;
pub const FIRE_TREASURY_TX: u128 = 0x102;

// DIESEL and frBTC genesis alkanes (cross-network identical AlkaneIds).
pub const DIESEL_BLOCK: u128 = 2;
pub const DIESEL_TX: u128 = 0;
pub const FRBTC_BLOCK: u128 = 32;
pub const FRBTC_TX: u128 = 0;

// ============================================================================
// Game-Economic Constants
// All values are forever; the autonomous formulas in foxkanes-game compute
// the *current* effective parameters from these + on-chain state.
// ============================================================================

// ---------- Decimals ----------
/// 8 decimals (matches FIRE / BTC)
pub const DECIMALS: u32 = 8;
pub const DECIMAL_FACTOR: u128 = 100_000_000;
/// Basis-point precision: 10_000 bps = 100%
pub const BPS: u128 = 10_000;

// ---------- Population ----------
/// Target equilibrium population. Lifespan auto-adjusts to maintain this
/// given the actual daily mint rate.
///   If wrong-too-large: lifespans extend, game pace slows, animals
///     accumulate state, hunts become more impactful per kill.
///   If wrong-too-small: lifespans shrink, churn dominates, players
///     can't develop strategic positions.
pub const TARGET_POPULATION: u128 = 9_000;

/// Target fox/farmer ratio in basis points (10% foxes, 90% farmers).
/// The fox-tax-rate formula uses this to homeostatically balance the
/// population: when fox ratio drifts above this, tax drops; below, tax
/// rises. Maintains the predator/prey equilibrium without governance.
pub const TARGET_FOX_RATIO_BPS: u128 = 1_000; // 10%

// ---------- Lottery ----------
/// Hard cap on daily lottery winners. Final mints/day is the min of this
/// and FIRE's current bonding capacity. Capped so a single high-yield
/// epoch can't drain FIRE's bonding pool through Foxkanes alone.
pub const HARDCODED_MAX_DAILY_MINTS: u128 = 100;

/// Lottery commit window in blocks. Players have this long to commit
/// before the day closes and reveals begin. ~24 hours.
pub const LOTTERY_COMMIT_WINDOW: u64 = DAY;

/// Reveal delay in blocks past the commit close. The block hash at
/// (commit_close_block + REVEAL_DELAY) is the randomness seed. Far enough
/// in the future that miners can't economically grind it.
pub const REVEAL_DELAY: u64 = 1;

/// Bounty paid (in FIRE base units) to whoever calls reveal() on an
/// unrevealed commitment. Self-funded from the game's treasury yield;
/// scales with treasury health via the autonomous formula in the game
/// contract. This constant is the *multiplier*; effective bounty is
/// `protocol_yield_per_block × BOUNTY_MULTIPLIER_NUM / BOUNTY_MULTIPLIER_DEN`.
pub const BOUNTY_MULTIPLIER_NUM: u128 = 1;
pub const BOUNTY_MULTIPLIER_DEN: u128 = 10_000; // 1 bp of last-block yield

// ---------- Protocol fee ----------
/// Foxkanes' fee on every mint LP commitment, in bps. 1% of player LP is
/// retained as protocol-owned-liquidity; the rest is bonded to FIRE on
/// the player's behalf. This treasury grows monotonically and is itself
/// bonded back to FIRE periodically — the recursive POL pattern.
pub const PROTOCOL_FEE_BPS: u128 = 100; // 1%

// ---------- Tax ----------
/// Base safe-claim tax rate, in bps. When fox/farmer ratio is exactly at
/// target, this is the tax. Effective tax scales as
/// `BASE_TAX_BPS × TARGET_FOX_RATIO_BPS / current_fox_ratio_bps`,
/// bounded by MIN_TAX_BPS and MAX_TAX_BPS.
pub const BASE_TAX_BPS: u128 = 2_000; // 20%
pub const MIN_TAX_BPS: u128 = 500; // 5% floor — foxes always earn something
pub const MAX_TAX_BPS: u128 = 5_000; // 50% ceiling — farmers always keep half

/// Risky-claim outcome rates. Independent of population dynamics — pure
/// variance preference. 50% chance to keep everything, 50% chance to lose
/// everything to foxes. The expected value matches a 50% tax, slightly
/// worse than the homeostatic safe-claim rate at equilibrium, but a
/// player who likes variance gets the option.
pub const RISKY_KEEP_PROBABILITY_BPS: u128 = 5_000; // 50%

// ---------- Hunts ----------
/// Hunt party size bounds. A hunting party must have between MIN_HUNT
/// and MAX_HUNT sheep staked into the hunt. Min prevents trivial solo
/// hunts; max caps the worst-case per-hunt computation cost.
pub const MIN_HUNT_PARTY: u128 = 5;
pub const MAX_HUNT_PARTY: u128 = 10;

/// Base per-hunter success probability against an "empty" fox (zero
/// unclaimed taxes), in bps. With a 5-sheep party, success is
/// ~22% (1 - (1 - 0.05)^5).
pub const HUNT_BASE_PROB_BPS: u128 = 500; // 5%

/// Bonus per-hunter success probability against a maximally-ripe fox
/// (unclaimed taxes at moving-max), in bps. With a 5-sheep party,
/// success is ~67% (1 - (1 - 0.20)^5).
pub const HUNT_RIPE_BONUS_BPS: u128 = 1_500; // +15%

/// Aging cost (in blocks) on each member of a failed hunting party.
/// Scales with recent failed-hunt-count via the formula in the game
/// contract: more failed hunts recently → higher cost to discourage
/// frivolous attempts. This constant is the floor and the multiplier base.
pub const HUNT_BASE_AGING_BLOCKS: u64 = DAY; // 1 day = floor cost
pub const HUNT_MAX_AGING_BLOCKS: u64 = 10 * DAY; // 10 days = ceiling

// ---------- TWAP ----------
/// Number of daily price samples used for the autonomous TWAP of FIRE
/// price. 7 = weekly TWAP. Sampled at the close of each lottery day from
/// fire-bonding's spot price.
pub const TWAP_WINDOW_DAYS: u128 = 7;

// ---------- Yield model (v0, pending FIRE distribution wiring) ----------
/// Yield units accrued per block per staked animal. Abstract unit until
/// we wire to real FIRE staking distribution; for testability we use a
/// deterministic constant. In production this will be replaced by a
/// staticcall to fire-staking's GetCurrentEmissionRate (opcode 15).
///
/// Chosen to make multi-block yield computations land at round numbers
/// in the test fixtures: 1000 units/block × 100 blocks = 100_000.
pub const TEST_YIELD_PER_BLOCK_PER_STAKE: u128 = 1_000;

/// Risky-claim seed salt — combined into the seed hash so the risky-claim
/// coinflip can't be predicted from the same context that determined the
/// lottery outcome. Distinct from REVEAL_DELAY and the lottery salts in
/// foxkanes-support to keep entropy clean.
pub const RISKY_CLAIM_SALT: u128 = 0xC0FFEE_FACE_FEEDu128;

/// V0 fixed bounty paid to the caller of Expire() for cleaning up an
/// aged animal. In production this scales with treasury yield via
/// BOUNTY_MULTIPLIER_*, but for testability and v0 deployment we use a
/// constant value denominated in the same yield-units as
/// TEST_YIELD_PER_BLOCK_PER_STAKE.
pub const EXPIRE_BOUNTY_UNITS: u128 = 100;

// ---------- Sanity / overflow protection ----------
/// 1e8 — used for fixed-point math like fire-misha
pub const PRECISION_SMALL: u128 = 100_000_000;
/// 1e18 — used for accumulator math to avoid precision loss
pub const PRECISION: u128 = 1_000_000_000_000_000_000;
