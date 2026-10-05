//! GGUF descriptor reader.
//!
//! GGUF is treated as a typed metadata and tensor-directory container, not a
//! single quantization scheme. Parsing is descriptor-only: header, metadata
//! entries, and the tensor directory are read with bounded positional reads;
//! no payload bytes are touched. The data origin uses general alignment
//! (`general.alignment` metadata, default 32) with the checked non-power-of-two
//! formula. Known encodings get exact extents from their block geometry;
//! unknown type codes stay visible descriptors with bounded extents.

use super::{align_up, Extent, Finding, FormatInventory, TensorEntry, Validity};
use crate::nn::budget::Budget;
use crate::nn::error::NnError;
use crate::nn::source::BoundedFile;

const GGUF_MAGIC: &[u8; 4] = b"GGUF";
const DEFAULT_ALIGNMENT: u64 = 32;
/// Longest accepted name/metadata-string length (sanity bound).
const MAX_STRING_BYTES: u64 = 4 * 1024 * 1024;
const MAX_DIMENSIONS: u32 = 64;

/// GGUF metadata value types (v2/v3 numbering).
const T_U8: u32 = 0;
const T_I8: u32 = 1;
const T_U16: u32 = 2;
const T_I16: u32 = 3;
const T_U32: u32 = 4;
const T_I32: u32 = 5;
const T_F32: u32 = 6;
const T_BOOL: u32 = 7;
const T_STRING: u32 = 8;
const T_ARRAY: u32 = 9;
const T_U64: u32 = 10;
const T_I64: u32 = 11;
const T_F64: u32 = 12;

fn scalar_width(value_type: u32) -> Option<usize> {
    match value_type {
        T_U8 | T_I8 | T_BOOL => Some(1),
        T_U16 | T_I16 => Some(2),
        T_U32 | T_I32 | T_F32 => Some(4),
        T_U64 | T_I64 | T_F64 => Some(8),
        _ => None,
    }
}

/// A parsed metadata value.
#[derive(Debug, Clone)]
pub enum MetadataValue {
    U8(u8),
    I8(i8),
    U16(u16),
    I16(i16),
    U32(u32),
    I32(i32),
    F32(f32),
    Bool(bool),
    Str(String),
    U64(u64),
    I64(i64),
    F64(f64),
    Array(u32, Vec<MetadataValue>),
}

/// Registered tensor encodings: `(type code, encoding id, bytes per block,
/// elements per block)`. The registry grows one encoding at a time, each with
/// independent reference tests; anything absent is treated as unknown.
fn registered_encoding(type_code: u32) -> Option<(&'static str, u64, u64, bool)> {
    match type_code {
        0 => Some(("ggml.f32", 4, 1, true)),
        1 => Some(("ggml.f16", 2, 1, true)),
        2 => Some(("ggml.q4_0", 18, 32, false)),
        8 => Some(("ggml.q8_0", 34, 32, false)),
        _ => None,
    }
}

struct Cursor {
    pos: u64,
}

