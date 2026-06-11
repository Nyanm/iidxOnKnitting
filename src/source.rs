//! Input resolution: turn a single user-supplied path into the renderer's raw inputs — the
//! ordered keysound blobs and the chart bytes — regardless of how the song is packaged on disk.
//!
//! IIDX ships a song three ways (see ROADMAP / STRUCTURE.md):
//!   - v30+   : a loose folder `<id>/` holding `<id>.s3p` + `<id>.1` (+ `<id>_pre.2dx`).
//!   - v25-29 : an `.ifs` archive wrapping an S3P0 keysound archive + the `.1` chart.
//!   - v1-24  : an `.ifs` archive wrapping a 2DX9 keysound archive + the `.1` chart.
//! We route on the input path alone: a directory is the v30+ loose layout; a regular file is
//! an `.ifs`, whose members we read via the minimal KBin manifest reader (`ifs`). The s3p case
//! (v25-29 and any s3p-packed song) is handled; 2dx-packed songs (v1-24 + 2dx songs) are Step 8.

use crate::dx2;
use crate::ifs;
use crate::s3p;

use std::fs;
use std::path::Path;

use anyhow::{Context, Result, bail, ensure};

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

// v1-29 .ifs: read the KBin manifest, take the `.1` chart and the `.s3p` keysound archive.
fn resolve_ifs(file: &Path) -> Result<SongSource> {
    let bytes_ifs = fs::read(file).with_context(|| format!("reading {}", file.display()))?;
    let members = ifs::list_members(&bytes_ifs)
        .with_context(|| format!("{} is neither a loose folder nor a readable .ifs", file.display()))?;

    let member_chart = members
        .iter()
        .find(|member| member.name.ends_with(".1"))
        .with_context(|| format!("no .1 chart inside {}", file.display()))?;
    let bytes_chart =
        bytes_ifs[member_chart.offset..member_chart.offset + member_chart.size].to_vec();

    // keysound archive: prefer `.s3p` (v25-29 + s3p songs); else the single `.2dx` (v1-24 + 2dx songs)
    if let Some(member_s3p) = members.iter().find(|member| member.name.ends_with(".s3p")) {
        let bytes_archive = &bytes_ifs[member_s3p.offset..member_s3p.offset + member_s3p.size];
        let vec_keysound = s3p::unpack(bytes_archive)
            .with_context(|| format!("unpacking {} from {}", member_s3p.name, file.display()))?;
        return Ok(SongSource { vec_keysound, bytes_chart });
    }

    let members_2dx: Vec<&ifs::Member> = members
        .iter()
        .filter(|member| member.name.ends_with(".2dx") && !member.name.ends_with("_pre.2dx"))
        .collect();
    match members_2dx.as_slice() {
        [member_2dx] => {
            let bytes_archive = &bytes_ifs[member_2dx.offset..member_2dx.offset + member_2dx.size];
            let vec_keysound = dx2::unpack(bytes_archive)
                .with_context(|| format!("unpacking {} from {}", member_2dx.name, file.display()))?;
            Ok(SongSource { vec_keysound, bytes_chart })
        }
        [] => bail!("no .s3p or .2dx keysound archive inside {}", file.display()),
        many => bail!(
            "{} has {} keysound .2dx archives (multi-source) — not yet supported: {:?}",
            file.display(),
            many.len(),
            many.iter().map(|member| &member.name).collect::<Vec<_>>()
        ),
    }
}
