//! IFS (`.ifs`) unpacking via a minimal, read-only KBin manifest reader.
//!
//! An `.ifs` is a Konami container: a 36-byte header, then a *manifest* (a compressed-KBin
//! binary-XML blob listing each packed file), then the packed data blob. We don't need a
//! general XML parser — only the manifest's file leaves, which are `3s32` triples
//! (offset, size, timestamp) whose offset/size are relative to the data blob. We walk the KBin
//! node+data buffers just far enough to recover each leaf's name and its (offset, size).
//!
//! File names are escaped in the manifest (`.`→`_E`, `_`→`__`, a leading digit gets a `_`
//! prefix); we reverse that so callers can match `.s3p` / `.2dx` / `.1` by extension.
//! Reference: ifstools (github.com/mon/ifstools) and kbinxml (github.com/mon/kbinxml).

use crate::bytes::read_u32_be;

use anyhow::{Context, Result, bail, ensure};

const IFS_MAGIC: [u8; 4] = [0x6C, 0xAD, 0x8F, 0x89]; // IFS signature (big-endian 0x6CAD8F89)
const IFS_ENCRYPTED_MAGIC: [u8; 4] = [0x72, 0x9B, 0x79, 0xB1]; // 256-byte encrypted/locked stubs (18 seen)
const IFS_HEADER_LEN: usize = 36;                    // sig+ver+~ver+time+treeSize+manifestEnd+md5
const MANIFEST_END_OFFSET: usize = 0x10;             // u32(be): where the manifest ends / data begins
const MANIFEST_BASE: usize = 0x24;                   // manifest (KBin) starts right after the 36B header

const KBIN_SIGNATURE: u8 = 0xA0;        // KBin magic byte
const KBIN_SIG_COMPRESSED: u8 = 0x42;   // node names are six-bit packed
const KBIN_SIG_UNCOMPRESSED: u8 = 0x45; // node names are length-prefixed raw bytes
const KBIN_HEADER_LEN: usize = 8;       // sig + compress + enc + ~enc + u32 nodeBufLen

const NODE_TYPE_VOID: u8 = 1;        // container node, carries no value
const NODE_TYPE_3S32: u8 = 30;       // a packed file leaf: (offset, size, timestamp)
const NODE_TYPE_ATTR: u8 = 46;       // an attribute (length-prefixed string value)
const NODE_TYPE_END: u8 = 190;       // pop to parent
const NODE_TYPE_SECTION_END: u8 = 191; // end of the node section
const ARRAY_FLAG: u8 = 64;           // bit 6 marks an array (and is always set on END markers)

// six-bit name alphabet (index -> char), per kbinxml/sixbit.py
const SIXBIT_CHARS: &[u8; 64] = b"0123456789:ABCDEFGHIJKLMNOPQRSTUVWXYZ_abcdefghijklmnopqrstuvwxyz";

/// One packed file inside an IFS. `offset` is absolute into the original `.ifs` bytes.
pub struct Member {
    pub name: String,  // un-escaped file name, e.g. "28000.s3p" / "28000.1" / "28000_pre.2dx"
    pub offset: usize, // absolute byte offset into the .ifs
    pub size: usize,   // byte length
}

/// Parse the IFS header + KBin manifest, returning every packed file with absolute offsets.
pub fn list_members(bytes_ifs: &[u8]) -> Result<Vec<Member>> {
    ensure!(bytes_ifs.len() >= 4, "file too small to be an IFS archive ({} bytes)", bytes_ifs.len());
    if bytes_ifs[0..4] == IFS_ENCRYPTED_MAGIC {
        bail!(
            "encrypted/locked IFS (magic {:02X?}) — not supported (DRM/online-unlock stub)",
            &bytes_ifs[0..4]
        );
    }
    ensure!(
        bytes_ifs.len() >= IFS_HEADER_LEN && bytes_ifs[0..4] == IFS_MAGIC,
        "not an IFS archive (magic {:02X?}, expected {:02X?})",
        &bytes_ifs[0..4],
        IFS_MAGIC
    );
    let manifest_end = read_u32_be(bytes_ifs, MANIFEST_END_OFFSET)? as usize;
    ensure!(
        MANIFEST_BASE < manifest_end && manifest_end <= bytes_ifs.len(),
        "IFS manifest range [{MANIFEST_BASE}..{manifest_end}] invalid for size {}",
        bytes_ifs.len()
    );

    // file offsets in the manifest are relative to the data blob, which begins at manifest_end
    let data_blob_base = manifest_end;
    let members_relative = read_manifest(bytes_ifs, MANIFEST_BASE, manifest_end)?;

    let mut members = Vec::with_capacity(members_relative.len());
    for (name, offset_rel, size) in members_relative {
        let offset = data_blob_base
            .checked_add(offset_rel)
            .with_context(|| format!("member {name} offset overflow"))?;
        ensure!(
            offset.checked_add(size).is_some_and(|end| end <= bytes_ifs.len()),
            "member {name} [{offset}..+{size}] exceeds .ifs size {}",
            bytes_ifs.len()
        );
        members.push(Member { name, offset, size });
    }
    Ok(members)
}