/// Try to parse the file as GGUF. `Err` means the file cannot be interpreted
/// as GGUF (bad magic, unsupported version, or a directory that cannot be
/// traversed); recoverable structural defects stay in the inventory.
pub fn inventory(reader: &BoundedFile, budget: &Budget) -> Result<FormatInventory, NnError> {
    let file_len = reader.length();
    let mut magic = [0u8; 4];
    reader.read_exact_at_bounded(0, &mut magic, budget)?;
    if &magic != GGUF_MAGIC {
        return Err(NnError::MalformedInput {
            detail: "not a GGUF file (magic mismatch)".to_string(),
        });
    }
    let version = read_u32(reader, 4, budget)?;
    if version != 2 && version != 3 {
        return Err(NnError::FormatUnsupported {
            format: format!("gguf.v{}", version),
            reason: "only GGUF v2 and v3 are supported".to_string(),
        });
    }
    let tensor_count = read_u64(reader, 8, budget)?;
    let metadata_count = read_u64(reader, 16, budget)?;

    // Sanity: every metadata entry needs at least 12 bytes (key length field,
    // empty key, type), every tensor entry at least 24. Reject absurd counts
    // before iterating.
    let remaining_after_header = file_len.saturating_sub(24);
    let min_metadata = metadata_count.saturating_mul(12);
    let min_tensors = tensor_count.saturating_mul(24);
    if min_metadata.saturating_add(min_tensors) > remaining_after_header {
        return Err(NnError::MalformedInput {
            detail: format!(
                "declared {} metadata and {} tensor entries cannot fit in the remaining {} bytes",
                metadata_count, tensor_count, remaining_after_header
            ),
        });
    }
    budget.consume_generated(metadata_count.saturating_add(tensor_count))?;

    let mut cursor = Cursor { pos: 24 };
    let mut findings: Vec<Finding> = Vec::new();
    let mut validity = Validity::Valid;
    let mut alignment: u64 = DEFAULT_ALIGNMENT;

    // ---- metadata entries ----
    let mut seen_keys: Vec<String> = Vec::new();
    for _ in 0..metadata_count {
        let key = read_string(reader, &mut cursor, budget)?;
        let value_type = read_u32(reader, cursor.pos, budget)?;
        cursor.pos += 4;
        let value = match read_metadata_value(reader, &mut cursor, value_type, budget)? {
            Ok(value) => value,
            Err(detail) => {
                // Unknown or untraversable type: stop at the justified
                // boundary; the tensor directory cannot be located.
                findings.push(Finding::error(
                    "GGUF_METADATA_TYPE",
                    format!("metadata key {}: {}", crate::nn::error::brief(&key), detail),
                ));
                return Ok(inventory_stopped(
                    version,
                    Validity::Invalid,
                    Vec::new(),
                    findings,
                    cursor.pos,
                ));
            }
        };
        if seen_keys.contains(&key) {
            validity = Validity::Invalid;
            findings.push(Finding::error(
                "GGUF_DUPLICATE_METADATA",
                format!("duplicate metadata key {}", crate::nn::error::brief(&key)),
            ));
        } else {
            seen_keys.push(key.clone());
        }
        if key == "general.alignment" {
            if let MetadataValue::U32(value) = value {
                if value == 0 {
                    validity = Validity::Invalid;
                    findings.push(Finding::error(
                        "GGUF_ALIGNMENT",
                        "general.alignment must be positive".to_string(),
                    ));
                } else {
                    alignment = value as u64;
                }
            }
        }
        budget.checkpoint()?;
    }

    // ---- tensor directory ----
    let mut tensors: Vec<TensorEntry> = Vec::new();
    let mut seen_names: Vec<String> = Vec::new();
    let directory_start = cursor.pos;
    for index in 0..tensor_count {
        let name = read_string(reader, &mut cursor, budget)?;
        let n_dims = read_u32(reader, cursor.pos, budget)?;
        cursor.pos += 4;
        if n_dims > MAX_DIMENSIONS {
            return Err(NnError::MalformedInput {
                detail: format!(
                    "tensor {} declares {} dimensions (limit {})",
                    crate::nn::error::brief(&name),
                    n_dims,
                    MAX_DIMENSIONS
                ),
            });
        }
        let mut dims: Vec<u64> = Vec::with_capacity(n_dims as usize);
        for _ in 0..n_dims {
            dims.push(read_u64(reader, cursor.pos, budget)?);
            cursor.pos += 8;
        }
        let type_code = read_u32(reader, cursor.pos, budget)?;
        cursor.pos += 4;
        let offset = read_u64(reader, cursor.pos, budget)?;
        cursor.pos += 8;

        if seen_names.contains(&name) {
            validity = Validity::Invalid;
            findings.push(Finding::error(
                "GGUF_DUPLICATE_TENSOR",
                format!("duplicate tensor name {}", crate::nn::error::brief(&name)),
            ));
            continue;
        }
        seen_names.push(name.clone());

        let element_count = dims
            .iter()
            .try_fold(1u64, |acc, &d| acc.checked_mul(d))
            .ok_or_else(|| NnError::MalformedInput {
                detail: format!(
                    "tensor {} shape product overflows u64",
                    crate::nn::error::brief(&name)
                ),
            })?;

        match registered_encoding(type_code) {
            Some((encoding, bytes_per_block, elements_per_block, decode_supported)) => {
                if elements_per_block > 1 && element_count % elements_per_block != 0 {
                    validity = Validity::Invalid;
                    findings.push(Finding::error(
                        "GGUF_BLOCK_DIVISIBILITY",
                        format!(
                            "tensor {} has {} elements, not divisible by the {}-element block of {}",
                            crate::nn::error::brief(&name),
                            element_count,
                            elements_per_block,
                            encoding
                        ),
                    ));
                    continue;
                }
                let blocks = element_count / elements_per_block;
                let payload_len =
                    blocks
                        .checked_mul(bytes_per_block)
                        .ok_or_else(|| NnError::MalformedInput {
                            detail: format!(
                                "tensor {} payload size overflows u64",
                                crate::nn::error::brief(&name)
                            ),
                        })?;
                tensors.push(TensorEntry {
                    original_name: name.clone(),
                    encoding: encoding.to_string(),
                    decode_supported,
                    shape: dims,
                    element_count,
                    // Absolute payload start is resolved once the data origin
                    // is known (after the directory); stored relative here via
                    // offset and fixed up below.
                    payload_start: offset,
                    payload_length: Some(payload_len),
                    extent: Extent::Exact,
                });
            }
            None => {
                findings.push(Finding::warning(
                    "GGUF_UNKNOWN_ENCODING",
                    format!(
                        "tensor {} uses unregistered type code {}",
                        crate::nn::error::brief(&name),
                        type_code
                    ),
                ));
                tensors.push(TensorEntry {
                    original_name: name.clone(),
                    encoding: format!("gguf.type{}", type_code),
                    decode_supported: false,
                    shape: dims,
                    element_count,
                    payload_start: offset,
                    payload_length: None,
                    extent: Extent::UpperBoundOnly,
                });
            }
        }
        budget.consume_generated(1)?;
        let _ = index;
    }

    // ---- data origin and payload bounds ----
    let data_origin = align_up(cursor.pos, alignment)?;
    let mut known_intervals: Vec<(u64, u64, usize)> = Vec::new();
    for (position, tensor) in tensors.iter_mut().enumerate() {
        let start_rel = tensor.payload_start;
        let absolute =
            data_origin
                .checked_add(start_rel)
                .ok_or_else(|| NnError::MalformedInput {
                    detail: format!(
                        "tensor {} offset overflows the data origin",
                        crate::nn::error::brief(&tensor.original_name)
                    ),
                })?;
        tensor.payload_start = absolute;
        if let Some(len) = tensor.payload_length {
            let end = absolute
                .checked_add(len)
                .ok_or_else(|| NnError::MalformedInput {
                    detail: format!(
                        "tensor {} extent overflows u64",
                        crate::nn::error::brief(&tensor.original_name)
                    ),
                })?;
            if end > file_len {
                validity = Validity::Invalid;
                findings.push(Finding::error(
                    "GGUF_PAYLOAD_OUT_OF_BOUNDS",
                    format!(
                        "tensor {} extent [{}, {}) exceeds the file length {}",
                        crate::nn::error::brief(&tensor.original_name),
                        absolute,
                        end,
                        file_len
                    ),
                ));
                tensor.payload_length = None;
                tensor.extent = Extent::UpperBoundOnly;
            } else {
                known_intervals.push((start_rel, start_rel + len, position));
            }
        }
    }

    // Overlap check among known extents.
    known_intervals.sort_unstable();
    for pair in known_intervals.windows(2) {
        if pair[1].0 < pair[0].1 {
            validity = Validity::Invalid;
            let a = &tensors[pair[0].2];
            let b = &tensors[pair[1].2];
            findings.push(Finding::error(
                "GGUF_OVERLAP",
                format!(
                    "tensors {} and {} have overlapping payload ranges",
                    crate::nn::error::brief(&a.original_name),
                    crate::nn::error::brief(&b.original_name)
                ),
            ));
        }
    }

    let metadata_bytes = cursor.pos.max(directory_start);
    Ok(FormatInventory {
        format: "gguf".to_string(),
        format_version: format!("v{}", version),
        validity,
        tensors,
        findings,
        metadata_bytes,
        payload_bytes: 0,
    })
}

