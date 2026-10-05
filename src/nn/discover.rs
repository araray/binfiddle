//! Artifact discovery: bounded, descriptor-only inventory of model files.
//!
//! `discover` resolves one file or a package directory, probes supported
//! containers (GGUF magic first, then SafeTensors structure), and reports a
//! per-source inventory inside a `binfiddle.nn.result/v1` envelope. No payload
//! bytes are read; an optional explicit `--verify-content` pass hashes whole
//! files and strengthens the recorded source revision.
//!
//! Files that cannot be interpreted remain reported evidence (outcome
//! `unrecognized` or `malformed`); `--require-complete` turns incomplete
//! coverage into a nonzero exit instead of silently narrowing scope.

use super::budget::Budget;
use super::error::NnError;
use super::format::{self, FormatInventory};
use super::json::Json;
use super::report::{CompletionStatus, Diagnostic, DiagnosticLevel, ResultEnvelope};
use super::source::{BoundedFile, SourceRevision};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// Directory traversal depth limit.
const MAX_DEPTH: usize = 32;

/// Options for one discovery run.
#[derive(Debug, Clone, Copy, Default)]
pub struct DiscoverOptions {
    /// Hash complete file contents, upgrading source revisions to
    /// `content_verified`.
    pub verify_content: bool,
}

/// Outcome of inventorying one source file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SourceOutcome {
    Parsed,
    /// Structure readable but defective (findings recorded).
    Invalid,
    /// No supported container interpretation.
    Unrecognized,
    /// Reader failed hard on a plausibly-intended format.
    Malformed,
    /// Open/read failed (permissions, vanished file).
    Unreadable,
}

impl SourceOutcome {
    pub fn as_str(&self) -> &'static str {
        match self {
            SourceOutcome::Parsed => "parsed",
            SourceOutcome::Invalid => "invalid",
            SourceOutcome::Unrecognized => "unrecognized",
            SourceOutcome::Malformed => "malformed",
            SourceOutcome::Unreadable => "unreadable",
        }
    }
}

/// Inventory result for one file.
#[derive(Debug)]
pub struct SourceReport {
    pub path: String,
    pub revision: Option<SourceRevision>,
    pub outcome: SourceOutcome,
    pub inventory: Option<FormatInventory>,
    pub notes: Vec<String>,
}

/// Classification of non-container package members.
#[derive(Debug)]
pub struct AssetReport {
    pub path: String,
    pub classification: String,
}

/// Complete discovery result for the requested root.
#[derive(Debug)]
pub struct DiscoverReport {
    pub root_kind: &'static str,
    pub sources: Vec<SourceReport>,
    pub assets: Vec<AssetReport>,
    pub skipped_symlinks: Vec<String>,
}

impl DiscoverReport {
    /// Coverage counts used by the envelope and `--require-complete`.
    pub fn coverage(&self) -> (usize, usize, usize) {
        let parsed = self
            .sources
            .iter()
            .filter(|s| matches!(s.outcome, SourceOutcome::Parsed | SourceOutcome::Invalid))
            .count();
        let problematic = self.sources.len() - parsed;
        (self.sources.len(), parsed, problematic)
    }

    pub fn complete(&self) -> bool {
        !self.sources.is_empty()
            && self
                .sources
                .iter()
                .all(|s| s.outcome == SourceOutcome::Parsed)
            && self.skipped_symlinks.is_empty()
    }

    /// Build the result envelope (semantic payload in canonical-ready form).
    pub fn envelope(&self) -> Result<ResultEnvelope, NnError> {
        let mut source_records = Vec::with_capacity(self.sources.len());
        for source in &self.sources {
            source_records.push(source_record(source)?);
        }
        let asset_records = self
            .assets
            .iter()
            .map(|a| {
                Json::object(vec![
                    ("path", Json::Str(a.path.clone())),
                    ("classification", Json::Str(a.classification.clone())),
                ])
            })
            .collect::<Result<Vec<_>, _>>()?;
        let (considered, parsed, problematic) = self.coverage();
        let semantic = Json::object(vec![
            ("root_kind", Json::Str(self.root_kind.to_string())),
            ("sources", Json::Array(source_records)),
            ("assets", Json::Array(asset_records)),
            (
                "coverage",
                Json::object(vec![
                    ("sources_considered", Json::Str(considered.to_string())),
                    ("sources_parsed", Json::Str(parsed.to_string())),
                    ("sources_problematic", Json::Str(problematic.to_string())),
                    (
                        "symlinks_skipped",
                        Json::Str(self.skipped_symlinks.len().to_string()),
                    ),
                ])?,
            ),
        ])?;
        let mut envelope = ResultEnvelope::new("discover").with_semantic(semantic);
        for note in &self.skipped_symlinks {
            envelope = envelope.with_diagnostic(Diagnostic::new(
                "SYMLINK_SKIPPED",
                DiagnosticLevel::Warning,
                format!("symlink not followed: {}", super::error::brief(note)),
            ));
        }
        if !self.complete() {
            envelope = envelope
                .with_status(CompletionStatus::Partial)
                .with_coverage(
                    false,
                    vec![format!(
                        "{} of {} sources could not be fully inventoried",
                        problematic.min(considered),
                        considered
                    )],
                );
        }
        Ok(envelope)
    }

