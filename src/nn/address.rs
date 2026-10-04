//! Checked tensor address math and reverse lookup.
//!
//! Forward mapping turns a logical coordinate into a file-qualified byte span
//! (or bit field) with checked arithmetic at every step. Reverse mapping turns
//! a file offset into owning tensors with coordinate families. Overflow,
//! out-of-range indices, and spans beyond a source length are errors, never
//! wrapped or truncated.

use super::codec::{q4_0_block_index, q4_0_code_location, ScalarCodec, TensorLayout};
use super::error::NnError;

/// Row-major linear index of `index` within `shape`, checked.
pub fn dense_linear_index(shape: &[u64], index: &[u64]) -> Result<u64, NnError> {
    if index.len() != shape.len() {
        return Err(NnError::InvalidRequest {
            message: format!(
                "coordinate has {} components but the tensor has {} axes",
                index.len(),
                shape.len()
            ),
        });
    }
    for (axis, (&i, &d)) in index.iter().zip(shape).enumerate() {
        if i >= d {
            return Err(NnError::InvalidRequest {
                message: format!(
                    "index {} on axis {} is out of bounds (extent {})",
                    i, axis, d
                ),
            });
        }
    }
    let mut linear: u64 = 0;
    for (axis, &i) in index.iter().enumerate() {
        // Stride of this axis: product of all later extents.
        let mut stride: u64 = 1;
        for &d in &shape[axis + 1..] {
            stride = stride
                .checked_mul(d)
                .ok_or_else(|| NnError::MalformedInput {
                    detail: "shape stride product overflows u64".to_string(),
                })?;
        }
        let contribution = i
            .checked_mul(stride)
            .ok_or_else(|| NnError::MalformedInput {
                detail: "linear index contribution overflows u64".to_string(),
            })?;
        linear = linear
            .checked_add(contribution)
            .ok_or_else(|| NnError::MalformedInput {
                detail: "linear index overflows u64".to_string(),
            })?;
    }
    Ok(linear)
}

/// Byte offset of one element: `O = tensor_start + width * L`, checked.
pub fn dense_element_offset(
    shape: &[u64],
    index: &[u64],
    element_width: u64,
    tensor_start: u64,
) -> Result<u64, NnError> {
    let linear = dense_linear_index(shape, index)?;
    let bytes = linear
        .checked_mul(element_width)
        .ok_or_else(|| NnError::MalformedInput {
            detail: "element byte offset overflows u64".to_string(),
        })?;
    tensor_start
        .checked_add(bytes)
        .ok_or_else(|| NnError::MalformedInput {
            detail: "element offset overflows u64".to_string(),
        })
}

/// A resolved byte/bit location with its precision classification.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AddressPrecision {
    /// One contiguous byte interval.
    ExactContiguous,
    /// Multiple exact intervals.
    ExactSpans,
    /// A symbolic stride expression defines the selected units exactly.
    ExactStrided,
    /// Exact bits within byte spans (mask/shift known).
    ExactBits,
    /// No stored payload.
    NoPayload,
    /// Mapping cannot be established under current evidence.
    Unresolved,
}

impl AddressPrecision {
    pub fn as_str(&self) -> &'static str {
        match self {
            AddressPrecision::ExactContiguous => "exact_contiguous",
            AddressPrecision::ExactSpans => "exact_spans",
            AddressPrecision::ExactStrided => "exact_strided",
            AddressPrecision::ExactBits => "exact_bits",
            AddressPrecision::NoPayload => "no_payload",
            AddressPrecision::Unresolved => "unresolved",
        }
    }
}

/// One exact location record for a logical coordinate.
#[derive(Debug, Clone)]
pub struct ElementLocation {
    pub precision: AddressPrecision,
    /// Byte span holding the value bits (start, length).
    pub byte_span: (u64, u64),
    /// Bit mask within each byte (LSB0), when narrower than a byte.
    pub bit_mask: Option<(u8, u8)>,
    /// Spans that must be read to decode the value (includes shared parameters).
    pub decode_dependencies: Vec<(u64, u64)>,
}

/// Forward-map one scalar coordinate of a dense scalar tensor.
pub fn locate_scalar_element(
    shape: &[u64],
    index: &[u64],
    codec: ScalarCodec,
    tensor_start: u64,
) -> Result<ElementLocation, NnError> {
    let offset = dense_element_offset(shape, index, codec.width(), tensor_start)?;
    Ok(ElementLocation {
        precision: AddressPrecision::ExactContiguous,
        byte_span: (offset, codec.width()),
        bit_mask: None,
        decode_dependencies: vec![(offset, codec.width())],
    })
}

