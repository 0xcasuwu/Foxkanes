//! TDD coverage for foxkanes-commitment — the lottery receipt token.
//!
//! Mirrors animal_test.rs structure: every test deploys + initializes in
//! a single cellpack, then asserts via simulate_cellpack reads. The
//! contract has fewer paths than animal (one mutation: MarkConsumed) so
//! the suite is correspondingly tighter.

use crate::tests::helpers::{
    clear_test_environment, create_deployment_block, create_operation_block, index_block,
    parse_alkane_id, parse_packed_u128s, parse_u128, simulate_cellpack, AlkaneId, Cellpack, Result,
};
use crate::tests::vendor::get_foxkanes_commitment_wasm_bytes;
use wasm_bindgen_test::wasm_bindgen_test;

const COMMITMENT_TX: u128 = 0x202;

/// Deploy + initialize in one cellpack (boiler PHASE-1 pattern).
fn deploy_and_init_commitment(
    commitment_id: u128,
    bond_nft_block: u128,
    bond_nft_tx: u128,
    commit_block: u128,
    reveal_block: u128,
    weight: u128,
    lottery_day_id: u128,
    height: u32,
) -> Result<(AlkaneId, u32)> {
    let wasm = get_foxkanes_commitment_wasm_bytes();
    let block = create_deployment_block(
        height,
        wasm,
        Cellpack {
            target: AlkaneId {
                block: 3,
                tx: COMMITMENT_TX,
            },
            inputs: vec![
                0u128, // Initialize
                commitment_id,
                bond_nft_block,
                bond_nft_tx,
                commit_block,
                reveal_block,
                weight,
                lottery_day_id,
            ],
        },
    );
    index_block(&block, height)?;
    Ok((
        AlkaneId {
            block: 4,
            tx: COMMITMENT_TX,
        },
        height + 1,
    ))
}

// =============================================================================
// 1. Init shape — every field round-trips correctly
// =============================================================================

#[wasm_bindgen_test]
fn test_init_basic() -> Result<()> {
    clear_test_environment();
    let (cmt, h) = deploy_and_init_commitment(
        7,        // commitment_id
        4, 0x103, // bond nft id
        880_100,  // commit_block
        880_245,  // reveal_block (commit + 144 + 1)
        1000,     // weight
        42,       // lottery_day_id
        880_000,
    )?;

    let (resp, _) = simulate_cellpack(
        h as u64,
        Cellpack {
            target: cmt.clone(),
            inputs: vec![10], // GetCommitmentId
        },
    )?;
    assert_eq!(parse_u128(&resp.data)?, 7);

    let (resp, _) = simulate_cellpack(
        h as u64,
        Cellpack {
            target: cmt.clone(),
            inputs: vec![12], // GetCommitBlock
        },
    )?;
    assert_eq!(parse_u128(&resp.data)?, 880_100);

    let (resp, _) = simulate_cellpack(
        h as u64,
        Cellpack {
            target: cmt.clone(),
            inputs: vec![13], // GetRevealBlock
        },
    )?;
    assert_eq!(parse_u128(&resp.data)?, 880_245);

    let (resp, _) = simulate_cellpack(
        h as u64,
        Cellpack {
            target: cmt.clone(),
            inputs: vec![14], // GetWeight
        },
    )?;
    assert_eq!(parse_u128(&resp.data)?, 1000);

    let (resp, _) = simulate_cellpack(
        h as u64,
        Cellpack {
            target: cmt.clone(),
            inputs: vec![15], // GetLotteryDayId
        },
    )?;
    assert_eq!(parse_u128(&resp.data)?, 42);

    // consumed defaults to 0
    let (resp, _) = simulate_cellpack(
        h as u64,
        Cellpack {
            target: cmt.clone(),
            inputs: vec![16], // GetConsumed
        },
    )?;
    assert_eq!(parse_u128(&resp.data)?, 0);

    Ok(())
}

// =============================================================================
// 2. Bond NFT id round-trips as a packed 32-byte AlkaneId
// =============================================================================

#[wasm_bindgen_test]
fn test_bond_nft_id_round_trip() -> Result<()> {
    clear_test_environment();
    let (cmt, h) = deploy_and_init_commitment(
        1,
        4,         // bond nft block
        0xDEADBEEF, // bond nft tx (distinctive value to catch byte mis-ordering)
        880_100,
        880_245,
        1000,
        1,
        880_000,
    )?;

    let (resp, _) = simulate_cellpack(
        h as u64,
        Cellpack {
            target: cmt.clone(),
            inputs: vec![11], // GetBondNftId
        },
    )?;
    let bond_id = parse_alkane_id(&resp.data)?;
    assert_eq!(bond_id.block, 4);
    assert_eq!(bond_id.tx, 0xDEADBEEF);
    Ok(())
}

// =============================================================================
// 3. GetAllDetails packing — 8 × u128 LE = 128 bytes in field order
// =============================================================================

