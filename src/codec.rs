//! libav FFI: decode WMAv2 keysound blobs to 44.1k stereo f32 PCM (Step 4), and encode a
//! mixed 44.1k timeline to Ogg/Opus (Step 6, not yet implemented).
//!
//! libav demuxes from a path, so each blob is written to a short-lived temp file. A RAII
//! guard (`TempFile`) removes that file on every exit path — normal return, `?` error, or
//! panic unwind. The decode loop mirrors the reused SDVX transcode pipeline, but resamples
//! to f32/stereo/44100 (the keysound native rate) so the mixer can sum without per-sample
//! rate conversion.

use std::fs;
use std::path::{Path, PathBuf};
use std::slice;
use std::sync::Once;
use std::sync::atomic::{AtomicU64, Ordering};

use anyhow::{Context, Result, anyhow};
use ffmpeg_the_third as ffmpeg;

const KEYSOUND_RATE: u32 = 44_100; // all IIDX s3p keysounds are 44.1k stereo (see S3P_FORMAT.md)

static INIT: Once = Once::new();
static TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);

// ffmpeg init is idempotent but not free; run it exactly once across all threads.
fn ensure_init() {
    INIT.call_once(|| {
        ffmpeg::init().expect("ffmpeg vendored init failed");
        // silence per-frame libav warnings (e.g. WMA "skipped samples") across ~1186 decodes
        ffmpeg::util::log::set_level(ffmpeg::util::log::Level::Error);
    });
}

/// Decode one ASF/WMAv2 keysound blob to interleaved stereo f32 PCM at 44.1 kHz.
pub fn decode_wma_to_pcm(blob: &[u8]) -> Result<Vec<f32>> {
    ensure_init();
    let temp = TempFile::create(blob)?; // removed on drop (success / error / panic)
    decode_file_to_pcm(&temp.path)
    // `temp` drops here; the demuxer opened inside decode_file_to_pcm is already closed
}

// Decode an audio file (any vendored-supported codec) to interleaved stereo f32 @ 44.1 kHz.
fn decode_file_to_pcm(path: &Path) -> Result<Vec<f32>> {
    let mut ictx =
        ffmpeg::format::input(path).with_context(|| format!("opening {}", path.display()))?;
    let input_stream = ictx
        .streams()
        .best(ffmpeg::media::Type::Audio)
        .ok_or_else(|| anyhow!("no audio stream in {}", path.display()))?;
    let stream_index = input_stream.index();

    let context = ffmpeg::codec::context::Context::from_parameters(input_stream.parameters())
        .context("decoder context from stream parameters")?;
    let mut decoder = context.decoder().audio().context("opening audio decoder")?;

    // The WMAv2 decoder reports an unspecified-mask 2-channel layout, which makes
    // ffmpeg-the-third's get2() panic on `.mask().unwrap()`. Every keysound is stereo
    // (see S3P_FORMAT.md), so declare the source layout as canonical STEREO.
    let mut resampler = ffmpeg::software::resampling::Context::get2(
        decoder.format(),
        ffmpeg::ChannelLayout::STEREO,
        decoder.rate(),
        ffmpeg::format::Sample::F32(ffmpeg::format::sample::Type::Packed),
        ffmpeg::ChannelLayout::STEREO,
        KEYSOUND_RATE,
    )
    .context("creating resampler")?;

    let mut pcm = Vec::new();
    for result in ictx.packets() {
        let (stream, packet) = result.context("reading packet")?;
        if stream.index() != stream_index {
            continue;
        }
        decoder.send_packet(&packet).context("sending packet to decoder")?;
        drain_decoded(&mut decoder, &mut resampler, &mut pcm)?;
    }
    decoder.send_eof().context("flushing decoder")?;
    drain_decoded(&mut decoder, &mut resampler, &mut pcm)?;

    Ok(pcm)
}

