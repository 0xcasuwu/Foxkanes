//! WASM vendor — include_bytes! of every Foxkanes alkane crate.
//! Refresh with `./scripts/build-wasms.sh` whenever a contract changes.

pub fn get_foxkanes_animal_wasm_bytes() -> &'static [u8] {
    include_bytes!("./wasm/foxkanes_animal.wasm")
}

pub fn get_foxkanes_commitment_wasm_bytes() -> &'static [u8] {
    include_bytes!("./wasm/foxkanes_commitment.wasm")
}

pub fn get_foxkanes_game_wasm_bytes() -> &'static [u8] {
    include_bytes!("./wasm/foxkanes_game.wasm")
}

pub fn get_foxkanes_zap_wasm_bytes() -> &'static [u8] {
    include_bytes!("./wasm/foxkanes_zap.wasm")
}
