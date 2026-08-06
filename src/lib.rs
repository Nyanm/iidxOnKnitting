//! iidxOnKnitting — an offline BMS-style renderer for Konami arcade song data, emitting Ogg/Opus.
//!
/*
Three game branches, in descending order of how much work rebuilding a song takes:
  - IIDX:     a chart (`.1`) names a keysound per note; the whole song is the sum of those keysounds.
              Assembly in `iidx::song`, entry points `render_iidx_song` / `render_iidx_packed_song`.
  - GITADORA: charts (`.sq3`) plus keysound archives (`.va3`) are laid over a pre-mixed backing
              track (`.bin`) that already contains whatever parts nobody is playing.
              Assembly in `gitadora::song`, entry point `render_gitadora_song`.
  - SDVX:     no keysounds at all — the song ships as one finished file, so there is nothing to
              assemble and only the container differs. Handled by the shared `convert_*` pair.

Each branch owns its own `song::mix_song`, which turns bytes into a mixed timeline; `render` is a
thin layer that reads the files, calls one of them, masters and encodes.

Two levels of entry point, and an embedder almost always wants the first:
  - `run_iidx` / `run_gitadora` / `run_sdvx` — one call per game. Hand them the files, get an `.ogg`.
    They own the retry policy for the two conditions that have only one sensible answer, and return
    a plain `anyhow::Result`.
  - `render_*` / `convert_*` — the mechanism, each doing exactly one thing and reporting those
    conditions as typed [`RenderError`] variants for callers that want to branch themselves.

Naming convention: anything tied to one game carries its name (`render_iidx_song`, `IidxDifficulty`),
while things named after a file format keep the format's name, because the format already identifies
the game (`unpack_s3p`, `gitadora::va3`). Genuinely shared work is unqualified: `convert_song` serves
IIDX previews and SDVX alike, and `audio` knows about no game at all.

This file is the library surface only — it declares the modules and re-exports the public API; all
business logic lives in the submodules. The crate is built to be embedded into iidxOnEar.
*/

// One module per game branch plus the shared audio layer, all public: an embedder may reasonably
// want the pieces on their own, and the regression tests drive them directly.
pub mod audio; // audio::{adpcm, mix, master}
pub mod gitadora; // gitadora::{bmp, sq3, va3, song}
pub mod iidx; // iidx::{chart, song}

mod codec;
mod render;
mod run;
mod tool; // tool::bytes, tool::ifs
mod unpack; // unpack_s3p (IIDX), unpack_2dx / is_2dx9 (IIDX + SDVX)

// Inside `iidx::` the module path already says which game, so the type is plain `Difficulty` there;
// re-exported at the crate root it needs the prefix back, since that context is gone.
pub use iidx::chart::Difficulty as IidxDifficulty;
pub use render::{
    RenderError, convert_packed_song, convert_song, render_gitadora_song, render_iidx_packed_song,
    render_iidx_song,
};
pub use run::{IidxSource, run_gitadora, run_iidx, run_sdvx};
