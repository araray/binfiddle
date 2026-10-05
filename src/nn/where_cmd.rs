//! `nn where` (coordinate → byte/bit location) and `nn locate` (file offset →
//! owning tensors).
//!
//! `where` classifies every answer: the precision field says exactly how much
//! is known (`exact_contiguous`, `exact_bits`, `no_payload`, `unresolved`),
//! the byte span and — for sub-byte storage — the bit mask and numbering are
//! stated, and decode dependencies list every span needed to interpret the
//! value, including shared parameters such as a quantization scale.

use super::address::{
    locate_offset, locate_q4_0_element, locate_scalar_element, locate_whole_tensor,
    ElementLocation, ReverseEntry, ReverseRole,
};
use super::catalog::{Catalog, CatalogTensor};
use super::codec::{layout_for_encoding, TensorLayout};
use super::error::NnError;
use super::json::Json;
use super::report::ResultEnvelope;

/// Parse a complete coordinate tuple: comma-separated decimal u64 values.
pub fn parse_coordinate(text: &str) -> Result<Vec<u64>, NnError> {
    let parts: Vec<&str> = text.split(',').collect();
    let mut index = Vec::with_capacity(parts.len());
    for part in parts {
        let trimmed = part.trim();
        if trimmed.is_empty() || !trimmed.bytes().all(|b| b.is_ascii_digit()) {
            return Err(NnError::InvalidRequest {
                message: format!(
                    "coordinate components must be decimal integers; got {}",
                    super::error::brief(text)
                ),
            });
        }
        index.push(
            trimmed
                .parse::<u64>()
                .map_err(|_| NnError::InvalidRequest {
                    message: format!(
                        "coordinate component out of u64 range: {}",
                        super::error::brief(trimmed)
                    ),
                })?,
        );
    }
    if index.is_empty() {
        return Err(NnError::InvalidRequest {
            message: "coordinate cannot be empty".to_string(),
        });
    }
    Ok(index)
}

fn location_json(location: &ElementLocation) -> Result<Json, NnError> {
    let mut pairs = vec![
        ("space", Json::Str("file".to_string())),
        (
            "precision",
            Json::Str(location.precision.as_str().to_string()),
        ),
        (
            "byte_span",
            Json::object(vec![
                ("start", Json::Str(location.byte_span.0.to_string())),
                ("length", Json::Str(location.byte_span.1.to_string())),
            ])?,
        ),
    ];
    if let Some((mask, shift)) = location.bit_mask {
        pairs.push((
            "bit_field",
            Json::object(vec![
                ("mask_hex", Json::Str(format!("{mask:02x}"))),
                ("shift", Json::Str(shift.to_string())),
                ("bit_numbering", Json::Str("lsb0".to_string())),
            ])?,
        ));
    }
    let dependencies = location
        .decode_dependencies
        .iter()
        .enumerate()
        .map(|(i, (start, length))| {
            Json::object(vec![
                ("start", Json::Str(start.to_string())),
                ("length", Json::Str(length.to_string())),
                (
                    "role",
                    Json::Str(
                        if i == 0 && location.bit_mask.is_some() {
                            "scale"
                        } else if location.bit_mask.is_some() {
                            "code"
                        } else {
                            "element"
                        }
                        .to_string(),
                    ),
                ),
            ])
        })
        .collect::<Result<Vec<_>, _>>()?;
    pairs.push(("decode_dependencies", Json::Array(dependencies)));
    Json::object(pairs)
}

/// Resolve the location of one element (with coordinate) or the whole tensor.
pub fn where_location(
    tensor: &CatalogTensor,
    coordinate: Option<&[u64]>,
) -> Result<ElementLocation, NnError> {
    let layout = layout_for_encoding(&tensor.encoding);
    match (layout, coordinate) {
        (TensorLayout::Scalar(codec), Some(index)) => {
            locate_scalar_element(&tensor.shape, index, codec, tensor.payload_start)
        }
        (TensorLayout::Q4_0, Some(index)) => {
            locate_q4_0_element(&tensor.shape, index, tensor.payload_start)
        }
        (TensorLayout::Unknown, Some(_)) => Err(NnError::CodecUnsupported {
            codec: tensor.encoding.clone(),
            operation: "address one element".to_string(),
            reason: "no qualified layout registers per-element addressing for this encoding"
                .to_string(),
        }),
        (_, None) => Ok(locate_whole_tensor(
            tensor.payload_start,
            tensor.payload_length,
        )),
    }
}

