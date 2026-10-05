//! Adapter checkpoint inspection (LoRA-style low-rank factors).
//!
//! An adapter checkpoint carries per-target factor pairs: `lora_A` with
//! stored shape `[r, in]` and `lora_B` with `[out, r]`, conventionally
//! combined as `W' = W + s·B·A`. This module inventories the pairs,
//! validates that ranks agree and orientations are coherent, and keeps
//! orphans and extra tensors visible. It makes no claims about the base
//! model: descriptor compatibility is not base identity, and merge
//! arithmetic is not verified without one.

use super::catalog::Catalog;
use super::error::NnError;
use super::json::Json;
use super::report::ResultEnvelope;

/// Suffixes identifying the down (A) and up (B) factors of one target.
const FACTOR_A_SUFFIX: &str = ".lora_A.weight";
const FACTOR_B_SUFFIX: &str = ".lora_B.weight";

/// One matched factor pair for one target module.
#[derive(Debug, Clone)]
pub struct AdapterPair {
    /// Target module path (the name before the factor suffix).
    pub target: String,
    /// Rank `r` read from the stored shapes.
    pub rank: u64,
    /// Input width `in` from A's second axis.
    pub input_dim: u64,
    /// Output width `out` from B's first axis.
    pub output_dim: u64,
    pub a_name: String,
    pub b_name: String,
}

/// One inspection finding.
#[derive(Debug, Clone)]
pub struct AdapterFinding {
    pub code: &'static str,
    pub message: String,
}

/// Inspection result.
#[derive(Debug, Clone)]
pub struct AdapterReport {
    pub pairs: Vec<AdapterPair>,
    pub orphans: Vec<(String, &'static str)>,
    /// Adapter-ish tensors that are neither an A nor a B factor.
    pub extra_tensors: Vec<String>,
    pub findings: Vec<AdapterFinding>,
    /// Tensors with no adapter naming at all.
    pub non_adapter_tensors: usize,
}

impl AdapterReport {
    /// Inspect a catalog's tensors for LoRA-style factor pairs.
    pub fn inspect(catalog: &Catalog) -> Result<AdapterReport, NnError> {
        let mut a_factors: Vec<(String, String)> = Vec::new(); // (target, tensor name)
        let mut b_factors: Vec<(String, String)> = Vec::new();
        let mut extras: Vec<String> = Vec::new();
        let mut non_adapter = 0usize;

        for tensor in &catalog.tensors {
            let name = &tensor.original_name;
            if let Some(target) = name.strip_suffix(FACTOR_A_SUFFIX) {
                a_factors.push((target.to_string(), name.clone()));
            } else if let Some(target) = name.strip_suffix(FACTOR_B_SUFFIX) {
                b_factors.push((target.to_string(), name.clone()));
            } else if name.contains("lora_") {
                extras.push(name.clone());
            } else {
                non_adapter += 1;
            }
        }

        let mut findings = Vec::new();
        let mut pairs = Vec::new();
        let mut paired_a: Vec<String> = Vec::new();
        let mut paired_b: Vec<String> = Vec::new();

        for (target, a_name) in &a_factors {
            let Some((_, b_name)) = b_factors.iter().find(|(t, _)| t == target) else {
                continue;
            };
            paired_a.push(a_name.clone());
            paired_b.push(b_name.clone());
            let a_tensor = catalog
                .tensors
                .iter()
                .find(|t| t.original_name == *a_name)
                .expect("names come from the catalog");
            let b_tensor = catalog
                .tensors
                .iter()
                .find(|t| t.original_name == *b_name)
                .expect("names come from the catalog");
            // A: [r, in]; B: [out, r].
            if a_tensor.shape.len() != 2 || b_tensor.shape.len() != 2 {
                findings.push(AdapterFinding {
                    code: "FACTOR_NOT_MATRIX",
                    message: format!(
                        "target {target}: A shape {:?} / B shape {:?}; factors must be 2-axis",
                        a_tensor.shape, b_tensor.shape
                    ),
                });
                continue;
            }
            let (rank_a, input_dim) = (a_tensor.shape[0], a_tensor.shape[1]);
            let (output_dim, rank_b) = (b_tensor.shape[0], b_tensor.shape[1]);
            if rank_a != rank_b {
                findings.push(AdapterFinding {
                    code: "RANK_MISMATCH",
                    message: format!("target {target}: A rank {rank_a} != B rank {rank_b}"),
                });
                continue;
            }
            pairs.push(AdapterPair {
                target: target.clone(),
                rank: rank_a,
                input_dim,
                output_dim,
                a_name: a_name.clone(),
                b_name: b_name.clone(),
            });
        }

        // Orphans: factors whose counterpart is missing.
        let mut orphans = Vec::new();
        for (target, name) in &a_factors {
            if !paired_a.contains(name) {
                orphans.push((name.clone(), "A"));
                findings.push(AdapterFinding {
                    code: "ORPHAN_FACTOR",
                    message: format!("target {target}: lora_A has no matching lora_B"),
                });
            }
        }
        for (target, name) in &b_factors {
            if !paired_b.contains(name) {
                orphans.push((name.clone(), "B"));
                findings.push(AdapterFinding {
                    code: "ORPHAN_FACTOR",
                    message: format!("target {target}: lora_B has no matching lora_A"),
                });
            }
        }
        if !extras.is_empty() {
            findings.push(AdapterFinding {
                code: "EXTRA_ADAPTER_TENSORS",
                message: format!(
                    "{} tensors carry lora_ naming but are not A/B factors",
                    extras.len()
                ),
            });
        }
        if pairs.is_empty() {
            findings.push(AdapterFinding {
                code: "NO_FACTOR_PAIRS",
                message: "no complete lora_A/lora_B pairs found; this does not look like a LoRA-style adapter checkpoint"
                    .to_string(),
            });
        }

        Ok(AdapterReport {
            pairs,
            orphans,
            extra_tensors: extras,
            findings,
            non_adapter_tensors: non_adapter,
        })
    }

