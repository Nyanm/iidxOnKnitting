//! Regression test for the GITADORA render pipeline, from `.ifs` pair to mastered timeline.
//!
//! It drives the real entry path — IFS routing, bed selection, chart and keysound parsing, overlay,
//! gain staging — and asserts on what came out. It stops short of the Opus encoder, which is shared
//! unchanged with the IIDX path, so a failure here is always on the GITADORA side.
//!
//! The reference figures are pinned from two deliberately different songs: a modern one whose bed
//! carries no instruments at all, and an early one that exercises the awkward cases (a pre-v2
//! keysound archive whose last entry overruns the file, a bass-only bed, chart difficulties that
//! disagree on their note sets, and notes hidden in a "metadata" chunk). The loudness figures come
//! from the calibration that fixed `KEYSOUND_GAIN` and `KNEE_THRESHOLD`, measured with an independent
//! pure-Python renderer, so they also guard against the mix drifting off the level the game's own
//! pre-mixed masters sit at.
//!
//! Run:  cargo run -r --example regression_gitadora -- <m<id>_seq.ifs> <m<id>_bgm.ifs> [--wav out.wav]

use std::env;
use std::fs;
use std::path::PathBuf;
use std::process::ExitCode;

use iidx_on_knitting::audio::master;
use iidx_on_knitting::gitadora::song;

/// Reference figures per song. `rms_db` is of the mono downmix, matching how the calibration
/// measured it; `tolerance_db` covers the tick-rounding differences between the two renderers.
struct Expected {
    label: &'static str,
    seq_ends_with: &'static str,
    bed_name: &'static str,
    frames: usize,
    notes: usize,
    /// Notes the game's own data points at a sound_id its archive does not hold. Not always zero:
    /// 21 of the library's 1359 renderable songs reference null placeholder ids (0/1/2) the way IIDX
    /// charts reference reserved sample numbers, so this is pinned per song rather than asserted to
    /// be zero everywhere.
    unresolved: usize,
    rms_db: f64,
    tolerance_db: f64,
}

const EXPECTED: &[Expected] = &[
    Expected {
        // the mainline case: a backing-only bed carrying drum + guitar + bass keysounds
        label: "m1800",
        seq_ends_with: "m1800_seq.ifs",
        bed_name: "bgm1800___k.bin",
        frames: 5_181_092,
        notes: 2144, // 1248 drum + 439 guitar + 457 bass, each with its own keysound
        unresolved: 0,
        rms_db: -10.50,
        tolerance_db: 0.5,
    },
    Expected {
        /* The fallback case: no backing-only bed exists, so the bass-only bed is used and the bass
        chart is skipped. Also a pre-v2 keysound archive whose last entry overruns the file. */
        label: "m0207",
        seq_ends_with: "m0207_seq.ifs",
        bed_name: "bgm0207__bk.bin",
        frames: 4_359_129,
        notes: 1186, // 917 drum (the union beats any single difficulty's 902) + 239 guitar
                     // + 30 from the metadata chunk; bass is already in the bed
        unresolved: 0,
        // the calibration measured -12.05 without the metadata chunk's 30 notes, which this pipeline
        // does play, so the mix sits a little louder
        rms_db: -11.83,
        tolerance_db: 0.5,
    },
];

const RATE_EXPECTED: u32 = 48_000;

