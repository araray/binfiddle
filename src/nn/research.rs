//! Transformation-aware similarity research: row-permutation alignment.
//!
//! Weight matrices are often permuted copies of each other (neuron/head/expert
//! reordering — the symmetry family Git Re-Basin studies). This module tests
//! the cleanest static instance: whether one stored 2-axis weight is a row
//! permutation of another, decided exactly by comparing per-row SHA-256
//! multisets and recovering the mapping. A recovered permutation is evidence
//! about the stored bytes; it is NOT a claim that the computation is
//! equivalent — that requires the paired input/output permutations, which no
//! static scan can establish. Duplicate rows make the mapping ambiguous (many
//! valid permutations); the ambiguity is reported, not resolved by guesswork.
//!
//! The method is qualified against a documented deterministic synthetic
//! corpus (same pattern as the approximate-fingerprint corpus): detection
//! rate and false-match rate are measured in tests, never asserted.

use super::budget::Budget;
use super::catalog::{Catalog, CatalogTensor};
use super::codec::{layout_for_encoding, TensorLayout};
use super::error::NnError;
use super::json::Json;
use super::report::ResultEnvelope;
use super::source::BoundedFile;
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::path::Path;

/// Method identity.
pub const METHOD_NAME: &str = "row-permutation-multiset";
pub const METHOD_VERSION: &str = "v1";

/// Per-row digests of one 2-axis weight.
#[derive(Debug, Clone, PartialEq)]
pub struct RowDigests {
    pub rows: u64,
    pub cols: u64,
    /// SHA-256 of each row's stored bytes, row-major order.
    pub digests: Vec<String>,
}

impl RowDigests {
    /// Hash every row of a dense scalar-encoded 2-axis tensor.
    pub fn of_tensor(
        catalog: &Catalog,
        tensor: &CatalogTensor,
        budget: &Budget,
    ) -> Result<Option<RowDigests>, NnError> {
        if tensor.shape.len() != 2 {
            return Ok(None);
        }
        let width = match layout_for_encoding(&tensor.encoding) {
            TensorLayout::Scalar(codec) => codec.width(),
            _ => return Ok(None),
        };
        let Some(length) = tensor.payload_length else {
            return Ok(None);
        };
        let (rows, cols) = (tensor.shape[0], tensor.shape[1]);
        let source = catalog.resolve_source(&tensor.source_id)?;
        let reader = BoundedFile::open(Path::new(&source.path))?;
        let row_bytes = cols
            .checked_mul(width)
            .ok_or_else(|| NnError::MalformedInput {
                detail: "row byte size overflows u64".to_string(),
            })?;
        if length != rows * row_bytes {
            // Not a dense row-major payload under this codec.
            return Ok(None);
        }
        let mut buffer = vec![0u8; row_bytes as usize];
        let mut digests = Vec::with_capacity(rows as usize);
        for row in 0..rows {
            reader.read_exact_at_bounded(
                tensor.payload_start + row * row_bytes,
                &mut buffer,
                budget,
            )?;
            digests.push(hex::encode(Sha256::digest(&buffer)));
            budget.checkpoint()?;
        }
        Ok(Some(RowDigests {
            rows,
            cols,
            digests,
        }))
    }

