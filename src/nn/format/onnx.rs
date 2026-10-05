//! ONNX model reader (descriptor tier).
//!
//! ONNX models are protobuf-encoded (`ModelProto` → `GraphProto` →
//! initializer `TensorProto`s). This reader walks the wire format with a
//! positional cursor over bounded reads: large payloads (`raw_data`) are
//! located and skipped without loading, packed numeric fields are reported
//! honestly as non-contiguous storage, and external-data tensors stay
//! visible descriptors with unresolved extents. Graph nodes are counted and
//! their operator types recorded — explicit computation evidence, never
//! fabricated weights.

use super::super::budget::Budget;
use super::super::error::NnError;
use super::super::source::BoundedFile;
use super::{Extent, Finding, FormatInventory, TensorEntry, Validity};

/// A positional protobuf cursor over a byte range of a file.
struct ProtoCursor<'a> {
    reader: &'a BoundedFile,
    /// Absolute position of the next byte.
    pos: u64,
    /// Exclusive end of the cursor's range.
    end: u64,
}

impl<'a> ProtoCursor<'a> {
    fn new(reader: &'a BoundedFile, start: u64, end: u64) -> Self {
        ProtoCursor {
            reader,
            pos: start,
            end,
        }
    }

    fn has_more(&self) -> bool {
        self.pos < self.end
    }

    /// Read one varint (max 10 bytes).
    fn varint(&mut self, budget: &Budget) -> Result<u64, NnError> {
        let mut value: u64 = 0;
        let mut shift = 0u32;
        for _ in 0..10 {
            if self.pos >= self.end {
                return Err(truncated("varint"));
            }
            let mut byte = [0u8; 1];
            self.reader
                .read_exact_at_bounded(self.pos, &mut byte, budget)?;
            self.pos += 1;
            value |= ((byte[0] & 0x7f) as u64) << shift;
            if byte[0] & 0x80 == 0 {
                return Ok(value);
            }
            shift += 7;
        }
        Err(NnError::MalformedInput {
            detail: "protobuf varint exceeds 10 bytes".to_string(),
        })
    }

    /// Read a field tag: `(field_number, wire_type)`.
    fn tag(&mut self, budget: &Budget) -> Result<(u32, u32), NnError> {
        let key = self.varint(budget)?;
        Ok(((key >> 3) as u32, (key & 0x7) as u32))
    }

    /// Length-delimited header: returns `(start, length)` of the payload and
    /// advances past it without reading the content.
    fn length_field(&mut self, budget: &Budget) -> Result<(u64, u64), NnError> {
        let length = self.varint(budget)?;
        let start = self.pos;
        let end = start
            .checked_add(length)
            .ok_or_else(|| truncated("length-delimited field"))?;
        if end > self.end {
            return Err(truncated("length-delimited field"));
        }
        self.pos = end;
        Ok((start, length))
    }

    /// Read the full content of a length-delimited field into a buffer
    /// (small fields only; bounded by `max`).
    fn bytes_field(&mut self, budget: &Budget, max: u64) -> Result<Vec<u8>, NnError> {
        let (start, length) = self.length_field(budget)?;
        if length > max {
            return Err(NnError::MalformedInput {
                detail: format!(
                    "protobuf field of {length} bytes exceeds the {max}-byte read limit"
                ),
            });
        }
        let mut buffer = vec![0u8; length as usize];
        self.reader
            .read_exact_at_bounded(start, &mut buffer, budget)?;
        Ok(buffer)
    }

    /// Skip a field of the given wire type.
    fn skip(&mut self, wire: u32, budget: &Budget) -> Result<(), NnError> {
        match wire {
            0 => {
                self.varint(budget)?;
            }
            1 => {
                self.advance(8)?;
            }
            2 => {
                self.length_field(budget)?;
            }
            5 => {
                self.advance(4)?;
            }
            other => {
                return Err(NnError::MalformedInput {
                    detail: format!("unsupported protobuf wire type {other}"),
                })
            }
        }
        Ok(())
    }

    fn advance(&mut self, n: u64) -> Result<(), NnError> {
        let end = self
            .pos
            .checked_add(n)
            .ok_or_else(|| truncated("fixed-width field"))?;
        if end > self.end {
            return Err(truncated("fixed-width field"));
        }
        self.pos = end;
        Ok(())
    }
}

