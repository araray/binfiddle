//! Bounded positional source readers and source identity records.
//!
//! Readers expose a known length and `read_exact_at`-style positional access
//! with checked arithmetic, explicit budget charging, and short-read errors.
//! Nothing in this layer assumes the whole artifact fits in memory or is
//! memory-mapped.
//!
//! Source identity separates three notions:
//! - [`SourceObservation`] — what was seen when opening (length, stamps).
//! - [`SourceRevision`] — bytes under a named verification policy; the record
//!   states whether content was actually hashed (`content_verified`) or only
//!   observed (`observation`).
//! - the computed identifier — a domain-separated `src:` id over the semantic
//!   revision record. Machine-local facts (paths, timestamps) stay out of the
//!   identity so the same bytes yield the same id everywhere.

use super::budget::Budget;
use super::error::NnError;
use super::id::{compute_id, IdKind};
use super::json::Json;
use sha2::{Digest, Sha256};
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

/// Read chunk used for streaming passes (full-content hashing).
const STREAM_CHUNK: usize = 8 * 1024 * 1024;

/// A bounded positional reader over an open file.
#[derive(Debug)]
pub struct BoundedFile {
    file: File,
    length: u64,
}

impl BoundedFile {
    /// Open a file and record its length. The length is re-checked against
    /// reality by `verify_length` before stored addresses are trusted.
    pub fn open(path: &Path) -> Result<BoundedFile, NnError> {
        let file = File::open(path)?;
        let length = file.metadata()?.len();
        Ok(BoundedFile { file, length })
    }

    /// The length observed at open time.
    pub fn length(&self) -> u64 {
        self.length
    }

    /// Re-stat the underlying file and compare with the open-time length.
    /// A change is reported as `SOURCE_CHANGED`.
    pub fn verify_length(&self, path: &Path) -> Result<(), NnError> {
        let current = std::fs::metadata(path)?.len();
        if current != self.length {
            return Err(NnError::SourceChanged {
                detail: format!(
                    "source length changed during operation: observed {}, now {}",
                    self.length, current
                ),
            });
        }
        Ok(())
    }

    /// Read exactly `buf.len()` bytes at `offset`. Fails with
    /// `INVALID_REQUEST` on range overflow and `SOURCE_CHANGED`-style bounds
    /// errors when the range passes the recorded end of the file; a short read
    /// at the physical end of file is never silently zero-filled.
    pub fn read_exact_at(&self, offset: u64, buf: &mut [u8]) -> Result<(), NnError> {
        if buf.is_empty() {
            return Ok(());
        }
        let end = offset
            .checked_add(buf.len() as u64)
            .ok_or_else(|| NnError::InvalidRequest {
                message: format!("read range {}..+{} overflows u64", offset, buf.len()),
            })?;
        if end > self.length {
            return Err(NnError::InvalidRequest {
                message: format!(
                    "read range {}..{} exceeds source length {}",
                    offset, end, self.length
                ),
            });
        }
        let mut file = &self.file;
        file.seek(SeekFrom::Start(offset))?;
        read_exact_short_fail(&mut file, buf).map_err(|_| NnError::SourceChanged {
            detail: format!(
                "short read at offset {} (expected {} bytes): source changed or truncated",
                offset,
                buf.len()
            ),
        })
    }

    /// Read exactly `buf.len()` bytes at `offset`, charging the budget for the
    /// bytes read. This is the normal entry point for NN operations.
    pub fn read_exact_at_bounded(
        &self,
        offset: u64,
        buf: &mut [u8],
        budget: &Budget,
    ) -> Result<(), NnError> {
        budget.consume_source_read(buf.len() as u64)?;
        self.read_exact_at(offset, buf)
    }

    /// Hash the entire content with SHA-256, streaming in bounded chunks and
    /// charging the budget. The caller decides when a full-content hash is
    /// worth the read; this is never implicit.
    pub fn content_digest(&self, budget: &Budget) -> Result<String, NnError> {
        let mut hasher = Sha256::new();
        let mut chunk = vec![0u8; STREAM_CHUNK];
        let mut offset: u64 = 0;
        while offset < self.length {
            let take = std::cmp::min(chunk.len() as u64, self.length - offset) as usize;
            let slice = &mut chunk[..take];
            self.read_exact_at_bounded(offset, slice, budget)?;
            budget.checkpoint()?;
            hasher.update(slice);
            offset += take as u64;
        }
        Ok(hex::encode(hasher.finalize()))
    }
}

