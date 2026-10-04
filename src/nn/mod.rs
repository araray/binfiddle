//! NN workbench layer: contracts and foundations for neural-network artifact
//! inspection. This module is self-contained and adds no mandatory ML
//! dependencies; the legacy binary toolkit remains fully independent.

pub mod budget;
pub mod cancel;
pub mod capabilities;
pub mod discover;
pub mod error;
pub mod format;
pub mod id;
pub mod json;
pub mod report;
pub mod source;
pub mod wire;

pub use budget::{Budget, BudgetCaps, BudgetConsumed};
pub use cancel::{CancellationToken, SignalGuard};
pub use capabilities::{capabilities, capabilities_envelope, capabilities_text};
pub use discover::{discover, DiscoverOptions, DiscoverReport, SourceOutcome};
pub use error::{ErrorCode, NnError};
pub use id::{compute_id, validate_id, IdKind};
pub use json::{Json, ParseLimits};
pub use report::{
    CompletionStatus, Diagnostic, DiagnosticLevel, PublicationStatus, ResultEnvelope,
};