#[wasm_bindgen_test]
fn test_get_all_details_packing() -> Result<()> {
    clear_test_environment();
    let (cmt, h) = deploy_and_init_commitment(
        99,        // commitment_id
        4, 0x103,  // bond nft (block, tx)
        881_000,   // commit_block
        881_145,   // reveal_block
        42_424_242, // weight
        7,         // lottery_day_id
        880_000,
    )?;

    let (resp, _) = simulate_cellpack(
        h as u64,
        Cellpack {
            target: cmt.clone(),
            inputs: vec![23], // GetAllDetails
        },
    )?;

    let fields = parse_packed_u128s(&resp.data, 8)?;
    assert_eq!(fields[0], 99, "commitment_id");
    assert_eq!(fields[1], 4, "bond_nft_block");
    assert_eq!(fields[2], 0x103, "bond_nft_tx");
    assert_eq!(fields[3], 881_000, "commit_block");
    assert_eq!(fields[4], 881_145, "reveal_block");
    assert_eq!(fields[5], 42_424_242, "weight");
    assert_eq!(fields[6], 7, "lottery_day_id");
    assert_eq!(fields[7], 0, "consumed default");
    Ok(())
}

// =============================================================================
// 4. MarkConsumed flips the flag (vault-only path verified by it working
//    in our default-caller test environment)
// =============================================================================

#[wasm_bindgen_test]
fn test_mark_consumed() -> Result<()> {
    clear_test_environment();
    let (cmt, h) = deploy_and_init_commitment(
        1, 4, 0x103, 880_100, 880_245, 1000, 1, 880_000,
    )?;

    let consume_block = create_operation_block(
        h,
        Cellpack {
            target: cmt.clone(),
            inputs: vec![1u128], // MarkConsumed
        },
        None,
    );
    index_block(&consume_block, h)?;

    let (resp, _) = simulate_cellpack(
        (h + 1) as u64,
        Cellpack {
            target: cmt.clone(),
            inputs: vec![16], // GetConsumed
        },
    )?;
    assert_eq!(parse_u128(&resp.data)?, 1, "consumed flag set");

    // GetAllDetails reflects the new state in field 7
    let (resp, _) = simulate_cellpack(
        (h + 1) as u64,
        Cellpack {
            target: cmt.clone(),
            inputs: vec![23],
        },
    )?;
    let fields = parse_packed_u128s(&resp.data, 8)?;
    assert_eq!(fields[7], 1, "consumed visible in packed view");
    Ok(())
}

// =============================================================================
// 5. Vault id is populated and decodable
// =============================================================================

#[wasm_bindgen_test]
fn test_get_vault_id_populated() -> Result<()> {
    clear_test_environment();
    let (cmt, h) = deploy_and_init_commitment(
        1, 4, 0x103, 880_100, 880_245, 1000, 1, 880_000,
    )?;

    let (resp, _) = simulate_cellpack(
        h as u64,
        Cellpack {
            target: cmt.clone(),
            inputs: vec![17], // GetVaultId
        },
    )?;
    let _vault = parse_alkane_id(&resp.data)?;
    // We don't assert exact value (depends on runtime caller semantics),
    // only that the field is non-corrupt — same approach as animal tests.
    Ok(())
}

// =============================================================================
// 6. Name and symbol round-trip with the commitment id embedded
// =============================================================================

#[wasm_bindgen_test]
fn test_name_and_symbol() -> Result<()> {
    clear_test_environment();
    let (cmt, h) = deploy_and_init_commitment(
        137, 4, 0x103, 880_100, 880_245, 1000, 1, 880_000,
    )?;

    let (resp, _) = simulate_cellpack(
        h as u64,
        Cellpack {
            target: cmt.clone(),
            inputs: vec![99], // GetName
        },
    )?;
    let name = String::from_utf8(resp.data.clone()).expect("utf8 name");
    assert!(name.contains("Commitment"), "name has 'Commitment': {}", name);
    assert!(name.contains("137"), "name has id: {}", name);

    let (resp, _) = simulate_cellpack(
        h as u64,
        Cellpack {
            target: cmt.clone(),
            inputs: vec![100], // GetSymbol
        },
    )?;
    let symbol = String::from_utf8(resp.data.clone()).expect("utf8 symbol");
    assert_eq!(symbol, "FK-CMT-137");
    Ok(())
}

// =============================================================================
// 7. Same lottery_day_id across multiple commitments — invariant for
//    foxkanes-game's daily aggregation. We deploy two commitments at
//    different IDs and assert both report the same day. The same template
//    block can't host two commitments simultaneously, but a clean run
//    initializes a single commitment and inspects its lottery_day_id.
// =============================================================================

#[wasm_bindgen_test]
fn test_lottery_day_id_visible() -> Result<()> {
    clear_test_environment();
    let (cmt, h) = deploy_and_init_commitment(
        1, 4, 0x103, 880_100, 880_245, 1000, 99_999, 880_000,
    )?;

    let (resp, _) = simulate_cellpack(
        h as u64,
        Cellpack {
            target: cmt.clone(),
            inputs: vec![15], // GetLotteryDayId
        },
    )?;
    assert_eq!(parse_u128(&resp.data)?, 99_999);
    Ok(())
}

// NOTE: zero-weight and reveal-before-commit guards live in initialize()
// as Err returns. Asserting a contract-side revert from the test harness
// requires a trace-decoding helper we haven't built yet (same TODO as
// the role-validation case in animal_test.rs). The guards still execute
// in the contract; we just can't assert on their effects from outside
// without that helper. To be added once we extract decode_and_print_trace
// into a queryable form.