fn truncated(what: &str) -> NnError {
    NnError::MalformedInput {
        detail: format!("truncated protobuf: {what} extends past its container"),
    }
}

// ---- ONNX data types ----

/// Map an ONNX `TensorProto.DataType` number to a precise encoding id
/// (`None` for unregistered types).
fn encoding_for(data_type: u64) -> Option<(&'static str, u64)> {
    Some(match data_type {
        1 => ("onnx.float", 4),
        2 => ("onnx.uint8", 1),
        3 => ("onnx.int8", 1),
        4 => ("onnx.uint16", 2),
        5 => ("onnx.int16", 2),
        6 => ("onnx.int32", 4),
        7 => ("onnx.int64", 8),
        9 => ("onnx.bool", 1),
        10 => ("onnx.float16", 2),
        11 => ("onnx.double", 8),
        12 => ("onnx.uint32", 4),
        13 => ("onnx.uint64", 8),
        14 => ("onnx.bfloat16", 2),
        _ => return None,
    })
}

/// A parsed graph summary (computation evidence, descriptor-level).
#[derive(Debug, Clone)]
pub struct GraphSummary {
    pub name: String,
    pub node_count: usize,
    /// Operator-type histogram, sorted by (count desc, name).
    pub op_types: Vec<(String, usize)>,
}

/// One external-data reference.
#[derive(Debug, Clone)]
pub struct ExternalRef {
    pub tensor: String,
    pub location: String,
    pub offset: Option<u64>,
    pub length: Option<u64>,
    pub checksum: Option<String>,
}

/// ONNX-specific findings carried outside `FormatInventory` (the graph
/// summary and external refs extend the standard shape).
#[derive(Debug, Clone)]
pub struct OnnxInventory {
    pub inventory: FormatInventory,
    pub graph: Option<GraphSummary>,
    pub external_refs: Vec<ExternalRef>,
    pub packed_data_tensors: Vec<String>,
}

/// Try to parse the file as an ONNX model. `Err(MalformedInput)` means the
/// bytes are not a parseable ModelProto.
pub fn inventory(reader: &BoundedFile, budget: &Budget) -> Result<OnnxInventory, NnError> {
    let total = reader.length();
    let mut findings: Vec<Finding> = Vec::new();
    let mut validity = Validity::Valid;
    let mut tensors: Vec<TensorEntry> = Vec::new();
    let mut graph: Option<GraphSummary> = None;
    let mut external_refs: Vec<ExternalRef> = Vec::new();
    let mut packed: Vec<String> = Vec::new();

    let mut cursor = ProtoCursor::new(reader, 0, total);
    let mut ir_version: Option<u64> = None;
    while cursor.has_more() {
        let (field, wire) = cursor.tag(budget)?;
        match (field, wire) {
            (1, 0) => ir_version = Some(cursor.varint(budget)?),
            (7, 2) => {
                let (start, length) = cursor.length_field(budget)?;
                let summary = parse_graph(
                    reader,
                    start,
                    start + length,
                    budget,
                    &mut tensors,
                    &mut findings,
                    &mut external_refs,
                    &mut packed,
                    &mut validity,
                )?;
                graph = Some(summary);
            }
            _ => cursor.skip(wire, budget)?,
        }
    }

    let Some(graph) = graph else {
        return Err(NnError::MalformedInput {
            detail: "protobuf parses but no ONNX graph field is present".to_string(),
        });
    };
    if graph.node_count == 0 && tensors.is_empty() {
        validity = Validity::Invalid;
        findings.push(Finding::warning(
            "ONNX_EMPTY_GRAPH",
            "the graph has neither nodes nor initializers",
        ));
    }

    let version = ir_version
        .map(|v| v.to_string())
        .unwrap_or_else(|| "unknown".to_string());
    Ok(OnnxInventory {
        inventory: FormatInventory {
            format: "onnx".to_string(),
            format_version: format!("ir{version}"),
            validity,
            tensors,
            findings,
            metadata_bytes: total.min(cursor.pos),
            payload_bytes: 0,
        },
        graph: Some(graph),
        external_refs,
        packed_data_tensors: packed,
    })
}

