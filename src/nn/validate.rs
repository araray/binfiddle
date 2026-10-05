//! Artifact validation: precise per-source structural verdicts.
//!
//! `nn validate` re-runs the structural checks and reports each source's
//! verdict with the corpus's precise categories —
//! `structurally_valid_for_reader`, `unsupported_feature`, `invalid`,
//! `incomplete` — plus findings, unresolved members, and shard-group notes.
//! It never returns a blanket "safe model": behavior is explicitly not
//! evaluated, and the report says so.

use super::discover::{self, DiscoverOptions, SourceOutcome};
use super::error::NnError;
use super::json::Json;
use super::report::ResultEnvelope;
use std::path::{Path, PathBuf};

/// One source verdict.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Verdict {
    StructurallyValidForReader,
    UnsupportedFeature,
    Invalid,
    Incomplete,
}

impl Verdict {
    pub fn as_str(&self) -> &'static str {
        match self {
            Verdict::StructurallyValidForReader => "structurally_valid_for_reader",
            Verdict::UnsupportedFeature => "unsupported_feature",
            Verdict::Invalid => "invalid",
            Verdict::Incomplete => "incomplete",
        }
    }
}

/// One source's validation record.
#[derive(Debug, Clone)]
pub struct SourceVerdict {
    pub path: String,
    pub verdict: Verdict,
    pub findings: Vec<(String, String, String)>, // (code, severity, message)
    pub note: String,
}

/// The complete validation report.
pub struct ValidationReport {
    pub root: PathBuf,
    pub sources: Vec<SourceVerdict>,
    pub all_valid: bool,
}

impl ValidationReport {
    /// Validate a file or directory through a fresh discovery pass.
    pub fn validate(
        root: &Path,
        budget: &super::budget::Budget,
    ) -> Result<ValidationReport, NnError> {
        let report = discover::discover(root, &DiscoverOptions::default(), budget)?;
        let mut sources = Vec::new();
        let mut all_valid = true;
        for source in &report.sources {
            let (verdict, findings, note) = match (&source.outcome, &source.inventory) {
                (SourceOutcome::Parsed, Some(inventory))
                    if inventory.validity == super::format::Validity::Valid
                        && !inventory
                            .findings
                            .iter()
                            .any(|f| f.severity == super::format::Severity::Error) =>
                {
                    let findings = inventory
                        .findings
                        .iter()
                        .map(|f| {
                            (
                                f.code.clone(),
                                f.severity.as_str().to_string(),
                                f.message.clone(),
                            )
                        })
                        .collect();
                    let note = if inventory
                        .findings
                        .iter()
                        .any(|f| f.code.starts_with("ONNX_"))
                        || inventory.tensors.iter().any(|t| t.payload_length.is_none())
                    {
                        "readable with caveats above (external or packed storage stays bounded)"
                            .to_string()
                    } else {
                        "all structural checks passed for this reader".to_string()
                    };
                    (Verdict::StructurallyValidForReader, findings, note)
                }
                (SourceOutcome::Parsed, Some(_)) | (SourceOutcome::Invalid, Some(_)) => {
                    let findings = source
                        .inventory
                        .as_ref()
                        .map(|i| {
                            i.findings
                                .iter()
                                .map(|f| {
                                    (
                                        f.code.clone(),
                                        f.severity.as_str().to_string(),
                                        f.message.clone(),
                                    )
                                })
                                .collect()
                        })
                        .unwrap_or_default();
                    (
                        Verdict::Invalid,
                        findings,
                        "structural defects found (invalid validity or error-severity findings); see findings"
                            .to_string(),
                    )
                }
                (SourceOutcome::Malformed, _) => (
                    Verdict::Invalid,
                    Vec::new(),
                    source.notes.first().cloned().unwrap_or_default(),
                ),
                (SourceOutcome::Parsed, _) | (SourceOutcome::Invalid, _) => (
                    Verdict::Incomplete,
                    Vec::new(),
                    "parsed outcome without a retained inventory".to_string(),
                ),
                (SourceOutcome::Unreadable, _) => (
                    Verdict::Incomplete,
                    Vec::new(),
                    source.notes.first().cloned().unwrap_or_default(),
                ),
                (SourceOutcome::Unrecognized, _) => (
                    Verdict::UnsupportedFeature,
                    Vec::new(),
                    "no supported container interpretation".to_string(),
                ),
            };
            if verdict != Verdict::StructurallyValidForReader {
                all_valid = false;
            }
            // Shard-group notes surfaced as findings context.
            let mut findings = findings;
            for note_line in &source.notes {
                if let Some(code) = note_line.strip_prefix('[') {
                    if let Some((code, message)) = code.split_once(']') {
                        findings.push((
                            code.to_string(),
                            "warning".to_string(),
                            message.trim().to_string(),
                        ));
                    }
                }
            }
            sources.push(SourceVerdict {
                path: source.path.clone(),
                verdict,
                findings,
                note,
            });
        }
        if sources.is_empty() {
            all_valid = false;
        }
        Ok(ValidationReport {
            root: root.to_path_buf(),
            sources,
            all_valid,
        })
    }