    /// Try to recover the permutation mapping `left row i -> right row` when
    /// the left is a row permutation of the right. Returns `Ok(None)` when
    /// the multisets differ (no permutation exists).
    pub fn permutation_to(&self, right: &RowDigests) -> Result<Option<Permutation>, NnError> {
        if self.rows != right.rows || self.cols != right.cols {
            return Ok(None);
        }
        let mut left_sorted = self.digests.clone();
        left_sorted.sort();
        let mut right_sorted = right.digests.clone();
        right_sorted.sort();
        if left_sorted != right_sorted {
            return Ok(None);
        }
        // Map each left row to the right rows with the same digest.
        let mut right_by_digest: BTreeMap<&str, Vec<u64>> = BTreeMap::new();
        for (index, digest) in right.digests.iter().enumerate() {
            right_by_digest
                .entry(digest.as_str())
                .or_default()
                .push(index as u64);
        }
        let mut mapping = Vec::with_capacity(self.digests.len());
        let mut ambiguous_rows = 0u64;
        // Greedy assignment: each right row is used exactly once; duplicate
        // digests get deterministic lowest-free-index assignment and are
        // counted ambiguous (any assignment among equals is valid).
        let mut used = vec![false; right.digests.len()];
        for digest in &self.digests {
            let Some(candidates) = right_by_digest.get(digest.as_str()) else {
                return Err(NnError::MalformedInput {
                    detail: "internal: digest present in sorted multiset but not the map"
                        .to_string(),
                });
            };
            let chosen = candidates
                .iter()
                .copied()
                .find(|&i| !used[i as usize])
                .ok_or_else(|| NnError::MalformedInput {
                    detail: "internal: multiset equal but assignment exhausted".to_string(),
                })?;
            used[chosen as usize] = true;
            if candidates.len() > 1 {
                ambiguous_rows += 1;
            }
            mapping.push(chosen);
        }
        Ok(Some(Permutation {
            mapping,
            ambiguous_rows,
        }))
    }
}

/// A recovered row mapping with its ambiguity count.
#[derive(Debug, Clone, PartialEq)]
pub struct Permutation {
    /// `mapping[i]` = right-row index that left row `i` equals.
    pub mapping: Vec<u64>,
    /// Rows whose digest appears more than once (multiple valid targets).
    pub ambiguous_rows: u64,
}

/// One alignment result for a tensor pair.
#[derive(Debug, Clone)]
pub struct AlignmentResult {
    pub tensor: String,
    pub outcome: &'static str, // "permutation" | "identical" | "not_permutation" | "ineligible"
    pub permuted_rows: u64,
    pub fixed_rows: u64,
    pub ambiguous_rows: u64,
    pub method: String,
}

/// The complete alignment report between two catalogs.
pub struct AlignmentReport {
    pub left_catalog_id: String,
    pub right_catalog_id: String,
    pub results: Vec<AlignmentResult>,
}

impl AlignmentReport {
    /// Test row-permutation alignment for every name-aligned, same-shape,
    /// same-encoding, dense 2-axis tensor pair.
    pub fn align(
        left: &Catalog,
        right: &Catalog,
        budget: &Budget,
    ) -> Result<AlignmentReport, NnError> {
        let right_by_name: BTreeMap<&str, &CatalogTensor> = right
            .tensors
            .iter()
            .map(|t| (t.original_name.as_str(), t))
            .collect();
        let mut results = Vec::new();
        for lt in &left.tensors {
            let Some(rt) = right_by_name.get(lt.original_name.as_str()) else {
                continue;
            };
            if lt.shape != rt.shape || lt.encoding != rt.encoding {
                continue;
            }
            let (Some(ld), Some(rd)) = (
                RowDigests::of_tensor(left, lt, budget)?,
                RowDigests::of_tensor(right, rt, budget)?,
            ) else {
                results.push(AlignmentResult {
                    tensor: lt.original_name.clone(),
                    outcome: "ineligible",
                    permuted_rows: 0,
                    fixed_rows: 0,
                    ambiguous_rows: 0,
                    method: format!("{METHOD_NAME}/{METHOD_VERSION}"),
                });
                continue;
            };
            match ld.permutation_to(&rd)? {
                Some(permutation) => {
                    let identity = permutation
                        .mapping
                        .iter()
                        .enumerate()
                        .all(|(i, &m)| m == i as u64);
                    let fixed = permutation
                        .mapping
                        .iter()
                        .enumerate()
                        .filter(|(i, &m)| m == *i as u64)
                        .count() as u64;
                    results.push(AlignmentResult {
                        tensor: lt.original_name.clone(),
                        outcome: if identity { "identical" } else { "permutation" },
                        permuted_rows: ld.rows - fixed,
                        fixed_rows: fixed,
                        ambiguous_rows: permutation.ambiguous_rows,
                        method: format!("{METHOD_NAME}/{METHOD_VERSION}"),
                    });
                }
                None => results.push(AlignmentResult {
                    tensor: lt.original_name.clone(),
                    outcome: "not_permutation",
                    permuted_rows: 0,
                    fixed_rows: 0,
                    ambiguous_rows: 0,
                    method: format!("{METHOD_NAME}/{METHOD_VERSION}"),
                }),
            }
        }
        Ok(AlignmentReport {
            left_catalog_id: left.id()?,
            right_catalog_id: right.id()?,
            results,
        })
    }

