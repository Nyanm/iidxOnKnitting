//! BMP audio container (`bgm<id><mask>k.bin`, `i<id><dm|gf>.bin`) — GITADORA's backing tracks.
//!
/*
A 0x20-byte header followed by one Konami 4-bit ADPCM stream. The header MIXES ENDIANNESS:
data_size / loop_start / loop_end / sample_rate are big-endian, while channels / bits are
little-endian. That is not a guess — every field was checked against real files, and reading
sample_rate little-endian yields 0x80BB0000 instead of 48000.

The file name's four trailing characters before `.bin` are a mask of which parts are ALREADY mixed
into this track (`d` drum, `g` guitar, `b` bass, `_` absent; the fourth is always `k`). So
`bgm1800___k.bin` is backing only — the bed a full BMS-style render lays keysounds onto — and
`bgm1800d_bk.bin` already contains drums and bass. Mask handling is the caller's job; this module
only decodes.
*/

use crate::audio::adpcm;
use crate::tool::bytes::{read_u16_le, read_u32_be};

use anyhow::{Context, Result, bail, ensure};

const BMP_MAGIC: &[u8; 4] = b"BMP\0";
const HEADER_LEN: usize = 0x20;    // payload starts here
const DATA_SIZE_AT: usize = 0x04;  // u32(be) payload length
const LOOP_START_AT: usize = 0x08; // u32(be)
const LOOP_END_AT: usize = 0x0C;   // u32(be)
const CHANNELS_AT: usize = 0x10;   // u16(le)
const BITS_AT: usize = 0x12;       // u16(le)
const RATE_AT: usize = 0x14;       // u32(be)

/// A decoded BMP track: interleaved stereo `i16` at its native rate. Mono sources are duplicated
/// to both channels so callers never branch on channel count.
pub struct Track {
    pub samples: Vec<i16>, // interleaved stereo
    pub rate_hz: u32,
    pub loop_start: u32, // 0 when the track does not loop (all GITADORA bgm observed so far)
    pub loop_end: u32,
}

impl Track {
    /// Frame count (one frame = one L/R pair).
    pub fn frames(&self) -> usize {
        self.samples.len() / 2
    }
}

/// Decode a BMP container to interleaved stereo PCM.
pub fn decode(bytes_bmp: &[u8]) -> Result<Track> {
    ensure!(
        bytes_bmp.len() >= HEADER_LEN && &bytes_bmp[0..4] == BMP_MAGIC,
        "not a BMP audio container (magic {:02X?})",
        &bytes_bmp[0..bytes_bmp.len().min(4)]
    );

    let data_size = read_u32_be(bytes_bmp, DATA_SIZE_AT)? as usize;
    let channels = read_u16_le(bytes_bmp, CHANNELS_AT)?;
    let bits = read_u16_le(bytes_bmp, BITS_AT)?;
    let rate_hz = read_u32_be(bytes_bmp, RATE_AT)?;
    let loop_start = read_u32_be(bytes_bmp, LOOP_START_AT)?;
    let loop_end = read_u32_be(bytes_bmp, LOOP_END_AT)?;

    ensure!(bits == 16, "unexpected BMP bit depth {bits} (only 16 seen)");
    ensure!(rate_hz > 0, "BMP declares a zero sample rate");
    let end_payload = HEADER_LEN.checked_add(data_size).context("BMP payload range overflow")?;
    ensure!(
        end_payload <= bytes_bmp.len(),
        "BMP declares {data_size} payload bytes but the file holds only {}",
        bytes_bmp.len() - HEADER_LEN.min(bytes_bmp.len())
    );
    let payload = &bytes_bmp[HEADER_LEN..end_payload];

    let samples = match channels {
        1 => interleave_mono(&adpcm::decode_mono(payload)),
        2 => adpcm::decode_stereo(payload),
        other => bail!("unexpected BMP channel count {other} (only 1 and 2 seen)"),
    };

    Ok(Track { samples, rate_hz, loop_start, loop_end })
}

// Duplicate a mono track into both channels so `Track` is always interleaved stereo.
fn interleave_mono(mono: &[i16]) -> Vec<i16> {
    let mut samples = Vec::with_capacity(mono.len() * 2);
    for sample in mono {
        samples.push(*sample);
        samples.push(*sample);
    }
    samples
}
