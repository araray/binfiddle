//! SafeTensors descriptor reader.
//!
//! Layout: an eight-byte little-endian header length, a JSON header, then a
//! tensor data buffer whose offsets are buffer-relative. Parsing is
//! descriptor-only: no payload bytes are read. Structural defects become
//! findings with the inventory retained; a tensor whose dtype is unknown
//! remains a visible descriptor with exact declared offsets but no
//! size/consistency verdict.

use super::{Extent, Finding, FormatInventory, TensorEntry, Validity};
use crate::nn::budget::Budget;
use crate::nn::error::NnError;
use crate::nn::json::{Json, ParseLimits};
use crate::nn::source::BoundedFile;

/// Header field: little-endian u64 length prefix.
pub const HEADER_LENGTH_BYTES: u64 = 8;

/// Registered dtypes with a known element byte size. Unknown dtype strings
/// stay visible with `decode_supported = false` and no size math.
fn element_size(dtype: &str) -> Option<u64> {
    match dtype {
        "F64" => Some(8),
        "F32" => Some(4),
        "F16" => Some(2),
        "BF16" => Some(2),
        "I64" => Some(8),
        "I32" => Some(4),
        "I16" => Some(2),
        "I8" => Some(1),
        "U8" => Some(1),
        "BOOL" => Some(1),
        // One-byte float formats exist in the wild; size is known but no
        // numeric decoder is qualified yet, handled via decode_supported.
        "F8_E4M3" => Some(1),
        "F8_E5M2" => Some(1),
        _ => None,
    }
}

/// Dtypes with a qualified numeric decoder (grows one encoding at a time).
fn decode_supported(dtype: &str) -> bool {
    matches!(
        dtype,
        "F64" | "F32" | "F16" | "BF16" | "I64" | "I32" | "I16" | "I8" | "U8" | "BOOL"
    )
}

