//! Structural recipe: MLP channel pruning.
//!
//! Removes selected intermediate channels from every MLP in one SafeTensors
//! source: gate and up projections lose the channels' rows, the down
//! projection loses the matching columns, and every other tensor is copied
//! byte-identically. The output is a freshly written SafeTensors file with
//! updated shapes; validation reparses the output and verifies every
//! untouched payload by digest. The recipe applies uniformly across all MLP
//! components because frameworks commonly require a common intermediate
//! width — the receipt states this scope, and behavior is explicitly not
//! evaluated.

use super::budget::Budget;
use super::catalog::Catalog;
use super::error::NnError;
use super::json::Json;
use super::packs::{BindingKind, Recognition};
use super::source::BoundedFile;
use sha2::{Digest, Sha256};
use std::path::Path;

/// One touched tensor record for the receipt.
#[derive(Debug, Clone)]
pub struct PruneChange {
    pub tensor_name: String,
    pub old_shape: Vec<u64>,
    pub new_shape: Vec<u64>,
}

/// Receipt for one applied prune.
pub struct PruneReceipt {
    pub channels: Vec<u64>,
    pub changes: Vec<PruneChange>,
    pub untouched_tensor_count: usize,
    pub output_path: std::path::PathBuf,
    pub output_digest: String,
}

impl PruneReceipt {
    pub fn text(&self) -> String {
        let mut out = String::from("mlp channel prune applied\n");
        let channels = self
            .channels
            .iter()
            .map(|c| c.to_string())
            .collect::<Vec<_>>()
            .join(",");
        out.push_str(&format!("  channels removed: {channels}\n"));
        for change in &self.changes {
            out.push_str(&format!(
                "  {}: {:?} -> {:?}\n",
                change.tensor_name, change.old_shape, change.new_shape
            ));
        }
        out.push_str(&format!(
            "  untouched tensors: {} (payloads digest-verified)\n",
            self.untouched_tensor_count
        ));
        out.push_str(&format!(
            "  output: {} ({})\n",
            self.output_path.display(),
            &self.output_digest[..16.min(self.output_digest.len())]
        ));
        out
    }

    pub fn envelope(&self) -> Result<super::report::ResultEnvelope, NnError> {
        let changes = self
            .changes
            .iter()
            .map(|c| {
                Json::object(vec![
                    ("tensor", Json::Str(c.tensor_name.clone())),
                    (
                        "old_shape",
                        Json::Array(
                            c.old_shape
                                .iter()
                                .map(|d| Json::Str(d.to_string()))
                                .collect(),
                        ),
                    ),
                    (
                        "new_shape",
                        Json::Array(
                            c.new_shape
                                .iter()
                                .map(|d| Json::Str(d.to_string()))
                                .collect(),
                        ),
                    ),
                ])
            })
            .collect::<Result<Vec<_>, _>>()?;
        let semantic = Json::object(vec![
            (
                "channels",
                Json::Array(
                    self.channels
                        .iter()
                        .map(|c| Json::Str(c.to_string()))
                        .collect(),
                ),
            ),
            ("changes", Json::Array(changes)),
            (
                "untouched_tensors",
                Json::Str(self.untouched_tensor_count.to_string()),
            ),
            ("output", Json::Str(self.output_path.display().to_string())),
            ("output_digest", Json::Str(self.output_digest.clone())),
            (
                "guarantees",
                Json::object(vec![
                    ("claim", Json::Str("structural channel removal".to_string())),
                    ("untouched_payloads", Json::Bool(true)),
                    ("behavior", Json::Str("not evaluated".to_string())),
                ])?,
            ),
        ])?;
        Ok(super::report::ResultEnvelope::new("edit prune").with_semantic(semantic))
    }
}

/// The MLP tensor triple groups found in a recognition.
struct MlpTensors<'a> {
    gates: Vec<&'a super::catalog::CatalogTensor>,
    ups: Vec<&'a super::catalog::CatalogTensor>,
    downs: Vec<&'a super::catalog::CatalogTensor>,
}

