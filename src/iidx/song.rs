//! Assembling one IIDX song: routing `.ifs` members and summing keysounds onto a timeline.
//!
/*
The mirror of [`crate::gitadora::song`], and much the shorter of the two because IIDX has no backing
track: a song is entirely the sum of the keysounds its chart names, so there is no bed to decode, no
mask to read, and no part that might already be mixed in.

What is left is still two decisions the entry layer should not have to make:
  - a "keysound archive" may turn out to be a single pre-mixed file. Early `.2dx` audio and a real
    keysound `.2dx` share an extension, so the only honest test is the magic: `RIFF` means one bare
    WAVE, which is not something to reconstruct. That is reported as [`Mixed::PreMixedAudio`] rather
    than treated as an error, because transcoding it is a perfectly good outcome.
  - a packed `.ifs` may hold several candidate keysound archives (see `extract_ifs`).

Mixing happens at the keysounds' own 44.1 kHz ([`codec::KEYSOUND_RATE`]) and the encoder resamples
once at the end, which keeps every keysound's samples on their original grid.
*/

use crate::audio::mix::Timeline;
use crate::codec;
use crate::iidx::chart::{self, Difficulty};
use crate::tool::ifs;
use crate::unpack;

use anyhow::{Context, Result, bail};

const MAGIC_S3P: &[u8; 4] = b"S3P0"; // keysound archive, v25+
const MAGIC_RIFF: &[u8; 4] = b"RIFF"; // a bare WAVE: one pre-mixed file, not keysounds

/// An assembled song: the audio plus what went into it, so the caller can report or assert on the
/// mix without re-deriving it.
pub struct Song {
    pub timeline: Timeline,
    /// Keysounds actually laid onto the timeline.
    pub cnt_note: usize,
    /// Events whose sample number fell outside the archive and were dropped. Charts do reference
    /// reserved/null sample numbers, so this is routinely non-zero.
    pub cnt_unresolved: usize,
}

/// What [`mix_song`] found.
pub enum Mixed {
    /// The archive held keysounds and the song was reconstructed.
    Song(Song),
    /// The "keysound archive" is a single pre-mixed audio file; transcode it instead of rebuilding.
    PreMixedAudio,
}

/// Assemble a song from a keysound archive (`.s3p` / `.2dx`) and a chart (`.1`).
pub fn mix_song(bytes_archive: &[u8], bytes_chart: &[u8], difficulty: Difficulty) -> Result<Mixed> {
    let Some(vec_keysound) = unpack_keysounds(bytes_archive)? else {
        return Ok(Mixed::PreMixedAudio);
    };
    let parsed_chart = chart::parse(bytes_chart, difficulty)?;

    // decode each referenced keysound once into a PCM cache, so a sample reused by 200 notes is
    // decoded once; out-of-range sample numbers are counted here and skipped when mixing
    let mut vec_pcm_cache: Vec<Option<Vec<f32>>> = vec![None; vec_keysound.len()];
    let mut cnt_unresolved = 0usize;
    for sounding in &parsed_chart.events {
        let sample = sounding.sample_1based;
        if sample < 1 || sample as usize > vec_keysound.len() {
            cnt_unresolved += 1;
            continue;
        }
        let index_sample = sample as usize - 1;
        if vec_pcm_cache[index_sample].is_none() {
            vec_pcm_cache[index_sample] = Some(codec::decode_keysound(&vec_keysound[index_sample])?);
        }
    }
    if cnt_unresolved > 0 {
        eprintln!(
            "iidxOnKnitting: skipped {cnt_unresolved} note(s) with no keysound in 1..={} \
             (reserved/null IDs)",
            vec_keysound.len()
        );
    }

    // size the timeline to the chart's own length first; each keysound then extends it if its tail
    // runs past that, so trailing audio is never clipped
    let mut timeline = Timeline::new(codec::KEYSOUND_RATE);
    timeline.ensure_frames(timeline.ms_to_frame(parsed_chart.duration_ms));
    let mut cnt_note = 0usize;
    for sounding in &parsed_chart.events {
        let Some(pcm) = keysound_pcm(&vec_pcm_cache, sounding.sample_1based) else { continue };
        let frame_start = timeline.ms_to_frame(sounding.time_ms);
        timeline.add_stereo_f32(frame_start, pcm, 1.0);
        cnt_note += 1;
    }

    Ok(Mixed::Song(Song { timeline, cnt_note, cnt_unresolved }))
}

