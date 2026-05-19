//! TDD coverage for foxkanes-game hunts (initiate + resolve).
//!
//! Similar to gameplay_test: full end-to-end hunt-then-resolve flows
//! depend on minting animals first, which depends on the deferred
//! AlkaneId discovery work. The tests in this file focus on the views
//! and error paths that don't require a minted animal.
//!
//! Full hunt lifecycle tests (initiate-against-fox, resolve-success,
//! resolve-failure, aging accumulation, role conversion) land in the
//! #18 integration suite once trace-decoded animal IDs are available.

use crate::tests::helpers::{
    clear_test_environment, create_deployment_block, index_block, parse_packed_u128s, parse_u128,
    simulate_cellpack, AlkaneId, Cellpack, Result,
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
// Total hunts = 0 at fresh game
// =============================================================================

#[wasm_bindgen_test]
fn test_total_hunts_starts_zero() -> Result<()> {
    clear_test_environment();
    let (game, h) = setup_game(880_000, 880_000)?;

    let (resp, _) = simulate_cellpack(
        h as u64,
        Cellpack { target: game.clone(), inputs: vec![26] }, // GetTotalHunts
    )?;
    assert_eq!(parse_u128(&resp.data)?, 0);

    let (resp, _) = simulate_cellpack(
        h as u64,
        Cellpack { target: game.clone(), inputs: vec![27] }, // GetRecentFailedHunts
    )?;
    assert_eq!(parse_u128(&resp.data)?, 0);

    let (resp, _) = simulate_cellpack(
        h as u64,
        Cellpack { target: game.clone(), inputs: vec![28] }, // GetRecentTotalHunts
    )?;
    assert_eq!(parse_u128(&resp.data)?, 0);

    Ok(())
}

// =============================================================================
// GetMaxFoxUnclaimed starts at 0 (no hunts have run)
// =============================================================================

#[wasm_bindgen_test]
fn test_max_fox_unclaimed_starts_zero() -> Result<()> {
    clear_test_environment();
    let (game, h) = setup_game(880_000, 880_000)?;

    let (resp, _) = simulate_cellpack(
        h as u64,
        Cellpack { target: game.clone(), inputs: vec![30] }, // GetMaxFoxUnclaimed
    )?;
    assert_eq!(parse_u128(&resp.data)?, 0);
    Ok(())
}

// =============================================================================
// GetHunt for an unknown hunt_id returns all-zeros (96 bytes)
// =============================================================================

#[wasm_bindgen_test]
fn test_get_hunt_unknown_returns_zeros() -> Result<()> {
    clear_test_environment();
    let (game, h) = setup_game(880_000, 880_000)?;

    let (resp, _) = simulate_cellpack(
        h as u64,
        Cellpack {
            target: game.clone(),
            inputs: vec![25, 999], // GetHunt(hunt_id=999)
        },
    )?;
    let fields = parse_packed_u128s(&resp.data, 6)?;
    // All zeros — unknown hunt id has uninitialized storage.
    assert_eq!(fields, vec![0; 6]);
    Ok(())
}

// =============================================================================
// GetAnimalAging for an unknown animal returns 0
// =============================================================================

#[wasm_bindgen_test]
fn test_animal_aging_unknown_returns_zero() -> Result<()> {
    clear_test_environment();
    let (game, h) = setup_game(880_000, 880_000)?;

    let (resp, _) = simulate_cellpack(
        h as u64,
        Cellpack {
            target: game.clone(),
            inputs: vec![29, 99, 999], // GetAnimalAging(99, 999)
        },
    )?;
    assert_eq!(parse_u128(&resp.data)?, 0);
    Ok(())
}

// =============================================================================
// InitiateHunt against a non-registered animal errors (returns Err in contract).
// Asserted indirectly: total_hunts counter stays at 0 after the failed call.
// =============================================================================

#[wasm_bindgen_test]
fn test_initiate_hunt_unknown_target_no_increment() -> Result<()> {
    clear_test_environment();
    let (game, h) = setup_game(880_000, 880_000)?;

    // Try InitiateHunt against a fake target. No incoming alkanes, no
    // registered party members. The contract returns Err inside the call;
    // index_block swallows the error; counter stays 0.
    use crate::tests::helpers::create_operation_block;
    let blk = create_operation_block(
        h,
        Cellpack {
            target: game.clone(),
            inputs: vec![6u128, 4, 999], // InitiateHunt(target=(4, 999))
        },
        None,
    );
    index_block(&blk, h)?;
    let h = h + 1;

    let (resp, _) = simulate_cellpack(
        h as u64,
        Cellpack { target: game.clone(), inputs: vec![26] }, // GetTotalHunts
    )?;
    assert_eq!(parse_u128(&resp.data)?, 0, "no hunt was registered");
    Ok(())
}

// =============================================================================
// ResolveHunt with unknown hunt_id errors (returns Err). Asserted via
// counter staying unchanged.
// =============================================================================

#[wasm_bindgen_test]
fn test_resolve_hunt_unknown_id_no_op() -> Result<()> {
    clear_test_environment();
    let (game, h) = setup_game(880_000, 880_000)?;

    use crate::tests::helpers::create_operation_block;
    let blk = create_operation_block(
        h,
        Cellpack {
            target: game.clone(),
            inputs: vec![7u128, 99], // ResolveHunt(hunt_id=99)
        },
        None,
    );
    index_block(&blk, h)?;

    // No state changed; total hunts still 0.
    let (resp, _) = simulate_cellpack(
        (h + 1) as u64,
        Cellpack { target: game.clone(), inputs: vec![26] },
    )?;
    assert_eq!(parse_u128(&resp.data)?, 0);
    Ok(())
}
