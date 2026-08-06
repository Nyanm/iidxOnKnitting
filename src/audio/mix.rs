//! Pure-Rust stereo mixing timeline, shared by every game's renderer.
//!
/*
One growable interleaved-stereo f32 buffer at a fixed sample rate, onto which callers sum sources at
frame offsets. Everything is f32 in -1.0..1.0, so i16 sources are scaled by 1/32768 on the way in
and the mastering stage in `super::master` can work in units of full scale regardless of who filled
the timeline.

The rate is carried by the timeline rather than hardcoded because the games differ: IIDX keysounds
are 44.1 kHz (so its whole pipeline mixes there and resamples once at encode time) while GITADORA is
natively 48 kHz and can go straight to Opus untouched.

`add_*` grows the buffer as needed, but callers that know their final length should still call
`ensure_frames` once up front — a few thousand keysounds each nudging the length would otherwise
realloc far more than necessary.
*/

const CHANNELS: usize = 2;         // interleaved stereo
const I16_FULL_SCALE: f32 = 32768.0; // i16 -> -1.0..1.0

/// An interleaved-stereo f32 mixing buffer at a fixed rate.
pub struct Timeline {
    samples: Vec<f32>,
    rate_hz: u32,
}

impl Timeline {
    /// An empty timeline at `rate_hz`.
    pub fn new(rate_hz: u32) -> Self {
        Timeline { samples: Vec::new(), rate_hz }
    }

    /// A timeline seeded with an interleaved-stereo i16 bed (GITADORA's backing track).
    pub fn from_stereo_i16(rate_hz: u32, bed: &[i16]) -> Self {
        let samples = bed.iter().map(|sample| *sample as f32 / I16_FULL_SCALE).collect();
        Timeline { samples, rate_hz }
    }

    pub fn rate_hz(&self) -> u32 {
        self.rate_hz
    }

    /// Length in stereo frames.
    pub fn frames(&self) -> usize {
        self.samples.len() / CHANNELS
    }

    /// Grow (never shrink) the timeline to hold at least `frames` stereo frames.
    pub fn ensure_frames(&mut self, frames: usize) {
        if frames * CHANNELS > self.samples.len() {
            self.samples.resize(frames * CHANNELS, 0.0);
        }
    }

    /// Sum an interleaved-stereo f32 source in at `frame_start`, scaled by `gain`.
    pub fn add_stereo_f32(&mut self, frame_start: usize, pcm: &[f32], gain: f32) {
        self.ensure_frames(frame_start + pcm.len() / CHANNELS);
        let offset = frame_start * CHANNELS;
        for (index_sample, value) in pcm.iter().enumerate() {
            self.samples[offset + index_sample] += value * gain;
        }
    }

    /// Sum a mono i16 source in at `frame_start`, panned by separate per-channel gains.
    pub fn add_mono_i16(&mut self, frame_start: usize, pcm: &[i16], gain_left: f32, gain_right: f32) {
        self.ensure_frames(frame_start + pcm.len());
        let offset = frame_start * CHANNELS;
        for (index_frame, value) in pcm.iter().enumerate() {
            let value = *value as f32 / I16_FULL_SCALE;
            self.samples[offset + index_frame * CHANNELS] += value * gain_left;
            self.samples[offset + index_frame * CHANNELS + 1] += value * gain_right;
        }
    }

    /// Milliseconds to a frame index at this timeline's rate (sub-frame rounding is ~20 us).
    pub fn ms_to_frame(&self, time_ms: u32) -> usize {
        (time_ms as u64 * self.rate_hz as u64 / 1000) as usize
    }

    pub fn samples(&self) -> &[f32] {
        &self.samples
    }

    pub fn samples_mut(&mut self) -> &mut [f32] {
        &mut self.samples
    }

    pub fn into_samples(self) -> Vec<f32> {
        self.samples
    }

    /// Length in seconds, for logging.
    pub fn seconds(&self) -> f64 {
        self.frames() as f64 / self.rate_hz as f64
    }
}

/// Split a GITADORA pan byte (0..127, 64 = centre) into per-channel gains.
///
/// A unity-centre balance law: a centred sound keeps full level in both channels and panning only
/// attenuates the opposite side. Constant-power (sin/cos) panning was rejected because it would
/// drop every centred keysound by 3 dB relative to the bed, which the game does not do.
pub fn pan_gains(pan: u8) -> (f32, f32) {
    const PAN_CENTRE: f32 = 64.0;
    let offset = (pan as f32 - PAN_CENTRE) / PAN_CENTRE;
    (1.0 - offset.max(0.0), 1.0 + offset.min(0.0))
}
