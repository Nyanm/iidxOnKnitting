//! The crate's public entry points. Two pairs, by what the caller hands us:
//!   - reconstruct a song from a chart + keysound archive: `render_song` (loose files) /
//!     `render_packed_song` (an `.ifs` that holds both).
//!   - transcode a single pre-mixed audio file: `convert_song` (a decodable file) /
//!     `convert_packed_song` (a 2DX9 container, whose first entry is the mix).
//!
//! Routing is the caller's job: this crate does no folder walking or path globbing — it takes
//! explicit file paths (the on-disk layout of IIDX vs SDVX songs is the caller's domain knowledge).
//! `render_*` returns [`RenderError::NotKeysound`] when the "keysound archive" is really a single
//! pre-mixed file, so the caller can fall back to `convert_song`.

use crate::chart::{self, Difficulty};
use crate::codec;
use crate::mix;
use crate::tool::ifs;
use crate::unpack;

use std::fs;
use std::path::Path;

use anyhow::{Context, Result, bail};

// global allocator: mimalloc's per-thread heaps avoid the default Windows heap-lock contention under the worker pool
#[global_allocator]
static GLOBAL: mimalloc::MiMalloc = mimalloc::MiMalloc;

/// How a render/convert failed, so the caller can branch. The intended use is to try a `render_*`
/// entry point and, when it returns [`RenderError::NotKeysound`], fall back to [`convert_song`].
#[derive(Debug, thiserror::Error)]
pub enum RenderError {
    #[error("input is a single pre-mixed audio file, not keysounds; use convert_song")]
    NotKeysound,
    #[error("input is a packed 2dx file, not single audio; use convert_packed_song")]
    NotSingleAudio,
    #[error("{0:#}")]
    Failed(anyhow::Error),
}

// Let the `?` operator turn the internal anyhow errors (fs, parsing, ffmpeg) into `Failed`, so the
// pipeline keeps using anyhow internally and only the public boundary is typed. anyhow::Error is
// not itself a std::error::Error, so this is hand-written rather than `#[from]`.
impl From<anyhow::Error> for RenderError {
    fn from(error: anyhow::Error) -> Self {
        RenderError::Failed(error)
    }
}

// ── render: BMS-style reconstruction from a keysound archive + chart ─────────────────────────────

/// Reconstruct a song from a loose keysound archive (`.s3p` or `.2dx`) and chart (`.1`), writing
/// Ogg/Opus to `output_path`. Returns [`RenderError::NotKeysound`] if `audio_path` is actually a
/// single pre-mixed file (then call [`convert_song`] on it).
pub fn render_song(audio_path: &Path, chart_path: &Path, output_path: &Path, difficulty: Difficulty) -> Result<(), RenderError> {
    let bytes_archive =
        fs::read(audio_path).with_context(|| format!("reading {}", audio_path.display()))?;
    let bytes_chart =
        fs::read(chart_path).with_context(|| format!("reading {}", chart_path.display()))?;
    render_keysound(&bytes_archive, &bytes_chart, output_path, difficulty)
}

/// Reconstruct a song packed in an `.ifs` (the chart and keysound archive live inside), writing
/// Ogg/Opus to `output_path`.
pub fn render_packed_song(ifs_path: &Path, output_path: &Path, difficulty: Difficulty) -> Result<(), RenderError> {
    let bytes_ifs = fs::read(ifs_path).with_context(|| format!("reading {}", ifs_path.display()))?;
    let (bytes_chart, bytes_archive) =
        extract_ifs(&bytes_ifs).with_context(|| format!("reading song from {}", ifs_path.display()))?;
    render_keysound(&bytes_archive, &bytes_chart, output_path, difficulty)
}

// Shared core: unpack the keysound archive, parse the chosen chart, mix every sounding onto a 44.1k
// stereo timeline, resample to 48k and encode Ogg/Opus.
fn render_keysound(bytes_archive: &[u8], bytes_chart: &[u8], output_path: &Path, difficulty: Difficulty) -> Result<(), RenderError> {
    let vec_keysound = unpack_keysounds(bytes_archive)?;
    let parsed_chart = chart::parse(bytes_chart, difficulty)?;

    // decode each referenced keysound once into a 44.1k stereo PCM cache
    let mut vec_pcm_cache: Vec<Option<Vec<f32>>> = vec![None; vec_keysound.len()];
    let mut skipped = 0usize;
    for sounding in &parsed_chart.events {
        let sample = sounding.sample_1based;
        if sample < 1 || sample as usize > vec_keysound.len() {
            skipped += 1;
            continue;
        }
        let index_sample = sample as usize - 1;
        if vec_pcm_cache[index_sample].is_none() {
            vec_pcm_cache[index_sample] =
                Some(codec::decode_keysound(&vec_keysound[index_sample])?);
        }
    }
    if skipped > 0 { eprintln!("iidxOnKnitting: skipped {skipped} note(s) with no keysound in 1..={} (reserved/null IDs)", vec_keysound.len()); }

    let timeline = mix::render(&parsed_chart.events, &vec_pcm_cache, parsed_chart.duration_ms);
    let seconds = (timeline.len() / 2) as f64 / 44_100.0;
    eprintln!(
        "iidxOnKnitting: {} events, {:.1}s; encoding Opus -> {}",
        parsed_chart.events.len(),
        seconds,
        output_path.display()
    );

    codec::encode_opus(&timeline, output_path)?;
    Ok(())
}