fn inventory_stopped(
    version: u32,
    validity: Validity,
    tensors: Vec<TensorEntry>,
    findings: Vec<Finding>,
    metadata_bytes: u64,
) -> FormatInventory {
    FormatInventory {
        format: "gguf".to_string(),
        format_version: format!("v{}", version),
        validity,
        tensors,
        findings,
        metadata_bytes,
        payload_bytes: 0,
    }
}

fn read_u32(reader: &BoundedFile, offset: u64, budget: &Budget) -> Result<u32, NnError> {
    let mut buf = [0u8; 4];
    reader.read_exact_at_bounded(offset, &mut buf, budget)?;
    Ok(u32::from_le_bytes(buf))
}

fn read_u64(reader: &BoundedFile, offset: u64, budget: &Budget) -> Result<u64, NnError> {
    let mut buf = [0u8; 8];
    reader.read_exact_at_bounded(offset, &mut buf, budget)?;
    Ok(u64::from_le_bytes(buf))
}

fn read_string(
    reader: &BoundedFile,
    cursor: &mut Cursor,
    budget: &Budget,
) -> Result<String, NnError> {
    let len = read_u64(reader, cursor.pos, budget)?;
    cursor.pos += 8;
    if len > MAX_STRING_BYTES {
        return Err(NnError::MalformedInput {
            detail: format!(
                "string length {} exceeds the {}-byte limit",
                len, MAX_STRING_BYTES
            ),
        });
    }
    let mut bytes = vec![0u8; len as usize];
    reader.read_exact_at_bounded(cursor.pos, &mut bytes, budget)?;
    cursor.pos += len;
    String::from_utf8(bytes).map_err(|_| NnError::MalformedInput {
        detail: format!("string at offset {} is not valid UTF-8", cursor.pos - len),
    })
}

