//! Input resolution: turn a single user-supplied path into the renderer's raw inputs — the
//! ordered keysound blobs and the chart bytes — regardless of how the song is packaged on disk.
//!
//! IIDX ships a song three ways (see ROADMAP / STRUCTURE.md):
//!   - v30+   : a loose folder `<id>/` holding `<id>.s3p` + `<id>.1` (+ `<id>_pre.2dx`).
//!   - v25-29 : an `.ifs` archive wrapping an S3P0 keysound archive + the `.1` chart.
//!   - v1-24  : an `.ifs` archive wrapping a 2DX9 keysound archive + the `.1` chart.
//! We route on the input path alone: a directory is the v30+ loose layout; a regular file is
//! an `.ifs`. The `.ifs` paths (v1-29) are stubbed here and land in Step 7 (carve) / Step 8 (2dx).

use crate::s3p;

use std::fs;
use std::path::Path;

use anyhow::{Context, Result, bail, ensure};

const IFS_MAGIC: [u8; 4] = [0x6C, 0xAD, 0x8F, 0x89]; // .ifs container magic (first 4 bytes)

/// Everything the renderer needs, decoupled from on-disk packaging.
pub struct SongSource {
    pub vec_keysound: Vec<Vec<u8>>, // ordered keysound blobs (WMA or WAV); index N-1 = sample N
    pub bytes_chart: Vec<u8>,       // raw .1 chart bytes
}

/// Resolve a single input path into a `SongSource`, detecting how the song is packaged.
pub fn resolve(input: &Path) -> Result<SongSource> {
    if input.is_dir() {
        resolve_loose_folder(input) // v30+ loose layout
    } else {
        resolve_ifs(input) // v1-29 packed in .ifs
    }
}

// v30+ loose folder: the folder name is the song id; it holds `<id>.s3p` and `<id>.1`.
fn resolve_loose_folder(dir: &Path) -> Result<SongSource> {
    let id = dir
        .file_name()
        .and_then(|name| name.to_str())
        .with_context(|| format!("cannot derive song id from folder {}", dir.display()))?;
    let s3p_path = dir.join(format!("{id}.s3p"));
    let chart_path = dir.join(format!("{id}.1"));
    ensure!(
        s3p_path.exists(),
        "expected keysound archive {} in loose folder {}",
        s3p_path.display(),
        dir.display()
    );
    ensure!(
        chart_path.exists(),
        "expected chart {} in loose folder {}",
        chart_path.display(),
        dir.display()
    );

    let bytes_archive =
        fs::read(&s3p_path).with_context(|| format!("reading {}", s3p_path.display()))?;
    let vec_keysound =
        s3p::unpack(&bytes_archive).with_context(|| format!("unpacking {}", s3p_path.display()))?;
    let bytes_chart =
        fs::read(&chart_path).with_context(|| format!("reading {}", chart_path.display()))?;

    Ok(SongSource { vec_keysound, bytes_chart })
}

// v1-29 .ifs: confirm the container magic, then defer to Step 7 for the actual carve.
fn resolve_ifs(file: &Path) -> Result<SongSource> {
    let bytes_ifs = fs::read(file).with_context(|| format!("reading {}", file.display()))?;
    ensure!(
        bytes_ifs.len() >= 4 && bytes_ifs[0..4] == IFS_MAGIC,
        "input {} is neither a loose song folder nor an .ifs archive",
        file.display()
    );
    bail!(".ifs (v1-29) support is not implemented yet — coming in Step 7: {}", file.display());
}
