//! Input resolution: turn a single user-supplied path into the renderer's raw inputs, regardless
//! of how the song is packaged on disk.
//!
//! IIDX packages a song a few ways (see ROADMAP / STRUCTURE.md):
//!   - loose folder `<id>/` (v30+, or omnimix-revived old songs): `<id>.1` + either `<id>.s3p`
//!     (WMA keysounds) or a loose `<id>.2dx` (WAV keysounds), plus an `<id>_pre.2dx` preview.
//!   - `.ifs` archive (v1-29): the `.1` chart + an S3P0 (WMA) or 2DX9 (WAV) keysound archive,
//!     read via the minimal KBin manifest reader (`ifs`).
//!
//! `locate` does the routing once (directory -> loose layout; regular file -> `.ifs`; container
//! type decided by what's present, not by version) and returns the raw, not-yet-unpacked parts.
//! Two consumers build on it: `resolve` unpacks the keysound archive for the renderer, while
//! `resolve_audio` returns the archive file as-is for `convert_song`. A few early songs ship
//! several keysound `.2dx` (multi-source); we pick one by the `<id>a` > `<id>1` > `<id>`
//! preference (an approximation — see README's "多音源" note).

use crate::render::RenderError;
use crate::tool::ifs;
use crate::unpack;

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail, ensure};

/// Everything the renderer needs, decoupled from on-disk packaging.
pub struct SongSource {
    pub vec_keysound: Vec<Vec<u8>>, // ordered keysound blobs (WMA or WAV); index N-1 = sample N
    pub bytes_chart: Vec<u8>,       // raw .1 chart bytes
}

// Which keysound-archive format the located bytes are in.
enum ArchiveKind {
    S3p,
    Dx2,
}

// A located-but-not-unpacked song: raw chart bytes + the raw keysound-archive file + its format.
// `locate` produces this; `resolve` / `resolve_audio` consume it.
struct Located {
    bytes_chart: Vec<u8>,
    bytes_archive: Vec<u8>,
    kind: ArchiveKind,
}

/// Resolve a single input path into a `SongSource` (ordered keysound blobs + chart bytes).
///
/// Returns [`RenderError::NotKeysound`] when the input's audio is a single pre-mixed file (a RIFF
/// `.2dx`) with no keysounds to reconstruct — the caller should use `convert_song` instead.
pub fn resolve(input: &Path) -> Result<SongSource, RenderError> {
    let located = locate(input)?;
    let vec_keysound = match located.kind {
        ArchiveKind::S3p => {
            unpack::unpack_s3p(&located.bytes_archive).context("unpacking s3p keysound archive")?
        }
        ArchiveKind::Dx2 => {
            // a `.2dx` that is actually a single RIFF/WAVE is a pre-mixed song, not a keysound
            // container — there is nothing to reconstruct. Signal the caller to use convert_song.
            if located.bytes_archive.starts_with(b"RIFF") {
                return Err(RenderError::NotKeysound);
            }
            unpack::unpack_2dx(&located.bytes_archive).context("unpacking 2dx keysound archive")?
        }
    };
    Ok(SongSource { vec_keysound, bytes_chart: located.bytes_chart })
}

/// Locate the song's keysound-archive file and return its raw bytes, for `convert_song`. Takes the
/// same input as [`resolve`] (a loose folder or `.ifs`) and runs the same routing, but returns the
/// archive file as-is instead of unpacking it — for a pre-mixed song that file is the audio itself.
pub fn resolve_audio(input: &Path) -> Result<Vec<u8>, RenderError> {
    Ok(locate(input)?.bytes_archive)
}

// Route the input to its raw parts: a directory -> loose layout, a regular file -> `.ifs`.
fn locate(input: &Path) -> Result<Located> {
    if input.is_dir() { locate_loose_folder(input) }  // v30+ loose layout
    else { locate_ifs(input) }  // v1-29 packed in .ifs
}

