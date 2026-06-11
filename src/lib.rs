//! iidxOnKnitting — offline BMS-style renderer that reconstructs a full IIDX song from
//! its keysound archive (.s3p) and chart (.1), emitting Ogg/Opus.
//!
//! This file is the library surface only: it declares the modules and re-exports the
//! public API. All business logic lives in the submodules. The crate is built to be
//! embedded into iidxOnEar as a library; `render_song` is the single entry point.

mod bytes;
mod chart;
mod codec;
mod ifs;
mod mix;
mod render;
mod s3p;
mod source;

pub use chart::Difficulty;
pub use render::render_song;
