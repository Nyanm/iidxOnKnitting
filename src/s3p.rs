//! S3P0 keysound-archive unpacking.
//!
//! An .s3p is a header (magic "S3P0", u32 count, then count x (u32 offset, u32 size))
//! followed by S3V0 blocks; each block's payload (after its 0x20 header) is one
//! ASF/WMAv2 keysound. 1-based sample N maps to archive index N-1. Implemented in Step 2.
