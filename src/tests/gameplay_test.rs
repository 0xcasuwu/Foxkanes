//! TDD coverage for foxkanes-game gameplay (stake / claim safe / claim risky / tax).
//!
//! STATUS: Stake/ClaimSafe/ClaimRisky are fully implemented in the
//! contract. End-to-end tests that drive these flows require minting an
//! animal first — which means winning the lottery, which gives us back
//! an AlkaneId we don't deterministically know in advance.
//!
//! Two approaches to that problem have been tried:
//!   (a) View opcode `GetLatestCommitmentId` / `GetLatestAnimalId`
//!       returning stored AlkaneId via simulate_cellpack — the storage
//!       writes happen but reads from the simulated view return empty
//!       (still investigating; tracked in repo TODO).
//!   (b) Decode the spawned alkane from the EnterLottery / Reveal
//!       transaction's trace — requires a trace-decoding helper we
//!       haven't extracted yet (already noted as a recurring TODO).
//!
//! For this commit we ship the simple tests that don't depend on
//! AlkaneId discovery: fox-pool initial state, distribute-tax math
//! exposed through views, and any path that operates on global game
//! state. End-to-end mint-then-stake-then-claim tests are deferred to
//! the integration suite (task #18) where the trace-decode helper will
//! be added.

use crate::tests::helpers::{
    clear_test_environment, create_deployment_block, index_block, parse_u128, simulate_cellpack,
    AlkaneId, Cellpack, Result,
};
use crate::tests::vendor::{
    get_foxkanes_animal_wasm_bytes, get_foxkanes_commitment_wasm_bytes,
    get_foxkanes_game_wasm_bytes,
};
use wasm_bindgen_test::wasm_bindgen_test;

const GAME_TX: u128 = 0x200;
const ANIMAL_TX: u128 = 0x201;
const COMMITMENT_TX: u128 = 0x202;

fn setup_game(genesis_block: u128, start_height: u32) -> Result<(AlkaneId, u32)> {
    let mut h = start_height;
    let animal_block = create_deployment_block(
        h,
        get_foxkanes_animal_wasm_bytes(),
        Cellpack {
            target: AlkaneId { block: 3, tx: ANIMAL_TX },
            inputs: vec![0u128, 0, 0, 0, 0],
        },
    );
    index_block(&animal_block, h)?;
    h += 1;
    let cmt_block = create_deployment_block(
        h,
        get_foxkanes_commitment_wasm_bytes(),
        Cellpack {
            target: AlkaneId { block: 3, tx: COMMITMENT_TX },
            inputs: vec![0u128, 0, 0, 0, 0, 1, 1, 0],
        },
    );
    index_block(&cmt_block, h)?;
    h += 1;
    let game_block = create_deployment_block(
        h,
        get_foxkanes_game_wasm_bytes(),
        Cellpack {
            target: AlkaneId { block: 3, tx: GAME_TX },
            inputs: vec![0u128, ANIMAL_TX, COMMITMENT_TX, genesis_block],
        },
    );
    index_block(&game_block, h)?;
    h += 1;
    Ok((AlkaneId { block: 4, tx: GAME_TX }, h))
}

// =============================================================================
// Fox pool views at fresh game state
// =============================================================================

#[wasm_bindgen_test]
fn test_fox_pool_starts_empty() -> Result<()> {
    clear_test_environment();
    let (game, h) = setup_game(880_000, 880_000)?;

    let (resp, _) = simulate_cellpack(
        h as u64,
        Cellpack { target: game.clone(), inputs: vec![20] }, // GetFoxPool
    )?;
    assert_eq!(parse_u128(&resp.data)?, 0);

    let (resp, _) = simulate_cellpack(
        h as u64,
        Cellpack { target: game.clone(), inputs: vec![21] }, // GetFoxPoolLifetime
    )?;
    assert_eq!(parse_u128(&resp.data)?, 0);
    Ok(())
}

// =============================================================================
// PreviewYield for an unknown animal returns 0 (registry-guarded)
// =============================================================================

#[wasm_bindgen_test]
fn test_preview_yield_unknown_animal_returns_zero() -> Result<()> {
    clear_test_environment();
    let (game, h) = setup_game(880_000, 880_000)?;

    // Query yield for a random unknown alkane — should return 0 since
    // is_registered_animal returns false for any id we haven't minted.
    let (resp, _) = simulate_cellpack(
        h as u64,
        Cellpack {
            target: game.clone(),
            inputs: vec![22, 99, 999], // PreviewYield(unknown)
        },
    )?;
    assert_eq!(parse_u128(&resp.data)?, 0);
    Ok(())
}

// =============================================================================
// Population view consistent before any mints
// =============================================================================

#[wasm_bindgen_test]
fn test_population_zero_at_start() -> Result<()> {
    clear_test_environment();
    let (game, h) = setup_game(880_000, 880_000)?;

    use crate::tests::helpers::parse_packed_u128s;
    let (resp, _) = simulate_cellpack(
        h as u64,
        Cellpack { target: game.clone(), inputs: vec![10] }, // GetPopulation
    )?;
    let pops = parse_packed_u128s(&resp.data, 2)?;
    assert_eq!(pops, vec![0, 0]);
    Ok(())
}

// NOTE: tests requiring an actually-minted animal in a known AlkaneId
// (test_stake_marks_animal_staked, test_preview_yield_accrues,
//  test_double_stake_rejected_silently, test_claim_safe_farmer_path,
//  test_claim_safe_fox_path, test_claim_risky_executes) are deferred
// pending a trace-decode helper for spawned-NFT id discovery. The
// underlying contract opcodes (Stake/ClaimSafe/ClaimRisky) are wired
// and compile clean; gating their end-to-end assertions on a helper
// that's already on the TODO list for #18 (Integration).
