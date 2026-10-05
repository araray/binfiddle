//! Artifact carving: scanning raw binary sources for embedded model
//! containers.
//!
//! Strong evidence first: GGUF magic bytes and structurally valid SafeTensors
//! headers (a plausible length prefix followed by parseable JSON with
//! coherent descriptor offsets). Candidates that survive a full structural
//! parse through the ordinary M1 readers on a subview are labeled
//! `verified`; plausible-but-unparseable findings stay `candidate` bytes —
//! never presented as tensors. Floating-point-looking bytes alone are not
//! evidence of anything.

use super::budget::Budget;
use super::error::NnError;
use super::format::{gguf, safetensors, Validity};
use super::json::{Json, ParseLimits};
use super::report::ResultEnvelope;
use super::source::BoundedFile;
use std::path::Path;

/// Scan chunk with overlap so magic patterns cannot straddle boundaries.
const CHUNK: usize = 4 * 1024 * 1024;
const OVERLAP: usize = 16;

/// One carved finding.
#[derive(Debug, Clone)]
pub struct CarveFinding {
    pub format: &'static str,
    /// Absolute byte span of the container in the scanned source.
    pub start: u64,
    pub length: u64,
    pub confidence: &'static str, // "verified" | "candidate"
    pub tensor_count: usize,
    pub detail: String,
}

/// Scan a file for embedded model containers.
pub fn carve(path: &Path, budget: &Budget) -> Result<Vec<CarveFinding>, NnError> {
    let reader = BoundedFile::open(path)?;
    let total = reader.length();
    let mut findings: Vec<CarveFinding> = Vec::new();
    let mut covered: Vec<(u64, u64)> = Vec::new();

    let mut buffer = vec![0u8; CHUNK + OVERLAP];
    let mut chunk_start: u64 = 0;
    while chunk_start < total {
        let take = ((CHUNK + OVERLAP) as u64).min(total - chunk_start) as usize;
        budget.consume_source_read(take as u64)?;
        reader.read_exact_at(chunk_start, &mut buffer[..take])?;
        budget.checkpoint()?;
        let window = &buffer[..take];

        // GGUF magic at every position in the window (skip positions inside
        // already-verified spans).
        for i in 0..window.len().saturating_sub(4) {
            let absolute = chunk_start + i as u64;
            if covered.iter().any(|(s, e)| absolute >= *s && absolute < *e) {
                continue;
            }
            if &window[i..i + 4] == b"GGUF" {
                if let Some(finding) = validate_gguf(&reader, absolute, budget)? {
                    covered.push((finding.start, finding.start + finding.length));
                    findings.push(finding);
                }
            }
        }

        // SafeTensors: the header starts with '{'; a plausible container has
        // an 8-byte LE length before it. Check every '{' whose preceding 8
        // bytes lie in the window.
        for i in 0..window.len() {
            if window[i] != b'{' {
                continue;
            }
            let absolute = chunk_start + i as u64;
            if covered.iter().any(|(s, e)| absolute >= *s && absolute < *e) {
                continue;
            }
            if absolute < 8 {
                continue;
            }
            let prefix_start = i as u64 + chunk_start - 8;
            let window_pos = (prefix_start - chunk_start) as usize;
            if window_pos + 8 > window.len() {
                continue;
            }
            let header_len = u64::from_le_bytes(
                window[window_pos..window_pos + 8]
                    .try_into()
                    .expect("8 bytes"),
            );
            // Plausible: positive, sane, within the file, within metadata
            // budgets.
            if !(2..=64 * 1024 * 1024).contains(&header_len) {
                continue;
            }
            if prefix_start + 8 + header_len > total {
                continue;
            }
            if let Some(finding) = validate_safetensors(&reader, prefix_start, header_len, budget)?
            {
                covered.push((finding.start, finding.start + finding.length));
                findings.push(finding);
            }
        }

        chunk_start += CHUNK as u64;
    }
    findings.sort_by_key(|f| f.start);
    Ok(findings)
}

