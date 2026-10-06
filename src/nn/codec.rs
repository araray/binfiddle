//! Scalar and block codecs.
//!
//! A scalar codec specifies bit width, byte order, decoding behavior, and
//! raw-bit preservation. Decoding a value and showing its original bits are
//! separate operations: NaN payloads, signaling/quiet distinctions, and signed
//! zeros survive in the raw bits even when the decoded float is ordinary.
//!
//! The Q4_0 block codec implements the reference mapping: 32 values per block,
//! a two-byte little-endian F16 scale, then 16 packed code bytes whose LOW
//! nibbles address the first 16 logical values and HIGH nibbles the second 16.
//! Bits are LSB0 within each byte; the paired values in one byte are therefore
//! 16 logical positions apart, not adjacent.

use super::error::NnError;

/// Scalar codec identifiers (container-qualified families).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScalarCodec {
    F64,
    F32,
    F16,
    Bf16,
    I64,
    I32,
    I16,
    I8,
    U8,
    Bool,
}

impl ScalarCodec {
    /// Element byte width.
    pub fn width(self) -> u64 {
        match self {
            ScalarCodec::F64 | ScalarCodec::I64 => 8,
            ScalarCodec::F32 | ScalarCodec::I32 => 4,
            ScalarCodec::F16 | ScalarCodec::Bf16 | ScalarCodec::I16 => 2,
            ScalarCodec::I8 | ScalarCodec::U8 | ScalarCodec::Bool => 1,
        }
    }

    /// Decode one little-endian element. `bytes.len()` must equal the width.
    pub fn decode(self, bytes: &[u8]) -> Result<Scalar, NnError> {
        if bytes.len() as u64 != self.width() {
            return Err(NnError::CodecUnsupported {
                codec: format!("{self:?}"),
                operation: "decode".to_string(),
                reason: format!("expected {} bytes, got {}", self.width(), bytes.len()),
            });
        }
        let raw_bits = hex::encode(bytes);
        let value = match self {
            ScalarCodec::F64 => {
                ScalarValue::Float(f64::from_le_bytes(bytes.try_into().expect("width checked")))
            }
            ScalarCodec::F32 => ScalarValue::Float(f32::from_le_bytes(
                bytes.try_into().expect("width checked"),
            ) as f64),
            ScalarCodec::F16 => {
                let bits = u16::from_le_bytes(bytes.try_into().expect("width checked"));
                ScalarValue::Float(f16_to_f64(bits))
            }
            ScalarCodec::Bf16 => {
                let bits = u16::from_le_bytes(bytes.try_into().expect("width checked"));
                ScalarValue::Float(bf16_to_f64(bits))
            }
            ScalarCodec::I64 => {
                ScalarValue::Int(i64::from_le_bytes(bytes.try_into().expect("width checked")))
            }
            ScalarCodec::I32 => {
                ScalarValue::Int(i32::from_le_bytes(bytes.try_into().expect("width checked")) as i64)
            }
            ScalarCodec::I16 => {
                ScalarValue::Int(i16::from_le_bytes(bytes.try_into().expect("width checked")) as i64)
            }
            ScalarCodec::I8 => ScalarValue::Int(bytes[0] as i8 as i64),
            ScalarCodec::U8 => ScalarValue::Uint(bytes[0] as u64),
            ScalarCodec::Bool => ScalarValue::Bool(bytes[0] != 0),
        };
        Ok(Scalar { raw_bits, value })
    }
}

