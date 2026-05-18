//! Foxkanes
//!
//! A predator/prey game protocol on alkanes that bonds player capital into
//! FIRE on every mint. Fully autonomous: no admin keys, no governance, no
//! parameter setters. Every variable is either hardcoded forever or computed
//! from on-chain state.
//!
//! Contracts:
//! - `foxkanes-game`: factory + game loop
//! - `foxkanes-animal`: per-NFT receipt token
//! - `foxkanes-commitment`: per-lottery-entry receipt
//! - `foxkanes-zap`: peripheral router (replaceable)
//! - `foxkanes-support`: pure autonomous-parameter formulas
//! - `foxkanes-constants`: hardcoded forever-values
//!
//! Tests use the alkanes test harness pattern (mirrored from
//! boiler/hude/review): build.rs cross-compiles every alkane crate to
//! wasm, hex-encodes the bytes, and emits `src/tests/std/*_build.rs`
//! files. Tests then deploy contracts in PHASE 1 of their setup via
//! `alkane_helpers::init_with_multiple_cellpacks_with_tx`.

#[cfg(test)]
mod tests;
