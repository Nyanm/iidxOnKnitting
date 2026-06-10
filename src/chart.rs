//! Chart (.1) parsing and difficulty selection.
//!
//! The .1 header is 14 slots of (u32 offset, u32 length); a non-empty slot is one
//! difficulty's event stream. SP difficulties occupy the low slots, DP the high ones.
//! Full parsing and the exact slot<->difficulty mapping land in Step 3; for now this
//! defines the public `Difficulty` selector consumed by `render_song`.

use std::str::FromStr;

/// A chart difficulty. Order mirrors IIDX's canonical levels array
/// (SPB SPN SPH SPA SPL DPB DPN DPH DPA DPL). Any difficulty reconstructs the same
/// audio, but not every song has every difficulty — SPN exists for every song.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Difficulty {
    SpBeginner,
    SpNormal,
    SpHyper,
    SpAnother,
    SpLeggendaria,
    DpBeginner,
    DpNormal,
    DpHyper,
    DpAnother,
    DpLeggendaria,
}

impl FromStr for Difficulty {
    type Err = String;

    fn from_str(str_input: &str) -> Result<Self, Self::Err> {
        match str_input.to_ascii_uppercase().as_str() {
            "SPB" => Ok(Self::SpBeginner),
            "SPN" => Ok(Self::SpNormal),
            "SPH" => Ok(Self::SpHyper),
            "SPA" => Ok(Self::SpAnother),
            "SPL" => Ok(Self::SpLeggendaria),
            "DPB" => Ok(Self::DpBeginner),
            "DPN" => Ok(Self::DpNormal),
            "DPH" => Ok(Self::DpHyper),
            "DPA" => Ok(Self::DpAnother),
            "DPL" => Ok(Self::DpLeggendaria),
            other => Err(format!(
                "unknown difficulty {other:?} (expected one of SPB/SPN/SPH/SPA/SPL/DPB/DPN/DPH/DPA/DPL)"
            )),
        }
    }
}
