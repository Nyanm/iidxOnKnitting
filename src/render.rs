//! Top-level render pipeline: archive unpack -> chart parse -> keysound decode ->
//! timeline mix -> Opus encode. Filled in incrementally over Steps 2-6.

use std::path::Path;

use anyhow::Result;

use crate::chart::Difficulty;

/// Render one IIDX song to an Ogg/Opus file. This is the crate's single public entry
/// point and the shape that will be called from iidxOnEar.
///
/// - `s3p_path`    keysound archive (S3P0 container, WMAv2 payloads)
/// - `chart_path`  chart file (.1)
/// - `difficulty`  which chart slot to render (any difficulty reconstructs the same song)
/// - `output_path` destination .ogg (Ogg/Opus)
pub fn render_song(
    s3p_path: &Path,
    chart_path: &Path,
    difficulty: Difficulty,
    output_path: &Path,
) -> Result<()> {
    // Pipeline, implemented across the coming steps:
    //   s3p::unpack(s3p_path)              -> keysound WMA blobs            (Step 2)
    //   chart::parse(chart_path, diff)     -> sounding events + duration    (Step 3)
    //   codec::decode_keysounds(..)        -> per-sample 44.1k stereo PCM   (Step 4)
    //   mix::render(events, pcm, dur)      -> 44.1k stereo timeline         (Step 5)
    //   codec::encode_opus(timeline, out)  -> Ogg/Opus                      (Step 6)
    let _ = (s3p_path, chart_path, difficulty, output_path);
    anyhow::bail!("render_song: skeleton only (Step 1); pipeline lands in Steps 2-6")
}
