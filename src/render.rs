//! Top-level render pipeline: input resolution -> chart parse -> keysound decode ->
//! timeline mix -> Opus encode. Plus `convert_song`, the fallback for songs that ship a single
//! pre-mixed audio file instead of keysounds. These are the crate's public entry points.

use crate::chart::{self, Difficulty};
use crate::codec;
use crate::mix;
use crate::source;

use std::path::Path;

use anyhow::anyhow;

// global allocator: mimalloc's per-thread heaps avoid the default Windows heap-lock contention under the worker pool
#[global_allocator]
static GLOBAL: mimalloc::MiMalloc = mimalloc::MiMalloc;

/// How a render failed, so the caller can branch. The intended use is to try `render_song` and,
/// when it returns [`RenderError::NotKeysound`], fall back to [`convert_song`] on the same input.
#[derive(Debug, thiserror::Error)]
pub enum RenderError {
    /// The input has no keysound archive to reconstruct from — its audio is a single pre-mixed
    /// file (a RIFF `.2dx`). There is nothing to rebuild from the chart; call [`convert_song`].
    #[error("input is a single pre-mixed audio file, not keysounds; use convert_song")]
    NotKeysound,
    /// Any genuine failure: I/O, unreadable/corrupt data, an unsupported or encrypted `.ifs`,
    /// a decode/encode error, etc. Carries the underlying `anyhow` error (with its context chain).
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

/// Render one IIDX song to an Ogg/Opus file. This is the shape called from iidxOnEar.
///
/// - `input_path`  one song: a v30+ loose folder, or an `.ifs` archive (v1-29)
/// - `output_path` destination .ogg (Ogg/Opus)
/// - `difficulty`  which chart slot to render (any difficulty reconstructs the same song)
///
/// Returns [`RenderError::NotKeysound`] for a pre-mixed song (then call [`convert_song`]).
pub fn render_song(input_path: &Path, output_path: &Path, difficulty: Difficulty) -> Result<(), RenderError> {
    // resolve the input into ordered keysound blobs + raw chart bytes (era-agnostic)
    let song_source = source::resolve(input_path)?;
    let vec_keysound = &song_source.vec_keysound;
    // parse the chosen difficulty into a flat list of sounding events
    let parsed_chart = chart::parse(&song_source.bytes_chart, difficulty)?;

    // decode each referenced keysound once into a 44.1k stereo PCM cache
    let mut vec_pcm_cache: Vec<Option<Vec<f32>>> = vec![None; vec_keysound.len()];
    for sounding in &parsed_chart.events {
        let sample = sounding.sample_1based;
        if sample < 1 || sample as usize > vec_keysound.len() {
            return Err(anyhow!(
                "chart references sample {sample} out of range 1..={}",
                vec_keysound.len()
            )
            .into());
        }
        let index_sample = sample as usize - 1;
        if vec_pcm_cache[index_sample].is_none() {
            vec_pcm_cache[index_sample] =
                Some(codec::decode_keysound(&vec_keysound[index_sample])?);
        }
    }

    // mix every sounding onto a 44.1k stereo timeline (peak-normalized)
    let timeline = mix::render(&parsed_chart.events, &vec_pcm_cache, parsed_chart.duration_ms);
    let seconds = (timeline.len() / 2) as f64 / 44_100.0;
    eprintln!(
        "iidxOnKnitting: {} events, {:.1}s; encoding Opus -> {}",
        parsed_chart.events.len(),
        seconds,
        output_path.display()
    );

    // resample to 48k and encode to Ogg/Opus
    codec::encode_opus(&timeline, output_path)?;
    Ok(())
}

/// Transcode a pre-mixed song (its audio is a single file, not keysounds) to Ogg/Opus. Call this
/// when [`render_song`] returned [`RenderError::NotKeysound`]: it takes the same `input_path`
/// (loose folder or `.ifs`), locates the song's audio file itself, and re-encodes it directly —
/// no chart, no keysound reconstruction.
pub fn convert_song(input_path: &Path, output_path: &Path) -> Result<(), RenderError> {
    // locate and read the song's single audio file (for a pre-mixed song this is the playable mix)
    let bytes_audio = source::resolve_audio(input_path)?;
    // decode the whole file to 44.1k stereo f32 (decode_keysound handles any container ffmpeg reads)
    let samples = codec::decode_keysound(&bytes_audio)?;
    let seconds = (samples.len() / 2) as f64 / 44_100.0;
    eprintln!(
        "iidxOnKnitting: pre-mixed {seconds:.1}s; encoding Opus -> {}",
        output_path.display()
    );
    codec::encode_opus(&samples, output_path)?;
    Ok(())
}
