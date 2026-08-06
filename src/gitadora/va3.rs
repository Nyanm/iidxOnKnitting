//! VA3W keysound archive (`spu<id>d.va3` for drums, `spu<id>g.va3` for guitar AND bass).
//!
/*
Header, a GDX block, a flat table of 0x40-byte entries, then one Konami ADPCM payload per entry.
Entries are looked up by their `sound_id`, which is exactly what an SQ3 note event carries — the
table order is irrelevant, so this module returns a sound_id -> Keysound map.

Two things here differ from fisyher/gitadora-customs, both settled on real data (memo m4f0#f0s2):
  - `filesize` is the EXACT payload length; the gap to the next entry's offset is merely 16-byte
    alignment padding. That tool reconstructs the length from the next offset for GDXH archives,
    which would fold the padding into the audio.
  - `volume` is a plain 0..127 linear value in every archive version. That tool maps it through a
    VOLUME_TABLE for version < 2. The pre-v2 drum banks do carry ~90 entries with volume 0, but
    those are precisely the slots no chart references (they index a MIDI-style drum map), so the
    table is not needed.

Guitar and bass share one archive and are separated only by sound_id range, so nothing here needs
to know which part a keysound belongs to.
*/

use crate::audio::adpcm;
use crate::tool::bytes::{read_u16_le, read_u32_le};

use std::collections::HashMap;

use anyhow::{Context, Result, bail, ensure};

const VA3_MAGIC: &[u8; 4] = b"VA3W";
const ENTRY_LEN: usize = 0x40;
const HEADER_END: usize = 0x1C; // last header field we read ends here

// header field offsets
const VERSION_AT: usize = 0x07;      // u8; 0 / 1 / 2 all occur
const ENTRY_COUNT_AT: usize = 0x08;  // u32
const GDX_SIZE_AT: usize = 0x0C;     // u32; 0x14 for GDXH, 0x18 for GDXG
const GDX_START_AT: usize = 0x10;    // u32
const ENTRY_START_AT: usize = 0x14;  // u32
const DATA_START_AT: usize = 0x18;   // u32

// within one entry
const OFFSET_AT: usize = 0x00;    // u32, relative to data_start
const FILESIZE_AT: usize = 0x04;  // u32, exact payload length
const CHANNELS_AT: usize = 0x08;  // u16
const BITS_AT: usize = 0x0A;      // u16
const RATE_AT: usize = 0x0C;      // u32
const VOLUME_AT: usize = 0x18;    // u8, 0..127 linear
const PAN_AT: usize = 0x19;       // u8, 64 = centre
const SOUND_ID_AT: usize = 0x1A;  // u16, what the chart references

const VOLUME_MAX: u8 = 127; // clamp; the field is a u8 but 127 is full scale
const ALIGNMENT: usize = 0x10; // payloads are padded to this, which the last entry may overrun

/// One keysound: decoded mono PCM plus the mix parameters stored alongside it.
pub struct Keysound {
    pub samples_mono: Vec<i16>,
    pub rate_hz: u32,
    pub volume: u8, // 0..127 linear
    pub pan: u8,    // 0..127, 64 = centre
}

/// A whole archive, indexed the way charts address it.
pub struct Archive {
    pub version: u8,
    pub map_keysound: HashMap<u16, Keysound>,
}