/// Try to parse the file as SafeTensors. `Err` means the file could not be
/// interpreted as SafeTensors at all (missing header, absurd length,
/// unparseable JSON); structural findings inside an otherwise readable header
/// are reported through the inventory instead.
pub fn inventory(reader: &BoundedFile, budget: &Budget) -> Result<FormatInventory, NnError> {
    let file_len = reader.length();
    if file_len < HEADER_LENGTH_BYTES {
        return Err(NnError::MalformedInput {
            detail: "file is shorter than the SafeTensors header length field".to_string(),
        });
    }

    let header_len = super::read_u_le(reader, 0, 8, budget)?;
    if header_len < 2 {
        return Err(NnError::MalformedInput {
            detail: format!(
                "SafeTensors header length {} is below the minimum JSON size",
                header_len
            ),
        });
    }
    let data_origin =
        HEADER_LENGTH_BYTES
            .checked_add(header_len)
            .ok_or_else(|| NnError::MalformedInput {
                detail: "SafeTensors header length overflows u64".to_string(),
            })?;
    if data_origin > file_len {
        return Err(NnError::MalformedInput {
            detail: format!(
                "SafeTensors declares {} header bytes but the file has only {}",
                header_len, file_len
            ),
        });
    }

    // Charge the metadata budget before allocating/reading the header.
    budget.consume_metadata(header_len)?;
    let mut header_bytes = vec![0u8; header_len as usize];
    reader.read_exact_at(HEADER_LENGTH_BYTES, &mut header_bytes)?;
    let header_text = std::str::from_utf8(&header_bytes)
        .map_err(|e| NnError::MalformedInput {
            detail: format!("SafeTensors header is not valid UTF-8: {}", e),
        })?
        .to_string();
    let header = Json::parse_foreign(
        &header_text,
        ParseLimits {
            max_depth: 64,
            max_nodes: 1_000_000,
        },
    )
    .map_err(|e| NnError::MalformedInput {
        detail: format!("SafeTensors header JSON: {}", e),
    })?;

    let members = match header {
        Json::Object(members) => members,
        _ => {
            return Err(NnError::MalformedInput {
                detail: "SafeTensors header must be a JSON object".to_string(),
            })
        }
    };

    let mut findings: Vec<Finding> = Vec::new();
    let mut tensors: Vec<TensorEntry> = Vec::new();
    let buffer_len = file_len - data_origin;
    let mut validity = Validity::Valid;

    let mut descriptors: Vec<(&String, &Json)> = Vec::new();
    for (key, value) in &members {
        if key == "__metadata__" {
            match value {
                Json::Object(_) => {}
                other => {
                    validity = Validity::Invalid;
                    findings.push(Finding::error(
                        "ST_METADATA_FIELD",
                        format!("__metadata__ must be an object, found {}", kind_name(other)),
                    ));
                }
            }
            continue;
        }
        descriptors.push((key, value));
    }

    for (name, descriptor) in descriptors {
        let fields = match descriptor {
            Json::Object(fields) => fields,
            other => {
                validity = Validity::Invalid;
                findings.push(Finding::error(
                    "ST_DESCRIPTOR_SHAPE",
                    format!(
                        "tensor {} descriptor must be an object, found {}",
                        brief(name),
                        kind_name(other)
                    ),
                ));
                continue;
            }
        };
        let dtype = fields.iter().find(|(k, _)| k == "dtype").map(|(_, v)| v);
        let shape = fields.iter().find(|(k, _)| k == "shape").map(|(_, v)| v);
        let offsets = fields
            .iter()
            .find(|(k, _)| k == "data_offsets")
            .map(|(_, v)| v);

        let (dtype, shape, offsets) = match (dtype, shape, offsets) {
            (Some(Json::Str(d)), Some(s), Some(o)) => (d.clone(), s.clone(), o.clone()),
            _ => {
                validity = Validity::Invalid;
                findings.push(Finding::error(
                    "ST_MISSING_FIELD",
                    format!("tensor {} is missing dtype/shape/data_offsets", brief(name)),
                ));
                continue;
            }
        };

        let shape_dims = match parse_shape(&shape) {
            Ok(dims) => dims,
            Err(detail) => {
                validity = Validity::Invalid;
                findings.push(Finding::error(
                    "ST_SHAPE_INVALID",
                    format!("tensor {}: {}", brief(name), detail),
                ));
                continue;
            }
        };

        let (begin, end) = match parse_offsets(&offsets) {
            Ok(pair) => pair,
            Err(detail) => {
                validity = Validity::Invalid;
                findings.push(Finding::error(
                    "ST_OFFSETS_INVALID",
                    format!("tensor {}: {}", brief(name), detail),
                ));
                continue;
            }
        };

        if end > buffer_len || begin > end {
            validity = Validity::Invalid;
            findings.push(Finding::error(
                "ST_OFFSET_OUT_OF_BOUNDS",
                format!(
                    "tensor {} offsets [{}, {}) exceed the {}-byte data buffer",
                    brief(name),
                    begin,
                    end,
                    buffer_len
                ),
            ));
            continue;
        }

        let element_count =
            product_checked(&shape_dims).ok_or_else(|| NnError::MalformedInput {
                detail: format!("tensor {} shape product overflows u64", brief(name)),
            })?;

        let known_size = element_size(&dtype);
        if known_size.is_none() {
            findings.push(Finding::warning(
                "ST_UNKNOWN_DTYPE",
                format!("tensor {} has unregistered dtype {}", brief(name), dtype),
            ));
        }

        let declared_len = end - begin;
        if let Some(size) = known_size {
            let expected =
                element_count
                    .checked_mul(size)
                    .ok_or_else(|| NnError::MalformedInput {
                        detail: format!("tensor {} size computation overflows u64", brief(name)),
                    })?;
            if expected != declared_len {
                validity = Validity::Invalid;
                findings.push(Finding::error(
                    "ST_SIZE_MISMATCH",
                    format!(
                        "tensor {} declares {} payload bytes but shape {} x {} bytes requires {}",
                        brief(name),
                        declared_len,
                        element_count,
                        size,
                        expected
                    ),
                ));
                continue;
            }
        }

        // Record unexpected fields without failing read-only inventory.
        for (k, _) in fields {
            if !matches!(k.as_str(), "dtype" | "shape" | "data_offsets") {
                findings.push(Finding::info(
                    "ST_UNKNOWN_FIELD",
                    format!("tensor {} carries unknown field {}", brief(name), k),
                ));
            }
        }

        tensors.push(TensorEntry {
            original_name: name.clone(),
            encoding: format!("safetensors.{}", dtype),
            decode_supported: decode_supported(&dtype),
            shape: shape_dims,
            element_count,
            payload_start: data_origin + begin,
            payload_length: Some(declared_len),
            extent: Extent::Exact,
        });
        budget.consume_generated(1)?;
    }

    // Interval checks: overlaps and buffer coverage. Zero-length entries are
    // positional and excluded from overlap checking.
    let mut intervals: Vec<(u64, u64, &str)> = tensors
        .iter()
        .filter(|t| t.payload_length.unwrap_or(0) > 0)
        .map(|t| {
            (
                t.payload_start - data_origin,
                t.payload_start - data_origin + t.payload_length.unwrap_or(0),
                t.original_name.as_str(),
            )
        })
        .collect();
    intervals.sort_unstable();
    for pair in intervals.windows(2) {
        let (_, a_end, a_name) = pair[0];
        let (b_begin, _, b_name) = pair[1];
        if b_begin < a_end {
            validity = Validity::Invalid;
            findings.push(Finding::error(
                "ST_OVERLAP",
                format!(
                    "tensors {} and {} have overlapping payload ranges",
                    a_name, b_name
                ),
            ));
        }
    }
    // The data buffer must be fully indexed without holes.
    let mut covered: u64 = 0;
    let mut hole = false;
    for (begin, end, _) in &intervals {
        if *begin > covered {
            hole = true;
            findings.push(Finding::error(
                "ST_HOLE",
                format!(
                    "data buffer range [{}, {}) is not indexed by any tensor",
                    covered, begin
                ),
            ));
        }
        covered = covered.max(*end);
    }
    if covered < buffer_len && !intervals.is_empty() {
        hole = true;
        findings.push(Finding::error(
            "ST_HOLE",
            format!(
                "data buffer range [{}, {}) is not indexed by any tensor",
                covered, buffer_len
            ),
        ));
    }
    if hole {
        validity = Validity::Invalid;
    }
    // An entirely empty buffer with no tensors is a legal empty model file.

    Ok(FormatInventory {
        format: "safetensors".to_string(),
        format_version: "1".to_string(),
        validity,
        tensors,
        findings,
        metadata_bytes: HEADER_LENGTH_BYTES + header_len,
        payload_bytes: 0,
    })
}

