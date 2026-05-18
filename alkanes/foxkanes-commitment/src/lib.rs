//! foxkanes-commitment — per-lottery-entry receipt token.
//!
//! Minted by foxkanes-game when a player commits LP into the daily lottery.
//! Carries the data needed at reveal time:
//!   - commitment_id: sequence number per lottery day
//!   - bond_nft_id (block, tx): the FIRE bond NFT the player got from
//!     pre-bonding their LP into FIRE
//!   - commit_block: when the commitment was made
//!   - reveal_block: when the lottery resolves (commit + ~24h + 1)
//!   - weight: sqrt(LP_committed), used for proportional win probability
//!   - lottery_day_id: bucket id for the day; commitments accumulate weight
//!     into this bucket
//!
//! At reveal time, anyone presents the commitment NFT to foxkanes-game.
//! Game reads the block hash at `reveal_block`, derives a seed, decides
//! win/lose, and either mints an animal (consuming the commitment) or
//! refunds the bond NFT (also consuming the commitment). The bearer holds
//! all rights; transferability between commit and reveal is intentional.
//!
//! Auth model mirrors foxkanes-animal: vault stored at init from
//! `context.caller`, only_vault gates the single mutation opcode
//! (MarkConsumed), reads are open. Bearer-token: 1 unit minted at init.

use alkanes_runtime::{
    declare_alkane, message::MessageDispatch, runtime::AlkaneResponder, storage::StoragePointer,
};
use alkanes_support::{
    id::AlkaneId,
    parcel::AlkaneTransfer,
    response::CallResponse,
};
use anyhow::{anyhow, Result};
use metashrew_support::compat::to_arraybuffer_layout;
use metashrew_support::index_pointer::KeyValuePointer;
use std::sync::Arc;

#[derive(Default)]
pub struct FoxkanesCommitment(());

impl AlkaneResponder for FoxkanesCommitment {}

#[derive(MessageDispatch)]
enum FoxkanesCommitmentMessage {
    /// One-shot init. `context.caller` becomes the vault_id. Mints 1 unit
    /// returned to the caller; the game then forwards it to the player.
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

    /// Vault-only: mark this commitment as consumed during reveal. After
    /// this, get_consumed returns 1 — the game uses this to enforce that
    /// a commitment can only be revealed once.
    #[opcode(1)]
    MarkConsumed,

    // ── Open view opcodes ────────────────────────────────────────

    #[opcode(10)]
    #[returns(u128)]
    GetCommitmentId,

    #[opcode(11)]
    #[returns(AlkaneId)]
    GetBondNftId,

    #[opcode(12)]
    #[returns(u128)]
    GetCommitBlock,

    #[opcode(13)]
    #[returns(u128)]
    GetRevealBlock,

    #[opcode(14)]
    #[returns(u128)]
    GetWeight,

    #[opcode(15)]
    #[returns(u128)]
    GetLotteryDayId,

    #[opcode(16)]
    #[returns(u128)]
    GetConsumed,

    #[opcode(17)]
    #[returns(AlkaneId)]
    GetVaultId,

    /// Packed view: 8 × u128 LE = 128 bytes
    /// [commitment_id, bond_nft_block, bond_nft_tx, commit_block,
    ///  reveal_block, weight, lottery_day_id, consumed]
    #[opcode(23)]
    #[returns(Vec<u8>)]
    GetAllDetails,

    #[opcode(99)]
    #[returns(String)]
    GetName,

    #[opcode(100)]
    #[returns(String)]
    GetSymbol,
}

impl FoxkanesCommitment {
    // ── Storage pointers ─────────────────────────────────────────

    fn vault_id_pointer(&self) -> StoragePointer {
        StoragePointer::from_keyword("/vault_id")
    }
    fn commitment_id_pointer(&self) -> StoragePointer {
        StoragePointer::from_keyword("/commitment_id")
    }
    fn bond_nft_id_pointer(&self) -> StoragePointer {
        StoragePointer::from_keyword("/bond_nft_id")
    }
    fn commit_block_pointer(&self) -> StoragePointer {
        StoragePointer::from_keyword("/commit_block")
    }
    fn reveal_block_pointer(&self) -> StoragePointer {
        StoragePointer::from_keyword("/reveal_block")
    }
    fn weight_pointer(&self) -> StoragePointer {
        StoragePointer::from_keyword("/weight")
    }
    fn lottery_day_id_pointer(&self) -> StoragePointer {
        StoragePointer::from_keyword("/lottery_day_id")
    }
    fn consumed_pointer(&self) -> StoragePointer {
        StoragePointer::from_keyword("/consumed")
    }
    fn name_pointer(&self) -> StoragePointer {
        StoragePointer::from_keyword("/name")
    }
    fn symbol_pointer(&self) -> StoragePointer {
        StoragePointer::from_keyword("/symbol")
    }

