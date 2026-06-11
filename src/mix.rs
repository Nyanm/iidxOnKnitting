//! Pure-Rust mixer: lay each decoded keysound's PCM onto a 44.1k stereo timeline at its
//! sounding event's sample offset (summing overlaps), then peak-normalize so the result
//! never clips. All PCM here is interleaved stereo f32 at 44.1 kHz (see codec / S3P_FORMAT).

use crate::chart::Sounding;

const RATE: u32 = 44_100;  // mixing sample rate (the keysound native rate)
const CHANNELS: usize = 2; // interleaved stereo

/// Mix sounding events into one interleaved-stereo f32 timeline at 44.1 kHz.
///
/// `pcm_cache[sample - 1]` holds each referenced sample's decoded PCM (None = unused or
/// not decoded). `duration_ms` is the chart's last-event time; the timeline is sized to the
/// later of that and the end of the last keysound, so trailing audio is never clipped.
pub fn render(events: &[Sounding], pcm_cache: &[Option<Vec<f32>>], duration_ms: u32) -> Vec<f32> {
    // timeline length (in stereo frames) = max(chart duration, end of every keysound)
    let duration_frames = ms_to_frame(duration_ms);
    let mut content_end_frames = 0usize;
    for sounding in events {
        if let Some(pcm) = sample_pcm(pcm_cache, sounding.sample_1based) {
            content_end_frames =
                content_end_frames.max(ms_to_frame(sounding.time_ms) + pcm.len() / CHANNELS);
        }
    }
    let length_frames = content_end_frames.max(duration_frames);

    let mut timeline = vec![0.0f32; length_frames * CHANNELS];
    for sounding in events {
        let Some(pcm) = sample_pcm(pcm_cache, sounding.sample_1based) else {
            continue;
        };
        // timeline is sized to the furthest keysound end, so the whole sample always fits
        let offset = ms_to_frame(sounding.time_ms) * CHANNELS;
        for (index_sample, value) in pcm.iter().copied().enumerate() {
            timeline[offset + index_sample] += value;
        }
    }

    peak_normalize(&mut timeline);
    timeline
}

// convert milliseconds to a stereo-frame index at the mixing rate (sub-ms rounding is ~11us)
fn ms_to_frame(time_ms: u32) -> usize {
    (time_ms as u64 * RATE as u64 / 1000) as usize
}

// look up a 1-based sample's decoded PCM, if present and in range
fn sample_pcm(pcm_cache: &[Option<Vec<f32>>], sample_1based: u16) -> Option<&Vec<f32>> {
    let index = (sample_1based as usize).checked_sub(1)?;
    pcm_cache.get(index)?.as_ref()
}

// scale the whole buffer so its peak magnitude is at most 1.0 (only when it exceeds 1.0),
// preserving the relative balance between keysounds instead of hard-clipping
fn peak_normalize(timeline: &mut [f32]) {
    let peak = timeline.iter().fold(0.0f32, |max_abs, value| max_abs.max(value.abs()));
    if peak > 1.0 {
        let gain = 1.0 / peak;
        for value in timeline.iter_mut() {
            *value *= gain;
        }
    }
}