/// Forward-map one element of a Q4_0 tensor: code nibble plus its scale
/// dependency. The selected code occupies four bits of one byte; the paired
/// nibble belongs to a different logical element and is not part of the span.
pub fn locate_q4_0_element(
    shape: &[u64],
    index: &[u64],
    tensor_start: u64,
) -> Result<ElementLocation, NnError> {
    if shape.len() != 2 {
        return Err(NnError::InvalidRequest {
            message: format!(
                "Q4_0 mapping expects a [rows, columns] tensor, got {} axes",
                shape.len()
            ),
        });
    }
    let linear = dense_linear_index(shape, index)?;
    let block = q4_0_block_index(linear, shape[1])?;
    let block_start = tensor_start
        .checked_add(
            block
                .checked_mul(18)
                .ok_or_else(|| NnError::MalformedInput {
                    detail: "Q4_0 block offset overflows u64".to_string(),
                })?,
        )
        .ok_or_else(|| NnError::MalformedInput {
            detail: "Q4_0 block offset overflows u64".to_string(),
        })?;
    let u = linear % 32;
    let (code_byte, mask, shift) = q4_0_code_location(u);
    let code_offset = block_start + code_byte;
    Ok(ElementLocation {
        precision: AddressPrecision::ExactBits,
        byte_span: (code_offset, 1),
        bit_mask: Some((mask, shift)),
        decode_dependencies: vec![
            (block_start, 2), // shared scale
            (code_offset, 1), // code byte
        ],
    })
}

/// Whole-tensor location: contiguous when the extent is exact, none otherwise.
pub fn locate_whole_tensor(tensor_start: u64, payload_length: Option<u64>) -> ElementLocation {
    match payload_length {
        Some(length) if length > 0 => ElementLocation {
            precision: AddressPrecision::ExactContiguous,
            byte_span: (tensor_start, length),
            bit_mask: None,
            decode_dependencies: vec![(tensor_start, length)],
        },
        Some(_) => ElementLocation {
            precision: AddressPrecision::NoPayload,
            byte_span: (tensor_start, 0),
            bit_mask: None,
            decode_dependencies: vec![],
        },
        None => ElementLocation {
            precision: AddressPrecision::Unresolved,
            byte_span: (tensor_start, 0),
            bit_mask: None,
            decode_dependencies: vec![],
        },
    }
}

// ---- Reverse lookup ----

/// One reverse-lookup hit.
#[derive(Debug, Clone)]
pub struct ReverseHit {
    /// Index into the caller's tensor list.
    pub tensor_index: usize,
    pub tensor_name: String,
    pub payload_start: u64,
    pub payload_length: u64,
    /// How the offset relates to the tensor.
    pub role: ReverseRole,
}

#[derive(Debug, Clone, PartialEq)]
pub enum ReverseRole {
    /// Offset is inside the payload.
    Inside {
        /// Decoded coordinate information, when the layout is qualified.
        detail: String,
    },
    /// Offset is exactly the payload start.
    AtStart,
    /// Offset is the byte past the payload end.
    AtEnd,
}

/// A reverse-lookup query entry describing one tensor.
pub struct ReverseEntry {
    pub name: String,
    pub payload_start: u64,
    pub payload_length: Option<u64>,
    pub layout: TensorLayout,
    pub shape: Vec<u64>,
}

/// Reverse-map a file offset to owning tensors (exact-extent tensors only;
/// bounded-extent tensors cannot own bytes).
pub fn locate_offset(entries: &[ReverseEntry], offset: u64) -> Vec<ReverseHit> {
    let mut hits = Vec::new();
    for (index, entry) in entries.iter().enumerate() {
        let Some(length) = entry.payload_length else {
            continue;
        };
        if offset < entry.payload_start {
            continue;
        }
        let rel = offset - entry.payload_start;
        let Some(end) = entry.payload_start.checked_add(length) else {
            continue;
        };
        let role = if offset == entry.payload_start && length > 0 {
            ReverseRole::AtStart
        } else if offset == end {
            ReverseRole::AtEnd
        } else if offset < end {
            ReverseRole::Inside {
                detail: reverse_detail(entry.layout, &entry.shape, rel, length),
            }
        } else {
            continue;
        };
        hits.push(ReverseHit {
            tensor_index: index,
            tensor_name: entry.name.clone(),
            payload_start: entry.payload_start,
            payload_length: length,
            role,
        });
    }
    hits
}