#[allow(clippy::too_many_arguments)]
fn parse_graph(
    reader: &BoundedFile,
    start: u64,
    end: u64,
    budget: &Budget,
    tensors: &mut Vec<TensorEntry>,
    findings: &mut Vec<Finding>,
    external_refs: &mut Vec<ExternalRef>,
    packed: &mut Vec<String>,
    validity: &mut Validity,
) -> Result<GraphSummary, NnError> {
    let mut cursor = ProtoCursor::new(reader, start, end);
    let mut name = String::new();
    let mut node_count = 0usize;
    let mut op_types: std::collections::BTreeMap<String, usize> = std::collections::BTreeMap::new();

    while cursor.has_more() {
        let (field, wire) = cursor.tag(budget)?;
        match (field, wire) {
            (1, 2) => {
                // NodeProto.
                let (nstart, nlen) = cursor.length_field(budget)?;
                let op = parse_node(reader, nstart, nstart + nlen, budget)?;
                node_count += 1;
                *op_types.entry(op).or_insert(0) += 1;
            }
            (2, 2) => {
                name = String::from_utf8(cursor.bytes_field(budget, 1 << 20)?).map_err(|_| {
                    NnError::MalformedInput {
                        detail: "graph name is not valid UTF-8".to_string(),
                    }
                })?;
            }
            (5, 2) => {
                // Initializer TensorProto.
                let (tstart, tlen) = cursor.length_field(budget)?;
                parse_tensor(
                    reader,
                    tstart,
                    tstart + tlen,
                    budget,
                    tensors,
                    findings,
                    external_refs,
                    packed,
                    validity,
                )?;
            }
            _ => cursor.skip(wire, budget)?,
        }
    }
    let mut histogram: Vec<(String, usize)> = op_types.into_iter().collect();
    histogram.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    Ok(GraphSummary {
        name,
        node_count,
        op_types: histogram,
    })
}

fn parse_node(
    reader: &BoundedFile,
    start: u64,
    end: u64,
    budget: &Budget,
) -> Result<String, NnError> {
    let mut cursor = ProtoCursor::new(reader, start, end);
    let mut op_type = String::new();
    while cursor.has_more() {
        let (field, wire) = cursor.tag(budget)?;
        match (field, wire) {
            (4, 2) => {
                op_type = String::from_utf8(cursor.bytes_field(budget, 4096)?).map_err(|_| {
                    NnError::MalformedInput {
                        detail: "node op_type is not valid UTF-8".to_string(),
                    }
                })?;
            }
            _ => cursor.skip(wire, budget)?,
        }
    }
    Ok(op_type)
}

