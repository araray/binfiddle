//! Minimal ZIP skeleton reader shared by container-format tiers
//! (PyTorch checkpoints, NPZ captures).
//!
//! Descriptor-only: locates entries and their data spans; never
//! decompresses anything.

use crate::nn::budget::Budget;
use crate::nn::error::NnError;
use crate::nn::source::BoundedFile;

pub const EOCD_MAGIC: [u8; 4] = [0x50, 0x4b, 0x05, 0x06];
pub const CD_ENTRY_MAGIC: [u8; 4] = [0x50, 0x4b, 0x01, 0x02];
pub const LOCAL_MAGIC: [u8; 4] = [0x50, 0x4b, 0x03, 0x04];
/// EOCD is a fixed 22-byte record plus an optional comment (<= 64 KiB).
const EOCD_SCAN: u64 = 22 + 65_536;

/// One ZIP entry, located.
#[derive(Debug, Clone)]
pub struct ZipEntry {
    pub name: String,
    /// Compression method (0 = STORED, 8 = DEFLATED).
    pub compression: u16,
    /// Absolute file offset of the entry's (possibly compressed) data.
    pub data_offset: u64,
    /// Bytes the data occupies in the file (compressed size).
    pub stored_size: u64,
    /// Declared uncompressed size (from the directory).
    pub uncompressed_size: u64,
}

pub fn read_at(
    file: &BoundedFile,
    offset: u64,
    len: usize,
    budget: &Budget,
) -> Result<Vec<u8>, NnError> {
    let mut buf = vec![0u8; len];
    budget.consume_metadata(len as u64)?;
    file.read_exact_at(offset, &mut buf)?;
    Ok(buf)
}

/// Locate the end-of-central-directory record and parse the archive.
pub fn parse_zip(file: &BoundedFile, budget: &Budget) -> Result<Vec<ZipEntry>, NnError> {
    let length = file.length();
    if length < 22 {
        return Err(NnError::MalformedInput {
            detail: "zip: file shorter than a ZIP end record".to_string(),
        });
    }
    let scan_start = length.saturating_sub(EOCD_SCAN);
    let scan = read_at(file, scan_start, (length - scan_start) as usize, budget)?;
    let eocd = scan
        .windows(4)
        .rev()
        .position(|w| w == EOCD_MAGIC)
        .map(|back| scan.len() - back - 4)
        .ok_or_else(|| NnError::MalformedInput {
            detail: "zip: no end-of-central-directory record".to_string(),
        })?;
    let total_entries = u16_at(&scan, eocd + 10) as usize;
    let cd_size = u32_at(&scan, eocd + 12) as u64;
    let cd_offset = u32_at(&scan, eocd + 16) as u64;
    if cd_offset.saturating_add(cd_size) > length {
        return Err(NnError::MalformedInput {
            detail: "zip: central directory beyond end of file".to_string(),
        });
    }
    let cd = read_at(file, cd_offset, cd_size as usize, budget)?;
    let mut entries = Vec::with_capacity(total_entries.min(4096));
    let mut pos = 0usize;
    for _ in 0..total_entries {
        if pos + 46 > cd.len() || cd[pos..pos + 4] != CD_ENTRY_MAGIC {
            return Err(NnError::MalformedInput {
                detail: format!("zip: central directory entry {} malformed", entries.len()),
            });
        }
        let compression = u16_at(&cd, pos + 10);
        let compressed = u32_at(&cd, pos + 20) as u64;
        let uncompressed = u32_at(&cd, pos + 24) as u64;
        let name_len = u16_at(&cd, pos + 28) as usize;
        let extra_len = u16_at(&cd, pos + 30) as usize;
        let comment_len = u16_at(&cd, pos + 32) as usize;
        let local_offset = u32_at(&cd, pos + 42) as u64;
        if pos + 46 + name_len > cd.len() {
            return Err(NnError::MalformedInput {
                detail: "zip: entry name beyond central directory".to_string(),
            });
        }
        let name = String::from_utf8_lossy(&cd[pos + 46..pos + 46 + name_len]).into_owned();
        // The local header carries its own (possibly different) name/extra
        // lengths; the data begins after them.
        let mut lhead = [0u8; 30];
        budget.consume_metadata(30)?;
        file.read_exact_at(local_offset, &mut lhead)?;
        if lhead[0..4] != LOCAL_MAGIC {
            return Err(NnError::MalformedInput {
                detail: format!("zip: local header missing for {name}"),
            });
        }
        let l_name = u16_at(&lhead, 26) as u64;
        let l_extra = u16_at(&lhead, 28) as u64;
        entries.push(ZipEntry {
            name,
            compression,
            data_offset: local_offset + 30 + l_name + l_extra,
            stored_size: compressed,
            uncompressed_size: uncompressed,
        });
        pos += 46 + name_len + extra_len + comment_len;
    }
    Ok(entries)
}

pub fn u16_at(buf: &[u8], at: usize) -> u16 {
    u16::from_le_bytes([buf[at], buf[at + 1]])
}

pub fn u32_at(buf: &[u8], at: usize) -> u32 {
    u32::from_le_bytes([buf[at], buf[at + 1], buf[at + 2], buf[at + 3]])
}

pub fn u64_at(buf: &[u8], at: usize) -> u64 {
    let mut b = [0u8; 8];
    b.copy_from_slice(&buf[at..at + 8]);
    u64::from_le_bytes(b)
}
