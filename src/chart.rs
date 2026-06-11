//! Chart (.1) parsing and difficulty selection.
//!
//! The .1 header is 14 slots of (u32 offset, u32 length); a non-empty slot is one
//! difficulty's event stream. SP difficulties occupy slots 0..4, DP slots 6..10 (see
//! DECISION D3 for the empirically confirmed map). Each event is 8 bytes:
//!   u32 time_ms, u8 type, u8 param, u16 value.
//! We walk one slot's events in order, tracking per-lane keysound assignments, and emit a
//! flat list of "sounding" events (a sample number to play at a time). Any difficulty
//! reconstructs the same song, so the renderer defaults to SPN.

use crate::bytes::{read_u16_le, read_u32_le};

use std::fs;
use std::path::Path;
use std::str::FromStr;

use anyhow::{Context, Result, ensure};

const SLOT_COUNT: usize = 14;             // .1 header slot count (offset/length pairs)
const EVENT_LEN: usize = 8;               // bytes per event
const TIME_END_SENTINEL: u32 = 0x7FFF_FFFF; // type-6 end marker carries this time; not real

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

impl Difficulty {
    /// The .1 header slot index for this difficulty (empirically confirmed; see DECISION D3).
    /// SP = 0..4, DP = 6..10 in N/H/A/B/L-reordered form: N H A B L.
    pub fn slot_index(self) -> usize {
        match self {
            Difficulty::SpNormal => 0,
            Difficulty::SpHyper => 1,
            Difficulty::SpAnother => 2,
            Difficulty::SpBeginner => 3,
            Difficulty::SpLeggendaria => 4,
            Difficulty::DpNormal => 6,
            Difficulty::DpHyper => 7,
            Difficulty::DpAnother => 8,
            Difficulty::DpBeginner => 9,
            Difficulty::DpLeggendaria => 10,
        }
    }
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

/// One scheduled keysound playback: play sample `sample_1based` at `time_ms`.
// fields are consumed by the mixer in Step 5; allow until that lands
#[allow(dead_code)]
pub struct Sounding {
    pub time_ms: u32,
    pub sample_1based: u16,
}

/// A parsed chart difficulty reduced to what the renderer needs.
// duration_ms is consumed by the mixer in Step 5; allow until then
#[allow(dead_code)]
pub struct Chart {
    pub events: Vec<Sounding>, // sounding events in chart (time) order
    pub duration_ms: u32,      // last real event time (excludes the end sentinel)
}

/// Parse one difficulty out of a .1 chart into a flat list of sounding events.
pub fn parse(chart_path: &Path, difficulty: Difficulty) -> Result<Chart> {
    let bytes_chart =
        fs::read(chart_path).with_context(|| format!("reading chart {}", chart_path.display()))?;
    ensure!(
        bytes_chart.len() >= SLOT_COUNT * EVENT_LEN,
        "chart too small for a {SLOT_COUNT}-slot header: {}",
        chart_path.display()
    );

    let slot = difficulty.slot_index();
    let offset_events = read_u32_le(&bytes_chart, slot * 8)? as usize;
    let length_events = read_u32_le(&bytes_chart, slot * 8 + 4)? as usize;
    ensure!(
        length_events > 0,
        "difficulty {difficulty:?} (slot {slot}) is not present in {}",
        chart_path.display()
    );
    ensure!(
        length_events % EVENT_LEN == 0,
        "event stream length {length_events} is not a multiple of {EVENT_LEN}"
    );
    let end_events = offset_events.checked_add(length_events).context("event range overflow")?;
    ensure!(
        end_events <= bytes_chart.len(),
        "event stream [{offset_events}..{end_events}] exceeds chart size {}",
        bytes_chart.len()
    );

    // current keysound assigned to each lane (param is a u8, so 256 lanes cover it); 0 = none
    let mut lane_to_sample = [0u16; 256];
    let mut events = Vec::new();
    let mut duration_ms = 0u32;

    let count_events = length_events / EVENT_LEN;
    for index_event in 0..count_events {
        let base = offset_events + index_event * EVENT_LEN;
        let time_ms = read_u32_le(&bytes_chart, base)?;
        let type_event = bytes_chart[base + 4];
        let param = bytes_chart[base + 5];
        let value = read_u16_le(&bytes_chart, base + 6)?;

        match type_event {
            // type 8 (initial) / type 2: assign keysound `value` to lane `param`
            8 | 2 => lane_to_sample[param as usize] = value,
            // type 7: auto-play keysound `value` at this time
            7 => events.push(Sounding { time_ms, sample_1based: value }),
            // type 0 (P1) / type 1 (P2) visible note: play the lane's currently assigned sample
            0 | 1 => {
                let sample = lane_to_sample[param as usize];
                if sample != 0 {
                    events.push(Sounding { time_ms, sample_1based: sample });
                }
            }
            // type 4/5/6/12/16: bar line / BPM / end / metadata — not audio
            _ => {}
        }

        if time_ms != TIME_END_SENTINEL {
            duration_ms = duration_ms.max(time_ms);
        }
    }

    Ok(Chart { events, duration_ms })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_chart() -> std::path::PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join(".sample/iidxOnEar/.sample/unpack/30000/30000.1")
    }

    #[test]
    fn parse_30000_spn() {
        let path_sample = sample_chart();
        if !path_sample.exists() {
            eprintln!("skipping parse_30000_spn: sample absent {}", path_sample.display());
            return;
        }

        let chart = parse(&path_sample, Difficulty::SpNormal).expect("parse 30000.1 SPN");

        // matches the offline probe: 114.65s, 2369 auto-plays + up to 1081 visible notes
        assert_eq!(chart.duration_ms, 114650, "SPN duration");
        assert!(
            chart.events.len() >= 2369 && chart.events.len() <= 3450,
            "sounding count {} out of expected range",
            chart.events.len()
        );
        assert!(
            chart.events.iter().all(|sounding| (1..=1186).contains(&sounding.sample_1based)),
            "all sample numbers within 1..=1186"
        );
        assert!(
            chart.events.windows(2).all(|pair| pair[0].time_ms <= pair[1].time_ms),
            "events should be in non-decreasing time order"
        );
    }
}
