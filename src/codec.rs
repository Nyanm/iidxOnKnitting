//! libav FFI: decode keysound blobs to 44.1k stereo f32 PCM (Step 4), and encode a mixed 44.1k
//! timeline to Ogg/Opus (Step 6).
//!
//! Each blob is demuxed straight from memory via a custom AVIO context (a read+seek callback
//! over the byte slice) — no temp file, no disk I/O (Step 9; on Windows the old temp-file churn
//! dominated runtime, ~6ms/keysound). The decode loop resamples to f32/stereo/44100 (the
//! keysound native rate) so the mixer can sum without per-sample rate conversion.

use std::borrow::Cow;
use std::ffi::{c_int, c_void};
use std::path::Path;
use std::ptr;
use std::slice;
use std::sync::Once;

use anyhow::{Context, Result, anyhow, ensure};
use ffmpeg_the_third as ffmpeg;

/// The rate [`decode_keysound`] resamples to. IIDX and SDVX audio is all 44.1 kHz, so their
/// pipelines mix there and resample once at encode time; GITADORA is 48 kHz and bypasses that.
pub const KEYSOUND_RATE: u32 = 44_100;

static INIT: Once = Once::new();

// ffmpeg init is idempotent but not free; run it exactly once across all threads.
fn ensure_init() {
    INIT.call_once(|| {
        ffmpeg::init().expect("ffmpeg vendored init failed");
        // silence per-frame libav warnings (e.g. WMA "skipped samples") across ~1186 decodes
        ffmpeg::util::log::set_level(ffmpeg::util::log::Level::Error);
    });
}

const AVIO_BUFFER: usize = 4096;          // libav read-chunk size for our in-memory AVIO
const AVERROR_EOF: c_int = -541_478_725;  // FFERRTAG('E','O','F',' '): end of the blob

// in-memory read cursor handed to the AVIO callbacks via the opaque pointer
struct BlobReader {
    data: *const u8,
    len: usize,
    pos: usize,
}

// AVIO read callback: copy up to buf_size bytes from the blob, advance, return count (or EOF).
unsafe extern "C" fn blob_read(opaque: *mut c_void, buf: *mut u8, buf_size: c_int) -> c_int {
    let reader = unsafe { &mut *(opaque as *mut BlobReader) };
    let remaining = reader.len - reader.pos;
    if remaining == 0 {
        return AVERROR_EOF;
    }
    let count = remaining.min(buf_size.max(0) as usize);
    unsafe { ptr::copy_nonoverlapping(reader.data.add(reader.pos), buf, count) };
    reader.pos += count;
    count as c_int
}

// AVIO seek callback: move the cursor; AVSEEK_SIZE asks for the total length. Returns new pos / -1.
unsafe extern "C" fn blob_seek(opaque: *mut c_void, offset: i64, whence: c_int) -> i64 {
    let reader = unsafe { &mut *(opaque as *mut BlobReader) };
    let whence = whence & !ffmpeg::ffi::AVSEEK_FORCE;
    if whence == ffmpeg::ffi::AVSEEK_SIZE {
        return reader.len as i64;
    }
    let new_pos = match whence {
        0 => offset,                       // SEEK_SET
        1 => reader.pos as i64 + offset,   // SEEK_CUR
        2 => reader.len as i64 + offset,   // SEEK_END
        _ => return -1,
    };
    if new_pos < 0 || new_pos > reader.len as i64 {
        return -1;
    }
    reader.pos = new_pos as usize;
    new_pos
}

// Frees the AVIO context + its (possibly realloc'd) buffer. Held alongside the Input so teardown
// order is deterministic: the Input (close_input) drops first, then this, then the cursor.
struct AvioGuard {
    avio: *mut ffmpeg::ffi::AVIOContext,
}

impl Drop for AvioGuard {
    fn drop(&mut self) {
        unsafe {
            ffmpeg::ffi::av_freep((&mut (*self.avio).buffer) as *mut *mut u8 as *mut c_void);
            ffmpeg::ffi::avio_context_free(&mut self.avio);
        }
    }
}