/// A decoded scalar with its original bits preserved.
#[derive(Debug, Clone, PartialEq)]
pub struct Scalar {
    /// Lowercase hex of the stored little-endian bytes.
    pub raw_bits: String,
    pub value: ScalarValue,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ScalarValue {
    Float(f64),
    Int(i64),
    Uint(u64),
    Bool(bool),
}

impl ScalarValue {
    /// Render for reports. Non-finite floats render as category names, never
    /// as invalid JSON literals.
    pub fn render(self) -> String {
        match self {
            ScalarValue::Float(f) => {
                if f.is_nan() {
                    "NaN".to_string()
                } else if f.is_infinite() {
                    if f > 0.0 {
                        "+Inf".to_string()
                    } else {
                        "-Inf".to_string()
                    }
                } else {
                    format!("{f}")
                }
            }
            ScalarValue::Int(i) => i.to_string(),
            ScalarValue::Uint(u) => u.to_string(),
            ScalarValue::Bool(b) => b.to_string(),
        }
    }
}

/// IEEE 754 binary16 → f64 (all values exactly representable).
pub fn f16_to_f64(bits: u16) -> f64 {
    let sign = ((bits >> 15) & 1) as u64;
    let exponent = ((bits >> 10) & 0x1F) as u64;
    let fraction = (bits & 0x3FF) as u64;
    let value = match exponent {
        0 => {
            // Subnormal or zero: fraction/1024 * 2^-14.
            (fraction as f64) * (2f64).powi(-24)
        }
        0x1F => {
            if fraction == 0 {
                f64::INFINITY
            } else {
                f64::NAN
            }
        }
        e => {
            // Normalized: 1.fraction * 2^(e-15), exactly representable in f64.
            let mantissa = (0x400 + fraction) as f64; // 10 explicit bits
            mantissa * (2f64).powi(e as i32 - 25)
        }
    };
    if sign == 1 {
        -value
    } else {
        value
    }
}

/// bfloat16 → f64: the low 16 bits of an f32.
pub fn bf16_to_f64(bits: u16) -> f64 {
    let f32_bits = (bits as u32) << 16;
    f32::from_bits(f32_bits) as f64
}

// ---- Q4_0 block codec ----

/// Q4_0 geometry: 32 logical values per block, 18 bytes per block (2-byte F16
/// scale + 16 code bytes).
pub const Q4_0_ELEMENTS_PER_BLOCK: u64 = 32;
pub const Q4_0_BLOCK_BYTES: u64 = 18;
pub const Q4_0_SCALE_BYTES: u64 = 2;

/// Storage layout knowledge for an encoding id.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TensorLayout {
    Scalar(ScalarCodec),
    Q4_0,
    /// Encoding with no qualified layout knowledge.
    Unknown,
}

