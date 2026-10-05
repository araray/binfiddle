//! Approximate fingerprints and the local evidence graph.
//!
//! Approximate fingerprints are bounded, sampled evidence: a structural
//! descriptor (shape + encoding + element count) plus digests of a fixed
//! number of evenly spaced payload blocks. They exist to find candidate
//! relationships cheaply; they never certify equality — only the exact
//! fingerprints do. Every approximate record carries its method name,
//! version, sample count, threshold context, and experimental status, and
//! the sampled-block limitation is stated wherever scores appear.
//!
//! The evidence graph links two catalogs with typed edges
//! (`exact_payload_match`, `same_structure`, `similar_under_mapping`), each
//! carrying method/version/score/threshold evidence. Edges assert
//! relationships between bytes — never direction, chronology, or lineage.

use super::budget::Budget;
use super::catalog::Catalog;
use super::error::NnError;
use super::json::Json;
use super::report::ResultEnvelope;
use super::source::BoundedFile;
use sha2::{Digest, Sha256};
use std::path::Path;

/// Method identity for the sampled-block fingerprint.
pub const METHOD_NAME: &str = "sampled-block-digest";
pub const METHOD_VERSION: &str = "v1";
/// Number of sampled blocks per tensor.
pub const SAMPLE_COUNT: usize = 8;
/// Sampled block size (bytes).
pub const BLOCK_BYTES: u64 = 64 * 1024;
/// Default similarity threshold for `similar_under_mapping` edges.
pub const DEFAULT_THRESHOLD: f64 = 0.75;

/// One sampled-block digest with its position.
#[derive(Debug, Clone, PartialEq)]
pub struct SampledBlock {
    /// Byte offset of the block within the tensor payload.
    pub offset: u64,
    /// SHA-256 of the block bytes.
    pub sha256: String,
}

/// An approximate (sampled) fingerprint for one tensor.
#[derive(Debug, Clone)]
pub struct ApproxFingerprint {
    pub tensor_id: String,
    pub name: String,
    pub encoding: String,
    pub shape: Vec<u64>,
    pub element_count: u64,
    pub payload_length: Option<u64>,
    /// Evenly spaced sampled block digests; empty when the extent is
    /// unresolved (structure-only record).
    pub blocks: Vec<SampledBlock>,
    pub method: String,
}

impl ApproxFingerprint {
    /// Structural descriptor equality: same shape and encoding.
    pub fn same_structure(&self, other: &ApproxFingerprint) -> bool {
        self.shape == other.shape && self.encoding == other.encoding
    }

    /// Sampled-block similarity in `[0, 1]`: the fraction of sampled blocks
    /// (by position) whose digests agree. `None` when either side has no
    /// samples (unresolved extents) or the sample positions differ.
    pub fn similarity(&self, other: &ApproxFingerprint) -> Option<f64> {
        if self.blocks.is_empty() || other.blocks.is_empty() {
            return None;
        }
        if self.blocks.len() != other.blocks.len() {
            return None;
        }
        let matching = self
            .blocks
            .iter()
            .zip(&other.blocks)
            .filter(|(a, b)| a.offset == b.offset && a.sha256 == b.sha256)
            .count();
        Some(matching as f64 / self.blocks.len() as f64)
    }
}

/// Deterministic sample positions: evenly spaced over the payload, always
/// including the first block and, when they differ, the last block.
fn sample_positions(payload_length: u64) -> Vec<u64> {
    if payload_length == 0 {
        return Vec::new();
    }
    let block = BLOCK_BYTES.min(payload_length);
    let block_count = payload_length.div_ceil(block);
    let take = SAMPLE_COUNT.min(block_count as usize) as u64;
    let mut offsets = Vec::with_capacity(take as usize);
    for i in 0..take {
        let offset = if take == 1 {
            0
        } else {
            (i * (block_count - 1) / (take - 1)) * block
        };
        offsets.push(offset);
    }
    offsets.dedup();
    offsets
}