    pub fn envelope(&self, catalog: &Catalog) -> Result<ResultEnvelope, NnError> {
        let pairs = self
            .pairs
            .iter()
            .map(|p| {
                Json::object(vec![
                    ("target", Json::Str(p.target.clone())),
                    ("rank", Json::Str(p.rank.to_string())),
                    ("input_dim", Json::Str(p.input_dim.to_string())),
                    ("output_dim", Json::Str(p.output_dim.to_string())),
                    ("a", Json::Str(p.a_name.clone())),
                    ("b", Json::Str(p.b_name.clone())),
                ])
            })
            .collect::<Result<Vec<_>, _>>()?;
        let orphans = self
            .orphans
            .iter()
            .map(|(name, side)| {
                Json::object(vec![
                    ("tensor", Json::Str(name.clone())),
                    ("side", Json::Str(side.to_string())),
                ])
            })
            .collect::<Result<Vec<_>, _>>()?;
        let findings = self
            .findings
            .iter()
            .map(|f| {
                Json::object(vec![
                    ("code", Json::Str(f.code.to_string())),
                    ("message", Json::Str(f.message.clone())),
                ])
            })
            .collect::<Result<Vec<_>, _>>()?;
        let semantic = Json::object(vec![
            ("catalog_id", Json::Str(catalog.id()?)),
            ("kind", Json::Str("lora_style_factors".to_string())),
            ("pair_count", Json::Str(self.pairs.len().to_string())),
            ("pairs", Json::Array(pairs)),
            ("orphans", Json::Array(orphans)),
            (
                "extra_tensors",
                Json::Array(
                    self.extra_tensors.iter().cloned().map(Json::Str).collect(),
                ),
            ),
            (
                "non_adapter_tensors",
                Json::Str(self.non_adapter_tensors.to_string()),
            ),
            ("findings", Json::Array(findings)),
            (
                "claims",
                Json::Str(
                    "descriptor-level factor inspection; merge arithmetic and base-model compatibility are NOT verified (no base model is involved)"
                        .to_string(),
                ),
            ),
        ])?;
        Ok(ResultEnvelope::new("adapter inspect").with_semantic(semantic))
    }

