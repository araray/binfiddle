//! Immutable tensor catalogs over discovery results.
//!
//! A catalog freezes one discovery snapshot into an addressable record set:
//! source revisions with their identifiers, tensor descriptors with
//! domain-separated `tensor:` identifiers, unresolved members kept visible,
//! and coverage counts. Catalogs serialize to canonical JSON and reload with
//! integrity verification: the embedded catalog id must equal the digest
//! recomputed from the semantic payload, and duplicate keys are rejected by
//! the strict parser. Saved selections bind the exact catalog id, so a
//! reloaded selection can never silently reinterpret against different bytes.
//!
//! M2 catalogs are tensor-only: components, edges, and pack evidence arrive
//! with the model-pack layer. Until then the tensor projection is the
//! complete and honest view.

use super::discover::{DiscoverReport, SourceOutcome};
use super::error::NnError;
use super::format::{encoding_decode_supported, Extent, TensorEntry};
use super::id::{compute_id, IdKind};
use super::json::{Json, ParseLimits};
use std::path::Path;

/// One source inside a catalog: semantic identity plus presentation facts.
#[derive(Debug, Clone)]
pub struct CatalogSource {
    pub id: String,
    pub semantic: Json,
    pub outcome: SourceOutcome,
    /// Display path as seen during discovery (not part of identity).
    pub path: String,
    pub format: Option<String>,
}

/// A member that could not be interpreted but must remain visible evidence.
#[derive(Debug, Clone)]
pub struct UnresolvedMember {
    pub path: String,
    pub outcome: SourceOutcome,
    pub note: String,
}

/// One tensor inside a catalog.
#[derive(Debug, Clone)]
pub struct CatalogTensor {
    pub id: String,
    pub semantic: Json,
    pub source_id: String,
    pub original_name: String,
    pub encoding: String,
    pub decode_supported: bool,
    pub shape: Vec<u64>,
    pub element_count: u64,
    pub payload_start: u64,
    pub payload_length: Option<u64>,
    pub extent: Extent,
}

/// Coverage snapshot of the catalog.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CatalogCoverage {
    pub sources_total: usize,
    pub sources_parsed: usize,
    pub tensors_total: usize,
    pub tensors_exact_extent: usize,
    pub tensors_bounded_extent: usize,
    pub unresolved_members: usize,
}

/// An immutable catalog.
#[derive(Debug, Clone)]
pub struct Catalog {
    pub sources: Vec<CatalogSource>,
    pub tensors: Vec<CatalogTensor>,
    pub unresolved: Vec<UnresolvedMember>,
    pub coverage: CatalogCoverage,
}

impl Catalog {
    /// Build a catalog from a discovery report. Sources without a revision
    /// (unreadable files) and sources without an interpretation stay in
    /// `unresolved` instead of erasing the readable evidence around them.
    pub fn from_discovery(report: &DiscoverReport) -> Result<Catalog, NnError> {
        let mut sources = Vec::new();
        let mut tensors = Vec::new();
        let mut unresolved = Vec::new();
        let mut sources_parsed = 0usize;
        let mut tensors_exact = 0usize;
        let mut tensors_bounded = 0usize;

        for source in &report.sources {
            let Some(revision) = source.revision.as_ref() else {
                unresolved.push(UnresolvedMember {
                    path: source.path.clone(),
                    outcome: source.outcome.clone(),
                    note: source.notes.first().cloned().unwrap_or_default(),
                });
                continue;
            };
            let semantic = revision.semantic()?;
            let id = compute_id(IdKind::Source, &semantic)?;
            let (format, has_tensors) = match &source.inventory {
                Some(inventory) => (
                    Some(format!("{} {}", inventory.format, inventory.format_version)),
                    !inventory.tensors.is_empty(),
                ),
                None => (None, false),
            };
            if has_tensors || source.outcome == SourceOutcome::Parsed {
                sources_parsed += 1;
            }
            if source.outcome != SourceOutcome::Parsed && source.outcome != SourceOutcome::Invalid {
                // A revision exists but no container interpretation applied.
                unresolved.push(UnresolvedMember {
                    path: source.path.clone(),
                    outcome: source.outcome.clone(),
                    note: source.notes.first().cloned().unwrap_or_default(),
                });
            }
            sources.push(CatalogSource {
                id: id.clone(),
                semantic,
                outcome: source.outcome.clone(),
                path: source.path.clone(),
                format,
            });

            let Some(inventory) = &source.inventory else {
                continue;
            };
            for tensor in &inventory.tensors {
                let entry = catalog_tensor(&id, tensor)?;
                match entry.extent {
                    Extent::Exact => tensors_exact += 1,
                    Extent::UpperBoundOnly => tensors_bounded += 1,
                }
                tensors.push(entry);
            }
        }

        Ok(Catalog {
            coverage: CatalogCoverage {
                sources_total: sources.len(),
                sources_parsed,
                tensors_total: tensors.len(),
                tensors_exact_extent: tensors_exact,
                tensors_bounded_extent: tensors_bounded,
                unresolved_members: unresolved.len(),
            },
            sources,
            tensors,
            unresolved,
        })
    }

