//! SEQP / SQ3T charts (`d<id>.sq3` holds the drum charts, `g<id>.sq3` the guitar and bass ones).
//!
/*
A SEQP container holds several 0x10-byte-headed chunks, each an SQ3T chart: one "metadata" chunk
(bar lines / BPM / measures) plus one chunk per (part, difficulty). Every event is 0x40 bytes; the
only audio-relevant kind is `note` (0x10), which names the VA3 sound_id to play plus a velocity.
There is no lane -> keysound state machine at all, unlike IIDX: each note names its own sound_id.

We UNION the notes of every chunk, deduplicating on (tick, sound_id), rather than picking one
difficulty. Two measurements forced that (memo m4f0#f0s7, 197 songs scanned):
  - difficulties of one part are NOT interchangeable in 45 of 582 parts (7.7%). m0000's drum
    difficulties hold 400 / 400 / 169 notes; unioning recovers 1408 notes across the sample that
    the richest single difficulty misses.
  - 22 "metadata" chunks carry 1666 notes between them, and those sound_ids resolve in the file's
    VA3 archive — they are auto-play sounds, not bookkeeping. Skipping metadata chunks (as the
    reference implementation does) silently drops that audio.
Deduplication makes the union safe: the difficulties overwhelmingly repeat the same (tick, sound_id)
pairs, and two identical samples at one instant would only double that sound's level.

A metadata chunk's `game_type` field is meaningless (it reads 0/drum even inside `g<id>.sq3`), so
its notes are reported separately as `vec_note_auto` and belong to whichever VA3 sits beside the
chart file.
*/

use crate::tool::bytes::{read_u16_le, read_u32_le};

use std::collections::HashSet;

use anyhow::{Context, Result, ensure};

const SEQP_MAGIC: &[u8; 4] = b"SEQP";
const SQ3T_MAGIC: &[u8; 4] = b"SQ3T";

// SEQP container
const DATA_OFFSET_AT: usize = 0x10; // u32, where the first chunk starts
const MUSIC_ID_AT: usize = 0x14;    // u32
const CHART_COUNT_AT: usize = 0x18; // u32
const SEQP_HEADER_END: usize = 0x1C;
const CHUNK_HEADER_LEN: usize = 0x10; // u32 data_size + u32 0x10 + 8 reserved bytes

// SQ3T chunk, relative to the chunk body
const HEADER_SIZE_AT: usize = 0x0C;   // u32, where events begin
const EVENT_COUNT_AT: usize = 0x10;   // u32
const IS_METADATA_AT: usize = 0x15;   // u8
const GAME_TYPE_AT: usize = 0x17;     // u8
const TIME_DIVISION_AT: usize = 0x18; // u16, ticks per second
const EVENT_SIZE_AT: usize = 0x1C;    // u32, 0x40 everywhere observed
const SQ3T_HEADER_END: usize = 0x20;

// event block
const TICK_AT: usize = 0x00;      // u32
const EVENT_ID_AT: usize = 0x04;  // u8
const SOUND_ID_AT: usize = 0x20;  // u32, but only the low u16 is used (matches a VA3 sound_id)
const VOLUME_AT: usize = 0x2D;    // u8, per-note velocity

const EVENT_ID_END_POSITION: u8 = 0x0F; // marks the chart's end time
const EVENT_ID_NOTE: u8 = 0x10;         // the only kind that makes sound
const VOLUME_MAX: u8 = 127;

const GAME_TYPE_DRUM: u8 = 0;
const GAME_TYPE_GUITAR: u8 = 1;
const GAME_TYPE_BASS: u8 = 2;

/// One scheduled keysound playback.
pub struct Note {
    pub tick: u32,
    pub sound_id: u16,
    pub volume: u8, // 0..127 linear, multiplied with the VA3 entry's own volume
}

/// Every sounding note in one `.sq3`, split by the part it belongs to. Note sets are already
/// unioned across difficulties and deduplicated.
pub struct Sq3 {
    pub music_id: u32,
    pub ticks_per_second: u32,
    pub end_tick: u32,
    pub vec_note_drum: Vec<Note>,
    pub vec_note_guitar: Vec<Note>,
    pub vec_note_bass: Vec<Note>,
    pub vec_note_auto: Vec<Note>, // from metadata chunks; part unknown, VA3 is the file's own
}

impl Sq3 {
    /// Chart length in milliseconds, from the end-position event (or the last note).
    pub fn duration_ms(&self) -> u32 {
        (self.end_tick as u64 * 1000 / self.ticks_per_second as u64) as u32
    }

    /// Total notes across every part, for logging.
    pub fn count_note(&self) -> usize {
        self.vec_note_drum.len()
            + self.vec_note_guitar.len()
            + self.vec_note_bass.len()
            + self.vec_note_auto.len()
    }
}

// One part's accumulating note list plus the dedup key set.
#[derive(Default)]
struct Bucket {
    vec_note: Vec<Note>,
    seen: HashSet<(u32, u16)>,
}

impl Bucket {
    fn push(&mut self, note: Note) {
        if self.seen.insert((note.tick, note.sound_id)) {
            self.vec_note.push(note);
        }
    }
}