fn read_exact_short_fail(reader: &mut impl Read, mut buf: &mut [u8]) -> std::io::Result<()> {
    while !buf.is_empty() {
        match reader.read(buf) {
            Ok(0) => {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::UnexpectedEof,
                    "unexpected end of file",
                ))
            }
            Ok(n) => buf = &mut buf[n..],
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
            Err(e) => return Err(e),
        }
    }
    Ok(())
}

/// How strongly a source revision's bytes are known.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SourceConsistency {
    /// Only opening metadata was observed; payload bytes are unverified.
    Observation,
    /// The complete identified byte stream was hashed and matched a digest.
    ContentVerified,
}

impl SourceConsistency {
    pub fn as_str(self) -> &'static str {
        match self {
            SourceConsistency::Observation => "observation",
            SourceConsistency::ContentVerified => "content_verified",
        }
    }
}

/// Identity record for one source byte stream.
#[derive(Debug, Clone)]
pub struct SourceRevision {
    /// Display/handling hint only; not part of the semantic identity.
    pub relative_path: PathBuf,
    pub length: u64,
    pub content_digest: Option<String>,
    pub consistency: SourceConsistency,
}

impl SourceRevision {
    /// Metadata-only revision (length observed, content not hashed).
    pub fn observed(relative_path: PathBuf, length: u64) -> SourceRevision {
        SourceRevision {
            relative_path,
            length,
            content_digest: None,
            consistency: SourceConsistency::Observation,
        }
    }

    /// Content-verified revision from an explicitly performed full hash.
    pub fn content_verified(relative_path: PathBuf, length: u64, digest: String) -> SourceRevision {
        SourceRevision {
            relative_path,
            length,
            content_digest: Some(digest),
            consistency: SourceConsistency::ContentVerified,
        }
    }

    /// Semantic record (wire subset; integers as decimal strings).
    pub fn semantic(&self) -> Result<Json, NnError> {
        let digest = match &self.content_digest {
            Some(value) => Json::object(vec![
                ("algorithm", Json::Str("sha256".to_string())),
                ("value", Json::Str(value.clone())),
            ])?,
            None => Json::Null,
        };
        Json::object(vec![
            ("schema", Json::Str("binfiddle.nn.source/v1".to_string())),
            ("kind", Json::Str("file".to_string())),
            ("length", Json::Str(self.length.to_string())),
            ("content_digest", digest),
            (
                "observation",
                Json::object(vec![("method", Json::Str("open_metadata".to_string()))])?,
            ),
            (
                "consistency",
                Json::Str(self.consistency.as_str().to_string()),
            ),
            (
                "acquisition",
                Json::object(vec![
                    ("method", Json::Str("bounded_reader".to_string())),
                    ("point_in_time_claim", Json::Bool(false)),
                ])?,
            ),
        ])
    }