/// Build the `where` result envelope.
pub fn where_envelope(
    catalog: &Catalog,
    tensor: &CatalogTensor,
    coordinate: Option<&[u64]>,
) -> Result<ResultEnvelope, NnError> {
    let location = where_location(tensor, coordinate)?;
    let mut pairs = vec![
        ("catalog_id", Json::Str(catalog.id()?)),
        ("tensor_id", Json::Str(tensor.id.clone())),
        ("name", Json::Str(tensor.original_name.clone())),
        ("encoding", Json::Str(tensor.encoding.clone())),
    ];
    match coordinate {
        Some(index) => {
            let index_json: Vec<Json> = index.iter().map(|i| Json::Str(i.to_string())).collect();
            pairs.push(("coordinate", Json::Array(index_json)));
        }
        None => {
            pairs.push(("scope", Json::Str("whole_tensor".to_string())));
        }
    }
    pairs.push(("location", location_json(&location)?));
    let semantic = Json::object(pairs)?;
    Ok(ResultEnvelope::new("where").with_semantic(semantic))
}

/// Human-readable `where` output.
pub fn where_text(tensor: &CatalogTensor, coordinate: Option<&[u64]>) -> Result<String, NnError> {
    let location = where_location(tensor, coordinate)?;
    let mut out = String::new();
    match coordinate {
        Some(index) => {
            let text = index
                .iter()
                .map(|i| i.to_string())
                .collect::<Vec<_>>()
                .join(",");
            out.push_str(&format!(
                "{} [{}] ({})\n",
                tensor.original_name, text, tensor.encoding
            ));
        }
        None => {
            out.push_str(&format!(
                "{} (whole tensor, {})\n",
                tensor.original_name, tensor.encoding
            ));
        }
    }
    out.push_str(&format!("  precision: {}\n", location.precision.as_str()));
    out.push_str(&format!(
        "  file span: [{}, {})\n",
        location.byte_span.0,
        location.byte_span.0 + location.byte_span.1
    ));
    if let Some((mask, shift)) = location.bit_mask {
        out.push_str(&format!(
            "  bits:      mask 0x{mask:02x} shift {shift} (lsb0 within the byte)\n"
        ));
        out.push_str("  note:      the other nibble of this byte is a different logical element\n");
    }
    out.push_str("  decode dependencies:\n");
    for (start, length) in &location.decode_dependencies {
        out.push_str(&format!("    [{start}, {})\n", start + length));
    }
    out.push_str(&format!("  tensor id: {}\n", tensor.id));
    Ok(out)
}

/// Build the `locate` result envelope: every exact-extent tensor containing
/// the offset, with the owning coordinate family.
pub fn locate_envelope(catalog: &Catalog, offset: u64) -> Result<ResultEnvelope, NnError> {
    let entries: Vec<ReverseEntry> = catalog
        .tensors
        .iter()
        .map(|t| ReverseEntry {
            name: t.original_name.clone(),
            payload_start: t.payload_start,
            payload_length: t.payload_length,
            layout: layout_for_encoding(&t.encoding),
            shape: t.shape.clone(),
        })
        .collect();
    let hits = locate_offset(&entries, offset);
    let hit_records = hits
        .iter()
        .map(|hit| {
            let (role, detail) = match &hit.role {
                ReverseRole::AtStart => ("payload_start".to_string(), String::new()),
                ReverseRole::AtEnd => ("payload_end".to_string(), String::new()),
                ReverseRole::Inside { detail } => ("inside".to_string(), detail.clone()),
            };
            let mut pairs = vec![
                ("tensor", Json::Str(hit.tensor_name.clone())),
                ("role", Json::Str(role)),
                (
                    "payload",
                    Json::object(vec![
                        ("start", Json::Str(hit.payload_start.to_string())),
                        ("length", Json::Str(hit.payload_length.to_string())),
                    ])?,
                ),
            ];
            if !detail.is_empty() {
                pairs.push(("detail", Json::Str(detail)));
            }
            Json::object(pairs)
        })
        .collect::<Result<Vec<_>, _>>()?;
    let semantic = Json::object(vec![
        ("catalog_id", Json::Str(catalog.id()?)),
        ("offset", Json::Str(offset.to_string())),
        ("hits", Json::Array(hit_records)),
        ("hit_count", Json::Str(hits.len().to_string())),
        (
            "note",
            Json::Str(if hits.is_empty() {
                "no exact-extent tensor owns this offset (padding, metadata, or bounded-extent payload)"
                        .to_string()
            } else {
                String::new()
            }),
        ),
    ])?;
    Ok(ResultEnvelope::new("locate").with_semantic(semantic))
}