/// Compute approximate fingerprints for every tensor in a catalog.
pub fn approximate_fingerprints(
    catalog: &Catalog,
    budget: &Budget,
) -> Result<Vec<ApproxFingerprint>, NnError> {
    let mut records = Vec::with_capacity(catalog.tensors.len());
    for tensor in &catalog.tensors {
        let mut blocks = Vec::new();
        if let Some(length) = tensor.payload_length {
            let source = catalog.resolve_source(&tensor.source_id)?;
            let reader = BoundedFile::open(Path::new(&source.path))?;
            for offset in sample_positions(length) {
                let take = BLOCK_BYTES.min(length - offset);
                let mut buffer = vec![0u8; take as usize];
                reader.read_exact_at_bounded(tensor.payload_start + offset, &mut buffer, budget)?;
                blocks.push(SampledBlock {
                    offset,
                    sha256: hex::encode(Sha256::digest(&buffer)),
                });
                budget.checkpoint()?;
            }
        }
        records.push(ApproxFingerprint {
            tensor_id: tensor.id.clone(),
            name: tensor.original_name.clone(),
            encoding: tensor.encoding.clone(),
            shape: tensor.shape.clone(),
            element_count: tensor.element_count,
            payload_length: tensor.payload_length,
            blocks,
            method: format!("{METHOD_NAME}/{METHOD_VERSION}"),
        });
        budget.consume_generated(1)?;
    }
    Ok(records)
}

// ---- evidence graph ----

/// One typed edge between tensors of two catalogs.
#[derive(Debug, Clone)]
pub struct EvidenceEdge {
    pub kind: &'static str,
    pub tensor: String,
    pub left_id: String,
    pub right_id: String,
    /// Similarity score for `similar_under_mapping` edges.
    pub score: Option<f64>,
    pub threshold: Option<f64>,
    pub method: String,
    pub note: String,
}

/// The graph between two catalogs.
pub struct EvidenceGraph {
    pub left_catalog_id: String,
    pub right_catalog_id: String,
    pub edges: Vec<EvidenceEdge>,
}

impl EvidenceGraph {
    /// Build the evidence graph: exact payload matches from exact
    /// fingerprints, structural matches, and approximate similarity edges
    /// under the given threshold. Approximate edges require structural
    /// equality and a score at or above the threshold; every edge carries
    /// its method and the sampled-block limitation note.
    pub fn build(
        left: &Catalog,
        right: &Catalog,
        threshold: f64,
        budget: &Budget,
    ) -> Result<EvidenceGraph, NnError> {
        if !(0.0..=1.0).contains(&threshold) {
            return Err(NnError::InvalidRequest {
                message: format!("threshold {threshold} must be within [0, 1]"),
            });
        }
        let left_exact = super::compare::fingerprints(left, budget)?;
        let right_exact = super::compare::fingerprints(right, budget)?;
        let left_approx = approximate_fingerprints(left, budget)?;
        let right_approx = approximate_fingerprints(right, budget)?;

        let exact_by_name = |records: &Vec<super::compare::Fingerprint>| {
            records
                .iter()
                .map(|r| (r.name.clone(), r.payload_sha256.clone()))
                .collect::<std::collections::BTreeMap<_, _>>()
        };
        let (left_exact_map, right_exact_map) =
            (exact_by_name(&left_exact), exact_by_name(&right_exact));
        let approx_by_name = |records: &Vec<ApproxFingerprint>| {
            records
                .iter()
                .map(|r| (r.name.clone(), r.clone()))
                .collect::<std::collections::BTreeMap<_, _>>()
        };
        let (left_approx_map, right_approx_map) =
            (approx_by_name(&left_approx), approx_by_name(&right_approx));

        let mut edges = Vec::new();
        for (name, left_record) in &left_approx_map {
            let Some(right_record) = right_approx_map.get(name) else {
                continue;
            };
            if let (Some(lsha), Some(rsha)) = (left_exact_map.get(name), right_exact_map.get(name))
            {
                if lsha == rsha {
                    edges.push(EvidenceEdge {
                        kind: "exact_payload_match",
                        tensor: name.clone(),
                        left_id: left_record.tensor_id.clone(),
                        right_id: right_record.tensor_id.clone(),
                        score: None,
                        threshold: None,
                        method: "sha256/exact-span".to_string(),
                        note: "complete payload bytes identical".to_string(),
                    });
                    continue;
                }
            }
            if !left_record.same_structure(right_record) {
                continue;
            }
            edges.push(EvidenceEdge {
                kind: "same_structure",
                tensor: name.clone(),
                left_id: left_record.tensor_id.clone(),
                right_id: right_record.tensor_id.clone(),
                score: None,
                threshold: None,
                method: "shape+encoding".to_string(),
                note: "identical shape and encoding; payload differs".to_string(),
            });
            if let Some(score) = left_record.similarity(right_record) {
                if score >= threshold {
                    edges.push(EvidenceEdge {
                        kind: "similar_under_mapping",
                        tensor: name.clone(),
                        left_id: left_record.tensor_id.clone(),
                        right_id: right_record.tensor_id.clone(),
                        score: Some(score),
                        threshold: Some(threshold),
                        method: format!("{METHOD_NAME}/{METHOD_VERSION}"),
                        note: format!(
                            "sampled-block similarity over {} blocks; unsampled bytes are NOT certified — verify with exact fingerprints before any consequential use",
                            left_record.blocks.len()
                        ),
                    });
                }
            }
        }

        Ok(EvidenceGraph {
            left_catalog_id: left.id()?,
            right_catalog_id: right.id()?,
            edges,
        })
    }

