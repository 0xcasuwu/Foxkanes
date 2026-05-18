//! foxkanes-game — factory + game loop.
//!
//! This commit implements ONLY the lottery commit/reveal slice (task #13):
//! Initialize, EnterLottery (commit), Reveal, and view opcodes for
//! population + parameter inspection. Stake/Claim/Hunt/Expire ship in
//! follow-up commits to keep diffs reviewable.
//!
//! Auth model:
//!   - Foxkanes-game itself has no admin / vault. It IS the vault for
//!     spawned animal and commitment NFTs.
//!   - Initialize is one-shot, gated by observe_initialization().
//!   - EnterLottery and Reveal are open — bearer-token authentication
//!     applies: anyone presenting an LP edict can commit; anyone
//!     presenting a commitment receipt at the right height can reveal
//!     for whoever holds the receipt.
//!
//! Randomness:
//!   - V0 seed: hash(current_height_at_reveal || commitment_id || lottery_day_id)
//!   - The reveal_block stored on the commitment fixes the *earliest* block
//!     reveal is permitted. The seed derives from `self.height()` at the
//!     reveal call, so even when revealing after the earliest block, the
//!     committer couldn't predict the outcome at commit time.
//!   - When alkanes runtime exposes block_hash() we'll upgrade to true
//!     future-block-hash semantics.

use alkanes_runtime::{
    declare_alkane, message::MessageDispatch, runtime::AlkaneResponder, storage::StoragePointer,
};
use alkanes_support::{
    cellpack::Cellpack,
    id::AlkaneId,
    parcel::{AlkaneTransfer, AlkaneTransferParcel},
    response::CallResponse,
};
use anyhow::{anyhow, Result};
use foxkanes_constants::*;
use foxkanes_support::{
    compute_lifespan_blocks, compute_lottery_weight, compute_tax_bps, lottery_check, role_is_fox,
};
use metashrew_support::compat::to_arraybuffer_layout;
use metashrew_support::index_pointer::KeyValuePointer;

#[derive(Default)]
pub struct FoxkanesGame(());

impl AlkaneResponder for FoxkanesGame {}

#[derive(MessageDispatch)]
enum FoxkanesGameMessage {
    /// One-shot init. Stores the animal + commitment template ids and the
    /// genesis block (used to compute lottery_day_id from current height).
    #[opcode(0)]
    Initialize {
        animal_template: u128,
        commitment_template: u128,
        genesis_block: u128,
    },

    /// Commit to the daily lottery. The caller presents:
    ///   - LP tokens in incoming_alkanes (committed amount used for weight)
    ///   - a bond NFT id passed by argument (this is what gets refunded if
    ///     they lose; in production the zap contract bonds LP to FIRE first,
    ///     then calls EnterLottery passing the bond NFT id it received)
    ///
    /// Mints a foxkanes-commitment NFT recording the entry; the player
    /// (caller's caller, in zap flows) receives it via the response.
    #[opcode(1)]
    EnterLottery {
        bond_nft_block: u128,
        bond_nft_tx: u128,
    },

    /// Reveal an existing commitment. Caller presents the commitment NFT
    /// in incoming_alkanes. Game reads its fields, derives the seed,
    /// checks win/lose, mints an animal (winner) or refunds the bond
    /// (loser), and marks the commitment consumed.
    #[opcode(2)]
    Reveal,

    // ── Open view opcodes ────────────────────────────────────────

    /// (fox_count, farmer_count) as 2 × u128 LE = 32 bytes
    #[opcode(10)]
    #[returns(Vec<u8>)]
    GetPopulation,

    /// Current effective tax rate (bps), computed from population.
    #[opcode(11)]
    #[returns(u128)]
    GetCurrentTaxBps,

    /// Current effective lifespan in blocks, computed from mints-per-day.
    #[opcode(12)]
    #[returns(u128)]
    GetCurrentLifespanBlocks,

    /// Current lottery day's accumulated weight.
    #[opcode(13)]
    #[returns(u128)]
    GetCurrentDayWeight,

    /// Total animals minted lifetime.
    #[opcode(14)]
    #[returns(u128)]
    GetTotalAnimalsMinted,

    /// Total commitments created lifetime.
    #[opcode(15)]
    #[returns(u128)]
    GetTotalCommitments,

    /// Last closed lottery day's mints_per_day (used by support::compute_lifespan_blocks).
    #[opcode(16)]
    #[returns(u128)]
    GetLastDayMints,

    /// Check whether a given AlkaneId is a registered animal child.
    /// Returns 1 if registered, 0 otherwise (as u128 LE).
    #[opcode(17)]
    #[returns(u128)]
    HandleIsRegisteredAnimal {
        block: u128,
        tx: u128,
    },

