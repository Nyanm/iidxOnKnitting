//! Konami's 4-bit ADPCM codec (decode only).
//!
/*
This is Konami's own variant, not IMA/MS ADPCM, and no FFmpeg decoder implements it — hence the
hand-rolled port rather than another vendored decoder. It is a pure nibble stream: no per-block
header, no predictor coefficients, no initial-state preamble. Every stream starts from
step_index = 0 and pcm = 0, so a payload can be decoded from its first byte with no framing.

Two packings appear in GITADORA, and they differ in more than channel count:
  - stereo: one byte is ONE stereo frame — high nibble feeds the left decoder, low nibble the
    right, and the two channels keep INDEPENDENT step/pcm state.
  - mono:   one byte is TWO consecutive samples (high nibble first), both advancing the SAME state.
Confusing the two still yields plausible-looking audio, just at half or double the intended rate,
which is why the regression test asserts on frame counts and not only on how the result sounds.

Reference: the `d`/`e` modes of `_misc/adpcmwavetool.cpp` in fisyher/gitadora-customs.
*/

// step size per step_index; index is clamped to 0..=48 so the table is exactly 49 entries
const STEPS: [i32; 49] = [
    256, 272, 304, 336, 368, 400, 448, 496, 544, 592, 656, 720,
    800, 880, 960, 1056, 1168, 1280, 1408, 1552, 1712, 1888, 2080, 2288,
    2512, 2768, 3040, 3344, 3680, 4048, 4464, 4912, 5392, 5936, 6528, 7184,
    7904, 8704, 9568, 10528, 11584, 12736, 14016, 15408, 16960, 18656, 20512, 22576,
    24832,
];

// step_index delta per 4-bit code; the sign bit (0x08) does not change the magnitude, so the
// upper half repeats the lower half
const CHANGES: [i32; 16] = [-1, -1, -1, -1, 2, 4, 6, 8, -1, -1, -1, -1, 2, 4, 6, 8];

const STEP_INDEX_MAX: i32 = (STEPS.len() - 1) as i32;
const CODE_SIGN: u8 = 0x08; // bit 3 of a code negates the delta

/// One channel's running ADPCM state. Every stream starts at the origin, so `Default` is the
/// correct and only way to begin decoding.
#[derive(Default)]
struct Decoder {
    step_index: i32,
    pcm: i32,
}

impl Decoder {
    // Advance by one 4-bit code and return the new PCM sample.
    fn next_sample(&mut self, code: u8) -> i16 {
        let step = STEPS[self.step_index as usize];
        // delta = step/8 + step/4*bit0 + step/2*bit1 + step*bit2 (the bit-mask form in the original)
        let mut delta = step >> 3;
        if code & 1 != 0 { delta += step >> 2; }
        if code & 2 != 0 { delta += step >> 1; }
        if code & 4 != 0 { delta += step; }

        self.step_index = (self.step_index + CHANGES[(code & 0x0F) as usize]).clamp(0, STEP_INDEX_MAX);
        if code & CODE_SIGN != 0 { delta = -delta; }

        self.pcm = (self.pcm + delta).clamp(i16::MIN as i32, i16::MAX as i32);
        self.pcm as i16
    }
}

/// Decode a stereo payload: one byte per stereo frame, high nibble left, low nibble right, one
/// independent decoder per channel. Returns interleaved `[L, R, L, R, …]`, so
/// `output.len() == payload.len() * 2`.
pub fn decode_stereo(payload: &[u8]) -> Vec<i16> {
    let mut decoder_left = Decoder::default();
    let mut decoder_right = Decoder::default();
    let mut samples = Vec::with_capacity(payload.len() * 2);
    for byte in payload {
        samples.push(decoder_left.next_sample(byte >> 4));
        samples.push(decoder_right.next_sample(byte & 0x0F));
    }
    samples
}

/// Decode a mono payload: one byte per two consecutive samples (high nibble first), both sharing
/// one decoder state. Returns `payload.len() * 2` samples.
pub fn decode_mono(payload: &[u8]) -> Vec<i16> {
    let mut decoder = Decoder::default();
    let mut samples = Vec::with_capacity(payload.len() * 2);
    for byte in payload {
        samples.push(decoder.next_sample(byte >> 4));
        samples.push(decoder.next_sample(byte & 0x0F));
    }
    samples
}
