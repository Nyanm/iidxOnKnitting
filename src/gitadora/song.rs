//! Assembling one GITADORA song: routing `.ifs` members, choosing the bed, and mixing.
//!
/*
A song is two archives. `m<id>_seq.ifs` holds the charts (`d<id>.sq3`, `g<id>.sq3`) and the keysound
archives (`spu<id>d.va3` for the drum kit, `spu<id>g.va3` for guitar AND bass phrases).
`m<id>_bgm.ifs` holds several pre-mixed backing tracks, named by which parts are already baked in:
`bgm<id><d|_><g|_><b|_>k.bin`.

The bed's own name says which parts it already contains, so it also decides which parts we may play:
summing in a part that is already baked into the bed would double it. We therefore parse the mask and
play exactly its absent parts, preferring the bed with the FEWEST instruments baked in — that is the
most BMS-like reconstruction, which is the point of this tool. Across the library that means:
  - `bgm<id>___k.bin` (backing only) -> drum + guitar + bass. 1279 songs, plus 38 more that use the
    XG-era `..._xg.bin` spelling of the same masks.
  - `bgm<id>__bk.bin` (bass baked in) -> drum + guitar. 39 songs, only 2 of which even have a bass
    chart, so almost nothing is given up.
  - a few songs ship a single variant only: m0019 has just `_gbk` (so we play drums) and m0209 just
    `d_bk` (so we play guitar). Mask parsing handles those without a special case.

`vec_note_auto` — notes found in a chart file's "metadata" chunk — is played against whichever VA3
sits beside that chart, since the chunk's own part field is meaningless.
*/

use crate::audio::master;
use crate::audio::mix::{self, Timeline};
use crate::gitadora::{bmp, sq3, va3};
use crate::tool::ifs;

use anyhow::{Context, Result, bail};

const BED_PREFIX: &str = "bgm";        // preview clips are `i<id><dm|gf>.bin`, not beds
const BED_SUFFIX: &str = ".bin";
const BED_SUFFIX_XG: &str = "_xg.bin"; // XG-era spelling: the same masks with `_xg` appended
const BED_MASK_LEN: usize = 4;         // the three part slots plus the trailing 'k'
const VOLUME_FULL_SCALE: f32 = 127.0;  // both VA3 entry volume and per-note velocity are 0..127

/// Which instruments a bgm variant already has mixed in, read off its file name's `<d|_><g|_><b|_>k`
/// mask. The parts it lacks are the ones the renderer must supply from keysounds.
#[derive(Clone, Copy)]
struct BedMask {
    has_drum: bool,
    has_guitar: bool,
    has_bass: bool,
}

impl BedMask {
    /// Parse `bgm<id><d|_><g|_><b|_>k.bin`, optionally with the `_xg` suffix. `None` for any other
    /// member name, which is what keeps the 10-second `i<id>dm.bin` previews from being chosen.
    fn from_name(name: &str) -> Option<Self> {
        if !name.starts_with(BED_PREFIX) {
            return None;
        }
        let stem = name
            .strip_suffix(BED_SUFFIX_XG)
            .or_else(|| name.strip_suffix(BED_SUFFIX))?;
        let mask: Vec<char> = stem.chars().rev().take(BED_MASK_LEN).collect();
        // collected in reverse: ['k', bass, guitar, drum]
        if mask.len() != BED_MASK_LEN || mask[0] != 'k' {
            return None;
        }
        Some(BedMask {
            has_drum: slot(mask[3], 'd')?,
            has_guitar: slot(mask[2], 'g')?,
            has_bass: slot(mask[1], 'b')?,
        })
    }

    /// How many parts this bed leaves for us to reconstruct — the selection score.
    fn count_missing(self) -> usize {
        usize::from(!self.has_drum) + usize::from(!self.has_guitar) + usize::from(!self.has_bass)
    }
}

// One mask slot: either the part's own letter (present) or '_' (absent).
fn slot(actual: char, letter: char) -> Option<bool> {
    match actual {
        '_' => Some(false),
        other if other == letter => Some(true),
        _ => None,
    }
}

/// An assembled song: the audio plus what went into it, so the caller can report or assert on the
/// mix without re-deriving it. Mirrors [`crate::iidx::song::Song`].
pub struct Song {
    pub timeline: Timeline,
    pub bed_name: String,
    /// Keysounds actually laid onto the bed.
    pub cnt_note: usize,
    /// Notes whose `sound_id` had no VA3 entry and were therefore dropped.
    pub cnt_unresolved: usize,
}

