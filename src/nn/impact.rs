//! Impact analysis: what a byte span or an edit plan touches.
//!
//! `nn impact` answers the Part 04 question precisely: which tensors own
//! these bytes, what must be read to interpret them (decode dependencies),
//! and which logical values *may* change if these bytes change (the
//! numerical influence set). The three answers are different sets — a
//! nibble edit reads the shared scale but influences one element; a scale
//! edit reads nothing extra yet influences the whole block. The report
//! states structural and encoding dependencies only: behavioral
//! consequences ("will change model quality") are explicitly out of scope.

use super::address::dense_element_offset;
use super::budget::Budget;
use super::catalog::Catalog;
use super::codec::{layout_for_encoding, TensorLayout};
use super::error::NnError;
use super::json::Json;
use super::report::ResultEnvelope;
use std::path::Path;

/// One element whose decoded value may change.
#[derive(Debug, Clone)]
pub struct InfluencedElement {
    /// Linear element index within the owning tensor.
    pub linear: u64,
    /// The influence reason.
    pub reason: &'static str, // "direct_write" | "shared_scale"
}

/// Impact of a span on one owning tensor.
#[derive(Debug, Clone)]
pub struct OwnerImpact {
    pub tensor: String,
    pub tensor_id: String,
    pub encoding: String,
    /// Elements whose decode requires bytes in the span.
    pub decode_dependencies: Vec<InfluencedElement>,
    /// Elements whose decoded values may change when the span changes.
    pub influence_set: Vec<InfluencedElement>,
    pub note: String,
}

/// The complete impact report.
pub struct ImpactReport {
    pub catalog_id: String,
    pub span: (u64, u64),
    pub owners: Vec<OwnerImpact>,
    pub unowned: bool,
}

impl ImpactReport {
    /// Analyze a file-qualified byte span `[start, end)`.
    pub fn for_span(
        catalog: &Catalog,
        start: u64,
        end: u64,
        budget: &Budget,
    ) -> Result<ImpactReport, NnError> {
        if end <= start {
            return Err(NnError::InvalidRequest {
                message: format!("span [{start}, {end}) must have end after start"),
            });
        }
        let mut owners = Vec::new();
        let mut owned = false;
        for tensor in &catalog.tensors {
            let Some(length) = tensor.payload_length else {
                continue;
            };
            let t_start = tensor.payload_start;
            let t_end = t_start
                .checked_add(length)
                .ok_or_else(|| NnError::MalformedInput {
                    detail: format!("tensor {} extent overflows u64", tensor.original_name),
                })?;
            if start >= t_end || end <= t_start {
                continue;
            }
            owned = true;
            let overlap_start = start.max(t_start) - t_start;
            let overlap_end = (end.min(t_end) - t_start).min(length);
            owners.push(analyze_overlap(
                catalog,
                tensor,
                overlap_start,
                overlap_end,
                budget,
            )?);
        }
        Ok(ImpactReport {
            catalog_id: catalog.id()?,
            span: (start, end),
            owners,
            unowned: !owned,
        })
    }

    /// Analyze the write unit of a saved edit plan.
    pub fn for_edit_plan(
        catalog: &Catalog,
        plan_path: &Path,
        budget: &Budget,
    ) -> Result<ImpactReport, NnError> {
        let plan = super::edit::EditPlan::load(plan_path)?;
        let (start, length) = (
            plan.span.0,
            if plan.bit_field.is_some() {
                1
            } else {
                plan.span.1
            },
        );
        let mut report = Self::for_span(catalog, start, start + length, budget)?;
        // The plan's own decode dependencies are authoritative context.
        for dep in &plan.decode_dependencies {
            for owner in &mut report.owners {
                if owner.tensor == plan.tensor_name {
                    owner.note = format!(
                        "{}; plan decode dependencies cover [{}, {})",
                        owner.note,
                        dep.0,
                        dep.0 + dep.1
                    );
                }
            }
        }
        Ok(report)
    }

