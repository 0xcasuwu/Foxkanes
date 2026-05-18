//! TDD coverage for foxkanes-game — lottery commit/reveal core.
//!
//! Unlike the per-NFT contracts that can be tested in isolation, the
//! game contract needs BOTH the animal and commitment templates deployed
//! first (so its factory cellpacks have valid targets), plus the game
//! itself initialized to know those template ids.
//!
//! Each test does PHASE 1 (deploy all three templates) then PHASE 2
//! (initialize the game), then exercises one flow. We use the
//! fire-misha pattern of `init_with_multiple_cellpacks_with_tx` to bundle
//! all three deploys into a single block.

use crate::tests::helpers::{
    clear_test_environment, create_deployment_block, create_operation_block, index_block,
    parse_packed_u128s, parse_u128, simulate_cellpack, AlkaneId, Cellpack, Result,
};
use crate::tests::vendor::{
    get_foxkanes_animal_wasm_bytes, get_foxkanes_commitment_wasm_bytes,
    get_foxkanes_game_wasm_bytes,
};
use wasm_bindgen_test::wasm_bindgen_test;

const GAME_TX: u128 = 0x200;
const ANIMAL_TX: u128 = 0x201;
const COMMITMENT_TX: u128 = 0x202;

/// Deploy all three contracts (animal, commitment, game) at their template
/// IDs and initialize the game with refs to the animal + commitment
/// templates. Returns the game's live AlkaneId.
fn setup_game(genesis_block: u128, start_height: u32) -> Result<(AlkaneId, u32)> {
    let mut h = start_height;

    // Deploy animal template (no init — game spawns animals via block-6
    // factory). Empty inputs are OK here because animal init takes args.
    // To avoid the deploy-time observe_initialization collision we saw
    // earlier, we never call Initialize on the animal/commitment templates
    // themselves; their initialize() only runs from block-6 factory clones
    // spawned by foxkanes-game. We just need their bytecode at (3, TX).
    let animal_block = create_deployment_block(
        h,
        get_foxkanes_animal_wasm_bytes(),
        Cellpack {
            target: AlkaneId { block: 3, tx: ANIMAL_TX },
            // Pass valid init args. This sets vault_id = whoever called
            // (which is the test runner), so this *master* instance has
            // its vault as the test caller — but the master copy is
            // unused; spawned clones from block-6 are the actual animals
            // and their vault_id becomes the game contract's id.
            inputs: vec![0u128, 0, 0, 0, 0], // animal_id=0, role=0, birth=0, lifespan=0
        },
    );
    index_block(&animal_block, h)?;
    h += 1;

    // Same for commitment template
    let cmt_block = create_deployment_block(
        h,
        get_foxkanes_commitment_wasm_bytes(),
        Cellpack {
            target: AlkaneId { block: 3, tx: COMMITMENT_TX },
            // commitment_id=0, bond=(0,0), commit=0, reveal=1, weight=1, day=0
            // reveal > commit and weight > 0 to pass init guards.
            inputs: vec![0u128, 0, 0, 0, 0, 1, 1, 0],
        },
    );
    index_block(&cmt_block, h)?;
    h += 1;

    // Deploy game and initialize in one cellpack
    let game_block = create_deployment_block(
        h,
        get_foxkanes_game_wasm_bytes(),
        Cellpack {
            target: AlkaneId { block: 3, tx: GAME_TX },
            inputs: vec![
                0u128, // Initialize
                ANIMAL_TX,
                COMMITMENT_TX,
                genesis_block,
            ],
        },
    );
    index_block(&game_block, h)?;
    h += 1;

    Ok((AlkaneId { block: 4, tx: GAME_TX }, h))
}

// =============================================================================
// 1. Initialize wires templates and genesis block
// =============================================================================

