//! beatmania IIDX song formats and assembly:
//!   - `chart`: the `.1` chart — difficulty slots and the per-lane keysound event stream.
//!   - `song`:  routes an `.ifs`'s members and assembles a song from its chart + keysound archive.
//!
//! The keysound containers themselves are not here but in [`crate::unpack`], because SDVX shares
//! the 2DX9 half of it. PCM-level work lives in [`crate::audio`].

pub mod chart;
pub mod song;
