//! CLI entry point. Thin wrapper over the library's `render_song`.

use std::path::PathBuf;
use std::str::FromStr;

use anyhow::Result;
use clap::Parser;

use iidx_on_knitting::{Difficulty, render_song};

/// Reconstruct a full IIDX song from its .s3p keysound archive and .1 chart, as Ogg/Opus.
#[derive(Parser, Debug)]
#[command(name = "iidxOnKnitting")]
struct CliArgs {
    /// Path to the song's .s3p keysound archive
    s3p: PathBuf,

    /// Path to the song's .1 chart file
    chart: PathBuf,

    /// Output .ogg path (Ogg/Opus)
    #[arg(short, long)]
    out: PathBuf,

    /// Difficulty to render: SPB/SPN/SPH/SPA/SPL/DPB/DPN/DPH/DPA/DPL
    #[arg(short, long, default_value = "SPN", value_parser = parse_difficulty)]
    difficulty: Difficulty,
}

// clap value parser: a free fn returning Result<T, E: Display> is the explicit, unambiguous
// way to parse a custom type, instead of relying on clap's FromStr auto-detection.
fn parse_difficulty(str_value: &str) -> Result<Difficulty, String> {
    Difficulty::from_str(str_value)
}

fn main() -> Result<()> {
    let args = CliArgs::parse();
    render_song(&args.s3p, &args.chart, args.difficulty, &args.out)
}
