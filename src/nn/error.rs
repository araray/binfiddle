//! NN-layer error types with stable machine-readable reason codes.
//!
//! Every error carries a stable code (Appendix A.12 of the workbench baseline) and a
//! process exit category (Part 11 §8). Codes are textually stable for automation;
//! messages are diagnostic aids and are not part of any contract.

use std::fmt;

/// Stable reason codes for NN operations.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ErrorCode {
    /// Invalid CLI syntax, incompatible options, or malformed request.
    InvalidRequest,
    /// Format or format version not implemented for the requested operation.
    FormatUnsupported,
    /// Encoding lacks the required qualified codec operation.
    CodecUnsupported,
    /// Structural contract of an input artifact violated.
    MalformedInput,
    /// Wire-contract violation (canonical integers, JSON subset, ID syntax).
    WireSyntax,
    /// More than one unresolved semantic mapping applies.
    AmbiguousBinding,
    /// A required executable interface dependency is unknown.
    BoundaryUnresolved,
    /// A requested semantic update lacks a supported inverse.
    InverseUnqualified,
    /// Resolved changes overlap incompatibly.
    WriteConflict,
    /// A named resource budget prevents completion.
    BudgetExceeded,
    /// A required output or operation check failed.
    ValidationFailed,
    /// Publication or receipt completion has an unresolved outcome.
    PublicationIncomplete,
    /// A required source revision or precondition no longer holds.
    SourceChanged,
    /// An identified source cannot be resolved under allowed roots.
    SourceMissing,
    /// The operation was cancelled by the caller.
    Cancelled,
    /// I/O failure.
    Io,
}

impl ErrorCode {
    /// Canonical wire spelling of this code.
    pub fn as_str(self) -> &'static str {
        match self {
            ErrorCode::InvalidRequest => "INVALID_REQUEST",
            ErrorCode::FormatUnsupported => "FORMAT_UNSUPPORTED",
            ErrorCode::CodecUnsupported => "CODEC_UNSUPPORTED",
            ErrorCode::MalformedInput => "MALFORMED_INPUT",
            ErrorCode::WireSyntax => "WIRE_SYNTAX",
            ErrorCode::AmbiguousBinding => "AMBIGUOUS_BINDING",
            ErrorCode::BoundaryUnresolved => "BOUNDARY_UNRESOLVED",
            ErrorCode::InverseUnqualified => "INVERSE_UNQUALIFIED",
            ErrorCode::WriteConflict => "WRITE_CONFLICT",
            ErrorCode::BudgetExceeded => "BUDGET_EXCEEDED",
            ErrorCode::ValidationFailed => "VALIDATION_FAILED",
            ErrorCode::PublicationIncomplete => "PUBLICATION_INCOMPLETE",
            ErrorCode::SourceChanged => "SOURCE_CHANGED",
            ErrorCode::SourceMissing => "SOURCE_MISSING",
            ErrorCode::Cancelled => "CANCELLED",
            ErrorCode::Io => "IO_ERROR",
        }
    }
}

/// Error type for the NN subsystem. Library callers match on variants; automation
/// relies on [`NnError::code`] and [`NnError::exit_code`].
#[derive(Debug)]
pub enum NnError {
    /// Malformed request, option combination, or unknown identifier domain.
    InvalidRequest { message: String },
    /// Container format or version is not implemented for the requested operation.
    FormatUnsupported { format: String, reason: String },
    /// The encoding lacks a qualified implementation for the requested operation.
    CodecUnsupported {
        codec: String,
        operation: String,
        reason: String,
    },
    /// Input artifact violates its structural contract.
    MalformedInput { detail: String },
    /// A value violates the wire contracts (canonical integers, JSON subset, IDs).
    WireSyntax { detail: String },
    /// Several unresolved semantic mappings apply to the request.
    AmbiguousBinding { detail: String },
    /// A required executable boundary dependency is unknown.
    BoundaryUnresolved { detail: String },
    /// The requested semantic update has no supported inverse.
    InverseUnqualified { detail: String },
    /// Resolved changes overlap incompatibly in some address space.
    WriteConflict { detail: String },
    /// A named resource budget was exhausted.
    BudgetExceeded {
        resource: &'static str,
        limit: u64,
        requested: u64,
    },
    /// A required validation failed before publication.
    ValidationFailed { detail: String },
    /// Publication or receipt completion is unresolved.
    PublicationIncomplete { detail: String },
    /// The source revision or a recorded precondition no longer holds.
    SourceChanged { detail: String },
    /// The identified source cannot be resolved under the allowed roots.
    SourceMissing { detail: String },
    /// Cooperative cancellation was observed between bounded steps.
    Cancelled,
    /// Underlying I/O failure.
    Io(std::io::Error),
}