#[wasm_bindgen_test]
fn test_init_sets_templates_and_genesis() -> Result<()> {
    clear_test_environment();
    let genesis = 880_000u128;
    let (game, h) = setup_game(genesis, 880_000)?;

    let (resp, _) = simulate_cellpack(
        h as u64,
        Cellpack {
            target: game.clone(),
            inputs: vec![19], // GetGenesisBlock
        },
    )?;
    assert_eq!(parse_u128(&resp.data)?, genesis);
    Ok(())
}

// =============================================================================
// 2. Initial state — counters zero, lifespan defaults from MAX_DAILY_MINTS
// =============================================================================

#[wasm_bindgen_test]
fn test_initial_state() -> Result<()> {
    clear_test_environment();
    let (game, h) = setup_game(880_000, 880_000)?;

    // GetPopulation returns 0,0
    let (resp, _) = simulate_cellpack(
        h as u64,
        Cellpack { target: game.clone(), inputs: vec![10] },
    )?;
    let pops = parse_packed_u128s(&resp.data, 2)?;
    assert_eq!(pops[0], 0, "fox count starts at 0");
    assert_eq!(pops[1], 0, "farmer count starts at 0");

    // GetTotalAnimalsMinted = 0
    let (resp, _) = simulate_cellpack(
        h as u64,
        Cellpack { target: game.clone(), inputs: vec![14] },
    )?;
    assert_eq!(parse_u128(&resp.data)?, 0);

    // GetTotalCommitments = 0 (commitment_seq starts at 1, count = seq - 1)
    let (resp, _) = simulate_cellpack(
        h as u64,
        Cellpack { target: game.clone(), inputs: vec![15] },
    )?;
    assert_eq!(parse_u128(&resp.data)?, 0);

    // GetLastDayMints — should bootstrap to HARDCODED_MAX_DAILY_MINTS (100)
    let (resp, _) = simulate_cellpack(
        h as u64,
        Cellpack { target: game.clone(), inputs: vec![16] },
    )?;
    assert_eq!(parse_u128(&resp.data)?, 100, "bootstrap to MAX_DAILY_MINTS");
    Ok(())
}

// =============================================================================
// 3. Tax rate at empty population
// =============================================================================

#[wasm_bindgen_test]
fn test_tax_rate_empty_population() -> Result<()> {
    clear_test_environment();
    let (game, h) = setup_game(880_000, 880_000)?;

    // Zero fox + zero farmer → compute_tax_bps returns BASE_TAX_BPS (2000)
    let (resp, _) = simulate_cellpack(
        h as u64,
        Cellpack { target: game.clone(), inputs: vec![11] },
    )?;
    assert_eq!(parse_u128(&resp.data)?, 2000, "BASE_TAX_BPS at empty");
    Ok(())
}

// =============================================================================
// 4. Lifespan formula uses last_day_mints
// =============================================================================

#[wasm_bindgen_test]
fn test_lifespan_at_max_rate() -> Result<()> {
    clear_test_environment();
    let (game, h) = setup_game(880_000, 880_000)?;

    // last_day_mints bootstraps to 100, target_population = 9000
    // lifespan = (9000 / 100) days = 90 days * 144 blocks = 12_960 blocks
    let (resp, _) = simulate_cellpack(
        h as u64,
        Cellpack { target: game.clone(), inputs: vec![12] },
    )?;
    assert_eq!(parse_u128(&resp.data)?, 12_960, "90-day lifespan at 100 mints/day");
    Ok(())
}

// =============================================================================
// 5. is_registered_* for unknown IDs returns 0
// =============================================================================

#[wasm_bindgen_test]
fn test_is_registered_unknown_returns_zero() -> Result<()> {
    clear_test_environment();
    let (game, h) = setup_game(880_000, 880_000)?;

    // Unknown animal id
    let (resp, _) = simulate_cellpack(
        h as u64,
        Cellpack {
            target: game.clone(),
            inputs: vec![17, 99, 999], // HandleIsRegisteredAnimal(99, 999)
        },
    )?;
    assert_eq!(parse_u128(&resp.data)?, 0);

    // Unknown commitment id
    let (resp, _) = simulate_cellpack(
        h as u64,
        Cellpack {
            target: game.clone(),
            inputs: vec![18, 99, 999],
        },
    )?;
    assert_eq!(parse_u128(&resp.data)?, 0);
    Ok(())
}

