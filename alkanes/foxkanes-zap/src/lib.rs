//! foxkanes-zap — peripheral router (1inch-aggregator pattern).
//!
//! Converts arbitrary input alkanes (frBTC, DIESEL, or pre-existing
//! DIESEL/FIRE LP) to DIESEL/FIRE LP, then calls fire-bonding Op 1 (Bond)
//! to deposit the LP into FIRE treasury and receive a bond NFT, then
//! enters the Foxkanes lottery via foxkanes-game Op (EnterLottery),
//! forwarding both the bond NFT and the commitment receipt to the
//! original tx initiator.
//!
//! Architectural property: this contract is *replaceable*. If the AMM
//! pool slot map changes (per subfrost-brain's "Retired slots — DO NOT
//! USE" guidance), a v2 zap can be deployed alongside the original game
//! contract without invalidating any animal NFTs. The game itself
//! accepts only canonical DIESEL/FIRE LP, so the zap can be swapped out
//! freely.
//!
//! TODO: full implementation in next pass. AMM routing logic will read
//! the canonical AMM factory pool registry at runtime rather than
//! hardcoding pool ids, to survive AMM redeploys.

use alkanes_runtime::{declare_alkane, message::MessageDispatch, runtime::AlkaneResponder};
use alkanes_support::response::CallResponse;
use anyhow::Result;

#[derive(Default)]
pub struct FoxkanesZap(());

impl AlkaneResponder for FoxkanesZap {}

#[derive(MessageDispatch)]
enum FoxkanesZapMessage {
    #[opcode(0)]
    Initialize {
        game: u128,
        amm_factory_block: u128,
        amm_factory_tx: u128,
    },

    /// Zap-in: caller sends any supported input alkane in incoming_alkanes;
    /// zap converts → DIESEL/FIRE LP → bonds to FIRE → enters lottery →
    /// forwards bond NFT + commitment receipt back to caller in response.
    #[opcode(1)]
    ZapEnterLottery,
}

impl FoxkanesZap {
    fn initialize(
        &self,
        _game: u128,
        _amm_factory_block: u128,
        _amm_factory_tx: u128,
    ) -> Result<CallResponse> {
        Ok(CallResponse::default())
    }

    fn zap_enter_lottery(&self) -> Result<CallResponse> {
        Ok(CallResponse::default())
    }
}

declare_alkane! {
    impl AlkaneResponder for FoxkanesZap {
        type Message = FoxkanesZapMessage;
    }
}