impl From<std::io::Error> for NnError {
    fn from(err: std::io::Error) -> Self {
        NnError::Io(err)
    }
}

impl NnError {
    /// Stable machine-readable reason code for this error.
    pub fn code(&self) -> ErrorCode {
        match self {
            NnError::InvalidRequest { .. } => ErrorCode::InvalidRequest,
            NnError::FormatUnsupported { .. } => ErrorCode::FormatUnsupported,
            NnError::CodecUnsupported { .. } => ErrorCode::CodecUnsupported,
            NnError::MalformedInput { .. } => ErrorCode::MalformedInput,
            NnError::WireSyntax { .. } => ErrorCode::WireSyntax,
            NnError::AmbiguousBinding { .. } => ErrorCode::AmbiguousBinding,
            NnError::BoundaryUnresolved { .. } => ErrorCode::BoundaryUnresolved,
            NnError::InverseUnqualified { .. } => ErrorCode::InverseUnqualified,
            NnError::WriteConflict { .. } => ErrorCode::WriteConflict,
            NnError::BudgetExceeded { .. } => ErrorCode::BudgetExceeded,
            NnError::ValidationFailed { .. } => ErrorCode::ValidationFailed,
            NnError::PublicationIncomplete { .. } => ErrorCode::PublicationIncomplete,
            NnError::SourceChanged { .. } => ErrorCode::SourceChanged,
            NnError::SourceMissing { .. } => ErrorCode::SourceMissing,
            NnError::Cancelled => ErrorCode::Cancelled,
            NnError::Io(_) => ErrorCode::Io,
        }
    }

    /// Process exit category for the NN command families. Legacy commands keep
    /// their own exit behavior; these codes apply only to `nn` commands.
    pub fn exit_code(&self) -> i32 {
        match self {
            NnError::InvalidRequest { .. } => 2,
            NnError::FormatUnsupported { .. }
            | NnError::CodecUnsupported { .. }
            | NnError::AmbiguousBinding { .. }
            | NnError::BoundaryUnresolved { .. }
            | NnError::InverseUnqualified { .. } => 3,
            NnError::MalformedInput { .. } | NnError::WireSyntax { .. } => 4,
            NnError::SourceChanged { .. }
            | NnError::SourceMissing { .. }
            | NnError::WriteConflict { .. } => 5,
            NnError::Io(_)
            | NnError::BudgetExceeded { .. }
            | NnError::PublicationIncomplete { .. } => 6,
            NnError::ValidationFailed { .. } => 7,
            NnError::Cancelled => 130,
        }
    }
}

impl fmt::Display for NnError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            NnError::InvalidRequest { message } => write!(f, "invalid request: {message}"),
            NnError::FormatUnsupported { format, reason } => {
                write!(f, "format unsupported: {format}: {reason}")
            }
            NnError::CodecUnsupported {
                codec,
                operation,
                reason,
            } => {
                write!(f, "codec unsupported: {codec} for {operation}: {reason}")
            }
            NnError::MalformedInput { detail } => write!(f, "malformed input: {detail}"),
            NnError::WireSyntax { detail } => write!(f, "wire syntax violation: {detail}"),
            NnError::AmbiguousBinding { detail } => write!(f, "ambiguous binding: {detail}"),
            NnError::BoundaryUnresolved { detail } => {
                write!(f, "boundary unresolved: {detail}")
            }
            NnError::InverseUnqualified { detail } => {
                write!(f, "inverse unqualified: {detail}")
            }
            NnError::WriteConflict { detail } => write!(f, "write conflict: {detail}"),
            NnError::BudgetExceeded {
                resource,
                limit,
                requested,
            } => write!(
                f,
                "budget exceeded for {resource}: requested {requested}, available {limit}"
            ),
            NnError::ValidationFailed { detail } => write!(f, "validation failed: {detail}"),
            NnError::PublicationIncomplete { detail } => {
                write!(f, "publication incomplete: {detail}")
            }
            NnError::SourceChanged { detail } => write!(f, "source changed: {detail}"),
            NnError::SourceMissing { detail } => write!(f, "source missing: {detail}"),
            NnError::Cancelled => write!(f, "operation cancelled"),
            NnError::Io(err) => write!(f, "I/O error: {err}"),
        }
    }
}