fn validate_gguf(
    reader: &BoundedFile,
    start: u64,
    budget: &Budget,
) -> Result<Option<CarveFinding>, NnError> {
    // A GGUF container's extent is only known after walking the directory;
    // validate on a subview covering the rest of the file (the readers
    // bound-check against the subview length).
    let remaining = reader.length() - start;
    let subview = reader.subview(start, remaining)?;
    match gguf::inventory(&subview, budget) {
        Ok(inventory) => {
            // Exact extent: data origin plus the largest tensor end when
            // known; otherwise report the directory end as a lower bound.
            let last_end = inventory
                .tensors
                .iter()
                .filter_map(|t| t.payload_length.map(|l| t.payload_start + l))
                .max()
                .unwrap_or(0);
            let exact_known = inventory.tensors.iter().all(|t| t.payload_length.is_some())
                && inventory.validity == Validity::Valid;
            let length = if exact_known && last_end > 0 {
                last_end
            } else {
                remaining // bounded: at most the rest of the file
            };
            Ok(Some(CarveFinding {
                format: "gguf",
                start,
                length,
                confidence: if inventory.validity == Validity::Valid {
                    "verified"
                } else {
                    "candidate"
                },
                tensor_count: inventory.tensors.len(),
                detail: format!(
                    "v{} inventory parsed (validity {})",
                    inventory.format_version,
                    inventory.validity.as_str()
                ),
            }))
        }
        Err(err) => Ok(Some(CarveFinding {
            format: "gguf",
            start,
            length: remaining,
            confidence: "candidate",
            tensor_count: 0,
            detail: format!(
                "magic found but structure unreadable: {}",
                super::error::brief(&err.to_string())
            ),
        })),
    }
}

fn validate_safetensors(
    reader: &BoundedFile,
    start: u64,
    header_len: u64,
    budget: &Budget,
) -> Result<Option<CarveFinding>, NnError> {
    // Read and parse the JSON header first (cheap, strong evidence).
    let mut header_bytes = vec![0u8; header_len as usize];
    budget.consume_source_read(header_len)?;
    reader.read_exact_at(start + 8, &mut header_bytes)?;
    let header_text = match std::str::from_utf8(&header_bytes) {
        Ok(text) => text.to_string(),
        Err(_) => {
            return Ok(Some(CarveFinding {
                format: "safetensors",
                start,
                length: 8 + header_len,
                confidence: "candidate",
                tensor_count: 0,
                detail: "length prefix plausible but header is not UTF-8".to_string(),
            }))
        }
    };
    let parsed = Json::parse_foreign(&header_text, ParseLimits::default())?;
    if !matches!(parsed, Json::Object(_)) {
        return Ok(None); // not a safetensors header at all — silent skip
    }
    // Exact extent from the descriptors: 8 + header + data buffer length.
    let mut buffer_len: u64 = 0;
    if let Json::Object(members) = &parsed {
        for (_, value) in members {
            if let Json::Object(fields) = value {
                if let Some(Json::Array(offsets)) = fields
                    .iter()
                    .find(|(k, _)| k == "data_offsets")
                    .map(|(_, v)| v)
                {
                    if let Some(end) = offsets.last().and_then(Json::as_number_u64) {
                        buffer_len = buffer_len.max(end);
                    }
                }
            }
        }
    }
    let extent = 8 + header_len + buffer_len;
    let subview = reader.subview(start, extent)?;
    match safetensors::inventory(&subview, budget) {
        Ok(inventory) => {
            let last_end = inventory
                .tensors
                .iter()
                .filter_map(|t| t.payload_length.map(|l| t.payload_start + l))
                .max()
                .unwrap_or(0);
            let exact = inventory.validity == Validity::Valid && last_end > 0;
            Ok(Some(CarveFinding {
                format: "safetensors",
                start,
                length: if exact { last_end } else { extent },
                confidence: if exact { "verified" } else { "candidate" },
                tensor_count: inventory.tensors.len(),
                detail: format!(
                    "header {} bytes, validity {}",
                    header_len,
                    inventory.validity.as_str()
                ),
            }))
        }
        Err(err) => Ok(Some(CarveFinding {
            format: "safetensors",
            start,
            length: 8 + header_len,
            confidence: "candidate",
            tensor_count: 0,
            detail: format!(
                "JSON header parses but structure is invalid: {}",
                super::error::brief(&err.to_string())
            ),
        })),
    }
}

pub fn carve_envelope(path: &Path, findings: &[CarveFinding]) -> Result<ResultEnvelope, NnError> {
    let items = findings
        .iter()
        .map(|f| {
            Json::object(vec![
                ("format", Json::Str(f.format.to_string())),
                ("start", Json::Str(f.start.to_string())),
                ("length", Json::Str(f.length.to_string())),
                ("confidence", Json::Str(f.confidence.to_string())),
                ("tensor_count", Json::Str(f.tensor_count.to_string())),
                ("detail", Json::Str(f.detail.clone())),
            ])
        })
        .collect::<Result<Vec<_>, _>>()?;
    let semantic = Json::object(vec![
        ("source", Json::Str(path.display().to_string())),
        ("findings", Json::Array(items)),
        (
            "claims",
            Json::Str(
                "candidate spans with stated confidence; verified spans parse structurally — nothing here asserts model validity or recoverability"
                    .to_string(),
            ),
        ),
    ])?;
    Ok(ResultEnvelope::new("carve").with_semantic(semantic))
}