    /// Human-readable text report.
    pub fn text(&self) -> String {
        let mut out = String::from("binfiddle nn discover\n\n");
        for source in &self.sources {
            out.push_str(&format!("{}: {}\n", source.path, source.outcome.as_str()));
            if let Some(inventory) = &source.inventory {
                out.push_str(&format!(
                    "  format: {} {}\n  validity: {}\n  tensors: {}\n",
                    inventory.format,
                    inventory.format_version,
                    inventory.validity.as_str(),
                    inventory.tensors.len()
                ));
                for finding in &inventory.findings {
                    out.push_str(&format!(
                        "  finding [{}] {}: {}\n",
                        finding.severity.as_str(),
                        finding.code,
                        finding.message
                    ));
                }
                for tensor in inventory.tensors.iter().take(10) {
                    let shape = tensor
                        .shape
                        .iter()
                        .map(|d| d.to_string())
                        .collect::<Vec<_>>()
                        .join("x");
                    out.push_str(&format!(
                        "  tensor {} [{}] {} elements, encoding {}\n",
                        tensor.original_name, shape, tensor.element_count, tensor.encoding
                    ));
                }
                if inventory.tensors.len() > 10 {
                    out.push_str(&format!(
                        "  ... and {} more tensors\n",
                        inventory.tensors.len() - 10
                    ));
                }
            }
            for note in &source.notes {
                out.push_str(&format!("  note: {}\n", note));
            }
        }
        for asset in &self.assets {
            out.push_str(&format!(
                "{}: asset ({})\n",
                asset.path, asset.classification
            ));
        }
        let (considered, parsed, problematic) = self.coverage();
        out.push_str(&format!(
            "\ncoverage: {} of {} sources parsed, {} problematic, {} symlinks skipped\n",
            parsed,
            considered,
            problematic,
            self.skipped_symlinks.len()
        ));
        out
    }
}

fn source_record(source: &SourceReport) -> Result<Json, NnError> {
    let mut pairs: Vec<(&str, Json)> = vec![
        ("path", Json::Str(source.path.clone())),
        ("outcome", Json::Str(source.outcome.as_str().to_string())),
    ];
    if let Some(revision) = &source.revision {
        pairs.push(("source_id", Json::Str(revision.id()?)));
        pairs.push(("length", Json::Str(revision.length.to_string())));
        pairs.push((
            "consistency",
            Json::Str(revision.consistency.as_str().to_string()),
        ));
        if let Some(digest) = &revision.content_digest {
            pairs.push(("content_digest", Json::Str(digest.clone())));
        }
    }
    if let Some(inventory) = &source.inventory {
        pairs.push(("format", Json::Str(inventory.format.clone())));
        pairs.push((
            "format_version",
            Json::Str(inventory.format_version.clone()),
        ));
        pairs.push((
            "validity",
            Json::Str(inventory.validity.as_str().to_string()),
        ));
        let mut tensors = Vec::with_capacity(inventory.tensors.len());
        for tensor in &inventory.tensors {
            tensors.push(Json::object(vec![
                ("name", Json::Str(tensor.original_name.clone())),
                ("encoding", Json::Str(tensor.encoding.clone())),
                (
                    "shape",
                    Json::Array(
                        tensor
                            .shape
                            .iter()
                            .map(|d| Json::Str(d.to_string()))
                            .collect(),
                    ),
                ),
                ("elements", Json::Str(tensor.element_count.to_string())),
                ("payload_start", Json::Str(tensor.payload_start.to_string())),
                (
                    "payload_length",
                    match tensor.payload_length {
                        Some(len) => Json::Str(len.to_string()),
                        None => Json::Null,
                    },
                ),
                ("extent", Json::Str(tensor.extent.as_str().to_string())),
                ("decode_supported", Json::Bool(tensor.decode_supported)),
            ])?);
        }
        pairs.push(("tensors", Json::Array(tensors)));
        let findings = inventory
            .findings
            .iter()
            .map(|f| {
                Json::object(vec![
                    ("code", Json::Str(f.code.clone())),
                    ("severity", Json::Str(f.severity.as_str().to_string())),
                    ("message", Json::Str(f.message.clone())),
                ])
            })
            .collect::<Result<Vec<_>, _>>()?;
        pairs.push(("findings", Json::Array(findings)));
        pairs.push((
            "reads",
            Json::object(vec![
                (
                    "metadata_bytes",
                    Json::Str(inventory.metadata_bytes.to_string()),
                ),
                (
                    "payload_bytes",
                    Json::Str(inventory.payload_bytes.to_string()),
                ),
            ])?,
        ));
    }
    if !source.notes.is_empty() {
        pairs.push((
            "notes",
            Json::Array(source.notes.iter().cloned().map(Json::Str).collect()),
        ));
    }
    Json::object(pairs)
}

