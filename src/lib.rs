//! iidxOnKnitting — offline BMS-style renderer that reconstructs a full IIDX song from
//! its keysound archive (.s3p) and chart (.1), emitting Ogg/Opus.
//!
//! This file is the library surface only: it declares the modules and re-exports the
//! public API. All business logic lives in the submodules. The crate is built to be
//! embedded into iidxOnEar as a library; `render_song` is the single entry point.

mod chart;
mod codec;
mod mix;
mod render;
mod source;
mod tool; // tool::bytes, tool::ifs
mod unpack; // unpack_s3p, unpack_2dx

pub use chart::Difficulty;
pub use render::{RenderError, convert_song, render_song};
