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
use super::json::{Json, ParseLimits};
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
    /// Shards referenced by a `model.safetensors.index.json` in the scanned
    /// root but absent from it. A partial sharded package is incomplete
    /// coverage, exactly like an unreadable source.
    pub missing_shards: Vec<String>,
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
            && self.missing_shards.is_empty()
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
                    (
                        "index_shards_missing",
                        Json::Str(self.missing_shards.len().to_string()),
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
            let mut notes = vec![format!(
                "{} of {} sources could not be fully inventoried",
                problematic.min(considered),
                considered
            )];
            if !self.missing_shards.is_empty() {
                notes.push(format!(
                    "model.safetensors.index.json declares {} shards absent from this directory",
                    self.missing_shards.len()
                ));
            }
            envelope = envelope
                .with_status(CompletionStatus::Partial)
                .with_coverage(false, notes);
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
        if !self.missing_shards.is_empty() {
            out.push_str(&format!(
                "sharded-package note: model.safetensors.index.json declares shards absent here ({} of them); coverage is incomplete\n",
                self.missing_shards.len()
            ));
        }
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
    let name_lower = path
        .file_name()
        .map(|n| n.to_string_lossy().to_ascii_lowercase())
        .unwrap_or_default();
    let is_torch = name_lower.ends_with(".pth")
        || name_lower.ends_with(".pt")
        || name_lower.ends_with(".ckpt");
    let parse_result = if is_torch {
        format::torch::inventory(&file, &budget).map(Some)
    } else if has_gguf_magic(&file, &budget) {
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

/// Parse a `<base>-<index>-of-<total>` GGUF split-shard stem. Returns
/// `(base, index (1-based), total)`.
fn parse_shard_name(name: &str) -> Option<(String, u64, u64)> {
    // The last three dash-separated segments must be [index, "of", total].
    let stem = name.strip_suffix(".gguf")?;
    let parts: Vec<&str> = stem.split('-').collect();
    if parts.len() < 4 {
        return None;
    }
    let n = parts.len();
    let (index, of, total) = (parts[n - 3], parts[n - 2], parts[n - 1]);
    if of != "of" {
        return None;
    }
    let index: u64 = index.parse().ok()?;
    let total: u64 = total.parse().ok()?;
    if total == 0 || index == 0 || index > total {
        return None;
    }
    Some((parts[..n - 3].join("-"), index, total))
}

/// Tag a source with its split-group identity when the name matches the
/// convention (a discovery hint; content checks follow in the group pass).
fn annotate_shard_group(report: &mut SourceReport, name: &str) {
    if let Some((base, index, total)) = parse_shard_name(name) {
        report.notes.push(format!(
            "split shard {index} of {total} in group '{base}' (naming convention; membership is verified by content checks)"
        ));
    }
}

/// Verify split-GGUF groups: every declared shard present, and no tensor
/// name duplicated across shards of one group.
fn check_shard_groups(sources: &mut [SourceReport]) {
    use std::collections::BTreeMap;
    // (base, total) -> {index -> source position}
    let mut groups: BTreeMap<(String, u64), BTreeMap<u64, usize>> = BTreeMap::new();
    for (position, report) in sources.iter().enumerate() {
        // Path::file_name handles both separators; a manual '/' split would
        // keep a ".\" prefix in shard-group bases on Windows.
        let file_name = std::path::Path::new(&report.path)
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or(report.path.as_str())
            .to_string();
        if let Some((base, index, total)) = parse_shard_name(&file_name) {
            groups
                .entry((base, total))
                .or_default()
                .insert(index, position);
        }
    }
    for ((base, total), members) in groups {
        let mut findings: Vec<(String, String)> = Vec::new();
        // Completeness.
        let present: Vec<u64> = members.keys().copied().collect();
        if present.len() != total as usize || (1..=total).any(|i| !members.contains_key(&i)) {
            let missing: Vec<String> = (1..=total)
                .filter(|i| !members.contains_key(i))
                .map(|i| i.to_string())
                .collect();
            findings.push((
                "GGUF_SHARD_GROUP_INCOMPLETE".to_string(),
                format!(
                    "group '{base}' declares {total} shards; missing indices {}",
                    missing.join(",")
                ),
            ));
        }
        // Cross-shard tensor-name uniqueness (checked over parsed
        // inventories only — unreadable shards keep their own findings).
        let mut seen: BTreeMap<&str, u64> = BTreeMap::new();
        let mut duplicates: Vec<String> = Vec::new();
        for (index, position) in &members {
            let Some(inventory) = sources[*position].inventory.as_ref() else {
                continue;
            };
            for tensor in &inventory.tensors {
                let name = tensor.original_name.as_str();
                if let Some(first) = seen.insert(name, *index) {
                    duplicates.push(format!("{name} (shards {first} and {index})"));
                }
            }
        }
        if !duplicates.is_empty() {
            findings.push((
                "GGUF_SHARD_DUPLICATE_TENSOR".to_string(),
                format!(
                    "group '{base}' repeats tensor names across shards: {}",
                    duplicates.join(", ")
                ),
            ));
        }
        for (_, position) in members {
            let report = &mut sources[position];
            for (code, message) in &findings {
                report.notes.push(format!("[{code}] {message}"));
            }
        }
    }
}

/// Sharded-SafeTensors index awareness. A `model.safetensors.index.json`
/// beside the shards declares every shard of the package; this mirrors the
/// split-GGUF discipline so partial downloads are visible instead of
/// silently reduced. Returns the absent shard names (sorted).
fn check_safetensors_index(root: &Path, sources: &mut [SourceReport]) -> Vec<String> {
    let index_path = root.join("model.safetensors.index.json");
    let Ok(text) = std::fs::read_to_string(&index_path) else {
        return Vec::new();
    };
    // The index is a foreign JSON asset; parse it bounded. A malformed index
    // stays merely an asset (its own classification) rather than failing
    // discovery of the shards that are present.
    let Ok(parsed) = Json::parse_foreign(&text, ParseLimits::for_input_len(text.len())) else {
        return Vec::new();
    };
    let Some(Json::Object(members)) = parsed.get("weight_map") else {
        return Vec::new();
    };
    let referenced: std::collections::BTreeSet<String> = members
        .iter()
        .filter_map(|(_, shard)| shard.as_str().map(str::to_string))
        .collect();
    if referenced.is_empty() {
        return Vec::new();
    }
    let file_name = |path: &str| {
        std::path::Path::new(path)
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default()
    };
    let present: std::collections::BTreeSet<String> =
        sources.iter().map(|s| file_name(&s.path)).collect();
    let missing: Vec<String> = referenced.difference(&present).cloned().collect();
    if !missing.is_empty() {
        let mut sample: Vec<String> = missing.iter().take(3).cloned().collect();
        if missing.len() > 3 {
            sample.push(format!("and {} more", missing.len() - 3));
        }
        let finding = (
            "SAFETENSORS_SHARD_INDEX_INCOMPLETE".to_string(),
            format!(
                "model.safetensors.index.json declares {} shards; {} absent from this directory ({})",
                referenced.len(),
                missing.len(),
                sample.join(", ")
            ),
        );
        // The full finding is stated once; every other present shard carries
        // the code and a pointer, so a 50-shard subset is not 50 copies of
        // the same paragraph.
        let mut stated = false;
        for report in sources.iter_mut() {
            if referenced.contains(&file_name(&report.path)) {
                if !stated {
                    report.notes.push(format!("[{}] {}", finding.0, finding.1));
                    stated = true;
                } else {
                    report.notes.push(format!(
                        "[SAFETENSORS_SHARD_INDEX_INCOMPLETE] (same as above; {} of {} declared shards absent)",
                        missing.len(),
                        referenced.len()
                    ));
                }
            }
        }
    }
    missing
}

/// Cross-check present shards against the index claims: tensor names the
/// index assigns to a present shard must be observed in it, and names
/// must never repeat across shards (GGUF parity).
fn check_safetensors_index_consistency(root: &Path, sources: &mut [SourceReport]) {
    let index_path = root.join("model.safetensors.index.json");
    let Ok(text) = std::fs::read_to_string(&index_path) else {
        return;
    };
    let Ok(parsed) = Json::parse_foreign(&text, ParseLimits::for_input_len(text.len())) else {
        return;
    };
    let Some(Json::Object(members)) = parsed.get("weight_map") else {
        return;
    };
    use std::collections::BTreeMap;
    let mut assigned: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for (tensor, shard) in members {
        if let Some(s) = shard.as_str() {
            assigned
                .entry(s.to_string())
                .or_default()
                .push(tensor.clone());
        }
    }
    let mut findings: Vec<(String, String)> = Vec::new();
    for report in sources.iter() {
        let Some(name) = std::path::Path::new(&report.path)
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
        else {
            continue;
        };
        let Some(expected) = assigned.get(&name) else {
            continue;
        };
        let Some(inventory) = &report.inventory else {
            continue;
        };
        let observed: std::collections::BTreeSet<&str> = inventory
            .tensors
            .iter()
            .map(|t| t.original_name.as_str())
            .collect();
        let absent: Vec<&str> = expected
            .iter()
            .map(|e| e.as_str())
            .filter(|e| !observed.contains(e))
            .take(3)
            .collect();
        if !absent.is_empty() {
            findings.push((
                "SAFETENSORS_INDEX_CONSISTENCY".to_string(),
                format!(
                    "{name}: index assigns tensors the shard does not contain ({}, ...)",
                    absent.join(", ")
                ),
            ));
        }
    }
    let mut seen: std::collections::BTreeMap<&str, &str> = std::collections::BTreeMap::new();
    let mut duplicates: Vec<String> = Vec::new();
    for report in sources.iter() {
        let Some(inventory) = &report.inventory else {
            continue;
        };
        let shard = report.path.rsplit('/').next().unwrap_or(&report.path);
        for tensor in &inventory.tensors {
            if let Some(first) = seen.insert(tensor.original_name.as_str(), shard) {
                duplicates.push(format!(
                    "{} (shards {first} and {shard})",
                    tensor.original_name
                ));
            }
        }
    }
    if !duplicates.is_empty() {
        findings.push((
            "SAFETENSORS_SHARD_DUPLICATE_TENSOR".to_string(),
            format!(
                "index package repeats tensor names: {}",
                duplicates.join(", ")
            ),
        ));
    }
    // Findings attach once at report level (first source's notes).
    if let Some(first) = sources.first_mut() {
        for (code, message) in findings {
            first.notes.push(format!("[{code}] {message}"));
        }
    }
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

/// Default spool cap for stdin discovery (bytes). Streams larger than this
/// are rejected honestly rather than filling the disk.
pub const STDIN_SPOOL_CAP: u64 = 1024 * 1024 * 1024;

/// Discover from stdin: spool the stream into a private bounded temporary
/// file, hash it (the spool becomes a content-verified source identity for
/// the captured bytes), discover on the spool, annotate that the source is a
/// retained spool, and delete the spool afterwards.
pub fn discover_stdin(
    _options: &DiscoverOptions,
    budget: &Budget,
) -> Result<DiscoverReport, NnError> {
    use sha2::Digest;
    use std::io::{Read, Write};
    budget.checkpoint()?;
    let mut spool = tempfile::NamedTempFile::new().map_err(NnError::Io)?;
    let mut hasher = sha2::Sha256::new();
    let mut written: u64 = 0;
    let mut stdin = std::io::stdin().lock();
    let mut chunk = vec![0u8; 1024 * 1024];
    loop {
        let take = stdin.read(&mut chunk).map_err(NnError::Io)?;
        if take == 0 {
            break;
        }
        written += take as u64;
        if written > STDIN_SPOOL_CAP {
            return Err(NnError::BudgetExceeded {
                resource: "stdin_spool",
                limit: STDIN_SPOOL_CAP,
                requested: written,
            });
        }
        budget.consume_source_read(take as u64)?;
        spool.write_all(&chunk[..take]).map_err(NnError::Io)?;
        hasher.update(&chunk[..take]);
        budget.checkpoint()?;
    }
    spool.flush().map_err(NnError::Io)?;
    let digest = hex::encode(hasher.finalize());

    // Discover on the spooled copy with a content-verified revision.
    let spool_options = DiscoverOptions {
        verify_content: true,
    };
    let mut report = discover(spool.path(), &spool_options, budget)?;
    for source in &mut report.sources {
        if source.revision.is_none() {
            source.revision = Some(super::source::SourceRevision::content_verified(
                spool.path().to_path_buf(),
                written,
                digest.clone(),
            ));
        }
        source.notes.push(format!(
            "source is a captured stdin stream spooled to a private temporary file ({} bytes, sha256 {}..); reference slices from this report are ephemeral unless the bytes are re-provided",
            written,
            &digest[..12.min(digest.len())]
        ));
    }
    Ok(report)
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
            missing_shards: Vec::new(),
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
        if lower.ends_with(".safetensors")
            || lower.ends_with(".gguf")
            || lower.ends_with(".pth")
            || lower.ends_with(".pt")
            || lower.ends_with(".ckpt")
        {
            let mut report = inventory_file(path, options, budget);
            annotate_shard_group(&mut report, &name);
            sources.push(report);
            budget.checkpoint()?;
        } else {
            assets.push(AssetReport {
                path: path.display().to_string(),
                classification: classify_asset(&name).to_string(),
            });
        }
    }

    // Split-GGUF completeness: group shards by the `name-NNNNN-of-MMMMM.gguf`
    // convention, then verify each group has all declared members and no
    // duplicate tensor names across shards. Naming is a hint; the checks are
    // performed on what the shards actually contain.
    check_shard_groups(&mut sources);

    // Sharded-SafeTensors index: a model.safetensors.index.json declares
    // the full shard set; absent shards make coverage incomplete.
    let missing_shards = check_safetensors_index(root, &mut sources);
    check_safetensors_index_consistency(root, &mut sources);

    Ok(DiscoverReport {
        root_kind: "directory",
        sources,
        assets,
        skipped_symlinks: skipped,
        missing_shards,
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

    /// A model.safetensors.index.json declares the full shard set; absent
    /// shards are visible findings and make coverage incomplete (so
    /// --require-complete rejects partial downloads), exactly like the
    /// split-GGUF discipline.
    #[test]
    fn safetensors_index_makes_missing_shards_visible() {
        let dir = tempfile::tempdir().unwrap();
        write_safetensors(dir.path(), "model-00001-of-00002.safetensors");
        std::fs::write(
            dir.path().join("model.safetensors.index.json"),
            r#"{"metadata":{"total_size":"4"},"weight_map":{"a":"model-00001-of-00002.safetensors","b":"model-00002-of-00002.safetensors"}}"#,
        )
        .unwrap();
        let report = discover(dir.path(), &DiscoverOptions::default(), &budget()).unwrap();
        assert_eq!(
            report.missing_shards,
            vec!["model-00002-of-00002.safetensors"]
        );
        assert!(
            !report.complete(),
            "partial shard set is incomplete coverage"
        );
        assert!(report.sources[0]
            .notes
            .iter()
            .any(|n| n.contains("SAFETENSORS_SHARD_INDEX_INCOMPLETE")));
        let text = report.text();
        assert!(text.contains("sharded-package note"));
        let envelope = report.envelope().unwrap().to_json_string().unwrap();
        assert!(envelope.contains("\"index_shards_missing\":\"1\""));

        // Complete shard set: no findings, coverage complete.
        write_safetensors(dir.path(), "model-00002-of-00002.safetensors");
        let report = discover(dir.path(), &DiscoverOptions::default(), &budget()).unwrap();
        assert!(report.missing_shards.is_empty());
        assert!(report.complete());
        assert!(report.sources.iter().all(|s| {
            !s.notes
                .iter()
                .any(|n| n.contains("SAFETENSORS_SHARD_INDEX_INCOMPLETE"))
        }));
    }

    /// No index, or an index without a weight map, changes nothing.
    #[test]
    fn safetensors_index_absent_or_foreign_is_ignored() {
        let dir = tempfile::tempdir().unwrap();
        write_safetensors(dir.path(), "m.safetensors");
        let report = discover(dir.path(), &DiscoverOptions::default(), &budget()).unwrap();
        assert!(report.missing_shards.is_empty() && report.complete());

        std::fs::write(
            dir.path().join("model.safetensors.index.json"),
            r#"{"not":"a shard index"}"#,
        )
        .unwrap();
        let report = discover(dir.path(), &DiscoverOptions::default(), &budget()).unwrap();
        assert!(report.missing_shards.is_empty() && report.complete());
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