    /// Semantic payload of the catalog (canonicalizable; integers as decimal
    /// strings; source semantics are wrapped with their outcome; diagnostic
    /// message text and local paths of unresolved members are the only
    /// environment-dependent content and deliberately so — a catalog id names
    /// one snapshot of one scan).
    pub fn semantic(&self) -> Result<Json, NnError> {
        let sources = self
            .sources
            .iter()
            .map(|s| {
                Json::object(vec![
                    ("source", s.semantic.clone()),
                    ("outcome", Json::Str(s.outcome.as_str().to_string())),
                ])
            })
            .collect::<Result<Vec<_>, _>>()?;
        let tensors = self
            .tensors
            .iter()
            .map(|t| t.semantic.clone())
            .collect::<Vec<_>>();
        let unresolved = self
            .unresolved
            .iter()
            .map(|u| {
                Json::object(vec![
                    ("path", Json::Str(u.path.clone())),
                    ("outcome", Json::Str(u.outcome.as_str().to_string())),
                    ("note", Json::Str(u.note.clone())),
                ])
            })
            .collect::<Result<Vec<_>, _>>()?;
        Json::object(vec![
            ("schema", Json::Str("binfiddle.nn.catalog/v1".to_string())),
            ("sources", Json::Array(sources)),
            ("tensors", Json::Array(tensors)),
            ("unresolved", Json::Array(unresolved)),
            (
                "coverage",
                Json::object(vec![
                    (
                        "sources_total",
                        Json::Str(self.coverage.sources_total.to_string()),
                    ),
                    (
                        "sources_parsed",
                        Json::Str(self.coverage.sources_parsed.to_string()),
                    ),
                    (
                        "tensors_total",
                        Json::Str(self.coverage.tensors_total.to_string()),
                    ),
                    (
                        "tensors_exact_extent",
                        Json::Str(self.coverage.tensors_exact_extent.to_string()),
                    ),
                    (
                        "tensors_bounded_extent",
                        Json::Str(self.coverage.tensors_bounded_extent.to_string()),
                    ),
                    (
                        "unresolved_members",
                        Json::Str(self.coverage.unresolved_members.to_string()),
                    ),
                ])?,
            ),
        ])
    }

    /// Domain-separated catalog identifier.
    pub fn id(&self) -> Result<String, NnError> {
        compute_id(IdKind::Catalog, &self.semantic()?)
    }

    /// Serialize the catalog file: schema wrapper, embedded id (verified on
    /// load), and the semantic payload.
    pub fn to_file_json(&self) -> Result<String, NnError> {
        let presentation = Json::object(vec![(
            "sources",
            Json::Array(
                self.sources
                    .iter()
                    .map(|s| {
                        Json::object(vec![
                            ("path", Json::Str(s.path.clone())),
                            (
                                "format",
                                match &s.format {
                                    Some(format) => Json::Str(format.clone()),
                                    None => Json::Null,
                                },
                            ),
                        ])
                    })
                    .collect::<Result<Vec<_>, _>>()?,
            ),
        )])?;
        let file = Json::object(vec![
            (
                "schema",
                Json::Str("binfiddle.nn.catalog-file/v1".to_string()),
            ),
            ("catalog_id", Json::Str(self.id()?)),
            ("catalog", self.semantic()?),
            ("presentation", presentation),
        ])?;
        file.to_canonical()
    }

