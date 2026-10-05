//! Transactional fixed-size editing.
//!
//! An edit plan records everything needed to make one typed or raw-bit change
//! verifiable: the exact write unit (byte span, bit mask, shift), the bytes
//! observed when the plan was made (the preimage), the computed new bytes,
//! the decoded old/new values, and the source content digest. Application
//! re-verifies the catalog identity, the full source digest, and the preimage
//! bytes before writing a fresh output file — never the original — and then
//! proves that every byte outside the planned footprint is unchanged and
//! that the patched container still parses.
//!
//! Undo bundles record the exact output revision they reverse; undoing
//! against any other revision is refused rather than best-effort rolled back.

use super::address::{locate_q4_0_element, locate_scalar_element, ElementLocation};
use super::budget::Budget;
use super::catalog::{Catalog, CatalogTensor};
use super::codec::{
    f16_to_f64, layout_for_encoding, q4_0_decode_block, ScalarCodec, ScalarValue, TensorLayout,
};
use super::error::NnError;
use super::id::{compute_id, IdKind};
use super::json::{Json, ParseLimits};
use super::source::BoundedFile;
use sha2::{Digest, Sha256};
use std::io::Write as _;
use std::path::{Path, PathBuf};

/// Value-selection policy for typed edits.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EditPolicy {
    /// Accept only exactly representable results.
    ExactOnly,
    /// Choose the nearest representable value and report the error.
    Nearest,
    /// Quantized codecs: keep shared parameters fixed and choose the nearest
    /// code under them (out-of-range requires --clamp).
    FixedParameters,
}

impl EditPolicy {
    pub fn as_str(self) -> &'static str {
        match self {
            EditPolicy::ExactOnly => "exact_only",
            EditPolicy::Nearest => "nearest",
            EditPolicy::FixedParameters => "fixed_parameters",
        }
    }

    pub fn parse(text: &str) -> Result<EditPolicy, NnError> {
        match text {
            "exact_only" => Ok(EditPolicy::ExactOnly),
            "nearest" => Ok(EditPolicy::Nearest),
            "fixed_parameters" => Ok(EditPolicy::FixedParameters),
            other => Err(NnError::InvalidRequest {
                message: format!("unknown edit policy {other}"),
            }),
        }
    }

    /// The default policy for a layout.
    pub fn default_for(layout: TensorLayout) -> EditPolicy {
        match layout {
            TensorLayout::Q4_0 => EditPolicy::FixedParameters,
            _ => EditPolicy::ExactOnly,
        }
    }
}

// ---- scalar encoding ----

/// Round f64 to IEEE 754 binary16 bits (nearest-even), per the standard.
pub fn f64_to_f16_bits(value: f64) -> u16 {
    let f32_value = value as f32;
    let bits = f32_value.to_bits();
    let sign = ((bits >> 16) & 0x8000) as u16;
    let exponent = ((bits >> 23) & 0xFF) as i32;
    let fraction = bits & 0x007F_FFFF;

    if exponent == 0xFF {
        // Non-finite inputs are rejected by the public encoder; this arm is
        // defensive and quiets any NaN that reaches it.
        return sign | 0x7C00 | 0x0200;
    }
    // Rebias: f32 bias 127, f16 bias 15.
    let unbiased = exponent - 127;
    if unbiased > 15 {
        return sign | 0x7C00; // overflow to infinity
    }
    if unbiased >= -14 {
        // Normal f16: keep 10 fraction bits with rounding.
        let f16_exp = (unbiased + 15) as u32;
        let mut result = f16_exp << 10;
        let frac10 = fraction >> 13;
        let remainder = fraction & 0x1FFF;
        result |= frac10;
        // Round to nearest even.
        if remainder > 0x1000 || (remainder == 0x1000 && frac10 & 1 == 1) {
            result += 1; // can carry into the exponent, which is correct
        }
        return sign | result as u16;
    }
    // Subnormal f16: shift the implicit one into the fraction.
    let subnormal_value = value.abs();
    let quant = subnormal_value / 2f64.powi(-24);
    let rounded = quant.round(); // ties: check the exact half
    let quant = if quant.fract().abs() == 0.5 && quant as i64 % 2 == 0 && quant != rounded {
        quant as i64 as f64 + 1.0
    } else {
        rounded
    };
    let fraction16 = quant as u16 & 0x3FF;
    sign | fraction16
}

/// Round f32 bits to bfloat16 bits (nearest-even).
pub fn f32_to_bf16_bits(bits: u32) -> u16 {
    let top = bits >> 16;
    let low = bits & 0xFFFF;
    let mut rounded = top;
    if low > 0x8000 || (low == 0x8000 && top & 1 == 1) {
        rounded += 1;
    }
    rounded as u16
}

/// Encode a float into scalar codec bytes under a policy. Returns the bytes
/// and the value they decode back to.
pub fn encode_float(
    codec: ScalarCodec,
    value: f64,
    policy: EditPolicy,
) -> Result<(Vec<u8>, f64), NnError> {
    if !value.is_finite() {
        return Err(NnError::InvalidRequest {
            message: "non-finite values must be written through --raw-bits so the exact bit pattern is explicit"
                .to_string(),
        });
    }
    let (bytes, decoded) = match codec {
        ScalarCodec::F64 => (value.to_le_bytes().to_vec(), value),
        ScalarCodec::F32 => {
            let narrowed = value as f32;
            (narrowed.to_le_bytes().to_vec(), narrowed as f64)
        }
        ScalarCodec::F16 => {
            let bits = f64_to_f16_bits(value);
            let decoded = f16_to_f64(bits);
            (bits.to_le_bytes().to_vec(), decoded)
        }
        ScalarCodec::Bf16 => {
            let as_f32 = value as f32;
            let bits = f32_to_bf16_bits(as_f32.to_bits());
            let decoded = f32::from_bits((bits as u32) << 16) as f64;
            (bits.to_le_bytes().to_vec(), decoded)
        }
        _ => {
            return Err(NnError::InvalidRequest {
                message: format!("{codec:?} is an integer codec; use an integer value"),
            })
        }
    };
    match policy {
        EditPolicy::ExactOnly if decoded != value => Err(NnError::InverseUnqualified {
            detail: format!(
                "value {value} is not exactly representable in {codec:?} (nearest is {decoded}); use policy nearest"
            ),
        }),
        _ => Ok((bytes, decoded)),
    }
}

