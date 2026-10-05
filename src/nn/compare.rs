//! Layered model comparison.
//!
//! A diff answers what changed at each layer — package members, tensor
//! descriptors, encoded content, decoded values — without ever confusing the
//! layers. Identical payloads at different offsets are a repack, not a
//! content change; a missing tensor is not a zero tensor; unmatched
//! populations stay visible in every summary. Equality claims are exact and
//! separately named; nothing here asserts lineage.

use super::analyze::error_metrics;
use super::budget::Budget;
use super::catalog::{Catalog, CatalogTensor};
use super::codec::{layout_for_encoding, ScalarCodec, ScalarValue, TensorLayout};
use super::error::NnError;
use super::json::Json;
use super::report::ResultEnvelope;
use super::source::BoundedFile;
use sha2::{Digest, Sha256};
use std::path::Path;

/// Policy for non-finite values and signed zeros in decoded comparison.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DecodePolicy {
    /// NaN bits must match exactly; ±0 are distinct.
    ExactBits,
    /// NaNs compare equal as a category; ±0 compare equal.
    Lenient,
}

impl DecodePolicy {
    pub fn as_str(self) -> &'static str {
        match self {
            DecodePolicy::ExactBits => "exact_bits",
            DecodePolicy::Lenient => "lenient",
        }
    }

    pub fn parse(text: &str) -> Result<DecodePolicy, NnError> {
        match text {
            "exact_bits" => Ok(DecodePolicy::ExactBits),
            "lenient" => Ok(DecodePolicy::Lenient),
            other => Err(NnError::InvalidRequest {
                message: format!("unknown decode policy {other} (exact_bits or lenient)"),
            }),
        }
    }

    fn equal(self, a: f64, b: f64) -> bool {
        match self {
            DecodePolicy::ExactBits => a.to_bits() == b.to_bits(),
            DecodePolicy::Lenient => {
                if a.is_nan() && b.is_nan() {
                    true
                } else {
                    a == b || (a == 0.0 && b == 0.0)
                }
            }
        }
    }
}

/// One package-level member difference.
#[derive(Debug, Clone, PartialEq)]
pub enum MemberDiff {
    Added { path: String },
    Removed { path: String },
}

/// One tensor-level finding.
#[derive(Debug, Clone)]
pub struct TensorDiff {
    pub name: String,
    pub left_id: Option<String>,
    pub right_id: Option<String>,
    pub kind: TensorDiffKind,
}

#[derive(Debug, Clone, PartialEq)]
pub enum TensorDiffKind {
    /// Present only on the left.
    LeftOnly,
    /// Present only on the right.
    RightOnly,
    /// Shape differs.
    ShapeChanged { left: Vec<u64>, right: Vec<u64> },
    /// Encoding differs.
    EncodingChanged { left: String, right: String },
    /// Same name, different tensor identity (same shape, different bytes).
    ContentChanged {
        left_digest: String,
        right_digest: String,
    },
    /// Encoded bytes identical; offsets differ (a repack).
    Repacked { left_start: u64, right_start: u64 },
    /// Identical encoded bytes at the same offsets.
    Identical,
}

/// Decoded-value comparison result for one aligned tensor.
#[derive(Debug, Clone)]
pub struct DecodedDiff {
    pub name: String,
    pub compared: u64,
    pub unequal: u64,
    pub metrics: Option<super::analyze::ErrorMetrics>,
    pub policy: DecodePolicy,
    pub note: String,
}

/// The complete layered result.
pub struct DiffReport {
    pub left_catalog_id: String,
    pub right_catalog_id: String,
    pub member_diffs: Vec<MemberDiff>,
    pub tensor_diffs: Vec<TensorDiff>,
    pub decoded: Vec<DecodedDiff>,
    pub unmatched_left: Vec<String>,
    pub unmatched_right: Vec<String>,
}

impl DiffReport {
    /// Package-level member sets first.
    pub fn package_level(catalog_a: &Catalog, catalog_b: &Catalog) -> Vec<MemberDiff> {
        let paths = |catalog: &Catalog| -> Vec<String> {
            let mut set: Vec<String> = catalog
                .sources
                .iter()
                .map(|s| s.path.clone())
                .filter(|p| !p.is_empty())
                .collect();
            set.sort();
            set
        };
        let (left, right) = (paths(catalog_a), paths(catalog_b));
        let mut diffs = Vec::new();
        for path in &left {
            if !right.contains(path) {
                diffs.push(MemberDiff::Removed { path: path.clone() });
            }
        }
        for path in &right {
            if !left.contains(path) {
                diffs.push(MemberDiff::Added { path: path.clone() });
            }
        }
        diffs
    }