/// Decode one keysound blob (WMAv2 from s3p, or WAV/MS-ADPCM from 2dx) to interleaved stereo
/// f32 PCM at 44.1 kHz. The codec is auto-detected; the blob is demuxed in-memory (no temp file).
pub fn decode_keysound(blob: &[u8]) -> Result<Vec<f32>> {
    ensure_init();

    // cursor + AVIO must outlive the Input; declared first so they drop last (after close_input)
    let mut cursor = BlobReader { data: blob.as_ptr(), len: blob.len(), pos: 0 };
    let opaque = (&mut cursor as *mut BlobReader).cast::<c_void>();

    let guard = unsafe {
        let buffer = ffmpeg::ffi::av_malloc(AVIO_BUFFER) as *mut u8;
        ensure!(!buffer.is_null(), "av_malloc for AVIO buffer failed");
        let avio = ffmpeg::ffi::avio_alloc_context(
            buffer,
            AVIO_BUFFER as c_int,
            0, // read-only
            opaque,
            Some(blob_read),
            None, // no write callback
            Some(blob_seek),
        );
        if avio.is_null() {
            ffmpeg::ffi::av_free(buffer as *mut c_void); // avio didn't take ownership on failure
            anyhow::bail!("avio_alloc_context failed");
        }
        AvioGuard { avio }
    };

    let input = unsafe {
        let mut format_ctx = ffmpeg::ffi::avformat_alloc_context();
        ensure!(!format_ctx.is_null(), "avformat_alloc_context failed");
        (*format_ctx).pb = guard.avio;
        (*format_ctx).flags |= ffmpeg::ffi::AVFMT_FLAG_CUSTOM_IO; // keep close_input off our pb
        let ret = ffmpeg::ffi::avformat_open_input(
            &mut format_ctx,
            ptr::null(),     // url (unused: we read via the custom AVIO)
            ptr::null(),     // fmt: let libav probe the container
            ptr::null_mut(), // options
        );
        ensure!(ret >= 0, "avformat_open_input (in-memory) failed: {ret}");
        ffmpeg::format::context::Input::wrap(format_ctx)
    };

    decode_input(input)
    // drop order: input (close_input) -> guard (free AVIO) -> cursor
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

// Decode an opened audio Input (any vendored-supported codec) to interleaved stereo f32 @ 44.1k.
fn decode_input(mut ictx: ffmpeg::format::context::Input) -> Result<Vec<f32>> {
    let input_stream = ictx
        .streams()
        .best(ffmpeg::media::Type::Audio)
        .ok_or_else(|| anyhow!("no audio stream in keysound blob"))?;
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

// ── Opus encode (Step 6) ───────────────────────────────────────────────────────
// Encode a mixed stereo f32 timeline to Ogg/Opus: resample to 48k if needed (libopus only accepts
// 48k), then chunk into 20 ms (960-sample) frames, encode, and mux into Ogg. Mirrors the reused
// SDVX transcode encode half.

const OPUS_RATE: u32 = 48_000;       // the only rate libopus accepts
const OPUS_BITRATE: usize = 192_000; // ~transparent for the lossy WMAv2 source
const OPUS_FRAME: usize = 960;       // 20 ms at 48 kHz, libopus's natural frame size
const ENCODE_CHANNELS: usize = 2;    // interleaved stereo
const FLUSH_FRAMES: usize = 8192;    // scratch size when draining the resampler tail

/// Encode an interleaved stereo f32 timeline at `rate_in_hz` to an Ogg/Opus file. A 48 kHz input
/// (GITADORA's native rate) bypasses the resampler entirely; 44.1 kHz (IIDX/SDVX keysounds) is
/// resampled up first.
pub fn encode_opus(timeline: &[f32], rate_in_hz: u32, output_path: &Path) -> Result<()> {
    ensure_init();
    ensure!(rate_in_hz > 0, "encode_opus called with a zero input rate");

    let codec = ffmpeg::encoder::find_by_name("libopus")
        .ok_or_else(|| anyhow!("libopus encoder not found in vendored build"))?;
    let mut encoder = ffmpeg::codec::context::Context::new_with_codec(codec)
        .encoder()
        .audio()
        .context("creating libopus encoder")?;
    encoder.set_rate(OPUS_RATE as i32);
    encoder.set_ch_layout(ffmpeg::ChannelLayout::STEREO);
    encoder.set_format(ffmpeg::format::Sample::F32(ffmpeg::format::sample::Type::Packed));
    encoder.set_bit_rate(OPUS_BITRATE);
    encoder.set_time_base(ffmpeg::Rational(1, OPUS_RATE as i32));
    let mut encoder = encoder.open().context("opening libopus encoder")?;

    let mut octx = ffmpeg::format::output(output_path)
        .with_context(|| format!("creating output {}", output_path.display()))?;
    let mut out_stream = octx.add_stream(codec).context("adding output stream")?;
    out_stream.copy_parameters_from_context(encoder.as_ref());
    out_stream.set_time_base(ffmpeg::Rational(1, OPUS_RATE as i32));
    octx.write_header().context("writing Ogg header")?;

    let pcm_48k = resample_to_opus_rate(timeline, rate_in_hz)?;

    // encode 20 ms frames; pad the final partial frame with silence
    let mut pts: i64 = 0;
    for chunk in pcm_48k.chunks(OPUS_FRAME * ENCODE_CHANNELS) {
        let mut frame_data = chunk.to_vec();
        frame_data.resize(OPUS_FRAME * ENCODE_CHANNELS, 0.0);
        let mut chunk_frame = make_audio_frame(&frame_data, OPUS_FRAME, OPUS_RATE)?;
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

/* Bring an interleaved-stereo f32 timeline to libopus's 48 kHz. A timeline already at 48 kHz is
borrowed unchanged — GITADORA is natively 48 kHz, and passing it through swresample would only add a
resampler's filter delay and rounding for no benefit. Otherwise run() emits the bulk (its auto-sized
output cannot hold the upsampled surplus) and flush() drains the buffered tail plus the filter delay,
so the song's end is not lost. */
fn resample_to_opus_rate(timeline: &[f32], rate_in_hz: u32) -> Result<Cow<'_, [f32]>> {
    if rate_in_hz == OPUS_RATE {
        return Ok(Cow::Borrowed(timeline));
    }
    let frames_in = timeline.len() / ENCODE_CHANNELS;
    if frames_in == 0 {
        return Ok(Cow::Borrowed(timeline));
    }

    let mut resampler = ffmpeg::software::resampling::Context::get2(
        ffmpeg::format::Sample::F32(ffmpeg::format::sample::Type::Packed),
        ffmpeg::ChannelLayout::STEREO,
        rate_in_hz,
        ffmpeg::format::Sample::F32(ffmpeg::format::sample::Type::Packed),
        ffmpeg::ChannelLayout::STEREO,
        OPUS_RATE,
    )
    .with_context(|| format!("creating {rate_in_hz}->{OPUS_RATE} resampler"))?;

    let mut pcm_48k: Vec<f32> = Vec::new();
    let in_frame = make_audio_frame(timeline, frames_in, rate_in_hz)?;
    let mut resampled = ffmpeg::frame::Audio::empty();
    resampler.run(&in_frame, &mut resampled).context("resampling to 48k")?;
    pcm_48k.extend_from_slice(read_stereo_packed(&resampled));
    loop {
        let mut tail = ffmpeg::frame::Audio::empty();
        tail.set_format(ffmpeg::format::Sample::F32(ffmpeg::format::sample::Type::Packed));
        tail.set_rate(OPUS_RATE);
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
    Ok(Cow::Owned(pcm_48k))
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
        align: c_int,
    ) -> c_int;
}