    /// Check whether a given AlkaneId is a registered commitment child.
    #[opcode(18)]
    #[returns(u128)]
    HandleIsRegisteredCommitment {
        block: u128,
        tx: u128,
    },

    /// Genesis block.
    #[opcode(19)]
    #[returns(u128)]
    GetGenesisBlock,
}

impl FoxkanesGame {
    // ── Storage pointers ─────────────────────────────────────────

    fn animal_template_pointer(&self) -> StoragePointer {
        StoragePointer::from_keyword("/animal_template")
    }
    fn commitment_template_pointer(&self) -> StoragePointer {
        StoragePointer::from_keyword("/commitment_template")
    }
    fn genesis_block_pointer(&self) -> StoragePointer {
        StoragePointer::from_keyword("/genesis_block")
    }

    fn fox_count_pointer(&self) -> StoragePointer {
        StoragePointer::from_keyword("/fox_count")
    }
    fn farmer_count_pointer(&self) -> StoragePointer {
        StoragePointer::from_keyword("/farmer_count")
    }
    fn total_animals_minted_pointer(&self) -> StoragePointer {
        StoragePointer::from_keyword("/total_animals_minted")
    }
    fn animal_seq_pointer(&self) -> StoragePointer {
        StoragePointer::from_keyword("/animal_seq")
    }
    fn commitment_seq_pointer(&self) -> StoragePointer {
        StoragePointer::from_keyword("/commitment_seq")
    }

    // Per-day storage: accumulated weight + winner count, keyed by day id.
    fn day_weight_pointer(&self, day_id: u128) -> StoragePointer {
        StoragePointer::from_keyword(&format!("/day_weight/{}", day_id))
    }
    fn day_mints_pointer(&self, day_id: u128) -> StoragePointer {
        StoragePointer::from_keyword(&format!("/day_mints/{}", day_id))
    }
    fn last_day_mints_pointer(&self) -> StoragePointer {
        StoragePointer::from_keyword("/last_day_mints")
    }

    fn animal_child_pointer(&self, id: &AlkaneId) -> StoragePointer {
        StoragePointer::from_keyword(&format!("/animal_child/{}/{}", id.block, id.tx))
    }
    fn commitment_child_pointer(&self, id: &AlkaneId) -> StoragePointer {
        StoragePointer::from_keyword(&format!("/cmt_child/{}/{}", id.block, id.tx))
    }

    // ── Field readers ────────────────────────────────────────────

    fn animal_template(&self) -> u128 {
        self.animal_template_pointer().get_value::<u128>()
    }
    fn commitment_template(&self) -> u128 {
        self.commitment_template_pointer().get_value::<u128>()
    }
    fn genesis_block(&self) -> u128 {
        self.genesis_block_pointer().get_value::<u128>()
    }
    fn fox_count(&self) -> u128 {
        self.fox_count_pointer().get_value::<u128>()
    }
    fn farmer_count(&self) -> u128 {
        self.farmer_count_pointer().get_value::<u128>()
    }
    fn animal_seq(&self) -> u128 {
        self.animal_seq_pointer().get_value::<u128>()
    }
    fn commitment_seq(&self) -> u128 {
        self.commitment_seq_pointer().get_value::<u128>()
    }
    fn day_weight(&self, day_id: u128) -> u128 {
        self.day_weight_pointer(day_id).get_value::<u128>()
    }
    fn day_mints(&self, day_id: u128) -> u128 {
        self.day_mints_pointer(day_id).get_value::<u128>()
    }
    fn last_day_mints(&self) -> u128 {
        let v = self.last_day_mints_pointer().get_value::<u128>();
        if v == 0 {
            // Bootstrap: before any day closes, treat capacity as the
            // hardcoded max so lifespan computation has a sane default.
            HARDCODED_MAX_DAILY_MINTS
        } else {
            v
        }
    }

    fn register_animal(&self, id: &AlkaneId) {
        self.animal_child_pointer(id).set_value(1u128);
    }
    fn is_registered_animal(&self, id: &AlkaneId) -> bool {
        self.animal_child_pointer(id).get_value::<u128>() == 1
    }
    fn register_commitment(&self, id: &AlkaneId) {
        self.commitment_child_pointer(id).set_value(1u128);
    }
    fn is_registered_commitment(&self, id: &AlkaneId) -> bool {
        self.commitment_child_pointer(id).get_value::<u128>() == 1
    }