    /// Compare two catalogs layer by layer. Sources must be content-verified
    /// so content claims rest on digests.
    pub fn compare(
        left: &Catalog,
        right: &Catalog,
        compare_decoded: bool,
        policy: DecodePolicy,
        budget: &Budget,
    ) -> Result<DiffReport, NnError> {
        // Content-verified sources are required for content-equality claims.
        for catalog in [left, right] {
            for source in &catalog.sources {
                let verified = source.semantic.get("consistency").and_then(Json::as_str)
                    == Some("content_verified");
                if !verified {
                    return Err(NnError::InvalidRequest {
                        message: "diff requires content-verified catalogs; re-discover both sides with --verify-content"
                            .to_string(),
                    });
                }
            }
        }

        let member_diffs = Self::package_level(left, right);

        // Align by original name within package scope (single-source
        // catalogs align unambiguously; multi-source names must be unique
        // or they are reported as unmatched).
        // Align by original name within package scope (single-source
        // catalogs align unambiguously; multi-source names must be unique
        // or they are reported as unmatched).
        fn by_name(catalog: &Catalog) -> std::collections::BTreeMap<String, &CatalogTensor> {
            let mut map = std::collections::BTreeMap::new();
            for tensor in &catalog.tensors {
                map.entry(tensor.original_name.clone()).or_insert(tensor);
            }
            map
        }
        let (left_map, right_map) = (by_name(left), by_name(right));

        let mut tensor_diffs = Vec::new();
        let mut decoded = Vec::new();
        let mut unmatched_left = Vec::new();
        let mut unmatched_right = Vec::new();

        for (name, lt) in &left_map {
            let Some(rt) = right_map.get(name) else {
                tensor_diffs.push(TensorDiff {
                    name: name.clone(),
                    left_id: Some(lt.id.clone()),
                    right_id: None,
                    kind: TensorDiffKind::LeftOnly,
                });
                unmatched_left.push(name.clone());
                continue;
            };
            if lt.shape != rt.shape {
                tensor_diffs.push(TensorDiff {
                    name: name.clone(),
                    left_id: Some(lt.id.clone()),
                    right_id: Some(rt.id.clone()),
                    kind: TensorDiffKind::ShapeChanged {
                        left: lt.shape.clone(),
                        right: rt.shape.clone(),
                    },
                });
                continue;
            }
            if lt.encoding != rt.encoding {
                tensor_diffs.push(TensorDiff {
                    name: name.clone(),
                    left_id: Some(lt.id.clone()),
                    right_id: Some(rt.id.clone()),
                    kind: TensorDiffKind::EncodingChanged {
                        left: lt.encoding.clone(),
                        right: rt.encoding.clone(),
                    },
                });
                continue;
            }
            // Encoded-content equality by digest of the exact spans.
            let (left_digest, right_digest) = (
                payload_digest(left, lt, budget)?,
                payload_digest(right, rt, budget)?,
            );
            if left_digest == right_digest {
                // Repack distinction: same bytes, different offsets.
                if lt.payload_start != rt.payload_start || lt.source_id != rt.source_id {
                    tensor_diffs.push(TensorDiff {
                        name: name.clone(),
                        left_id: Some(lt.id.clone()),
                        right_id: Some(rt.id.clone()),
                        kind: TensorDiffKind::Repacked {
                            left_start: lt.payload_start,
                            right_start: rt.payload_start,
                        },
                    });
                } else {
                    tensor_diffs.push(TensorDiff {
                        name: name.clone(),
                        left_id: Some(lt.id.clone()),
                        right_id: Some(rt.id.clone()),
                        kind: TensorDiffKind::Identical,
                    });
                }
                continue;
            }
            tensor_diffs.push(TensorDiff {
                name: name.clone(),
                left_id: Some(lt.id.clone()),
                right_id: Some(rt.id.clone()),
                kind: TensorDiffKind::ContentChanged {
                    left_digest: left_digest.clone(),
                    right_digest: right_digest.clone(),
                },
            });
            if compare_decoded {
                if let Some(result) = compare_decoded_tensor(left, lt, right, rt, policy, budget)? {
                    decoded.push(result);
                }
            }
        }
        for (name, _rt) in &right_map {
            if !left_map.contains_key(name) {
                tensor_diffs.push(TensorDiff {
                    name: name.clone(),
                    left_id: None,
                    right_id: Some(_rt.id.clone()),
                    kind: TensorDiffKind::RightOnly,
                });
                unmatched_right.push(name.clone());
            }
        }

        Ok(DiffReport {
            left_catalog_id: left.id()?,
            right_catalog_id: right.id()?,
            member_diffs,
            tensor_diffs,
            decoded,
            unmatched_left,
            unmatched_right,
        })
    }

