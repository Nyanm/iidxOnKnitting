//! 2DX9 keysound-archive (`.2dx`) unpacking — the v1-24 counterpart of `s3p`.
//!
//! A `.2dx` is a flat archive of keysounds. Header: `char[16] name`, `u32 header_size`,
//! `u32 file_count`, a fixed sub-header, then a `u32[file_count]` offset table at 0x48 (each
//! offset is relative to the archive start). Every offset points at a 2DX9 block: `"2DX9"`
//! magic, `u32 block_header_size`, `u32 data_size`, then — at `block + block_header_size` — a
//! RIFF/WAVE (MS-ADPCM) keysound of `data_size` bytes. 1-based chart sample N maps to index
//! N-1. All integers are little-endian (unlike the big-endian `.ifs`/KBin).

use crate::bytes::read_u32_le;

use anyhow::{Context, Result, ensure};

const DX2_MAGIC: &[u8; 4] = b"2DX9";        // per-keysound block magic
const FILE_COUNT_OFFSET: usize = 0x14;      // u32 number of keysound blocks
const OFFSET_TABLE_BASE: usize = 0x48;      // u32[file_count] block offsets begin here
const BLOCK_HEADER_SIZE_AT: usize = 0x04;   // within a block: u32 block header size
const BLOCK_DATA_SIZE_AT: usize = 0x08;     // within a block: u32 RIFF/WAVE payload size
const BLOCK_MIN_HEADER: usize = 0x0C;       // we read magic + two u32s from a block header

/// Unpack a 2DX9 archive (raw bytes) into its keysound payloads (each = raw RIFF/WAVE bytes).
/// The returned Vec is 0-indexed; chart sample number N (1-based) maps to index N-1.
pub fn unpack(bytes_archive: &[u8]) -> Result<Vec<Vec<u8>>> {
    ensure!(
        bytes_archive.len() >= OFFSET_TABLE_BASE,
        "2dx archive too small for a header"
    );
    let count_entries = read_u32_le(bytes_archive, FILE_COUNT_OFFSET)? as usize;
    let table_end = OFFSET_TABLE_BASE
        + count_entries.checked_mul(4).context("2dx offset table size overflow")?;
    ensure!(
        table_end <= bytes_archive.len(),
        "2dx offset table ({count_entries} entries) exceeds file size {}",
        bytes_archive.len()
    );

    let mut vec_keysound = Vec::with_capacity(count_entries);
    for index_entry in 0..count_entries {
        let offset_block = read_u32_le(bytes_archive, OFFSET_TABLE_BASE + index_entry * 4)? as usize;
        ensure!(
            offset_block.checked_add(BLOCK_MIN_HEADER).is_some_and(|end| end <= bytes_archive.len())
                && &bytes_archive[offset_block..offset_block + 4] == DX2_MAGIC,
            "entry {index_entry} is not a 2DX9 block"
        );

        let block_header_size = read_u32_le(bytes_archive, offset_block + BLOCK_HEADER_SIZE_AT)? as usize;
        let data_size = read_u32_le(bytes_archive, offset_block + BLOCK_DATA_SIZE_AT)? as usize;
        let payload_start = offset_block
            .checked_add(block_header_size)
            .with_context(|| format!("entry {index_entry} payload start overflow"))?;
        let payload_end = payload_start
            .checked_add(data_size)
            .with_context(|| format!("entry {index_entry} payload end overflow"))?;
        ensure!(
            payload_end <= bytes_archive.len(),
            "entry {index_entry} payload [{payload_start}..{payload_end}] exceeds file size {}",
            bytes_archive.len()
        );

        // payload = the RIFF/WAVE keysound (skip the 2DX9 block header)
        vec_keysound.push(bytes_archive[payload_start..payload_end].to_vec());
    }
    Ok(vec_keysound)
}