// A loose song folder (folder name = song id). v30+ ships `<id>.s3p`; omnimix-revived old songs
// may instead ship a loose `<id>.2dx`. Either way the chart is `<id>.1`.
fn locate_loose_folder(dir: &Path) -> Result<Located> {
    let id = dir
        .file_name()
        .and_then(|name| name.to_str())
        .with_context(|| format!("cannot derive song id from folder {}", dir.display()))?;

    // some omnimix loose folders hold a packed `<id>.ifs` (e.g. 29083) instead of loose members
    let ifs_path = dir.join(format!("{id}.ifs"));
    if ifs_path.exists() {
        return locate_ifs(&ifs_path);
    }

    // the chart is required in either layout
    let chart_path = dir.join(format!("{id}.1"));
    ensure!(
        chart_path.exists(),
        "expected chart {} in loose folder {}",
        chart_path.display(),
        dir.display()
    );
    let bytes_chart =
        fs::read(&chart_path).with_context(|| format!("reading {}", chart_path.display()))?;

    // v30+ keysound archive: `<id>.s3p` (WMA)
    let s3p_path = dir.join(format!("{id}.s3p"));
    if s3p_path.exists() {
        let bytes_archive =
            fs::read(&s3p_path).with_context(|| format!("reading {}", s3p_path.display()))?;
        return Ok(Located { bytes_chart, bytes_archive, kind: ArchiveKind::S3p });
    }

    // omnimix-revived old song stored loose: `.2dx` keysound archive(s) (no `<id>.s3p`)
    let keysound_2dx = loose_2dx_archives(dir)?;
    let chosen = match keysound_2dx.as_slice() {
        [] => bail!("loose folder {} has no {id}.s3p and no keysound .2dx", dir.display()),
        [only] => only,
        _ => {
            let names: Vec<&str> = keysound_2dx
                .iter()
                .map(|path| path.file_name().and_then(|name| name.to_str()).unwrap_or(""))
                .collect();
            let index_chosen = pick_multisource(id, &names).with_context(|| {
                format!(
                    "multi-source loose folder {} matched none of {id}a/{id}1/{id}.2dx; found {names:?}",
                    dir.display()
                )
            })?;
            &keysound_2dx[index_chosen]
        }
    };
    let bytes_archive = fs::read(chosen).with_context(|| format!("reading {}", chosen.display()))?;
    Ok(Located { bytes_chart, bytes_archive, kind: ArchiveKind::Dx2 })
}

// Collect the non-preview `.2dx` keysound archives in a loose folder (excludes `<id>_pre.2dx`),
// sorted for a deterministic result.
fn loose_2dx_archives(dir: &Path) -> Result<Vec<PathBuf>> {
    let mut archives = Vec::new();
    for entry in fs::read_dir(dir).with_context(|| format!("listing {}", dir.display()))? {
        let path = entry.with_context(|| format!("reading an entry in {}", dir.display()))?.path();
        let is_keysound = path
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| name.ends_with(".2dx") && !name.ends_with("_pre.2dx"));
        if is_keysound {
            archives.push(path);
        }
    }
    archives.sort();
    Ok(archives)
}

// Multi-source songs ship several keysound `.2dx`; pick one by the preference `<id>a` > `<id>1`
// > `<id>` (the `<id>a` variant is usually the modern, re-added version — see README). `names`
// are the candidate archive file names; returns the index of the first preference that matches.
fn pick_multisource(id: &str, names: &[&str]) -> Option<usize> {
    ["a", "1", ""].iter().find_map(|suffix| {
        let wanted = format!("{id}{suffix}.2dx");
        names.iter().position(|name| *name == wanted)
    })
}

// v1-29 .ifs: read the KBin manifest, take the `.1` chart and the `.s3p`/`.2dx` keysound archive.
fn locate_ifs(file: &Path) -> Result<Located> {
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
        let bytes_archive =
            bytes_ifs[member_s3p.offset..member_s3p.offset + member_s3p.size].to_vec();
        return Ok(Located { bytes_chart, bytes_archive, kind: ArchiveKind::S3p });
    }

    let members_2dx: Vec<&ifs::Member> = members
        .iter()
        .filter(|member| member.name.ends_with(".2dx") && !member.name.ends_with("_pre.2dx"))
        .collect();
    // single keysound `.2dx`, or multi-source -> pick by `<id>a` > `<id>1` > `<id>` (see README)
    let member_2dx = match members_2dx.as_slice() {
        [] => bail!("no .s3p or .2dx keysound archive inside {}", file.display()),
        [only] => *only,
        _ => {
            let id = member_chart.name.strip_suffix(".1").unwrap_or(member_chart.name.as_str());
            let names: Vec<&str> = members_2dx.iter().map(|member| member.name.as_str()).collect();
            let index_chosen = pick_multisource(id, &names).with_context(|| {
                format!(
                    "multi-source {} matched none of {id}a/{id}1/{id}.2dx; found {names:?}",
                    file.display()
                )
            })?;
            members_2dx[index_chosen]
        }
    };
    let bytes_archive = bytes_ifs[member_2dx.offset..member_2dx.offset + member_2dx.size].to_vec();
    Ok(Located { bytes_chart, bytes_archive, kind: ArchiveKind::Dx2 })
}