#[allow(clippy::too_many_arguments)]
fn parse_tensor(
    reader: &BoundedFile,
    start: u64,
    end: u64,
    budget: &Budget,
    tensors: &mut Vec<TensorEntry>,
    findings: &mut Vec<Finding>,
    external_refs: &mut Vec<ExternalRef>,
    packed: &mut Vec<String>,
    validity: &mut Validity,
) -> Result<(), NnError> {
    let mut cursor = ProtoCursor::new(reader, start, end);
    let mut dims: Vec<u64> = Vec::new();
    let mut data_type: Option<u64> = None;
    let mut name = String::new();
    let mut raw_span: Option<(u64, u64)> = None;
    let mut external: Vec<(String, String)> = Vec::new();
    let mut data_location: u64 = 0;
    let mut packed_spans: Vec<(u64, u64)> = Vec::new();

    while cursor.has_more() {
        let (field, wire) = cursor.tag(budget)?;
        match (field, wire) {
            (1, 0) => dims.push(cursor.varint(budget)?),
            (1, 2) => {
                // Packed dims.
                let (pstart, plen) = cursor.length_field(budget)?;
                let mut inner = ProtoCursor::new(reader, pstart, pstart + plen);
                while inner.has_more() {
                    dims.push(inner.varint(budget)?);
                }
            }
            (2, 0) => data_type = Some(cursor.varint(budget)?),
            (8, 2) => {
                name = String::from_utf8(cursor.bytes_field(budget, 1 << 20)?).map_err(|_| {
                    NnError::MalformedInput {
                        detail: "tensor name is not valid UTF-8".to_string(),
                    }
                })?;
            }
            (9, 2) => raw_span = Some(cursor.length_field(budget)?),
            (13, 2) => {
                // StringStringEntryProto (external_data).
                let (estart, elen) = cursor.length_field(budget)?;
                let entry = parse_entry(reader, estart, estart + elen, budget)?;
                external.push(entry);
            }
            (14, 0) => data_location = cursor.varint(budget)?,
            // Packed numeric data fields: record spans only.
            (4, 2) | (5, 2) | (6, 2) | (7, 2) => {
                packed_spans.push(cursor.length_field(budget)?);
            }
            // Unpacked scalar entries fall through the generic skip.
            _ => cursor.skip(wire, budget)?,
        }
    }

    let Some(data_type) = data_type else {
        *validity = Validity::Invalid;
        findings.push(Finding::error(
            "ONNX_MISSING_DTYPE",
            format!(
                "initializer {} has no data_type field",
                crate::nn::error::brief(&name)
            ),
        ));
        return Ok(());
    };
    let element_count: u64 = dims.iter().product();
    let (encoding, _width) = match encoding_for(data_type) {
        Some((encoding, width)) => (encoding.to_string(), width),
        None => {
            findings.push(Finding::warning(
                "ONNX_UNKNOWN_DTYPE",
                format!(
                    "initializer {} uses unregistered ONNX dtype {data_type}",
                    crate::nn::error::brief(&name)
                ),
            ));
            (format!("onnx.dtype{data_type}"), 0)
        }
    };

    let get = |key: &str| {
        external
            .iter()
            .find(|(k, _)| k == key)
            .map(|(_, v)| v.clone())
    };
    if data_location == 1 {
        // External data: descriptor stays visible with an unresolved extent.
        external_refs.push(ExternalRef {
            tensor: name.clone(),
            location: get("location").unwrap_or_default(),
            offset: get("offset").and_then(|v| v.parse().ok()),
            length: get("length").and_then(|v| v.parse().ok()),
            checksum: get("checksum"),
        });
        findings.push(Finding::warning(
            "ONNX_EXTERNAL_DATA",
            format!(
                "initializer {} stores its payload externally ({}); the span is unresolved until the referenced data is bound",
                crate::nn::error::brief(&name),
                get("location").as_deref().unwrap_or("location unset")
            ),
        ));
        tensors.push(TensorEntry {
            original_name: name.clone(),
            encoding,
            decode_supported: false,
            shape: dims,
            element_count,
            payload_start: 0,
            payload_length: None,
            extent: Extent::UpperBoundOnly,
        });
        return Ok(());
    }

    if let Some((rstart, rlen)) = raw_span {
        // Exact-span tensor: raw_data holds the payload bytes verbatim.
        tensors.push(TensorEntry {
            original_name: name.clone(),
            encoding,
            decode_supported: false,
            shape: dims,
            element_count,
            payload_start: rstart,
            payload_length: Some(rlen),
            extent: Extent::Exact,
        });
        return Ok(());
    }

    if !packed_spans.is_empty() {
        // Packed protobuf fields: multiple blobs with framing between them —
        // not one contiguous payload. Reported honestly.
        packed.push(name.clone());
        let first = packed_spans.first().copied().unwrap_or((0, 0));
        findings.push(Finding::info(
            "ONNX_PACKED_DATA",
            format!(
                "initializer {} stores values in {} packed protobuf field(s) starting at byte {}; no single contiguous payload span is claimed",
                crate::nn::error::brief(&name),
                packed_spans.len(),
                first.0
            ),
        ));
        tensors.push(TensorEntry {
            original_name: name.clone(),
            encoding,
            decode_supported: false,
            shape: dims,
            element_count,
            payload_start: first.0,
            payload_length: None,
            extent: Extent::UpperBoundOnly,
        });
        return Ok(());
    }

    // No payload at all: an empty initializer is still a descriptor.
    tensors.push(TensorEntry {
        original_name: name.clone(),
        encoding,
        decode_supported: false,
        shape: dims,
        element_count,
        payload_start: 0,
        payload_length: Some(0),
        extent: Extent::Exact,
    });
    Ok(())
}