    /// Save to a path (creates or truncates; the caller owns overwrite policy).
    pub fn save(&self, path: &Path) -> Result<(), NnError> {
        let text = self.to_file_json()?;
        std::fs::write(path, text.as_bytes()).map_err(NnError::Io)?;
        Ok(())
    }

    /// Load and verify a catalog file. The embedded id must equal the digest
    /// recomputed from the semantic payload; mismatches and duplicate keys are
    /// errors, never silent reinterpretation.
    pub fn load(path: &Path) -> Result<Catalog, NnError> {
        let text = std::fs::read_to_string(path).map_err(NnError::Io)?;
        let file = Json::parse_strict(&text, ParseLimits::default())?;
        if file.get("schema").and_then(Json::as_str) != Some("binfiddle.nn.catalog-file/v1") {
            return Err(NnError::MalformedInput {
                detail: "not a binfiddle catalog file".to_string(),
            });
        }
        let embedded_id = file
            .get("catalog_id")
            .and_then(Json::as_str)
            .ok_or_else(|| NnError::MalformedInput {
                detail: "catalog file is missing catalog_id".to_string(),
            })?
            .to_string();
        let semantic = file
            .get("catalog")
            .cloned()
            .ok_or_else(|| NnError::MalformedInput {
                detail: "catalog file is missing the catalog payload".to_string(),
            })?;
        let computed = compute_id(IdKind::Catalog, &semantic)?;
        if computed != embedded_id {
            return Err(NnError::MalformedInput {
                detail: format!(
                    "catalog id mismatch: file claims {}, payload hashes to {}",
                    embedded_id, computed
                ),
            });
        }
        let mut catalog = catalog_from_semantic(&semantic)?;
        if let Some(presentation) = file.get("presentation") {
            catalog.apply_presentation(presentation)?;
        }
        Ok(catalog)
    }

    /// Apply the presentation section (paths, formats) to a loaded catalog.
    /// Shape mismatches are errors: a presentation that drifted from its own
    /// semantic payload indicates a corrupted file.
    fn apply_presentation(&mut self, presentation: &Json) -> Result<(), NnError> {
        let sources = presentation
            .get("sources")
            .and_then(Json::as_array)
            .ok_or_else(|| NnError::MalformedInput {
                detail: "presentation section is missing the sources array".to_string(),
            })?;
        if sources.len() != self.sources.len() {
            return Err(NnError::MalformedInput {
                detail: format!(
                    "presentation lists {} sources but the catalog has {}",
                    sources.len(),
                    self.sources.len()
                ),
            });
        }
        for (source, presented) in self.sources.iter_mut().zip(sources) {
            source.path = presented
                .get("path")
                .and_then(Json::as_str)
                .unwrap_or_default()
                .to_string();
            source.format = presented
                .get("format")
                .and_then(Json::as_str)
                .map(str::to_string);
        }
        Ok(())
    }

    /// Resolve a catalog from the one permitted input route: a previously
    /// saved catalog file, or discovery over a file/directory.
    pub fn from_route(
        catalog_file: Option<&Path>,
        input: Option<&Path>,
        options: &super::discover::DiscoverOptions,
        budget: &super::budget::Budget,
    ) -> Result<Catalog, NnError> {
        match (catalog_file, input) {
            (Some(_), Some(_)) => Err(NnError::InvalidRequest {
                message: "choose exactly one source route: --catalog or --input".to_string(),
            }),
            (Some(path), None) => Catalog::load(path),
            (None, Some(path)) => {
                let report = super::discover::discover(path, options, budget)?;
                Catalog::from_discovery(&report)
            }
            (None, None) => Err(NnError::InvalidRequest {
                message: "a source is required: --catalog <file> or --input <file-or-dir>"
                    .to_string(),
            }),
        }
    }

