//! foxkanes-animal — per-NFT receipt token (boiler/alk4626 pattern).
//!
//! Each animal is a freshly-minted alkane (deployed via block-6 factory call
//! from foxkanes-game). The animal NFT carries the player's game position
//! state: role (fox|farmer), birth block, lifespan_blocks, accumulated
//! unclaimed taxes, last claim block, hunt_in_progress flag.
//!
//! Auth pattern:
//!   - At Initialize, the animal stores `vault_id = context.caller`, which
//!     becomes its sole authority for state mutations (per the alk4626
//!     pattern in /reference/boiler/alkanes/alk4626-position-token).
//!   - Read opcodes are open (anyone can query GetAllDetails).
//!   - Write opcodes (SetClaimedBlock, SetAccumulatedTaxes, MarkDead,
//!     ConvertToFox, SetHuntInProgress) reject any caller other than
//!     `vault_id`.
//!   - Bearer-token transferability: whoever holds the 1 unit of this
//!     alkane is the "owner" and can present it to foxkanes-game to claim
//!     rewards, initiate hunts, etc.
//!
//! TODO: full implementation in next pass. This stub establishes the
//! crate layout and dependency graph.

use alkanes_runtime::{declare_alkane, message::MessageDispatch, runtime::AlkaneResponder};
use alkanes_support::response::CallResponse;
use anyhow::Result;

#[derive(Default)]
pub struct FoxkanesAnimal(());

impl AlkaneResponder for FoxkanesAnimal {}

#[derive(MessageDispatch)]
enum FoxkanesAnimalMessage {
    #[opcode(0)]
    Initialize {
        animal_id: u128,
        role: u128,           // 0 = farmer, 1 = fox
        birth_block: u128,
        lifespan_blocks: u128,
    },

    /// Returns (animal_id, role, birth_block, lifespan_blocks, accumulated_taxes, last_claim_block, hunt_in_progress) as 7 × u128 LE.
    #[opcode(23)]
    #[returns(Vec<u8>)]
    GetAllDetails,
}

impl FoxkanesAnimal {
    fn initialize(
        &self,
        _animal_id: u128,
        _role: u128,
        _birth_block: u128,
        _lifespan_blocks: u128,
    ) -> Result<CallResponse> {
        // TODO: implement
        Ok(CallResponse::default())
    }

    fn get_all_details(&self) -> Result<CallResponse> {
        // TODO: implement
        Ok(CallResponse::default())
    }
}

declare_alkane! {
    impl AlkaneResponder for FoxkanesAnimal {
        type Message = FoxkanesAnimalMessage;
    }
}
