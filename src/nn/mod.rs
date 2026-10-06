//! NN workbench layer: contracts and foundations for neural-network artifact
//! inspection. This module is self-contained and adds no mandatory ML
//! dependencies; the legacy binary toolkit remains fully independent.

pub mod adapter;
pub mod address;
pub mod analyze;
pub mod approx;
pub mod budget;
pub mod cancel;
pub mod capabilities;
pub mod carve;
pub mod catalog;
pub mod codec;
pub mod compare;
pub mod component_selection;
pub mod discover;
pub mod edit;
pub mod error;
pub mod format;
pub mod id;
pub mod impact;
pub mod json;
pub mod packs;
pub mod partition;
pub mod profile;
pub mod queries;
pub mod recipes;
pub mod report;
pub mod research;
pub mod selection;
pub mod selector;
pub mod shard_map;
pub mod show;
pub mod slice;
pub mod source;
pub mod split_cmd;
pub mod tokenizer;
pub mod validate;
pub mod where_cmd;
pub mod wire;

pub use budget::{Budget, BudgetCaps, BudgetConsumed};
pub use cancel::{CancellationToken, SignalGuard};
pub use capabilities::{capabilities, capabilities_envelope, capabilities_text};
pub use catalog::{Catalog, CatalogCoverage, CatalogSource, CatalogTensor};
pub use discover::{discover, DiscoverOptions, DiscoverReport, SourceOutcome};
pub use error::{ErrorCode, NnError};
pub use id::{compute_id, validate_id, IdKind};
pub use json::{Json, ParseLimits};
pub use report::{
    CompletionStatus, Diagnostic, DiagnosticLevel, PublicationStatus, ResultEnvelope,
};
pub use selection::{EmptyPolicy, Selection, SelectionRequest};
pub use selector::{IndexSpec, Segment, Selector};