/// Human-readable `locate` output.
pub fn locate_text(catalog: &Catalog, offset: u64) -> Result<String, NnError> {
    let entries: Vec<ReverseEntry> = catalog
        .tensors
        .iter()
        .map(|t| ReverseEntry {
            name: t.original_name.clone(),
            payload_start: t.payload_start,
            payload_length: t.payload_length,
            layout: layout_for_encoding(&t.encoding),
            shape: t.shape.clone(),
        })
        .collect();
    let hits = locate_offset(&entries, offset);
    let mut out = format!("offset {offset} ({offset:#x}):\n");
    if hits.is_empty() {
        out.push_str("  no exact-extent tensor owns this offset\n");
    }
    for hit in &hits {
        match &hit.role {
            ReverseRole::AtStart => out.push_str(&format!(
                "  {}: payload start [{}, {})\n",
                hit.tensor_name,
                hit.payload_start,
                hit.payload_start + hit.payload_length
            )),
            ReverseRole::AtEnd => {
                out.push_str(&format!("  {}: payload end (exclusive)\n", hit.tensor_name))
            }
            ReverseRole::Inside { detail } => out.push_str(&format!(
                "  {}: {detail} (payload [{}, {}))\n",
                hit.tensor_name,
                hit.payload_start,
                hit.payload_start + hit.payload_length
            )),
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::nn::address::AddressPrecision;
    use crate::nn::budget::{Budget, BudgetCaps};
    use crate::nn::cancel::CancellationToken;
    use crate::nn::catalog::Catalog;
    use crate::nn::discover::{discover, DiscoverOptions};
    use std::path::Path;

    fn budget() -> Budget {
        Budget::new(BudgetCaps::default(), None, CancellationToken::new())
    }

    fn catalog_with_f32(dir: &Path) -> Catalog {
        // F32 [2,3] then U8 [4] in one file.
        let header = "{\"w\":{\"dtype\":\"F32\",\"shape\":[2,3],\"data_offsets\":[0,24]},\"b\":{\"dtype\":\"U8\",\"shape\":[4],\"data_offsets\":[24,28]}}";
        let mut data = Vec::new();
        data.extend_from_slice(&(header.len() as u64).to_le_bytes());
        data.extend_from_slice(header.as_bytes());
        data.extend_from_slice(&[0u8; 28]);
        std::fs::write(dir.join("m.safetensors"), data).unwrap();
        let report = discover(
            &dir.join("m.safetensors"),
            &DiscoverOptions::default(),
            &budget(),
        )
        .unwrap();
        Catalog::from_discovery(&report).unwrap()
    }

    fn catalog_with_q4(dir: &Path) -> Catalog {
        // GGUF with one Q4_0 [1,32] tensor (18-byte payload).
        let mut out = Vec::new();
        out.extend_from_slice(b"GGUF");
        out.extend_from_slice(&3u32.to_le_bytes());
        out.extend_from_slice(&1u64.to_le_bytes()); // tensors
        out.extend_from_slice(&1u64.to_le_bytes()); // metadata
        let key = "general.architecture";
        out.extend_from_slice(&(key.len() as u64).to_le_bytes());
        out.extend_from_slice(key.as_bytes());
        out.extend_from_slice(&8u32.to_le_bytes());
        out.extend_from_slice(&1u64.to_le_bytes());
        out.push(b't');
        out.extend_from_slice(&1u64.to_le_bytes()); // name length
        out.push(b'w');
        out.extend_from_slice(&2u32.to_le_bytes()); // dims
        out.extend_from_slice(&1u64.to_le_bytes()); // dim0
        out.extend_from_slice(&32u64.to_le_bytes()); // dim1
        out.extend_from_slice(&2u32.to_le_bytes()); // Q4_0
        out.extend_from_slice(&0u64.to_le_bytes()); // offset
        while out.len() % 32 != 0 {
            out.push(0);
        }
        out.extend_from_slice(&[0x00, 0x38, 0xA3]); // corpus fixture head
        out.extend(std::iter::repeat_n(0u8, 15));
        std::fs::write(dir.join("q.gguf"), &out).unwrap();
        let report = discover(&dir.join("q.gguf"), &DiscoverOptions::default(), &budget()).unwrap();
        Catalog::from_discovery(&report).unwrap()
    }

    #[test]
    fn coordinate_parsing() {
        assert_eq!(parse_coordinate("123,456").unwrap(), vec![123, 456]);
        assert_eq!(parse_coordinate("7").unwrap(), vec![7]);
        assert!(parse_coordinate("").is_err());
        assert!(parse_coordinate("1,,2").is_err());
        assert!(parse_coordinate("1,x").is_err());
        assert!(parse_coordinate("-1").is_err());
        assert!(parse_coordinate("0x10").is_err());
    }

    #[test]
    fn where_dense_element_reports_exact_contiguous() {
        let dir = tempfile::tempdir().unwrap();
        let catalog = catalog_with_f32(dir.path());
        let tensor = &catalog.tensors[0]; // w, F32 [2,3]
        let envelope = where_envelope(&catalog, tensor, Some(&[1, 2])).unwrap();
        let text = envelope.to_json_string().unwrap();
        assert!(text.contains("\"precision\":\"exact_contiguous\""));
        // linear 5 → offset data_origin + 20.
        let base = tensor.payload_start;
        assert!(text.contains(&format!("\"start\":\"{}\"", base + 20)));

        let human = where_text(tensor, Some(&[1, 2])).unwrap();
        assert!(human.contains("precision: exact_contiguous"));
        assert!(human.contains(&format!("[{}, {})", base + 20, base + 24)));
    }

    #[test]
    fn where_whole_tensor_span() {
        let dir = tempfile::tempdir().unwrap();
        let catalog = catalog_with_f32(dir.path());
        let envelope = where_envelope(&catalog, &catalog.tensors[0], None).unwrap();
        let text = envelope.to_json_string().unwrap();
        assert!(text.contains("\"scope\":\"whole_tensor\""));
        assert!(text.contains("\"length\":\"24\""));
    }

    #[test]
    fn where_out_of_bounds_coordinate_is_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let catalog = catalog_with_f32(dir.path());
        let err = where_envelope(&catalog, &catalog.tensors[0], Some(&[2, 0])).unwrap_err();
        assert_eq!(err.code().as_str(), "INVALID_REQUEST");
    }

    #[test]
    fn where_q4_0_element_reports_exact_bits_with_scale_dependency() {
        let dir = tempfile::tempdir().unwrap();
        let catalog = catalog_with_q4(dir.path());
        let tensor = &catalog.tensors[0];
        assert_eq!(tensor.encoding, "ggml.q4_0");
        let envelope = where_envelope(&catalog, tensor, Some(&[0, 0])).unwrap();
        let text = envelope.to_json_string().unwrap();
        assert!(text.contains("\"precision\":\"exact_bits\""));
        assert!(text.contains("\"mask_hex\":\"0f\""));
        assert!(text.contains("\"bit_numbering\":\"lsb0\""));
        assert!(text.contains("\"role\":\"scale\""));
        assert!(text.contains("\"role\":\"code\""));

        // Element 16 shares byte 2 at the high nibble.
        let envelope = where_envelope(&catalog, tensor, Some(&[0, 16])).unwrap();
        let text = envelope.to_json_string().unwrap();
        assert!(text.contains("\"mask_hex\":\"f0\""));
        assert!(text.contains("\"shift\":\"4\""));
    }

    #[test]
    fn where_unknown_encoding_element_is_unsupported() {
        // A tensor whose encoding has no layout: whole-tensor mapping works,
        // per-element addressing reports CODEC_UNSUPPORTED.
        let tensor = CatalogTensor {
            id: "tensor:x".to_string(),
            semantic: Json::Null,
            source_id: "src:x".to_string(),
            original_name: "odd".to_string(),
            encoding: "gguf.type14".to_string(),
            decode_supported: false,
            shape: vec![8],
            element_count: 8,
            payload_start: 100,
            payload_length: Some(8),
            extent: crate::nn::format::Extent::Exact,
        };
        let whole = where_location(&tensor, None).unwrap();
        assert_eq!(whole.precision, AddressPrecision::ExactContiguous);
        let err = where_location(&tensor, Some(&[1])).unwrap_err();
        assert_eq!(err.code().as_str(), "CODEC_UNSUPPORTED");
    }

    #[test]
    fn locate_reports_owners_and_details() {
        let dir = tempfile::tempdir().unwrap();
        let catalog = catalog_with_f32(dir.path());
        let w = &catalog.tensors[0];
        let b = &catalog.tensors[1];
        // Header length bytes: anything before w.payload_start has no owner.
        let text = locate_text(&catalog, w.payload_start + 20).unwrap();
        assert!(text.contains("w: element [1,2]"), "{text}");
        let text = locate_text(&catalog, b.payload_start).unwrap();
        assert!(text.contains("b: payload start"), "{text}");
        let text = locate_text(&catalog, 3).unwrap();
        assert!(text.contains("no exact-extent tensor owns"), "{text}");

        let envelope = locate_envelope(&catalog, w.payload_start + 4).unwrap();
        let json = envelope.to_json_string().unwrap();
        assert!(json.contains("\"operation\":\"locate\""));
        assert!(json.contains("\"hit_count\":\"1\""));
    }

    #[test]
    fn locate_q4_0_scale_and_code_roles() {
        let dir = tempfile::tempdir().unwrap();
        let catalog = catalog_with_q4(dir.path());
        let tensor = &catalog.tensors[0];
        let text = locate_text(&catalog, tensor.payload_start + 1).unwrap();
        assert!(text.contains("scale of block 0"), "{text}");
        let text = locate_text(&catalog, tensor.payload_start + 2).unwrap();
        assert!(text.contains("low nibble = element 0"), "{text}");
        assert!(text.contains("high nibble = element 16"), "{text}");
    }
}