    pub fn envelope(&self) -> Result<ResultEnvelope, NnError> {
        let items = self
            .results
            .iter()
            .map(|r| {
                Json::object(vec![
                    ("tensor", Json::Str(r.tensor.clone())),
                    ("outcome", Json::Str(r.outcome.to_string())),
                    ("permuted_rows", Json::Str(r.permuted_rows.to_string())),
                    ("fixed_rows", Json::Str(r.fixed_rows.to_string())),
                    ("ambiguous_rows", Json::Str(r.ambiguous_rows.to_string())),
                    ("method", Json::Str(r.method.clone())),
                ])
            })
            .collect::<Result<Vec<_>, _>>()?;
        let count = |outcome: &str| {
            self.results
                .iter()
                .filter(|r| r.outcome == outcome)
                .count()
                .to_string()
        };
        let semantic = Json::object(vec![
            ("left_catalog_id", Json::Str(self.left_catalog_id.clone())),
            ("right_catalog_id", Json::Str(self.right_catalog_id.clone())),
            ("results", Json::Array(items)),
            (
                "counts",
                Json::object(vec![
                    ("permutation", Json::Str(count("permutation"))),
                    ("identical", Json::Str(count("identical"))),
                    ("not_permutation", Json::Str(count("not_permutation"))),
                    ("ineligible", Json::Str(count("ineligible"))),
                ])?,
            ),
            (
                "status",
                Json::Str("experimental".to_string()),
            ),
            (
                "claims",
                Json::Str(
                    "a recovered permutation is evidence about stored bytes; computational equivalence needs the paired input/output permutations and is NOT claimed"
                        .to_string(),
                ),
            ),
        ])?;
        Ok(ResultEnvelope::new("research align").with_semantic(semantic))
    }

    pub fn text(&self) -> String {
        let mut out = String::from("row-permutation alignment (experimental)\n");
        if self.results.is_empty() {
            out.push_str("  no eligible same-shape dense 2-axis tensor pairs\n");
        }
        for result in &self.results {
            match result.outcome {
                "permutation" => out.push_str(&format!(
                    "  {}: permutation ({} rows moved, {} fixed, {} ambiguous rows)\n",
                    result.tensor, result.permuted_rows, result.fixed_rows, result.ambiguous_rows
                )),
                "identical" => out.push_str(&format!(
                    "  {}: identical rows (identity permutation; {} ambiguous rows)\n",
                    result.tensor, result.ambiguous_rows
                )),
                "not_permutation" => {
                    out.push_str(&format!("  {}: not a row permutation\n", result.tensor))
                }
                _ => out.push_str(&format!(
                    "  {}: ineligible (not dense 2-axis)\n",
                    result.tensor
                )),
            }
        }
        out.push_str(
            "  claims: a recovered permutation is evidence about stored bytes; computational equivalence is NOT claimed\n",
        );
        out
    }
}

// ---- documented synthetic corpus ----

/// Corpus evaluation outcome.
#[derive(Debug, Clone, PartialEq)]
pub struct AlignCorpusReport {
    pub pairs: usize,
    pub detected: usize,
    pub false_matches: usize,
    pub method: String,
    pub corpus: String,
}