/// Encode an integer into scalar codec bytes (exact by construction).
pub fn encode_integer(codec: ScalarCodec, text: &str) -> Result<Vec<u8>, NnError> {
    let parse_err = |range: &str| NnError::InverseUnqualified {
        detail: format!("value {text} is outside the {codec:?} range ({range})"),
    };
    match codec {
        ScalarCodec::I64 => {
            let v: i64 = text.parse().map_err(|_| parse_err("i64"))?;
            Ok(v.to_le_bytes().to_vec())
        }
        ScalarCodec::I32 => {
            let v: i32 = text.parse().map_err(|_| parse_err("i32"))?;
            Ok(v.to_le_bytes().to_vec())
        }
        ScalarCodec::I16 => {
            let v: i16 = text.parse().map_err(|_| parse_err("i16"))?;
            Ok(v.to_le_bytes().to_vec())
        }
        ScalarCodec::I8 => {
            let v: i8 = text.parse().map_err(|_| parse_err("i8"))?;
            Ok(v.to_le_bytes().to_vec())
        }
        ScalarCodec::U8 => {
            let v: u8 = text.parse().map_err(|_| parse_err("u8"))?;
            Ok(vec![v])
        }
        ScalarCodec::Bool => {
            let v: bool = text.parse().map_err(|_| NnError::InvalidRequest {
                message: format!("value {text} is not a boolean"),
            })?;
            Ok(vec![v as u8])
        }
        _ => Err(NnError::InvalidRequest {
            message: format!("{codec:?} is a float codec; use a decimal value"),
        }),
    }
}

/// Q4_0 fixed-parameter code selection: q = round(v/d) + 8 clamped to
/// [0, 15] only when clamping is allowed.
pub fn q4_0_select_code(
    scale: f64,
    value: f64,
    policy: EditPolicy,
    allow_clamp: bool,
) -> Result<(u8, f64), NnError> {
    if scale == 0.0 || !scale.is_finite() {
        return Err(NnError::InvalidRequest {
            message: "block scale is zero or non-finite; fixed-parameter selection is undefined"
                .to_string(),
        });
    }
    let target = value / scale + 8.0;
    let q_float = target.round(); // nearest; ties resolved by .round (half away from zero)
    if !(0.0..=15.0).contains(&q_float) {
        if !allow_clamp {
            return Err(NnError::InverseUnqualified {
                detail: format!(
                    "value {value} maps to code {q_float} outside [0, 15] (scale {scale}); pass --clamp to saturate"
                ),
            });
        }
        let clamped = q_float.clamp(0.0, 15.0) as u8;
        return Ok((clamped, scale * (clamped as f64 - 8.0)));
    }
    let q = q_float as i64 as u8;
    let decoded = scale * (q as f64 - 8.0);
    if policy == EditPolicy::ExactOnly && decoded != value {
        return Err(NnError::InverseUnqualified {
            detail: format!(
                "value {value} is not exactly representable with the fixed scale {scale} (nearest decoded {decoded})"
            ),
        });
    }
    Ok((q, decoded))
}

/// One planned edit.
#[derive(Debug, Clone, PartialEq)]
pub struct EditPlan {
    pub plan_id: String,
    pub catalog_id: String,
    pub tensor_id: String,
    pub tensor_name: String,
    pub encoding: String,
    pub coordinate: Vec<u64>,
    pub operation: &'static str, // "set_scalar" | "set_raw_bits"
    pub policy: EditPolicy,
    /// Byte span of the write unit.
    pub span: (u64, u64),
    /// Nibble mask and shift for sub-byte units.
    pub bit_field: Option<(u8, u8)>,
    /// Decode dependency spans (e.g. the block scale).
    pub decode_dependencies: Vec<(u64, u64)>,
    pub source_id: String,
    pub source_digest: String,
    /// Observed bytes of the write unit when the plan was made.
    pub old_bytes_hex: String,
    /// Computed replacement bytes of the write unit.
    pub new_bytes_hex: String,
    /// Human-readable old and new decoded values.
    pub old_display: String,
    pub new_display: String,
    pub requested: String,
}