    /// Resolve tensors by exact original name. Multiple matches are returned
    /// so callers can require a scope; `source_scope` accepts a unique source
    /// id prefix or an exact path.
    pub fn tensors_by_name(
        &self,
        name: &str,
        source_scope: Option<&str>,
    ) -> Result<Vec<&CatalogTensor>, NnError> {
        let scope = match source_scope {
            Some(scope) => Some(self.resolve_source(scope)?),
            None => None,
        };
        Ok(self
            .tensors
            .iter()
            .filter(|t| t.original_name == name && scope.is_none_or(|s| s.id == t.source_id))
            .collect())
    }

    /// Resolve a tensor identifier: full `tensor:<64hex>` or a unique digest
    /// prefix. Ambiguous prefixes are rejected.
    pub fn tensor_by_id(&self, id_or_prefix: &str) -> Result<&CatalogTensor, NnError> {
        let matches: Vec<&CatalogTensor> = self
            .tensors
            .iter()
            .filter(|t| id_prefix_matches(&t.id, id_or_prefix))
            .collect();
        match matches.as_slice() {
            [one] => Ok(one),
            [] => Err(NnError::SourceMissing {
                detail: format!("no tensor id matches {}", super::error::brief(id_or_prefix)),
            }),
            _ => Err(NnError::AmbiguousBinding {
                detail: format!(
                    "tensor id prefix {} matches {} tensors; provide more digest characters",
                    super::error::brief(id_or_prefix),
                    matches.len()
                ),
            }),
        }
    }

    /// Resolve a source by full/prefix id or exact path.
    pub fn resolve_source(&self, id_path_or_prefix: &str) -> Result<&CatalogSource, NnError> {
        let matches: Vec<&CatalogSource> = self
            .sources
            .iter()
            .filter(|s| {
                id_prefix_matches(&s.id, id_path_or_prefix)
                    || s.path == id_path_or_prefix
                    || std::path::Path::new(&s.path)
                        .file_name()
                        .is_some_and(|n| n == id_path_or_prefix)
            })
            .collect();
        match matches.as_slice() {
            [one] => Ok(one),
            [] => Err(NnError::SourceMissing {
                detail: format!(
                    "no source matches {}",
                    super::error::brief(id_path_or_prefix)
                ),
            }),
            _ => Err(NnError::AmbiguousBinding {
                detail: format!(
                    "source reference {} is ambiguous across {} sources",
                    super::error::brief(id_path_or_prefix),
                    matches.len()
                ),
            }),
        }
    }
}

/// Match a full `kind:<digest>` id against a reference that may be a full id,
/// a `kind:`-prefixed prefix, or a bare digest prefix.
fn id_prefix_matches(full: &str, reference: &str) -> bool {
    if reference.is_empty() {
        return false;
    }
    if reference.contains(':') {
        return full.starts_with(reference);
    }
    // Bare digest prefix: compare against the digest part.
    full.rsplit(':')
        .next()
        .is_some_and(|digest| digest.starts_with(reference))
}

fn catalog_tensor(source_id: &str, tensor: &TensorEntry) -> Result<CatalogTensor, NnError> {
    let semantic = Json::object(vec![
        ("schema", Json::Str("binfiddle.nn.tensor/v1".to_string())),
        ("source_id", Json::Str(source_id.to_string())),
        ("original_name", Json::Str(tensor.original_name.clone())),
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
        ("encoding_id", Json::Str(tensor.encoding.clone())),
        ("payload_start", Json::Str(tensor.payload_start.to_string())),
        (
            "payload_length",
            match tensor.payload_length {
                Some(len) => Json::Str(len.to_string()),
                None => Json::Null,
            },
        ),
    ])?;
    let id = compute_id(IdKind::Tensor, &semantic)?;
    Ok(CatalogTensor {
        id,
        semantic,
        source_id: source_id.to_string(),
        original_name: tensor.original_name.clone(),
        encoding: tensor.encoding.clone(),
        decode_supported: tensor.decode_supported,
        shape: tensor.shape.clone(),
        element_count: tensor.element_count,
        payload_start: tensor.payload_start,
        payload_length: tensor.payload_length,
        extent: tensor.extent,
    })
}