    pub fn envelope(&self) -> Result<ResultEnvelope, NnError> {
        let edges = self
            .edges
            .iter()
            .map(|e| {
                let mut pairs = vec![
                    ("kind", Json::Str(e.kind.to_string())),
                    ("tensor", Json::Str(e.tensor.clone())),
                    ("left_id", Json::Str(e.left_id.clone())),
                    ("right_id", Json::Str(e.right_id.clone())),
                    ("method", Json::Str(e.method.clone())),
                ];
                if let Some(score) = e.score {
                    pairs.push(("score", Json::Str(format!("{score:.4}"))));
                }
                if let Some(threshold) = e.threshold {
                    pairs.push(("threshold", Json::Str(format!("{threshold:.4}"))));
                }
                pairs.push(("note", Json::Str(e.note.clone())));
                Json::object(pairs)
            })
            .collect::<Result<Vec<_>, _>>()?;
        let count = |kind: &str| {
            self.edges
                .iter()
                .filter(|e| e.kind == kind)
                .count()
                .to_string()
        };
        let semantic = Json::object(vec![
            ("left_catalog_id", Json::Str(self.left_catalog_id.clone())),
            ("right_catalog_id", Json::Str(self.right_catalog_id.clone())),
            ("edges", Json::Array(edges)),
            (
                "counts",
                Json::object(vec![
                    ("exact_payload_match", Json::Str(count("exact_payload_match"))),
                    ("same_structure", Json::Str(count("same_structure"))),
                    ("similar_under_mapping", Json::Str(count("similar_under_mapping"))),
                ])?,
            ),
            (
                "status",
                Json::Str("experimental".to_string()),
            ),
            (
                "claims",
                Json::Str(
                    "edges assert byte-level relationships only; direction, chronology, and lineage are NOT claimed"
                        .to_string(),
                ),
            ),
        ])?;
        Ok(ResultEnvelope::new("fingerprint compare").with_semantic(semantic))
    }

    pub fn text(&self) -> String {
        let mut out = String::from("fingerprint evidence graph (experimental)\n");
        if self.edges.is_empty() {
            out.push_str("  no aligned tensors with evidence edges\n");
        }
        for edge in &self.edges {
            let score = edge
                .score
                .map(|s| {
                    format!(
                        " score {s:.4} (threshold {:.4})",
                        edge.threshold.unwrap_or(0.0)
                    )
                })
                .unwrap_or_default();
            out.push_str(&format!("  {} {}{}\n", edge.tensor, edge.kind, score));
        }
        out.push_str(
            "  claims: edges assert byte-level relationships only; direction, chronology, and lineage are NOT claimed\n",
        );
        out.push_str(
            "  status: experimental — sampled-block similarity does not certify unsampled bytes\n",
        );
        out
    }
}

// ---- synthetic-corpus false-match analysis ----

/// Outcome of one corpus evaluation.
#[derive(Debug, Clone, PartialEq)]
pub struct CorpusReport {
    pub related_pairs: usize,
    pub unrelated_pairs: usize,
    /// Related pairs scoring at or above the threshold.
    pub related_matched: usize,
    /// Unrelated pairs scoring at or above the threshold (false matches).
    pub unrelated_matched: usize,
    pub threshold: f64,
    pub method: String,
    pub sample_count: usize,
    pub corpus: String,
}

impl CorpusReport {
    pub fn false_match_rate(&self) -> f64 {
        if self.unrelated_pairs == 0 {
            return 0.0;
        }
        self.unrelated_matched as f64 / self.unrelated_pairs as f64
    }

    pub fn recall(&self) -> f64 {
        if self.related_pairs == 0 {
            return 0.0;
        }
        self.related_matched as f64 / self.related_pairs as f64
    }

