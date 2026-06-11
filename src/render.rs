//! Top-level render pipeline: archive unpack -> chart parse -> keysound decode ->
//! timeline mix -> Opus encode. This is the crate's single public entry point.

use crate::chart::{self, Difficulty};
use crate::codec;
use crate::mix;
use crate::s3p;

use std::path::Path;

use anyhow::{Result, ensure};

/// Render one IIDX song to an Ogg/Opus file. This is the shape called from iidxOnEar.
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
    // unpack the keysound archive (index i -> 1-based sample i+1)
    let vec_keysound = s3p::unpack(s3p_path)?;
    // parse the chosen difficulty into a flat list of sounding events
    let parsed_chart = chart::parse(chart_path, difficulty)?;

    // decode each referenced keysound once into a 44.1k stereo PCM cache
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
    codec::encode_opus(&timeline, output_path)
}
