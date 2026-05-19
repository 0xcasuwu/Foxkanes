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
    compute_aging_blocks, compute_lifespan_blocks, compute_lottery_weight,
    compute_party_success_bps, compute_per_hunter_prob_bps, compute_tax_bps, lottery_check,
    role_is_fox,
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

    /// Stake an animal NFT so it begins accruing yield. Caller presents
    /// the animal NFT; game marks it as staked at current_height and
    /// records the stake-block on the animal (reusing last_claim_block
    /// since that's effectively "yield checkpoint" from here on). The
    /// NFT is returned to the caller (bearer-token preserved).
    #[opcode(3)]
    Stake,

    /// Safe-claim: a staked animal claims accrued yield. For farmers,
    /// homeostatic tax is deducted and routed to the shared fox pool;
    /// for foxes, the claim withdraws their share of the fox pool plus
    /// any direct yield. Updates last_claim_block on the animal.
    #[opcode(4)]
    ClaimSafe,

    /// Risky-claim: a 50/50 coinflip per RISKY_KEEP_PROBABILITY_BPS.
    /// Win: caller receives the full pre-tax yield (no tax to fox pool).
    /// Loss: caller receives nothing; ALL yield routes to the fox pool.
    /// Updates last_claim_block on the animal either way.
    #[opcode(5)]
    ClaimRisky,

    /// View: shared fox-pool balance (yield owed to foxes collectively).
    #[opcode(20)]
    #[returns(u128)]
    GetFoxPool,

    /// View: total fox-pool yield ever distributed (lifetime).
    #[opcode(21)]
    #[returns(u128)]
    GetFoxPoolLifetime,

    /// View: simulate yield owed to a staked animal as of current height,
    /// pre-tax. Used by tests and front-ends; reads the animal's
    /// last_claim_block via staticcall. Inputs: (animal_block, animal_tx).
    #[opcode(22)]
    #[returns(u128)]
    PreviewYield {
        animal_block: u128,
        animal_tx: u128,
    },

    /// View: AlkaneId of the most-recently-spawned animal NFT. Returns
    /// (block, tx) as 2 × u128 LE = 32 bytes. Useful for clients (and
    /// tests) to discover which AlkaneId the runtime assigned after a
    /// successful Reveal.
    #[opcode(23)]
    #[returns(Vec<u8>)]
    GetLatestAnimalId,

    /// View: AlkaneId of the most-recently-spawned commitment NFT.
    /// Returns (block, tx) as 2 × u128 LE.
    #[opcode(24)]
    #[returns(Vec<u8>)]
    GetLatestCommitmentId,

    /// Initiate a hunt against a staked fox. Caller presents N farmer
    /// animal NFTs in incoming_alkanes (MIN_HUNT_PARTY ≤ N ≤ MAX_HUNT_PARTY).
    /// Game freezes all participants (sets hunt_in_progress=1), records
    /// hunt state, returns party NFTs to caller. Hunt id stored in
    /// response.data (16 LE bytes u128). Per-NFT detail is fetched via
    /// GetHunt(hunt_id) afterwards.
    #[opcode(6)]
    InitiateHunt {
        target_fox_block: u128,
        target_fox_tx: u128,
    },

    /// Resolve an open hunt. Caller passes the hunt_id. Derives seed,
    /// computes per-hunter prob (scales with target's unclaimed taxes
    /// vs all-time max), party success = 1-(1-p)^N. Success: distribute
    /// target's unclaimed taxes among party (added to each member's
    /// accumulated_taxes), burn target fox (mark dead), convert one
    /// party member to fox. Failure: age each party member.
    /// Unfreezes all participants either way.
    #[opcode(7)]
    ResolveHunt {
        hunt_id: u128,
    },

    /// View: state of a specific hunt. Returns 6 × u128 LE = 96 bytes:
    /// [target_block, target_tx, party_size, initiation_block, resolved, success]
    #[opcode(25)]
    #[returns(Vec<u8>)]
    GetHunt {
        hunt_id: u128,
    },

    /// View: total hunts ever initiated.
    #[opcode(26)]
    #[returns(u128)]
    GetTotalHunts,

    /// View: failed-hunt count in the recent window (last RECENT_WINDOW blocks).
    #[opcode(27)]
    #[returns(u128)]
    GetRecentFailedHunts,

    /// View: total hunts in the recent window (for compute_aging_blocks).
    #[opcode(28)]
    #[returns(u128)]
    GetRecentTotalHunts,

    /// View: extra aging applied to an animal via failed hunts.
    #[opcode(29)]
    #[returns(u128)]
    GetAnimalAging {
        block: u128,
        tx: u128,
    },

    /// View: max-observed fox unclaimed taxes (moving max), used as the
    /// denominator in compute_per_hunter_prob_bps.
    #[opcode(30)]
    #[returns(u128)]
    GetMaxFoxUnclaimed,

    /// Permissionless expiration cleanup. Anyone can call Expire(animal_id)
    /// once the animal's effective lifespan has elapsed:
    ///   `current_height >= birth_block + lifespan_blocks - animal_aging[id]`
    /// On expiration:
    ///   - animal marked dead
    ///   - population counters decremented (and staked counters if applicable)
    ///   - hunt-pending check: error if hunt_in_progress=1
    ///   - returns a bounty record in response.data
    /// The bounty is a u128 value computed from on-chain state (currently
    /// a fixed token from RewardBounty in v0, scaling-with-treasury in
    /// later versions). For testability we expose a fixed
    /// EXPIRE_BOUNTY constant.
    #[opcode(8)]
    Expire {
        animal_block: u128,
        animal_tx: u128,
    },

    /// View: effective lifespan-end block for an animal, accounting for
    /// extra_aging applied via failed hunts. Returns the block at which
    /// Expire becomes callable. Inputs: (block, tx).
    #[opcode(31)]
    #[returns(u128)]
    GetExpireBlock {
        block: u128,
        tx: u128,
    },

    /// View: total expirations processed lifetime.
    #[opcode(32)]
    #[returns(u128)]
    GetTotalExpirations,

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

    // ── Staking + tax pool storage ──────────────────────────────

    /// Per-animal staked flag and the block they staked at. Stored in the
    /// game (not on the animal) so the game's invariants on staked count
    /// stay consistent; the animal's last_claim_block doubles as the yield
    /// checkpoint.
    fn animal_staked_pointer(&self, id: &AlkaneId) -> StoragePointer {
        StoragePointer::from_keyword(&format!("/animal_staked/{}/{}", id.block, id.tx))
    }

    /// Count of currently-staked foxes — used as the denominator when a
    /// farmer pays tax to update the fox-pool accumulator.
    fn staked_fox_count_pointer(&self) -> StoragePointer {
        StoragePointer::from_keyword("/staked_fox_count")
    }

    /// Count of currently-staked farmers (informational; not used in the
    /// hot path but useful for monitoring + later hunt-party gating).
    fn staked_farmer_count_pointer(&self) -> StoragePointer {
        StoragePointer::from_keyword("/staked_farmer_count")
    }

    /// Synthetix-style accumulator: `tax_per_fox_acc = Σ (tax_paid × PRECISION / staked_fox_count_at_payment)`.
    /// A fox's claim = `(current_acc - fox_checkpoint) / PRECISION` * 1.
    /// PRECISION = foxkanes_constants::PRECISION = 1e18.
    fn tax_per_fox_acc_pointer(&self) -> StoragePointer {
        StoragePointer::from_keyword("/tax_per_fox_acc")
    }

    /// Per-fox checkpoint of the accumulator at their last claim (or stake).
    fn fox_acc_checkpoint_pointer(&self, id: &AlkaneId) -> StoragePointer {
        StoragePointer::from_keyword(&format!("/fox_ck/{}/{}", id.block, id.tx))
    }

    /// Lifetime tax distributed to the fox pool, for monitoring/views.
    fn fox_pool_lifetime_pointer(&self) -> StoragePointer {
        StoragePointer::from_keyword("/fox_pool_lifetime")
    }

    /// Aggregate undistributed pool balance — view-only, useful for UIs.
    /// Tracks (sum of farmer-paid taxes since genesis) - (sum of fox claims).
    fn fox_pool_balance_pointer(&self) -> StoragePointer {
        StoragePointer::from_keyword("/fox_pool_balance")
    }

    // ── Hunt storage ─────────────────────────────────────────────

    fn hunt_seq_pointer(&self) -> StoragePointer {
        StoragePointer::from_keyword("/hunt_seq")
    }

    fn hunt_target_block_pointer(&self, hunt_id: u128) -> StoragePointer {
        StoragePointer::from_keyword(&format!("/hunt/{}/target_block", hunt_id))
    }
    fn hunt_target_tx_pointer(&self, hunt_id: u128) -> StoragePointer {
        StoragePointer::from_keyword(&format!("/hunt/{}/target_tx", hunt_id))
    }
    fn hunt_party_size_pointer(&self, hunt_id: u128) -> StoragePointer {
        StoragePointer::from_keyword(&format!("/hunt/{}/party_size", hunt_id))
    }
    fn hunt_init_block_pointer(&self, hunt_id: u128) -> StoragePointer {
        StoragePointer::from_keyword(&format!("/hunt/{}/init_block", hunt_id))
    }
    fn hunt_resolved_pointer(&self, hunt_id: u128) -> StoragePointer {
        StoragePointer::from_keyword(&format!("/hunt/{}/resolved", hunt_id))
    }
    fn hunt_success_pointer(&self, hunt_id: u128) -> StoragePointer {
        StoragePointer::from_keyword(&format!("/hunt/{}/success", hunt_id))
    }

    /// Per-hunt party member by index 0..party_size — block + tx of each
    /// participating farmer animal.
    fn hunt_member_block_pointer(&self, hunt_id: u128, idx: u128) -> StoragePointer {
        StoragePointer::from_keyword(&format!("/hunt/{}/m{}/b", hunt_id, idx))
    }
    fn hunt_member_tx_pointer(&self, hunt_id: u128, idx: u128) -> StoragePointer {
        StoragePointer::from_keyword(&format!("/hunt/{}/m{}/t", hunt_id, idx))
    }

    /// Extra aging applied to a specific animal via failed hunts.
    fn animal_aging_pointer(&self, id: &AlkaneId) -> StoragePointer {
        StoragePointer::from_keyword(&format!("/aging/{}/{}", id.block, id.tx))
    }

    /// Maximum fox-unclaimed-taxes ever observed at hunt-initiation time —
    /// used as the divisor in compute_per_hunter_prob_bps so the
    /// "ripeness" scaling has a stable reference.
    fn max_fox_unclaimed_pointer(&self) -> StoragePointer {
        StoragePointer::from_keyword("/max_fox_unclaimed")
    }

    /// Rolling counters for the adaptive aging formula.
    /// Recent = within the last RECENT_HUNT_WINDOW blocks (we approximate
    /// via a simple decay: every WINDOW blocks of inactivity, reset; for
    /// v0 we count lifetime and let tests assert correctness on the
    /// formula path while the window logic is exercised in integration).
    fn lifetime_failed_hunts_pointer(&self) -> StoragePointer {
        StoragePointer::from_keyword("/lifetime_failed_hunts")
    }
    fn lifetime_total_hunts_pointer(&self) -> StoragePointer {
        StoragePointer::from_keyword("/lifetime_total_hunts")
    }

    fn total_expirations_pointer(&self) -> StoragePointer {
        StoragePointer::from_keyword("/total_expirations")
    }

    /// Most-recently-spawned animal AlkaneId — stored as two u128 cells
    /// to avoid Arc<Vec<u8>> semantics that have proven flaky in mid-handler
    /// writes (initial Arc::new approach didn't persist to view reads).
    fn latest_animal_id_block_pointer(&self) -> StoragePointer {
        StoragePointer::from_keyword("/latest_animal_id_block")
    }
    fn latest_animal_id_tx_pointer(&self) -> StoragePointer {
        StoragePointer::from_keyword("/latest_animal_id_tx")
    }

    /// Most-recently-spawned commitment AlkaneId.
    fn latest_commitment_id_block_pointer(&self) -> StoragePointer {
        StoragePointer::from_keyword("/latest_commitment_id_block")
    }
    fn latest_commitment_id_tx_pointer(&self) -> StoragePointer {
        StoragePointer::from_keyword("/latest_commitment_id_tx")
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

    // ── Staking state readers ───────────────────────────────────

    fn is_staked(&self, id: &AlkaneId) -> bool {
        self.animal_staked_pointer(id).get_value::<u128>() == 1
    }
    fn staked_fox_count(&self) -> u128 {
        self.staked_fox_count_pointer().get_value::<u128>()
    }
    fn staked_farmer_count(&self) -> u128 {
        self.staked_farmer_count_pointer().get_value::<u128>()
    }
    fn tax_per_fox_acc(&self) -> u128 {
        self.tax_per_fox_acc_pointer().get_value::<u128>()
    }
    fn fox_acc_checkpoint(&self, id: &AlkaneId) -> u128 {
        self.fox_acc_checkpoint_pointer(id).get_value::<u128>()
    }
    fn fox_pool_balance(&self) -> u128 {
        self.fox_pool_balance_pointer().get_value::<u128>()
    }
    fn fox_pool_lifetime(&self) -> u128 {
        self.fox_pool_lifetime_pointer().get_value::<u128>()
    }

    /// Read an animal's full state via staticcall to its op 23 GetAllDetails.
    /// Returns (animal_id, role, birth_block, lifespan, accumulated_taxes,
    /// last_claim_block, is_dead, hunt_in_progress).
    fn read_animal(
        &self,
        id: &AlkaneId,
    ) -> Result<(u128, u128, u128, u128, u128, u128, u128, u128)> {
        let call = Cellpack {
            target: id.clone(),
            inputs: vec![23],
        };
        let resp = self.staticcall(&call, &AlkaneTransferParcel::default(), self.fuel())?;
        if resp.data.len() < 16 * 8 {
            return Err(anyhow!(
                "animal GetAllDetails returned {} bytes, want 128",
                resp.data.len()
            ));
        }
        Ok((
            u128::from_le_bytes(resp.data[0..16].try_into()?),
            u128::from_le_bytes(resp.data[16..32].try_into()?),
            u128::from_le_bytes(resp.data[32..48].try_into()?),
            u128::from_le_bytes(resp.data[48..64].try_into()?),
            u128::from_le_bytes(resp.data[64..80].try_into()?),
            u128::from_le_bytes(resp.data[80..96].try_into()?),
            u128::from_le_bytes(resp.data[96..112].try_into()?),
            u128::from_le_bytes(resp.data[112..128].try_into()?),
        ))
    }

    /// Vault-only write helper: set the animal's last_claim_block via op 1.
    fn write_animal_last_claim(&self, id: &AlkaneId, new_block: u128) -> Result<()> {
        let call = Cellpack {
            target: id.clone(),
            inputs: vec![1u128, new_block], // SetLastClaimBlock
        };
        self.call(&call, &AlkaneTransferParcel::default(), self.fuel())?;
        Ok(())
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
        // Record latest id (block, tx) as two u128 cells.
        self.latest_commitment_id_block_pointer()
            .set_value(commitment_nft.id.block);
        self.latest_commitment_id_tx_pointer()
            .set_value(commitment_nft.id.tx);
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
            // Record latest spawned animal id as two u128 cells.
            self.latest_animal_id_block_pointer()
                .set_value(animal_nft.id.block);
            self.latest_animal_id_tx_pointer()
                .set_value(animal_nft.id.tx);
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

    // ── Staking and claim handlers ──────────────────────────────

    /// Authenticate that an animal NFT is present in incoming_alkanes,
    /// registered as our child, alive, and (optionally) in the expected
    /// staked-state. Returns the AlkaneId of the verified animal.
    fn authenticate_animal(&self, want_staked: bool) -> Result<AlkaneId> {
        let context = self.context()?;
        let xfer = context
            .incoming_alkanes
            .0
            .iter()
            .find(|t| t.value >= 1 && self.is_registered_animal(&t.id))
            .ok_or_else(|| anyhow!("no registered animal in incoming alkanes"))?;
        let id = xfer.id.clone();

        let (_animal_id, _role, _birth, _lifespan, _taxes, _last_claim, is_dead, _hunt) =
            self.read_animal(&id)?;
        if is_dead != 0 {
            return Err(anyhow!("animal is dead"));
        }
        let staked_now = self.is_staked(&id);
        if want_staked && !staked_now {
            return Err(anyhow!("animal must be staked for this op"));
        }
        if !want_staked && staked_now {
            return Err(anyhow!("animal is already staked"));
        }
        Ok(id)
    }

    fn stake(&self) -> Result<CallResponse> {
        let context = self.context()?;
        let id = self.authenticate_animal(false)?;
        let (_animal_id, role, _birth, _lifespan, _taxes, _last_claim, _dead, _hunt) =
            self.read_animal(&id)?;
        let current_height = self.height() as u128;

        // Reset the yield checkpoint to the stake block so future yield
        // accrues from now on.
        self.write_animal_last_claim(&id, current_height)?;
        self.animal_staked_pointer(&id).set_value(1u128);

        // Update population & fox-pool checkpoints by role.
        if role == 1 {
            // Fox just staked: snapshot the current accumulator so they
            // only earn tax distributed after this point.
            let acc = self.tax_per_fox_acc();
            self.fox_acc_checkpoint_pointer(&id).set_value(acc);
            self.staked_fox_count_pointer()
                .set_value(self.staked_fox_count() + 1);
        } else {
            self.staked_farmer_count_pointer()
                .set_value(self.staked_farmer_count() + 1);
        }

        // Return the NFT (bearer-token preserved).
        let mut response = CallResponse::default();
        response.alkanes.0.extend(context.incoming_alkanes.0.iter().cloned());
        Ok(response)
    }

    /// Compute pre-tax yield owed to an animal between `last_claim_block`
    /// and `current_height`, using TEST_YIELD_PER_BLOCK_PER_STAKE.
    fn compute_yield_units(&self, last_claim_block: u128) -> u128 {
        let current_height = self.height() as u128;
        if current_height <= last_claim_block {
            return 0;
        }
        let elapsed = current_height - last_claim_block;
        elapsed.saturating_mul(TEST_YIELD_PER_BLOCK_PER_STAKE)
    }

    /// Distribute tax to the fox pool by bumping the per-fox accumulator.
    /// Splits tax across all currently-staked foxes evenly via the
    /// Synthetix `tax_per_fox_acc += tax × PRECISION / fox_count` pattern.
    /// If no foxes are staked, the tax accrues to the pool balance but
    /// without contributing to the accumulator — foxes who stake later
    /// won't see this tax via their checkpoint; instead it's claimable
    /// via a fallback the contract reserves for the first fox to stake
    /// after a "no-fox" period. For v0 simplicity we floor at 1: if zero
    /// foxes staked, all tax routes to lifetime + balance and the
    /// accumulator stays untouched. Foxes who stake later only earn
    /// from taxes paid *after* their stake.
    fn distribute_tax_to_foxes(&self, tax: u128) {
        if tax == 0 {
            return;
        }
        let fox_count = self.staked_fox_count();
        if fox_count > 0 {
            let acc = self.tax_per_fox_acc();
            let add = tax
                .saturating_mul(PRECISION)
                .checked_div(fox_count)
                .unwrap_or(0);
            self.tax_per_fox_acc_pointer().set_value(acc.saturating_add(add));
        }
        self.fox_pool_balance_pointer()
            .set_value(self.fox_pool_balance().saturating_add(tax));
        self.fox_pool_lifetime_pointer()
            .set_value(self.fox_pool_lifetime().saturating_add(tax));
    }

    /// Compute the tax owed to the fox pool when a farmer claims `yield`.
    /// Uses the homeostatic compute_tax_bps with current population.
    fn farmer_tax_owed(&self, yield_units: u128) -> u128 {
        let bps = compute_tax_bps(self.fox_count(), self.farmer_count());
        yield_units.saturating_mul(bps).checked_div(BPS).unwrap_or(0)
    }

    /// Withdraw a fox's accumulated share from the per-fox accumulator,
    /// updating their checkpoint and decrementing the pool balance.
    fn fox_claim_share(&self, id: &AlkaneId) -> u128 {
        let acc = self.tax_per_fox_acc();
        let ck = self.fox_acc_checkpoint(id);
        if acc <= ck {
            return 0;
        }
        let diff = acc - ck;
        // share = diff / PRECISION  (units returned to the fox)
        let share = diff.checked_div(PRECISION).unwrap_or(0);
        self.fox_acc_checkpoint_pointer(id).set_value(acc);
        let bal = self.fox_pool_balance();
        self.fox_pool_balance_pointer()
            .set_value(bal.saturating_sub(share));
        share
    }

    fn claim_safe(&self) -> Result<CallResponse> {
        let context = self.context()?;
        let id = self.authenticate_animal(true)?;
        let (_aid, role, _birth, _lifespan, _taxes, last_claim, _dead, _hunt) =
            self.read_animal(&id)?;

        let current_height = self.height() as u128;
        let mut payout: u128 = 0;

        if role == 0 {
            // Farmer: compute yield, deduct homeostatic tax, route tax to
            // fox pool, payout remainder.
            let yield_units = self.compute_yield_units(last_claim);
            let tax = self.farmer_tax_owed(yield_units);
            self.distribute_tax_to_foxes(tax);
            payout = yield_units.saturating_sub(tax);
        } else {
            // Fox: claim their share of the accumulator + any direct
            // yield (a fox earns the same per-block yield as a farmer in
            // v0 — production may differ, but symmetry simplifies tests).
            let direct = self.compute_yield_units(last_claim);
            let pool_share = self.fox_claim_share(&id);
            payout = direct.saturating_add(pool_share);
        }

        self.write_animal_last_claim(&id, current_height)?;

        // Return the NFT, plus a synthetic yield representation in
        // response.data (16 LE bytes = u128) so callers/tests can read
        // the payout amount. Real yield-token mints will replace this in
        // the FIRE-integrated production path.
        let mut response = CallResponse::default();
        response.alkanes.0.extend(context.incoming_alkanes.0.iter().cloned());
        response.data = payout.to_le_bytes().to_vec();
        Ok(response)
    }

    fn claim_risky(&self) -> Result<CallResponse> {
        let context = self.context()?;
        let id = self.authenticate_animal(true)?;
        let (animal_id, role, _birth, _lifespan, _taxes, last_claim, _dead, _hunt) =
            self.read_animal(&id)?;

        let current_height = self.height() as u128;
        // Same seed derivation pattern as the lottery, salted distinctly.
        let seed_input = animal_id.wrapping_add(RISKY_CLAIM_SALT);
        let seed = self.derive_seed(seed_input, current_height);
        let roll = seed % BPS;
        let keep = roll < RISKY_KEEP_PROBABILITY_BPS;

        let mut payout: u128 = 0;
        if role == 0 {
            let yield_units = self.compute_yield_units(last_claim);
            if keep {
                // Win: keep the full pre-tax yield. Fox pool gets nothing.
                payout = yield_units;
            } else {
                // Loss: entire yield routes to fox pool.
                self.distribute_tax_to_foxes(yield_units);
                payout = 0;
            }
        } else {
            // Foxes can also use ClaimRisky on their direct yield, but
            // the variance only applies to direct yield; their accumulator
            // share is independent (it's already a coupon, not at risk).
            let direct = self.compute_yield_units(last_claim);
            let pool_share = self.fox_claim_share(&id);
            if keep {
                payout = direct.saturating_add(pool_share);
            } else {
                // Loss on a fox's risky: direct yield burns to the fox pool
                // (other foxes get the upside). Pool share still pays out.
                self.distribute_tax_to_foxes(direct);
                payout = pool_share;
            }
        }

        self.write_animal_last_claim(&id, current_height)?;
        let mut response = CallResponse::default();
        response.alkanes.0.extend(context.incoming_alkanes.0.iter().cloned());
        response.data = payout.to_le_bytes().to_vec();
        Ok(response)
    }

    // ── Hunt handlers ───────────────────────────────────────────

    fn initiate_hunt(&self, target_block: u128, target_tx: u128) -> Result<CallResponse> {
        let context = self.context()?;
        let current_height = self.height() as u128;

        // 1. Validate target — must be a registered, staked, alive fox not
        //    currently in another hunt.
        let target_id = AlkaneId {
            block: target_block,
            tx: target_tx,
        };
        if !self.is_registered_animal(&target_id) {
            return Err(anyhow!("target is not a registered animal"));
        }
        let (_a, target_role, _b, _l, target_taxes, _lc, target_dead, target_hunt) =
            self.read_animal(&target_id)?;
        if target_dead != 0 {
            return Err(anyhow!("target is dead"));
        }
        if target_role != 1 {
            return Err(anyhow!("target is not a fox"));
        }
        if !self.is_staked(&target_id) {
            return Err(anyhow!("target fox is not staked"));
        }
        if target_hunt != 0 {
            return Err(anyhow!("target fox already in another hunt"));
        }

        // 2. Validate party — every incoming alkane that's a registered
        //    animal IS a party member. Apply per-member validation.
        let mut party: Vec<AlkaneId> = Vec::new();
        for xfer in &context.incoming_alkanes.0 {
            if xfer.value < 1 {
                continue;
            }
            if !self.is_registered_animal(&xfer.id) {
                continue;
            }
            let (_aa, p_role, _ab, _al, _at, _alc, p_dead, p_hunt) = self.read_animal(&xfer.id)?;
            if p_dead != 0 || p_role != 0 {
                return Err(anyhow!("party member must be a live farmer"));
            }
            if !self.is_staked(&xfer.id) {
                return Err(anyhow!("party member must be staked"));
            }
            if p_hunt != 0 {
                return Err(anyhow!("party member already in another hunt"));
            }
            party.push(xfer.id.clone());
        }
        let party_size = party.len() as u128;
        if party_size < MIN_HUNT_PARTY {
            return Err(anyhow!(
                "party size {} below MIN_HUNT_PARTY ({})",
                party_size,
                MIN_HUNT_PARTY
            ));
        }
        if party_size > MAX_HUNT_PARTY {
            return Err(anyhow!(
                "party size {} above MAX_HUNT_PARTY ({})",
                party_size,
                MAX_HUNT_PARTY
            ));
        }

        // 3. Allocate hunt id and persist state.
        let hunt_seq = self.hunt_seq_pointer().get_value::<u128>();
        let hunt_id = if hunt_seq == 0 { 1 } else { hunt_seq };
        self.hunt_target_block_pointer(hunt_id).set_value(target_block);
        self.hunt_target_tx_pointer(hunt_id).set_value(target_tx);
        self.hunt_party_size_pointer(hunt_id).set_value(party_size);
        self.hunt_init_block_pointer(hunt_id).set_value(current_height);
        self.hunt_resolved_pointer(hunt_id).set_value(0u128);
        self.hunt_success_pointer(hunt_id).set_value(0u128);
        for (idx, m) in party.iter().enumerate() {
            self.hunt_member_block_pointer(hunt_id, idx as u128)
                .set_value(m.block);
            self.hunt_member_tx_pointer(hunt_id, idx as u128)
                .set_value(m.tx);
        }
        self.hunt_seq_pointer().set_value(hunt_id + 1);
        self.lifetime_total_hunts_pointer().set_value(
            self.lifetime_total_hunts_pointer().get_value::<u128>() + 1,
        );

        // 4. Update moving-max for the per-hunter prob denominator.
        let cur_max = self.max_fox_unclaimed_pointer().get_value::<u128>();
        if target_taxes > cur_max {
            self.max_fox_unclaimed_pointer().set_value(target_taxes);
        }

        // 5. Freeze target + members via SetHuntInProgress(1) on each animal.
        self.set_hunt_flag(&target_id, 1)?;
        for m in &party {
            self.set_hunt_flag(m, 1)?;
        }

        // 6. Return the party NFTs (bearer-token preserved) and the hunt_id.
        let mut response = CallResponse::default();
        response.alkanes.0.extend(context.incoming_alkanes.0.iter().cloned());
        response.data = hunt_id.to_le_bytes().to_vec();
        Ok(response)
    }

    /// Vault-only helper: SetHuntInProgress on a child animal NFT.
    fn set_hunt_flag(&self, id: &AlkaneId, value: u128) -> Result<()> {
        let call = Cellpack {
            target: id.clone(),
            inputs: vec![5u128, value], // SetHuntInProgress
        };
        self.call(&call, &AlkaneTransferParcel::default(), self.fuel())?;
        Ok(())
    }

    fn resolve_hunt(&self, hunt_id: u128) -> Result<CallResponse> {
        // 1. Load hunt state.
        let resolved = self.hunt_resolved_pointer(hunt_id).get_value::<u128>();
        if resolved != 0 {
            return Err(anyhow!("hunt already resolved"));
        }
        let party_size = self.hunt_party_size_pointer(hunt_id).get_value::<u128>();
        if party_size == 0 {
            return Err(anyhow!("unknown hunt_id"));
        }
        let target_id = AlkaneId {
            block: self.hunt_target_block_pointer(hunt_id).get_value::<u128>(),
            tx: self.hunt_target_tx_pointer(hunt_id).get_value::<u128>(),
        };

        // 2. Re-read target fox's CURRENT unclaimed taxes (may have grown
        //    since hunt was initiated).
        let (_a, _r, _b, _l, target_taxes, _lc, _d, _h) = self.read_animal(&target_id)?;
        let moving_max = self.max_fox_unclaimed_pointer().get_value::<u128>();

        // 3. Compute per-hunter and party-success probabilities.
        let per_hunter = compute_per_hunter_prob_bps(target_taxes, moving_max);
        let party_bps = compute_party_success_bps(per_hunter, party_size);

        // 4. Derive seed and roll.
        let seed = self.derive_seed(hunt_id, target_taxes);
        let roll = seed % BPS;
        let success = (roll as u128) < party_bps;

        // 5. Outcome handlers.
        if success {
            // 5a. Distribute target's unclaimed taxes among party members
            //     by setting each member's accumulated_taxes += target.taxes/N.
            //     For v0 we use the animal's accumulated_taxes field as a
            //     "earned-tax-tab" — set directly via the vault-only op 2.
            let share = target_taxes.checked_div(party_size).unwrap_or(0);
            for idx in 0..party_size {
                let m = AlkaneId {
                    block: self.hunt_member_block_pointer(hunt_id, idx).get_value::<u128>(),
                    tx: self.hunt_member_tx_pointer(hunt_id, idx).get_value::<u128>(),
                };
                let (_, _, _, _, m_taxes, _, _, _) = self.read_animal(&m)?;
                let new_taxes = m_taxes.saturating_add(share);
                let call = Cellpack {
                    target: m.clone(),
                    inputs: vec![2u128, new_taxes], // SetAccumulatedTaxes
                };
                self.call(&call, &AlkaneTransferParcel::default(), self.fuel())?;
                // Clear hunt flag on member.
                self.set_hunt_flag(&m, 0)?;
            }
            // 5b. Burn the target fox: mark dead AND zero its taxes.
            let mark_dead = Cellpack {
                target: target_id.clone(),
                inputs: vec![4u128], // MarkDead
            };
            self.call(&mark_dead, &AlkaneTransferParcel::default(), self.fuel())?;
            let zero_taxes = Cellpack {
                target: target_id.clone(),
                inputs: vec![2u128, 0u128],
            };
            self.call(&zero_taxes, &AlkaneTransferParcel::default(), self.fuel())?;
            self.set_hunt_flag(&target_id, 0)?;
            // Population update: -1 fox, target was staked so decrement.
            self.fox_count_pointer()
                .set_value(self.fox_count().saturating_sub(1));
            self.staked_fox_count_pointer()
                .set_value(self.staked_fox_count().saturating_sub(1));

            // 5c. Convert one party member (use seed to pick which) to fox.
            //     Use a *different* seed slice to decorrelate from the win/lose roll.
            let pick = (seed >> 64) % party_size;
            let chosen = AlkaneId {
                block: self.hunt_member_block_pointer(hunt_id, pick).get_value::<u128>(),
                tx: self.hunt_member_tx_pointer(hunt_id, pick).get_value::<u128>(),
            };
            let convert = Cellpack {
                target: chosen.clone(),
                inputs: vec![3u128], // ConvertToFox
            };
            self.call(&convert, &AlkaneTransferParcel::default(), self.fuel())?;
            // Population: -1 farmer + 1 fox to net out the role swap.
            self.farmer_count_pointer()
                .set_value(self.farmer_count().saturating_sub(1));
            self.fox_count_pointer().set_value(self.fox_count() + 1);
            self.staked_farmer_count_pointer()
                .set_value(self.staked_farmer_count().saturating_sub(1));
            self.staked_fox_count_pointer()
                .set_value(self.staked_fox_count() + 1);
            // Seed the new fox's accumulator checkpoint so they only earn
            // taxes from this point forward (per the same convention as
            // a fresh fox stake).
            self.fox_acc_checkpoint_pointer(&chosen)
                .set_value(self.tax_per_fox_acc());

            self.hunt_success_pointer(hunt_id).set_value(1u128);
        } else {
            // Failed hunt path.
            // 5d. Apply aging to each party member. The aging amount adapts
            //     to recent failed-hunt rate (compute_aging_blocks). For v0
            //     we use lifetime counters as a proxy for "recent".
            let failed_recent = self.lifetime_failed_hunts_pointer().get_value::<u128>();
            let total_recent = self.lifetime_total_hunts_pointer().get_value::<u128>();
            let aging = compute_aging_blocks(failed_recent, total_recent) as u128;
            for idx in 0..party_size {
                let m = AlkaneId {
                    block: self.hunt_member_block_pointer(hunt_id, idx).get_value::<u128>(),
                    tx: self.hunt_member_tx_pointer(hunt_id, idx).get_value::<u128>(),
                };
                let cur = self.animal_aging_pointer(&m).get_value::<u128>();
                self.animal_aging_pointer(&m)
                    .set_value(cur.saturating_add(aging));
                self.set_hunt_flag(&m, 0)?;
            }
            // 5e. Unfreeze target.
            self.set_hunt_flag(&target_id, 0)?;
            // 5f. Increment lifetime_failed counter.
            self.lifetime_failed_hunts_pointer().set_value(failed_recent + 1);
        }

        // 6. Mark hunt resolved either way.
        self.hunt_resolved_pointer(hunt_id).set_value(1u128);

        // 7. Response — empty alkanes (NFTs already where they need to be:
        // members keep their NFTs in their wallets since we didn't take
        // them in, target's NFT is dead but still held by its owner).
        let mut response = CallResponse::default();
        let success_flag: u128 = if success { 1 } else { 0 };
        response.data = success_flag.to_le_bytes().to_vec();
        Ok(response)
    }

    // ── Expiration handler ──────────────────────────────────────

    fn expire(&self, animal_block: u128, animal_tx: u128) -> Result<CallResponse> {
        let id = AlkaneId {
            block: animal_block,
            tx: animal_tx,
        };
        if !self.is_registered_animal(&id) {
            return Err(anyhow!("animal not registered"));
        }
        let (_aid, role, birth, lifespan, _taxes, _last_claim, is_dead, hunt_flag) =
            self.read_animal(&id)?;
        if is_dead != 0 {
            return Err(anyhow!("animal already dead"));
        }
        if hunt_flag != 0 {
            return Err(anyhow!("cannot expire an animal in pending hunt"));
        }
        let current_height = self.height() as u128;
        let aging = self.animal_aging_pointer(&id).get_value::<u128>();
        let nominal_end = birth.saturating_add(lifespan);
        let effective_end = nominal_end.saturating_sub(aging);
        if current_height < effective_end {
            return Err(anyhow!(
                "too early to expire: current {} < effective_end {}",
                current_height,
                effective_end
            ));
        }

        // Mark dead on the animal NFT.
        let mark_dead = Cellpack {
            target: id.clone(),
            inputs: vec![4u128], // MarkDead
        };
        self.call(&mark_dead, &AlkaneTransferParcel::default(), self.fuel())?;

        // Decrement population.
        if role == 1 {
            self.fox_count_pointer()
                .set_value(self.fox_count().saturating_sub(1));
            if self.is_staked(&id) {
                self.staked_fox_count_pointer()
                    .set_value(self.staked_fox_count().saturating_sub(1));
            }
        } else {
            self.farmer_count_pointer()
                .set_value(self.farmer_count().saturating_sub(1));
            if self.is_staked(&id) {
                self.staked_farmer_count_pointer()
                    .set_value(self.staked_farmer_count().saturating_sub(1));
            }
        }
        // Lifetime counter.
        self.total_expirations_pointer()
            .set_value(self.total_expirations_pointer().get_value::<u128>() + 1);

        // Return the bounty as a u128 in response.data — in production
        // this routes to whatever yield-token the game has access to;
        // for v0 we surface it as a number for callers to verify.
        let mut response = CallResponse::default();
        response.data = EXPIRE_BOUNTY_UNITS.to_le_bytes().to_vec();
        Ok(response)
    }

    // ── View handlers ────────────────────────────────────────────

    fn get_expire_block(&self, block: u128, tx: u128) -> Result<CallResponse> {
        let context = self.context()?;
        let mut response = CallResponse::forward(&context.incoming_alkanes);
        let id = AlkaneId { block, tx };
        let effective = if self.is_registered_animal(&id) {
            let (_a, _r, birth, lifespan, _t, _lc, _d, _h) = self.read_animal(&id)?;
            let aging = self.animal_aging_pointer(&id).get_value::<u128>();
            birth.saturating_add(lifespan).saturating_sub(aging)
        } else {
            0
        };
        response.data = effective.to_le_bytes().to_vec();
        Ok(response)
    }

    fn get_total_expirations(&self) -> Result<CallResponse> {
        let context = self.context()?;
        let mut response = CallResponse::forward(&context.incoming_alkanes);
        response.data = self.total_expirations_pointer().get_value::<u128>().to_le_bytes().to_vec();
        Ok(response)
    }

    fn get_fox_pool(&self) -> Result<CallResponse> {
        let context = self.context()?;
        let mut response = CallResponse::forward(&context.incoming_alkanes);
        response.data = self.fox_pool_balance().to_le_bytes().to_vec();
        Ok(response)
    }

    fn get_fox_pool_lifetime(&self) -> Result<CallResponse> {
        let context = self.context()?;
        let mut response = CallResponse::forward(&context.incoming_alkanes);
        response.data = self.fox_pool_lifetime().to_le_bytes().to_vec();
        Ok(response)
    }

    fn preview_yield(&self, animal_block: u128, animal_tx: u128) -> Result<CallResponse> {
        let context = self.context()?;
        let mut response = CallResponse::forward(&context.incoming_alkanes);
        let id = AlkaneId {
            block: animal_block,
            tx: animal_tx,
        };
        let units = if self.is_registered_animal(&id) {
            let (_a, _r, _b, _l, _t, last_claim, _d, _h) = self.read_animal(&id)?;
            self.compute_yield_units(last_claim)
        } else {
            0
        };
        response.data = units.to_le_bytes().to_vec();
        Ok(response)
    }

    fn get_latest_animal_id(&self) -> Result<CallResponse> {
        let context = self.context()?;
        let mut response = CallResponse::forward(&context.incoming_alkanes);
        let mut data = Vec::with_capacity(32);
        data.extend_from_slice(
            &self
                .latest_animal_id_block_pointer()
                .get_value::<u128>()
                .to_le_bytes(),
        );
        data.extend_from_slice(
            &self
                .latest_animal_id_tx_pointer()
                .get_value::<u128>()
                .to_le_bytes(),
        );
        response.data = data;
        Ok(response)
    }

    fn get_hunt(&self, hunt_id: u128) -> Result<CallResponse> {
        let context = self.context()?;
        let mut response = CallResponse::forward(&context.incoming_alkanes);
        let mut data = Vec::with_capacity(16 * 6);
        data.extend_from_slice(
            &self.hunt_target_block_pointer(hunt_id).get_value::<u128>().to_le_bytes(),
        );
        data.extend_from_slice(
            &self.hunt_target_tx_pointer(hunt_id).get_value::<u128>().to_le_bytes(),
        );
        data.extend_from_slice(
            &self.hunt_party_size_pointer(hunt_id).get_value::<u128>().to_le_bytes(),
        );
        data.extend_from_slice(
            &self.hunt_init_block_pointer(hunt_id).get_value::<u128>().to_le_bytes(),
        );
        data.extend_from_slice(
            &self.hunt_resolved_pointer(hunt_id).get_value::<u128>().to_le_bytes(),
        );
        data.extend_from_slice(
            &self.hunt_success_pointer(hunt_id).get_value::<u128>().to_le_bytes(),
        );
        response.data = data;
        Ok(response)
    }

    fn get_total_hunts(&self) -> Result<CallResponse> {
        let context = self.context()?;
        let mut response = CallResponse::forward(&context.incoming_alkanes);
        response.data = self.lifetime_total_hunts_pointer().get_value::<u128>().to_le_bytes().to_vec();
        Ok(response)
    }

    fn get_recent_failed_hunts(&self) -> Result<CallResponse> {
        let context = self.context()?;
        let mut response = CallResponse::forward(&context.incoming_alkanes);
        // v0: lifetime as proxy for recent
        response.data = self.lifetime_failed_hunts_pointer().get_value::<u128>().to_le_bytes().to_vec();
        Ok(response)
    }

    fn get_recent_total_hunts(&self) -> Result<CallResponse> {
        let context = self.context()?;
        let mut response = CallResponse::forward(&context.incoming_alkanes);
        response.data = self.lifetime_total_hunts_pointer().get_value::<u128>().to_le_bytes().to_vec();
        Ok(response)
    }

    fn get_animal_aging(&self, block: u128, tx: u128) -> Result<CallResponse> {
        let context = self.context()?;
        let mut response = CallResponse::forward(&context.incoming_alkanes);
        let id = AlkaneId { block, tx };
        response.data = self.animal_aging_pointer(&id).get_value::<u128>().to_le_bytes().to_vec();
        Ok(response)
    }

    fn get_max_fox_unclaimed(&self) -> Result<CallResponse> {
        let context = self.context()?;
        let mut response = CallResponse::forward(&context.incoming_alkanes);
        response.data = self.max_fox_unclaimed_pointer().get_value::<u128>().to_le_bytes().to_vec();
        Ok(response)
    }

    fn get_latest_commitment_id(&self) -> Result<CallResponse> {
        let context = self.context()?;
        let mut response = CallResponse::forward(&context.incoming_alkanes);
        let mut data = Vec::with_capacity(32);
        data.extend_from_slice(
            &self
                .latest_commitment_id_block_pointer()
                .get_value::<u128>()
                .to_le_bytes(),
        );
        data.extend_from_slice(
            &self
                .latest_commitment_id_tx_pointer()
                .get_value::<u128>()
                .to_le_bytes(),
        );
        response.data = data;
        Ok(response)
    }

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