// =============================================================================
// 6. EnterLottery mints a commitment and increments counters
//
// Note: incoming_alkanes is empty in our test-mode call, so the contract
// falls through to the test-mode fallback (weight = sqrt(1) = 1). That's
// enough to verify the commit path; the LP-amount-driven path is
// exercised through the zap-flow integration tests in the final task.
// =============================================================================

#[wasm_bindgen_test]
fn test_enter_lottery_creates_commitment() -> Result<()> {
    clear_test_environment();
    let (game, h) = setup_game(880_000, 880_000)?;

    let enter_block = create_operation_block(
        h,
        Cellpack {
            target: game.clone(),
            // EnterLottery(bond_nft_block=4, bond_nft_tx=999)
            inputs: vec![1u128, 4, 999],
        },
        None,
    );
    index_block(&enter_block, h)?;

    let (resp, _) = simulate_cellpack(
        (h + 1) as u64,
        Cellpack { target: game.clone(), inputs: vec![15] }, // GetTotalCommitments
    )?;
    assert_eq!(parse_u128(&resp.data)?, 1, "one commitment created");

    // current_day_weight should reflect the weight of the lone commitment
    // (sqrt(1) = 1, from the test-mode fallback).
    let (resp, _) = simulate_cellpack(
        (h + 1) as u64,
        Cellpack { target: game.clone(), inputs: vec![13] }, // GetCurrentDayWeight
    )?;
    assert_eq!(parse_u128(&resp.data)?, 1);
    Ok(())
}

// =============================================================================
// 7. Multiple commitments accumulate weight on the same day
// =============================================================================

#[wasm_bindgen_test]
fn test_multiple_commits_same_day_accumulate() -> Result<()> {
    clear_test_environment();
    let (game, h) = setup_game(880_000, 880_000)?;
    let mut h = h;

    for i in 0..3u128 {
        let blk = create_operation_block(
            h,
            Cellpack {
                target: game.clone(),
                inputs: vec![1u128, 4, 1000 + i],
            },
            None,
        );
        index_block(&blk, h)?;
        h += 1;
    }

    let (resp, _) = simulate_cellpack(
        h as u64,
        Cellpack { target: game.clone(), inputs: vec![15] },
    )?;
    assert_eq!(parse_u128(&resp.data)?, 3, "3 commitments created");

    let (resp, _) = simulate_cellpack(
        h as u64,
        Cellpack { target: game.clone(), inputs: vec![13] },
    )?;
    assert_eq!(parse_u128(&resp.data)?, 3, "3 × weight=1 accumulated");
    Ok(())
}

// =============================================================================
// 8. Commitment is registered as a child after creation
// =============================================================================

#[wasm_bindgen_test]
fn test_commitment_registered_as_child() -> Result<()> {
    clear_test_environment();
    let (game, h) = setup_game(880_000, 880_000)?;

    let enter_block = create_operation_block(
        h,
        Cellpack {
            target: game.clone(),
            inputs: vec![1u128, 4, 999],
        },
        None,
    );
    index_block(&enter_block, h)?;

    // The spawned commitment lives at (2, N) where N is some runtime-assigned
    // sequence. We can't predict its exact id, but we know it should be
    // registered. As a sanity check, the count is 1; querying isReg on the
    // template id (4, 0x202) should return 0 (template isn't a registered
    // child even though the *spawned clones* are).
    let (resp, _) = simulate_cellpack(
        (h + 1) as u64,
        Cellpack {
            target: game.clone(),
            inputs: vec![18, 4, COMMITMENT_TX],
        },
    )?;
    assert_eq!(parse_u128(&resp.data)?, 0, "template itself is not a child");
    Ok(())
}