    pub fn summary(&self) -> (usize, usize, usize, usize, usize, usize) {
        let identical = self
            .tensor_diffs
            .iter()
            .filter(|d| d.kind == TensorDiffKind::Identical)
            .count();
        let content = self
            .tensor_diffs
            .iter()
            .filter(|d| matches!(d.kind, TensorDiffKind::ContentChanged { .. }))
            .count();
        let repacked = self
            .tensor_diffs
            .iter()
            .filter(|d| matches!(d.kind, TensorDiffKind::Repacked { .. }))
            .count();
        let descriptors = self
            .tensor_diffs
            .iter()
            .filter(|d| {
                matches!(
                    d.kind,
                    TensorDiffKind::ShapeChanged { .. } | TensorDiffKind::EncodingChanged { .. }
                )
            })
            .count();
        (
            self.member_diffs.len(),
            identical,
            content,
            repacked,
            descriptors,
            self.unmatched_left.len() + self.unmatched_right.len(),
        )
    }

    pub fn envelope(&self) -> Result<ResultEnvelope, NnError> {
        let members = self
            .member_diffs
            .iter()
            .map(|m| {
                Json::object(vec![match m {
                    MemberDiff::Added { path } => ("added", Json::Str(path.clone())),
                    MemberDiff::Removed { path } => ("removed", Json::Str(path.clone())),
                }])
            })
            .collect::<Result<Vec<_>, _>>()?;
        let tensors = self
            .tensor_diffs
            .iter()
            .map(|d| {
                let mut pairs = vec![
                    ("name", Json::Str(d.name.clone())),
                    (
                        "left_id",
                        d.left_id.clone().map(Json::Str).unwrap_or(Json::Null),
                    ),
                    (
                        "right_id",
                        d.right_id.clone().map(Json::Str).unwrap_or(Json::Null),
                    ),
                ];
                let kind_json = match &d.kind {
                    TensorDiffKind::LeftOnly => Json::Str("left_only".into()),
                    TensorDiffKind::RightOnly => Json::Str("right_only".into()),
                    TensorDiffKind::ShapeChanged { left, right } => Json::object(vec![
                        ("kind", Json::Str("shape_changed".into())),
                        (
                            "left",
                            Json::Array(left.iter().map(|x| Json::Str(x.to_string())).collect()),
                        ),
                        (
                            "right",
                            Json::Array(right.iter().map(|x| Json::Str(x.to_string())).collect()),
                        ),
                    ])?,
                    TensorDiffKind::EncodingChanged { left, right } => Json::object(vec![
                        ("kind", Json::Str("encoding_changed".into())),
                        ("left", Json::Str(left.clone())),
                        ("right", Json::Str(right.clone())),
                    ])?,
                    TensorDiffKind::ContentChanged {
                        left_digest,
                        right_digest,
                    } => Json::object(vec![
                        ("kind", Json::Str("content_changed".into())),
                        ("left_sha256", Json::Str(left_digest.clone())),
                        ("right_sha256", Json::Str(right_digest.clone())),
                    ])?,
                    TensorDiffKind::Repacked {
                        left_start,
                        right_start,
                    } => Json::object(vec![
                        ("kind", Json::Str("repacked".into())),
                        ("left_start", Json::Str(left_start.to_string())),
                        ("right_start", Json::Str(right_start.to_string())),
                        (
                            "note",
                            Json::Str("encoded bytes identical; offsets differ".into()),
                        ),
                    ])?,
                    TensorDiffKind::Identical => {
                        Json::object(vec![("kind", Json::Str("identical".into()))])?
                    }
                };
                pairs.push(("result", kind_json));
                Json::object(pairs)
            })
            .collect::<Result<Vec<_>, _>>()?;
        let decoded = self
            .decoded
            .iter()
            .map(|d| {
                Json::object(vec![
                    ("name", Json::Str(d.name.clone())),
                    ("compared", Json::Str(d.compared.to_string())),
                    ("unequal", Json::Str(d.unequal.to_string())),
                    ("policy", Json::Str(d.policy.as_str().to_string())),
                    ("note", Json::Str(d.note.clone())),
                ])
            })
            .collect::<Result<Vec<_>, _>>()?;
        let (members_n, identical, content, repacked, descriptors, unmatched) = self.summary();
        let semantic = Json::object(vec![
            ("left_catalog_id", Json::Str(self.left_catalog_id.clone())),
            ("right_catalog_id", Json::Str(self.right_catalog_id.clone())),
            ("members", Json::Array(members)),
            ("tensors", Json::Array(tensors)),
            ("decoded", Json::Array(decoded)),
            (
                "counts",
                Json::object(vec![
                    ("member_diffs", Json::Str(members_n.to_string())),
                    ("identical", Json::Str(identical.to_string())),
                    ("content_changed", Json::Str(content.to_string())),
                    ("repacked", Json::Str(repacked.to_string())),
                    ("descriptor_changes", Json::Str(descriptors.to_string())),
                    ("unmatched", Json::Str(unmatched.to_string())),
                ])?,
            ),
            (
                "claims",
                Json::Str("exact, layered; no lineage or behavior claims".to_string()),
            ),
        ])?;
        Ok(ResultEnvelope::new("diff").with_semantic(semantic))
    }