fn reverse_detail(layout: TensorLayout, shape: &[u64], rel: u64, length: u64) -> String {
    match layout {
        TensorLayout::Scalar(codec) => {
            let width = codec.width();
            let element = rel / width;
            let intra = rel % width;
            match coordinate_from_linear(shape, element) {
                Some(index) => {
                    let index_text = index
                        .iter()
                        .map(|i| i.to_string())
                        .collect::<Vec<_>>()
                        .join(",");
                    if intra == 0 {
                        format!("element [{index_text}]")
                    } else {
                        format!("byte {intra} inside element [{index_text}]")
                    }
                }
                None => format!("element {element}"),
            }
        }
        TensorLayout::Q4_0 => {
            if shape.len() != 2 || rel >= length {
                return format!("byte {rel} of the payload");
            }
            let block = rel / 18;
            let within = rel % 18;
            if within < 2 {
                format!("scale of block {block} (shared by its 32 values)")
            } else {
                let code_index = within - 2;
                format!(
                    "code byte {code_index} of block {block}: low nibble = element {}, high nibble = element {}",
                    block * 32 + code_index,
                    block * 32 + code_index + 16
                )
            }
        }
        TensorLayout::Unknown => format!("byte {rel} of the payload (layout unknown)"),
    }
}

/// Invert the row-major linear index into a coordinate, when shape knowledge
/// permits it (non-zero extents).
fn coordinate_from_linear(shape: &[u64], linear: u64) -> Option<Vec<u64>> {
    let mut index = Vec::with_capacity(shape.len());
    let mut remaining = linear;
    for &d in shape {
        if d == 0 {
            return None;
        }
    }
    // Row-major: last axis varies fastest.
    for &d in shape.iter().rev() {
        index.push(remaining % d);
        remaining /= d;
    }
    if remaining != 0 {
        return None;
    }
    index.reverse();
    Some(index)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bf16_reference_vector_from_the_spec() {
        // Shape [4096,4096], 2 bytes/element, tensor start 0x01000000,
        // coordinate [123,456] → 0x010F6390.
        let offset = dense_element_offset(&[4096, 4096], &[123, 456], 2, 0x0100_0000).unwrap();
        assert_eq!(offset, 0x010F_6390);
        let end = offset + 2;
        assert_eq!(end, 0x010F_6392);
    }

    #[test]
    fn non_square_dense_indices() {
        // [3,5] row-major: index [2,4] → linear 14 (last element).
        assert_eq!(dense_linear_index(&[3, 5], &[2, 4]).unwrap(), 14);
        assert_eq!(dense_linear_index(&[3, 5], &[1, 2]).unwrap(), 7);
        assert_eq!(dense_linear_index(&[4], &[3]).unwrap(), 3);
        // Rank zero has exactly one element at coordinate [].
        assert_eq!(dense_linear_index(&[], &[]).unwrap(), 0);
    }

    #[test]
    fn out_of_range_and_arity_errors() {
        assert!(dense_linear_index(&[3, 5], &[3, 0]).is_err());
        assert!(dense_linear_index(&[3, 5], &[0, 5]).is_err());
        assert!(dense_linear_index(&[3, 5], &[0]).is_err());
        assert!(dense_linear_index(&[3, 5], &[0, 0, 0]).is_err());
        assert!(dense_linear_index(&[0], &[0]).is_err());
    }

    #[test]
    fn overflow_is_rejected_never_wrapped() {
        let huge_shape = [u64::MAX, u64::MAX, 2];
        assert!(dense_element_offset(&huge_shape, &[1, 1, 1], 8, 0).is_err());
        assert!(dense_element_offset(&[u64::MAX], &[u64::MAX - 1], 16, u64::MAX - 8).is_err());
    }

    #[test]
    fn column_selection_is_strided_not_bounding() {
        // Column j of [4096,4096] F32: 4096 spans of 4 bytes with stride 16384.
        let first = dense_element_offset(&[4096, 4096], &[0, 456], 4, 0).unwrap();
        let second = dense_element_offset(&[4096, 4096], &[1, 456], 4, 0).unwrap();
        assert_eq!(first, 456 * 4);
        assert_eq!(second - first, 4096 * 4);
    }

    #[test]
    fn scalar_element_location() {
        let location = locate_scalar_element(&[2, 3], &[1, 2], ScalarCodec::F32, 100).unwrap();
        assert_eq!(location.precision, AddressPrecision::ExactContiguous);
        assert_eq!(location.byte_span, (120, 4));
        assert!(location.bit_mask.is_none());
        assert_eq!(location.decode_dependencies, vec![(120, 4)]);
    }

    #[test]
    fn q4_0_element_location_shares_scale() {
        // Element [0,0] of a [1,32] tensor at T: code byte T+2 low nibble,
        // scale dependency [T,T+2).
        let location = locate_q4_0_element(&[1, 32], &[0, 0], 500).unwrap();
        assert_eq!(location.precision, AddressPrecision::ExactBits);
        assert_eq!(location.byte_span, (502, 1));
        assert_eq!(location.bit_mask, Some((0x0F, 0)));
        assert_eq!(location.decode_dependencies, vec![(500, 2), (502, 1)]);

        // Element [1,16] of [2,32]: linear 48 → block 1 → scale [T+18,T+20),
        // code byte T+18+2 low nibble? u=16 → byte 2 of block, HIGH nibble.
        let location = locate_q4_0_element(&[2, 32], &[1, 16], 500).unwrap();
        assert_eq!(location.precision, AddressPrecision::ExactBits);
        assert_eq!(location.byte_span, (500 + 18 + 2, 1));
        assert_eq!(location.bit_mask, Some((0xF0, 4)));
        assert_eq!(
            location.decode_dependencies,
            vec![(500 + 18, 2), (500 + 18 + 2, 1)]
        );
    }

    #[test]
    fn whole_tensor_locations() {
        let location = locate_whole_tensor(1000, Some(200));
        assert_eq!(location.precision, AddressPrecision::ExactContiguous);
        assert_eq!(location.byte_span, (1000, 200));
        let empty = locate_whole_tensor(1000, Some(0));
        assert_eq!(empty.precision, AddressPrecision::NoPayload);
        let bounded = locate_whole_tensor(1000, None);
        assert_eq!(bounded.precision, AddressPrecision::Unresolved);
    }

    #[test]
    fn reverse_lookup_dense_and_q4_0() {
        // One F32 [2,2] tensor at 100 and one Q4_0 [1,32] tensor at 200.
        let tensors = vec![
            ReverseEntry {
                name: "dense.w".to_string(),
                payload_start: 100,
                payload_length: Some(16),
                layout: TensorLayout::Scalar(ScalarCodec::F32),
                shape: vec![2, 2],
            },
            ReverseEntry {
                name: "quant.w".to_string(),
                payload_start: 200,
                payload_length: Some(18),
                layout: TensorLayout::Q4_0,
                shape: vec![1, 32],
            },
        ];
        // Dense element [1,0] lives at 100 + 2*4 = 108.
        let hits = locate_offset(&tensors, 108);
        assert_eq!(hits.len(), 1);
        match &hits[0].role {
            ReverseRole::Inside { detail } => assert!(detail.contains("[1,0]"), "{detail}"),
            other => panic!("unexpected role {other:?}"),
        }
        // Q4_0 scale byte of block 0.
        let hits = locate_offset(&tensors, 200);
        match &hits[0].role {
            ReverseRole::AtStart => {}
            other => panic!("unexpected role {other:?}"),
        }
        let hits = locate_offset(&tensors, 201);
        assert!(hits[0].tensor_name == "quant.w");
        match &hits[0].role {
            ReverseRole::Inside { detail } => {
                assert!(detail.contains("scale of block 0"), "{detail}")
            }
            other => panic!("unexpected role {other:?}"),
        }
        // Code byte 0 of block 0 covers elements 0 and 16.
        let hits = locate_offset(&tensors, 202);
        match &hits[0].role {
            ReverseRole::Inside { detail } => {
                assert!(detail.contains("low nibble = element 0"), "{detail}");
                assert!(detail.contains("high nibble = element 16"), "{detail}");
            }
            other => panic!("unexpected role {other:?}"),
        }
        // Padding between tensors: no owner.
        assert!(locate_offset(&tensors, 99).is_empty());
        assert!(locate_offset(&tensors, 150).is_empty());
        // Just past the dense tensor end.
        let hits = locate_offset(&tensors, 116);
        assert!(matches!(hits[0].role, ReverseRole::AtEnd));
    }

    #[test]
    fn coordinate_round_trip_through_reverse() {
        // Non-square on purpose: square shapes can hide transposition bugs.
        let shape = [3, 5, 7];
        for linear in [0u64, 1, 6, 7, 34, 104] {
            let index = coordinate_from_linear(&shape, linear).unwrap();
            assert_eq!(dense_linear_index(&shape, &index).unwrap(), linear);
        }
        // Out-of-range linear (beyond 3*5*7-1=104) has no coordinate.
        assert!(coordinate_from_linear(&shape, 105).is_none());
    }
}
