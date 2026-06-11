//! Top-level render pipeline: archive unpack -> chart parse -> keysound decode ->
//! timeline mix -> Opus encode. Filled in incrementally over Steps 2-6.

use crate::chart::{self, Difficulty};
use crate::codec;
use crate::s3p;

use std::path::Path;

use anyhow::{Result, ensure};

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
    // Step 2: unpack the keysound archive (index i -> 1-based sample i+1).
    let vec_keysound = s3p::unpack(s3p_path)?;
    // Step 3: parse the chosen difficulty into a flat list of sounding events.
    let parsed_chart = chart::parse(chart_path, difficulty)?;

    // Step 4: decode each referenced keysound once into a 44.1k stereo PCM cache.
    let mut vec_pcm_cache: Vec<Option<Vec<f32>>> = vec![None; vec_keysound.len()];
    for sounding in &parsed_chart.events {
        let sample = sounding.sample_1based;
        ensure!(
            sample >= 1 && (sample as usize) <= vec_keysound.len(),
            "chart references sample {sample} out of range 1..={}",
            vec_keysound.len()
        );
        let index_sample = sample as usize - 1;
        if vec_pcm_cache[index_sample].is_none() {
            vec_pcm_cache[index_sample] =
                Some(codec::decode_wma_to_pcm(&vec_keysound[index_sample])?);
        }
    }
    let count_decoded = vec_pcm_cache.iter().filter(|pcm| pcm.is_some()).count();

    // Remaining pipeline (Steps 5-6):
    //   mix::render(events, cache, duration)  -> 44.1k stereo timeline   (Step 5)
    //   codec::encode_opus(timeline, output)  -> Ogg/Opus                (Step 6)
    let _ = output_path;
    anyhow::bail!(
        "render_song: decoded {} of {} keysounds for {} events; mix/encode land in Steps 5-6",
        count_decoded,
        vec_keysound.len(),
        parsed_chart.events.len()
    )
}