impl std::error::Error for NnError {}

/// Truncate a free-form string for inclusion in a bounded diagnostic message.
pub(crate) fn brief(text: &str) -> String {
    if text.len() <= 64 {
        text.to_string()
    } else {
        let mut end = 64;
        while !text.is_char_boundary(end) {
            end -= 1;
        }
        format!("{}…", &text[..end])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn codes_are_stable_strings() {
        assert_eq!(ErrorCode::SourceChanged.as_str(), "SOURCE_CHANGED");
        assert_eq!(ErrorCode::BudgetExceeded.as_str(), "BUDGET_EXCEEDED");
        assert_eq!(ErrorCode::Cancelled.as_str(), "CANCELLED");
        assert_eq!(ErrorCode::InvalidRequest.as_str(), "INVALID_REQUEST");
    }

    #[test]
    fn exit_code_mapping_is_complete() {
        let samples: Vec<NnError> = vec![
            NnError::InvalidRequest {
                message: "x".into(),
            },
            NnError::FormatUnsupported {
                format: "f".into(),
                reason: "r".into(),
            },
            NnError::CodecUnsupported {
                codec: "c".into(),
                operation: "o".into(),
                reason: "r".into(),
            },
            NnError::MalformedInput { detail: "x".into() },
            NnError::WireSyntax { detail: "x".into() },
            NnError::AmbiguousBinding { detail: "x".into() },
            NnError::BoundaryUnresolved { detail: "x".into() },
            NnError::InverseUnqualified { detail: "x".into() },
            NnError::WriteConflict { detail: "x".into() },
            NnError::BudgetExceeded {
                resource: "memory",
                limit: 1,
                requested: 2,
            },
            NnError::ValidationFailed { detail: "x".into() },
            NnError::PublicationIncomplete { detail: "x".into() },
            NnError::SourceChanged { detail: "x".into() },
            NnError::SourceMissing { detail: "x".into() },
            NnError::Cancelled,
            NnError::Io(std::io::Error::other("x")),
        ];
        let expected = [2, 3, 3, 4, 4, 3, 3, 3, 5, 6, 7, 6, 5, 5, 130, 6];
        for (err, code) in samples.iter().zip(expected) {
            assert_eq!(err.exit_code(), code, "wrong exit for {:?}", err.code());
        }
        // Every variant must be covered: exhaustiveness is enforced by construction
        // here because samples lists one error per variant.
        assert_eq!(samples.len(), 16);
    }

    #[test]
    fn display_is_bounded_and_informative() {
        let err = NnError::BudgetExceeded {
            resource: "metadata_bytes",
            limit: 10,
            requested: 12,
        };
        assert_eq!(
            err.to_string(),
            "budget exceeded for metadata_bytes: requested 12, available 10"
        );
    }

    #[test]
    fn brief_truncates_on_char_boundary() {
        assert_eq!(brief("short"), "short");
        // 32 two-byte characters = exactly 64 bytes: unchanged.
        let fits = "é".repeat(32);
        assert_eq!(brief(&fits), fits);
        // 33 two-byte characters = 66 bytes: truncated to 32 chars + ellipsis.
        let longer = "é".repeat(33);
        let truncated = brief(&longer);
        assert!(truncated.ends_with('…'));
        assert_eq!(truncated.chars().count(), 33); // 32 two-byte chars + ellipsis
    }
}
