//! CLI: a two-level command tree — first the game, then the operation on it.
//!
//!   iidxOnKnitting iidx render  --ifs <.ifs> -o out.ogg
//!   iidxOnKnitting iidx render  --audio <.s3p|.2dx> --chart <.1> [-d spa] -o out.ogg
//!   iidxOnKnitting iidx convert <file> -o out.ogg
//!   iidxOnKnitting sdvx convert <file> -o out.ogg
//!   iidxOnKnitting gd   render  --seq <_seq.ifs> --bgm <_bgm.ifs> -o out.ogg
//!
//! This file is argument parsing and nothing else: each leaf maps onto exactly one of the library's
//! `run_*` entry points, which own everything else — including the two "retry the other way" rules
//! (a pre-mixed keysound archive, a 2DX9 container handed in as bare audio) that used to live here
//! and that every embedder would otherwise have to rewrite.
//!
//! Only exact file paths are accepted; folder walking and layout guessing are the caller's job.

use std::path::PathBuf;

use anyhow::{Result, bail};
use clap::{Args, Parser, Subcommand};

use iidx_on_knitting::{IidxDifficulty, IidxSource, run_gitadora, run_iidx, run_sdvx};

/// Reconstruct or transcode one arcade song into Ogg/Opus.
#[derive(Parser, Debug)]
#[command(name = "iidxOnKnitting", version, arg_required_else_help = true)]
struct CliArgs {
    #[command(subcommand)]
    game: Game,
}

/// First level: which game's data layout the inputs follow.
#[derive(Subcommand, Debug)]
enum Game {
    /// beatmania IIDX — chart (.1) plus a keysound archive (.s3p / .2dx).
    Iidx {
        #[command(subcommand)]
        operation: IidxOperation,
    },
    /// SOUND VOLTEX — pre-mixed audio only (.s3v, or a .2dx container).
    Sdvx {
        #[command(subcommand)]
        operation: SdvxOperation,
    },
    /// GITADORA — charts (.sq3) plus keysounds (.va3) over a pre-mixed backing track (.bin).
    Gd {
        #[command(subcommand)]
        operation: GdOperation,
    },
}

// Flattened into every operation so `-o` sits next to the inputs it applies to. A `global` arg would
// read more loosely but clap forbids marking those required, and the output path always is.
#[derive(Args, Debug)]
struct OutputArg {
    /// Output path (Ogg/Opus).
    #[arg(short, long)]
    output: PathBuf,
}

#[derive(Subcommand, Debug)]
enum IidxOperation {
    /// Rebuild a song by laying its keysounds onto the chart's timeline.
    Render {
        /// A song `.ifs` holding both the chart and the keysound archive.
        #[arg(long, conflicts_with_all = ["audio", "chart"], required_unless_present = "audio")]
        ifs: Option<PathBuf>,

        /// Loose keysound archive (`.s3p` / `.2dx`). Requires --chart.
        #[arg(long, requires = "chart")]
        audio: Option<PathBuf>,

        /// Loose chart (`.1`), paired with --audio.
        #[arg(long)]
        chart: Option<PathBuf>,

        /// Which difficulty's event stream to read. Any of them rebuilds the same audio, but not
        /// every song has every difficulty — spn always exists.
        #[arg(short, long, default_value = "spn")]
        difficulty: IidxDifficulty,

        #[command(flatten)]
        output: OutputArg,
    },

    /// Transcode a pre-mixed IIDX audio file as-is, e.g. a `_pre.2dx` preview.
    Convert {
        /// A decodable audio file, or a 2DX9 container whose first entry is the mix.
        input: PathBuf,

        #[command(flatten)]
        output: OutputArg,
    },
}

#[derive(Subcommand, Debug)]
enum SdvxOperation {
    /// Transcode an SDVX audio file as-is. SDVX ships no keysounds, so there is nothing to rebuild.
    Convert {
        /// A `.s3v`, or a `.2dx` container whose first entry is the mix.
        input: PathBuf,

        #[command(flatten)]
        output: OutputArg,
    },
}

#[derive(Subcommand, Debug)]
enum GdOperation {
    /// Rebuild a song by laying the drum / guitar / bass keysounds onto the backing track.
    Render {
        /// `m<id>_seq.ifs` — the charts (`.sq3`) and keysound archives (`.va3`).
        #[arg(long)]
        seq: PathBuf,

        /// `m<id>_bgm.ifs` — the pre-mixed backing-track variants (`.bin`).
        #[arg(long)]
        bgm: PathBuf,

        #[command(flatten)]
        output: OutputArg,
    },
}

fn main() -> Result<()> {
    match CliArgs::parse().game {
        Game::Iidx { operation: IidxOperation::Render { ifs: Some(ifs), difficulty, output, .. } } => {
            run_iidx(IidxSource::Packed { ifs: &ifs, difficulty }, &output.output)
        }
        Game::Iidx {
            operation: IidxOperation::Render { audio: Some(audio), chart: Some(chart), difficulty, output, .. },
        } => run_iidx(IidxSource::Loose { audio: &audio, chart: &chart, difficulty }, &output.output),
        // clap enforces --ifs or (--audio + --chart), so this only guards against a lax rule
        Game::Iidx { operation: IidxOperation::Render { .. } } => {
            bail!("iidx render needs --ifs, or --audio with --chart")
        }
        Game::Iidx { operation: IidxOperation::Convert { input, output } } => {
            run_iidx(IidxSource::PreMixed(&input), &output.output)
        }
        Game::Sdvx { operation: SdvxOperation::Convert { input, output } } => {
            run_sdvx(&input, &output.output)
        }
        Game::Gd { operation: GdOperation::Render { seq, bgm, output } } => {
            run_gitadora(&seq, &bgm, &output.output)
        }
    }
}