    pub fn envelope(&self) -> Result<ResultEnvelope, NnError> {
        let items = self
            .sources
            .iter()
            .map(|s| {
                Json::object(vec![
                    ("path", Json::Str(s.path.clone())),
                    ("verdict", Json::Str(s.verdict.as_str().to_string())),
                    (
                        "findings",
                        Json::Array(
                            s.findings
                                .iter()
                                .map(|(code, severity, message)| {
                                    Json::object(vec![
                                        ("code", Json::Str(code.clone())),
                                        ("severity", Json::Str(severity.clone())),
                                        ("message", Json::Str(message.clone())),
                                    ])
                                })
                                .collect::<Result<Vec<_>, _>>()?,
                        ),
                    ),
                    ("note", Json::Str(s.note.clone())),
                ])
            })
            .collect::<Result<Vec<_>, _>>()?;
        let count = |v: Verdict| {
            self.sources
                .iter()
                .filter(|s| s.verdict == v)
                .count()
                .to_string()
        };
        let semantic = Json::object(vec![
            ("root", Json::Str(self.root.display().to_string())),
            ("sources", Json::Array(items)),
            (
                "counts",
                Json::object(vec![
                    (
                        "structurally_valid_for_reader",
                        Json::Str(count(Verdict::StructurallyValidForReader)),
                    ),
                    ("unsupported_feature", Json::Str(count(Verdict::UnsupportedFeature))),
                    ("invalid", Json::Str(count(Verdict::Invalid))),
                    ("incomplete", Json::Str(count(Verdict::Incomplete))),
                ])?,
            ),
            (
                "behavior",
                Json::Str("behavior_not_evaluated".to_string()),
            ),
            (
                "claims",
                Json::Str(
                    "structural verdicts per reader only; this is not a safety or behavioral assessment"
                        .to_string(),
                ),
            ),
        ])?;
        let mut envelope = ResultEnvelope::new("validate").with_semantic(semantic);
        if !self.all_valid {
            envelope = envelope.with_coverage(
                false,
                vec!["one or more sources are not structurally valid".to_string()],
            );
        }
        Ok(envelope)
    }

    pub fn text(&self) -> String {
        let mut out = String::from("artifact validation\n");
        for source in &self.sources {
            out.push_str(&format!("  {}: {}\n", source.path, source.verdict.as_str()));
            for (code, severity, message) in &source.findings {
                out.push_str(&format!("    [{severity}] {code}: {message}\n"));
            }
            if !source.note.is_empty() {
                out.push_str(&format!("    note: {}\n", source.note));
            }
        }
        out.push_str("  behavior: behavior_not_evaluated\n");
        out.push_str(
            "  claims: structural verdicts per reader only; not a safety or behavioral assessment\n",
        );
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::nn::budget::{Budget, BudgetCaps};
    use crate::nn::cancel::CancellationToken;

    fn budget() -> Budget {
        Budget::new(BudgetCaps::default(), None, CancellationToken::new())
    }

    #[test]
    fn valid_invalid_and_unrecognized_verdicts() {
        let dir = tempfile::tempdir().unwrap();
        // Valid safetensors.
        let header = r#"{"w":{"dtype":"U8","shape":[1],"data_offsets":[0,1]}}"#;
        let mut data = Vec::new();
        data.extend_from_slice(&(header.len() as u64).to_le_bytes());
        data.extend_from_slice(header.as_bytes());
        data.push(7);
        std::fs::write(dir.path().join("good.safetensors"), data).unwrap();
        // Malformed: size mismatch ([2] but 3 bytes).
        let bad = r#"{"b":{"dtype":"U8","shape":[2],"data_offsets":[0,3]}}"#;
        let mut data = Vec::new();
        data.extend_from_slice(&(bad.len() as u64).to_le_bytes());
        data.extend_from_slice(bad.as_bytes());
        data.extend_from_slice(&[1, 2, 3]);
        std::fs::write(dir.path().join("bad.safetensors"), data).unwrap();
        // Unrecognized garbage (must carry a model extension to be scanned
        // as a source; other files are assets).
        std::fs::write(dir.path().join("junk.safetensors"), b"plain text").unwrap();

        let report = ValidationReport::validate(dir.path(), &budget()).unwrap();
        let by_verdict = |v: Verdict| report.sources.iter().filter(|s| s.verdict == v).count();
        assert_eq!(by_verdict(Verdict::StructurallyValidForReader), 1);
        assert_eq!(by_verdict(Verdict::Invalid), 1);
        assert_eq!(by_verdict(Verdict::UnsupportedFeature), 1);
        assert!(!report.all_valid);
        let text = report.text();
        assert!(text.contains("structurally_valid_for_reader"), "{text}");
        assert!(text.contains("behavior_not_evaluated"), "{text}");
        assert!(
            text.contains("not a safety or behavioral assessment"),
            "{text}"
        );
    }

    #[test]
    fn empty_directory_is_not_valid() {
        let dir = tempfile::tempdir().unwrap();
        let inner = dir.path().join("empty");
        std::fs::create_dir_all(&inner).unwrap();
        let report = ValidationReport::validate(&inner, &budget()).unwrap();
        assert!(report.sources.is_empty());
        assert!(!report.all_valid);
    }
}
