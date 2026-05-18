//! Test helpers — common cellpack construction, deployment, response
//! extraction. Modeled on fire-misha/src/tests/helpers.rs which uses
//! `simulate_cellpack` for view queries (clean response.data) and
//! `index_block` for mutating txs.

pub use alkanes::indexer::index_block;
pub use alkanes::view;
pub use alkanes::view::simulate_parcel;
pub use alkanes_support::cellpack::Cellpack;
pub use alkanes_support::envelope::RawEnvelope;
pub use alkanes_support::id::AlkaneId;
pub use alkanes_support::response::ExtendedCallResponse;
pub use anyhow::Result;
pub use bitcoin::address::NetworkChecked;
pub use bitcoin::blockdata::transaction::OutPoint;
pub use bitcoin::{transaction::Version, ScriptBuf, Sequence};
pub use bitcoin::{Address, Amount, Block, Transaction, TxIn, TxOut, Witness};
pub use metashrew_core::index_pointer::AtomicPointer;
pub use ordinals::Runestone;
pub use protorune::message::MessageContextParcel;
pub use protorune::protostone::Protostones;
pub use protorune::test_helpers::{create_block_with_coinbase_tx, get_address, ADDRESS1};
pub use protorune_support::balance_sheet::BalanceSheet;
pub use protorune_support::protostone::{Protostone, ProtostoneEdict};
pub use std::str::FromStr;

pub use protorune_support::network::{set_network, NetworkParams};

/// Configure network parameters for regtest.
pub fn configure_network() {
    set_network(NetworkParams {
        bech32_prefix: String::from("bcrt"),
        p2pkh_prefix: 0x64,
        p2sh_prefix: 0xc4,
    });
}

/// Clear and re-prime the test environment. Indexes empty blocks to height
/// 2 so block-height queries don't underflow.
pub fn clear_test_environment() {
    metashrew_core::clear();
    configure_network();
    for height in 0..3 {
        let block = create_block_with_coinbase_tx(height);
        index_block(&block, height).expect("Failed to index empty block");
    }
}

/// Static-call a cellpack — no chain mutation, no edicts. Returns the
/// `ExtendedCallResponse` so tests can assert against `response.data`
/// directly without trace decoding.
pub fn simulate_cellpack(height: u64, cellpack: Cellpack) -> Result<(ExtendedCallResponse, u64)> {
    let parcel = MessageContextParcel {
        atomic: AtomicPointer::default(),
        runes: vec![],
        transaction: Transaction {
            version: bitcoin::blockdata::transaction::Version::ONE,
            input: vec![],
            output: vec![],
            lock_time: bitcoin::absolute::LockTime::ZERO,
        },
        block: create_block_with_coinbase_tx(height as u32),
        height,
        pointer: 0,
        refund_pointer: 0,
        calldata: cellpack.encipher(),
        sheets: Box::<BalanceSheet<AtomicPointer>>::new(BalanceSheet::default()),
        txindex: 0,
        vout: 0,
        runtime_balances: Box::<BalanceSheet<AtomicPointer>>::new(BalanceSheet::default()),
    };
    simulate_parcel(&parcel, u64::MAX)
}

/// Create a transaction that wraps multiple cellpacks (called by deploy
/// helpers below). Adapted from fire-misha's helpers.
pub fn create_multiple_cellpack_with_witness(
    witness: Witness,
    cellpacks: Vec<Cellpack>,
    etch: bool,
) -> Transaction {
    let txin = TxIn {
        previous_output: OutPoint::null(),
        script_sig: ScriptBuf::new(),
        sequence: Sequence::MAX,
        witness,
    };
    create_multiple_cellpack_with_witness_and_txins_edicts(cellpacks, vec![txin], etch, vec![])
}

pub fn create_multiple_cellpack_with_witness_and_in(
    witness: Witness,
    cellpacks: Vec<Cellpack>,
    previous_output: OutPoint,
    etch: bool,
) -> Transaction {
    let txin = TxIn {
        previous_output,
        script_sig: ScriptBuf::new(),
        sequence: Sequence::MAX,
        witness,
    };
    create_multiple_cellpack_with_witness_and_txins_edicts(cellpacks, vec![txin], etch, vec![])
}

