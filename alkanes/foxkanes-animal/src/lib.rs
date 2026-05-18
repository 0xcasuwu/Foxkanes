//! foxkanes-animal — per-NFT receipt token (bearer-NFT pattern).
//!
//! Each animal is a freshly-minted alkane, spawned by foxkanes-game via the
//! block-6 factory pattern. State lives in this NFT's own storage. The game
//! is recorded as `vault_id` at init and is the only authority that can
//! mutate state (per the fire-bond-token / alk4626-position-token model).
//!
//! Reads are open. Writes (claim, taxes, hunt-in-progress, role-conversion,
//! mark-dead) are vault-only.
//!
//! Bearer-token: whoever physically holds 1 unit of this alkane owns the
//! position. There is no separate owner ledger; presenting the unit in a
//! call to foxkanes-game *is* the proof of ownership.

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
pub struct FoxkanesAnimal(());

impl AlkaneResponder for FoxkanesAnimal {}

#[derive(MessageDispatch)]
enum FoxkanesAnimalMessage {
    /// One-shot init. `context.caller` is recorded as `vault_id`. Mints 1
    /// unit returned to the caller (which is foxkanes-game; the game then
    /// forwards the unit to the player).
    #[opcode(0)]
    Initialize {
        animal_id: u128,
        role: u128,           // 0 = farmer, 1 = fox
        birth_block: u128,
        lifespan_blocks: u128,
    },

    // ── Vault-only state mutations ────────────────────────────────

    /// Set the last_claim_block (called on every successful claim).
    #[opcode(1)]
    SetLastClaimBlock { new_block: u128 },

    /// Set the accumulated_taxes (fox-only — fox NFTs accrue tax revenue
    /// from staked farmers; reset to 0 after a claim).
    #[opcode(2)]
    SetAccumulatedTaxes { new_amount: u128 },

    /// Flip role from farmer (0) to fox (1). Used when a hunting party
    /// member is promoted on a successful hunt. Idempotent — calling on
    /// an already-fox is a no-op.
    #[opcode(3)]
    ConvertToFox,

    /// Mark the animal dead. After this, all reads still work (for
    /// historical query) but the game will not accept the NFT for any
    /// gameplay action.
    #[opcode(4)]
    MarkDead,

    /// Set the hunt_in_progress flag. Used to freeze a fox during a
    /// pending hunt resolution — prevents the fox-suicide attack
    /// (transfer-to-fresh-alt to dodge a hunt).
    #[opcode(5)]
    SetHuntInProgress { value: u128 },

    // ── Open view opcodes ────────────────────────────────────────

    #[opcode(10)]
    #[returns(u128)]
    GetAnimalId,

    #[opcode(11)]
    #[returns(u128)]
    GetRole,

    #[opcode(12)]
    #[returns(u128)]
    GetBirthBlock,

    #[opcode(13)]
    #[returns(u128)]
    GetLifespanBlocks,

    #[opcode(14)]
    #[returns(u128)]
    GetAccumulatedTaxes,

    #[opcode(15)]
    #[returns(u128)]
    GetLastClaimBlock,

    #[opcode(16)]
    #[returns(u128)]
    GetIsDead,

    #[opcode(17)]
    #[returns(u128)]
    GetHuntInProgress,

    #[opcode(18)]
    #[returns(AlkaneId)]
    GetVaultId,

    /// Returns all fields packed as 8 × u128 LE = 128 bytes:
    /// [animal_id, role, birth_block, lifespan_blocks, accumulated_taxes, last_claim_block, is_dead, hunt_in_progress]
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

impl FoxkanesAnimal {
    // ── Storage pointers ─────────────────────────────────────────