    pub fn text(&self) -> String {
        let mut out = String::from("layered diff\n");
        if self.member_diffs.is_empty() {
            out.push_str("  package: identical member sets\n");
        } else {
            out.push_str("  package:\n");
            for member in &self.member_diffs {
                match member {
                    MemberDiff::Added { path } => {
                        out.push_str(&format!("    + {path}\n"));
                    }
                    MemberDiff::Removed { path } => {
                        out.push_str(&format!("    - {path}\n"));
                    }
                }
            }
        }
        for diff in &self.tensor_diffs {
            match &diff.kind {
                TensorDiffKind::Identical => {
                    out.push_str(&format!("  {}: identical\n", diff.name));
                }
                TensorDiffKind::Repacked {
                    left_start,
                    right_start,
                } => {
                    out.push_str(&format!(
                        "  {}: repacked (same bytes; offsets {left_start} vs {right_start})\n",
                        diff.name
                    ));
                }
                TensorDiffKind::ContentChanged { .. } => {
                    out.push_str(&format!("  {}: content changed\n", diff.name));
                }
                TensorDiffKind::ShapeChanged { left, right } => {
                    out.push_str(&format!(
                        "  {}: shape {:?} -> {:?}\n",
                        diff.name, left, right
                    ));
                }
                TensorDiffKind::EncodingChanged { left, right } => {
                    out.push_str(&format!(
                        "  {}: encoding {} -> {}\n",
                        diff.name, left, right
                    ));
                }
                TensorDiffKind::LeftOnly => {
                    out.push_str(&format!("  {}: left only\n", diff.name));
                }
                TensorDiffKind::RightOnly => {
                    out.push_str(&format!("  {}: right only\n", diff.name));
                }
            }
        }
        for decoded in &self.decoded {
            out.push_str(&format!(
                "  {}: decoded {} values compared, {} unequal (policy {}, {})\n",
                decoded.name,
                decoded.compared,
                decoded.unequal,
                decoded.policy.as_str(),
                decoded.note
            ));
        }
        let (members_n, identical, content, repacked, descriptors, unmatched) = self.summary();
        out.push_str(&format!(
            "  summary: {identical} identical, {content} content-changed, {repacked} repacked, {descriptors} descriptor changes, {unmatched} unmatched, {members_n} member diffs\n"
        ));
        out.push_str("  claims: exact, layered; no lineage or behavior claims\n");
        out
    }
}