    pub fn envelope(&self) -> Result<ResultEnvelope, NnError> {
        let owners = self
            .owners
            .iter()
            .map(|o| {
                let elements = |items: &[InfluencedElement]| -> Result<Json, NnError> {
                    Ok(Json::Array(
                        items
                            .iter()
                            .map(|e| {
                                Json::object(vec![
                                    ("linear", Json::Str(e.linear.to_string())),
                                    ("reason", Json::Str(e.reason.to_string())),
                                ])
                            })
                            .collect::<Result<Vec<_>, _>>()?,
                    ))
                };
                Json::object(vec![
                    ("tensor", Json::Str(o.tensor.clone())),
                    ("tensor_id", Json::Str(o.tensor_id.clone())),
                    ("encoding", Json::Str(o.encoding.clone())),
                    ("decode_dependencies", elements(&o.decode_dependencies)?),
                    ("influence_set", elements(&o.influence_set)?),
                    ("note", Json::Str(o.note.clone())),
                ])
            })
            .collect::<Result<Vec<_>, _>>()?;
        let semantic = Json::object(vec![
            ("catalog_id", Json::Str(self.catalog_id.clone())),
            (
                "span",
                Json::Array(vec![
                    Json::Str(self.span.0.to_string()),
                    Json::Str(self.span.1.to_string()),
                ]),
            ),
            ("owners", Json::Array(owners)),
            (
                "unowned",
                Json::Bool(self.unowned),
            ),
            (
                "claims",
                Json::Str(
                    "structural and encoding dependencies only; behavioral consequences are NOT predicted"
                        .to_string(),
                ),
            ),
        ])?;
        Ok(ResultEnvelope::new("impact").with_semantic(semantic))
    }

    pub fn text(&self) -> String {
        let mut out = format!("impact of [{}, {})\n", self.span.0, self.span.1);
        if self.unowned {
            out.push_str("  no exact-extent tensor owns this span (padding/metadata)\n");
        }
        for owner in &self.owners {
            out.push_str(&format!(
                "  {} ({}): {} decode deps, {} influenced elements — {}\n",
                owner.tensor,
                owner.encoding,
                owner.decode_dependencies.len(),
                owner.influence_set.len(),
                owner.note
            ));
        }
        out.push_str(
            "  claims: structural and encoding dependencies only; behavioral consequences are NOT predicted\n",
        );
        out
    }
}