/// Resolve the storage layout of an encoding id (e.g. from a catalog).
pub fn layout_for_encoding(encoding: &str) -> TensorLayout {
    match encoding {
        "safetensors.F64" | "ggml.f64" => TensorLayout::Scalar(ScalarCodec::F64),
        "safetensors.F32" | "ggml.f32" => TensorLayout::Scalar(ScalarCodec::F32),
        "safetensors.F16" | "ggml.f16" => TensorLayout::Scalar(ScalarCodec::F16),
        "safetensors.BF16" => TensorLayout::Scalar(ScalarCodec::Bf16),
        "safetensors.I64" => TensorLayout::Scalar(ScalarCodec::I64),
        "safetensors.I32" => TensorLayout::Scalar(ScalarCodec::I32),
        "safetensors.I16" => TensorLayout::Scalar(ScalarCodec::I16),
        "safetensors.I8" => TensorLayout::Scalar(ScalarCodec::I8),
        "safetensors.U8" => TensorLayout::Scalar(ScalarCodec::U8),
        "safetensors.BOOL" => TensorLayout::Scalar(ScalarCodec::Bool),
        // ONNX dtypes with exact codec counterparts. Wide unsigned types
        // (uint16/32/64) and strings have no exact scalar codec and stay
        // Unknown — descriptor-visible, addressing unsupported.
        "onnx.float" => TensorLayout::Scalar(ScalarCodec::F32),
        "onnx.double" => TensorLayout::Scalar(ScalarCodec::F64),
        "onnx.float16" => TensorLayout::Scalar(ScalarCodec::F16),
        "onnx.bfloat16" => TensorLayout::Scalar(ScalarCodec::Bf16),
        "onnx.int64" => TensorLayout::Scalar(ScalarCodec::I64),
        "onnx.int32" => TensorLayout::Scalar(ScalarCodec::I32),
        "onnx.int16" => TensorLayout::Scalar(ScalarCodec::I16),
        "onnx.int8" => TensorLayout::Scalar(ScalarCodec::I8),
        "onnx.uint8" => TensorLayout::Scalar(ScalarCodec::U8),
        "onnx.bool" => TensorLayout::Scalar(ScalarCodec::Bool),
        // Torch storage types with exact codec counterparts (read tier;
        // the torch descriptor tier is read-only — edits are refused).
        "torch.float64" => TensorLayout::Scalar(ScalarCodec::F64),
        "torch.float32" => TensorLayout::Scalar(ScalarCodec::F32),
        "torch.float16" => TensorLayout::Scalar(ScalarCodec::F16),
        "torch.bfloat16" => TensorLayout::Scalar(ScalarCodec::Bf16),
        "torch.int64" => TensorLayout::Scalar(ScalarCodec::I64),
        "torch.int32" => TensorLayout::Scalar(ScalarCodec::I32),
        "torch.int16" => TensorLayout::Scalar(ScalarCodec::I16),
        "torch.int8" => TensorLayout::Scalar(ScalarCodec::I8),
        "torch.uint8" => TensorLayout::Scalar(ScalarCodec::U8),
        "torch.bool" => TensorLayout::Scalar(ScalarCodec::Bool),
        "ggml.q4_0" => TensorLayout::Q4_0,
        _ => TensorLayout::Unknown,
    }
}

/// One decoded Q4_0 block.
#[derive(Debug, Clone, PartialEq)]
pub struct Q4Block {
    /// Decoded scale (F16 → f64).
    pub scale: f64,
    /// Raw scale bytes (hex) preserving the original F16 bits.
    pub scale_raw: String,
    /// Decoded 32 values, in logical order.
    pub values: [f64; 32],
}

/// Decode one 18-byte block.
pub fn q4_0_decode_block(block: &[u8]) -> Result<Q4Block, NnError> {
    if block.len() != Q4_0_BLOCK_BYTES as usize {
        return Err(NnError::CodecUnsupported {
            codec: "ggml.q4_0".to_string(),
            operation: "decode".to_string(),
            reason: format!(
                "block must be {} bytes, got {}",
                Q4_0_BLOCK_BYTES,
                block.len()
            ),
        });
    }
    let scale = f16_to_f64(u16::from_le_bytes([block[0], block[1]]));
    let mut values = [0f64; 32];
    for u in 0..32usize {
        let code_byte = block[2 + (u % 16)];
        let shift = if u < 16 { 0 } else { 4 };
        let q = (code_byte >> shift) & 0xF;
        values[u] = scale * (q as f64 - 8.0);
    }
    Ok(Q4Block {
        scale,
        scale_raw: hex::encode(&block[..2]),
        values,
    })
}

/// Where element `u` of a block lives: code byte index within the block,
/// nibble mask, and shift. Bits are LSB0 within each byte.
pub fn q4_0_code_location(u: u64) -> (u64, u8, u8) {
    let byte_index = 2 + (u % 16);
    if u < 16 {
        (byte_index, 0x0F, 0)
    } else {
        (byte_index, 0xF0, 4)
    }
}