    fn vault_id_pointer(&self) -> StoragePointer {
        StoragePointer::from_keyword("/vault_id")
    }
    fn animal_id_pointer(&self) -> StoragePointer {
        StoragePointer::from_keyword("/animal_id")
    }
    fn role_pointer(&self) -> StoragePointer {
        StoragePointer::from_keyword("/role")
    }
    fn birth_block_pointer(&self) -> StoragePointer {
        StoragePointer::from_keyword("/birth_block")
    }
    fn lifespan_blocks_pointer(&self) -> StoragePointer {
        StoragePointer::from_keyword("/lifespan_blocks")
    }
    fn accumulated_taxes_pointer(&self) -> StoragePointer {
        StoragePointer::from_keyword("/accumulated_taxes")
    }
    fn last_claim_block_pointer(&self) -> StoragePointer {
        StoragePointer::from_keyword("/last_claim_block")
    }
    fn is_dead_pointer(&self) -> StoragePointer {
        StoragePointer::from_keyword("/is_dead")
    }
    fn hunt_in_progress_pointer(&self) -> StoragePointer {
        StoragePointer::from_keyword("/hunt_in_progress")
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
    fn animal_id(&self) -> u128 {
        self.animal_id_pointer().get_value::<u128>()
    }
    fn role(&self) -> u128 {
        self.role_pointer().get_value::<u128>()
    }
    fn birth_block(&self) -> u128 {
        self.birth_block_pointer().get_value::<u128>()
    }
    fn lifespan_blocks(&self) -> u128 {
        self.lifespan_blocks_pointer().get_value::<u128>()
    }
    fn accumulated_taxes(&self) -> u128 {
        self.accumulated_taxes_pointer().get_value::<u128>()
    }
    fn last_claim_block(&self) -> u128 {
        self.last_claim_block_pointer().get_value::<u128>()
    }
    fn is_dead(&self) -> u128 {
        self.is_dead_pointer().get_value::<u128>()
    }
    fn hunt_in_progress(&self) -> u128 {
        self.hunt_in_progress_pointer().get_value::<u128>()
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
        animal_id: u128,
        role: u128,
        birth_block: u128,
        lifespan_blocks: u128,
    ) -> Result<CallResponse> {
        let context = self.context()?;
        self.observe_initialization()?;

        // Sanity: role must be 0 (farmer) or 1 (fox).
        if role > 1 {
            return Err(anyhow!("invalid role: {} (must be 0 or 1)", role));
        }

        // vault = context.caller (foxkanes-game during factory create).
        self.vault_id_pointer()
            .set(Arc::new(context.caller.clone().into()));

        self.animal_id_pointer().set_value(animal_id);
        self.role_pointer().set_value(role);
        self.birth_block_pointer().set_value(birth_block);
        self.lifespan_blocks_pointer().set_value(lifespan_blocks);
        // Initial values: no taxes accumulated, no claims yet, alive, no hunt pending.
        self.accumulated_taxes_pointer().set_value(0u128);
        self.last_claim_block_pointer().set_value(birth_block);
        self.is_dead_pointer().set_value(0u128);
        self.hunt_in_progress_pointer().set_value(0u128);

        let role_name = if role == 1 { "Fox" } else { "Farmer" };
        let name_str = format!("Foxkanes {} #{}", role_name, animal_id);
        let symbol_str = format!("FK-{}", animal_id);
        self.name_pointer().set(Arc::new(name_str.into_bytes()));
        self.symbol_pointer().set(Arc::new(symbol_str.into_bytes()));

        // Mint exactly 1 unit, returned to the caller (vault). The vault
        // forwards it to the player in its own response.
        let mut response = CallResponse::default();
        response.alkanes.0.push(AlkaneTransfer {
            id: context.myself.clone(),
            value: 1u128,
        });
        Ok(response)
    }

    fn set_last_claim_block(&self, new_block: u128) -> Result<CallResponse> {
        self.only_vault()?;
        self.last_claim_block_pointer().set_value(new_block);
        Ok(CallResponse::default())
    }

    fn set_accumulated_taxes(&self, new_amount: u128) -> Result<CallResponse> {
        self.only_vault()?;
        self.accumulated_taxes_pointer().set_value(new_amount);
        Ok(CallResponse::default())
    }

    fn convert_to_fox(&self) -> Result<CallResponse> {
        self.only_vault()?;
        // Idempotent — calling on an already-fox is a silent no-op so the
        // game doesn't have to branch on current role.
        self.role_pointer().set_value(1u128);
        Ok(CallResponse::default())
    }

    fn mark_dead(&self) -> Result<CallResponse> {
        self.only_vault()?;
        self.is_dead_pointer().set_value(1u128);
        Ok(CallResponse::default())
    }

    fn set_hunt_in_progress(&self, value: u128) -> Result<CallResponse> {
        self.only_vault()?;
        // Coerce non-zero to 1 for a clean flag.
        let flag = if value == 0 { 0u128 } else { 1u128 };
        self.hunt_in_progress_pointer().set_value(flag);
        Ok(CallResponse::default())
    }

    // ── View handlers ────────────────────────────────────────────

    fn get_animal_id(&self) -> Result<CallResponse> {
        let context = self.context()?;
        let mut response = CallResponse::forward(&context.incoming_alkanes);
        response.data = self.animal_id().to_le_bytes().to_vec();
        Ok(response)
    }

    fn get_role(&self) -> Result<CallResponse> {
        let context = self.context()?;
        let mut response = CallResponse::forward(&context.incoming_alkanes);
        response.data = self.role().to_le_bytes().to_vec();
        Ok(response)
    }

    fn get_birth_block(&self) -> Result<CallResponse> {
        let context = self.context()?;
        let mut response = CallResponse::forward(&context.incoming_alkanes);
        response.data = self.birth_block().to_le_bytes().to_vec();
        Ok(response)
    }

    fn get_lifespan_blocks(&self) -> Result<CallResponse> {
        let context = self.context()?;
        let mut response = CallResponse::forward(&context.incoming_alkanes);
        response.data = self.lifespan_blocks().to_le_bytes().to_vec();
        Ok(response)
    }

    fn get_accumulated_taxes(&self) -> Result<CallResponse> {
        let context = self.context()?;
        let mut response = CallResponse::forward(&context.incoming_alkanes);
        response.data = self.accumulated_taxes().to_le_bytes().to_vec();
        Ok(response)
    }

    fn get_last_claim_block(&self) -> Result<CallResponse> {
        let context = self.context()?;
        let mut response = CallResponse::forward(&context.incoming_alkanes);
        response.data = self.last_claim_block().to_le_bytes().to_vec();
        Ok(response)
    }

    fn get_is_dead(&self) -> Result<CallResponse> {
        let context = self.context()?;
        let mut response = CallResponse::forward(&context.incoming_alkanes);
        response.data = self.is_dead().to_le_bytes().to_vec();
        Ok(response)
    }

    fn get_hunt_in_progress(&self) -> Result<CallResponse> {
        let context = self.context()?;
        let mut response = CallResponse::forward(&context.incoming_alkanes);
        response.data = self.hunt_in_progress().to_le_bytes().to_vec();
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
        let mut data = Vec::with_capacity(16 * 8);
        data.extend_from_slice(&self.animal_id().to_le_bytes());
        data.extend_from_slice(&self.role().to_le_bytes());
        data.extend_from_slice(&self.birth_block().to_le_bytes());
        data.extend_from_slice(&self.lifespan_blocks().to_le_bytes());
        data.extend_from_slice(&self.accumulated_taxes().to_le_bytes());
        data.extend_from_slice(&self.last_claim_block().to_le_bytes());
        data.extend_from_slice(&self.is_dead().to_le_bytes());
        data.extend_from_slice(&self.hunt_in_progress().to_le_bytes());
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

impl FoxkanesAnimal {
    fn handle(&self, message: FoxkanesAnimalMessage) -> Result<CallResponse> {
        match message {
            FoxkanesAnimalMessage::Initialize {
                animal_id,
                role,
                birth_block,
                lifespan_blocks,
            } => self.initialize(animal_id, role, birth_block, lifespan_blocks),
            FoxkanesAnimalMessage::SetLastClaimBlock { new_block } => {
                self.set_last_claim_block(new_block)
            }
            FoxkanesAnimalMessage::SetAccumulatedTaxes { new_amount } => {
                self.set_accumulated_taxes(new_amount)
            }
            FoxkanesAnimalMessage::ConvertToFox => self.convert_to_fox(),
            FoxkanesAnimalMessage::MarkDead => self.mark_dead(),
            FoxkanesAnimalMessage::SetHuntInProgress { value } => {
                self.set_hunt_in_progress(value)
            }
            FoxkanesAnimalMessage::GetAnimalId => self.get_animal_id(),
            FoxkanesAnimalMessage::GetRole => self.get_role(),
            FoxkanesAnimalMessage::GetBirthBlock => self.get_birth_block(),
            FoxkanesAnimalMessage::GetLifespanBlocks => self.get_lifespan_blocks(),
            FoxkanesAnimalMessage::GetAccumulatedTaxes => self.get_accumulated_taxes(),
            FoxkanesAnimalMessage::GetLastClaimBlock => self.get_last_claim_block(),
            FoxkanesAnimalMessage::GetIsDead => self.get_is_dead(),
            FoxkanesAnimalMessage::GetHuntInProgress => self.get_hunt_in_progress(),
            FoxkanesAnimalMessage::GetVaultId => self.get_vault_id(),
            FoxkanesAnimalMessage::GetAllDetails => self.get_all_details(),
            FoxkanesAnimalMessage::GetName => self.get_name(),
            FoxkanesAnimalMessage::GetSymbol => self.get_symbol(),
        }
    }
}

declare_alkane! {
    impl AlkaneResponder for FoxkanesAnimal {
        type Message = FoxkanesAnimalMessage;
    }
}