// Pull every ready decoded frame, resample to f32/stereo/44.1k, append interleaved samples.
fn drain_decoded(
    decoder: &mut ffmpeg::decoder::Audio,
    resampler: &mut ffmpeg::software::resampling::Context,
    pcm: &mut Vec<f32>,
) -> Result<()> {
    let mut decoded = unsafe { ffmpeg::Frame::empty() };
    while decoder.receive_frame(&mut decoded).is_ok() {
        let mut decoded_audio = ffmpeg::frame::Audio::from(decoded);
        // Stamp the canonical STEREO layout the resampler was configured with (the decoder's
        // own layout carries no mask), so swr_convert_frame accepts the input frame.
        decoded_audio.set_ch_layout(ffmpeg::ChannelLayout::STEREO);
        let mut resampled = ffmpeg::frame::Audio::empty();
        resampler.run(&decoded_audio, &mut resampled).context("resampling frame")?;
        pcm.extend_from_slice(read_stereo_packed(&resampled));
        decoded = unsafe { ffmpeg::Frame::empty() };
    }
    Ok(())
}

// Reinterpret a packed-stereo f32 frame as a flat interleaved &[f32] (L,R,L,R,...).
// For packed stereo, plane::<(f32,f32)>(0) is nb_samples (L,R) pairs, identical in memory
// to a flat &[f32] of double the length, so the cast is sound.
fn read_stereo_packed(frame: &ffmpeg::frame::Audio) -> &[f32] {
    let stereo: &[(f32, f32)] = frame.plane::<(f32, f32)>(0);
    unsafe { slice::from_raw_parts(stereo.as_ptr() as *const f32, stereo.len() * 2) }
}

// ── temp file with RAII cleanup ────────────────────────────────────────────────
// Unique name = temp_dir + process id + atomic counter, so concurrent/sequential decodes
// never collide. Drop removes the file; a hard process kill is the only leak path, and the
// OS reclaims its temp dir.
struct TempFile {
    path: PathBuf,
}

impl TempFile {
    fn create(bytes: &[u8]) -> Result<Self> {
        let serial = TEMP_COUNTER.fetch_add(1, Ordering::Relaxed);
        let mut path = std::env::temp_dir();
        path.push(format!("iidx_keysound_{}_{}.wma", std::process::id(), serial));
        fs::write(&path, bytes).with_context(|| format!("writing temp {}", path.display()))?;
        Ok(Self { path })
    }
}

impl Drop for TempFile {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path); // best-effort; nothing useful to do on failure
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::s3p;

    fn sample_s3p() -> std::path::PathBuf {
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join(".sample/iidxOnEar/.sample/unpack/30000/30000.s3p")
    }

    #[test]
    fn decode_base_and_keysounds() {
        let path_sample = sample_s3p();
        if !path_sample.exists() {
            eprintln!("skipping decode test: sample absent {}", path_sample.display());
            return;
        }
        let blobs = s3p::unpack(&path_sample).expect("unpack 30000.s3p");

        // sample 1 (the background base) ~ full song length
        let base = decode_wma_to_pcm(&blobs[0]).expect("decode base");
        assert_eq!(base.len() % 2, 0, "interleaved stereo length is even");
        let seconds = (base.len() / 2) as f64 / KEYSOUND_RATE as f64;
        assert!((100.0..130.0).contains(&seconds), "base ~110s, got {seconds:.1}s");
        assert!(base.iter().all(|value| value.is_finite()), "no NaN/inf in base");

        // a spread of short keysounds decode to non-empty finite stereo PCM
        for index_sample in [1usize, 5, 100, 1185] {
            let pcm = decode_wma_to_pcm(&blobs[index_sample]).expect("decode keysound");
            assert!(!pcm.is_empty() && pcm.len() % 2 == 0, "keysound {index_sample} shape");
            assert!(pcm.iter().all(|value| value.is_finite()), "keysound {index_sample} finite");
        }
    }
}
