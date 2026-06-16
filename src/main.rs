//! CLI: a thin, explicit wrapper over the library's four entry points. It does NOT walk folders or
//! guess layouts — you name the exact files. Pick one mode:
//!
//!   - `--audio <.s3p|.2dx> --chart <.1>`  reconstruct a loose song (render_song)
//!   - `--ifs <.ifs>`  reconstruct a packed song (render_packed_song)
//!   - `--convert <file>`  transcode a single audio file (convert_song)
//!   - `--convert-packed <.2dx>`  transcode a 2DX9 container's first entry (convert_packed_song)
//!
//! Folder routing / multi-source selection is the embedding caller's job (e.g. iidxOnEar).

use std::path::PathBuf;

use anyhow::{Result, bail};
use clap::Parser;

use iidx_on_knitting::{
    Difficulty, RenderError, convert_packed_song, convert_song, render_packed_song, render_song,
};

/// Reconstruct or transcode one IIDX/SDVX song into Ogg/Opus. Specify exactly one mode (see below).
#[derive(Parser, Debug)]
#[command(name = "iidxOnKnitting")]
struct CliArgs {
    /// render: keysound archive (.s3p/.2dx). Requires --chart.
    #[arg(long, requires = "chart", conflicts_with_all = ["ifs", "convert", "convert_packed"])]
    audio: Option<PathBuf>,

    /// render: chart (.1), paired with --audio.
    #[arg(long)]
    chart: Option<PathBuf>,

    /// render-packed: an .ifs archive (chart + keysounds inside).
    #[arg(long, conflicts_with_all = ["convert", "convert_packed"])]
    ifs: Option<PathBuf>,

    /// convert: a single decodable audio file (pre-mixed .2dx, SDVX .s3v, …).
    #[arg(long, conflicts_with = "convert_packed")]
    convert: Option<PathBuf>,

    /// convert-packed: a 2DX9 container; transcodes its first entry.
    #[arg(long)]
    convert_packed: Option<PathBuf>,

    /// Output .ogg path (Ogg/Opus).
    #[arg(short, long)]
    output: PathBuf,

    /// Difficulty for the render modes (spb/spn/sph/spa/spl/dpb/dpn/dph/dpa/dpl).
    #[arg(short, long, default_value = "spn")]
    difficulty: Difficulty,
}

fn main() -> Result<()> {
    let args = CliArgs::parse();
    let output = &args.output;

    if let Some(audio) = &args.audio {
        // clap guarantees --chart is present (requires = "chart")
        let chart = args.chart.as_ref().expect("--audio requires --chart");
        // a `.2dx` may turn out to be pre-mixed (NotKeysound) — fall back to a direct transcode
        match render_song(audio, chart, output, args.difficulty) {
            Err(RenderError::NotKeysound) => {
                eprintln!(
                    "iidxOnKnitting: {} is pre-mixed (no keysounds); transcoding directly",
                    audio.display()
                );
                report(convert_song(audio, output))
            }
            other => report(other),
        }
    } else if let Some(ifs) = &args.ifs {
        report(render_packed_song(ifs, output, args.difficulty))
    } else if let Some(audio) = &args.convert {
        report(convert_song(audio, output))
    } else if let Some(audio) = &args.convert_packed {
        report(convert_packed_song(audio, output))
    } else {
        bail!(
            "specify one mode: (--audio + --chart) | --ifs | --convert <file> | --convert-packed <file>"
        )
    }
}

// Surface a RenderError as the process result: the inner anyhow error (with its context chain) for
// a genuine failure, or a clear hint for the pre-mixed case.
fn report(result: Result<(), RenderError>) -> Result<()> {
    match result {
        Ok(()) => Ok(()),
        Err(RenderError::Failed(error)) => Err(error),
        Err(RenderError::NotKeysound) => bail!("input is pre-mixed (no keysounds); use --convert"),
        Err(RenderError::NotSingleAudio) => bail!("input is packed 2dx file (not single audio); use --convert-packed"),
    }
}