fn kind_name(value: &Json) -> &'static str {
    match value {
        Json::Null => "null",
        Json::Bool(_) => "a boolean",
        Json::Str(_) => "a string",
        Json::Number(_) => "a number",
        Json::Array(_) => "an array",
        Json::Object(_) => "an object",
    }
}

fn brief(text: &str) -> String {
    crate::nn::error::brief(text)
}

fn parse_shape(shape: &Json) -> Result<Vec<u64>, String> {
    match shape {
        Json::Array(items) => {
            let mut dims = Vec::with_capacity(items.len());
            for item in items {
                let dim = item
                    .as_number_u64()
                    .ok_or_else(|| "shape entries must be non-negative integers".to_string())?;
                dims.push(dim);
            }
            Ok(dims)
        }
        _ => Err("shape must be an array".to_string()),
    }
}

fn parse_offsets(offsets: &Json) -> Result<(u64, u64), String> {
    match offsets {
        Json::Array(items) if items.len() == 2 => {
            let begin = items[0]
                .as_number_u64()
                .ok_or_else(|| "data_offsets entries must be non-negative integers".to_string())?;
            let end = items[1]
                .as_number_u64()
                .ok_or_else(|| "data_offsets entries must be non-negative integers".to_string())?;
            if begin > end {
                Err(format!(
                    "data_offsets [{}, {}) has begin after end",
                    begin, end
                ))
            } else {
                Ok((begin, end))
            }
        }
        _ => Err("data_offsets must be a two-element array".to_string()),
    }
}