/// Parse a VA3W archive and decode every entry to mono PCM.
pub fn parse(bytes_va3: &[u8]) -> Result<Archive> {
    ensure!(
        bytes_va3.len() >= HEADER_END && &bytes_va3[0..4] == VA3_MAGIC,
        "not a VA3W keysound archive (magic {:02X?})",
        &bytes_va3[0..bytes_va3.len().min(4)]
    );

    let version = bytes_va3[VERSION_AT];
    let count_entries = read_u32_le(bytes_va3, ENTRY_COUNT_AT)? as usize;
    let gdx_size = read_u32_le(bytes_va3, GDX_SIZE_AT)? as usize;
    let gdx_start = read_u32_le(bytes_va3, GDX_START_AT)? as usize;
    let entry_start = read_u32_le(bytes_va3, ENTRY_START_AT)? as usize;
    let data_start = read_u32_le(bytes_va3, DATA_START_AT)? as usize;

    // the GDX block only carries default drum-pad sound ids, which a renderer never needs; we just
    // check it is where the header claims so a mis-parsed header fails loudly here
    ensure!(
        gdx_start.checked_add(gdx_size).is_some_and(|end| end == entry_start),
        "VA3 gdx block [{gdx_start:#x}..+{gdx_size:#x}] does not abut entry_start {entry_start:#x}"
    );
    let table_end = entry_start
        .checked_add(count_entries.checked_mul(ENTRY_LEN).context("VA3 entry table size overflow")?)
        .context("VA3 entry table range overflow")?;
    ensure!(
        table_end <= data_start && data_start <= bytes_va3.len(),
        "VA3 entry table ({count_entries} entries ending {table_end:#x}) does not fit before \
         data_start {data_start:#x} in a {}-byte file",
        bytes_va3.len()
    );

    let mut map_keysound = HashMap::with_capacity(count_entries);
    for index_entry in 0..count_entries {
        let base = entry_start + index_entry * ENTRY_LEN;
        let offset = read_u32_le(bytes_va3, base + OFFSET_AT)? as usize;
        let filesize = read_u32_le(bytes_va3, base + FILESIZE_AT)? as usize;
        let channels = read_u16_le(bytes_va3, base + CHANNELS_AT)?;
        let bits = read_u16_le(bytes_va3, base + BITS_AT)?;
        let rate_hz = read_u32_le(bytes_va3, base + RATE_AT)?;
        let sound_id = read_u16_le(bytes_va3, base + SOUND_ID_AT)?;

        ensure!(bits == 16, "VA3 entry {index_entry} declares {bits} bits (only 16 seen)");
        let start_payload = data_start.checked_add(offset).context("VA3 payload start overflow")?;
        let end_declared = start_payload.checked_add(filesize).context("VA3 payload end overflow")?;
        ensure!(
            start_payload <= bytes_va3.len(),
            "VA3 entry {index_entry} payload starts at {start_payload}, past the file's {} bytes",
            bytes_va3.len()
        );
        /* The last entry of a pre-v2 archive routinely declares more bytes than the file holds: the
        writer rounded `filesize` up to the 16-byte alignment it uses between payloads but did not
        emit that padding at EOF. Measured over the full library (1214165 entries in 2718 archives):
        365 entries overrun, EVERY one of them its archive's last entry and in a version-0 archive,
        by 1..16 bytes with a clean cutoff at 16. Clamping loses at most 32 samples (0.7 ms) of what
        was zero padding anyway, so erroring would reject ~40 songs over nothing. Both halves of the
        invariant are enforced, because a mis-parsed header would break the position rule even when
        its byte count happens to look plausible. */
        let end_payload = end_declared.min(bytes_va3.len());
        let overrun = end_declared - end_payload;
        ensure!(
            overrun == 0 || (index_entry + 1 == count_entries && overrun <= ALIGNMENT),
            "VA3 entry {index_entry} of {count_entries} payload [{start_payload}..{end_declared}] \
             overruns the {}-byte file by {overrun} bytes; only the last entry may overrun, and by \
             at most the {ALIGNMENT}-byte payload alignment",
            bytes_va3.len()
        );
        let payload = &bytes_va3[start_payload..end_payload];

        // every entry observed library-wide is mono; stereo is handled by keeping the left channel
        // so an odd archive degrades instead of failing the whole song
        let samples_mono = match channels {
            1 => adpcm::decode_mono(payload),
            2 => adpcm::decode_stereo(payload).into_iter().step_by(2).collect(),
            other => bail!("VA3 entry {index_entry} declares {other} channels (only 1 and 2 seen)"),
        };

        map_keysound.insert(
            sound_id,
            Keysound {
                samples_mono,
                rate_hz,
                volume: bytes_va3[base + VOLUME_AT].min(VOLUME_MAX),
                pan: bytes_va3[base + PAN_AT],
            },
        );
    }

    Ok(Archive { version, map_keysound })
}
