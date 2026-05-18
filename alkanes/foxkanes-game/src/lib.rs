//! foxkanes-game — the factory + game loop.
//!
//! Responsibilities:
//!   1. Daily lottery commit / reveal lifecycle (commit-reveal randomness
//!      against future block hashes; see foxkanes-support::lottery_check
//!      and role_is_fox).
//!   2. Animal NFT factory — spawns foxkanes-animal alkanes via block-6
//!      factory call, registers each as a child for boiler-pattern auth.
//!   3. Tax dispatch — claims by farmers route a portion of vested FIRE
//!      to staked foxes (rate set autonomously by population homeostasis
//!      via foxkanes-support::compute_tax_bps).
//!   4. Hunt orchestration — coordinated hunting parties, success
//!      probability scaled by target fox ripeness, role-conversion on
//!      successful hunt.
//!   5. Expiration — permissionless expire(animal_id) call after lifespan,
//!      pays a bounty to the cleanup caller.
//!   6. TWAP — daily samples of fire-bonding's spot price, used for mint
//!      cost denominated in time-averaged FIRE value.
//!
//! Immutability:
//!   - No admin opcode.
//!   - No upgrade path.
//!   - No parameter setter.
//!   - All parameters either hardcoded in foxkanes-constants or computed
//!     from on-chain state via foxkanes-support.
//!
//! TODO: full implementation in next pass. Opcodes 0/1/2/3/4/5/6/7/... will
//! cover Initialize, EnterLottery, Reveal, Stake, ClaimSafe, ClaimRisky,
//! InitiateHunt, ResolveHunt, Expire, plus view opcodes (10–30) for state
//! queries.

use alkanes_runtime::{declare_alkane, message::MessageDispatch, runtime::AlkaneResponder};
use alkanes_support::response::CallResponse;
use anyhow::Result;

#[derive(Default)]
pub struct FoxkanesGame(());

impl AlkaneResponder for FoxkanesGame {}

#[derive(MessageDispatch)]
enum FoxkanesGameMessage {
    /// One-shot initialization. Wires the references to FIRE contracts +
    /// the animal/commitment template ids.
    #[opcode(0)]
    Initialize {
        animal_template: u128,
        commitment_template: u128,
        genesis_block: u128,
    },

    /// Population view: returns (fox_count, farmer_count) as 2 × u128 LE.
    #[opcode(10)]
    #[returns(Vec<u8>)]
    GetPopulation,

    /// Current effective tax rate in bps (computed from current population).
    #[opcode(11)]
    #[returns(u128)]
    GetCurrentTaxBps,

    /// Current effective lifespan in blocks (computed from current daily mint rate).
    #[opcode(12)]
    #[returns(u128)]
    GetCurrentLifespanBlocks,
}

impl FoxkanesGame {
    fn initialize(
        &self,
        _animal_template: u128,
        _commitment_template: u128,
        _genesis_block: u128,
    ) -> Result<CallResponse> {
        Ok(CallResponse::default())
    }

    fn get_population(&self) -> Result<CallResponse> {
        Ok(CallResponse::default())
    }

    fn get_current_tax_bps(&self) -> Result<CallResponse> {
        Ok(CallResponse::default())
    }

    fn get_current_lifespan_blocks(&self) -> Result<CallResponse> {
        Ok(CallResponse::default())
    }
}

declare_alkane! {
    impl AlkaneResponder for FoxkanesGame {
        type Message = FoxkanesGameMessage;
    }
}