    /// Run the false-match analysis over an in-memory synthetic corpus:
    /// `related` tensors differ from their partners in exactly one block;
    /// `unrelated` tensors are independent pseudo-random bytes. Contents are
    /// generated deterministically from the pair index (no RNG, no wall
    /// time), so the corpus is reproducible.
    pub fn evaluate_synthetic(pairs: usize, payload_length: u64, threshold: f64) -> CorpusReport {
        let mut related_matched = 0;
        let mut unrelated_matched = 0;
        for i in 0..pairs {
            let related = build_pair(i as u64, payload_length, true);
            let unrelated = build_pair(i as u64, payload_length, false);
            let (left, right_related) = related;
            let (_, right_unrelated) = unrelated;
            if left
                .similarity(&right_related)
                .is_some_and(|s| s >= threshold)
            {
                related_matched += 1;
            }
            if left
                .similarity(&right_unrelated)
                .is_some_and(|s| s >= threshold)
            {
                unrelated_matched += 1;
            }
        }
        CorpusReport {
            related_pairs: pairs,
            unrelated_pairs: pairs,
            related_matched,
            unrelated_matched,
            threshold,
            method: format!("{METHOD_NAME}/{METHOD_VERSION}"),
            sample_count: SAMPLE_COUNT,
            corpus: format!(
                "synthetic/deterministic: {pairs} related pairs (one block perturbed) + {pairs} unrelated pairs (independent bytes), {payload_length}-byte payloads"
            ),
        }
    }

    pub fn text(&self) -> String {
        format!(
            "false-match analysis ({}): related {}/{} matched (recall {:.4}), unrelated {}/{} matched (false-match rate {:.4}) at threshold {:.4} over {} samples",
            self.method,
            self.related_matched,
            self.related_pairs,
            self.recall(),
            self.unrelated_matched,
            self.unrelated_pairs,
            self.false_match_rate(),
            self.threshold,
            self.sample_count,
        )
    }
}

/// Deterministic pseudo-random byte stream (SplitMix64-driven).
fn synthetic_bytes(seed: u64, length: u64) -> Vec<u8> {
    let mut out = vec![0u8; length as usize];
    let mut state = seed | 1;
    for chunk in out.chunks_mut(8) {
        state = state.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = state;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^= z >> 31;
        for (i, byte) in chunk.iter_mut().enumerate() {
            *byte = (z >> (i * 8)) as u8;
        }
    }
    out
}