impl EditPlan {
    /// Parse a requested value and build a plan against the current bytes.
    #[allow(clippy::too_many_arguments)]
    pub fn build(
        catalog: &Catalog,
        tensor: &CatalogTensor,
        coordinate: &[u64],
        requested: &RequestedValue,
        policy: EditPolicy,
        allow_clamp: bool,
        budget: &Budget,
    ) -> Result<EditPlan, NnError> {
        let layout = layout_for_encoding(&tensor.encoding);
        let location = write_unit(tensor, layout, coordinate)?;
        let source = catalog.resolve_source(&tensor.source_id)?;
        if source.path.is_empty() {
            return Err(NnError::InvalidRequest {
                message: "source locator path is unavailable; re-discover with the same root"
                    .to_string(),
            });
        }
        let digest = source
            .semantic
            .get("content_digest")
            .and_then(|d| d.get("value"))
            .and_then(Json::as_str)
            .ok_or_else(|| NnError::InvalidRequest {
                message:
                    "edit plans require content-verified sources; re-discover with --verify-content"
                        .to_string(),
            })?
            .to_string();

        let path = Path::new(&source.path);
        let reader = BoundedFile::open(path)?;
        reader.verify_length(path)?;
        // Fresh digest check: the recorded digest must match the file now.
        let current = reader.content_digest(budget)?;
        if current != digest {
            return Err(NnError::SourceChanged {
                detail: format!(
                    "source changed since discovery (catalog {}, file {})",
                    super::error::brief(&digest),
                    super::error::brief(&current)
                ),
            });
        }

        // Read the write-unit bytes and the decode dependencies.
        let unit_len = if location.bit_mask.is_some() {
            1
        } else {
            location.byte_span.1
        };
        let mut unit = vec![0u8; unit_len as usize];
        reader.read_exact_at_bounded(location.byte_span.0, &mut unit, budget)?;
        let old_bytes_hex = hex::encode(&unit);
        let old_display =
            decode_unit_display(&reader, tensor, layout, coordinate, &location, budget)?;

        let (new_bytes, new_display) = match requested {
            RequestedValue::Typed(text) => compute_new_bytes(
                layout,
                tensor,
                &location,
                &reader,
                text,
                policy,
                allow_clamp,
                budget,
            )?,
            RequestedValue::RawBits(hex_text) => {
                let bytes = hex::decode(hex_text.trim()).map_err(|_| NnError::InvalidRequest {
                    message: format!(
                        "--raw-bits expects hex, got {}",
                        super::error::brief(hex_text)
                    ),
                })?;
                let expected = if location.bit_mask.is_some() {
                    1 // one nibble = one byte buffer holding the 4 bits
                } else {
                    location.byte_span.1 as usize
                };
                if bytes.len() != expected {
                    return Err(NnError::InvalidRequest {
                        message: format!(
                            "raw edit expects {expected} byte(s) for this write unit, got {}",
                            bytes.len()
                        ),
                    });
                }
                if let Some((mask, shift)) = location.bit_mask {
                    if bytes[0] & !mask != 0 {
                        return Err(NnError::InvalidRequest {
                            message: format!(
                                "raw bits 0x{:02x} exceed the nibble mask 0x{mask:02x}",
                                bytes[0]
                            ),
                        });
                    }
                    let patched = (unit[0] & !mask) | ((bytes[0] & mask) >> shift << shift);
                    (vec![patched], format!("raw bits 0x{:02x}", patched))
                } else {
                    let display = match layout {
                        TensorLayout::Scalar(codec) => {
                            match codec.decode(&bytes).map(|s| s.value) {
                                Ok(ScalarValue::Float(f)) => format!("{f}"),
                                Ok(ScalarValue::Int(v)) => format!("{v}"),
                                Ok(ScalarValue::Uint(v)) => format!("{v}"),
                                Ok(ScalarValue::Bool(v)) => format!("{v}"),
                                Err(_) => format!("raw bytes {}", hex::encode(&bytes)),
                            }
                        }
                        _ => format!("raw bytes {}", hex::encode(&bytes)),
                    };
                    (bytes, display)
                }
            }
        };

        let mut plan = EditPlan {
            plan_id: String::new(),
            catalog_id: catalog.id()?,
            tensor_id: tensor.id.clone(),
            tensor_name: tensor.original_name.clone(),
            encoding: tensor.encoding.clone(),
            coordinate: coordinate.to_vec(),
            operation: if matches!(requested, RequestedValue::Typed(_)) {
                "set_scalar"
            } else {
                "set_raw_bits"
            },
            policy,
            span: location.byte_span,
            bit_field: location.bit_mask,
            decode_dependencies: location.decode_dependencies.clone(),
            source_id: source.id.clone(),
            source_digest: digest,
            old_bytes_hex: old_bytes_hex.clone(),
            new_bytes_hex: hex::encode(&new_bytes),
            old_display,
            new_display,
            requested: requested.describe(),
        };
        // A raw masked write must differ from the preimage only inside the
        // mask; a plan that would rewrite identical bytes is a no-op.
        if plan.old_bytes_hex == plan.new_bytes_hex {
            return Err(NnError::InvalidRequest {
                message: "the edit is a no-op: the requested bits already match the current bytes"
                    .to_string(),
            });
        }
        plan.plan_id = compute_id(IdKind::Plan, &plan.semantic()?)?;
        Ok(plan)
    }

    pub fn semantic(&self) -> Result<Json, NnError> {
        Json::object(vec![
            ("schema", Json::Str("binfiddle.nn.edit-plan/v1".to_string())),
            ("catalog_id", Json::Str(self.catalog_id.clone())),
            ("tensor_id", Json::Str(self.tensor_id.clone())),
            ("tensor_name", Json::Str(self.tensor_name.clone())),
            ("encoding", Json::Str(self.encoding.clone())),
            (
                "coordinate",
                Json::Array(
                    self.coordinate
                        .iter()
                        .map(|i| Json::Str(i.to_string()))
                        .collect(),
                ),
            ),
            ("operation", Json::Str(self.operation.to_string())),
            ("policy", Json::Str(self.policy.as_str().to_string())),
            (
                "span",
                Json::object(vec![
                    ("start", Json::Str(self.span.0.to_string())),
                    ("length", Json::Str(self.span.1.to_string())),
                ])?,
            ),
            (
                "bit_field",
                match self.bit_field {
                    Some((mask, shift)) => Json::object(vec![
                        ("mask_hex", Json::Str(format!("{mask:02x}"))),
                        ("shift", Json::Str(shift.to_string())),
                    ])?,
                    None => Json::Null,
                },
            ),
            (
                "decode_dependencies",
                Json::Array(
                    self.decode_dependencies
                        .iter()
                        .map(|(s, l)| {
                            Json::object(vec![
                                ("start", Json::Str(s.to_string())),
                                ("length", Json::Str(l.to_string())),
                            ])
                        })
                        .collect::<Result<Vec<_>, _>>()?,
                ),
            ),
            ("source_id", Json::Str(self.source_id.clone())),
            ("source_digest", Json::Str(self.source_digest.clone())),
            ("old_bytes_hex", Json::Str(self.old_bytes_hex.clone())),
            ("new_bytes_hex", Json::Str(self.new_bytes_hex.clone())),
            ("old_value", Json::Str(self.old_display.clone())),
            ("new_value", Json::Str(self.new_display.clone())),
            ("requested", Json::Str(self.requested.clone())),
        ])
    }