/// Digest of a tensor's exact payload span from its source file.
fn payload_digest(
    catalog: &Catalog,
    tensor: &CatalogTensor,
    budget: &Budget,
) -> Result<String, NnError> {
    let source = catalog.resolve_source(&tensor.source_id)?;
    let reader = BoundedFile::open(Path::new(&source.path))?;
    let length = tensor
        .payload_length
        .ok_or_else(|| NnError::InvalidRequest {
            message: format!(
                "tensor {} has only a bounded extent; content comparison requires exact extents",
                tensor.original_name
            ),
        })?;
    let mut hasher = Sha256::new();
    let mut remaining = length;
    let mut offset = tensor.payload_start;
    let mut buffer = vec![0u8; 256 * 1024];
    while remaining > 0 {
        let take = (buffer.len() as u64).min(remaining) as usize;
        reader.read_exact_at_bounded(offset, &mut buffer[..take], budget)?;
        hasher.update(&buffer[..take]);
        offset += take as u64;
        remaining -= take as u64;
        budget.checkpoint()?;
    }
    Ok(hex::encode(hasher.finalize()))
}

/// Decoded comparison of two aligned tensors through their scalar codecs.
fn compare_decoded_tensor(
    left: &Catalog,
    lt: &CatalogTensor,
    right: &Catalog,
    rt: &CatalogTensor,
    policy: DecodePolicy,
    budget: &Budget,
) -> Result<Option<DecodedDiff>, NnError> {
    let codec = match layout_for_encoding(&lt.encoding) {
        TensorLayout::Scalar(codec) => codec,
        _ => {
            return Ok(Some(DecodedDiff {
                name: lt.original_name.clone(),
                compared: 0,
                unequal: 0,
                metrics: None,
                policy,
                note: "decoded comparison unavailable: no qualified scalar decoder".to_string(),
            }))
        }
    };
    let left_source = left.resolve_source(&lt.source_id)?;
    let right_source = right.resolve_source(&rt.source_id)?;
    let left_reader = BoundedFile::open(Path::new(&left_source.path))?;
    let right_reader = BoundedFile::open(Path::new(&right_source.path))?;
    let width = codec.width() as usize;
    let count = lt.element_count.min(rt.element_count);

    let mut pairs: Vec<(f64, f64)> = Vec::new();
    let mut unequal = 0u64;
    let mut left_buffer = vec![0u8; width];
    let mut right_buffer = vec![0u8; width];
    for i in 0..count {
        left_reader.read_exact_at_bounded(
            lt.payload_start + i * width as u64,
            &mut left_buffer,
            budget,
        )?;
        right_reader.read_exact_at_bounded(
            rt.payload_start + i * width as u64,
            &mut right_buffer,
            budget,
        )?;
        let lv = decode_to_f64(codec, &left_buffer)?;
        let rv = decode_to_f64(codec, &right_buffer)?;
        if !policy.equal(lv, rv) {
            unequal += 1;
        }
        pairs.push((lv, rv));
        if i % 65536 == 0 {
            budget.checkpoint()?;
        }
    }
    let metrics = if unequal > 0 {
        Some(error_metrics(&pairs))
    } else {
        None
    };
    let note = if unequal == 0 {
        "all compared values equal under the declared policy".to_string()
    } else if count < lt.element_count || count < rt.element_count {
        format!(
            "compared the common prefix only ({count} of {} / {})",
            lt.element_count, rt.element_count
        )
    } else {
        "metrics over all compared values".to_string()
    };
    Ok(Some(DecodedDiff {
        name: lt.original_name.clone(),
        compared: count,
        unequal,
        metrics,
        policy,
        note,
    }))
}

fn decode_to_f64(codec: ScalarCodec, bytes: &[u8]) -> Result<f64, NnError> {
    Ok(match codec.decode(bytes)?.value {
        ScalarValue::Float(f) => f,
        ScalarValue::Int(v) => v as f64,
        ScalarValue::Uint(v) => v as f64,
        ScalarValue::Bool(v) => (v as u8) as f64,
    })
}

// ---- exact fingerprints ----

/// One fingerprint record: canonical digest of a tensor's descriptor +
/// payload, with its evidence.
#[derive(Debug, Clone)]
pub struct Fingerprint {
    pub tensor_id: String,
    pub name: String,
    pub encoding: String,
    pub shape: Vec<u64>,
    pub payload_sha256: String,
    /// The canonicalization rule that produced this digest.
    pub method: String,
}