/// Per-file metadata budget (bytes) for discovery parsing.
const PER_FILE_METADATA_BYTES: u64 = 64 * 1024 * 1024;

fn child_budget(parent: &Budget) -> Budget {
    let mut caps = parent.caps();
    caps.metadata_bytes = PER_FILE_METADATA_BYTES;
    parent.child(caps, None)
}

/// Probe and parse one file as a supported container.
fn inventory_file(path: &Path, options: &DiscoverOptions, parent: &Budget) -> SourceReport {
    let display = path.display().to_string();
    let file = match BoundedFile::open(path) {
        Ok(file) => file,
        Err(err) => {
            return SourceReport {
                path: display,
                revision: None,
                outcome: SourceOutcome::Unreadable,
                inventory: None,
                notes: vec![err.to_string()],
            }
        }
    };
    let budget = child_budget(parent);

    let mut revision = SourceRevision::observed(PathBuf::from(path), file.length());
    if options.verify_content {
        match file.content_digest(&budget) {
            Ok(digest) => {
                revision =
                    SourceRevision::content_verified(PathBuf::from(path), file.length(), digest);
            }
            Err(err) => {
                return SourceReport {
                    path: display,
                    revision: Some(revision),
                    outcome: SourceOutcome::Unreadable,
                    inventory: None,
                    notes: vec![format!("content verification failed: {}", err)],
                }
            }
        }
    }

    // GGUF identifies by magic; SafeTensors and ONNX identify structurally
    // (ONNX is protobuf with a ModelProto/graph; the attempt is cheap and
    // fails fast on non-protobuf bytes).
    let parse_result = if has_gguf_magic(&file, &budget) {
        format::gguf::inventory(&file, &budget).map(Some)
    } else {
        match format::safetensors::inventory(&file, &budget) {
            Ok(inventory) => Ok(Some(inventory)),
            Err(NnError::MalformedInput { .. }) => match format::onnx::inventory(&file, &budget) {
                Ok(onnx) => Ok(Some(onnx.inventory)),
                Err(NnError::MalformedInput { .. }) => Ok(None),
                Err(err) => Err(err),
            },
            Err(err) => Err(err),
        }
    };

    match parse_result {
        Ok(Some(inventory)) => {
            let outcome = if inventory.validity == super::format::Validity::Valid {
                SourceOutcome::Parsed
            } else {
                SourceOutcome::Invalid
            };
            SourceReport {
                path: display,
                revision: Some(revision),
                outcome,
                inventory: Some(inventory),
                notes: Vec::new(),
            }
        }
        Ok(None) => {
            // Not GGUF, and neither SafeTensors nor ONNX parsing accepted the
            // structure. A plausible leading header length means this was
            // probably a malformed or truncated SafeTensors file; otherwise
            // it is simply unrecognized.
            let looks_like_st = file.length() >= 8 && {
                let mut buf = [0u8; 8];
                file.read_exact_at(0, &mut buf).is_ok()
                    && u64::from_le_bytes(buf) >= 2
                    && (u64::from_le_bytes(buf) + 8 <= file.length()
                        || u64::from_le_bytes(buf) <= PER_FILE_METADATA_BYTES)
            };
            SourceReport {
                path: display,
                revision: Some(revision),
                outcome: if looks_like_st {
                    SourceOutcome::Malformed
                } else {
                    SourceOutcome::Unrecognized
                },
                inventory: None,
                notes: vec![
                    "no supported container interpretation (tried gguf magic, safetensors structure, then onnx protobuf)".to_string(),
                ],
            }
        }
        Err(NnError::FormatUnsupported { format, reason }) => SourceReport {
            path: display,
            revision: Some(revision),
            outcome: SourceOutcome::Malformed,
            inventory: None,
            notes: vec![format!("{}: {}", format, reason)],
        },
        Err(err) => SourceReport {
            path: display,
            revision: Some(revision),
            outcome: SourceOutcome::Malformed,
            inventory: None,
            notes: vec![err.to_string()],
        },
    }
}