fn parse_entry(
    reader: &BoundedFile,
    start: u64,
    end: u64,
    budget: &Budget,
) -> Result<(String, String), NnError> {
    let mut cursor = ProtoCursor::new(reader, start, end);
    let mut key = String::new();
    let mut value = String::new();
    while cursor.has_more() {
        let (field, wire) = cursor.tag(budget)?;
        match (field, wire) {
            (1, 2) => {
                key = String::from_utf8(cursor.bytes_field(budget, 4096)?).map_err(|_| {
                    NnError::MalformedInput {
                        detail: "external_data key is not valid UTF-8".to_string(),
                    }
                })?;
            }
            (2, 2) => {
                value = String::from_utf8(cursor.bytes_field(budget, 1 << 20)?).map_err(|_| {
                    NnError::MalformedInput {
                        detail: "external_data value is not valid UTF-8".to_string(),
                    }
                })?;
            }
            _ => cursor.skip(wire, budget)?,
        }
    }
    Ok((key, value))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::nn::budget::{Budget, BudgetCaps};
    use crate::nn::cancel::CancellationToken;

    fn budget() -> Budget {
        Budget::new(BudgetCaps::default(), None, CancellationToken::new())
    }

    /// Minimal protobuf builders for fixtures.
    fn varint(mut value: u64) -> Vec<u8> {
        let mut out = Vec::new();
        loop {
            let byte = (value & 0x7f) as u8;
            value >>= 7;
            if value == 0 {
                out.push(byte);
                break;
            }
            out.push(byte | 0x80);
        }
        out
    }

    fn tag(field: u32, wire: u32) -> Vec<u8> {
        varint(((field as u64) << 3) | wire as u64)
    }

    fn len_delim(field: u32, payload: &[u8]) -> Vec<u8> {
        let mut out = tag(field, 2);
        out.extend(varint(payload.len() as u64));
        out.extend_from_slice(payload);
        out
    }

    fn varint_field(field: u32, value: u64) -> Vec<u8> {
        let mut out = tag(field, 0);
        out.extend(varint(value));
        out
    }

    fn tensor_proto(name: &str, data_type: u64, dims: &[u64], payload: TensorPayload) -> Vec<u8> {
        let mut out = Vec::new();
        let mut packed_dims = Vec::new();
        for &d in dims {
            packed_dims.extend(varint(d));
        }
        if !packed_dims.is_empty() {
            out.extend(len_delim(1, &packed_dims));
        }
        out.extend(varint_field(2, data_type));
        out.extend(len_delim(8, name.as_bytes()));
        match payload {
            TensorPayload::Raw(bytes) => out.extend(len_delim(9, &bytes)),
            TensorPayload::External {
                location,
                offset,
                length,
            } => {
                out.extend(len_delim(
                    13,
                    &[len_delim(1, b"location"), len_delim(2, location.as_bytes())].concat(),
                ));
                if let Some(offset) = offset {
                    out.extend(len_delim(
                        13,
                        &[
                            len_delim(1, b"offset"),
                            len_delim(2, offset.to_string().as_bytes()),
                        ]
                        .concat(),
                    ));
                }
                if let Some(length) = length {
                    out.extend(len_delim(
                        13,
                        &[
                            len_delim(1, b"length"),
                            len_delim(2, length.to_string().as_bytes()),
                        ]
                        .concat(),
                    ));
                }
                out.extend(varint_field(14, 1));
            }
            TensorPayload::PackedFloat(values) => {
                let mut blob = Vec::new();
                for v in values {
                    blob.extend_from_slice(&v.to_le_bytes());
                }
                out.extend(len_delim(4, &blob));
            }
            TensorPayload::None => {}
        }
        out
    }

    #[expect(
        dead_code,
        reason = "fixture builders read clearer with an explicit empty payload"
    )]
    enum TensorPayload {
        Raw(Vec<u8>),
        External {
            location: String,
            offset: Option<u64>,
            length: Option<u64>,
        },
        PackedFloat(Vec<f32>),
        None,
    }

    fn node_proto(op_type: &str) -> Vec<u8> {
        let mut out = Vec::new();
        out.extend(len_delim(1, b"input_a"));
        out.extend(len_delim(2, b"output_a"));
        out.extend(len_delim(4, op_type.as_bytes()));
        out
    }

    fn model_proto(graph: &[u8]) -> Vec<u8> {
        let mut out = Vec::new();
        out.extend(varint_field(1, 9));
        out.extend(len_delim(7, graph));
        out
    }

    fn graph_proto(name: &str, nodes: &[Vec<u8>], tensors: &[Vec<u8>]) -> Vec<u8> {
        let mut out = Vec::new();
        for node in nodes {
            out.extend(len_delim(1, node));
        }
        out.extend(len_delim(2, name.as_bytes()));
        for tensor in tensors {
            out.extend(len_delim(5, tensor));
        }
        out
    }

    fn open(_dir: &std::path::Path, bytes: &[u8]) -> (tempfile::TempDir, BoundedFile) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("m.onnx");
        std::fs::write(&path, bytes).unwrap();
        let reader = BoundedFile::open(&path).unwrap();
        (dir, reader)
    }

    #[test]
    fn inventories_raw_data_tensors_with_exact_spans() {
        // graph: one MatMul node + one F32 [2,2] initializer with raw_data.
        let payload = vec![0u8; 16];
        let tensor = tensor_proto("w", 1, &[2, 2], TensorPayload::Raw(payload));
        let graph = graph_proto("g", &[node_proto("MatMul")], &[tensor]);
        let model = model_proto(&graph);
        let (_dir, reader) = open(std::path::Path::new("/tmp"), &model);
        let result = inventory(&reader, &budget()).unwrap();
        assert_eq!(result.inventory.validity, Validity::Valid);
        assert_eq!(result.inventory.tensors.len(), 1);
        let t = &result.inventory.tensors[0];
        assert_eq!(t.original_name, "w");
        assert_eq!(t.encoding, "onnx.float");
        assert_eq!(t.shape, vec![2, 2]);
        assert_eq!(t.payload_length, Some(16));
        assert_eq!(t.extent, Extent::Exact);
        assert!(t.payload_start > 0);
        // The raw bytes at the recorded span are the payload.
        let mut buf = vec![0u8; 16];
        reader.read_exact_at(t.payload_start, &mut buf).unwrap();
        assert_eq!(buf, vec![0u8; 16]);
        // Graph evidence present.
        let graph = result.graph.unwrap();
        assert_eq!(graph.node_count, 1);
        assert_eq!(graph.op_types, vec![("MatMul".to_string(), 1)]);
        assert!(
            result.inventory.findings.is_empty(),
            "{:?}",
            result.inventory.findings
        );
    }

    #[test]
    fn external_data_stays_visible_with_unresolved_extent() {
        let tensor = tensor_proto(
            "big",
            1,
            &[4, 4],
            TensorPayload::External {
                location: "weights.bin".to_string(),
                offset: Some(4096),
                length: Some(64),
            },
        );
        let graph = graph_proto("g", &[], &[tensor]);
        let model = model_proto(&graph);
        let (_dir, reader) = open(std::path::Path::new("/tmp"), &model);
        let result = inventory(&reader, &budget()).unwrap();
        let t = &result.inventory.tensors[0];
        assert_eq!(t.payload_length, None);
        assert_eq!(t.extent, Extent::UpperBoundOnly);
        assert_eq!(result.external_refs.len(), 1);
        assert_eq!(result.external_refs[0].location, "weights.bin");
        assert_eq!(result.external_refs[0].offset, Some(4096));
        assert_eq!(result.external_refs[0].length, Some(64));
        assert!(result
            .inventory
            .findings
            .iter()
            .any(|f| f.code == "ONNX_EXTERNAL_DATA"));
    }

    #[test]
    fn packed_data_is_reported_not_faked() {
        let tensor = tensor_proto(
            "p",
            1,
            &[3],
            TensorPayload::PackedFloat(vec![1.0, 2.0, 3.0]),
        );
        let graph = graph_proto("g", &[], &[tensor]);
        let model = model_proto(&graph);
        let (_dir, reader) = open(std::path::Path::new("/tmp"), &model);
        let result = inventory(&reader, &budget()).unwrap();
        let t = &result.inventory.tensors[0];
        assert_eq!(
            t.payload_length, None,
            "packed fields are not one contiguous span"
        );
        assert_eq!(result.packed_data_tensors, vec!["p".to_string()]);
        assert!(result
            .inventory
            .findings
            .iter()
            .any(|f| f.code == "ONNX_PACKED_DATA"));
    }

    #[test]
    fn non_onnx_files_are_rejected() {
        let (_dir, reader) = open(std::path::Path::new("/tmp"), b"not protobuf at all");
        assert!(inventory(&reader, &budget()).is_err());
        // A varint-only file without a graph is also not a model.
        let (_dir, reader) = open(std::path::Path::new("/tmp"), &varint_field(1, 9));
        assert!(inventory(&reader, &budget()).is_err());
    }

    #[test]
    fn dtype_registry_covers_known_types() {
        assert_eq!(encoding_for(1), Some(("onnx.float", 4)));
        assert_eq!(encoding_for(14), Some(("onnx.bfloat16", 2)));
        assert!(encoding_for(99).is_none());
    }
}
