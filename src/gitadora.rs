//! GITADORA song formats, all reached through the `.ifs` pair `m<id>_bgm.ifs` + `m<id>_seq.ifs`:
//!   - `bmp`:  the `bgm<id><mask>k.bin` backing-track container (BMP + Konami ADPCM).
//!   - `va3`:  the `spu<id><d|g>.va3` keysound archive, keyed by chart sound id.
//!   - `sq3`:  the `d<id>.sq3` / `g<id>.sq3` charts (SEQP container of SQ3T chart chunks).
//!   - `song`: routes the `.ifs` members, picks the bed, and mixes the whole song.
//!
//! The formats are little-endian except the BMP header, which mixes endiannesses (see `bmp`).
//! PCM-level work lives in `crate::audio`, not here.

pub mod bmp;
pub mod song;
pub mod sq3;
pub mod va3;