// Detect the keysound-archive format by magic and unpack to ordered keysounds. A `RIFF` archive is
// a single pre-mixed file, not a keysound container -> NotKeysound (caller should use convert_song).
fn unpack_keysounds(bytes_archive: &[u8]) -> Result<Vec<Vec<u8>>, RenderError> {
    if bytes_archive.starts_with(b"S3P0") {
        Ok(unpack::unpack_s3p(bytes_archive).context("unpacking s3p keysound archive")?)
    } else if bytes_archive.starts_with(b"RIFF") {
        Err(RenderError::NotKeysound)
    } else {
        Ok(unpack::unpack_2dx(bytes_archive).context("unpacking 2dx keysound archive")?)
    }
}

// Read a song's chart bytes + keysound-archive bytes out of an `.ifs` (KBin manifest). Prefer the
// `.s3p` archive (v25-29 + s3p songs); else the single non-preview `.2dx` (v1-24 + 2dx songs), or
// for the few multi-source songs pick by the `<id>a` > `<id>1` > `<id>` preference (see README).
fn extract_ifs(bytes_ifs: &[u8]) -> Result<(Vec<u8>, Vec<u8>)> {
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

// Multi-source songs pack several keysound `.2dx`; pick by the preference `<id>a` > `<id>1` >
// `<id>` (the `<id>a` variant is usually the modern, re-added version). Returns the index of the
// first preference that matches the candidate member names.
fn pick_multisource(id: &str, names: &[&str]) -> Option<usize> {
    ["a", "1", ""].iter().find_map(|suffix| {
        let wanted = format!("{id}{suffix}.2dx");
        names.iter().position(|name| *name == wanted)
    })
}

// ── convert: transcode a single pre-mixed audio file as-is ───────────────────────────────────────

/// Transcode a single decodable audio file (a pre-mixed `.2dx`, an SDVX `.s3v`, …) to Ogg/Opus.
/// The file is demuxed directly — no chart, no keysound reconstruction. Returns
/// [`RenderError::NotSingleAudio`] if it is actually a 2DX9 container (use [`convert_packed_song`]).
pub fn convert_song(audio_path: &Path, output_path: &Path) -> Result<(), RenderError> {
    let bytes_audio = fs::read(audio_path).with_context(|| format!("reading {}", audio_path.display()))?;
    if unpack::is_2dx9(&bytes_audio) {
        return Err(RenderError::NotSingleAudio);
    }
    convert_bytes(&bytes_audio, output_path)
}

/// Transcode a 2DX9 audio container (an SDVX `.2dx`, which packs the BGM as 2DX9) to Ogg/Opus by
/// decoding its first entry — the main mix.
pub fn convert_packed_song(audio_path: &Path, output_path: &Path) -> Result<(), RenderError> {
    let bytes_archive =
        fs::read(audio_path).with_context(|| format!("reading {}", audio_path.display()))?;
    let payload = unpack::unpack_2dx(&bytes_archive)
        .with_context(|| format!("unpacking 2dx container {}", audio_path.display()))?
        .into_iter()
        .next()
        .context("2dx container has no audio entries")?;
    convert_bytes(&payload, output_path)
}

// Shared core: decode one audio blob to 44.1k stereo f32, resample to 48k and encode Ogg/Opus.
fn convert_bytes(bytes_audio: &[u8], output_path: &Path) -> Result<(), RenderError> {
    let samples = codec::decode_keysound(bytes_audio)?;
    let seconds = (samples.len() / 2) as f64 / 44_100.0;
    eprintln!(
        "iidxOnKnitting: transcoding {seconds:.1}s -> {}",
        output_path.display()
    );
    codec::encode_opus(&samples, output_path)?;
    Ok(())
}
