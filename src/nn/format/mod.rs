//! Container format inventory readers.
//!
//! Readers are descriptor-first: they parse headers, metadata, and tensor
//! directories with bounded reads, and never touch payload bytes. Structural
//! findings are collected inside the inventory (unknown encoding stays a
//! visible descriptor) instead of erasing readable evidence.

pub mod gguf;
pub mod safetensors;

use crate::nn::error::NnError;

/// Extent knowledge for a tensor's payload.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Extent {
    /// Payload byte range is exactly known.
    Exact,
    /// A containing bound is known but exact membership is not.
    UpperBoundOnly,
}

impl Extent {
    pub fn as_str(self) -> &'static str {
        match self {
            Extent::Exact => "exact",
            Extent::UpperBoundOnly => "upper_bound_only",
        }
    }
}

/// Severity of an inventory finding.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Severity {
    Info,
    Warning,
    Error,
}

impl Severity {
    pub fn as_str(self) -> &'static str {
        match self {
            Severity::Info => "info",
            Severity::Warning => "warning",
            Severity::Error => "error",
        }
    }
}

/// One structural finding about a container.
#[derive(Debug, Clone)]
pub struct Finding {
    pub code: String,
    pub severity: Severity,
    pub message: String,
}

impl Finding {
    pub fn error(code: &str, message: impl Into<String>) -> Finding {
        Finding {
            code: code.to_string(),
            severity: Severity::Error,
            message: message.into(),
        }
    }

    pub fn warning(code: &str, message: impl Into<String>) -> Finding {
        Finding {
            code: code.to_string(),
            severity: Severity::Warning,
            message: message.into(),
        }
    }

    pub fn info(code: &str, message: impl Into<String>) -> Finding {
        Finding {
            code: code.to_string(),
            severity: Severity::Info,
            message: message.into(),
        }
    }
}

/// One tensor descriptor extracted from a container.
#[derive(Debug, Clone)]
pub struct TensorEntry {
    pub original_name: String,
    /// Precise encoding identifier (e.g. `safetensors.F32`, `ggml.q4_0`).
    pub encoding: String,
    /// True when a qualified numeric decoder is registered for this encoding.
    pub decode_supported: bool,
    /// Raw stored axis order (per the container, not normalized).
    pub shape: Vec<u64>,
    pub element_count: u64,
    /// File-qualified payload start.
    pub payload_start: u64,
    /// Payload byte length; `None` when the extent is only bounded.
    pub payload_length: Option<u64>,
    pub extent: Extent,
}

/// Structural validity of the parsed container.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Validity {
    Valid,
    /// Structure understood but defective; descriptors may still be useful.
    Invalid,
    /// Input incomplete (e.g. truncated below what the header declares).
    Incomplete,
}

impl Validity {
    pub fn as_str(&self) -> &'static str {
        match self {
            Validity::Valid => "valid",
            Validity::Invalid => "invalid",
            Validity::Incomplete => "incomplete",
        }
    }
}

/// Result of parsing one container file.
#[derive(Debug, Clone)]
pub struct FormatInventory {
    pub format: String,
    pub format_version: String,
    pub validity: Validity,
    pub tensors: Vec<TensorEntry>,
    pub findings: Vec<Finding>,
    /// Bytes read from the header/metadata/directory regions.
    pub metadata_bytes: u64,
    /// Payload bytes read; descriptor-only parsing reads none.
    pub payload_bytes: u64,
}

/// General alignment: `align_up(x, a) = x + ((a - (x % a)) % a)` with checked
/// arithmetic. No power-of-two mask shortcut: alignments are not guaranteed to
/// be powers of two.
pub fn align_up(x: u64, a: u64) -> Result<u64, NnError> {
    if a == 0 {
        return Err(NnError::MalformedInput {
            detail: "alignment must be positive".to_string(),
        });
    }
    let rem = (a - (x % a)) % a;
    x.checked_add(rem).ok_or_else(|| NnError::MalformedInput {
        detail: format!("alignment of {} to {} overflows u64", x, a),
    })
}

/// Read a little-endian unsigned integer at `offset`.
pub(crate) fn read_u_le(
    reader: &crate::nn::source::BoundedFile,
    offset: u64,
    width: usize,
    budget: &crate::nn::budget::Budget,
) -> Result<u64, NnError> {
    let mut buf = vec![0u8; width];
    reader.read_exact_at_bounded(offset, &mut buf, budget)?;
    Ok(u64::from_le_bytes(pad_le(buf)))
}

fn pad_le(bytes: Vec<u8>) -> [u8; 8] {
    let mut out = [0u8; 8];
    out[..bytes.len()].copy_from_slice(&bytes);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn align_up_reference_vectors() {
        assert_eq!(align_up(65, 32).unwrap(), 96);
        assert_eq!(align_up(64, 32).unwrap(), 64);
        assert_eq!(align_up(17, 6).unwrap(), 18);
        assert_eq!(align_up(0, 7).unwrap(), 0);
        assert!(align_up(10, 0).is_err());
        assert!(align_up(u64::MAX, 2).is_err());
    }
}