// Walk the KBin manifest and collect file leaves as (name, relative_offset, size).
fn read_manifest(bytes: &[u8], base: usize, end: usize) -> Result<Vec<(String, usize, usize)>> {
    ensure!(base + KBIN_HEADER_LEN <= end, "KBin manifest too short");
    ensure!(bytes[base] == KBIN_SIGNATURE, "bad KBin signature");
    let compressed = match bytes[base + 1] {
        KBIN_SIG_COMPRESSED => true,
        KBIN_SIG_UNCOMPRESSED => false,
        other => bail!("unknown KBin compression flag {other:#x}"),
    };

    let node_buf_len = read_u32_be(bytes, base + 4)? as usize;
    let node_end = base + KBIN_HEADER_LEN + node_buf_len; // node section [base+8 .. node_end)
    ensure!(node_end + 4 <= end, "KBin node section overruns the manifest");

    let mut walk = Walk {
        bytes,
        manifest_end: end,
        node_cursor: base + KBIN_HEADER_LEN,
        node_end,
        compressed,
        // data section: a u32 dataSize at node_end, then values; the dword cursor starts past it,
        // the byte/word cursors start at node_end and snap to the dword cursor on first use.
        off_dword: node_end + 4,
        off_byte: node_end,
        off_word: node_end,
    };
    walk.run()
}

// Mutable state for one manifest walk: the node cursor plus the three data-buffer cursors.
struct Walk<'a> {
    bytes: &'a [u8],
    manifest_end: usize,
    node_cursor: usize,
    node_end: usize,
    compressed: bool,
    off_dword: usize, // 4-byte-aligned values, bin/str/array length-prefixed reads, attr strings
    off_byte: usize,  // sub-cursor for 1-byte values packed into a shared 4-byte word
    off_word: usize,  // sub-cursor for 2-byte values packed into a shared 4-byte word
}

