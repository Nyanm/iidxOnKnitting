//! CLI entry point. Thin wrapper over the library's `render_song`, with the same pre-mixed
//! fallback to `convert_song` that an embedding caller (iidxOnEar) is expected to use.

use std::path::PathBuf;

use anyhow::Result;
use clap::Parser;

use iidx_on_knitting::{Difficulty, RenderError, convert_song, render_song};

/// Reconstruct a full IIDX song (BMS-style) into Ogg/Opus from a loose folder or an .ifs archive.
#[derive(Parser, Debug)]
#[command(name = "iidxOnKnitting")]
struct CliArgs {
    /// Input song: a v30+ loose folder, or a v1-29 .ifs archive
    #[arg(short, long)]
    input: PathBuf,

    /// Output .ogg path (Ogg/Opus)
    #[arg(short, long)]
    output: PathBuf,

    /// Difficulty to render (spb/spn/sph/spa/spl/dpb/dpn/dph/dpa/dpl)
    #[arg(short, long, default_value = "spn")]
    difficulty: Difficulty,
}

fn main() -> Result<()> {
    let args = CliArgs::parse();
    // try keysound reconstruction; for a pre-mixed song (no keysounds), transcode it directly
    match render_song(&args.input, &args.output, args.difficulty) {
        Ok(()) => Ok(()),
        Err(RenderError::NotKeysound) => {
            eprintln!("iidxOnKnitting: no keysounds (pre-mixed song); transcoding the audio directly");
            convert_song(&args.input, &args.output).map_err(Into::into)
        }
        Err(RenderError::Failed(error)) => Err(error),
    }
}