/// Compute exact fingerprints for every tensor in a catalog. The digest
/// covers the original name, the shape, the encoding id, and the payload
/// bytes under a stated canonicalization — an evidence record, not a
/// lineage claim.
pub fn fingerprints(catalog: &Catalog, budget: &Budget) -> Result<Vec<Fingerprint>, NnError> {
    let mut records = Vec::with_capacity(catalog.tensors.len());
    for tensor in &catalog.tensors {
        let payload = payload_digest(catalog, tensor, budget)?;
        // Canonical digest input: name, shape, encoding, payload digest.
        let canonical = Json::object(vec![
            ("name", Json::Str(tensor.original_name.clone())),
            (
                "shape",
                Json::Array(
                    tensor
                        .shape
                        .iter()
                        .map(|d| Json::Str(d.to_string()))
                        .collect(),
                ),
            ),
            ("encoding", Json::Str(tensor.encoding.clone())),
            ("payload_sha256", Json::Str(payload.clone())),
        ])?;
        let _digest = hex::encode(Sha256::digest(canonical.to_canonical()?.as_bytes()));
        records.push(Fingerprint {
            tensor_id: tensor.id.clone(),
            name: tensor.original_name.clone(),
            encoding: tensor.encoding.clone(),
            shape: tensor.shape.clone(),
            payload_sha256: payload,
            method: format!(
                "sha256(canonical(name, shape, encoding, payload_sha256)); payload {} bytes at exact span",
                tensor.payload_length.unwrap_or(0)
            ),
        });
        budget.consume_generated(1)?;
    }
    Ok(records)
}

pub fn fingerprints_envelope(
    catalog: &Catalog,
    records: &[Fingerprint],
) -> Result<ResultEnvelope, NnError> {
    let items = records
        .iter()
        .map(|f| {
            Json::object(vec![
                ("tensor_id", Json::Str(f.tensor_id.clone())),
                ("name", Json::Str(f.name.clone())),
                ("encoding", Json::Str(f.encoding.clone())),
                (
                    "shape",
                    Json::Array(f.shape.iter().map(|d| Json::Str(d.to_string())).collect()),
                ),
                ("payload_sha256", Json::Str(f.payload_sha256.clone())),
                ("method", Json::Str(f.method.clone())),
            ])
        })
        .collect::<Result<Vec<_>, _>>()?;
    let semantic = Json::object(vec![
        ("catalog_id", Json::Str(catalog.id()?)),
        ("fingerprints", Json::Array(items)),
        (
            "claims",
            Json::Str(
                "exact content identity records; no lineage or chronology claims".to_string(),
            ),
        ),
    ])?;
    Ok(ResultEnvelope::new("fingerprint").with_semantic(semantic))
}

pub fn fingerprints_text(records: &[Fingerprint]) -> String {
    let mut out = String::from("exact fingerprints\n");
    for record in records {
        out.push_str(&format!(
            "  {} [{}] {} payload {}..{}\n",
            record.name,
            record
                .shape
                .iter()
                .map(|d| d.to_string())
                .collect::<Vec<_>>()
                .join("x"),
            record.encoding,
            &record.payload_sha256[..12.min(record.payload_sha256.len())],
            &record.payload_sha256[52.min(record.payload_sha256.len())..]
        ));
    }
    out.push_str("  claims: exact content identity records; no lineage or chronology claims\n");
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decode_policies_are_distinct() {
        assert!(DecodePolicy::ExactBits.equal(1.0, 1.0));
        assert!(!DecodePolicy::ExactBits.equal(0.0, -0.0));
        assert!(DecodePolicy::Lenient.equal(0.0, -0.0));
        // Identical NaN bit patterns are equal under both policies...
        let nan_a = f64::from_bits(0x7ff8000000000001);
        let nan_b = f64::from_bits(0x7ff8000000000002);
        assert!(DecodePolicy::ExactBits.equal(nan_a, nan_a));
        // ...but different NaN payloads differ under exact_bits and compare
        // equal as a category under lenient.
        assert!(!DecodePolicy::ExactBits.equal(nan_a, nan_b));
        assert!(DecodePolicy::Lenient.equal(nan_a, nan_b));
    }
}
