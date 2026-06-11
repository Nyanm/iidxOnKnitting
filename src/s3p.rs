//! S3P0 keysound-archive unpacking.
//!
//! An .s3p is a flat archive of keysounds:
//!   magic "S3P0", u32 count, then count x (u32 offset, u32 size).
//! Each entry points at an S3V0 block: 4-byte "S3V0" magic, a 0x20-byte header, then the
//! payload = one ASF/WMAv2 keysound. 1-based chart sample number N maps to archive index N-1.

use crate::bytes::read_u32_le;

use anyhow::{Context, Result, ensure};

const S3P0_MAGIC: &[u8; 4] = b"S3P0"; // archive magic at file offset 0
const S3V0_MAGIC: &[u8; 4] = b"S3V0"; // per-keysound block magic
const S3V0_HEADER_LEN: usize = 0x20;  // S3V0 block header size; payload follows it

/// Unpack an S3P0 archive (raw bytes) into its keysound payloads (each = raw ASF/WMAv2 bytes).
/// The returned Vec is 0-indexed; chart sample number N (1-based) maps to index N-1.
pub fn unpack(bytes_archive: &[u8]) -> Result<Vec<Vec<u8>>> {
    ensure!(
        bytes_archive.len() >= 8 && &bytes_archive[0..4] == S3P0_MAGIC,
        "not an S3P0 archive"
    );

    let count_entries = read_u32_le(bytes_archive, 4)? as usize;
    let table_end = 8 + count_entries.checked_mul(8).context("entry table size overflow")?;
    ensure!(
        table_end <= bytes_archive.len(),
        "entry table ({count_entries} entries) exceeds file size {}",
        bytes_archive.len()
    );

    let mut vec_keysound = Vec::with_capacity(count_entries);
    for index_entry in 0..count_entries {
        let offset_table = 8 + index_entry * 8;
        let offset_block = read_u32_le(bytes_archive, offset_table)? as usize;
        let size_block = read_u32_le(bytes_archive, offset_table + 4)? as usize;
        let end_block = offset_block
            .checked_add(size_block)
            .with_context(|| format!("entry {index_entry} range overflow"))?;

        ensure!(
            end_block <= bytes_archive.len(),
            "entry {index_entry} block [{offset_block}..{end_block}] exceeds file size {}",
            bytes_archive.len()
        );
        ensure!(
            size_block >= S3V0_HEADER_LEN
                && &bytes_archive[offset_block..offset_block + 4] == S3V0_MAGIC,
            "entry {index_entry} is not an S3V0 block"
        );

        // payload = block minus its 0x20 header = the ASF/WMA keysound bytes
        let payload = bytes_archive[offset_block + S3V0_HEADER_LEN..end_block].to_vec();
        vec_keysound.push(payload);
    }

    Ok(vec_keysound)
}