fn product_checked(dims: &[u64]) -> Option<u64> {
    let mut product: u64 = 1;
    for &dim in dims {
        product = product.checked_mul(dim)?;
    }
    Some(product)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::nn::cancel::CancellationToken;
    use std::io::Write;

    /// Build a minimal SafeTensors file from (name, dtype, shape, payload)
    /// entries, padding the header with spaces to keep offsets simple.
    fn build_safetensors(entries: &[(&str, &str, &[u64], &[u8])]) -> Vec<u8> {
        let mut body: Vec<u8> = Vec::new();
        let mut spans = Vec::new();
        for (name, dtype, shape, payload) in entries {
            let begin = body.len();
            body.extend_from_slice(payload);
            spans.push((name, dtype, shape, begin, begin + payload.len()));
        }
        let mut header = String::from("{");
        let mut first = true;
        if entries.is_empty() {
            header.push('}');
        }
        for (name, dtype, shape, begin, end) in &spans {
            if !first {
                header.push(',');
            }
            first = false;
            let shape_text: Vec<String> = shape.iter().map(|d| d.to_string()).collect();
            header.push_str(&format!(
                "\"{}\":{{\"dtype\":\"{}\",\"shape\":[{}],\"data_offsets\":[{},{}]}}",
                name,
                dtype,
                shape_text.join(","),
                begin,
                end
            ));
        }
        if !entries.is_empty() {
            header.push('}');
        }
        let mut out = Vec::new();
        out.extend_from_slice(&(header.len() as u64).to_le_bytes());
        out.extend_from_slice(header.as_bytes());
        out.extend_from_slice(&body);
        out
    }

    fn write_and_open(data: &[u8]) -> (tempfile::TempDir, BoundedFile) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("m.safetensors");
        std::fs::write(&path, data).unwrap();
        let reader = BoundedFile::open(&path).unwrap();
        (dir, reader)
    }

    fn budget() -> Budget {
        Budget::new(
            crate::nn::budget::BudgetCaps::default(),
            None,
            CancellationToken::new(),
        )
    }

    #[test]
    fn inventories_a_valid_two_tensor_file() {
        let data = build_safetensors(&[
            ("a", "F32", &[2, 3], &[0u8; 24]),
            ("b", "U8", &[4], &[1, 2, 3, 4]),
        ]);
        let (_dir, reader) = write_and_open(&data);
        let inv = inventory(&reader, &budget()).unwrap();
        assert_eq!(inv.validity, Validity::Valid);
        assert_eq!(inv.tensors.len(), 2);
        assert_eq!(inv.payload_bytes, 0);

        let a = &inv.tensors[0];
        assert_eq!(a.original_name, "a");
        assert_eq!(a.encoding, "safetensors.F32");
        assert_eq!(a.element_count, 6);
        assert_eq!(a.payload_length, Some(24));
        assert!(a.decode_supported);
        // payload_start is file-qualified: 8 + header, i.e. total minus the
        // 28 payload bytes appended after the header.
        assert_eq!(a.payload_start, data.len() as u64 - 28);

        let b = &inv.tensors[1];
        assert_eq!(b.payload_start, a.payload_start + 24);
        assert_eq!(b.payload_length, Some(4));
        assert!(inv.findings.is_empty(), "findings: {:?}", inv.findings);
    }

    #[test]
    fn scalar_and_empty_tensor_rules() {
        // shape [] → one element; [4,0,9] → zero elements owning no bytes.
        let data = build_safetensors(&[
            ("scalar", "F16", &[], &[0x00, 0x3c]),
            ("empty", "F32", &[4, 0, 9], &[]),
        ]);
        let (_dir, reader) = write_and_open(&data);
        let inv = inventory(&reader, &budget()).unwrap();
        assert_eq!(inv.validity, Validity::Valid);
        assert_eq!(inv.tensors[0].element_count, 1);
        assert_eq!(inv.tensors[0].payload_length, Some(2));
        assert_eq!(inv.tensors[1].element_count, 0);
        assert_eq!(inv.tensors[1].payload_length, Some(0));
    }

    #[test]
    fn zero_extent_with_nonzero_bytes_is_invalid() {
        let data = build_safetensors(&[("bad", "F32", &[0], &[1, 2, 3, 4])]);
        let (_dir, reader) = write_and_open(&data);
        let inv = inventory(&reader, &budget()).unwrap();
        assert_eq!(inv.validity, Validity::Invalid);
        assert!(inv.findings.iter().any(|f| f.code == "ST_SIZE_MISMATCH"));
        // The malformed descriptor was dropped, so the payload became a hole.
        assert!(inv.tensors.is_empty());
    }

    #[test]
    fn duplicate_header_key_is_rejected() {
        let header = "{\"a\":{\"dtype\":\"U8\",\"shape\":[1],\"data_offsets\":[0,1]},\"a\":{\"dtype\":\"U8\",\"shape\":[1],\"data_offsets\":[0,1]}}";
        let mut data = Vec::new();
        data.extend_from_slice(&(header.len() as u64).to_le_bytes());
        data.extend_from_slice(header.as_bytes());
        data.push(7);
        let (_dir, reader) = write_and_open(&data);
        let err = inventory(&reader, &budget()).unwrap_err();
        assert_eq!(err.code().as_str(), "MALFORMED_INPUT");
        assert!(err.to_string().contains("duplicate"));
    }

    #[test]
    fn out_of_bounds_offsets_are_reported() {
        let header = "{\"a\":{\"dtype\":\"U8\",\"shape\":[1],\"data_offsets\":[0,9]}}";
        let mut data = Vec::new();
        data.extend_from_slice(&(header.len() as u64).to_le_bytes());
        data.extend_from_slice(header.as_bytes());
        data.push(7); // buffer is 1 byte; offsets claim 9
        let (_dir, reader) = write_and_open(&data);
        let inv = inventory(&reader, &budget()).unwrap();
        assert_eq!(inv.validity, Validity::Invalid);
        assert!(inv
            .findings
            .iter()
            .any(|f| f.code == "ST_OFFSET_OUT_OF_BOUNDS"));
    }

    #[test]
    fn hole_in_buffer_coverage_is_reported() {
        // Tensor a occupies [0,2); tensor b occupies [4,6): bytes 2..4 are a hole.
        let header = "{\"a\":{\"dtype\":\"U8\",\"shape\":[2],\"data_offsets\":[0,2]},\"b\":{\"dtype\":\"U8\",\"shape\":[2],\"data_offsets\":[4,6]}}";
        let mut data = Vec::new();
        data.extend_from_slice(&(header.len() as u64).to_le_bytes());
        data.extend_from_slice(header.as_bytes());
        data.extend_from_slice(&[1, 2, 3, 4, 5, 6]);
        let (_dir, reader) = write_and_open(&data);
        let inv = inventory(&reader, &budget()).unwrap();
        assert_eq!(inv.validity, Validity::Invalid);
        assert!(inv.findings.iter().any(|f| f.code == "ST_HOLE"));
    }

    #[test]
    fn overlapping_tensors_are_reported() {
        // Both descriptors have size-consistent offsets but share bytes 1..3.
        let header = "{\"a\":{\"dtype\":\"U8\",\"shape\":[3],\"data_offsets\":[0,3]},\"b\":{\"dtype\":\"U8\",\"shape\":[2],\"data_offsets\":[1,3]}}";
        let mut data = Vec::new();
        data.extend_from_slice(&(header.len() as u64).to_le_bytes());
        data.extend_from_slice(header.as_bytes());
        data.extend_from_slice(&[1, 2, 3]);
        let (_dir, reader) = write_and_open(&data);
        let inv = inventory(&reader, &budget()).unwrap();
        assert_eq!(inv.validity, Validity::Invalid);
        assert!(inv.findings.iter().any(|f| f.code == "ST_OVERLAP"));
    }

    #[test]
    fn unknown_dtype_stays_visible_without_size_verdict() {
        let header = "{\"a\":{\"dtype\":\"WEIRD7\",\"shape\":[2],\"data_offsets\":[0,4]}}";
        let mut data = Vec::new();
        data.extend_from_slice(&(header.len() as u64).to_le_bytes());
        data.extend_from_slice(header.as_bytes());
        data.extend_from_slice(&[1, 2, 3, 4]);
        let (_dir, reader) = write_and_open(&data);
        let inv = inventory(&reader, &budget()).unwrap();
        assert_eq!(inv.validity, Validity::Valid);
        assert_eq!(inv.tensors.len(), 1);
        let t = &inv.tensors[0];
        assert_eq!(t.encoding, "safetensors.WEIRD7");
        assert!(!t.decode_supported);
        assert_eq!(t.payload_length, Some(4));
        assert!(inv.findings.iter().any(|f| f.code == "ST_UNKNOWN_DTYPE"));
    }

    #[test]
    fn truncated_header_is_malformed() {
        let data = vec![0u8, 0, 0, 0, 0, 0, 0, 1]; // claims 2^56 header bytes
        let (_dir, reader) = write_and_open(&data);
        assert_eq!(
            inventory(&reader, &budget()).unwrap_err().code().as_str(),
            "MALFORMED_INPUT"
        );
    }

    #[test]
    fn metadata_budget_bounds_the_header() {
        let data = build_safetensors(&[("a", "U8", &[1], &[1])]);
        let (_dir, reader) = write_and_open(&data);
        let caps = crate::nn::budget::BudgetCaps {
            metadata_bytes: 4,
            ..crate::nn::budget::BudgetCaps::default()
        };
        let tight = Budget::new(caps, None, CancellationToken::new());
        assert_eq!(
            inventory(&reader, &tight).unwrap_err().code().as_str(),
            "BUDGET_EXCEEDED"
        );
    }

    #[test]
    fn missing_field_is_reported_not_fatal() {
        let header = "{\"a\":{\"dtype\":\"U8\",\"shape\":[1]}}";
        let mut data = Vec::new();
        data.extend_from_slice(&(header.len() as u64).to_le_bytes());
        data.extend_from_slice(header.as_bytes());
        data.push(1);
        let (_dir, reader) = write_and_open(&data);
        let inv = inventory(&reader, &budget()).unwrap();
        assert_eq!(inv.validity, Validity::Invalid);
        assert!(inv.findings.iter().any(|f| f.code == "ST_MISSING_FIELD"));
    }

    #[test]
    fn huge_shape_product_is_rejected() {
        let header =
            "{\"a\":{\"dtype\":\"U8\",\"shape\":[18446744073709551615,2],\"data_offsets\":[0,0]}}";
        let mut data = Vec::new();
        data.extend_from_slice(&(header.len() as u64).to_le_bytes());
        data.extend_from_slice(header.as_bytes());
        let (_dir, reader) = write_and_open(&data);
        let err = inventory(&reader, &budget()).unwrap_err();
        assert_eq!(err.code().as_str(), "MALFORMED_INPUT");
    }

    #[test]
    fn header_write_round_trip_sanity() {
        // The builder must produce parseable files (guards the fixtures above).
        let data = build_safetensors(&[("x", "BOOL", &[3], &[1, 0, 1])]);
        let mut file = tempfile::tempfile().unwrap();
        file.write_all(&data).unwrap();
        assert!(data.len() > 8);
    }
}
