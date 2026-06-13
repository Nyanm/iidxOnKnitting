//! Shared low-level tools, free of any song/render logic:
//!   - `bytes`: bounds-checked little-/big-endian integer readers.
//!   - `ifs`:   the minimal read-only IFS/KBin manifest reader.

pub mod bytes;
pub mod ifs;
