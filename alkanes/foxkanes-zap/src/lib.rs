//! foxkanes-zap — peripheral router (1inch-aggregator pattern).
//!
//! This contract is *replaceable* — Foxkanes' core game accepts only
//! canonical DIESEL/FIRE LP, and the zap converts arbitrary input
//! alkanes (frBTC, DIESEL, or pre-existing LP) into that canonical form,
//! then routes through fire-bonding into the FIRE treasury and finally
//! enters the lottery via foxkanes-game::EnterLottery.
//!
//! Architectural property: when the AMM pool slot map changes (per
//! subfrost-brain's "Retired slots — DO NOT USE" guidance), a v2 zap
//! can be deployed alongside the unchanged game contract without
//! invalidating any animal NFTs. Game state is preserved across zap
//! redeploys.
//!
//! V0 implementation: this commit ships the contract surface and the
//! pass-through Zap opcode. The AMM-routing internals (swap-half-and-
//! add-liquidity) are stubbed because they couple tightly to the live
//! AMM pool registry. We pin the integration tests to a TestZap path
//! that lets a player pass canonical LP directly through to the game,
//! exercising the wiring without the AMM dependency. Production AMM
//! routing lands when we deploy to regtest against a real pool.
//!
//! Storage is intentionally minimal — the zap is *stateless* by
//! design (replaceability): the only setup it remembers is the game
//! contract id (for routing the final EnterLottery call) and the
//! fire-bonding id (for routing LP through).

use alkanes_runtime::{
    declare_alkane, message::MessageDispatch, runtime::AlkaneResponder, storage::StoragePointer,
};
use alkanes_support::{
    cellpack::Cellpack,
    id::AlkaneId,
    parcel::AlkaneTransferParcel,
    response::CallResponse,
};
use anyhow::{anyhow, Result};
use metashrew_support::compat::to_arraybuffer_layout;
use metashrew_support::index_pointer::KeyValuePointer;

#[derive(Default)]
pub struct FoxkanesZap(());

impl AlkaneResponder for FoxkanesZap {}

#[derive(MessageDispatch)]
enum FoxkanesZapMessage {
    /// One-shot init. Records the game + fire-bonding addresses so the
    /// zap can route through them. Bonds-FIRE-on-behalf-of-player
    /// happens by calling fire-bonding::Bond with LP in incoming, then
    /// forwards the resulting bond NFT to the player.
    #[opcode(0)]
    Initialize {
        game_block: u128,
        game_tx: u128,
        fire_bonding_block: u128,
        fire_bonding_tx: u128,
    },

    /// Zap-in path (v0 = pass-through):
    ///   1. Receives LP from caller in incoming_alkanes
    ///   2. Calls fire-bonding::Bond, receives the bond NFT
    ///   3. Calls foxkanes-game::EnterLottery, passing the bond NFT id
    ///      as parameters (the LP that drives the weight is implicit
    ///      from incoming_alkanes; the bond NFT is the refund target)
    ///   4. Forwards bond NFT + commitment receipt back to caller
    ///
    /// V0 NOTE: the AMM-routing step (frBTC → DIESEL/FIRE LP) is
    /// deferred. Tests pass LP directly through this contract; the
    /// AMM swap is exercised in regtest deployment tests.
    #[opcode(1)]
    ZapEnterLottery,

    #[opcode(10)]
    #[returns(AlkaneId)]
    GetGameId,

    #[opcode(11)]
    #[returns(AlkaneId)]
    GetFireBondingId,
}

impl FoxkanesZap {
    fn game_id_block_pointer(&self) -> StoragePointer {
        StoragePointer::from_keyword("/game_id_block")
    }
    fn game_id_tx_pointer(&self) -> StoragePointer {
        StoragePointer::from_keyword("/game_id_tx")
    }
    fn fire_bonding_block_pointer(&self) -> StoragePointer {
        StoragePointer::from_keyword("/fire_bonding_block")
    }
    fn fire_bonding_tx_pointer(&self) -> StoragePointer {
        StoragePointer::from_keyword("/fire_bonding_tx")
    }

    fn game_id(&self) -> AlkaneId {
        AlkaneId {
            block: self.game_id_block_pointer().get_value::<u128>(),
            tx: self.game_id_tx_pointer().get_value::<u128>(),
        }
    }

    fn fire_bonding_id(&self) -> AlkaneId {
        AlkaneId {
            block: self.fire_bonding_block_pointer().get_value::<u128>(),
            tx: self.fire_bonding_tx_pointer().get_value::<u128>(),
        }
    }