fn mlp_tensors<'a>(catalog: &'a Catalog, recognition: &Recognition) -> MlpTensors<'a> {
    let mut groups = MlpTensors {
        gates: Vec::new(),
        ups: Vec::new(),
        downs: Vec::new(),
    };
    for component in &recognition.components {
        let Some(tensor) = catalog
            .tensors
            .iter()
            .find(|t| component.tensor_ids.contains(&t.id))
        else {
            continue;
        };
        match component.kind {
            BindingKind::MlpGate => groups.gates.push(tensor),
            BindingKind::MlpUp => groups.ups.push(tensor),
            BindingKind::MlpDown => groups.downs.push(tensor),
            _ => {}
        }
    }
    groups
}

/// Validate channel indices against the shared intermediate width; returns
/// the planned shape changes and the touched tensor names.
pub fn validate_prune(
    catalog: &Catalog,
    recognition: &Recognition,
    channels: &[u64],
) -> Result<(Vec<PruneChange>, Vec<String>), NnError> {
    let groups = mlp_tensors(catalog, recognition);
    if groups.gates.is_empty() || groups.ups.is_empty() || groups.downs.is_empty() {
        return Err(NnError::InvalidRequest {
            message:
                "the pack must recognize at least one mlp_gate, mlp_up, and mlp_down component"
                    .to_string(),
        });
    }
    let m = groups.gates[0].shape.first().copied().unwrap_or(0);
    for tensor in groups.gates.iter().chain(&groups.ups) {
        if tensor.shape.first().copied().unwrap_or(0) != m {
            return Err(NnError::InvalidRequest {
                message: format!(
                    "gate/up tensors must share one intermediate width; {} has first dim {} (expected {m})",
                    tensor.original_name,
                    tensor.shape.first().copied().unwrap_or(0)
                ),
            });
        }
    }
    for tensor in &groups.downs {
        if tensor.shape.get(1).copied().unwrap_or(0) != m {
            return Err(NnError::InvalidRequest {
                message: format!(
                    "down tensor {} has second dim {} but the intermediate width is {m}",
                    tensor.original_name,
                    tensor.shape.get(1).copied().unwrap_or(0)
                ),
            });
        }
    }
    if channels.is_empty() {
        return Err(NnError::InvalidRequest {
            message: "at least one channel index is required".to_string(),
        });
    }
    let mut sorted = channels.to_vec();
    sorted.sort_unstable();
    sorted.dedup();
    if sorted != channels {
        return Err(NnError::InvalidRequest {
            message: "channel indices must be sorted and unique".to_string(),
        });
    }
    for &channel in channels {
        if channel >= m {
            return Err(NnError::InvalidRequest {
                message: format!("channel {channel} is out of range (intermediate width {m})"),
            });
        }
    }
    if channels.len() as u64 >= m {
        return Err(NnError::InvalidRequest {
            message: format!("cannot remove all {m} channels; at least one must remain"),
        });
    }

    let mut changes = Vec::new();
    let mut touched = Vec::new();
    for tensor in groups.gates.iter().chain(&groups.ups).chain(&groups.downs) {
        let mut new_shape = tensor.shape.clone();
        if groups.gates.iter().any(|g| g.id == tensor.id)
            || groups.ups.iter().any(|u| u.id == tensor.id)
        {
            new_shape[0] -= channels.len() as u64;
        } else {
            new_shape[1] -= channels.len() as u64;
        }
        touched.push(tensor.original_name.clone());
        changes.push(PruneChange {
            tensor_name: tensor.original_name.clone(),
            old_shape: tensor.shape.clone(),
            new_shape,
        });
    }
    Ok((changes, touched))
}