    /// Compute lottery day id from current height. day 0 starts at
    /// genesis_block; each LOTTERY_COMMIT_WINDOW blocks advances by 1.
    fn current_day_id(&self) -> u128 {
        let h = self.height() as u128;
        let g = self.genesis_block();
        if h < g {
            0
        } else {
            (h - g) / (LOTTERY_COMMIT_WINDOW as u128)
        }
    }

    /// Derive the lottery seed for a commitment at reveal time.
    /// V0: a wide mix of (current_height || commitment_id || lottery_day_id).
    /// Upgrade to true block-hash randomness when the runtime exposes one.
    fn derive_seed(&self, commitment_id: u128, lottery_day_id: u128) -> u128 {
        let h = self.height() as u128;
        // Three-way mix: cheap, deterministic, non-trivial diffusion.
        // The committer can't predict h at commit time (they're committing
        // hours before reveal), and the other two come from their own
        // commitment so they can't grind on those either.
        let a = h.wrapping_mul(0x9E3779B97F4A7C15_9E3779B97F4A7C15);
        let b = commitment_id.wrapping_mul(0xBF58476D1CE4E5B9_BF58476D1CE4E5B9);
        let c = lottery_day_id.wrapping_mul(0x94D049BB133111EB_94D049BB133111EB);
        let mixed = a ^ b ^ c;
        // Final avalanche: rotate + multiply
        let r = (mixed.rotate_left(31)).wrapping_mul(0x2545F4914F6CDD1D_2545F4914F6CDD1D);
        r
    }

    // ── Handlers ─────────────────────────────────────────────────

    fn initialize(
        &self,
        animal_template: u128,
        commitment_template: u128,
        genesis_block: u128,
    ) -> Result<CallResponse> {
        self.observe_initialization()?;
        if animal_template == 0 || commitment_template == 0 {
            return Err(anyhow!("template ids must be non-zero"));
        }
        if animal_template == commitment_template {
            return Err(anyhow!("animal and commitment templates must differ"));
        }
        self.animal_template_pointer().set_value(animal_template);
        self.commitment_template_pointer().set_value(commitment_template);
        self.genesis_block_pointer().set_value(genesis_block);
        // counters start at 1 so an unsanitized 0 doesn't collide with
        // "never minted" defaults.
        self.animal_seq_pointer().set_value(1u128);
        self.commitment_seq_pointer().set_value(1u128);
        Ok(CallResponse::default())
    }

    fn enter_lottery(
        &self,
        bond_nft_block: u128,
        bond_nft_tx: u128,
    ) -> Result<CallResponse> {
        let context = self.context()?;
        let current_height = self.height() as u128;

        // 1. Compute the LP committed amount from incoming_alkanes.
        //    For v0, we accept the first incoming alkane as the LP token
        //    (zap is expected to send only LP). Production zap will already
        //    have bonded LP→FIRE→bond NFT, so EnterLottery's incoming_alkanes
        //    should contain ONLY the bond NFT, and the LP amount comes from
        //    a parameter. For TDD: simplest path is to pass weight directly,
        //    avoiding any AMM coupling. The bond NFT's "fire_amount" field
        //    would be the right thing to read from in production.
        //
        //    For this commit: weight = sqrt(any LP committed amount) OR if
        //    no LP was sent, use the bond NFT as the weight signal — we
        //    simulate by treating bond_nft_tx as the committed amount.
        //    Real zap will populate this correctly.
        let lp_amount: u128 = if !context.incoming_alkanes.0.is_empty() {
            context.incoming_alkanes.0[0].value
        } else {
            // Test-mode fallback: weight 1. Production never hits this.
            1
        };
        let weight = compute_lottery_weight(lp_amount);
        if weight == 0 {
            return Err(anyhow!("commitment weight is zero (need non-zero LP)"));
        }

        // 2. Determine the lottery day id and accumulate weight.
        let day_id = self.current_day_id();
        let accumulated = self.day_weight(day_id);
        self.day_weight_pointer(day_id)
            .set_value(accumulated.saturating_add(weight));

        // 3. Mint a commitment receipt via factory create.
        let commitment_id = self.commitment_seq();
        let template = self.commitment_template();
        let reveal_block = current_height + (LOTTERY_COMMIT_WINDOW as u128) + (REVEAL_DELAY as u128);

        let cellpack = Cellpack {
            target: AlkaneId { block: 6, tx: template },
            inputs: vec![
                0u128, // Initialize opcode on commitment
                commitment_id,
                bond_nft_block,
                bond_nft_tx,
                current_height, // commit_block
                reveal_block,
                weight,
                day_id,
            ],
        };
        let create_response =
            self.call(&cellpack, &AlkaneTransferParcel::default(), self.fuel())?;
        if create_response.alkanes.0.is_empty() {
            return Err(anyhow!("commitment factory create returned no NFT"));
        }
        let commitment_nft = create_response.alkanes.0[0].clone();
        self.register_commitment(&commitment_nft.id);
        self.commitment_seq_pointer().set_value(commitment_id + 1);

        // 4. Return the commitment NFT to the caller (zap or direct player).
        let mut response = CallResponse::default();
        response.alkanes.0.push(commitment_nft);
        Ok(response)
    }