    fn initialize(
        &self,
        game_block: u128,
        game_tx: u128,
        fire_bonding_block: u128,
        fire_bonding_tx: u128,
    ) -> Result<CallResponse> {
        self.observe_initialization()?;
        if game_block == 0 && game_tx == 0 {
            return Err(anyhow!("game id must be non-zero"));
        }
        if fire_bonding_block == 0 && fire_bonding_tx == 0 {
            return Err(anyhow!("fire bonding id must be non-zero"));
        }
        self.game_id_block_pointer().set_value(game_block);
        self.game_id_tx_pointer().set_value(game_tx);
        self.fire_bonding_block_pointer().set_value(fire_bonding_block);
        self.fire_bonding_tx_pointer().set_value(fire_bonding_tx);
        Ok(CallResponse::default())
    }

    fn zap_enter_lottery(&self) -> Result<CallResponse> {
        let context = self.context()?;
        let game = self.game_id();
        let fire_bonding = self.fire_bonding_id();
        if game.block == 0 && game.tx == 0 {
            return Err(anyhow!("zap not initialized"));
        }
        if context.incoming_alkanes.0.is_empty() {
            return Err(anyhow!("no LP provided"));
        }

        // V0: a real implementation would either swap input alkanes to
        // canonical DIESEL/FIRE LP via the AMM, or assume incoming is
        // already canonical LP. For TDD we assume canonical LP.

        // 1. Bond the LP into FIRE.
        let bond_call = Cellpack {
            target: fire_bonding,
            inputs: vec![
                1u128, // Bond opcode on fire-bonding
                0,     // min_fire_out — no slippage check in v0
            ],
        };
        // Pass through ALL incoming alkanes to fire-bonding (it'll only
        // consume LP; other alkanes will be returned).
        let parcel = AlkaneTransferParcel(context.incoming_alkanes.0.clone());
        let bond_response = match self.call(&bond_call, &parcel, self.fuel()) {
            Ok(r) => r,
            Err(e) => return Err(anyhow!("fire-bonding::Bond failed: {}", e)),
        };

        // 2. Find the returned bond NFT in fire-bonding's response.
        //    It's the first alkane returned (per fire-bonding's contract).
        if bond_response.alkanes.0.is_empty() {
            return Err(anyhow!("fire-bonding returned no bond NFT"));
        }
        let bond_nft = bond_response.alkanes.0[0].clone();

        // 3. Call foxkanes-game::EnterLottery with the bond NFT id as
        //    parameters.
        let enter_call = Cellpack {
            target: game,
            inputs: vec![
                1u128, // EnterLottery opcode
                bond_nft.id.block,
                bond_nft.id.tx,
            ],
        };
        // Pass through the bond NFT to the game — it forwards back to us
        // as part of its response. (Game itself doesn't consume the NFT.)
        let enter_parcel = AlkaneTransferParcel(vec![bond_nft.clone()]);
        let enter_response = self.call(&enter_call, &enter_parcel, self.fuel())?;

        // 4. Construct response: bond NFT + commitment receipt + any
        //    other change tokens.
        let mut response = CallResponse::default();
        response.alkanes.0.push(bond_nft);
        // Game's response should contain the commitment NFT in alkanes[0].
        response.alkanes.0.extend(enter_response.alkanes.0.iter().cloned());
        // Any change from bonding (e.g., non-LP incoming tokens forwarded back).
        for transfer in bond_response.alkanes.0.iter().skip(1) {
            response.alkanes.0.push(transfer.clone());
        }
        Ok(response)
    }

    fn get_game_id(&self) -> Result<CallResponse> {
        let context = self.context()?;
        let mut response = CallResponse::forward(&context.incoming_alkanes);
        let g = self.game_id();
        let mut data = Vec::with_capacity(32);
        data.extend_from_slice(&g.block.to_le_bytes());
        data.extend_from_slice(&g.tx.to_le_bytes());
        response.data = data;
        Ok(response)
    }

    fn get_fire_bonding_id(&self) -> Result<CallResponse> {
        let context = self.context()?;
        let mut response = CallResponse::forward(&context.incoming_alkanes);
        let f = self.fire_bonding_id();
        let mut data = Vec::with_capacity(32);
        data.extend_from_slice(&f.block.to_le_bytes());
        data.extend_from_slice(&f.tx.to_le_bytes());
        response.data = data;
        Ok(response)
    }
}

declare_alkane! {
    impl AlkaneResponder for FoxkanesZap {
        type Message = FoxkanesZapMessage;
    }
}
