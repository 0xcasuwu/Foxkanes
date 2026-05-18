# Foxkanes

A predator/prey game protocol on alkanes that bonds player capital into FIRE
on every mint. Fully autonomous: no admin keys, no governance, no parameter
setters. Every variable is either hardcoded forever or computed from on-chain
state.

## Status

**Scaffolded.** Workspace, constants, support math, and four contract crates
(`foxkanes-game`, `foxkanes-animal`, `foxkanes-commitment`, `foxkanes-zap`)
are wired up. Contract bodies are stubs; pure-math formulas in
`foxkanes-support` are implemented and unit-tested.

## Design summary

| Concern | Decision |
|---|---|
| Mint mechanic | Daily VRF lottery, ≤100 winners (hardcoded), capacity-gated by FIRE bonding |
| Mint cost | LP committed to lottery; 1% retained by Foxkanes treasury, 99% bonded to FIRE |
| Lottery weight | `sqrt(LP_committed)` (quadratic discount) |
| Randomness | Commit-reveal against future block hash at commit + ~24h + 1 |
| Role assignment | 90% farmer / 10% fox via VRF at reveal |
| Lifespan | Autonomous: `target_pop / mints_per_day × DAY` (≈90 days at steady state) |
| Tax rate (safe-claim) | Autonomous: homeostatic to maintain 10/90 fox/farmer ratio |
| Tax rate (risky-claim) | 50/50 keep-all or lose-all |
| Hunt mechanic | Coordinated parties (5–10 sheep); success probability scales with target fox's unclaimed taxes |
| Hunt outcome | Success: party shares fox's taxes, one member converts to fox, fox NFT burns. Failure: party members age. |
| Aging penalty | Autonomous: scales with recent failed-hunt rate |
| Pity system | None |
| Zap | Separate contract (replaceable; AMM-routing complexity isolated from game state) |
| Treasury fee | 1% of mint LP (hardcoded), retained as protocol-owned-LP |
| Admin | None |
| Upgrade path | None |
| Governance | None |

## Contracts

### `alkanes/foxkanes-game`
The factory + game loop. Runs lottery commit/reveal, mints animal NFTs,
dispatches taxes, resolves hunts, expires aged animals. Holds the
registry of valid animal/commitment NFTs (boiler `register_child` pattern).
Holds the protocol treasury LP.

### `alkanes/foxkanes-animal`
Per-NFT receipt token, one alkane per animal. Carries role (fox|farmer),
birth block, lifespan, accumulated unclaimed taxes, last-claim block,
hunt_in_progress flag. Authenticates the game via stored `vault_id`.
Bearer-token transferability: whoever holds the 1 unit owns the position.

### `alkanes/foxkanes-commitment`
Per-lottery-entry receipt. Carries bond_nft_id, commit_block, reveal_block,
weight, lottery_day_id. Consumed at reveal; either upgraded to an animal
NFT (winner path) or refunds the bond NFT (loser path).

### `alkanes/foxkanes-zap`
Peripheral router. Converts arbitrary input alkanes to DIESEL/FIRE LP,
calls fire-bonding on player's behalf, enters lottery, returns bond NFT
+ commitment receipt to the caller. Replaceable without affecting game
state — game accepts only canonical LP.

## Crates

### `crates/foxkanes-constants`
All hardcoded forever-values. Template IDs (0x200–0x203), block durations,
game-economic constants (target population, ratios, tax rates, hunt params,
treasury fee).

### `crates/foxkanes-support`
Pure math helpers for the autonomous-parameter formulas. Each function
takes observable on-chain state as input and returns the current effective
parameter value. No I/O. WASM-safe (no_std, no floats). Host-side unit
tests in `#[cfg(test)]`.

## Build

```bash
# Pure-math tests run on the host
cargo test -p foxkanes-support

# WASM build (for deploy)
cargo build --target wasm32-unknown-unknown --release
# Requires Homebrew LLVM when cross-compiling secp256k1-sys:
#   export CC_wasm32_unknown_unknown=/usr/local/opt/llvm/bin/clang
#   export AR_wasm32_unknown_unknown=/usr/local/opt/llvm/bin/llvm-ar
```

## Integration with FIRE

Foxkanes does not modify FIRE; it acts as a third-party caller. The
bearer-token nature of FIRE bond NFTs (per fire-bonding's
`authenticate_bond_position`, which only checks registered-child status
and incoming-alkanes presence) makes a "bond on player's behalf" flow
trivial: Foxkanes calls fire-bonding Op 1 with LP, receives the bond NFT
in its response, and forwards it to the player in the same transaction.

The game's distribution stream is the standard FIRE staking yield earned
on Foxkanes' protocol-owned LP (compounded by re-bonding accumulated fees).
There is no FIRE-side privilege or whitelist. Anyone could build a
Foxkanes-equivalent without our cooperation; we compete on game design,
not on access.

## Non-goals

- No directional trading exposure for players.
- No leverage, no perp-like primitives.
- No fiat on/off ramp; everything is LP-denominated.
- No mutable parameters under any conditions.