impl Walk<'_> {
    fn run(&mut self) -> Result<Vec<(String, usize, usize)>> {
        let mut files = Vec::new();
        let mut guard_node_count = 0usize;
        while self.node_cursor < self.node_end {
            guard_node_count += 1;
            ensure!(guard_node_count < 1_000_000, "KBin walk exceeded node budget (corrupt?)");

            while self.node_cursor < self.node_end && self.bytes[self.node_cursor] == 0 {
                self.node_cursor += 1; // skip inter-node zero padding
            }
            if self.node_cursor >= self.node_end {
                break;
            }
            let raw = self.bytes[self.node_cursor];
            self.node_cursor += 1;
            let is_array = raw & ARRAY_FLAG != 0;
            let type_id = raw & !ARRAY_FLAG;

            match type_id {
                NODE_TYPE_SECTION_END => break,
                NODE_TYPE_END => continue, // no name, no value
                NODE_TYPE_VOID => {
                    self.read_name()?; // container node, no value
                }
                NODE_TYPE_ATTR => {
                    self.read_name()?;
                    self.grab_prefixed()?; // attribute string value — consumed, not used
                }
                NODE_TYPE_3S32 => {
                    let name = self.read_name()?;
                    let (start, _len) = self.grab_value(type_id, is_array)?;
                    let offset = read_u32_be(self.bytes, start)? as usize;
                    let size = read_u32_be(self.bytes, start + 4)? as usize;
                    files.push((fix_name(&name), offset, size));
                }
                _ => {
                    self.read_name()?;
                    self.grab_value(type_id, is_array)?; // other typed value — consume to stay aligned
                }
            }
        }
        Ok(files)
    }

    // Read a node/attribute name: six-bit packed (compressed) or length-prefixed raw (uncompressed).
    fn read_name(&mut self) -> Result<String> {
        if self.compressed {
            self.read_sixbit_name()
        } else {
            ensure!(self.node_cursor < self.node_end, "name length past node section");
            let length = (self.bytes[self.node_cursor] & !ARRAY_FLAG) as usize + 1;
            self.node_cursor += 1;
            let end = self.node_cursor + length;
            ensure!(end <= self.node_end, "raw name overruns node section");
            let name = String::from_utf8_lossy(&self.bytes[self.node_cursor..end]).into_owned();
            self.node_cursor = end;
            Ok(name)
        }
    }

    // Decode a six-bit packed name: u8 char-count, then ceil(count*6/8) bytes of MSB-first groups.
    fn read_sixbit_name(&mut self) -> Result<String> {
        ensure!(self.node_cursor < self.node_end, "sixbit length past node section");
        let length = self.bytes[self.node_cursor] as usize;
        self.node_cursor += 1;
        let nbytes = (length * 6 + 7) / 8;
        let end = self.node_cursor + nbytes;
        ensure!(end <= self.node_end, "sixbit name overruns node section");
        let raw = &self.bytes[self.node_cursor..end];
        self.node_cursor = end;

        let mut name = String::with_capacity(length);
        for index_char in 0..length {
            let mut value = 0u8;
            for bit in 0..6 {
                let bit_index = index_char * 6 + bit;
                let byte = raw[bit_index / 8];
                value = (value << 1) | ((byte >> (7 - (bit_index % 8))) & 1);
            }
            name.push(SIXBIT_CHARS[value as usize] as char);
        }
        Ok(name)
    }

    // Read one typed value, returning the byte range [start, start+len) it occupied in the data
    // buffer. Arrays and the variable types (bin/str) are length-prefixed; fixed types use the
    // dword/word/byte aligned scheme.
    fn grab_value(&mut self, type_id: u8, is_array: bool) -> Result<(usize, usize)> {
        let (elem_size, count) = value_dims(type_id)
            .with_context(|| format!("unsupported KBin node type {type_id}"))?;
        if is_array || count < 0 {
            self.grab_prefixed()
        } else {
            self.grab_aligned(elem_size * count as usize)
        }
    }

    // Length-prefixed read: a u32(be) byte count, then that many bytes, then realign to 4.
    fn grab_prefixed(&mut self) -> Result<(usize, usize)> {
        let length = read_u32_be(self.bytes, self.off_dword)? as usize;
        self.off_dword += 4;
        let start = self.off_dword;
        let end = start.checked_add(length).context("prefixed value overflow")?;
        ensure!(end <= self.manifest_end, "prefixed value overruns manifest");
        self.off_dword = end;
        self.realign_dword();
        Ok((start, length))
    }

    // Aligned fixed-size read (KBin's three-cursor scheme): 1-byte values pack into a shared
    // 4-byte word via off_byte, 2-byte via off_word, everything else advances the dword cursor.
    fn grab_aligned(&mut self, size: usize) -> Result<(usize, usize)> {
        if self.off_byte % 4 == 0 {
            self.off_byte = self.off_dword;
        }
        if self.off_word % 4 == 0 {
            self.off_word = self.off_dword;
        }
        let start = match size {
            1 => {
                let at = self.off_byte;
                self.off_byte += 1;
                at
            }
            2 => {
                let at = self.off_word;
                self.off_word += 2;
                at
            }
            _ => {
                let at = self.off_dword;
                self.off_dword += size;
                self.realign_dword();
                at
            }
        };
        // the dword cursor must clear any word/byte sub-cursor that ran ahead of it
        let trailing = self.off_byte.max(self.off_word);
        if self.off_dword < trailing {
            self.off_dword = trailing;
            self.realign_dword();
        }
        let end = start.checked_add(size).context("aligned value overflow")?;
        ensure!(end <= self.manifest_end, "aligned value overruns manifest");
        Ok((start, size))
    }

    fn realign_dword(&mut self) {
        while self.off_dword % 4 != 0 {
            self.off_dword += 1;
        }
    }
}

// KBin type id -> (element byte size, element count); count == -1 means length-prefixed (bin/str).
// Mirrors kbinxml/format_ids.py for the types an IFS manifest can carry.
fn value_dims(type_id: u8) -> Option<(usize, i64)> {
    let dims = match type_id {
        2 | 3 => (1, 1),
        4 | 5 => (2, 1),
        6 | 7 | 12 | 13 | 14 => (4, 1),
        8 | 9 | 15 => (8, 1),
        10 | 11 => (1, -1), // bin / str
        16 | 17 => (1, 2),
        18 | 19 => (2, 2),
        20 | 21 | 24 => (4, 2),
        22 | 23 | 25 => (8, 2),
        26 | 27 => (1, 3),
        28 | 29 => (2, 3),
        30 | 31 | 34 => (4, 3),
        32 | 33 | 35 => (8, 3),
        36 | 37 => (1, 4),
        38 | 39 => (2, 4),
        40 | 41 | 44 => (4, 4),
        42 | 43 | 45 => (8, 4),
        48 | 49 | 56 => (1, 16),
        50 | 51 => (2, 8),
        52 => (1, 1),
        53 => (1, 2),
        54 => (1, 3),
        55 => (1, 4),
        _ => return None,
    };
    Some(dims)
}

// Reverse the manifest's name escaping into a real filename.
fn fix_name(raw: &str) -> String {
    let mut name = raw.replace("_E", ".").replace("__", "_");
    let mut chars = name.chars();
    if chars.next() == Some('_') && chars.next().is_some_and(|c| c.is_ascii_digit()) {
        name.remove(0);
    }
    name
}
