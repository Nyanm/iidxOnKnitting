//! Small bounds-checked little-endian integer readers, shared by the binary-format parsers
//! (s3p, chart). Keeping them in one place avoids divergent copies across modules.

use anyhow::{Context, Result, ensure};

/// Read a little-endian u16 at `offset`, bounds-checked.
pub fn read_u16_le(bytes: &[u8], offset: usize) -> Result<u16> {
    let end = offset.checked_add(2).context("u16 offset overflow")?;
    ensure!(end <= bytes.len(), "u16 read at {offset} out of bounds (len {})", bytes.len());
    Ok(u16::from_le_bytes([bytes[offset], bytes[offset + 1]]))
}

/// Read a little-endian u32 at `offset`, bounds-checked.
pub fn read_u32_le(bytes: &[u8], offset: usize) -> Result<u32> {
    let end = offset.checked_add(4).context("u32 offset overflow")?;
    ensure!(end <= bytes.len(), "u32 read at {offset} out of bounds (len {})", bytes.len());
    Ok(u32::from_le_bytes([
        bytes[offset],
        bytes[offset + 1],
        bytes[offset + 2],
        bytes[offset + 3],
    ]))
}