    // ── Field readers ────────────────────────────────────────────

    fn vault_id(&self) -> AlkaneId {
        let data = self.vault_id_pointer().get();
        if data.is_empty() {
            return AlkaneId::default();
        }
        AlkaneId::try_from(data.as_slice().to_vec()).unwrap_or_default()
    }
    fn commitment_id(&self) -> u128 {
        self.commitment_id_pointer().get_value::<u128>()
    }
    fn bond_nft_id(&self) -> AlkaneId {
        let data = self.bond_nft_id_pointer().get();
        if data.is_empty() {
            return AlkaneId::default();
        }
        AlkaneId::try_from(data.as_slice().to_vec()).unwrap_or_default()
    }
    fn commit_block(&self) -> u128 {
        self.commit_block_pointer().get_value::<u128>()
    }
    fn reveal_block(&self) -> u128 {
        self.reveal_block_pointer().get_value::<u128>()
    }
    fn weight(&self) -> u128 {
        self.weight_pointer().get_value::<u128>()
    }
    fn lottery_day_id(&self) -> u128 {
        self.lottery_day_id_pointer().get_value::<u128>()
    }
    fn consumed(&self) -> u128 {
        self.consumed_pointer().get_value::<u128>()
    }

    // ── Auth ─────────────────────────────────────────────────────

    fn only_vault(&self) -> Result<()> {
        let context = self.context()?;
        let vault = self.vault_id();
        if context.caller != vault {
            return Err(anyhow!(
                "only vault can call this (caller={:?}, vault={:?})",
                context.caller,
                vault
            ));
        }
        Ok(())
    }

    // ── Handlers ─────────────────────────────────────────────────

    fn initialize(
        &self,
        commitment_id: u128,
        bond_nft_block: u128,
        bond_nft_tx: u128,
        commit_block: u128,
        reveal_block: u128,
        weight: u128,
        lottery_day_id: u128,
    ) -> Result<CallResponse> {
        let context = self.context()?;
        self.observe_initialization()?;

        // Sanity: reveal must be strictly after commit. Catches the
        // (commit=N, reveal=N) misuse where the game forgot to add the
        // delay; this is a forever-immutable check.
        if reveal_block <= commit_block {
            return Err(anyhow!(
                "reveal_block ({}) must be > commit_block ({})",
                reveal_block,
                commit_block
            ));
        }
        // Sanity: weight must be non-zero. A zero-weight commitment has
        // zero win probability and would be unproductive to mint.
        if weight == 0 {
            return Err(anyhow!("commitment weight cannot be zero"));
        }

        self.vault_id_pointer()
            .set(Arc::new(context.caller.clone().into()));
        self.commitment_id_pointer().set_value(commitment_id);

        // Pack bond_nft_id (block, tx) into 32 bytes LE
        let bond_id = AlkaneId {
            block: bond_nft_block,
            tx: bond_nft_tx,
        };
        let mut bond_bytes = Vec::with_capacity(32);
        bond_bytes.extend_from_slice(&bond_id.block.to_le_bytes());
        bond_bytes.extend_from_slice(&bond_id.tx.to_le_bytes());
        self.bond_nft_id_pointer().set(Arc::new(bond_bytes));

        self.commit_block_pointer().set_value(commit_block);
        self.reveal_block_pointer().set_value(reveal_block);
        self.weight_pointer().set_value(weight);
        self.lottery_day_id_pointer().set_value(lottery_day_id);
        self.consumed_pointer().set_value(0u128);

        let name_str = format!("Foxkanes Commitment #{}", commitment_id);
        let symbol_str = format!("FK-CMT-{}", commitment_id);
        self.name_pointer().set(Arc::new(name_str.into_bytes()));
        self.symbol_pointer().set(Arc::new(symbol_str.into_bytes()));

        let mut response = CallResponse::default();
        response.alkanes.0.push(AlkaneTransfer {
            id: context.myself.clone(),
            value: 1u128,
        });
        Ok(response)
    }