/// Parse a `.sq3`, unioning the notes of every chunk.
pub fn parse(bytes_sq3: &[u8]) -> Result<Sq3> {
    ensure!(
        bytes_sq3.len() >= SEQP_HEADER_END && &bytes_sq3[0..4] == SEQP_MAGIC,
        "not a SEQP chart container (magic {:02X?})",
        &bytes_sq3[0..bytes_sq3.len().min(4)]
    );
    let music_id = read_u32_le(bytes_sq3, MUSIC_ID_AT)?;
    let mut cursor = read_u32_le(bytes_sq3, DATA_OFFSET_AT)? as usize;
    let count_charts = read_u32_le(bytes_sq3, CHART_COUNT_AT)? as usize;

    let mut ticks_per_second = 0u32;
    let mut end_tick = 0u32;
    let mut bucket_drum = Bucket::default();
    let mut bucket_guitar = Bucket::default();
    let mut bucket_bass = Bucket::default();
    let mut bucket_auto = Bucket::default();

    for index_chunk in 0..count_charts {
        ensure!(
            cursor + CHUNK_HEADER_LEN <= bytes_sq3.len(),
            "SEQP chunk {index_chunk} header at {cursor} exceeds file size {}",
            bytes_sq3.len()
        );
        let size_chunk = read_u32_le(bytes_sq3, cursor)? as usize;
        ensure!(size_chunk > 0, "SEQP chunk {index_chunk} declares a zero size");
        let start_body = cursor + CHUNK_HEADER_LEN;
        // a chunk's declared size covers its own header, so the last one can reach past EOF by 0x10
        let end_body = start_body
            .checked_add(size_chunk)
            .context("SEQP chunk range overflow")?
            .min(bytes_sq3.len());
        cursor += size_chunk;

        let body = &bytes_sq3[start_body..end_body];
        if body.len() < SQ3T_HEADER_END || &body[0..4] != SQ3T_MAGIC {
            continue; // older SEQT chunks and padding both land here
        }

        let bucket = if body[IS_METADATA_AT] != 0 {
            &mut bucket_auto
        } else {
            match body[GAME_TYPE_AT] {
                GAME_TYPE_DRUM => &mut bucket_drum,
                GAME_TYPE_GUITAR => &mut bucket_guitar,
                GAME_TYPE_BASS => &mut bucket_bass,
                // open / guitar1 / guitar2 re-frame the guitar part's own sounds
                _ => &mut bucket_guitar,
            }
        };
        let (ticks_chunk, end_chunk) = read_chunk(body, index_chunk, bucket)?;
        ticks_per_second = ticks_per_second.max(ticks_chunk);
        end_tick = end_tick.max(end_chunk);
    }

    ensure!(ticks_per_second > 0, "no readable SQ3T chart in the container");
    Ok(Sq3 {
        music_id,
        ticks_per_second,
        end_tick,
        vec_note_drum: bucket_drum.vec_note,
        vec_note_guitar: bucket_guitar.vec_note,
        vec_note_bass: bucket_bass.vec_note,
        vec_note_auto: bucket_auto.vec_note,
    })
}

// Read one SQ3T chunk's note events into `bucket`; returns its (ticks_per_second, end_tick).
fn read_chunk(body: &[u8], index_chunk: usize, bucket: &mut Bucket) -> Result<(u32, u32)> {
    let header_size = read_u32_le(body, HEADER_SIZE_AT)? as usize;
    let count_events = read_u32_le(body, EVENT_COUNT_AT)? as usize;
    let ticks_per_second = read_u16_le(body, TIME_DIVISION_AT)? as u32;
    let size_event = read_u32_le(body, EVENT_SIZE_AT)? as usize;

    ensure!(ticks_per_second > 0, "SQ3T chunk {index_chunk} declares a zero time_division");
    ensure!(
        size_event > VOLUME_AT,
        "SQ3T chunk {index_chunk} event size {size_event} is too small to hold a note"
    );
    let end_events = header_size
        .checked_add(count_events.checked_mul(size_event).context("SQ3T event table size overflow")?)
        .context("SQ3T event table range overflow")?;
    ensure!(
        end_events <= body.len(),
        "SQ3T chunk {index_chunk} event table [{header_size}..{end_events}] exceeds chunk size {}",
        body.len()
    );

    let mut end_tick = 0u32;
    for index_event in 0..count_events {
        let base = header_size + index_event * size_event;
        let tick = read_u32_le(body, base + TICK_AT)?;
        match body[base + EVENT_ID_AT] {
            EVENT_ID_NOTE => {
                // the field is a u32 but VA3 sound ids are u16; a high word means we mis-parsed
                let sound_id = read_u32_le(body, base + SOUND_ID_AT)?;
                ensure!(
                    sound_id <= u16::MAX as u32,
                    "SQ3T chunk {index_chunk} note {index_event} sound_id {sound_id} does not fit \
                     a VA3 sound id"
                );
                end_tick = end_tick.max(tick);
                bucket.push(Note {
                    tick,
                    sound_id: sound_id as u16,
                    volume: body[base + VOLUME_AT].min(VOLUME_MAX),
                });
            }
            EVENT_ID_END_POSITION => end_tick = end_tick.max(tick),
            _ => {}
        }
    }
    Ok((ticks_per_second, end_tick))
}