/// Rebuild an in-memory catalog from its verified semantic payload.
fn catalog_from_semantic(semantic: &Json) -> Result<Catalog, NnError> {
    if semantic.get("schema").and_then(Json::as_str) != Some("binfiddle.nn.catalog/v1") {
        return Err(NnError::MalformedInput {
            detail: "catalog payload schema mismatch".to_string(),
        });
    }
    let sources_json = semantic
        .get("sources")
        .and_then(Json::as_array)
        .ok_or_else(|| NnError::MalformedInput {
            detail: "catalog payload is missing the sources array".to_string(),
        })?;
    let tensors_json = semantic
        .get("tensors")
        .and_then(Json::as_array)
        .ok_or_else(|| NnError::MalformedInput {
            detail: "catalog payload is missing the tensors array".to_string(),
        })?;
    let unresolved_json = semantic
        .get("unresolved")
        .and_then(Json::as_array)
        .ok_or_else(|| NnError::MalformedInput {
            detail: "catalog payload is missing the unresolved array".to_string(),
        })?;

    let mut sources = Vec::with_capacity(sources_json.len());
    let mut sources_parsed = 0usize;
    for wrapper in sources_json {
        let source_semantic =
            wrapper
                .get("source")
                .cloned()
                .ok_or_else(|| NnError::MalformedInput {
                    detail: "catalog source wrapper is missing the source payload".to_string(),
                })?;
        let id = compute_id(IdKind::Source, &source_semantic)?;
        let outcome = parse_outcome(
            wrapper
                .get("outcome")
                .and_then(Json::as_str)
                .unwrap_or("parsed"),
        );
        if matches!(outcome, SourceOutcome::Parsed) {
            sources_parsed += 1;
        }
        sources.push(CatalogSource {
            id,
            semantic: source_semantic,
            outcome,
            path: String::new(),
            format: None,
        });
    }

    let mut tensors = Vec::with_capacity(tensors_json.len());
    let mut tensors_exact = 0usize;
    let mut tensors_bounded = 0usize;
    for tensor_semantic in tensors_json {
        let id = compute_id(IdKind::Tensor, tensor_semantic)?;
        let source_id = tensor_semantic
            .get("source_id")
            .and_then(Json::as_str)
            .unwrap_or_default()
            .to_string();
        let original_name = tensor_semantic
            .get("original_name")
            .and_then(Json::as_str)
            .unwrap_or_default()
            .to_string();
        let encoding = tensor_semantic
            .get("encoding_id")
            .and_then(Json::as_str)
            .unwrap_or_default()
            .to_string();
        let shape = tensor_semantic
            .get("shape")
            .and_then(Json::as_array)
            .map(|dims| {
                dims.iter()
                    .filter_map(|d| d.as_str().and_then(|s| s.parse::<u64>().ok()))
                    .collect::<Vec<u64>>()
            })
            .unwrap_or_default();
        let payload_start = tensor_semantic
            .get("payload_start")
            .and_then(Json::as_str)
            .and_then(|s| s.parse::<u64>().ok())
            .unwrap_or(0);
        let payload_length = tensor_semantic
            .get("payload_length")
            .and_then(Json::as_str)
            .and_then(|s| s.parse::<u64>().ok());
        let extent = if payload_length.is_some() {
            tensors_exact += 1;
            Extent::Exact
        } else {
            tensors_bounded += 1;
            Extent::UpperBoundOnly
        };
        let element_count = shape.iter().product::<u64>();
        tensors.push(CatalogTensor {
            id,
            semantic: tensor_semantic.clone(),
            source_id,
            original_name,
            decode_supported: encoding_decode_supported(&encoding),
            encoding,
            shape,
            element_count,
            payload_start,
            payload_length,
            extent,
        });
    }

    let unresolved = unresolved_json
        .iter()
        .map(|u| UnresolvedMember {
            path: u
                .get("path")
                .and_then(Json::as_str)
                .unwrap_or_default()
                .to_string(),
            outcome: parse_outcome(
                u.get("outcome")
                    .and_then(Json::as_str)
                    .unwrap_or("unrecognized"),
            ),
            note: u
                .get("note")
                .and_then(Json::as_str)
                .unwrap_or_default()
                .to_string(),
        })
        .collect::<Vec<_>>();

    Ok(Catalog {
        coverage: CatalogCoverage {
            sources_total: sources.len(),
            sources_parsed,
            tensors_total: tensors.len(),
            tensors_exact_extent: tensors_exact,
            tensors_bounded_extent: tensors_bounded,
            unresolved_members: unresolved.len(),
        },
        sources,
        tensors,
        unresolved,
    })
}