/// Compute dependency/influence sets for the overlap `[o_start, o_end)` of a
/// tensor's payload.
fn analyze_overlap(
    _catalog: &Catalog,
    tensor: &super::catalog::CatalogTensor,
    o_start: u64,
    o_end: u64,
    budget: &Budget,
) -> Result<OwnerImpact, NnError> {
    let layout = layout_for_encoding(&tensor.encoding);
    match layout {
        TensorLayout::Scalar(codec) => {
            let width = codec.width();
            let first = o_start / width;
            let last = (o_end - 1) / width;
            let mut deps = Vec::new();
            let mut influence = Vec::new();
            for linear in first..=last {
                deps.push(InfluencedElement {
                    linear,
                    reason: "direct_write",
                });
                influence.push(InfluencedElement {
                    linear,
                    reason: "direct_write",
                });
            }
            Ok(OwnerImpact {
                tensor: tensor.original_name.clone(),
                tensor_id: tensor.id.clone(),
                encoding: tensor.encoding.clone(),
                decode_dependencies: deps,
                influence_set: influence,
                note: format!(
                    "dense scalar storage: each element owns its {} bytes; read and influence sets coincide",
                    width
                ),
            })
        }
        TensorLayout::Q4_0 => {
            // Block geometry: 18 bytes per 32 elements.
            const BLOCK_BYTES: u64 = 18;
            const ELEMENTS: u64 = 32;
            let first_block = o_start / BLOCK_BYTES;
            let last_block = (o_end - 1) / BLOCK_BYTES;
            let mut deps: Vec<InfluencedElement> = Vec::new();
            let mut influence: Vec<InfluencedElement> = Vec::new();
            let mut scale_blocks = 0u64;
            for block in first_block..=last_block {
                let b_start = block * BLOCK_BYTES;
                let b_end = b_start + BLOCK_BYTES;
                let scale_span = b_start..b_start + 2;
                let overlap = o_start.max(b_start)..o_end.min(b_end);
                if overlap.start < scale_span.end && overlap.end > scale_span.start {
                    // The span touches the shared scale: every element of
                    // the block depends on it and is influenced.
                    scale_blocks += 1;
                    for u in 0..ELEMENTS {
                        let linear = block * ELEMENTS + u;
                        if !deps.iter().any(|e| e.linear == linear) {
                            deps.push(InfluencedElement {
                                linear,
                                reason: "shared_scale",
                            });
                        }
                        influence.push(InfluencedElement {
                            linear,
                            reason: "shared_scale",
                        });
                    }
                } else {
                    // Code bytes: decode needs the block scale; the touched
                    // nibbles each influence exactly one element (the
                    // neighboring nibble in the same byte is preserved only
                    // under masked-write policy — flagged in the note).
                    for u in 0..ELEMENTS {
                        let linear = block * ELEMENTS + u;
                        if !deps.iter().any(|e| e.linear == linear) {
                            deps.push(InfluencedElement {
                                linear,
                                reason: "shared_scale",
                            });
                        }
                    }
                    let first_code = overlap.start.max(b_start + 2);
                    let last_code = overlap.end.min(b_end);
                    for byte in first_code..last_code {
                        let code_index = byte - (b_start + 2);
                        for (u, shift) in [(code_index, 0u64), (code_index + 16, 4u64)] {
                            let _ = shift;
                            let linear = block * ELEMENTS + u;
                            if !influence.iter().any(|e| e.linear == linear) {
                                influence.push(InfluencedElement {
                                    linear,
                                    reason: "direct_write",
                                });
                            }
                        }
                    }
                }
            }
            let note = if scale_blocks > 0 {
                format!(
                    "span touches the shared F16 scale of {} block(s): all 32 elements per block are influenced; code edits under masked-write policy leave neighbors untouched",
                    scale_blocks
                )
            } else {
                "code nibbles only: each nibble influences exactly one element (its byte-mate is preserved under masked writes); the block scale is a read dependency".to_string()
            };
            let _ = budget;
            Ok(OwnerImpact {
                tensor: tensor.original_name.clone(),
                tensor_id: tensor.id.clone(),
                encoding: tensor.encoding.clone(),
                decode_dependencies: deps,
                influence_set: influence,
                note,
            })
        }
        TensorLayout::Unknown => Ok(OwnerImpact {
            tensor: tensor.original_name.clone(),
            tensor_id: tensor.id.clone(),
            encoding: tensor.encoding.clone(),
            decode_dependencies: Vec::new(),
            influence_set: Vec::new(),
            note: "encoding has no qualified layout: physical ownership is known, element-level analysis is unavailable".to_string(),
        }),
    }
}

