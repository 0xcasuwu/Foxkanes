//! Foxkanes
//!
//! A predator/prey game protocol on alkanes that bonds player capital into
//! FIRE on every mint. Fully autonomous: no admin keys, no governance, no
//! parameter setters. Every variable is either hardcoded forever or computed
//! from on-chain state.
//!
//! Contracts:
//! - `foxkanes-game`: The factory + game loop. Runs the daily lottery, mints
//!   animal NFTs, dispatches taxes, resolves hunts, expires aged animals.
//! - `foxkanes-animal`: Per-NFT receipt token. Stores role (fox|farmer),
//!   birth block, lifespan, accumulated taxes, last-claim block. Authenticates
//!   the game via stored `vault_id` (boiler pattern).
//! - `foxkanes-commitment`: Per-lottery-entry receipt. Stores bond NFT id,
//!   commit block, reveal block, weight. Consumed at reveal. Future-block-hash
//!   commit-reveal randomness.
//! - `foxkanes-zap`: Peripheral router. Converts arbitrary input alkanes to
//!   DIESEL/FIRE LP, calls fire-bonding on player's behalf, forwards the bond
//!   NFT + commitment receipt back. Separable from core game (replaceable
//!   without touching game state) per the 1inch-aggregator pattern.
//! - `foxkanes-support`: Shared math + cellpack helpers.
//! - `foxkanes-constants`: Template IDs, hardcoded forever-constants
//!   (target_fox_ratio = 10%, base_tax_rate = 2000 bps, target_population,
//!   protocol_fee_bps = 100, etc).
//!
//! Ownership model: pure bearer-token. No EOA, no address mapping. Whoever
//! presents the receipt NFT in `incoming_alkanes` is the owner. The game
//! authenticates callers by checking the receipt's AlkaneId against its
//! `register_child` registry.
//!
//! Randomness: commit-reveal against a future block hash 144 blocks out
//! (~24 hours). Commitments cannot grind for favorable seeds because the
//! seed block hash isn't known until reveal time.

#[cfg(test)]
mod tests;