fn main() -> ExitCode {
    let args: Vec<String> = env::args().collect();
    if args.len() < 3 {
        eprintln!("usage: {} <m<id>_seq.ifs> <m<id>_bgm.ifs> [--wav out.wav]", args[0]);
        return ExitCode::FAILURE;
    }
    let path_seq = PathBuf::from(&args[1]);
    let path_bgm = PathBuf::from(&args[2]);
    let path_wav = args.iter().position(|arg| arg == "--wav").and_then(|index| args.get(index + 1));

    let expected = EXPECTED
        .iter()
        .find(|row| path_seq.to_string_lossy().replace('\\', "/").ends_with(row.seq_ends_with));
    let mut vec_failure: Vec<String> = Vec::new();

    let bytes_seq = match fs::read(&path_seq) {
        Ok(bytes) => bytes,
        Err(error) => {
            eprintln!("FATAL: reading {}: {error}", path_seq.display());
            return ExitCode::FAILURE;
        }
    };
    let bytes_bgm = match fs::read(&path_bgm) {
        Ok(bytes) => bytes,
        Err(error) => {
            eprintln!("FATAL: reading {}: {error}", path_bgm.display());
            return ExitCode::FAILURE;
        }
    };

    let mut assembled = match song::mix_song(&bytes_seq, &bytes_bgm) {
        Ok(assembled) => assembled,
        Err(error) => {
            eprintln!("FATAL: {error:#}");
            return ExitCode::FAILURE;
        }
    };
    let timeline = &mut assembled.timeline;

    let peak_before = master::peak(timeline.samples());
    master::soft_knee_limit(timeline.samples_mut(), master::KNEE_THRESHOLD);
    let peak_after = master::peak(timeline.samples());

    // mono downmix, so the figure is comparable with the calibration's
    let samples = timeline.samples();
    let frames = timeline.frames();
    let mut energy = 0.0f64;
    let mut cnt_shaped = 0usize;
    for frame in samples.chunks(2) {
        let mono = (frame[0] as f64 + frame[1] as f64) * 0.5;
        energy += mono * mono;
        if frame.iter().any(|value| value.abs() >= master::KNEE_THRESHOLD) {
            cnt_shaped += 1;
        }
    }
    let rms_db = 20.0 * (energy / frames as f64).sqrt().log10();

    println!("frames={frames} ({:.3}s) @ {} Hz", timeline.seconds(), timeline.rate_hz());
    println!("peak before knee = {peak_before:.3}x full scale, after = {peak_after:.4}");
    println!("mono rms = {rms_db:.2} dBFS, frames at or above the knee = {:.3}%",
             cnt_shaped as f64 / frames as f64 * 100.0);

    check(&mut vec_failure, "timeline rate", timeline.rate_hz() == RATE_EXPECTED,
          format!("{} Hz, expected {RATE_EXPECTED}", timeline.rate_hz()));
    // the knee is asymptotic to 1.0, so nothing may go PAST full scale however hot the input was
    // (it does land exactly on 1.0: tanh saturates in f32 well before a 2x-hot peak)
    check(&mut vec_failure, "nothing exceeds full scale after the knee", peak_after <= 1.0,
          format!("peak {peak_after:.6}"));
    check(&mut vec_failure, "the knee had work to do", peak_before > master::KNEE_THRESHOLD,
          format!("peak before the knee was only {peak_before:.3}"));
    // a mis-read sound_id field would strand most notes, not a handful, so flag only a wholesale loss
    let share_unresolved = assembled.cnt_unresolved as f64 / (assembled.cnt_note + assembled.cnt_unresolved).max(1) as f64;
    check(&mut vec_failure, "keysound lookup is not wholesale broken", share_unresolved < 0.1,
          format!("{:.1}% of notes found no VA3 entry", share_unresolved * 100.0));

    if let Some(row) = expected {
        println!("checking against reference figures for {}", row.label);
        check(&mut vec_failure, "bed chosen", assembled.bed_name == row.bed_name,
              format!("{}, expected {}", assembled.bed_name, row.bed_name));
        check(&mut vec_failure, "keysounds laid", assembled.cnt_note == row.notes,
              format!("{}, expected {}", assembled.cnt_note, row.notes));
        check(&mut vec_failure, "unresolved notes", assembled.cnt_unresolved == row.unresolved,
              format!("{}, expected {}", assembled.cnt_unresolved, row.unresolved));
        check(&mut vec_failure, "frame count", frames == row.frames,
              format!("{frames}, expected {}", row.frames));
        check(&mut vec_failure, "loudness matches the calibration",
              (rms_db - row.rms_db).abs() <= row.tolerance_db,
              format!("{rms_db:.2} dBFS, expected {:.2} +/- {:.2}", row.rms_db, row.tolerance_db));
    } else {
        println!("no reference figures for this song; structural checks only");
    }

    if let Some(path) = path_wav {
        if let Err(error) = write_wav(path, samples, timeline.rate_hz()) {
            eprintln!("warning: writing {path}: {error}");
        } else {
            println!("wrote {path}");
        }
    }

    if vec_failure.is_empty() {
        println!("\nOK");
        ExitCode::SUCCESS
    } else {
        println!("\nFAILED: {vec_failure:?}");
        ExitCode::FAILURE
    }
}

fn check(vec_failure: &mut Vec<String>, label: &str, is_ok: bool, detail: String) {
    if !is_ok {
        println!("  FAIL {label}: {detail}");
        vec_failure.push(label.to_string());
    }
}

// Minimal 16-bit PCM WAV writer, so a run can be listened to or diffed against another renderer.
fn write_wav(path: &str, samples: &[f32], rate_hz: u32) -> std::io::Result<()> {
    let bytes_per_frame = 4u32; // stereo * i16
    let size_data = samples.len() as u32 * 2;
    let mut out: Vec<u8> = Vec::with_capacity(44 + size_data as usize);
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&(36 + size_data).to_le_bytes());
    out.extend_from_slice(b"WAVEfmt ");
    out.extend_from_slice(&16u32.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes()); // PCM
    out.extend_from_slice(&2u16.to_le_bytes()); // stereo
    out.extend_from_slice(&rate_hz.to_le_bytes());
    out.extend_from_slice(&(rate_hz * bytes_per_frame).to_le_bytes());
    out.extend_from_slice(&(bytes_per_frame as u16).to_le_bytes());
    out.extend_from_slice(&16u16.to_le_bytes());
    out.extend_from_slice(b"data");
    out.extend_from_slice(&size_data.to_le_bytes());
    for value in samples {
        let scaled = (value.clamp(-1.0, 1.0) * 32767.0).round() as i16;
        out.extend_from_slice(&scaled.to_le_bytes());
    }
    fs::write(path, out)
}