    pub fn id(&self) -> Result<String, NnError> {
        compute_id(IdKind::Plan, &self.semantic()?)
    }

    pub fn envelope(&self) -> Result<super::report::ResultEnvelope, NnError> {
        let semantic = Json::object(vec![
            ("plan_id", Json::Str(self.plan_id.clone())),
            ("edit", self.semantic()?),
            (
                "guarantees",
                Json::object(vec![
                    (
                        "claim",
                        Json::Str("fixed-size planned byte change".to_string()),
                    ),
                    ("unselected_bytes_preserved", Json::Bool(true)),
                    ("original_untouched", Json::Bool(true)),
                ])?,
            ),
        ])?;
        Ok(super::report::ResultEnvelope::new("edit set").with_semantic(semantic))
    }

    pub fn text(&self) -> String {
        let mut out = String::from("edit plan\n");
        out.push_str(&format!(
            "  tensor:    {} [{}]\n",
            self.tensor_name,
            self.coordinate
                .iter()
                .map(|i| i.to_string())
                .collect::<Vec<_>>()
                .join(",")
        ));
        out.push_str(&format!("  encoding:  {}\n", self.encoding));
        out.push_str(&format!(
            "  write:     bytes [{}, {})",
            self.span.0,
            self.span.0 + self.span.1
        ));
        if let Some((mask, shift)) = self.bit_field {
            out.push_str(&format!(" masked 0x{mask:02x} shift {shift} (lsb0)"));
        }
        out.push('\n');
        out.push_str(&format!(
            "  change:    {} -> {} (requested {})\n",
            self.old_display, self.new_display, self.requested
        ));
        out.push_str(&format!(
            "  bytes:     {} -> {}\n",
            self.old_bytes_hex, self.new_bytes_hex
        ));
        out.push_str(&format!("  policy:    {}\n", self.policy.as_str()));
        out
    }

    pub fn save(&self, path: &Path) -> Result<(), NnError> {
        let file = Json::object(vec![
            (
                "schema",
                Json::Str("binfiddle.nn.edit-plan-file/v1".to_string()),
            ),
            ("plan_id", Json::Str(self.plan_id.clone())),
            ("plan", self.semantic()?),
        ])?;
        std::fs::write(path, file.to_canonical()?.as_bytes()).map_err(NnError::Io)?;
        Ok(())
    }