/// Build a (left, right) pair: related perturbs one sampled block; unrelated
/// uses fully independent bytes.
fn build_pair(index: u64, length: u64, related: bool) -> (ApproxFingerprint, ApproxFingerprint) {
    let left_bytes = synthetic_bytes(index, length);
    let right_bytes = if related {
        let mut bytes = left_bytes.clone();
        // Perturb bytes in the LAST sampled block region.
        let last = length.saturating_sub(1) as usize;
        bytes[last] ^= 0xFF;
        bytes
    } else {
        synthetic_bytes(index ^ 0xFFFF_0000, length)
    };
    let fingerprint = |bytes: &[u8]| ApproxFingerprint {
        tensor_id: "tensor:synthetic".to_string(),
        name: "synthetic".to_string(),
        encoding: "synthetic".to_string(),
        shape: vec![bytes.len() as u64],
        element_count: bytes.len() as u64,
        payload_length: Some(bytes.len() as u64),
        blocks: sample_positions(bytes.len() as u64)
            .into_iter()
            .map(|offset| {
                let take = BLOCK_BYTES.min(bytes.len() as u64 - offset) as usize;
                SampledBlock {
                    offset,
                    sha256: hex::encode(Sha256::digest(
                        &bytes[offset as usize..offset as usize + take],
                    )),
                }
            })
            .collect(),
        method: format!("{METHOD_NAME}/{METHOD_VERSION}"),
    };
    (fingerprint(&left_bytes), fingerprint(&right_bytes))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sample_positions_are_deterministic_and_spread() {
        // Tiny payload: one block at 0.
        assert_eq!(sample_positions(100), vec![0]);
        // Two-block payload: first and last blocks.
        assert_eq!(sample_positions(BLOCK_BYTES * 2), vec![0, BLOCK_BYTES]);
        // Many blocks: SAMPLE_COUNT positions, first and last included.
        let positions = sample_positions(BLOCK_BYTES * 100);
        assert_eq!(positions.len(), SAMPLE_COUNT);
        assert_eq!(positions[0], 0);
        assert_eq!(positions.last().copied(), Some(BLOCK_BYTES * 99));
        // Monotonic.
        assert!(positions.windows(2).all(|w| w[0] < w[1]));
        // Zero payload: no samples.
        assert!(sample_positions(0).is_empty());
    }

    #[test]
    fn similarity_counts_matching_blocks_only() {
        let block = |offset: u64, sha: &str| SampledBlock {
            offset,
            sha256: sha.to_string(),
        };
        let left = ApproxFingerprint {
            tensor_id: "a".into(),
            name: "t".into(),
            encoding: "e".into(),
            shape: vec![8],
            element_count: 8,
            payload_length: Some(8 * BLOCK_BYTES),
            blocks: vec![
                block(0, "aa"),
                block(BLOCK_BYTES, "bb"),
                block(BLOCK_BYTES * 2, "cc"),
                block(BLOCK_BYTES * 3, "dd"),
            ],
            method: "m".into(),
        };
        let mut right = left.clone();
        right.blocks[2].sha256 = "xx".to_string();
        assert_eq!(left.similarity(&right), Some(0.75));
        assert_eq!(left.similarity(&left), Some(1.0));
        // Offset mismatch invalidates comparison.
        let mut shifted = left.clone();
        shifted.blocks[1].offset = 999;
        assert_eq!(left.similarity(&shifted), Some(0.75));
        // Different sample counts: None.
        let mut shorter = left.clone();
        shorter.blocks.pop();
        assert_eq!(left.similarity(&shorter), None);
        // Empty samples: None (unresolved extents).
        let mut empty = left.clone();
        empty.blocks.clear();
        assert_eq!(left.similarity(&empty), None);
        // Structure check independent of blocks.
        let mut other_shape = left.clone();
        other_shape.shape = vec![4];
        assert!(!left.same_structure(&other_shape));
    }

    /// Documented synthetic corpus: at the default threshold the method must
    /// recall all one-block-perturbation pairs and produce zero false
    /// matches on independent-byte pairs.
    #[test]
    fn synthetic_corpus_false_match_analysis() {
        let report = CorpusReport::evaluate_synthetic(200, BLOCK_BYTES * 8, DEFAULT_THRESHOLD);
        // 8 blocks, one perturbed: score 7/8 = 0.875 ≥ 0.75 for every
        // related pair.
        assert_eq!(report.related_matched, report.related_pairs);
        assert_eq!(report.recall(), 1.0);
        // Independent bytes: SHA-256 block collisions do not occur.
        assert_eq!(report.unrelated_matched, 0);
        assert_eq!(report.false_match_rate(), 0.0);
        assert_eq!(report.sample_count, SAMPLE_COUNT);
        assert!(report.corpus.starts_with("synthetic/deterministic"));
        assert!(report.text().contains("false-match rate 0.0000"));
    }

    /// Documented limitation: a difference confined to UNSAMPLED bytes
    /// scores 1.0 — sampled fingerprints never certify unsampled content.
    #[test]
    fn sampled_limitation_is_demonstrated() {
        // Payload of 9 blocks; sample positions cover 8 of them. Perturb a
        // byte at an unsampled offset.
        let length = BLOCK_BYTES * 9;
        let left_bytes = synthetic_bytes(42, length);
        let mut right_bytes = left_bytes.clone();
        let positions = sample_positions(length);
        // With 9 blocks and 8 samples the positions cover blocks
        // 0,1,2,3,4,5,6,8 — block 7 is the unsampled one.
        let unsampled_offset = BLOCK_BYTES * 7;
        assert!(
            !positions.contains(&unsampled_offset),
            "fixture must perturb an unsampled block"
        );
        right_bytes[unsampled_offset as usize] ^= 0xFF;
        let fingerprint = |bytes: &[u8]| ApproxFingerprint {
            tensor_id: "t".into(),
            name: "t".into(),
            encoding: "e".into(),
            shape: vec![bytes.len() as u64],
            element_count: 0,
            payload_length: Some(bytes.len() as u64),
            blocks: sample_positions(bytes.len() as u64)
                .into_iter()
                .map(|offset| {
                    let take = BLOCK_BYTES.min(bytes.len() as u64 - offset) as usize;
                    SampledBlock {
                        offset,
                        sha256: hex::encode(Sha256::digest(
                            &bytes[offset as usize..offset as usize + take],
                        )),
                    }
                })
                .collect(),
            method: format!("{METHOD_NAME}/{METHOD_VERSION}"),
        };
        let (left, right) = (fingerprint(&left_bytes), fingerprint(&right_bytes));
        assert_eq!(left.similarity(&right), Some(1.0));
    }
}