fn has_gguf_magic(file: &BoundedFile, budget: &Budget) -> bool {
    if file.length() < 4 {
        return false;
    }
    let mut magic = [0u8; 4];
    if file.read_exact_at_bounded(0, &mut magic, budget).is_err() {
        return false;
    }
    &magic == b"GGUF"
}

fn classify_asset(name: &str) -> &'static str {
    let lower = name.to_ascii_lowercase();
    if lower.ends_with(".safetensors.index.json") {
        "safetensors-index"
    } else if lower.ends_with(".json") {
        "json-configuration"
    } else if lower.ends_with(".yaml") || lower.ends_with(".yml") {
        "yaml-configuration"
    } else if lower.ends_with(".txt") {
        "text-asset"
    } else if lower.ends_with(".md") {
        "documentation"
    } else {
        "opaque"
    }
}

fn walk_directory(
    root: &Path,
    dir: &Path,
    depth: usize,
    files: &mut BTreeMap<PathBuf, ()>,
    skipped: &mut Vec<String>,
) -> Result<(), NnError> {
    if depth > MAX_DEPTH {
        return Err(NnError::InvalidRequest {
            message: format!(
                "directory traversal exceeds the {}-level depth limit below {}",
                MAX_DEPTH,
                root.display()
            ),
        });
    }
    let entries = std::fs::read_dir(dir).map_err(NnError::Io)?;
    for entry in entries {
        let entry = entry.map_err(NnError::Io)?;
        let path = entry.path();
        let file_type = entry.file_type().map_err(NnError::Io)?;
        if file_type.is_symlink() {
            skipped.push(path.display().to_string());
            continue;
        }
        if path.is_dir() {
            walk_directory(root, &path, depth + 1, files, skipped)?;
        } else {
            files.insert(path, ());
        }
    }
    Ok(())
}