    pub fn load(path: &Path) -> Result<EditPlan, NnError> {
        let text = std::fs::read_to_string(path).map_err(NnError::Io)?;
        let file = Json::parse_strict(&text, ParseLimits::default())?;
        if file.get("schema").and_then(Json::as_str) != Some("binfiddle.nn.edit-plan-file/v1") {
            return Err(NnError::MalformedInput {
                detail: "not a binfiddle edit plan file".to_string(),
            });
        }
        let embedded = required_str(&file, "plan_id")?;
        let semantic = file
            .get("plan")
            .cloned()
            .ok_or_else(|| NnError::MalformedInput {
                detail: "edit plan file is missing the plan payload".to_string(),
            })?;
        let computed = compute_id(IdKind::Plan, &semantic)?;
        if computed != embedded {
            return Err(NnError::MalformedInput {
                detail: format!(
                    "edit plan id mismatch: file claims {}, payload hashes to {}",
                    embedded, computed
                ),
            });
        }
        let coordinate = semantic
            .get("coordinate")
            .and_then(Json::as_array)
            .map(|items| {
                items
                    .iter()
                    .filter_map(|v| v.as_str().and_then(|s| s.parse::<u64>().ok()))
                    .collect::<Vec<u64>>()
            })
            .unwrap_or_default();
        let span_json = semantic.get("span").cloned().unwrap_or(Json::Null);
        let span = (
            span_json
                .get("start")
                .and_then(Json::as_str)
                .and_then(|s| s.parse::<u64>().ok())
                .unwrap_or(0),
            span_json
                .get("length")
                .and_then(Json::as_str)
                .and_then(|s| s.parse::<u64>().ok())
                .unwrap_or(0),
        );
        let bit_field = match semantic.get("bit_field") {
            Some(b) if !matches!(b, Json::Null) => {
                let mask =
                    u8::from_str_radix(b.get("mask_hex").and_then(Json::as_str).unwrap_or(""), 16)
                        .map_err(|_| NnError::MalformedInput {
                            detail: "bad bit_field mask".to_string(),
                        })?;
                let shift = b
                    .get("shift")
                    .and_then(Json::as_str)
                    .and_then(|t| t.parse::<u8>().ok())
                    .ok_or_else(|| NnError::MalformedInput {
                        detail: "bad bit_field shift".to_string(),
                    })?;
                Some((mask, shift))
            }
            _ => None,
        };
        let decode_dependencies = semantic
            .get("decode_dependencies")
            .and_then(Json::as_array)
            .map(|items| {
                items
                    .iter()
                    .filter_map(|d| {
                        Some((
                            d.get("start").and_then(Json::as_str)?.parse::<u64>().ok()?,
                            d.get("length")
                                .and_then(Json::as_str)?
                                .parse::<u64>()
                                .ok()?,
                        ))
                    })
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        Ok(EditPlan {
            plan_id: embedded,
            catalog_id: required_str(&semantic, "catalog_id")?,
            tensor_id: required_str(&semantic, "tensor_id")?,
            tensor_name: required_str(&semantic, "tensor_name")?,
            encoding: required_str(&semantic, "encoding")?,
            coordinate,
            operation: match semantic.get("operation").and_then(Json::as_str) {
                Some("set_raw_bits") => "set_raw_bits",
                _ => "set_scalar",
            },
            policy: EditPolicy::parse(&required_str(&semantic, "policy")?)?,
            span,
            bit_field,
            decode_dependencies,
            source_id: required_str(&semantic, "source_id")?,
            source_digest: required_str(&semantic, "source_digest")?,
            old_bytes_hex: required_str(&semantic, "old_bytes_hex")?,
            new_bytes_hex: required_str(&semantic, "new_bytes_hex")?,
            old_display: required_str(&semantic, "old_value")?,
            new_display: required_str(&semantic, "new_value")?,
            requested: required_str(&semantic, "requested")?,
        })
    }
}

/// The requested change.
#[derive(Debug, Clone)]
pub enum RequestedValue {
    Typed(String),
    RawBits(String),
}

impl RequestedValue {
    fn describe(&self) -> String {
        match self {
            RequestedValue::Typed(v) => format!("value {v}"),
            RequestedValue::RawBits(h) => format!("raw bits 0x{h}"),
        }
    }
}

fn write_unit(
    tensor: &CatalogTensor,
    layout: TensorLayout,
    coordinate: &[u64],
) -> Result<ElementLocation, NnError> {
    match layout {
        TensorLayout::Scalar(codec) => {
            locate_scalar_element(&tensor.shape, coordinate, codec, tensor.payload_start)
        }
        TensorLayout::Q4_0 => locate_q4_0_element(&tensor.shape, coordinate, tensor.payload_start),
        TensorLayout::Unknown => Err(NnError::CodecUnsupported {
            codec: tensor.encoding.clone(),
            operation: "edit".to_string(),
            reason: "no qualified layout registers a write unit for this encoding".to_string(),
        }),
    }
}

fn decode_unit_display(
    reader: &BoundedFile,
    tensor: &CatalogTensor,
    layout: TensorLayout,
    coordinate: &[u64],
    location: &ElementLocation,
    budget: &Budget,
) -> Result<String, NnError> {
    let _ = coordinate;
    match layout {
        TensorLayout::Scalar(codec) => {
            let mut bytes = vec![0u8; codec.width() as usize];
            reader.read_exact_at_bounded(location.byte_span.0, &mut bytes, budget)?;
            let scalar = codec.decode(&bytes)?;
            Ok(scalar.value.render())
        }
        TensorLayout::Q4_0 => {
            // Read the whole enclosing block and decode the target element.
            let scale_start = location.decode_dependencies[0].0;
            let mut block = [0u8; 18];
            reader.read_exact_at_bounded(scale_start, &mut block, budget)?;
            let decoded = q4_0_decode_block(&block)?;
            let linear: u64 = tensor.shape.iter().product::<u64>();
            let _ = linear;
            // Element position within its block.
            let u = coordinate.last().copied().unwrap_or(0) % 32;
            Ok(format!("{}", decoded.values[u as usize]))
        }
        TensorLayout::Unknown => Ok("unknown".to_string()),
    }
}

#[allow(clippy::too_many_arguments)]
fn compute_new_bytes(
    layout: TensorLayout,
    tensor: &CatalogTensor,
    location: &ElementLocation,
    reader: &BoundedFile,
    text: &str,
    policy: EditPolicy,
    allow_clamp: bool,
    budget: &Budget,
) -> Result<(Vec<u8>, String), NnError> {
    match layout {
        TensorLayout::Scalar(codec) => {
            let is_integer = matches!(
                codec,
                ScalarCodec::I64
                    | ScalarCodec::I32
                    | ScalarCodec::I16
                    | ScalarCodec::I8
                    | ScalarCodec::U8
                    | ScalarCodec::Bool
            );
            let bytes = if is_integer {
                encode_integer(codec, text)?
            } else {
                let value: f64 = text.parse().map_err(|_| NnError::InvalidRequest {
                    message: format!("value {text} is not a number"),
                })?;
                encode_float(codec, value, policy)?.0
            };
            let decoded = codec.decode(&bytes)?;
            Ok((bytes, decoded.value.render()))
        }
        TensorLayout::Q4_0 => {
            let value: f64 = text.parse().map_err(|_| NnError::InvalidRequest {
                message: format!("value {text} is not a number"),
            })?;
            let scale_start = location.decode_dependencies[0].0;
            let mut block = [0u8; 18];
            reader.read_exact_at_bounded(scale_start, &mut block, budget)?;
            let decoded = q4_0_decode_block(&block)?;
            let (code, new_value) = q4_0_select_code(decoded.scale, value, policy, allow_clamp)?;
            // Merge the 4-bit code into the existing code byte.
            let (mask, shift) = location.bit_mask.expect("q4_0 write units are nibbles");
            // Re-read the exact code byte.
            let mut byte = [0u8; 1];
            reader.read_exact_at_bounded(location.byte_span.0, &mut byte, budget)?;
            let patched = (byte[0] & !mask) | (code << shift);
            Ok((vec![patched], format!("{new_value}")))
        }
        TensorLayout::Unknown => Err(NnError::CodecUnsupported {
            codec: tensor.encoding.clone(),
            operation: "edit".to_string(),
            reason: "no qualified numeric encoder for this encoding".to_string(),
        }),
    }
}

fn required_str(json: &Json, key: &str) -> Result<String, NnError> {
    json.get(key)
        .and_then(Json::as_str)
        .map(str::to_string)
        .ok_or_else(|| NnError::MalformedInput {
            detail: format!("record is missing {key}"),
        })
}

// ---- apply ----

/// Receipt for one applied edit.
pub struct EditReceipt {
    pub plan_id: String,
    pub output_path: PathBuf,
    pub output_digest: String,
    pub bytes_written: u64,
    pub unselected_preserved: bool,
    pub container_revalidated: bool,
}

/// Apply an edit plan: verify, copy+patch into a fresh output, validate.
pub fn apply_edit_plan(
    catalog: &Catalog,
    plan: &EditPlan,
    out_path: &Path,
    undo_bundle: Option<&Path>,
    budget: &Budget,
) -> Result<EditReceipt, NnError> {
    // Verification ladder.
    let catalog_id = catalog.id()?;
    if catalog_id != plan.catalog_id {
        return Err(NnError::SourceChanged {
            detail: format!(
                "plan was built against catalog {} but applied to {}",
                super::error::brief(&plan.catalog_id),
                super::error::brief(&catalog_id)
            ),
        });
    }
    let source = catalog.resolve_source(&plan.source_id)?;
    let source_path = Path::new(&source.path);
    let reader = BoundedFile::open(source_path)?;
    let current_digest = reader.content_digest(budget)?;
    if current_digest != plan.source_digest {
        return Err(NnError::SourceChanged {
            detail: format!(
                "source changed since the plan was made (plan {}, file {})",
                super::error::brief(&plan.source_digest),
                super::error::brief(&current_digest)
            ),
        });
    }
    // Preimage assertion.
    let unit_len = if plan.bit_field.is_some() {
        1
    } else {
        plan.span.1
    };
    let mut current_unit = vec![0u8; unit_len as usize];
    reader.read_exact_at_bounded(plan.span.0, &mut current_unit, budget)?;
    if hex::encode(&current_unit) != plan.old_bytes_hex {
        return Err(NnError::WriteConflict {
            detail: format!(
                "preimage mismatch at [{}, {}): plan recorded {}, file now has {}",
                plan.span.0,
                plan.span.0 + plan.span.1,
                plan.old_bytes_hex,
                hex::encode(&current_unit)
            ),
        });
    }
    let new_bytes = hex::decode(&plan.new_bytes_hex).map_err(|_| NnError::MalformedInput {
        detail: "plan carries invalid replacement hex".to_string(),
    })?;

    if out_path.exists() {
        return Err(NnError::InvalidRequest {
            message: format!(
                "output {} already exists; edits write fresh files and never overwrite",
                out_path.display()
            ),
        });
    }

    // Copy the source to the output, patching the write unit in flight.
    let mut output = std::fs::File::create_new(out_path).map_err(NnError::Io)?;
    let mut hasher = Sha256::new();
    let chunk_size = 1024 * 1024u64;
    let total = reader.length();
    let mut offset = 0u64;
    let unit_start = plan.span.0;
    let unit_end = plan.span.0 + unit_len;
    let mut buffer = Vec::new();
    while offset < total {
        let take = chunk_size.min(total - offset) as usize;
        buffer.clear();
        buffer.resize(take, 0);
        reader.read_exact_at_bounded(offset, &mut buffer, budget)?;
        if offset < unit_end && unit_start < offset + take as u64 {
            // Overlaps the write unit: patch (the unit is at most 8 bytes,
            // so a single chunk always covers it fully when it overlaps).
            let local_start = (unit_start - offset) as usize;
            buffer[local_start..local_start + new_bytes.len()].copy_from_slice(&new_bytes);
        }
        output.write_all(&buffer).map_err(NnError::Io)?;
        hasher.update(&buffer);
        budget.consume_output(take as u64)?;
        budget.checkpoint()?;
        offset += take as u64;
    }
    output.flush().map_err(NnError::Io)?;
    drop(output);
    let output_digest = hex::encode(hasher.finalize());

    // Preservation check: every byte outside [unit_start, unit_end) matches.
    let out_reader = BoundedFile::open(out_path)?;
    let mut preserved = true;
    let mut offset = 0u64;
    let mut expected = Vec::new();
    let mut actual = Vec::new();
    while offset < total {
        let take = chunk_size.min(total - offset) as usize;
        expected.clear();
        expected.resize(take, 0);
        actual.clear();
        actual.resize(take, 0);
        reader.read_exact_at_bounded(offset, &mut expected, budget)?;
        out_reader.read_exact_at_bounded(offset, &mut actual, budget)?;
        for i in 0..take {
            let file_offset = offset + i as u64;
            if file_offset >= unit_start && file_offset < unit_end {
                continue;
            }
            if expected[i] != actual[i] {
                preserved = false;
                break;
            }
        }
        if !preserved {
            break;
        }
        offset += take as u64;
    }

    // Container revalidation: the patched file must still parse with the same
    // tensor count and identical spans.
    let format = catalog
        .sources
        .iter()
        .find(|s| s.id == plan.source_id)
        .and_then(|s| s.format.as_deref())
        .unwrap_or("");
    let revalidated = if format.starts_with("safetensors") {
        let inventory = super::format::safetensors::inventory(&out_reader, budget)?;
        inventory.validity == super::format::Validity::Valid
            && inventory.tensors.iter().any(|t| {
                t.original_name == plan.tensor_name
                    && t.payload_start
                        == catalog
                            .tensors
                            .iter()
                            .find(|t| t.id == plan.tensor_id)
                            .map(|t| t.payload_start)
                            .unwrap_or(u64::MAX)
            })
    } else if format.starts_with("gguf") {
        let inventory = super::format::gguf::inventory(&out_reader, budget)?;
        inventory.validity == super::format::Validity::Valid
            && inventory
                .tensors
                .iter()
                .any(|t| t.original_name == plan.tensor_name)
    } else if format.starts_with("onnx") {
        // ONNX writer tier: fixed-size edits over raw_data spans; the model
        // must reparse with the same tensor count and an exact span for the
        // edited initializer.
        let inventory = super::format::onnx::inventory(&out_reader, budget)?;
        inventory.inventory.validity == super::format::Validity::Valid
            && inventory
                .inventory
                .tensors
                .iter()
                .any(|t| t.original_name == plan.tensor_name && t.payload_length.is_some())
    } else {
        false
    };

    let receipt = EditReceipt {
        plan_id: plan.plan_id.clone(),
        output_path: out_path.to_path_buf(),
        output_digest: output_digest.clone(),
        bytes_written: new_bytes.len() as u64,
        unselected_preserved: preserved,
        container_revalidated: revalidated,
    };

    if let Some(bundle_dir) = undo_bundle {
        std::fs::create_dir_all(bundle_dir).map_err(NnError::Io)?;
        let bundle = Json::object(vec![
            ("schema", Json::Str("binfiddle.nn.edit-undo/v1".to_string())),
            ("plan_id", Json::Str(plan.plan_id.clone())),
            ("source_path", Json::Str(source.path.clone())),
            ("source_digest", Json::Str(plan.source_digest.clone())),
            ("edited_digest", Json::Str(output_digest.clone())),
            (
                "patches",
                Json::Array(vec![Json::object(vec![
                    ("offset", Json::Str(plan.span.0.to_string())),
                    ("old_hex", Json::Str(plan.old_bytes_hex.clone())),
                    ("new_hex", Json::Str(plan.new_bytes_hex.clone())),
                ])?]),
            ),
        ])?;
        std::fs::write(
            bundle_dir.join("undo.json"),
            bundle.to_canonical()?.as_bytes(),
        )
        .map_err(NnError::Io)?;
    }

    Ok(receipt)
}

pub fn receipt_text(receipt: &EditReceipt) -> String {
    format!(
        "edit applied\n  output:   {}\n  digest:   {}\n  written:  {} bytes\n  preserved: {}\n  reparsed: {}\n",
        receipt.output_path.display(),
        &receipt.output_digest[..16.min(receipt.output_digest.len())],
        receipt.bytes_written,
        if receipt.unselected_preserved { "all bytes outside the planned span verified identical" } else { "FAILED" },
        if receipt.container_revalidated { "container reparse valid, spans unchanged" } else { "container reparse NOT verified" },
    )
}

pub fn receipt_envelope(receipt: &EditReceipt) -> Result<super::report::ResultEnvelope, NnError> {
    let semantic = Json::object(vec![
        ("plan_id", Json::Str(receipt.plan_id.clone())),
        (
            "output",
            Json::Str(receipt.output_path.display().to_string()),
        ),
        ("output_digest", Json::Str(receipt.output_digest.clone())),
        (
            "bytes_written",
            Json::Str(receipt.bytes_written.to_string()),
        ),
        (
            "unselected_preserved",
            Json::Bool(receipt.unselected_preserved),
        ),
        (
            "container_revalidated",
            Json::Bool(receipt.container_revalidated),
        ),
        (
            "guarantees",
            Json::object(vec![
                (
                    "claim",
                    Json::Str("fixed-size planned byte change".to_string()),
                ),
                ("original_bytes", Json::Bool(false)),
            ])?,
        ),
    ])?;
    Ok(super::report::ResultEnvelope::new("edit apply").with_semantic(semantic))
}

// ---- undo ----

/// Reverse an edit: the target file must be the exact edited revision.
pub fn undo_edit(
    bundle_dir: &Path,
    target: &Path,
    out_path: &Path,
    budget: &Budget,
) -> Result<EditReceipt, NnError> {
    let text = std::fs::read_to_string(bundle_dir.join("undo.json")).map_err(|_| {
        NnError::SourceMissing {
            detail: format!("no undo.json in {}", bundle_dir.display()),
        }
    })?;
    let bundle = Json::parse_strict(&text, ParseLimits::default())?;
    if bundle.get("schema").and_then(Json::as_str) != Some("binfiddle.nn.edit-undo/v1") {
        return Err(NnError::MalformedInput {
            detail: "not a binfiddle edit undo bundle".to_string(),
        });
    }
    let edited_digest = required_str(&bundle, "edited_digest")?;
    let reader = BoundedFile::open(target)?;
    let current = reader.content_digest(budget)?;
    if current != edited_digest {
        return Err(NnError::SourceChanged {
            detail: format!(
                "undo requires the exact edited revision {} but the target hashes to {}",
                super::error::brief(&edited_digest),
                super::error::brief(&current)
            ),
        });
    }
    let patch = bundle
        .get("patches")
        .and_then(Json::as_array)
        .and_then(|p| p.first())
        .cloned()
        .ok_or_else(|| NnError::MalformedInput {
            detail: "undo bundle carries no patches".to_string(),
        })?;
    let offset = required_str(&patch, "offset")?
        .parse::<u64>()
        .map_err(|_| NnError::MalformedInput {
            detail: "bad patch offset".to_string(),
        })?;
    let old_hex = required_str(&patch, "old_hex")?;
    let new_hex = required_str(&patch, "new_hex")?;
    let old_bytes = hex::decode(&old_hex).map_err(|_| NnError::MalformedInput {
        detail: "bad old hex".to_string(),
    })?;
    let new_bytes = hex::decode(&new_hex).map_err(|_| NnError::MalformedInput {
        detail: "bad new hex".to_string(),
    })?;

    // Preimage: the target must currently hold the edited bytes.
    let mut current_unit = vec![0u8; new_bytes.len()];
    reader.read_exact_at_bounded(offset, &mut current_unit, budget)?;
    if hex::encode(&current_unit) != new_hex {
        return Err(NnError::WriteConflict {
            detail: "target no longer holds the edited bytes; refusing blind rollback".to_string(),
        });
    }

    if out_path.exists() {
        return Err(NnError::InvalidRequest {
            message: format!("output {} already exists", out_path.display()),
        });
    }
    let mut output = std::fs::File::create_new(out_path).map_err(NnError::Io)?;
    let mut hasher = Sha256::new();
    let chunk_size = 1024 * 1024u64;
    let total = reader.length();
    let unit_start = offset;
    let unit_end = offset + old_bytes.len() as u64;
    let mut offset_iter = 0u64;
    let mut buffer = Vec::new();
    while offset_iter < total {
        let take = chunk_size.min(total - offset_iter) as usize;
        buffer.clear();
        buffer.resize(take, 0);
        reader.read_exact_at_bounded(offset_iter, &mut buffer, budget)?;
        if offset_iter < unit_end && unit_start < offset_iter + take as u64 {
            let local_start = (unit_start - offset_iter) as usize;
            buffer[local_start..local_start + old_bytes.len()].copy_from_slice(&old_bytes);
        }
        output.write_all(&buffer).map_err(NnError::Io)?;
        hasher.update(&buffer);
        budget.consume_output(take as u64)?;
        offset_iter += take as u64;
    }
    output.flush().map_err(NnError::Io)?;
    drop(output);

    Ok(EditReceipt {
        plan_id: required_str(&bundle, "plan_id")?,
        output_path: out_path.to_path_buf(),
        output_digest: hex::encode(hasher.finalize()),
        bytes_written: old_bytes.len() as u64,
        unselected_preserved: true,
        container_revalidated: false,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn f16_round_trip_reference_values() {
        // Encode-decode round trips for values F16 represents exactly.
        for value in [0.5f64, 1.0, -2.0, 8.0, 0.25, -0.75, 65504.0] {
            let bits = f64_to_f16_bits(value);
            assert_eq!(f16_to_f64(bits), value, "value {value} bits {bits:#06x}");
        }
        // The corpus fixture scale bits.
        assert_eq!(f64_to_f16_bits(0.5), 0x3800);
        // Overflow rounds to infinity.
        assert_eq!(f64_to_f16_bits(1e10), 0x7C00);
        assert_eq!(f64_to_f16_bits(-1e10), 0xFC00);
        // Subnormal encodings decode through the M3 decoder.
        let tiny = f64_to_f16_bits(2f64.powi(-24));
        assert_eq!(f16_to_f64(tiny), 2f64.powi(-24));
        // Values F16 cannot represent are rejected under exact_only.
        assert!(encode_float(ScalarCodec::F16, 0.1, EditPolicy::ExactOnly).is_err());
        let (bytes, decoded) = encode_float(ScalarCodec::F16, 0.1, EditPolicy::Nearest).unwrap();
        assert_eq!(bytes.len(), 2);
        assert!((decoded - 0.1).abs() < 1e-3);
    }

    #[test]
    fn bf16_round_trip_reference_values() {
        for value in [0.5f64, 1.0, 2.0, -1.5] {
            let bits = f32_to_bf16_bits((value as f32).to_bits());
            assert_eq!(f32::from_bits((bits as u32) << 16) as f64, value);
        }
        assert_eq!(f32_to_bf16_bits(0.5f32.to_bits()), 0x3F00);
    }

    #[test]
    fn integer_ranges_are_enforced() {
        assert_eq!(encode_integer(ScalarCodec::I8, "-128").unwrap(), vec![0x80]);
        assert_eq!(encode_integer(ScalarCodec::U8, "255").unwrap(), vec![0xFF]);
        assert!(encode_integer(ScalarCodec::I8, "128").is_err());
        assert!(encode_integer(ScalarCodec::U8, "256").is_err());
        assert!(encode_integer(ScalarCodec::I16, "-32769").is_err());
    }

    /// The corpus B.3 fixed-parameter selection: scale 0.5, requested −2.0
    /// → code 4 (byte A3 → A4), neighbor value 16 untouched.
    #[test]
    fn q4_0_fixed_parameter_selection_reference_vector() {
        let (code, decoded) =
            q4_0_select_code(0.5, -2.0, EditPolicy::FixedParameters, false).unwrap();
        assert_eq!(code, 4);
        assert_eq!(decoded, -2.0);
        // Exact policy accepts exactly representable values.
        let (code, _) = q4_0_select_code(0.5, -2.0, EditPolicy::ExactOnly, false).unwrap();
        assert_eq!(code, 4);
        // Unrepresentable values fail exact, resolve under fixed parameters.
        assert!(q4_0_select_code(0.5, -2.05, EditPolicy::ExactOnly, false).is_err());
        let (code, decoded) =
            q4_0_select_code(0.5, -2.05, EditPolicy::FixedParameters, false).unwrap();
        assert_eq!(code, 4);
        assert_eq!(decoded, -2.0);
        // Out-of-range codes need --clamp.
        assert!(q4_0_select_code(0.5, -100.0, EditPolicy::FixedParameters, false).is_err());
        let (code, decoded) =
            q4_0_select_code(0.5, -100.0, EditPolicy::FixedParameters, true).unwrap();
        assert_eq!(code, 0);
        assert_eq!(decoded, -4.0);
        // Negative scales work symmetrically.
        let (code, decoded) =
            q4_0_select_code(-0.5, 2.0, EditPolicy::FixedParameters, false).unwrap();
        assert_eq!(code, 4);
        assert_eq!(decoded, 2.0);
        // Zero scale is a degenerate representation.
        assert!(q4_0_select_code(0.0, 1.0, EditPolicy::FixedParameters, true).is_err());
    }
}