pub fn carve_text(path: &Path, findings: &[CarveFinding]) -> String {
    let mut out = format!("carve {}\n", path.display());
    if findings.is_empty() {
        out.push_str("  no model-container signatures found\n");
    }
    for finding in findings {
        out.push_str(&format!(
            "  {} at [{}, {}) [{}] {} tensors — {}\n",
            finding.format,
            finding.start,
            finding.start + finding.length,
            finding.confidence,
            finding.tensor_count,
            finding.detail
        ));
    }
    out.push_str(
        "  claims: candidate spans with stated confidence; verified spans parse structurally — nothing here asserts model validity or recoverability\n",
    );
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn carve_finds_embedded_safetensors_and_gguf() {
        let dir = tempfile::tempdir().unwrap();
        // Padding + a real safetensors model + padding + a GGUF + padding.
        let header = r#"{"w":{"dtype":"F32","shape":[2,2],"data_offsets":[0,16]}}"#;
        let mut st = Vec::new();
        st.extend_from_slice(&(header.len() as u64).to_le_bytes());
        st.extend_from_slice(header.as_bytes());
        st.extend_from_slice(&[0u8; 16]);

        let mut gguf_bytes = Vec::new();
        gguf_bytes.extend_from_slice(b"GGUF");
        gguf_bytes.extend_from_slice(&3u32.to_le_bytes());
        gguf_bytes.extend_from_slice(&1u64.to_le_bytes()); // tensors
        gguf_bytes.extend_from_slice(&1u64.to_le_bytes()); // metadata
        let key = b"general.architecture";
        gguf_bytes.extend_from_slice(&(key.len() as u64).to_le_bytes());
        gguf_bytes.extend_from_slice(key);
        gguf_bytes.extend_from_slice(&8u32.to_le_bytes());
        gguf_bytes.extend_from_slice(&1u64.to_le_bytes());
        gguf_bytes.push(b't');
        gguf_bytes.extend_from_slice(&1u64.to_le_bytes());
        gguf_bytes.push(b'w');
        gguf_bytes.extend_from_slice(&2u32.to_le_bytes());
        gguf_bytes.extend_from_slice(&1u64.to_le_bytes());
        gguf_bytes.extend_from_slice(&32u64.to_le_bytes());
        gguf_bytes.extend_from_slice(&2u32.to_le_bytes()); // Q4_0
        gguf_bytes.extend_from_slice(&0u64.to_le_bytes());
        while gguf_bytes.len() % 32 != 0 {
            gguf_bytes.push(0);
        }
        gguf_bytes.extend(std::iter::repeat_n(0u8, 18));

        let mut image = Vec::new();
        image.extend(std::iter::repeat_n(0xABu8, 1000));
        let st_start = image.len() as u64;
        image.extend_from_slice(&st);
        image.extend(std::iter::repeat_n(0xCDu8, 500));
        let gguf_start = image.len() as u64;
        image.extend_from_slice(&gguf_bytes);
        image.extend(std::iter::repeat_n(0xEFu8, 300));
        let path = dir.path().join("image.bin");
        std::fs::write(&path, &image).unwrap();

        let budget = crate::nn::budget::Budget::new(
            crate::nn::budget::BudgetCaps::default(),
            None,
            crate::nn::cancel::CancellationToken::new(),
        );
        let findings = carve(&path, &budget).unwrap();
        let st_finding = findings
            .iter()
            .find(|f| f.format == "safetensors")
            .expect("safetensors found");
        assert_eq!(st_finding.start, st_start);
        assert_eq!(st_finding.confidence, "verified");
        assert_eq!(st_finding.tensor_count, 1);
        assert_eq!(st_finding.length, (8 + header.len() + 16) as u64);

        let gguf_finding = findings
            .iter()
            .find(|f| f.format == "gguf")
            .expect("gguf found");
        assert_eq!(gguf_finding.start, gguf_start);
        assert_eq!(gguf_finding.confidence, "verified");
        assert_eq!(gguf_finding.tensor_count, 1);

        let text = carve_text(&path, &findings);
        assert!(text.contains("[verified]"), "{text}");
        assert!(text.contains("asserts model validity"), "{text}");
    }

    #[test]
    fn carve_reports_clean_files_honestly() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("random.bin");
        std::fs::write(&path, vec![0x13u8; 4096]).unwrap();
        let budget = crate::nn::budget::Budget::new(
            crate::nn::budget::BudgetCaps::default(),
            None,
            crate::nn::cancel::CancellationToken::new(),
        );
        let findings = carve(&path, &budget).unwrap();
        assert!(findings.is_empty());
    }
}
