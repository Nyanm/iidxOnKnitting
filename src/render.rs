//! The crate's public entry points — a thin layer over each branch's own assembly module.
//!
/*
Every render entry does the same four things and nothing else: read the named files, hand the bytes
to `<game>::song::mix_song`, master the returned timeline, encode it. The per-game knowledge — which
`.ifs` member is which, how a chart maps notes to keysounds, which backing track to use — lives in
[`crate::iidx::song`] and [`crate::gitadora::song`], so the two branches stay comparable here.

Mastering is the one step that genuinely differs, and only because of what sits underneath:
  - IIDX has no backing track, so the sum's absolute level is arbitrary and `peak_normalize` — scale
    back only if the peak exceeds full scale — costs nothing.
  - GITADORA lays keysounds over a bed that is already a loudness-maximised master, so the sum runs
    hot and gets the soft knee instead. Peak-normalising it would throw away ~9 dB.

Routing is the caller's job: this crate does no folder walking or path globbing — it takes explicit
file paths, since the on-disk layout of each game's songs is the caller's domain knowledge. Two
conditions are reported rather than guessed around, so the caller can branch:
[`RenderError::NotKeysound`] when a "keysound archive" is really one pre-mixed file, and
[`RenderError::NotSingleAudio`] when a file handed to `convert_song` is really a container.
*/

use crate::audio::master;
use crate::codec;
use crate::gitadora;
use crate::iidx;
use crate::iidx::chart::Difficulty;
use crate::unpack;

use std::fs;
use std::path::Path;

use anyhow::{Context, Result};

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

// ── IIDX ─────────────────────────────────────────────────────────────────────────────────────────

/// Reconstruct an IIDX song from a loose keysound archive (`.s3p` or `.2dx`) and chart (`.1`),
/// writing Ogg/Opus to `output_path`. Returns [`RenderError::NotKeysound`] if `audio_path` is
/// actually a single pre-mixed file (then call [`convert_song`] on it).
pub fn render_iidx_song(audio_path: &Path, chart_path: &Path, output_path: &Path, difficulty: Difficulty) -> Result<(), RenderError> {
    let bytes_archive =
        fs::read(audio_path).with_context(|| format!("reading {}", audio_path.display()))?;
    let bytes_chart =
        fs::read(chart_path).with_context(|| format!("reading {}", chart_path.display()))?;
    finish_iidx(&bytes_archive, &bytes_chart, output_path, difficulty)
}

/// Reconstruct an IIDX song packed in an `.ifs` (the chart and keysound archive live inside),
/// writing Ogg/Opus to `output_path`.
pub fn render_iidx_packed_song(ifs_path: &Path, output_path: &Path, difficulty: Difficulty) -> Result<(), RenderError> {
    let bytes_ifs = fs::read(ifs_path).with_context(|| format!("reading {}", ifs_path.display()))?;
    let (bytes_chart, bytes_archive) = iidx::song::extract_ifs(&bytes_ifs)
        .with_context(|| format!("reading song from {}", ifs_path.display()))?;
    finish_iidx(&bytes_archive, &bytes_chart, output_path, difficulty)
}

// Assemble, peak-normalize, encode.
fn finish_iidx(bytes_archive: &[u8], bytes_chart: &[u8], output_path: &Path, difficulty: Difficulty) -> Result<(), RenderError> {
    let mut song = match iidx::song::mix_song(bytes_archive, bytes_chart, difficulty)? {
        iidx::song::Mixed::PreMixedAudio => return Err(RenderError::NotKeysound),
        iidx::song::Mixed::Song(song) => song,
    };

    master::peak_normalize(song.timeline.samples_mut());
    eprintln!(
        "iidxOnKnitting: {} keysound(s), {:.1}s; encoding Opus -> {}",
        song.cnt_note,
        song.timeline.seconds(),
        output_path.display()
    );
    codec::encode_opus(song.timeline.samples(), song.timeline.rate_hz(), output_path)?;
    Ok(())
}

// ── GITADORA ─────────────────────────────────────────────────────────────────────────────────────

/// Reconstruct a GITADORA song from its `.ifs` pair — `m<id>_seq.ifs` (charts + keysounds) and
/// `m<id>_bgm.ifs` (the backing-track variants) — writing Ogg/Opus to `output_path`.
///
/// Which parts are rebuilt depends on the backing track chosen: the one with the fewest instruments
/// already mixed in wins, and only the parts it lacks are laid over it. See [`gitadora::song`].
pub fn render_gitadora_song(seq_ifs_path: &Path, bgm_ifs_path: &Path, output_path: &Path) -> Result<(), RenderError> {
    let bytes_seq =
        fs::read(seq_ifs_path).with_context(|| format!("reading {}", seq_ifs_path.display()))?;
    let bytes_bgm =
        fs::read(bgm_ifs_path).with_context(|| format!("reading {}", bgm_ifs_path.display()))?;

    let mut song = gitadora::song::mix_song(&bytes_seq, &bytes_bgm).with_context(|| {
        format!("rendering {} + {}", seq_ifs_path.display(), bgm_ifs_path.display())
    })?;

    let peak_before = master::peak(song.timeline.samples());
    master::soft_knee_limit(song.timeline.samples_mut(), master::KNEE_THRESHOLD);
    eprintln!(
        "iidxOnKnitting: {:.1}s, peak {peak_before:.2}x full scale before the soft knee; \
         encoding Opus -> {}",
        song.timeline.seconds(),
        output_path.display()
    );
    codec::encode_opus(song.timeline.samples(), song.timeline.rate_hz(), output_path)?;
    Ok(())
}

// ── shared: transcode a single pre-mixed audio file as-is ────────────────────────────────────────
// SDVX needs nothing beyond this pair; IIDX uses it for its `_pre.2dx` previews and for the early
// `.2dx` files that hold one finished mix rather than keysounds.

/// Transcode a single decodable audio file (an SDVX `.s3v`, a pre-mixed `.2dx`, …) to Ogg/Opus.
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
    let seconds = (samples.len() / 2) as f64 / codec::KEYSOUND_RATE as f64;
    eprintln!(
        "iidxOnKnitting: transcoding {seconds:.1}s -> {}",
        output_path.display()
    );
    codec::encode_opus(&samples, codec::KEYSOUND_RATE, output_path)?;
    Ok(())
}