impl AlignCorpusReport {
    /// Deterministic synthetic corpus: `pairs` permuted copies (seeded
    /// rotations of row order — always non-identity) and `pairs` independent
    /// matrices of the same shape. Detection = permutation found for the
    /// permuted copies; false match = permutation found for independent
    /// content.
    pub fn evaluate_synthetic(pairs: usize, rows: u64, cols: u64) -> AlignCorpusReport {
        let mut detected = 0;
        let mut false_matches = 0;
        for i in 0..pairs {
            let left = synthetic_digests(i as u64, rows, cols);
            // Permuted copy: rotate rows by (i % rows) + 1 — always a real
            // permutation, never identity.
            let shift = (i as u64 % rows) + 1;
            let mut right = left.clone();
            right.digests = (0..rows)
                .map(|r| left.digests[((r + shift) % rows) as usize].clone())
                .collect();
            if left.permutation_to(&right).is_ok_and(|p| p.is_some()) {
                detected += 1;
            }
            // Independent content: distinct bytes per row.
            let independent = synthetic_digests(i as u64 ^ 0x5A5A_0000, rows, cols);
            let unrelated = RowDigests {
                rows,
                cols,
                digests: independent
                    .digests
                    .iter()
                    .map(|d| {
                        // Perturb deterministically so no digest can collide.
                        let mut hasher = Sha256::new();
                        hasher.update(d.as_bytes());
                        hasher.update([0xAB]);
                        hex::encode(hasher.finalize())
                    })
                    .collect(),
            };
            if left.permutation_to(&unrelated).is_ok_and(|p| p.is_some()) {
                false_matches += 1;
            }
        }
        AlignCorpusReport {
            pairs,
            detected,
            false_matches,
            method: format!("{METHOD_NAME}/{METHOD_VERSION}"),
            corpus: format!(
                "synthetic/deterministic: {pairs} rotated copies + {pairs} independent matrices ({rows}x{cols})"
            ),
        }
    }

    pub fn detection_rate(&self) -> f64 {
        if self.pairs == 0 {
            0.0
        } else {
            self.detected as f64 / self.pairs as f64
        }
    }

    pub fn false_match_rate(&self) -> f64 {
        if self.pairs == 0 {
            0.0
        } else {
            self.false_matches as f64 / self.pairs as f64
        }
    }
}

/// Deterministic distinct row digests (one unique digest per row).
fn synthetic_digests(seed: u64, rows: u64, cols: u64) -> RowDigests {
    let mut digests = Vec::with_capacity(rows as usize);
    for r in 0..rows {
        let mut hasher = Sha256::new();
        hasher.update(seed.to_le_bytes());
        hasher.update(r.to_le_bytes());
        hasher.update(cols.to_le_bytes());
        digests.push(hex::encode(hasher.finalize()));
    }
    RowDigests {
        rows,
        cols,
        digests,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn digests(rows: u64, cols: u64, tags: &[&str]) -> RowDigests {
        RowDigests {
            rows,
            cols,
            digests: tags.iter().map(|t| t.to_string()).collect(),
        }
    }

    #[test]
    fn permutation_recovery_and_ambiguity() {
        // Left rows [a,b,c], right rows [b,c,a]: left row 0 -> right 1...
        let left = digests(3, 4, &["a", "b", "c"]);
        let right = digests(3, 4, &["b", "c", "a"]);
        let permutation = left.permutation_to(&right).unwrap().unwrap();
        assert_eq!(permutation.mapping, vec![2, 0, 1]); // a@2, b@0, c@1
        assert_eq!(permutation.ambiguous_rows, 0);

        // Duplicate rows: mapping exists but is ambiguous.
        let left_dup = digests(3, 4, &["a", "a", "b"]);
        let right_dup = digests(3, 4, &["a", "b", "a"]);
        let permutation = left_dup.permutation_to(&right_dup).unwrap().unwrap();
        assert_eq!(permutation.ambiguous_rows, 2);

        // Different multiset: no permutation.
        let other = digests(3, 4, &["a", "b", "z"]);
        assert!(left.permutation_to(&other).unwrap().is_none());
        // Different shapes: no permutation.
        let smaller = digests(2, 4, &["a", "b"]);
        assert!(left.permutation_to(&smaller).unwrap().is_none());
    }

    /// Documented synthetic corpus: rotations detected 100%, independent
    /// content never matches.
    #[test]
    fn synthetic_corpus_detection_and_false_matches() {
        let report = AlignCorpusReport::evaluate_synthetic(100, 8, 16);
        assert_eq!(report.detected, report.pairs);
        assert_eq!(report.detection_rate(), 1.0);
        assert_eq!(report.false_matches, 0);
        assert_eq!(report.false_match_rate(), 0.0);
        assert!(report.corpus.starts_with("synthetic/deterministic"));
    }
}