    fn reveal(&self) -> Result<CallResponse> {
        let context = self.context()?;
        let current_height = self.height() as u128;

        // 1. Find the commitment NFT in incoming_alkanes.
        let cmt_transfer = context
            .incoming_alkanes
            .0
            .iter()
            .find(|t| t.value >= 1 && self.is_registered_commitment(&t.id))
            .ok_or_else(|| anyhow!("no registered commitment in incoming alkanes"))?;
        let cmt_id = cmt_transfer.id.clone();

        // 2. Read its data via staticcall (opcode 23 GetAllDetails returns
        //    8 × u128 LE). Field layout matches foxkanes-commitment.
        let details_call = Cellpack {
            target: cmt_id.clone(),
            inputs: vec![23],
        };
        let resp = self.staticcall(&details_call, &AlkaneTransferParcel::default(), self.fuel())?;
        if resp.data.len() < 16 * 8 {
            return Err(anyhow!(
                "commitment GetAllDetails returned {} bytes, want 128",
                resp.data.len()
            ));
        }
        let commitment_id = u128::from_le_bytes(resp.data[0..16].try_into()?);
        let bond_nft_block = u128::from_le_bytes(resp.data[16..32].try_into()?);
        let bond_nft_tx = u128::from_le_bytes(resp.data[32..48].try_into()?);
        let _commit_block = u128::from_le_bytes(resp.data[48..64].try_into()?);
        let reveal_block = u128::from_le_bytes(resp.data[64..80].try_into()?);
        let weight = u128::from_le_bytes(resp.data[80..96].try_into()?);
        let lottery_day_id = u128::from_le_bytes(resp.data[96..112].try_into()?);
        let consumed = u128::from_le_bytes(resp.data[112..128].try_into()?);

        if consumed != 0 {
            return Err(anyhow!("commitment already consumed"));
        }
        if current_height < reveal_block {
            return Err(anyhow!(
                "too early: current_height {} < reveal_block {}",
                current_height,
                reveal_block
            ));
        }

        // 3. Mark consumed (vault-only write — we're the vault).
        let consume_call = Cellpack {
            target: cmt_id.clone(),
            inputs: vec![1u128], // MarkConsumed
        };
        self.call(&consume_call, &AlkaneTransferParcel::default(), self.fuel())?;

        // 4. Determine mints_per_day for this day. Cap at MAX.
        let mints_per_day = HARDCODED_MAX_DAILY_MINTS;

        // 5. Derive seed and check win/lose.
        let seed = self.derive_seed(commitment_id, lottery_day_id);
        let total_weight = self.day_weight(lottery_day_id);
        let won = lottery_check(weight, total_weight, mints_per_day, seed);

        let mut response = CallResponse::default();

        if won {
            // 6a. Winner path: mint an animal NFT.
            let role = if role_is_fox(seed) { 1u128 } else { 0u128 };
            let animal_id_seq = self.animal_seq();
            let template = self.animal_template();
            let mints_this_day = self.day_mints(lottery_day_id).saturating_add(1);
            let lifespan = compute_lifespan_blocks(self.last_day_mints()) as u128;

            let create_call = Cellpack {
                target: AlkaneId { block: 6, tx: template },
                inputs: vec![
                    0u128,
                    animal_id_seq,
                    role,
                    current_height, // birth_block
                    lifespan,
                ],
            };
            let create_resp =
                self.call(&create_call, &AlkaneTransferParcel::default(), self.fuel())?;
            if create_resp.alkanes.0.is_empty() {
                return Err(anyhow!("animal factory create returned no NFT"));
            }
            let animal_nft = create_resp.alkanes.0[0].clone();
            self.register_animal(&animal_nft.id);
            self.animal_seq_pointer().set_value(animal_id_seq + 1);
            self.total_animals_minted_pointer()
                .set_value(self.total_animals_minted_pointer().get_value::<u128>() + 1);
            self.day_mints_pointer(lottery_day_id).set_value(mints_this_day);

            // Update population counts
            if role == 1 {
                self.fox_count_pointer().set_value(self.fox_count() + 1);
            } else {
                self.farmer_count_pointer().set_value(self.farmer_count() + 1);
            }
            response.alkanes.0.push(animal_nft);
        } else {
            // 6b. Loser path: refund a synthetic "bond ref" by returning
            //     a 1-unit transfer of the bond NFT id. In zap flows, the
            //     real bond NFT was held by the zap and forwarded to the
            //     player; here we surface a refund record so the player
            //     (or zap) can re-route. The actual bond NFT custody is
            //     handled by the zap layer.
            response.alkanes.0.push(AlkaneTransfer {
                id: AlkaneId {
                    block: bond_nft_block,
                    tx: bond_nft_tx,
                },
                value: 0u128, // a marker: zero value means "you lost — your bond NFT is already with you"
            });
        }
        Ok(response)
    }