    fn mark_consumed(&self) -> Result<CallResponse> {
        self.only_vault()?;
        self.consumed_pointer().set_value(1u128);
        Ok(CallResponse::default())
    }

    // ── View handlers ────────────────────────────────────────────

    fn get_commitment_id(&self) -> Result<CallResponse> {
        let context = self.context()?;
        let mut response = CallResponse::forward(&context.incoming_alkanes);
        response.data = self.commitment_id().to_le_bytes().to_vec();
        Ok(response)
    }

    fn get_bond_nft_id(&self) -> Result<CallResponse> {
        let context = self.context()?;
        let mut response = CallResponse::forward(&context.incoming_alkanes);
        let v = self.bond_nft_id();
        let mut data = Vec::with_capacity(32);
        data.extend_from_slice(&v.block.to_le_bytes());
        data.extend_from_slice(&v.tx.to_le_bytes());
        response.data = data;
        Ok(response)
    }

    fn get_commit_block(&self) -> Result<CallResponse> {
        let context = self.context()?;
        let mut response = CallResponse::forward(&context.incoming_alkanes);
        response.data = self.commit_block().to_le_bytes().to_vec();
        Ok(response)
    }

    fn get_reveal_block(&self) -> Result<CallResponse> {
        let context = self.context()?;
        let mut response = CallResponse::forward(&context.incoming_alkanes);
        response.data = self.reveal_block().to_le_bytes().to_vec();
        Ok(response)
    }

    fn get_weight(&self) -> Result<CallResponse> {
        let context = self.context()?;
        let mut response = CallResponse::forward(&context.incoming_alkanes);
        response.data = self.weight().to_le_bytes().to_vec();
        Ok(response)
    }

    fn get_lottery_day_id(&self) -> Result<CallResponse> {
        let context = self.context()?;
        let mut response = CallResponse::forward(&context.incoming_alkanes);
        response.data = self.lottery_day_id().to_le_bytes().to_vec();
        Ok(response)
    }

    fn get_consumed(&self) -> Result<CallResponse> {
        let context = self.context()?;
        let mut response = CallResponse::forward(&context.incoming_alkanes);
        response.data = self.consumed().to_le_bytes().to_vec();
        Ok(response)
    }

    fn get_vault_id(&self) -> Result<CallResponse> {
        let context = self.context()?;
        let mut response = CallResponse::forward(&context.incoming_alkanes);
        let v = self.vault_id();
        let mut data = Vec::with_capacity(32);
        data.extend_from_slice(&v.block.to_le_bytes());
        data.extend_from_slice(&v.tx.to_le_bytes());
        response.data = data;
        Ok(response)
    }

    fn get_all_details(&self) -> Result<CallResponse> {
        let context = self.context()?;
        let mut response = CallResponse::forward(&context.incoming_alkanes);
        let bond = self.bond_nft_id();
        let mut data = Vec::with_capacity(16 * 8);
        data.extend_from_slice(&self.commitment_id().to_le_bytes());
        data.extend_from_slice(&bond.block.to_le_bytes());
        data.extend_from_slice(&bond.tx.to_le_bytes());
        data.extend_from_slice(&self.commit_block().to_le_bytes());
        data.extend_from_slice(&self.reveal_block().to_le_bytes());
        data.extend_from_slice(&self.weight().to_le_bytes());
        data.extend_from_slice(&self.lottery_day_id().to_le_bytes());
        data.extend_from_slice(&self.consumed().to_le_bytes());
        response.data = data;
        Ok(response)
    }

    fn get_name(&self) -> Result<CallResponse> {
        let context = self.context()?;
        let mut response = CallResponse::forward(&context.incoming_alkanes);
        response.data = self.name_pointer().get().as_ref().clone();
        Ok(response)
    }

    fn get_symbol(&self) -> Result<CallResponse> {
        let context = self.context()?;
        let mut response = CallResponse::forward(&context.incoming_alkanes);
        response.data = self.symbol_pointer().get().as_ref().clone();
        Ok(response)
    }
}

declare_alkane! {
    impl AlkaneResponder for FoxkanesCommitment {
        type Message = FoxkanesCommitmentMessage;
    }
}
