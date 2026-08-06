//! Final gain staging on a mixed timeline: peak normalisation and soft-knee limiting.
//!
/*
Two strategies, because the two kinds of source need different treatment.

IIDX reconstructs a song purely from keysounds, so the sum's absolute level is arbitrary and
`peak_normalize` — scale down only if the peak exceeds full scale — is both sufficient and lossless.

GITADORA lays keysounds over a bed that is already a finished, loudness-maximised master, so the sum
runs hot: measured on a full song, the peak reaches 2.81x full scale while only 2.33% of samples
exceed 1.0 at all (p99 = 1.19, p99.9 = 1.68). Peak-normalising that thin tail would cost 9 dB and
leave the result ~7 dB quieter than the game's own mixes. Instead the keysound layer is attenuated by
`KEYSOUND_GAIN` and the remainder folded by `soft_knee_limit`. Calibrated across three songs, that
pair lands within 0.6 dB of the game's own masters while touching under 0.61% of samples, leaving
the added distortion 27 dB or more below the signal.
*/

/// Gain applied to GITADORA's keysound layer before it is summed onto the bed (-3.1 dB).
///
/// Fixed rather than per-song adaptive: the bed passes through untouched, so absolute loudness
/// follows each song's own mastering (the game is quieter on old songs too) and only the balance
/// between bed and keysounds is pinned. Measured against the game's own pre-mixed variants, 1.0
/// leaves the sum ~1.9 dB hot with 2.7% of samples clipping, and 0.5 lands 2.2 dB under.
pub const KEYSOUND_GAIN: f32 = 0.70;

/// Where the soft knee starts. Loudness is nearly independent of this (within 0.04 dB across
/// 0.70..0.95), but a higher threshold touches far fewer samples, so it sits just under full scale.
pub const KNEE_THRESHOLD: f32 = 0.95;

/// Scale the whole buffer so its peak magnitude is at most 1.0, and only when it exceeds 1.0.
/// Preserves the relative balance between sources instead of hard-clipping.
pub fn peak_normalize(samples: &mut [f32]) {
    let peak = samples.iter().fold(0.0f32, |max_abs, value| max_abs.max(value.abs()));
    if peak > 1.0 {
        let gain = 1.0 / peak;
        for value in samples.iter_mut() {
            *value *= gain;
        }
    }
}

/// Fold everything above `threshold` smoothly towards full scale, leaving quieter samples untouched.
///
/// `|x| <= threshold` passes through unchanged; above it the excess is compressed by `tanh`, which is
/// continuous and monotone at the knee and asymptotic to 1.0, so the output never EXCEEDS full scale.
/// It does reach it: `tanh` saturates to exactly 1.0 in f32 once the input is a few multiples of the
/// knee width above the threshold, which a 2x-hot mix easily is. Stateless and sample-local: no
/// look-ahead, no attack/release, therefore no pumping — at the cost of the mild harmonic distortion
/// inherent to waveshaping.
pub fn soft_knee_limit(samples: &mut [f32], threshold: f32) {
    debug_assert!((0.0..1.0).contains(&threshold), "knee threshold must be inside 0.0..1.0");
    let span = 1.0 - threshold;
    for value in samples.iter_mut() {
        let magnitude = value.abs();
        if magnitude > threshold {
            let shaped = threshold + span * ((magnitude - threshold) / span).tanh();
            *value = shaped * value.signum();
        }
    }
}

/// Peak magnitude of a buffer, for logging how hard the knee had to work.
pub fn peak(samples: &[f32]) -> f32 {
    samples.iter().fold(0.0f32, |max_abs, value| max_abs.max(value.abs()))
}