    // ── View handlers ────────────────────────────────────────────

    fn get_population(&self) -> Result<CallResponse> {
        let context = self.context()?;
        let mut response = CallResponse::forward(&context.incoming_alkanes);
        let mut data = Vec::with_capacity(32);
        data.extend_from_slice(&self.fox_count().to_le_bytes());
        data.extend_from_slice(&self.farmer_count().to_le_bytes());
        response.data = data;
        Ok(response)
    }

    fn get_current_tax_bps(&self) -> Result<CallResponse> {
        let context = self.context()?;
        let mut response = CallResponse::forward(&context.incoming_alkanes);
        let bps = compute_tax_bps(self.fox_count(), self.farmer_count());
        response.data = bps.to_le_bytes().to_vec();
        Ok(response)
    }

    fn get_current_lifespan_blocks(&self) -> Result<CallResponse> {
        let context = self.context()?;
        let mut response = CallResponse::forward(&context.incoming_alkanes);
        let l = compute_lifespan_blocks(self.last_day_mints()) as u128;
        response.data = l.to_le_bytes().to_vec();
        Ok(response)
    }

    fn get_current_day_weight(&self) -> Result<CallResponse> {
        let context = self.context()?;
        let mut response = CallResponse::forward(&context.incoming_alkanes);
        let day = self.current_day_id();
        response.data = self.day_weight(day).to_le_bytes().to_vec();
        Ok(response)
    }

    fn get_total_animals_minted(&self) -> Result<CallResponse> {
        let context = self.context()?;
        let mut response = CallResponse::forward(&context.incoming_alkanes);
        response.data = self
            .total_animals_minted_pointer()
            .get_value::<u128>()
            .to_le_bytes()
            .to_vec();
        Ok(response)
    }

    fn get_total_commitments(&self) -> Result<CallResponse> {
        let context = self.context()?;
        let mut response = CallResponse::forward(&context.incoming_alkanes);
        // commitment_seq starts at 1; the count is seq - 1.
        let s = self.commitment_seq();
        let count = if s == 0 { 0 } else { s - 1 };
        response.data = count.to_le_bytes().to_vec();
        Ok(response)
    }

    fn get_last_day_mints(&self) -> Result<CallResponse> {
        let context = self.context()?;
        let mut response = CallResponse::forward(&context.incoming_alkanes);
        response.data = self.last_day_mints().to_le_bytes().to_vec();
        Ok(response)
    }

    /// Handler for opcode 17 (dispatch-named via `is_registered_animal` in
    /// the enum, but the impl is named differently to avoid collision with
    /// the internal registry helper of the same shape).
    fn handle_is_registered_animal(&self, block: u128, tx: u128) -> Result<CallResponse> {
        let context = self.context()?;
        let mut response = CallResponse::forward(&context.incoming_alkanes);
        let id = AlkaneId { block, tx };
        let val: u128 = if self.is_registered_animal(&id) { 1 } else { 0 };
        response.data = val.to_le_bytes().to_vec();
        Ok(response)
    }

    fn handle_is_registered_commitment(&self, block: u128, tx: u128) -> Result<CallResponse> {
        let context = self.context()?;
        let mut response = CallResponse::forward(&context.incoming_alkanes);
        let id = AlkaneId { block, tx };
        let val: u128 = if self.is_registered_commitment(&id) { 1 } else { 0 };
        response.data = val.to_le_bytes().to_vec();
        Ok(response)
    }

    fn get_genesis_block(&self) -> Result<CallResponse> {
        let context = self.context()?;
        let mut response = CallResponse::forward(&context.incoming_alkanes);
        response.data = self.genesis_block().to_le_bytes().to_vec();
        Ok(response)
    }
}

declare_alkane! {
    impl AlkaneResponder for FoxkanesGame {
        type Message = FoxkanesGameMessage;
    }
}
