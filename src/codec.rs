//! libav FFI: decode WMAv2 keysound blobs to 44.1k stereo f32 PCM (Step 4), and encode a
//! mixed 44.1k timeline to Ogg/Opus (Step 6).
//!
//! libav demuxes from a path, so each blob is written to a short-lived temp file. A RAII
//! guard (`TempFile`) removes that file on every exit path — normal return, `?` error, or
//! panic unwind. The decode loop mirrors the reused SDVX transcode pipeline, but resamples
//! to f32/stereo/44100 (the keysound native rate) so the mixer can sum without per-sample
//! rate conversion.

use std::fs;
use std::path::{Path, PathBuf};
use std::ptr;
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

/// Decode one keysound blob (WMAv2 from s3p, or WAV/MS-ADPCM from 2dx) to interleaved stereo
/// f32 PCM at 44.1 kHz. The codec is auto-detected from the blob's contents.
pub fn decode_keysound(blob: &[u8]) -> Result<Vec<f32>> {
    ensure_init();
    let temp = TempFile::create(blob)?; // removed on drop (success / error / panic)
    decode_file_to_pcm(&temp.path)
    // `temp` drops here; the demuxer opened inside decode_file_to_pcm is already closed
}

// Map a source channel count to a canonical, mask-bearing layout for the resampler. We derive
// this from the count rather than trust the decoder: WMAv2 reports an unspecified-mask 2-channel
// layout, which makes ffmpeg-the-third's get2()/swr panic on `.mask().unwrap()`. Keysounds are
// only ever mono (some 2dx) or stereo (s3p WMA + many 2dx); anything else falls back to stereo.
fn source_layout(channels: u32) -> ffmpeg::ChannelLayout<'static> {
    match channels {
        1 => ffmpeg::ChannelLayout::MONO,
        _ => ffmpeg::ChannelLayout::STEREO,
    }
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

    // source layout derived from channel count (mono keysounds exist in 2dx); target is stereo.
    // ffmpeg 8.0 dropped the old `channels()`; read the count off the new ch_layout (nb_channels
    // is valid even when the layout carries no mask, as WMAv2 reports).
    let channels_source = decoder.ch_layout().channels();
    let mut resampler = ffmpeg::software::resampling::Context::get2(
        decoder.format(),
        source_layout(channels_source),
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
        drain_decoded(&mut decoder, &mut resampler, &mut pcm, channels_source)?;
    }
    decoder.send_eof().context("flushing decoder")?;
    drain_decoded(&mut decoder, &mut resampler, &mut pcm, channels_source)?;

    Ok(pcm)
}

