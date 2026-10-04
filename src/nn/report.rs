//! Versioned result envelopes for NN operations.
//!
//! Every NN command emits a `binfiddle.nn.result/v1` envelope. Stdout carries
//! the primary report; diagnostics are structured records inside the envelope
//! when reporting as JSON. Completion status and publication truth are
//! independent fields.

use super::error::NnError;
use super::json::Json;

/// Operation completion status.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompletionStatus {
    Complete,
    Partial,
    Failed,
    Cancelled,
}

impl CompletionStatus {
    fn as_str(self) -> &'static str {
        match self {
            CompletionStatus::Complete => "complete",
            CompletionStatus::Partial => "partial",
            CompletionStatus::Failed => "failed",
            CompletionStatus::Cancelled => "cancelled",
        }
    }
}

/// Publication truth of a mutating operation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PublicationStatus {
    NotApplicable,
    NotPublished,
    Staged,
    Published,
    PublishedReceiptIncomplete,
}

impl PublicationStatus {
    fn as_str(self) -> &'static str {
        match self {
            PublicationStatus::NotApplicable => "not_applicable",
            PublicationStatus::NotPublished => "not_published",
            PublicationStatus::Staged => "staged",
            PublicationStatus::Published => "published",
            PublicationStatus::PublishedReceiptIncomplete => "published_receipt_incomplete",
        }
    }
}

/// Diagnostic severity.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiagnosticLevel {
    Info,
    Warning,
    Error,
}

impl DiagnosticLevel {
    pub fn as_str(self) -> &'static str {
        match self {
            DiagnosticLevel::Info => "info",
            DiagnosticLevel::Warning => "warning",
            DiagnosticLevel::Error => "error",
        }
    }
}

/// One structured diagnostic record.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Diagnostic {
    pub code: String,
    pub level: DiagnosticLevel,
    pub message: String,
    /// Operation phase where the diagnostic originated, when known.
    pub phase: Option<String>,
}

impl Diagnostic {
    pub fn new(
        code: impl Into<String>,
        level: DiagnosticLevel,
        message: impl Into<String>,
    ) -> Self {
        Diagnostic {
            code: code.into(),
            level,
            message: message.into(),
            phase: None,
        }
    }

    pub fn with_phase(mut self, phase: impl Into<String>) -> Self {
        self.phase = Some(phase.into());
        self
    }

    fn to_json(&self) -> Result<Json, NnError> {
        let mut pairs = vec![
            ("code", Json::Str(self.code.clone())),
            ("level", Json::Str(self.level.as_str().to_string())),
            ("message", Json::Str(self.message.clone())),
        ];
        if let Some(phase) = &self.phase {
            pairs.push(("phase", Json::Str(phase.clone())));
        }
        Json::object(pairs)
    }
}

/// Result envelope for one NN operation.
#[derive(Debug, Clone)]
pub struct ResultEnvelope {
    pub operation: String,
    pub status: CompletionStatus,
    /// Operation-specific semantic payload (must be an object).
    pub semantic: Json,
    pub coverage_complete: bool,
    pub coverage_notes: Vec<String>,
    pub diagnostics: Vec<Diagnostic>,
    pub publication: PublicationStatus,
}

impl ResultEnvelope {
    /// A successful, complete envelope with an empty semantic object.
    pub fn new(operation: impl Into<String>) -> Self {
        ResultEnvelope {
            operation: operation.into(),
            status: CompletionStatus::Complete,
            semantic: Json::Object(Vec::new()),
            coverage_complete: true,
            coverage_notes: Vec::new(),
            diagnostics: Vec::new(),
            publication: PublicationStatus::NotApplicable,
        }
    }

    pub fn with_semantic(mut self, semantic: Json) -> Self {
        self.semantic = semantic;
        self
    }

    pub fn with_status(mut self, status: CompletionStatus) -> Self {
        self.status = status;
        self
    }

    pub fn with_publication(mut self, publication: PublicationStatus) -> Self {
        self.publication = publication;
        self
    }

    pub fn with_diagnostic(mut self, diagnostic: Diagnostic) -> Self {
        self.diagnostics.push(diagnostic);
        self
    }

    pub fn with_coverage(mut self, complete: bool, notes: Vec<String>) -> Self {
        self.coverage_complete = complete;
        self.coverage_notes = notes;
        self
    }