/// Read a song's chart bytes + keysound-archive bytes out of an `.ifs` (KBin manifest). Prefer the
/// `.s3p` archive (v25-29 + s3p songs); else the single non-preview `.2dx` (v1-24 + 2dx songs), or
/// for the few multi-source songs pick by the `<id>a` > `<id>1` > `<id>` preference (see README).
pub fn extract_ifs(bytes_ifs: &[u8]) -> Result<(Vec<u8>, Vec<u8>)> {
    let members = ifs::list_members(bytes_ifs).context("not a readable .ifs (KBin manifest)")?;

    let member_chart = members
        .iter()
        .find(|member| member.name.ends_with(".1"))
        .context("no .1 chart inside .ifs")?;
    let bytes_chart =
        bytes_ifs[member_chart.offset..member_chart.offset + member_chart.size].to_vec();

    if let Some(member_s3p) = members.iter().find(|member| member.name.ends_with(".s3p")) {
        let bytes_archive =
            bytes_ifs[member_s3p.offset..member_s3p.offset + member_s3p.size].to_vec();
        return Ok((bytes_chart, bytes_archive));
    }

    let members_2dx: Vec<&ifs::Member> = members
        .iter()
        .filter(|member| member.name.ends_with(".2dx") && !member.name.ends_with("_pre.2dx"))
        .collect();
    let member_2dx = match members_2dx.as_slice() {
        [] => bail!("no .s3p or .2dx keysound archive inside .ifs"),
        [only] => *only,
        _ => {
            let id = member_chart.name.strip_suffix(".1").unwrap_or(member_chart.name.as_str());
            let names: Vec<&str> = members_2dx.iter().map(|member| member.name.as_str()).collect();
            let index_chosen = pick_multisource(id, &names).with_context(|| {
                format!("multi-source .ifs matched none of {id}a/{id}1/{id}.2dx; found {names:?}")
            })?;
            members_2dx[index_chosen]
        }
    };
    let bytes_archive = bytes_ifs[member_2dx.offset..member_2dx.offset + member_2dx.size].to_vec();
    Ok((bytes_chart, bytes_archive))
}

// Detect the keysound-archive format by magic and unpack to ordered keysounds. `None` means the
// bytes are one pre-mixed RIFF/WAVE rather than a keysound container.
fn unpack_keysounds(bytes_archive: &[u8]) -> Result<Option<Vec<Vec<u8>>>> {
    if bytes_archive.starts_with(MAGIC_S3P) {
        Ok(Some(unpack::unpack_s3p(bytes_archive).context("unpacking s3p keysound archive")?))
    } else if bytes_archive.starts_with(MAGIC_RIFF) {
        Ok(None)
    } else {
        Ok(Some(unpack::unpack_2dx(bytes_archive).context("unpacking 2dx keysound archive")?))
    }
}

// Look up a 1-based chart sample's decoded PCM, if it was in range and got decoded.
fn keysound_pcm(vec_pcm_cache: &[Option<Vec<f32>>], sample_1based: u16) -> Option<&Vec<f32>> {
    let index_sample = (sample_1based as usize).checked_sub(1)?;
    vec_pcm_cache.get(index_sample)?.as_ref()
}

// Multi-source songs pack several keysound `.2dx`; pick by the preference `<id>a` > `<id>1` >
// `<id>` (the `<id>a` variant is usually the modern, re-added version). Returns the index of the
// first preference that matches the candidate member names.
fn pick_multisource(id: &str, names: &[&str]) -> Option<usize> {
    ["a", "1", ""].iter().find_map(|suffix| {
        let wanted = format!("{id}{suffix}.2dx");
        names.iter().position(|name| *name == wanted)
    })
}
