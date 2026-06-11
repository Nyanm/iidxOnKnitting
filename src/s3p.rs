//! S3P0 keysound-archive unpacking.
//!
//! An .s3p is a flat archive of keysounds:
//!   magic "S3P0", u32 count, then count x (u32 offset, u32 size).
//! Each entry points at an S3V0 block: 4-byte "S3V0" magic, a 0x20-byte header, then the
//! payload = one ASF/WMAv2 keysound. 1-based chart sample number N maps to archive index N-1.

use crate::bytes::read_u32_le;

use std::fs;
use std::path::Path;

use anyhow::{Context, Result, ensure};

const S3P0_MAGIC: &[u8; 4] = b"S3P0"; // archive magic at file offset 0
const S3V0_MAGIC: &[u8; 4] = b"S3V0"; // per-keysound block magic
const S3V0_HEADER_LEN: usize = 0x20;  // S3V0 block header size; payload follows it

/// Unpack an S3P0 archive into its keysound payloads (each = raw ASF/WMAv2 bytes).
/// The returned Vec is 0-indexed; chart sample number N (1-based) maps to index N-1.
pub fn unpack(s3p_path: &Path) -> Result<Vec<Vec<u8>>> {
    let bytes_archive =
        fs::read(s3p_path).with_context(|| format!("reading s3p {}", s3p_path.display()))?;

    ensure!(
        bytes_archive.len() >= 8 && &bytes_archive[0..4] == S3P0_MAGIC,
        "not an S3P0 archive: {}",
        s3p_path.display()
    );

    let count_entries = read_u32_le(&bytes_archive, 4)? as usize;
    let table_end = 8 + count_entries.checked_mul(8).context("entry table size overflow")?;
    ensure!(
        table_end <= bytes_archive.len(),
        "entry table ({count_entries} entries) exceeds file size {}",
        bytes_archive.len()
    );

    let mut vec_keysound = Vec::with_capacity(count_entries);
    for index_entry in 0..count_entries {
        let offset_table = 8 + index_entry * 8;
        let offset_block = read_u32_le(&bytes_archive, offset_table)? as usize;
        let size_block = read_u32_le(&bytes_archive, offset_table + 4)? as usize;
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

#[cfg(test)]
mod tests {
    use super::*;

    // Local sample (gitignored but present on the dev machine); skip gracefully if absent.
    fn sample_s3p() -> std::path::PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join(".sample/iidxOnEar/.sample/unpack/30000/30000.s3p")
    }

    #[test]
    fn unpack_30000() {
        let path_sample = sample_s3p();
        if !path_sample.exists() {
            eprintln!("skipping unpack_30000: sample not present at {}", path_sample.display());
            return;
        }

        let vec_keysound = unpack(&path_sample).expect("unpack 30000.s3p");
        assert_eq!(vec_keysound.len(), 1186, "30000.s3p should hold 1186 keysounds");

        // sample 1 (index 0) is the largest entry = background base (~4.88MB payload)
        let index_largest = (0..vec_keysound.len())
            .max_by_key(|&index_keysound| vec_keysound[index_keysound].len())
            .expect("non-empty archive");
        assert_eq!(index_largest, 0, "sample 1 should be the largest entry");
        assert!(vec_keysound[0].len() > 4_000_000, "base keysound payload ~4.88MB");

        // every payload should begin with the ASF header-object GUID
        const ASF_GUID: [u8; 16] = [
            0x30, 0x26, 0xb2, 0x75, 0x8e, 0x66, 0xcf, 0x11, 0xa6, 0xd9, 0x00, 0xaa, 0x00, 0x62,
            0xce, 0x6c,
        ];
        for (index_keysound, payload) in vec_keysound.iter().enumerate() {
            assert!(payload.len() >= 16, "keysound {index_keysound} too short");
            assert_eq!(&payload[0..16], &ASF_GUID, "keysound {index_keysound} not ASF");
        }
    }
}