    /// Build the envelope as a JSON value.
    pub fn to_json(&self) -> Result<Json, NnError> {
        if !matches!(self.semantic, Json::Object(_)) {
            return Err(NnError::WireSyntax {
                detail: "envelope semantic payload must be an object".to_string(),
            });
        }
        let diagnostics = self
            .diagnostics
            .iter()
            .map(Diagnostic::to_json)
            .collect::<Result<Vec<_>, _>>()?;
        Json::object(vec![
            ("schema", Json::Str("binfiddle.nn.result/v1".to_string())),
            ("operation", Json::Str(self.operation.clone())),
            ("status", Json::Str(self.status.as_str().to_string())),
            (
                "semantic",
                // Clone is acceptable: envelopes are small, bounded outputs.
                self.semantic.clone(),
            ),
            (
                "coverage",
                Json::object(vec![
                    ("complete", Json::Bool(self.coverage_complete)),
                    (
                        "notes",
                        Json::Array(self.coverage_notes.iter().cloned().map(Json::Str).collect()),
                    ),
                ])?,
            ),
            ("diagnostics", Json::Array(diagnostics)),
            (
                "execution",
                Json::object(vec![(
                    "publication",
                    Json::Str(self.publication.as_str().to_string()),
                )])?,
            ),
        ])
    }

    /// Serialize the envelope canonically (single line, deterministic).
    pub fn to_json_string(&self) -> Result<String, NnError> {
        self.to_json()?.to_canonical()
    }

    /// Write the canonical JSON envelope to a stream with a trailing newline.
    pub fn write_json(&self, writer: &mut dyn std::io::Write) -> Result<(), NnError> {
        let text = self.to_json_string()?;
        writer.write_all(text.as_bytes())?;
        writer.write_all(b"\n")?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn envelope_shape_matches_contract() {
        let envelope = ResultEnvelope::new("capabilities")
            .with_semantic(Json::object(vec![("count", Json::Str("1".to_string()))]).unwrap())
            .with_diagnostic(
                Diagnostic::new("SAMPLE_INFO", DiagnosticLevel::Info, "note").with_phase("report"),
            );
        let text = envelope.to_json_string().unwrap();
        // Canonical serialization sorts object keys, so coverage comes first.
        assert!(text.starts_with(r#"{"coverage":{"complete":true,"notes":[]},"diagnostics":"#));
        assert!(text.contains(r#""operation":"capabilities""#));
        assert!(text.contains(r#""schema":"binfiddle.nn.result/v1""#));
        assert!(text.contains(r#""status":"complete""#));
        assert!(text.contains(r#""publication":"not_applicable""#));
        assert!(text.contains(r#""code":"SAMPLE_INFO""#));
        assert!(text.contains(r#""phase":"report""#));
        assert!(text.ends_with("}"));
    }

    #[test]
    fn statuses_use_wire_spellings() {
        assert_eq!(CompletionStatus::Partial.as_str(), "partial");
        assert_eq!(CompletionStatus::Cancelled.as_str(), "cancelled");
        assert_eq!(
            PublicationStatus::PublishedReceiptIncomplete.as_str(),
            "published_receipt_incomplete"
        );
        assert_eq!(PublicationStatus::Staged.as_str(), "staged");
    }

    #[test]
    fn semantic_must_be_object() {
        let bad = ResultEnvelope::new("x").with_semantic(Json::Array(vec![]));
        assert!(bad.to_json().is_err());
    }

    #[test]
    fn failed_envelope_serializes() {
        let envelope = ResultEnvelope::new("slice")
            .with_status(CompletionStatus::Failed)
            .with_publication(PublicationStatus::NotPublished)
            .with_coverage(false, vec!["selection unresolved".to_string()])
            .with_diagnostic(Diagnostic::new(
                "BOUNDARY_UNRESOLVED",
                DiagnosticLevel::Error,
                "required interface dependency unknown",
            ));
        let text = envelope.to_json_string().unwrap();
        assert!(text.contains(r#""status":"failed""#));
        assert!(text.contains(r#""complete":false"#));
        assert!(text.contains("selection unresolved"));
        assert!(text.contains("BOUNDARY_UNRESOLVED"));
    }

    #[test]
    fn write_json_ends_with_newline() {
        let mut out = Vec::new();
        ResultEnvelope::new("capabilities")
            .write_json(&mut out)
            .unwrap();
        assert!(out.ends_with(b"}\n"));
    }
}