    pub fn text(&self) -> String {
        let mut out = String::from("adapter inspection (LoRA-style factors)\n");
        if self.pairs.is_empty() {
            out.push_str("  no complete factor pairs found\n");
        }
        for pair in &self.pairs {
            out.push_str(&format!(
                "  {} rank {} [{} -> {}]\n",
                pair.target, pair.rank, pair.input_dim, pair.output_dim
            ));
        }
        for (name, side) in &self.orphans {
            out.push_str(&format!("  orphan {side} factor: {name}\n"));
        }
        for name in &self.extra_tensors {
            out.push_str(&format!("  extra adapter tensor: {name}\n"));
        }
        for finding in &self.findings {
            out.push_str(&format!(
                "  finding [{}]: {}\n",
                finding.code, finding.message
            ));
        }
        out.push_str(
            "  claims: descriptor-level factor inspection; merge arithmetic and base-model compatibility are NOT verified\n",
        );
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::nn::budget::{Budget, BudgetCaps};
    use crate::nn::cancel::CancellationToken;
    use crate::nn::catalog::Catalog;
    use crate::nn::discover::{discover, DiscoverOptions};
    use std::path::Path;

    fn catalog_with_tensors(dir: &Path, file: &str, tensors: &[(&str, Vec<u64>)]) -> Catalog {
        let mut body: Vec<u8> = Vec::new();
        let mut spans = Vec::new();
        for (name, shape) in tensors {
            let begin = body.len();
            let bytes: usize = shape.iter().product::<u64>() as usize * 4;
            body.extend(std::iter::repeat_n(0u8, bytes));
            spans.push((name, shape, begin, body.len()));
        }
        let entries: Vec<String> = spans
            .iter()
            .map(|(n, sh, b, e)| {
                let dims: Vec<String> = sh.iter().map(|d| d.to_string()).collect();
                format!(
                    "\"{n}\":{{\"dtype\":\"F32\",\"shape\":[{}],\"data_offsets\":[{b},{e}]}}",
                    dims.join(",")
                )
            })
            .collect();
        let header = format!("{{{}}}", entries.join(","));
        let mut data = Vec::new();
        data.extend_from_slice(&(header.len() as u64).to_le_bytes());
        data.extend_from_slice(header.as_bytes());
        data.extend_from_slice(&body);
        std::fs::write(dir.join(file), data).unwrap();
        let budget = Budget::new(BudgetCaps::default(), None, CancellationToken::new());
        let report = discover(&dir.join(file), &DiscoverOptions::default(), &budget).unwrap();
        Catalog::from_discovery(&report).unwrap()
    }

    #[test]
    fn pairs_orphans_and_rank_mismatches() {
        let dir = tempfile::tempdir().unwrap();
        let catalog = catalog_with_tensors(
            dir.path(),
            "adapter.safetensors",
            &[
                ("m1.lora_A.weight", vec![4, 8]),
                ("m1.lora_B.weight", vec![6, 4]),
                ("m2.lora_A.weight", vec![4, 8]),
                ("m2.lora_B.weight", vec![6, 5]), // rank mismatch
                ("m3.lora_B.weight", vec![6, 4]), // orphan B
                ("m4.lora_alpha", vec![1]),       // extra lora_ tensor
                ("base.weight", vec![2, 2]),      // non-adapter
            ],
        );
        let report = AdapterReport::inspect(&catalog).unwrap();
        assert_eq!(report.pairs.len(), 1);
        assert_eq!(report.pairs[0].target, "m1");
        assert_eq!(report.pairs[0].rank, 4);
        assert_eq!(report.pairs[0].input_dim, 8);
        assert_eq!(report.pairs[0].output_dim, 6);
        assert_eq!(report.orphans.len(), 1);
        assert_eq!(report.orphans[0].0, "m3.lora_B.weight");
        assert_eq!(report.extra_tensors.len(), 1);
        assert_eq!(report.non_adapter_tensors, 1);
        let codes: Vec<&str> = report.findings.iter().map(|f| f.code).collect();
        assert!(codes.contains(&"RANK_MISMATCH"));
        assert!(codes.contains(&"ORPHAN_FACTOR"));
        assert!(codes.contains(&"EXTRA_ADAPTER_TENSORS"));
        let text = report.text();
        assert!(text.contains("m1 rank 4 [8 -> 6]"));
        assert!(text.contains("NOT verified"));
    }

    #[test]
    fn non_adapter_checkpoints_report_honestly() {
        let dir = tempfile::tempdir().unwrap();
        let catalog = catalog_with_tensors(dir.path(), "model.safetensors", &[("w", vec![2, 2])]);
        let report = AdapterReport::inspect(&catalog).unwrap();
        assert!(report.pairs.is_empty());
        assert!(report.findings.iter().any(|f| f.code == "NO_FACTOR_PAIRS"));
    }
}