    /// Domain-separated identifier for this revision.
    pub fn id(&self) -> Result<String, NnError> {
        compute_id(IdKind::Source, &self.semantic()?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn write_temp(name: &str, data: &[u8]) -> (tempfile::TempDir, PathBuf) {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join(name);
        let mut file = File::create(&path).expect("create");
        file.write_all(data).expect("write");
        (dir, path)
    }

    #[test]
    fn reads_exact_ranges() {
        let (_dir, path) = write_temp("f.bin", &[0, 1, 2, 3, 4, 5, 6, 7]);
        let reader = BoundedFile::open(&path).unwrap();
        let mut buf = [0u8; 4];
        reader.read_exact_at(2, &mut buf).unwrap();
        assert_eq!(buf, [2, 3, 4, 5]);
    }

    #[test]
    fn rejects_out_of_range_reads() {
        let (_dir, path) = write_temp("f.bin", &[0, 1, 2, 3]);
        let reader = BoundedFile::open(&path).unwrap();
        let mut buf = [0u8; 4];
        assert_eq!(
            reader
                .read_exact_at(1, &mut buf)
                .unwrap_err()
                .code()
                .as_str(),
            "INVALID_REQUEST"
        );
        let mut buf2 = [0u8; 1];
        assert_eq!(
            reader
                .read_exact_at(4, &mut buf2)
                .unwrap_err()
                .code()
                .as_str(),
            "INVALID_REQUEST"
        );
    }

    #[test]
    fn rejects_overflowing_ranges() {
        let (_dir, path) = write_temp("f.bin", &[0]);
        let reader = BoundedFile::open(&path).unwrap();
        let mut buf = [0u8; 16];
        assert!(reader.read_exact_at(u64::MAX - 2, &mut buf).is_err());
    }

    #[test]
    fn empty_read_is_ok() {
        let (_dir, path) = write_temp("f.bin", &[0]);
        let reader = BoundedFile::open(&path).unwrap();
        assert!(reader.read_exact_at(0, &mut []).is_ok());
    }

    #[test]
    fn content_digest_matches_sha256sum() {
        // sha256("abc")
        let (_dir, path) = write_temp("abc.bin", b"abc");
        let reader = BoundedFile::open(&path).unwrap();
        let budget = Budget::unrestricted();
        assert_eq!(
            reader.content_digest(&budget).unwrap(),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }

    #[test]
    fn content_digest_is_budgeted() {
        let (_dir, path) = write_temp("abc.bin", b"abc");
        let reader = BoundedFile::open(&path).unwrap();
        let caps = super::super::budget::BudgetCaps {
            source_bytes_read: 2,
            ..super::super::budget::BudgetCaps::default()
        };
        let budget = Budget::new(caps, None, super::super::cancel::CancellationToken::new());
        assert_eq!(
            reader.content_digest(&budget).unwrap_err().code().as_str(),
            "BUDGET_EXCEEDED"
        );
    }

    #[test]
    fn truncated_file_surfaces_short_read_as_source_change() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("f.bin");
        let mut file = File::create(&path).unwrap();
        file.write_all(&[0, 1, 2, 3]).unwrap();
        let reader = BoundedFile::open(&path).unwrap();
        // Shrink the file after opening: recorded length is now a lie.
        let file2 = File::create(&path).unwrap();
        drop(file2);
        let mut buf = [0u8; 4];
        let err = reader.read_exact_at(0, &mut buf).unwrap_err();
        assert_eq!(err.code().as_str(), "SOURCE_CHANGED");
    }

    #[test]
    fn verify_length_detects_growth() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("f.bin");
        std::fs::write(&path, [0, 1]).unwrap();
        let reader = BoundedFile::open(&path).unwrap();
        assert!(reader.verify_length(&path).is_ok());
        std::fs::write(&path, [0, 1, 2]).unwrap();
        assert_eq!(
            reader.verify_length(&path).unwrap_err().code().as_str(),
            "SOURCE_CHANGED"
        );
    }

    #[test]
    fn revision_id_is_deterministic_and_digest_sensitive() {
        let a = SourceRevision::observed(PathBuf::from("a.bin"), 18);
        let b = SourceRevision::observed(PathBuf::from("different-name.bin"), 18);
        // Same bytes and policy: identity is path-independent.
        assert_eq!(a.id().unwrap(), b.id().unwrap());
        let c = SourceRevision::observed(PathBuf::from("a.bin"), 19);
        assert_ne!(a.id().unwrap(), c.id().unwrap());
        let d = SourceRevision::content_verified(
            PathBuf::from("a.bin"),
            18,
            // sha256("abc"), verified against sha256sum.
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad".to_string(),
        );
        assert_ne!(a.id().unwrap(), d.id().unwrap());
        assert!(a.id().unwrap().starts_with("src:"));
        assert_eq!(a.id().unwrap().len(), "src:".len() + 64);
    }

    #[test]
    fn semantic_record_is_canonicalizable() {
        let revision = SourceRevision::observed(PathBuf::from("a.bin"), 18);
        let semantic = revision.semantic().unwrap();
        assert!(semantic.to_canonical().is_ok());
        assert_eq!(
            semantic.get("consistency"),
            Some(&Json::Str("observation".to_string()))
        );
    }
}