fn parse_outcome(text: &str) -> SourceOutcome {
    match text {
        "parsed" => SourceOutcome::Parsed,
        "invalid" => SourceOutcome::Invalid,
        "malformed" => SourceOutcome::Malformed,
        "unreadable" => SourceOutcome::Unreadable,
        _ => SourceOutcome::Unrecognized,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::nn::budget::{Budget, BudgetCaps};
    use crate::nn::cancel::CancellationToken;
    use crate::nn::discover::{discover, DiscoverOptions};

    fn budget() -> Budget {
        Budget::new(BudgetCaps::default(), None, CancellationToken::new())
    }

    fn write_safetensors(dir: &Path, name: &str, tensors: &[(&str, &str, &[u64], &[u8])]) {
        let mut body: Vec<u8> = Vec::new();
        let mut spans = Vec::new();
        for (tname, dtype, shape, payload) in tensors {
            let begin = body.len();
            body.extend_from_slice(payload);
            spans.push((tname, dtype, shape, begin, begin + payload.len()));
        }
        let entries: Vec<String> = spans
            .iter()
            .map(|(n, dt, sh, b, e)| {
                let dims: Vec<String> = sh.iter().map(|d| d.to_string()).collect();
                format!(
                    "\"{}\":{{\"dtype\":\"{}\",\"shape\":[{}],\"data_offsets\":[{},{}]}}",
                    n,
                    dt,
                    dims.join(","),
                    b,
                    e
                )
            })
            .collect();
        let header = format!("{{{}}}", entries.join(","));
        let mut data = Vec::new();
        data.extend_from_slice(&(header.len() as u64).to_le_bytes());
        data.extend_from_slice(header.as_bytes());
        data.extend_from_slice(&body);
        std::fs::write(dir.join(name), data).unwrap();
    }

    fn sample_report(dir: &Path) -> DiscoverReport {
        write_safetensors(
            dir,
            "m.safetensors",
            &[
                ("model.w1", "F32", &[2, 3], &[0u8; 24]),
                ("model.w2", "U8", &[4], &[1, 2, 3, 4]),
            ],
        );
        discover(
            &dir.join("m.safetensors"),
            &DiscoverOptions::default(),
            &budget(),
        )
        .unwrap()
    }

    #[test]
    fn builds_catalog_with_stable_ids() {
        let dir = tempfile::tempdir().unwrap();
        let catalog = Catalog::from_discovery(&sample_report(dir.path())).unwrap();
        assert_eq!(catalog.coverage.sources_total, 1);
        assert_eq!(catalog.coverage.tensors_total, 2);
        assert_eq!(catalog.coverage.tensors_exact_extent, 2);
        let id = catalog.id().unwrap();
        assert!(id.starts_with("catalog:"));
        let report2 = sample_report(dir.path());
        assert_eq!(Catalog::from_discovery(&report2).unwrap().id().unwrap(), id);
    }

    #[test]
    fn save_and_load_round_trip_preserves_ids_and_tensors() {
        let dir = tempfile::tempdir().unwrap();
        let catalog = Catalog::from_discovery(&sample_report(dir.path())).unwrap();
        let path = dir.path().join("model.nn.json");
        catalog.save(&path).unwrap();
        let loaded = Catalog::load(&path).unwrap();
        assert_eq!(loaded.id().unwrap(), catalog.id().unwrap());
        assert_eq!(loaded.tensors.len(), 2);
        assert_eq!(loaded.tensors[0].original_name, "model.w1");
        assert_eq!(loaded.tensors[0].shape, vec![2, 3]);
        assert_eq!(loaded.coverage, catalog.coverage);
        for (a, b) in catalog.tensors.iter().zip(&loaded.tensors) {
            assert_eq!(a.id, b.id);
            assert_eq!(a.decode_supported, b.decode_supported);
        }
    }

    #[test]
    fn tampered_catalog_file_is_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let catalog = Catalog::from_discovery(&sample_report(dir.path())).unwrap();
        let path = dir.path().join("model.nn.json");
        catalog.save(&path).unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        let tampered = text.replace("model.w1", "model.wX");
        assert_ne!(tampered, text);
        std::fs::write(&path, tampered).unwrap();
        let err = Catalog::load(&path).unwrap_err();
        assert_eq!(err.code().as_str(), "MALFORMED_INPUT");
        assert!(err.to_string().contains("mismatch"));
    }

    #[test]
    fn duplicate_keys_in_catalog_file_are_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("dup.nn.json");
        std::fs::write(
            &path,
            r#"{"schema":"binfiddle.nn.catalog-file/v1","schema":"binfiddle.nn.catalog-file/v1"}"#,
        )
        .unwrap();
        assert_eq!(
            Catalog::load(&path).unwrap_err().code().as_str(),
            "WIRE_SYNTAX"
        );
    }

    #[test]
    fn unrecognized_members_stay_visible() {
        let dir = tempfile::tempdir().unwrap();
        write_safetensors(dir.path(), "a.safetensors", &[("w", "U8", &[1], &[1])]);
        std::fs::write(
            dir.path().join("junk.safetensors"),
            b"garbage that is not a model",
        )
        .unwrap();
        let report = discover(dir.path(), &DiscoverOptions::default(), &budget()).unwrap();
        let catalog = Catalog::from_discovery(&report).unwrap();
        assert_eq!(catalog.tensors.len(), 1);
        assert_eq!(catalog.coverage.unresolved_members, 1);
        assert_eq!(catalog.unresolved[0].outcome, SourceOutcome::Unrecognized);
        // Unresolved members persist through save/load.
        let path = dir.path().join("c.nn.json");
        catalog.save(&path).unwrap();
        let loaded = Catalog::load(&path).unwrap();
        assert_eq!(loaded.unresolved.len(), 1);
    }

    #[test]
    fn exact_name_resolution_reports_ambiguity_and_scope_narrows() {
        let dir = tempfile::tempdir().unwrap();
        write_safetensors(
            dir.path(),
            "a.safetensors",
            &[("shared.w", "U8", &[1], &[1])],
        );
        write_safetensors(
            dir.path(),
            "b.safetensors",
            &[("shared.w", "U8", &[1], &[2])],
        );
        let report = discover(dir.path(), &DiscoverOptions::default(), &budget()).unwrap();
        let catalog = Catalog::from_discovery(&report).unwrap();

        let matches = catalog.tensors_by_name("shared.w", None).unwrap();
        assert_eq!(matches.len(), 2);
        let scoped = catalog
            .tensors_by_name("shared.w", Some("a.safetensors"))
            .unwrap();
        assert_eq!(scoped.len(), 1);
        assert_eq!(scoped[0].source_id, catalog.sources[0].id);
    }

    #[test]
    fn id_prefix_resolution_rejects_ambiguity() {
        let dir = tempfile::tempdir().unwrap();
        let catalog = Catalog::from_discovery(&sample_report(dir.path())).unwrap();
        let id = catalog.tensors[0].id.clone();
        assert_eq!(catalog.tensor_by_id(&id).unwrap().id, id);
        let prefix = id
            .trim_start_matches("tensor:")
            .chars()
            .take(12)
            .collect::<String>();
        assert_eq!(catalog.tensor_by_id(&prefix).unwrap().id, id);
        assert_eq!(
            catalog.tensor_by_id("tensor:").unwrap_err().code().as_str(),
            "AMBIGUOUS_BINDING"
        );
        let miss = format!("tensor:{}", "0".repeat(64));
        assert_eq!(
            catalog.tensor_by_id(&miss).unwrap_err().code().as_str(),
            "SOURCE_MISSING"
        );
    }

    #[test]
    fn semantic_is_canonicalizable() {
        let dir = tempfile::tempdir().unwrap();
        let catalog = Catalog::from_discovery(&sample_report(dir.path())).unwrap();
        assert!(catalog.semantic().unwrap().to_canonical().is_ok());
    }
}