pub fn create_multiple_cellpack_with_witness_and_txins_edicts(
    cellpacks: Vec<Cellpack>,
    txins: Vec<TxIn>,
    etch: bool,
    edicts: Vec<ProtostoneEdict>,
) -> Transaction {
    use ordinals::{Etching, Rune};
    let protocol_id = 1;
    let protostones: Vec<Protostone> = cellpacks
        .into_iter()
        .map(|cellpack| Protostone {
            message: cellpack.encipher(),
            pointer: Some(0),
            refund: Some(0),
            edicts: edicts.clone(),
            from: None,
            burn: None,
            protocol_tag: protocol_id,
        })
        .collect();

    let etching = if etch {
        Some(Etching {
            rune: Some(Rune(0)),
            divisibility: Some(0),
            premine: Some(21_000_000),
            spacers: Some(0),
            symbol: Some('X'),
            terms: None,
            turbo: false,
        })
    } else {
        None
    };

    let runestone: ScriptBuf = (Runestone {
        etching,
        pointer: Some(0),
        edicts: Vec::new(),
        mint: None,
        protocol: protostones.encipher().ok(),
    })
    .encipher();

    let address: Address<NetworkChecked> = get_address(&ADDRESS1().as_str());
    let op_return = TxOut {
        value: Amount::from_sat(0),
        script_pubkey: runestone,
    };
    let recipient_output = TxOut {
        value: Amount::from_sat(100_000_000),
        script_pubkey: address.script_pubkey(),
    };

    let inputs = if txins.is_empty() {
        vec![TxIn {
            previous_output: OutPoint::null(),
            script_sig: ScriptBuf::new(),
            sequence: Sequence::MAX,
            witness: Witness::new(),
        }]
    } else {
        txins
    };

    Transaction {
        version: bitcoin::blockdata::transaction::Version::ONE,
        lock_time: bitcoin::absolute::LockTime::ZERO,
        input: inputs,
        output: vec![recipient_output, op_return],
    }
}

/// Create a deployment block (wasm in tx witness, cellpack in OP_RETURN).
pub fn create_deployment_block(
    height: u32,
    wasm_bytes: &[u8],
    cellpack: Cellpack,
) -> Block {
    let mut block = create_block_with_coinbase_tx(height);
    let witness = RawEnvelope::from(wasm_bytes.to_vec()).to_witness(true);
    let tx = create_multiple_cellpack_with_witness(witness, vec![cellpack], false);
    block.txdata.push(tx);
    block
}

/// Create an operation block (no wasm, just a cellpack call).
pub fn create_operation_block(
    height: u32,
    cellpack: Cellpack,
    input_outpoint: Option<OutPoint>,
) -> Block {
    let mut block = create_block_with_coinbase_tx(height);
    let tx = if let Some(outpoint) = input_outpoint {
        create_multiple_cellpack_with_witness_and_in(
            Witness::new(),
            vec![cellpack],
            outpoint,
            false,
        )
    } else {
        create_multiple_cellpack_with_witness(Witness::new(), vec![cellpack], false)
    };
    block.txdata.push(tx);
    block
}

/// Parse the first 16 LE bytes of response.data as a u128.
pub fn parse_u128(data: &[u8]) -> Result<u128> {
    if data.len() < 16 {
        return Err(anyhow::anyhow!("response too short: {} bytes", data.len()));
    }
    Ok(u128::from_le_bytes(data[0..16].try_into()?))
}

/// Parse 32 LE bytes of response.data as an AlkaneId (block then tx).
pub fn parse_alkane_id(data: &[u8]) -> Result<AlkaneId> {
    if data.len() < 32 {
        return Err(anyhow::anyhow!(
            "alkane id response too short: {} bytes",
            data.len()
        ));
    }
    Ok(AlkaneId {
        block: u128::from_le_bytes(data[0..16].try_into()?),
        tx: u128::from_le_bytes(data[16..32].try_into()?),
    })
}

/// Parse `GetAllDetails` packed response — N consecutive u128 LE values.
pub fn parse_packed_u128s(data: &[u8], count: usize) -> Result<Vec<u128>> {
    let needed = 16 * count;
    if data.len() < needed {
        return Err(anyhow::anyhow!(
            "packed response too short: {} bytes, need {}",
            data.len(),
            needed
        ));
    }
    let mut out = Vec::with_capacity(count);
    for i in 0..count {
        let start = i * 16;
        out.push(u128::from_le_bytes(data[start..start + 16].try_into()?));
    }
    Ok(out)
}

#[macro_export]
macro_rules! test_log {
    ($($arg:tt)*) => {
        let _ = ::std::format!($($arg)*);
    };
}
