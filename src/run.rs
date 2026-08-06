//! One call per game — the layer an embedder should reach for.
//!
/*
[`crate::render`] exposes the mechanism: each entry point does exactly one thing and reports the
conditions it cannot decide for you as typed [`RenderError`] variants. That is the right shape when
the caller wants control, but it means every caller then rewrites the same two fallbacks:

  - a keysound archive that turns out to be one pre-mixed file (`NotKeysound`) should be transcoded;
  - a file handed to `convert_song` that turns out to be a 2DX9 container (`NotSingleAudio`) should
    go through `convert_packed_song` instead.

Both are decided from container magic before anything is decoded or written, so retrying costs
nothing — and both have exactly one sensible answer, which is policy this crate should own rather
than leave to each consumer. `run_iidx` / `run_gitadora` / `run_sdvx` are that policy: hand them the
files, get an `.ogg`. They also flatten [`RenderError`] into `anyhow::Error`, since with the
branchable conditions handled internally there is nothing left for a caller to match on.

The typed layer stays public for callers that do want to branch themselves.
*/

use crate::iidx::chart::Difficulty;
use crate::render::{
    RenderError, convert_packed_song, convert_song, render_gitadora_song, render_iidx_packed_song,
    render_iidx_song,
};

use std::path::Path;

use anyhow::Result;

/// Which shape of IIDX input you have. Difficulty lives on the variants that reconstruct a chart,
/// so there is no parameter to ignore when transcoding a pre-mixed file.
#[derive(Debug, Clone, Copy)]
pub enum IidxSource<'a> {
    /// A song `.ifs` holding both the chart and the keysound archive.
    Packed { ifs: &'a Path, difficulty: Difficulty },
    /// A loose keysound archive (`.s3p` / `.2dx`) plus its chart (`.1`).
    Loose { audio: &'a Path, chart: &'a Path, difficulty: Difficulty },
    /// An already-mixed audio file — a `_pre.2dx` preview, or an early `.2dx` holding one finished
    /// mix rather than keysounds. Nothing to reconstruct, so it is only transcoded.
    PreMixed(&'a Path),
}

/// Render or transcode one IIDX song to Ogg/Opus at `output_path`.
///
/// A [`IidxSource::Loose`] archive that turns out to be a single pre-mixed file is transcoded
/// instead of reconstructed. That fallback is deliberately not applied to [`IidxSource::Packed`]:
/// there the archive lives inside the `.ifs`, so there is no path to hand to a transcoder, and a
/// packed song whose keysound member is a bare RIFF is a malformed archive rather than a normal
/// case — it fails loudly.
pub fn run_iidx(source: IidxSource<'_>, output_path: &Path) -> Result<()> {
    match source {
        IidxSource::Packed { ifs, difficulty } => {
            flatten(render_iidx_packed_song(ifs, output_path, difficulty))
        }
        IidxSource::Loose { audio, chart, difficulty } => {
            match render_iidx_song(audio, chart, output_path, difficulty) {
                Err(RenderError::NotKeysound) => {
                    eprintln!(
                        "iidxOnKnitting: {} is pre-mixed (no keysounds); transcoding directly",
                        audio.display()
                    );
                    run_transcode(audio, output_path)
                }
                other => flatten(other),
            }
        }
        IidxSource::PreMixed(audio) => run_transcode(audio, output_path),
    }
}

/// Render one GITADORA song to Ogg/Opus at `output_path`, from its `m<id>_seq.ifs` (charts +
/// keysounds) and `m<id>_bgm.ifs` (backing-track variants) pair.
pub fn run_gitadora(seq_ifs_path: &Path, bgm_ifs_path: &Path, output_path: &Path) -> Result<()> {
    flatten(render_gitadora_song(seq_ifs_path, bgm_ifs_path, output_path))
}

/// Transcode one SDVX song to Ogg/Opus at `output_path`. SDVX ships no keysounds, so there is
/// nothing to reconstruct; a bare `.s3v` and a 2DX9-packed `.2dx` are told apart from the container
/// magic, so either can be handed in.
pub fn run_sdvx(source_path: &Path, output_path: &Path) -> Result<()> {
    run_transcode(source_path, output_path)
}

// Transcode one already-mixed file, resolving bare audio vs 2DX9 container from the bytes.
fn run_transcode(source_path: &Path, output_path: &Path) -> Result<()> {
    match convert_song(source_path, output_path) {
        Err(RenderError::NotSingleAudio) => flatten(convert_packed_song(source_path, output_path)),
        other => flatten(other),
    }
}

// Collapse the typed boundary error into anyhow. `Failed` already carries an anyhow error with its
// context chain, so it is unwrapped rather than nested; the branchable variants can only reach here
// when this layer chose not to act on them, and their Display message is the whole story.
fn flatten(result: Result<(), RenderError>) -> Result<()> {
    result.map_err(|error| match error {
        RenderError::Failed(inner) => inner,
        other => anyhow::Error::new(other),
    })
}