/// Read a song's two `.ifs` archives and mix it, keysound gain already applied. The caller does the
/// final limiting and encoding.
pub fn mix_song(bytes_seq: &[u8], bytes_bgm: &[u8]) -> Result<Song> {
    let members_bgm = ifs::list_members(bytes_bgm).context("reading the bgm .ifs")?;
    let members_seq = ifs::list_members(bytes_seq).context("reading the seq .ifs")?;

    // pick the bed with the fewest instruments baked in; its mask tells us what to play
    let (member_bed, mask) = members_bgm
        .iter()
        .filter_map(|member| BedMask::from_name(&member.name).map(|mask| (member, mask)))
        .max_by_key(|(_member, mask)| mask.count_missing())
        .with_context(|| {
            format!(
                "no backing track in the bgm .ifs (need a bgm*<d|_><g|_><b|_>k.bin member); \
                 it holds {:?}",
                member_names(&members_bgm)
            )
        })?;
    let bed = bmp::decode(slice_member(bytes_bgm, member_bed))
        .with_context(|| format!("decoding bed {}", member_bed.name))?;
    let mut timeline = Timeline::from_stereo_i16(bed.rate_hz, &bed.samples);
    eprintln!(
        "iidxOnKnitting: bed {} ({:.1}s @ {} Hz); playing{}{}{}",
        member_bed.name,
        timeline.seconds(),
        bed.rate_hz,
        if mask.has_drum { "" } else { " drum" },
        if mask.has_guitar { "" } else { " guitar" },
        if mask.has_bass { "" } else { " bass" }
    );
    let bed_name = member_bed.name.clone();

    // each chart file is paired with the VA3 beside it; a song missing one part still renders
    let mut cnt_note = 0usize;
    let mut cnt_unresolved = 0usize;
    let mut flag_any_part = false;
    for letter in ['d', 'g'] {
        let (Some(member_chart), Some(member_va3)) =
            (find_chart(&members_seq, letter), find_keysounds(&members_seq, letter))
        else {
            eprintln!("iidxOnKnitting: no {letter}<id>.sq3 + spu<id>{letter}.va3 pair; skipping that part");
            continue;
        };
        let chart = sq3::parse(slice_member(bytes_seq, member_chart))
            .with_context(|| format!("parsing chart {}", member_chart.name))?;
        let archive = va3::parse(slice_member(bytes_seq, member_va3))
            .with_context(|| format!("parsing keysounds {}", member_va3.name))?;
        flag_any_part = true;

        // skip any part the bed already carries; `vec_note_auto` is never baked into a bed
        let mut vec_bucket = vec![&chart.vec_note_auto];
        if !mask.has_drum { vec_bucket.push(&chart.vec_note_drum); }
        if !mask.has_guitar { vec_bucket.push(&chart.vec_note_guitar); }
        if !mask.has_bass { vec_bucket.push(&chart.vec_note_bass); }
        for vec_note in vec_bucket {
            let (added, unresolved) = overlay(&mut timeline, vec_note, &archive, chart.ticks_per_second);
            cnt_note += added;
            cnt_unresolved += unresolved;
        }
    }
    if !flag_any_part {
        bail!(
            "no chart + keysound pair in the seq .ifs; it holds {:?}",
            member_names(&members_seq)
        );
    }
    if cnt_unresolved > 0 {
        eprintln!("iidxOnKnitting: skipped {cnt_unresolved} note(s) whose sound_id has no keysound");
    }
    eprintln!("iidxOnKnitting: laid {cnt_note} keysound(s) over the bed");
    Ok(Song { timeline, bed_name, cnt_note, cnt_unresolved })
}

/* Sum one bucket of notes onto the timeline. Gain is the VA3 entry's own volume times the note's
velocity (both 0..127 linear), times the global keysound attenuation that keeps the sum from
overwhelming the pre-mastered bed; pan comes from the entry. Notes are NOT cut when the next one
starts: measured on real data, ~99% of guitar phrases outlive the gap to the next note by about 25%,
which is a natural decay tail meant to ring on, not something the game truncates. */
fn overlay(timeline: &mut Timeline, vec_note: &[sq3::Note], archive: &va3::Archive, ticks_per_second: u32) -> (usize, usize) {
    let mut cnt_added = 0usize;
    let mut cnt_unresolved = 0usize;
    for note in vec_note {
        let Some(keysound) = archive.map_keysound.get(&note.sound_id) else {
            cnt_unresolved += 1;
            continue;
        };
        let frame_start = tick_to_frame(note.tick, ticks_per_second, timeline.rate_hz());
        let gain = master::KEYSOUND_GAIN
            * (keysound.volume as f32 / VOLUME_FULL_SCALE)
            * (note.volume as f32 / VOLUME_FULL_SCALE);
        let (gain_left, gain_right) = mix::pan_gains(keysound.pan);
        timeline.add_mono_i16(frame_start, &keysound.samples_mono, gain * gain_left, gain * gain_right);
        cnt_added += 1;
    }
    (cnt_added, cnt_unresolved)
}

// A chart tick (1/300 s units) to a frame index at the timeline's rate. Done in one u64 expression
// so the 3.33 ms tick grid is not rounded twice through milliseconds.
fn tick_to_frame(tick: u32, ticks_per_second: u32, rate_hz: u32) -> usize {
    (tick as u64 * rate_hz as u64 / ticks_per_second as u64) as usize
}

// Charts are named `<letter><id>.sq3` -- the part letter is a PREFIX, unlike the keysound archives.
// The `.sq2` siblings are an older revision of the same charts and are deliberately not matched.
fn find_chart(members: &[ifs::Member], letter: char) -> Option<&ifs::Member> {
    members
        .iter()
        .find(|member| member.name.starts_with(letter) && member.name.ends_with(".sq3"))
}

// Keysound archives are named `spu<id><letter>.va3` -- here the part letter IS a suffix.
fn find_keysounds(members: &[ifs::Member], letter: char) -> Option<&ifs::Member> {
    members.iter().find(|member| member.name.ends_with(&format!("{letter}.va3")))
}

fn slice_member<'a>(bytes_ifs: &'a [u8], member: &ifs::Member) -> &'a [u8] {
    &bytes_ifs[member.offset..member.offset + member.size]
}

fn member_names(members: &[ifs::Member]) -> Vec<&str> {
    members.iter().map(|member| member.name.as_str()).collect()
}