/// The block index of element (row-major `linear`) in a tensor with
/// `elements_per_row` per row. Rows must be 32-divisible for Q4_0.
pub fn q4_0_block_index(linear: u64, elements_per_row: u64) -> Result<u64, NnError> {
    if elements_per_row == 0 || !elements_per_row.is_multiple_of(Q4_0_ELEMENTS_PER_BLOCK) {
        return Err(NnError::MalformedInput {
            detail: format!("Q4_0 requires 32-divisible rows, got {elements_per_row}"),
        });
    }
    let blocks_per_row = elements_per_row / Q4_0_ELEMENTS_PER_BLOCK;
    let row = linear / elements_per_row;
    let column = linear % elements_per_row;
    Ok(row * blocks_per_row + column / Q4_0_ELEMENTS_PER_BLOCK)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn f32_decode_round_trip() {
        let scalar = ScalarCodec::F32.decode(&1.0f32.to_le_bytes()).unwrap();
        assert_eq!(scalar.value, ScalarValue::Float(1.0));
        assert_eq!(scalar.raw_bits, "0000803f");
    }

    #[test]
    fn f16_reference_values() {
        // F16 0.5 = 0x3800 → little-endian bytes 00 38 (the corpus fixture scale).
        let scalar = ScalarCodec::F16.decode(&[0x00, 0x38]).unwrap();
        assert_eq!(scalar.value, ScalarValue::Float(0.5));
        assert_eq!(scalar.raw_bits, "0038");
        // F16 1.0 = 0x3C00, -2.0 = 0xC000, 8.0 = 0x4800.
        assert_eq!(f16_to_f64(0x3C00), 1.0);
        assert_eq!(f16_to_f64(0xC000), -2.0);
        assert_eq!(f16_to_f64(0x4800), 8.0);
        // Subnormal and specials.
        assert_eq!(f16_to_f64(0), 0.0);
        assert_eq!(f16_to_f64(1), (2f64).powi(-24)); // smallest subnormal
        assert!(f16_to_f64(0x7C00).is_infinite());
        assert!(f16_to_f64(0x7C01).is_nan());
    }

    #[test]
    fn bf16_reference_values() {
        // BF16 0.5 = top half of f32 0.5 (0x3F000000) → 0x3F00.
        assert_eq!(bf16_to_f64(0x3F00), 0.5);
        assert_eq!(bf16_to_f64(0x3F80), 1.0);
        assert!(bf16_to_f64(0x7F80).is_infinite());
        assert!(bf16_to_f64(0x7FC0).is_nan());
    }

    #[test]
    fn integer_and_bool_codecs() {
        assert_eq!(
            ScalarCodec::U8.decode(&[0xFF]).unwrap().value,
            ScalarValue::Uint(255)
        );
        assert_eq!(
            ScalarCodec::I8.decode(&[0xFF]).unwrap().value,
            ScalarValue::Int(-1)
        );
        assert_eq!(
            ScalarCodec::I16
                .decode(&0x8000u16.to_le_bytes())
                .unwrap()
                .value,
            ScalarValue::Int(-32768)
        );
        assert_eq!(
            ScalarCodec::I32
                .decode(&(-5i32).to_le_bytes())
                .unwrap()
                .value,
            ScalarValue::Int(-5)
        );
        assert_eq!(
            ScalarCodec::I64
                .decode(&i64::MIN.to_le_bytes())
                .unwrap()
                .value,
            ScalarValue::Int(i64::MIN)
        );
        assert_eq!(
            ScalarCodec::Bool.decode(&[1]).unwrap().value,
            ScalarValue::Bool(true)
        );
    }

    #[test]
    fn nan_payload_survives_in_raw_bits() {
        // F32 with a distinctive NaN payload.
        let bits = 0x7F_A0_00_01u32;
        let scalar = ScalarCodec::F32.decode(&bits.to_le_bytes()).unwrap();
        assert!(matches!(scalar.value, ScalarValue::Float(f) if f.is_nan()));
        // Raw bits are the little-endian byte sequence: 01 00 a0 7f.
        assert_eq!(scalar.raw_bits, format!("{:08x}", bits.swap_bytes()));
        assert_eq!(ScalarValue::Float(f64::NAN).render(), "NaN");
        assert_eq!(ScalarValue::Float(f64::INFINITY).render(), "+Inf");
    }

    #[test]
    fn wrong_width_is_rejected() {
        assert!(ScalarCodec::F32.decode(&[0, 1]).is_err());
        assert!(ScalarCodec::U8.decode(&[]).is_err());
    }

    #[test]
    fn layout_lookup_matches_registry() {
        assert_eq!(
            layout_for_encoding("safetensors.F32"),
            TensorLayout::Scalar(ScalarCodec::F32)
        );
        assert_eq!(layout_for_encoding("ggml.q4_0"), TensorLayout::Q4_0);
        assert_eq!(layout_for_encoding("gguf.type14"), TensorLayout::Unknown);
        assert_eq!(
            layout_for_encoding("ggml.f16"),
            TensorLayout::Scalar(ScalarCodec::F16)
        );
    }

    /// The corpus 18-byte fixture: scale 0.5 (`00 38`), first code byte `A3`.
    #[test]
    fn q4_0_corpus_fixture_values() {
        let mut block = [0u8; 18];
        block[0] = 0x00;
        block[1] = 0x38; // F16 0.5
        block[2] = 0xA3; // low nibble 3, high nibble 10 (0xA)
        let decoded = q4_0_decode_block(&block).unwrap();
        assert_eq!(decoded.scale, 0.5);
        assert_eq!(decoded.scale_raw, "0038");
        // value[0] = (3-8)*0.5 = -2.5 ; value[16] = (10-8)*0.5 = +1.0
        assert_eq!(decoded.values[0], -2.5);
        assert_eq!(decoded.values[16], 1.0);
        // All other codes are zero → (0-8)*0.5 = -4.0.
        assert_eq!(decoded.values[1], -4.0);
        assert_eq!(decoded.values[31], -4.0);
    }

    #[test]
    fn q4_0_all_codes_for_one_scale() {
        // Code byte enumerating every low nibble.
        for q in 0u8..16 {
            let mut block = [0u8; 18];
            block[0] = 0x00;
            block[1] = 0x38;
            block[2] = q;
            let decoded = q4_0_decode_block(&block).unwrap();
            assert_eq!(decoded.values[0], (q as f64 - 8.0) * 0.5, "code {q}");
        }
    }

    #[test]
    fn q4_0_negative_scale() {
        // F16 -0.5 = 0xB800 → bytes 00 B8.
        let mut block = [0u8; 18];
        block[0] = 0x00;
        block[1] = 0xB8;
        block[2] = 0x0B; // low nibble 11 → (11-8)=3
        let decoded = q4_0_decode_block(&block).unwrap();
        assert_eq!(decoded.scale, -0.5);
        assert_eq!(decoded.values[0], -1.5);
    }

    #[test]
    fn q4_0_code_location_nibbles() {
        // Element 0: code byte 2, low nibble.
        assert_eq!(q4_0_code_location(0), (2, 0x0F, 0));
        // Element 16 shares byte 2 at the high nibble.
        assert_eq!(q4_0_code_location(16), (2, 0xF0, 4));
        assert_eq!(q4_0_code_location(15), (17, 0x0F, 0));
        assert_eq!(q4_0_code_location(31), (17, 0xF0, 4));
    }

    #[test]
    fn q4_0_block_index_rows_and_columns() {
        // Two rows of 32: row 1 col 5 → linear 37 → block 1 (column block 0).
        assert_eq!(q4_0_block_index(37, 32).unwrap(), 1);
        // One row of 64: linear 40 → second block of row 0.
        assert_eq!(q4_0_block_index(40, 64).unwrap(), 1);
        assert_eq!(q4_0_block_index(31, 64).unwrap(), 0);
        // Non-32-divisible rows are malformed for this codec.
        assert!(q4_0_block_index(0, 30).is_err());
    }

    #[test]
    fn q4_0_block_size_enforced() {
        assert!(q4_0_decode_block(&[0u8; 17]).is_err());
        assert!(q4_0_decode_block(&[0u8; 19]).is_err());
    }
}