// Pull every ready decoded frame, resample to f32/stereo/44.1k, append interleaved samples.
fn drain_decoded(
    decoder: &mut ffmpeg::decoder::Audio,
    resampler: &mut ffmpeg::software::resampling::Context,
    pcm: &mut Vec<f32>,
    channels_source: u32,
) -> Result<()> {
    let mut decoded = unsafe { ffmpeg::Frame::empty() };
    while decoder.receive_frame(&mut decoded).is_ok() {
        let mut decoded_audio = ffmpeg::frame::Audio::from(decoded);
        // Stamp the same source layout the resampler was configured with (the decoder's own
        // layout may carry no mask), so swr_convert_frame accepts the input frame.
        decoded_audio.set_ch_layout(source_layout(channels_source));
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

// ── Opus encode (Step 6) ───────────────────────────────────────────────────────
// Encode a mixed 44.1k stereo f32 timeline to Ogg/Opus: resample 44.1k -> 48k (libopus only
// accepts 48k), then chunk into 20 ms (960-sample) frames, encode, and mux into Ogg. Mirrors
// the reused SDVX transcode encode half.

const OPUS_BITRATE: usize = 192_000; // ~transparent for the lossy WMAv2 source
const OPUS_FRAME: usize = 960;       // 20 ms at 48 kHz, libopus's natural frame size
const ENCODE_CHANNELS: usize = 2;    // interleaved stereo
const FLUSH_FRAMES: usize = 8192;    // scratch size when draining the resampler tail

/// Encode an interleaved stereo f32 timeline at 44.1 kHz to an Ogg/Opus file.
pub fn encode_opus(timeline_44k: &[f32], output_path: &Path) -> Result<()> {
    ensure_init();

    let codec = ffmpeg::encoder::find_by_name("libopus")
        .ok_or_else(|| anyhow!("libopus encoder not found in vendored build"))?;
    let mut encoder = ffmpeg::codec::context::Context::new_with_codec(codec)
        .encoder()
        .audio()
        .context("creating libopus encoder")?;
    encoder.set_rate(48_000);
    encoder.set_ch_layout(ffmpeg::ChannelLayout::STEREO);
    encoder.set_format(ffmpeg::format::Sample::F32(ffmpeg::format::sample::Type::Packed));
    encoder.set_bit_rate(OPUS_BITRATE);
    encoder.set_time_base(ffmpeg::Rational(1, 48_000));
    let mut encoder = encoder.open().context("opening libopus encoder")?;

    let mut resampler = ffmpeg::software::resampling::Context::get2(
        ffmpeg::format::Sample::F32(ffmpeg::format::sample::Type::Packed),
        ffmpeg::ChannelLayout::STEREO,
        KEYSOUND_RATE,
        ffmpeg::format::Sample::F32(ffmpeg::format::sample::Type::Packed),
        ffmpeg::ChannelLayout::STEREO,
        48_000,
    )
    .context("creating 44.1k->48k resampler")?;

    let mut octx = ffmpeg::format::output(output_path)
        .with_context(|| format!("creating output {}", output_path.display()))?;
    let mut out_stream = octx.add_stream(codec).context("adding output stream")?;
    out_stream.copy_parameters_from_context(encoder.as_ref());
    out_stream.set_time_base(ffmpeg::Rational(1, 48_000));
    octx.write_header().context("writing Ogg header")?;

    // resample the whole 44.1k timeline to 48k: run() emits the bulk (its auto-sized output
    // can't hold the upsampled surplus), then flush() drains the buffered tail + filter delay
    // so the song's end is not lost.
    let frames_in = timeline_44k.len() / ENCODE_CHANNELS;
    let mut pcm_48k: Vec<f32> = Vec::new();
    if frames_in > 0 {
        let in_frame = make_audio_frame(timeline_44k, frames_in, KEYSOUND_RATE)?;
        let mut resampled = ffmpeg::frame::Audio::empty();
        resampler.run(&in_frame, &mut resampled).context("resampling to 48k")?;
        pcm_48k.extend_from_slice(read_stereo_packed(&resampled));
        loop {
            let mut tail = ffmpeg::frame::Audio::empty();
            tail.set_format(ffmpeg::format::Sample::F32(ffmpeg::format::sample::Type::Packed));
            tail.set_rate(48_000);
            tail.set_ch_layout(ffmpeg::ChannelLayout::STEREO);
            tail.set_samples(FLUSH_FRAMES);
            unsafe {
                av_frame_get_buffer(tail.as_mut_ptr(), 0);
            }
            resampler.flush(&mut tail).context("flushing resampler")?;
            if tail.samples() == 0 {
                break;
            }
            pcm_48k.extend_from_slice(read_stereo_packed(&tail));
        }
    }

    // encode 20 ms frames; pad the final partial frame with silence
    let mut pts: i64 = 0;
    for chunk in pcm_48k.chunks(OPUS_FRAME * ENCODE_CHANNELS) {
        let mut frame_data = chunk.to_vec();
        frame_data.resize(OPUS_FRAME * ENCODE_CHANNELS, 0.0);
        let mut chunk_frame = make_audio_frame(&frame_data, OPUS_FRAME, 48_000)?;
        chunk_frame.set_pts(Some(pts));
        encoder.send_frame(&chunk_frame).context("sending frame to encoder")?;
        receive_and_mux(&mut encoder, &mut octx);
        pts += OPUS_FRAME as i64;
    }

    encoder.send_eof().context("flushing encoder")?;
    receive_and_mux(&mut encoder, &mut octx);
    octx.write_trailer().context("writing Ogg trailer")?;
    Ok(())
}

// allocate a packed-stereo f32 AVFrame of `samples` samples at `rate` and copy `data` in
fn make_audio_frame(data: &[f32], samples: usize, rate: u32) -> Result<ffmpeg::frame::Audio> {
    let mut audio = ffmpeg::frame::Audio::empty();
    audio.set_format(ffmpeg::format::Sample::F32(ffmpeg::format::sample::Type::Packed));
    audio.set_rate(rate);
    audio.set_samples(samples);
    audio.set_ch_layout(ffmpeg::ChannelLayout::STEREO);
    unsafe {
        av_frame_get_buffer(audio.as_mut_ptr(), 0);
    }
    let dst: &mut [(f32, f32)] = audio.plane_mut::<(f32, f32)>(0);
    unsafe {
        ptr::copy_nonoverlapping(data.as_ptr(), dst.as_mut_ptr() as *mut f32, data.len());
    }
    Ok(audio)
}

// drain encoded packets from the encoder and mux them into the Ogg output
fn receive_and_mux(
    encoder: &mut ffmpeg::encoder::audio::Encoder,
    octx: &mut ffmpeg::format::context::Output,
) {
    let mut encoded = ffmpeg::Packet::empty();
    while encoder.receive_packet(&mut encoded).is_ok() {
        encoded.set_stream(0);
        if let Err(error) = encoded.write_interleaved(octx) {
            eprintln!("encode warning: writing packet failed: {error}");
        }
        encoded = ffmpeg::Packet::empty();
    }
}

// av_frame_get_buffer is not re-exported through the safe wrapper
unsafe extern "C" {
    fn av_frame_get_buffer(
        frame: *mut ffmpeg_the_third::ffi::AVFrame,
        align: std::ffi::c_int,
    ) -> std::ffi::c_int;
}