/// Apply the prune: rewrite touched tensors, copy the rest byte-identically,
/// write a fresh SafeTensors output, and validate it.
pub fn apply_prune(
    catalog: &Catalog,
    recognition: &Recognition,
    channels: &[u64],
    out_path: &Path,
    budget: &Budget,
) -> Result<PruneReceipt, NnError> {
    let (changes, touched) = validate_prune(catalog, recognition, channels)?;
    let groups = mlp_tensors(catalog, recognition);
    let m = groups.gates[0].shape.first().copied().unwrap_or(0);
    let kept: Vec<u64> = (0..m).filter(|i| !channels.contains(i)).collect();

    // Single SafeTensors source only.
    let source = catalog
        .sources
        .first()
        .ok_or_else(|| NnError::InvalidRequest {
            message: "catalog has no sources".to_string(),
        })?;
    if catalog.sources.iter().any(|s| s.id != source.id) {
        return Err(NnError::InvalidRequest {
            message: "the prune recipe operates on single-source SafeTensors catalogs".to_string(),
        });
    }
    if !source
        .format
        .as_deref()
        .unwrap_or("")
        .starts_with("safetensors")
    {
        return Err(NnError::InvalidRequest {
            message: "the prune recipe writes SafeTensors outputs; this source is not SafeTensors"
                .to_string(),
        });
    }
    let reader = BoundedFile::open(Path::new(&source.path))?;
    reader.verify_length(Path::new(&source.path))?;

    if out_path.exists() {
        return Err(NnError::InvalidRequest {
            message: format!(
                "output {} already exists; recipes write fresh files",
                out_path.display()
            ),
        });
    }

    // Element width per tensor (scalar encodings only).
    let width_of = |tensor: &super::catalog::CatalogTensor| -> Result<u64, NnError> {
        match crate::nn::codec::layout_for_encoding(&tensor.encoding) {
            crate::nn::codec::TensorLayout::Scalar(codec) => Ok(codec.width()),
            _ => Err(NnError::CodecUnsupported {
                codec: tensor.encoding.clone(),
                operation: "prune rewrite".to_string(),
                reason: "recipe rewrites need a scalar encoding".to_string(),
            }),
        }
    };
    let dtype_of = |tensor: &super::catalog::CatalogTensor| -> String {
        tensor
            .encoding
            .strip_prefix("safetensors.")
            .unwrap_or(&tensor.encoding)
            .to_string()
    };

    // Build the new tensor set in catalog order (header order = file order).
    struct NewTensor {
        name: String,
        dtype: String,
        shape: Vec<u64>,
        bytes: Vec<u8>,
        touched: bool,
    }
    let mut new_tensors: Vec<NewTensor> = Vec::new();
    for tensor in &catalog.tensors {
        let Some(length) = tensor.payload_length else {
            return Err(NnError::InvalidRequest {
                message: format!(
                    "tensor {} has a bounded extent; recipes require exact extents",
                    tensor.original_name
                ),
            });
        };
        let mut payload = vec![0u8; length as usize];
        reader.read_exact_at_bounded(tensor.payload_start, &mut payload, budget)?;
        if touched.contains(&tensor.original_name) {
            let width = width_of(tensor)?;
            let dtype = dtype_of(tensor);
            let is_gate_up = groups.gates.iter().any(|g| g.id == tensor.id)
                || groups.ups.iter().any(|u| u.id == tensor.id);
            let (shape, bytes) = if is_gate_up {
                // Channels are rows (axis 0).
                let cols = tensor.shape.get(1).copied().unwrap_or(1);
                let row_bytes = (cols * width) as usize;
                let mut bytes = Vec::with_capacity(kept.len() * row_bytes);
                for row in &kept {
                    let start = (*row as usize) * row_bytes;
                    bytes.extend_from_slice(&payload[start..start + row_bytes]);
                }
                (vec![kept.len() as u64, cols], bytes)
            } else {
                // Channels are columns (axis 1): keep per-row elements.
                let rows = tensor.shape[0];
                let cols = tensor.shape[1];
                let w = width as usize;
                let mut bytes = Vec::with_capacity(rows as usize * kept.len() * w);
                for r in 0..rows {
                    for c in &kept {
                        let offset = ((r * cols + c) as usize) * w;
                        bytes.extend_from_slice(&payload[offset..offset + w]);
                    }
                }
                (vec![rows, kept.len() as u64], bytes)
            };
            budget.consume_output(bytes.len() as u64)?;
            new_tensors.push(NewTensor {
                name: tensor.original_name.clone(),
                dtype,
                shape,
                bytes,
                touched: true,
            });
        } else {
            new_tensors.push(NewTensor {
                name: tensor.original_name.clone(),
                dtype: dtype_of(tensor),
                shape: tensor.shape.clone(),
                bytes: payload,
                touched: false,
            });
        }
        budget.checkpoint()?;
    }

    // Serialize: compute offsets, write header, then payloads.
    let mut offsets: Vec<(u64, u64)> = Vec::with_capacity(new_tensors.len());
    let mut cursor: u64 = 0;
    for tensor in &new_tensors {
        let begin = cursor;
        cursor += tensor.bytes.len() as u64;
        offsets.push((begin, cursor));
    }
    let mut header = String::from("{");
    for (i, tensor) in new_tensors.iter().enumerate() {
        if i > 0 {
            header.push(',');
        }
        let dims: Vec<String> = tensor.shape.iter().map(|d| d.to_string()).collect();
        header.push_str(&format!(
            "\"{}\":{{\"dtype\":\"{}\",\"shape\":[{}],\"data_offsets\":[{},{}]}}",
            tensor.name,
            tensor.dtype,
            dims.join(","),
            offsets[i].0,
            offsets[i].1
        ));
    }
    header.push('}');
    let mut buffer = Vec::with_capacity(8 + header.len() + cursor as usize);
    buffer.extend_from_slice(&(header.len() as u64).to_le_bytes());
    buffer.extend_from_slice(header.as_bytes());
    for tensor in &new_tensors {
        buffer.extend_from_slice(&tensor.bytes);
    }
    budget.consume_output(buffer.len() as u64)?;
    std::fs::write(out_path, &buffer).map_err(NnError::Io)?;
    let output_digest = hex::encode(Sha256::digest(&buffer));

    // Validation: reparse the output; verify shapes and untouched payloads.
    let out_reader = BoundedFile::open(out_path)?;
    let inventory = super::format::safetensors::inventory(&out_reader, budget)?;
    if inventory.validity != super::format::Validity::Valid
        || inventory.tensors.len() != new_tensors.len()
    {
        return Err(NnError::ValidationFailed {
            detail: "the pruned output failed container revalidation".to_string(),
        });
    }
    for tensor in &new_tensors {
        let out_tensor = inventory
            .tensors
            .iter()
            .find(|t| t.original_name == tensor.name)
            .ok_or_else(|| NnError::ValidationFailed {
                detail: format!("tensor {} missing from the output", tensor.name),
            })?;
        if out_tensor.shape != tensor.shape {
            return Err(NnError::ValidationFailed {
                detail: format!(
                    "tensor {} has shape {:?} in the output but {:?} was planned",
                    tensor.name, out_tensor.shape, tensor.shape
                ),
            });
        }
        if !tensor.touched {
            let mut out_payload = vec![0u8; tensor.bytes.len()];
            out_reader.read_exact_at_bounded(out_tensor.payload_start, &mut out_payload, budget)?;
            if Sha256::digest(&out_payload) != Sha256::digest(&tensor.bytes) {
                return Err(NnError::ValidationFailed {
                    detail: format!(
                        "untouched tensor {} changed during the rewrite",
                        tensor.name
                    ),
                });
            }
        }
    }

    let untouched = new_tensors.iter().filter(|t| !t.touched).count();
    Ok(PruneReceipt {
        channels: channels.to_vec(),
        changes,
        untouched_tensor_count: untouched,
        output_path: out_path.to_path_buf(),
        output_digest,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prune_receipt_renders() {
        let receipt = PruneReceipt {
            channels: vec![1, 3],
            changes: vec![PruneChange {
                tensor_name: "mlp.gate".to_string(),
                old_shape: vec![6, 4],
                new_shape: vec![4, 4],
            }],
            untouched_tensor_count: 2,
            output_path: std::path::PathBuf::from("out.safetensors"),
            output_digest: "0123456789abcdef".to_string(),
        };
        let text = receipt.text();
        assert!(text.contains("channels removed: 1,3"));
        assert!(text.contains("mlp.gate: [6, 4] -> [4, 4]"));
        assert!(text.contains("untouched tensors: 2"));
        let envelope = receipt.envelope().unwrap();
        let json = envelope.to_json_string().unwrap();
        assert!(json.contains("\"behavior\":\"not evaluated\""));
    }
}
