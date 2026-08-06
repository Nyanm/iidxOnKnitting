//! Generic audio processing, free of any game-specific container or chart logic:
//!   - `adpcm`:  Konami's 4-bit ADPCM decoder (used by GITADORA's BMP and VA3 payloads).
//!   - `mix`:    the stereo mixing timeline every renderer sums into, plus the pan law.
//!   - `master`: final gain staging — peak normalisation and soft-knee limiting.
//!
//! Deliberately not a GITADORA submodule: nothing here knows about GITADORA, and `codec` is
//! meant to move in here later so that all PCM-level work lives under one roof.

pub mod adpcm;
pub mod master;
pub mod mix;
