//! Top-level render pipeline: input resolution -> chart parse -> keysound decode ->
//! timeline mix -> Opus encode. This is the crate's single public entry point.

use crate::chart::{self, Difficulty};
use crate::codec;
use crate::mix;
use crate::source;

use std::path::Path;

use anyhow::{Result, ensure};

/// Render one IIDX song to an Ogg/Opus file. This is the shape called from iidxOnEar.
///
/// - `input_path`  one song: a v30+ loose folder, or an `.ifs` archive (v1-29)
/// - `difficulty`  which chart slot to render (any difficulty reconstructs the same song)
/// - `output_path` destination .ogg (Ogg/Opus)
pub fn render_song(input_path: &Path, difficulty: Difficulty, output_path: &Path) -> Result<()> {
    // resolve the input into ordered keysound blobs + raw chart bytes (era-agnostic)
    let song_source = source::resolve(input_path)?;
    let vec_keysound = &song_source.vec_keysound;
    // parse the chosen difficulty into a flat list of sounding events
    let parsed_chart = chart::parse(&song_source.bytes_chart, difficulty)?;

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
    codec::encode_opus(&timeline, output_path)
}
