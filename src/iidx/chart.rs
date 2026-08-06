//! IIDX chart (`.1`) parsing and difficulty selection. GITADORA's charts are a different format
//! entirely — see [`crate::gitadora::sq3`].
//!
//! The .1 header is 14 slots of (u32 offset, u32 length); a non-empty slot is one
//! difficulty's event stream. SP difficulties occupy slots 0..4, DP slots 6..10 (see
//! DECISION D3 for the empirically confirmed map). Each event is 8 bytes:
//!   u32 time_ms, u8 type, u8 param, u16 value.
//! We walk one slot's events in order, tracking per-lane keysound assignments, and emit a
//! flat list of "sounding" events (a sample number to play at a time). Any difficulty
//! reconstructs the same song, so the renderer defaults to SPN.

use crate::tool::bytes::{read_u16_le, read_u32_le};

use anyhow::{Context, Result, ensure};

const SLOT_COUNT: usize = 14;             // .1 header slot count (offset/length pairs)
const EVENT_LEN: usize = 8;               // bytes per event
const TIME_END_SENTINEL: u32 = 0x7FFF_FFFF; // type-6 end marker carries this time; not real

/// A chart difficulty. Order mirrors IIDX's canonical levels array
/// (SPB SPN SPH SPA SPL DPB DPN DPH DPA DPL). Any difficulty reconstructs the same
/// audio, but not every song has every difficulty — SPN exists for every song.
/// Derives `ValueEnum` so the CLI accepts the enum directly (value names spb/spn/.../dpl).
#[derive(Clone, Copy, Debug, PartialEq, Eq, clap::ValueEnum)]
pub enum Difficulty {
    #[value(name = "spb")]
    SpBeginner,
    #[value(name = "spn")]
    SpNormal,
    #[value(name = "sph")]
    SpHyper,
    #[value(name = "spa")]
    SpAnother,
    #[value(name = "spl")]
    SpLeggendaria,
    #[value(name = "dpb")]
    DpBeginner,
    #[value(name = "dpn")]
    DpNormal,
    #[value(name = "dph")]
    DpHyper,
    #[value(name = "dpa")]
    DpAnother,
    #[value(name = "dpl")]
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

/// One scheduled keysound playback: play sample `sample_1based` at `time_ms`.
pub struct Sounding {
    pub time_ms: u32,
    pub sample_1based: u16,
}

/// A parsed chart difficulty reduced to what the renderer needs.
pub struct Chart {
    pub events: Vec<Sounding>, // sounding events in chart (time) order
    pub duration_ms: u32,      // last real event time (excludes the end sentinel)
}

/// Chart event kind, decoded from the raw u8 type field. Only the audio-relevant kinds plus the
/// end-of-chart marker are named; bar line / BPM / metadata (types 4/5/12/16) collapse into `Other`.
enum EventType {
    VisibleNoteP1, // 0: P1 visible note (plays its lane's assigned sample)
    VisibleNoteP2, // 1: P2 visible note (DP)
    AssignLane,    // 2: assign keysound `value` to lane `param`
    End,           // 6: end of chart — events after it are padding (sentinel/junk times)
    AutoPlay,      // 7: auto-play keysound `value`
    InitialAssign, // 8: initial lane->sample assignment at song start
    Other,         // 4/5/12/16 etc.: not audio
}

impl EventType {
    fn from_u8(raw: u8) -> Self {
        match raw {
            0 => Self::VisibleNoteP1,
            1 => Self::VisibleNoteP2,
            2 => Self::AssignLane,
            6 => Self::End,
            7 => Self::AutoPlay,
            8 => Self::InitialAssign,
            _ => Self::Other,
        }
    }
}

/// Parse one difficulty out of a .1 chart (raw bytes) into a flat list of sounding events.
pub fn parse(bytes_chart: &[u8], difficulty: Difficulty) -> Result<Chart> {
    ensure!(
        bytes_chart.len() >= SLOT_COUNT * EVENT_LEN,
        "chart too small for a {SLOT_COUNT}-slot header"
    );

    let slot = difficulty.slot_index();
    let offset_events = read_u32_le(bytes_chart, slot * 8)? as usize;
    let length_events = read_u32_le(bytes_chart, slot * 8 + 4)? as usize;
    ensure!(
        length_events > 0,
        "difficulty {difficulty:?} (slot {slot}) is not present in this chart"
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
        let time_ms = read_u32_le(bytes_chart, base)?;
        if time_ms == TIME_END_SENTINEL { continue; }

        let type_event = EventType::from_u8(bytes_chart[base + 4]);
        let param = bytes_chart[base + 5];
        let value = read_u16_le(bytes_chart, base + 6)?;

        match type_event {
            // assign keysound `value` to lane `param` (initial or mid-song)
            EventType::InitialAssign | EventType::AssignLane => {
                lane_to_sample[param as usize] = value;
            }
            // auto-play keysound `value` at this time (0 = "no sound", skip — as visible notes do)
            EventType::AutoPlay => {
                if value != 0 {
                    events.push(Sounding { time_ms, sample_1based: value });
                }
            }
            // visible note: play the lane's currently assigned sample
            EventType::VisibleNoteP1 | EventType::VisibleNoteP2 => {
                let sample = lane_to_sample[param as usize];
                if sample != 0 {
                    events.push(Sounding { time_ms, sample_1based: sample });
                }
            }
            // end of chart: take its time as the song length and stop. Any events after it are
            // padding stamped with sentinel / near-sentinel times (e.g. 15302's 0x7FFFFF37) that
            // the exact-sentinel skip above misses and that would blow the timeline up to ~757 GB.
            EventType::End => {
                duration_ms = duration_ms.max(time_ms);
                break;
            }
            // bar line / BPM / metadata — not audio
            EventType::Other => {}
        }

        duration_ms = duration_ms.max(time_ms);
    }

    Ok(Chart { events, duration_ms })
}
