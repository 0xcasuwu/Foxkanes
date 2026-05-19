//! TDD coverage for foxkanes-game expiration (opcode 8 + view 31/32).
//!
//! Same shape as hunts_test: full lifecycle tests (mint → age past
//! lifespan → expire) need AlkaneId discovery from spawned animals,
//! deferred to #18. The tests here cover the views at fresh state and
//! the error paths (unknown animal, animal not yet old enough — both
//! return Err inside the contract, observed indirectly).

use crate::tests::helpers::{
    clear_test_environment, create_deployment_block, create_operation_block, index_block,
    parse_u128, simulate_cellpack, AlkaneId, Cellpack, Result,
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
// Total expirations = 0 at fresh game
// =============================================================================

#[wasm_bindgen_test]
fn test_total_expirations_starts_zero() -> Result<()> {
    clear_test_environment();
    let (game, h) = setup_game(880_000, 880_000)?;

    let (resp, _) = simulate_cellpack(
        h as u64,
        Cellpack { target: game.clone(), inputs: vec![32] }, // GetTotalExpirations
    )?;
    assert_eq!(parse_u128(&resp.data)?, 0);
    Ok(())
}

// =============================================================================
// GetExpireBlock for an unknown animal returns 0 (registry-guarded)
// =============================================================================

#[wasm_bindgen_test]
fn test_expire_block_unknown_returns_zero() -> Result<()> {
    clear_test_environment();
    let (game, h) = setup_game(880_000, 880_000)?;

    let (resp, _) = simulate_cellpack(
        h as u64,
        Cellpack {
            target: game.clone(),
            inputs: vec![31, 99, 999], // GetExpireBlock(unknown)
        },
    )?;
    assert_eq!(parse_u128(&resp.data)?, 0);
    Ok(())
}

// =============================================================================
// Expire against unknown animal — error, total_expirations stays 0
// =============================================================================

#[wasm_bindgen_test]
fn test_expire_unknown_animal_no_increment() -> Result<()> {
    clear_test_environment();
    let (game, h) = setup_game(880_000, 880_000)?;

    let blk = create_operation_block(
        h,
        Cellpack {
            target: game.clone(),
            inputs: vec![8u128, 99, 999], // Expire(unknown)
        },
        None,
    );
    index_block(&blk, h)?;
    let h = h + 1;

    let (resp, _) = simulate_cellpack(
        h as u64,
        Cellpack { target: game.clone(), inputs: vec![32] },
    )?;
    assert_eq!(parse_u128(&resp.data)?, 0, "no expiration registered");
    Ok(())
}