fn read_metadata_value(
    reader: &BoundedFile,
    cursor: &mut Cursor,
    value_type: u32,
    budget: &Budget,
) -> Result<Result<MetadataValue, String>, NnError> {
    if let Some(width) = scalar_width(value_type) {
        let mut buf = vec![0u8; width];
        reader.read_exact_at_bounded(cursor.pos, &mut buf, budget)?;
        cursor.pos += width as u64;
        let value = match value_type {
            T_U8 => MetadataValue::U8(buf[0]),
            T_I8 => MetadataValue::I8(buf[0] as i8),
            T_U16 => MetadataValue::U16(u16::from_le_bytes([buf[0], buf[1]])),
            T_I16 => MetadataValue::I16(i16::from_le_bytes([buf[0], buf[1]])),
            T_U32 => MetadataValue::U32(u32::from_le_bytes(buf.try_into().unwrap())),
            T_I32 => MetadataValue::I32(i32::from_le_bytes(buf.try_into().unwrap())),
            T_F32 => MetadataValue::F32(f32::from_le_bytes(buf.try_into().unwrap())),
            T_BOOL => MetadataValue::Bool(buf[0] != 0),
            T_U64 => MetadataValue::U64(u64::from_le_bytes(buf.try_into().unwrap())),
            T_I64 => MetadataValue::I64(i64::from_le_bytes(buf.try_into().unwrap())),
            T_F64 => MetadataValue::F64(f64::from_le_bytes(buf.try_into().unwrap())),
            _ => unreachable!("scalar_width gates the match"),
        };
        return Ok(Ok(value));
    }
    match value_type {
        T_STRING => Ok(Ok(MetadataValue::Str(read_string(reader, cursor, budget)?))),
        T_ARRAY => {
            let element_type = read_u32(reader, cursor.pos, budget)?;
            cursor.pos += 4;
            let count = read_u64(reader, cursor.pos, budget)?;
            cursor.pos += 8;
            if element_type == T_ARRAY {
                return Ok(Err(
                    "nested arrays are not representable in GGUF".to_string()
                ));
            }
            // Bound the element count against remaining bytes before looping.
            let min_elem = scalar_width(element_type).map(|w| w as u64).unwrap_or(12); // strings: length field + type-ish minimum
            let min_bytes = count.saturating_mul(min_elem);
            if min_bytes > reader.length().saturating_sub(cursor.pos) {
                return Ok(Err(format!(
                    "array of {} elements cannot fit in the remaining bytes",
                    count
                )));
            }
            budget.consume_generated(count.min(1_000_000))?;
            let mut items = Vec::new();
            for _ in 0..count {
                match read_metadata_value(reader, cursor, element_type, budget)? {
                    Ok(item) => items.push(item),
                    Err(detail) => return Ok(Err(detail)),
                }
            }
            Ok(Ok(MetadataValue::Array(element_type, items)))
        }
        other => Ok(Err(format!(
            "unknown metadata value type {} cannot be skipped safely",
            other
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::nn::cancel::CancellationToken;

    fn budget() -> Budget {
        Budget::new(
            crate::nn::budget::BudgetCaps::default(),
            None,
            CancellationToken::new(),
        )
    }

    /// Minimal GGUF builder for fixtures.
    struct GgufBuilder {
        metadata: Vec<(String, Vec<u8>)>,
        tensors: Vec<(String, Vec<u64>, u32, u64)>,
        payload: Vec<u8>,
        version: u32,
        alignment: Option<u32>,
    }

    impl GgufBuilder {
        fn new() -> Self {
            GgufBuilder {
                metadata: Vec::new(),
                tensors: Vec::new(),
                payload: Vec::new(),
                version: 3,
                alignment: None,
            }
        }

        fn meta_string(mut self, key: &str, value: &str) -> Self {
            let mut bytes = Vec::new();
            bytes.extend_from_slice(&(key.len() as u64).to_le_bytes());
            bytes.extend_from_slice(key.as_bytes());
            bytes.extend_from_slice(&T_STRING.to_le_bytes());
            bytes.extend_from_slice(&(value.len() as u64).to_le_bytes());
            bytes.extend_from_slice(value.as_bytes());
            self.metadata.push((key.to_string(), bytes));
            self
        }

        fn meta_u32(mut self, key: &str, value: u32) -> Self {
            let mut bytes = Vec::new();
            bytes.extend_from_slice(&(key.len() as u64).to_le_bytes());
            bytes.extend_from_slice(key.as_bytes());
            bytes.extend_from_slice(&T_U32.to_le_bytes());
            bytes.extend_from_slice(&value.to_le_bytes());
            if key == "general.alignment" {
                self.alignment = Some(value);
            }
            self.metadata.push((key.to_string(), bytes));
            self
        }

        /// Add a tensor with its payload appended at the next aligned offset.
        fn tensor(mut self, name: &str, dims: &[u64], type_code: u32, payload: &[u8]) -> Self {
            let alignment = self.alignment.unwrap_or(DEFAULT_ALIGNMENT as u32) as u64;
            let aligned_offset = align_up(self.payload.len() as u64, alignment).unwrap();
            while self.payload.len() < aligned_offset as usize {
                self.payload.push(0);
            }
            let offset = self.payload.len() as u64;
            self.payload.extend_from_slice(payload);
            self.tensors
                .push((name.to_string(), dims.to_vec(), type_code, offset));
            self
        }

        fn build(self) -> Vec<u8> {
            let mut out = Vec::new();
            out.extend_from_slice(GGUF_MAGIC);
            out.extend_from_slice(&self.version.to_le_bytes());
            out.extend_from_slice(&(self.tensors.len() as u64).to_le_bytes());
            out.extend_from_slice(&(self.metadata.len() as u64).to_le_bytes());
            for (_, bytes) in &self.metadata {
                out.extend_from_slice(bytes);
            }
            for (name, dims, type_code, offset) in &self.tensors {
                out.extend_from_slice(&(name.len() as u64).to_le_bytes());
                out.extend_from_slice(name.as_bytes());
                out.extend_from_slice(&(dims.len() as u32).to_le_bytes());
                for dim in dims {
                    out.extend_from_slice(&dim.to_le_bytes());
                }
                out.extend_from_slice(&type_code.to_le_bytes());
                out.extend_from_slice(&offset.to_le_bytes());
            }
            let origin = align_up(
                out.len() as u64,
                self.alignment.unwrap_or(DEFAULT_ALIGNMENT as u32) as u64,
            )
            .unwrap();
            while (out.len() as u64) < origin {
                out.push(0);
            }
            out.extend_from_slice(&self.payload);
            out
        }
    }

    fn open(data: &[u8]) -> (tempfile::TempDir, BoundedFile) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("m.gguf");
        std::fs::write(&path, data).unwrap();
        (dir, BoundedFile::open(&path).unwrap())
    }

    #[test]
    fn inventories_f32_and_q4_0_tensors() {
        // 32 Q4_0 elements = one 18-byte block; 4 F32 elements = 16 bytes.
        let data = GgufBuilder::new()
            .meta_string("general.architecture", "test")
            .tensor(
                "w_q4",
                &[32],
                2,
                &[
                    0x00, 0x38, 0xa3, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
                ],
            )
            .tensor("w_f32", &[4], 0, &[0u8; 16])
            .build();
        let (_dir, reader) = open(&data);
        let inv = inventory(&reader, &budget()).unwrap();
        assert_eq!(inv.validity, Validity::Valid);
        assert_eq!(inv.tensors.len(), 2);
        assert_eq!(inv.payload_bytes, 0);

        let q4 = &inv.tensors[0];
        assert_eq!(q4.encoding, "ggml.q4_0");
        assert_eq!(q4.payload_length, Some(18));
        assert!(!q4.decode_supported);
        // payload_start is file-qualified and 32-byte aligned after the directory.
        assert_eq!(q4.payload_start % 32, 0);

        let f32_t = &inv.tensors[1];
        assert_eq!(f32_t.encoding, "ggml.f32");
        assert_eq!(f32_t.payload_length, Some(16));
        // The builder pads each tensor to a 32-byte boundary: after an
        // 18-byte Q4_0 block, the next payload starts at offset 32.
        assert_eq!(f32_t.payload_start, q4.payload_start + 32);
        assert!(f32_t.decode_supported);
        assert!(inv.findings.is_empty(), "findings: {:?}", inv.findings);
    }

    #[test]
    fn quantized_tensor_not_block_divisible_is_invalid() {
        let data = GgufBuilder::new().tensor("w", &[31], 2, &[0u8; 18]).build();
        let (_dir, reader) = open(&data);
        let inv = inventory(&reader, &budget()).unwrap();
        assert_eq!(inv.validity, Validity::Invalid);
        assert!(inv
            .findings
            .iter()
            .any(|f| f.code == "GGUF_BLOCK_DIVISIBILITY"));
    }

    #[test]
    fn unknown_type_code_stays_visible() {
        let data = GgufBuilder::new().tensor("w", &[8], 14, &[0u8; 8]).build();
        let (_dir, reader) = open(&data);
        let inv = inventory(&reader, &budget()).unwrap();
        // Unknown encoding is a visible descriptor, not a failure.
        assert_eq!(inv.tensors.len(), 1);
        assert_eq!(inv.tensors[0].encoding, "gguf.type14");
        assert_eq!(inv.tensors[0].extent, Extent::UpperBoundOnly);
        assert_eq!(inv.tensors[0].payload_length, None);
        assert!(inv
            .findings
            .iter()
            .any(|f| f.code == "GGUF_UNKNOWN_ENCODING"));
    }

    #[test]
    fn duplicate_tensor_name_is_invalid() {
        let data = GgufBuilder::new()
            .tensor("w", &[4], 0, &[0u8; 16])
            .tensor("w", &[4], 0, &[0u8; 16])
            .build();
        let (_dir, reader) = open(&data);
        let inv = inventory(&reader, &budget()).unwrap();
        assert_eq!(inv.validity, Validity::Invalid);
        assert!(inv
            .findings
            .iter()
            .any(|f| f.code == "GGUF_DUPLICATE_TENSOR"));
        assert_eq!(inv.tensors.len(), 1);
    }

    #[test]
    fn duplicate_metadata_key_is_invalid() {
        let data = GgufBuilder::new()
            .meta_string("general.name", "a")
            .meta_string("general.name", "b")
            .build();
        let (_dir, reader) = open(&data);
        let inv = inventory(&reader, &budget()).unwrap();
        assert_eq!(inv.validity, Validity::Invalid);
        assert!(inv
            .findings
            .iter()
            .any(|f| f.code == "GGUF_DUPLICATE_METADATA"));
    }

    #[test]
    fn custom_alignment_respected() {
        // Non-power-of-two alignment (6) must work via the general formula:
        // the reader's data origin must match the builder's.
        let data = GgufBuilder::new()
            .meta_u32("general.alignment", 6)
            .tensor("w", &[2], 0, &[0u8; 8])
            .build();
        let (_dir, reader) = open(&data);
        let inv = inventory(&reader, &budget()).unwrap();
        assert_eq!(inv.validity, Validity::Valid);
        let t = &inv.tensors[0];
        assert_eq!(t.payload_length, Some(8));
        // Origin and tensor offsets are both 6-aligned in this fixture.
        assert_eq!(t.payload_start % 6, 0);
        assert_eq!(
            t.payload_start + 8,
            reader.length(),
            "tensor extent must end exactly at EOF"
        );
    }

    #[test]
    fn bad_magic_is_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("m.gguf");
        std::fs::write(&path, b"NOPE....").unwrap();
        let reader = BoundedFile::open(&path).unwrap();
        assert_eq!(
            inventory(&reader, &budget()).unwrap_err().code().as_str(),
            "MALFORMED_INPUT"
        );
    }

    #[test]
    fn unsupported_version_is_format_unsupported() {
        let mut data = GgufBuilder::new().build();
        data[4..8].copy_from_slice(&1u32.to_le_bytes());
        let (_dir, reader) = open(&data);
        assert_eq!(
            inventory(&reader, &budget()).unwrap_err().code().as_str(),
            "FORMAT_UNSUPPORTED"
        );
    }

    #[test]
    fn absurd_entry_counts_are_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("m.gguf");
        let mut data = Vec::new();
        data.extend_from_slice(GGUF_MAGIC);
        data.extend_from_slice(&3u32.to_le_bytes());
        data.extend_from_slice(&u64::MAX.to_le_bytes()); // tensor_count
        data.extend_from_slice(&0u64.to_le_bytes()); // metadata_count
        std::fs::write(&path, data).unwrap();
        let reader = BoundedFile::open(&path).unwrap();
        assert_eq!(
            inventory(&reader, &budget()).unwrap_err().code().as_str(),
            "MALFORMED_INPUT"
        );
    }

    #[test]
    fn payload_extent_beyond_file_is_reported() {
        // Declare an F32 tensor with 4 elements but provide only 8 payload bytes.
        let data = GgufBuilder::new().tensor("w", &[4], 0, &[0u8; 8]).build();
        let (_dir, reader) = open(&data);
        let inv = inventory(&reader, &budget()).unwrap();
        assert_eq!(inv.validity, Validity::Invalid);
        assert!(inv
            .findings
            .iter()
            .any(|f| f.code == "GGUF_PAYLOAD_OUT_OF_BOUNDS"));
    }

    #[test]
    fn empty_tensor_directory_is_valid() {
        let data = GgufBuilder::new()
            .meta_string("general.architecture", "x")
            .build();
        let (_dir, reader) = open(&data);
        let inv = inventory(&reader, &budget()).unwrap();
        assert_eq!(inv.validity, Validity::Valid);
        assert!(inv.tensors.is_empty());
    }
}