/// Dense coordinate helper re-exported for span→element conversions in
/// future callers (kept internal to avoid API sprawl).
#[allow(dead_code)]
fn element_offset_wrapper(
    shape: &[u64],
    index: &[u64],
    width: u64,
    start: u64,
) -> Result<u64, NnError> {
    dense_element_offset(shape, index, width, start)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::nn::budget::{Budget, BudgetCaps};
    use crate::nn::cancel::CancellationToken;
    use crate::nn::catalog::Catalog;
    use crate::nn::discover::{discover, DiscoverOptions};

    fn budget() -> Budget {
        Budget::new(BudgetCaps::default(), None, CancellationToken::new())
    }

    fn catalog_with(dir: &std::path::Path, file: &str, header: &str, body: &[u8]) -> Catalog {
        let mut data = Vec::new();
        data.extend_from_slice(&(header.len() as u64).to_le_bytes());
        data.extend_from_slice(header.as_bytes());
        data.extend_from_slice(body);
        std::fs::write(dir.join(file), data).unwrap();
        let report = discover(&dir.join(file), &DiscoverOptions::default(), &budget()).unwrap();
        Catalog::from_discovery(&report).unwrap()
    }

    #[test]
    fn scalar_span_read_and_influence_sets_coincide() {
        let dir = tempfile::tempdir().unwrap();
        let catalog = catalog_with(
            dir.path(),
            "m.safetensors",
            r#"{"w":{"dtype":"F32","shape":[2,2],"data_offsets":[0,16]}}"#,
            &[0u8; 16],
        );
        // Find w's payload start from the catalog.
        let tensor = &catalog.tensors[0];
        let start = tensor.payload_start;
        // Span covering elements 1 and 2 (bytes 4..12).
        let report = ImpactReport::for_span(&catalog, start + 4, start + 12, &budget()).unwrap();
        assert_eq!(report.owners.len(), 1);
        let owner = &report.owners[0];
        assert_eq!(owner.decode_dependencies.len(), 2);
        assert_eq!(owner.influence_set.len(), 2);
        assert!(owner.note.contains("coincide"));
        assert!(!report.unowned);
    }

    /// B.3 doctrine verified: a code-nibble span influences ONE element; a
    /// scale-byte span influences all 32 elements of the block.
    #[test]
    fn q4_0_code_vs_scale_influence() {
        let dir = tempfile::tempdir().unwrap();
        // One Q4_0 [1,32] tensor = one 18-byte block.
        let mut out = Vec::new();
        out.extend_from_slice(b"GGUF");
        out.extend_from_slice(&3u32.to_le_bytes());
        out.extend_from_slice(&1u64.to_le_bytes());
        out.extend_from_slice(&1u64.to_le_bytes());
        let key = b"general.architecture";
        out.extend_from_slice(&(key.len() as u64).to_le_bytes());
        out.extend_from_slice(key);
        out.extend_from_slice(&8u32.to_le_bytes());
        out.extend_from_slice(&1u64.to_le_bytes());
        out.push(b't');
        out.extend_from_slice(&1u64.to_le_bytes());
        out.push(b'w');
        out.extend_from_slice(&2u32.to_le_bytes());
        out.extend_from_slice(&1u64.to_le_bytes());
        out.extend_from_slice(&32u64.to_le_bytes());
        out.extend_from_slice(&2u32.to_le_bytes());
        out.extend_from_slice(&0u64.to_le_bytes());
        while out.len() % 32 != 0 {
            out.push(0);
        }
        out.extend_from_slice(&[0u8; 18]);
        std::fs::write(dir.path().join("q.gguf"), out).unwrap();
        let report = discover(
            &dir.path().join("q.gguf"),
            &DiscoverOptions::default(),
            &budget(),
        )
        .unwrap();
        let catalog = Catalog::from_discovery(&report).unwrap();
        let tensor = &catalog.tensors[0];
        let block = tensor.payload_start;

        // Scale byte: 32 influenced elements, 32 decode deps.
        let scale = ImpactReport::for_span(&catalog, block, block + 1, &budget()).unwrap();
        let owner = &scale.owners[0];
        assert_eq!(owner.influence_set.len(), 32);
        assert_eq!(owner.decode_dependencies.len(), 32);
        assert!(owner.note.contains("shared F16 scale"));

        // Code byte 0: elements 0 and 16 influenced; deps = all 32 (scale).
        let code = ImpactReport::for_span(&catalog, block + 2, block + 3, &budget()).unwrap();
        let owner = &code.owners[0];
        assert_eq!(owner.influence_set.len(), 2);
        assert!(owner
            .influence_set
            .iter()
            .any(|e| e.linear == 0 && e.reason == "direct_write"));
        assert!(owner.influence_set.iter().any(|e| e.linear == 16));
        assert_eq!(owner.decode_dependencies.len(), 32);
        assert!(owner.note.contains("exactly one element"));
    }

    #[test]
    fn unowned_span_is_reported_honestly() {
        let dir = tempfile::tempdir().unwrap();
        let catalog = catalog_with(
            dir.path(),
            "m.safetensors",
            r#"{"w":{"dtype":"U8","shape":[1],"data_offsets":[0,1]}}"#,
            &[7u8],
        );
        // Header bytes before the payload: no owner.
        let report = ImpactReport::for_span(&catalog, 0, 4, &budget()).unwrap();
        assert!(report.unowned);
        assert!(report.text().contains("no exact-extent tensor owns"));
        // Invalid spans are usage errors.
        assert!(ImpactReport::for_span(&catalog, 9, 9, &budget()).is_err());
    }
}
