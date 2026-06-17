//! Keysound-archive unpacking. Two flat container formats, each flattened to an ordered
//! `Vec<Vec<u8>>` where index N-1 is the chart's 1-based sample N:
//!   - S3P0 (`.s3p`, v25+): WMAv2 keysounds — `unpack_s3p`.
//!   - 2DX9 (`.2dx`, v1-24): WAV/MS-ADPCM keysounds — `unpack_2dx`.
//! All integers are little-endian. (The container that *holds* these — `.ifs` / KBin — is parsed
//! in `tool::ifs` and is big-endian.)

use crate::tool::bytes::read_u32_le;

use anyhow::{Context, Result, ensure};

// ── S3P0 (`.s3p`) ────────────────────────────────────────────────────────────────
// magic "S3P0", u32 count, then count x (u32 offset, u32 size). Each entry points at an S3V0
// block: 4-byte "S3V0" magic, a 0x20-byte header, then the payload = one ASF/WMAv2 keysound.
const S3P0_MAGIC: &[u8; 4] = b"S3P0"; // archive magic at file offset 0
const S3V0_MAGIC: &[u8; 4] = b"S3V0"; // per-keysound block magic
const S3V0_HEADER_LEN: usize = 0x20;  // S3V0 block header size; payload follows it

/// Unpack an S3P0 archive (raw bytes) into its keysound payloads (each = raw ASF/WMAv2 bytes).
/// The returned Vec is 0-indexed; chart sample number N (1-based) maps to index N-1.
pub fn unpack_s3p(bytes_archive: &[u8]) -> Result<Vec<Vec<u8>>> {
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

// ── 2DX9 (`.2dx`) ────────────────────────────────────────────────────────────────
// char[16] name, u32 header_size, u32 file_count, a fixed sub-header, then a u32[file_count]
// offset table at 0x48 (each offset relative to the archive start). Every offset points at a
// 2DX9 block ("2DX9" magic, u32 block_header_size, u32 data_size); the RIFF/WAVE (MS-ADPCM)
// keysound payload sits at block + block_header_size.
const DX2_MAGIC: &[u8; 4] = b"2DX9";        // per-keysound block magic
const FILE_COUNT_OFFSET: usize = 0x14;      // u32 number of keysound blocks
const OFFSET_TABLE_BASE: usize = 0x48;      // u32[file_count] block offsets begin here
const BLOCK_HEADER_SIZE_AT: usize = 0x04;   // within a block: u32 block header size
const BLOCK_DATA_SIZE_AT: usize = 0x08;     // within a block: u32 RIFF/WAVE payload size
const BLOCK_MIN_HEADER: usize = 0x0C;       // we read magic + two u32s from a block header

/// Unpack a 2DX9 archive (raw bytes) into its keysound payloads (each = raw RIFF/WAVE bytes).
/// The returned Vec is 0-indexed; chart sample number N (1-based) maps to index N-1.
pub fn unpack_2dx(bytes_archive: &[u8]) -> Result<Vec<Vec<u8>>> {
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

/// Cheaply test whether `bytes_archive` is a 2DX9 container — a non-zero entry count plus a `2DX9`
/// magic on the first block — without extracting any payloads. Distinguishes a packed `.2dx` from a
/// bare RIFF/WAVE or ASF audio file (which start with other magic, and whose 0x14 often reads 0).
pub fn is_2dx9(bytes_archive: &[u8]) -> bool {
    let count_entries = match read_u32_le(bytes_archive, FILE_COUNT_OFFSET) {
        Ok(count) => count,
        Err(_) => return false,
    };
    if count_entries == 0 {
        return false;
    }
    let offset_block = match read_u32_le(bytes_archive, OFFSET_TABLE_BASE) {
        Ok(offset) => offset as usize,
        Err(_) => return false,
    };
    bytes_archive.get(offset_block..offset_block + 4) == Some(&DX2_MAGIC[..])
}