/// Discover one file or a package directory.
pub fn discover(
    root: &Path,
    options: &DiscoverOptions,
    budget: &Budget,
) -> Result<DiscoverReport, NnError> {
    budget.checkpoint()?;
    let metadata = std::fs::metadata(root).map_err(|err| NnError::SourceMissing {
        detail: format!("{}: {}", root.display(), err),
    })?;

    if metadata.is_file() {
        let report = inventory_file(root, options, budget);
        return Ok(DiscoverReport {
            root_kind: "file",
            sources: vec![report],
            assets: Vec::new(),
            skipped_symlinks: Vec::new(),
        });
    }

    if !metadata.is_dir() {
        return Err(NnError::InvalidRequest {
            message: format!(
                "{} is neither a regular file nor a directory",
                root.display()
            ),
        });
    }

    let mut files = BTreeMap::new();
    let mut skipped = Vec::new();
    walk_directory(root, root, 0, &mut files, &mut skipped)?;

    let mut sources = Vec::new();
    let mut assets = Vec::new();
    for (path, ()) in &files {
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        let lower = name.to_ascii_lowercase();
        if lower.ends_with(".safetensors") || lower.ends_with(".gguf") {
            sources.push(inventory_file(path, options, budget));
            budget.checkpoint()?;
        } else {
            assets.push(AssetReport {
                path: path.display().to_string(),
                classification: classify_asset(&name).to_string(),
            });
        }
    }

    Ok(DiscoverReport {
        root_kind: "directory",
        sources,
        assets,
        skipped_symlinks: skipped,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::nn::budget::BudgetCaps;
    use crate::nn::cancel::CancellationToken;

    fn budget() -> Budget {
        Budget::new(BudgetCaps::default(), None, CancellationToken::new())
    }

    fn write_safetensors(dir: &Path, name: &str) {
        let header = "{\"a\":{\"dtype\":\"U8\",\"shape\":[2],\"data_offsets\":[0,2]}}";
        let mut data = Vec::new();
        data.extend_from_slice(&(header.len() as u64).to_le_bytes());
        data.extend_from_slice(header.as_bytes());
        data.extend_from_slice(&[7, 9]);
        std::fs::write(dir.join(name), data).unwrap();
    }

    #[test]
    fn discovers_single_safetensors_file() {
        let dir = tempfile::tempdir().unwrap();
        write_safetensors(dir.path(), "m.safetensors");
        let report = discover(
            &dir.path().join("m.safetensors"),
            &DiscoverOptions::default(),
            &budget(),
        )
        .unwrap();
        assert_eq!(report.root_kind, "file");
        assert_eq!(report.sources.len(), 1);
        assert_eq!(report.sources[0].outcome, SourceOutcome::Parsed);
        assert!(report.complete());
        let envelope = report.envelope().unwrap();
        assert!(envelope
            .to_json_string()
            .unwrap()
            .contains("\"outcome\":\"parsed\""));
    }

    #[test]
    fn unrecognized_file_is_reported_not_hidden() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("random.bin"), b"not a model at all").unwrap();
        let report = discover(
            &dir.path().join("random.bin"),
            &DiscoverOptions::default(),
            &budget(),
        )
        .unwrap();
        assert_eq!(report.sources[0].outcome, SourceOutcome::Unrecognized);
        assert!(!report.complete());
        let envelope = report.envelope().unwrap();
        let text = envelope.to_json_string().unwrap();
        assert!(text.contains("\"outcome\":\"unrecognized\""));
        assert!(text.contains("\"complete\":false"));
    }

    #[test]
    fn directory_scan_classifies_and_skips_symlinks() {
        let dir = tempfile::tempdir().unwrap();
        write_safetensors(dir.path(), "shard.safetensors");
        std::fs::write(dir.path().join("config.json"), b"{}").unwrap();
        std::fs::write(dir.path().join("README.md"), b"# model").unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink(
            dir.path().join("shard.safetensors"),
            dir.path().join("alias.safetensors"),
        )
        .unwrap();
        let report = discover(dir.path(), &DiscoverOptions::default(), &budget()).unwrap();
        assert_eq!(report.root_kind, "directory");
        assert_eq!(report.sources.len(), 1, "symlink must not be followed");
        #[cfg(unix)]
        assert_eq!(report.skipped_symlinks.len(), 1);
        let classifications: Vec<&str> = report
            .assets
            .iter()
            .map(|a| a.classification.as_str())
            .collect();
        assert!(classifications.contains(&"json-configuration"));
        assert!(classifications.contains(&"documentation"));
    }

    #[test]
    fn verify_content_upgrades_revision() {
        let dir = tempfile::tempdir().unwrap();
        write_safetensors(dir.path(), "m.safetensors");
        let options = DiscoverOptions {
            verify_content: true,
        };
        let report = discover(&dir.path().join("m.safetensors"), &options, &budget()).unwrap();
        let revision = report.sources[0].revision.as_ref().unwrap();
        assert!(revision.content_digest.is_some());
        let text = report.envelope().unwrap().to_json_string().unwrap();
        assert!(text.contains("\"content_verified\""));
    }

    #[test]
    fn missing_root_is_source_missing() {
        let dir = tempfile::tempdir().unwrap();
        let err = discover(
            &dir.path().join("nope.safetensors"),
            &DiscoverOptions::default(),
            &budget(),
        )
        .unwrap_err();
        assert_eq!(err.code().as_str(), "SOURCE_MISSING");
    }

    #[test]
    fn text_report_lists_tensors_and_coverage() {
        let dir = tempfile::tempdir().unwrap();
        write_safetensors(dir.path(), "m.safetensors");
        let report = discover(
            &dir.path().join("m.safetensors"),
            &DiscoverOptions::default(),
            &budget(),
        )
        .unwrap();
        let text = report.text();
        assert!(text.contains("m.safetensors: parsed"));
        assert!(text.contains("tensor a [2] 2 elements, encoding safetensors.U8"));
        assert!(text.contains("coverage: 1 of 1 sources parsed"));
    }
}
