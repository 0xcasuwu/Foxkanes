//! foxkanes-commitment — per-lottery-entry receipt.
//!
//! When a player commits to the daily lottery, foxkanes-game mints one of
//! these. The commitment NFT carries: bond_nft_id (the FIRE bond the player
//! got from their pre-bonded LP), commit_block, reveal_block, weight
//! (sqrt of LP committed), and the lottery_day_id.
//!
//! At reveal time (reveal_block + 1), anyone presents the commitment to
//! foxkanes-game which reads the block hash at reveal_block, derives a
//! seed, and either:
//!   - Mints an animal NFT to the commitment's bearer (winner path), or
//!   - Returns the bond NFT to the commitment's bearer (loser path).
//! Either way the commitment is burned in the same call.
//!
//! Bearer-token: whoever holds the commitment at reveal time gets the
//! outcome. Transferable on a secondary market while open.
//!
//! TODO: full implementation in next pass.

use alkanes_runtime::{declare_alkane, message::MessageDispatch, runtime::AlkaneResponder};
use alkanes_support::response::CallResponse;
use anyhow::Result;
use metashrew_support::compat::to_arraybuffer_layout;

#[derive(Default)]
pub struct FoxkanesCommitment(());

impl AlkaneResponder for FoxkanesCommitment {}

#[derive(MessageDispatch)]
enum FoxkanesCommitmentMessage {
    #[opcode(0)]
    Initialize {
        commitment_id: u128,
        bond_nft_block: u128,
        bond_nft_tx: u128,
        commit_block: u128,
        reveal_block: u128,
        weight: u128,
        lottery_day_id: u128,
    },

    /// Returns (commitment_id, bond_nft_block, bond_nft_tx, commit_block, reveal_block, weight, lottery_day_id) as 7 × u128 LE.
    #[opcode(23)]
    #[returns(Vec<u8>)]
    GetAllDetails,
}

impl FoxkanesCommitment {
    fn initialize(
        &self,
        _commitment_id: u128,
        _bond_nft_block: u128,
        _bond_nft_tx: u128,
        _commit_block: u128,
        _reveal_block: u128,
        _weight: u128,
        _lottery_day_id: u128,
    ) -> Result<CallResponse> {
        Ok(CallResponse::default())
    }

    fn get_all_details(&self) -> Result<CallResponse> {
        Ok(CallResponse::default())
    }
}

declare_alkane! {
    impl AlkaneResponder for FoxkanesCommitment {
        type Message = FoxkanesCommitmentMessage;
    }
}
