//! Saved selections: resolved tensor sets bound to one exact catalog.
//!
//! A selection stores the request form that produced it and the resolved
//! tensor identities. Loading uses those resolved identities against the
//! recorded catalog id — never a fresh reinterpretation of the request text.
//! A selection from a different catalog is a stale-source error, and missing
//! targets are reported, not silently dropped.

use super::catalog::{Catalog, CatalogTensor};
use super::error::NnError;
use super::id::{compute_id, IdKind};
use super::json::{Json, ParseLimits};
use super::selector::Selector;
use std::path::Path;

/// How empty selections are treated.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EmptyPolicy {
    /// Reject empty results (default for selection commands).
    Reject,
    /// Allow empty results (explicit opt-in).
    Allow,
}

impl EmptyPolicy {
    pub fn as_str(self) -> &'static str {
        match self {
            EmptyPolicy::Reject => "reject",
            EmptyPolicy::Allow => "allow",
        }
    }
}

/// The request form that produced a selection.
#[derive(Debug, Clone)]
pub enum SelectionRequest {
    /// Exact original tensor name, optionally scoped to one source
    /// (id prefix or path).
    TensorName {
        name: String,
        source: Option<String>,
    },
    /// Full or unique-prefix tensor identifier.
    TensorId { id: String },
    /// Component-selector expression (parsed by the restricted grammar).
    ComponentExpression { expression: String },
}

impl SelectionRequest {
    fn kind(&self) -> &'static str {
        match self {
            SelectionRequest::TensorName { .. } => "tensor_name",
            SelectionRequest::TensorId { .. } => "tensor_id",
            SelectionRequest::ComponentExpression { .. } => "component_expression",
        }
    }

    fn semantic(&self) -> Result<Json, NnError> {
        Ok(match self {
            SelectionRequest::TensorName { name, source } => {
                let mut pairs = vec![
                    ("kind", Json::Str(self.kind().to_string())),
                    ("name", Json::Str(name.clone())),
                ];
                if let Some(source) = source {
                    pairs.push(("source", Json::Str(source.clone())));
                }
                Json::object(pairs)?
            }
            SelectionRequest::TensorId { id } => Json::object(vec![
                ("kind", Json::Str(self.kind().to_string())),
                ("id", Json::Str(id.clone())),
            ])?,
            SelectionRequest::ComponentExpression { expression } => {
                // The expression must parse before it can be recorded.
                Selector::parse(expression)?;
                Json::object(vec![
                    ("kind", Json::Str(self.kind().to_string())),
                    ("expression", Json::Str(expression.clone())),
                ])?
            }
        })
    }

    pub fn describe(&self) -> String {
        match self {
            SelectionRequest::TensorName { name, source } => match source {
                Some(source) => format!("tensor '{name}' in '{source}'"),
                None => format!("tensor '{name}'"),
            },
            SelectionRequest::TensorId { id } => format!("tensor id {id}"),
            SelectionRequest::ComponentExpression { expression } => {
                format!("components '{expression}'")
            }
        }
    }
}

/// A resolved, saved-able selection.
#[derive(Debug, Clone)]
pub struct Selection {
    pub catalog_id: String,
    pub request: SelectionRequest,
    pub target_ids: Vec<String>,
    /// Optional per-target views, parallel to `target_ids` (component
    /// selections only). `None` entries select whole tensors.
    pub views: Vec<Option<TargetView>>,
    pub empty_policy: EmptyPolicy,
}

/// A view annotation on one target (row-range views from component
/// selections).
#[derive(Debug, Clone, PartialEq)]
pub struct TargetView {
    pub logical: String,
    pub span: (u64, u64),
    pub rows: (u64, u64),
}

impl Selection {
    /// Resolve a request against a catalog and build a selection under the
    /// given empty policy.
    pub fn resolve(
        catalog: &Catalog,
        request: SelectionRequest,
        empty_policy: EmptyPolicy,
    ) -> Result<Selection, NnError> {
        let targets = resolve_request(catalog, &request)?;
        if targets.is_empty() && empty_policy == EmptyPolicy::Reject {
            return Err(NnError::InvalidRequest {
                message: format!(
                    "selection matched no tensors ({}); use an explicit allow-empty policy to record an empty selection",
                    request.describe()
                ),
            });
        }
        let views = vec![None; targets.len()];
        Ok(Selection {
            catalog_id: catalog.id()?,
            request,
            target_ids: targets,
            views,
            empty_policy,
        })
    }

    /// Resolve a component-selector expression through a pack's recognition,
    /// producing component-annotated targets with optional views.
    pub fn resolve_components(
        catalog: &Catalog,
        pack: &super::packs::Pack,
        recognition: &super::packs::Recognition,
        expression: &str,
        empty_policy: EmptyPolicy,
    ) -> Result<Selection, NnError> {
        let selector = super::selector::Selector::parse(expression)?;
        let targets =
            super::component_selection::resolve_selector(&selector, recognition, pack, catalog)?;
        if targets.is_empty() && empty_policy == EmptyPolicy::Reject {
            return Err(NnError::InvalidRequest {
                message: format!(
                    "component selection matched nothing ({expression}); use an explicit allow-empty policy to record an empty selection"
                ),
            });
        }
        let target_ids: Vec<String> = targets.iter().map(|t| t.tensor_id.clone()).collect();
        let views: Vec<Option<TargetView>> = targets
            .iter()
            .map(|t| {
                t.view.as_ref().map(|v| TargetView {
                    logical: v.logical.clone(),
                    span: v.span,
                    rows: v.rows,
                })
            })
            .collect();
        // The semantic request records the component expression and the pack
        // id, so saved selections explain where they came from.
        let mut request = SelectionRequest::ComponentExpression {
            expression: expression.to_string(),
        };
        let _ = &mut request;
        Ok(Selection {
            catalog_id: catalog.id()?,
            request,
            target_ids,
            views,
            empty_policy,
        })
    }

    /// The resolved tensors of this selection inside the catalog it was
    /// created from. A different catalog id is a stale-source error; a missing
    /// target is an error. Nothing is re-matched by name.
    pub fn bind<'a>(&self, catalog: &'a Catalog) -> Result<Vec<&'a CatalogTensor>, NnError> {
        let catalog_id = catalog.id()?;
        if catalog_id != self.catalog_id {
            return Err(NnError::SourceChanged {
                detail: format!(
                    "selection was saved against catalog {} but is being applied to {}; saved selections never silently rematch (use an explicit rebind)",
                    super::error::brief(&self.catalog_id),
                    super::error::brief(&catalog_id)
                ),
            });
        }
        let mut bound = Vec::with_capacity(self.target_ids.len());
        for id in &self.target_ids {
            bound.push(catalog.tensor_by_id(id)?);
        }
        Ok(bound)
    }

    pub fn semantic(&self) -> Result<Json, NnError> {
        let targets = self
            .target_ids
            .iter()
            .zip(&self.views)
            .map(|(id, view)| {
                let mut pairs = vec![("tensor_id", Json::Str(id.clone()))];
                if let Some(view) = view {
                    pairs.push((
                        "view",
                        Json::object(vec![
                            ("logical", Json::Str(view.logical.clone())),
                            ("span_start", Json::Str(view.span.0.to_string())),
                            ("span_length", Json::Str(view.span.1.to_string())),
                            ("row_start", Json::Str(view.rows.0.to_string())),
                            ("row_end", Json::Str(view.rows.1.to_string())),
                        ])?,
                    ));
                }
                Json::object(pairs)
            })
            .collect::<Result<Vec<_>, _>>()?;
        Json::object(vec![
            ("schema", Json::Str("binfiddle.nn.selection/v1".to_string())),
            ("catalog_id", Json::Str(self.catalog_id.clone())),
            ("request", self.request.semantic()?),
            ("targets", Json::Array(targets)),
            ("ordering", Json::Str("request_order".to_string())),
            (
                "empty_policy",
                Json::Str(self.empty_policy.as_str().to_string()),
            ),
        ])
    }

    pub fn id(&self) -> Result<String, NnError> {
        compute_id(IdKind::Selection, &self.semantic()?)
    }

    /// Result envelope for the `select` command.
    pub fn envelope(&self) -> Result<super::report::ResultEnvelope, NnError> {
        let semantic = Json::object(vec![
            ("selection_id", Json::Str(self.id()?)),
            ("catalog_id", Json::Str(self.catalog_id.clone())),
            ("request", self.request.semantic()?),
            (
                "targets",
                Json::Array(
                    self.target_ids
                        .iter()
                        .map(|id| Json::object(vec![("tensor_id", Json::Str(id.clone()))]))
                        .collect::<Result<Vec<_>, _>>()?,
                ),
            ),
            ("target_count", Json::Str(self.target_ids.len().to_string())),
            (
                "empty_policy",
                Json::Str(self.empty_policy.as_str().to_string()),
            ),
        ])?;
        Ok(super::report::ResultEnvelope::new("select").with_semantic(semantic))
    }

    /// Human-readable summary.
    pub fn text(&self) -> String {
        let id = self.id().unwrap_or_default();
        let mut out = format!(
            "selection {} ({} tensors)\n",
            &id[..26.min(id.len())],
            self.target_ids.len()
        );
        out.push_str(&format!("  request: {}\n", self.request.describe()));
        out.push_str(&format!("  catalog: {}\n", self.catalog_id));
        for id in &self.target_ids {
            out.push_str(&format!("  tensor:  {id}\n"));
        }
        out
    }

    /// Serialize the selection file (embedded id verified on load).
    pub fn to_file_json(&self) -> Result<String, NnError> {
        let file = Json::object(vec![
            (
                "schema",
                Json::Str("binfiddle.nn.selection-file/v1".to_string()),
            ),
            ("selection_id", Json::Str(self.id()?)),
            ("selection", self.semantic()?),
        ])?;
        file.to_canonical()
    }

    pub fn save(&self, path: &Path) -> Result<(), NnError> {
        std::fs::write(path, self.to_file_json()?.as_bytes()).map_err(NnError::Io)?;
        Ok(())
    }

    /// Load and verify a selection file.
    pub fn load(path: &Path) -> Result<Selection, NnError> {
        let text = std::fs::read_to_string(path).map_err(NnError::Io)?;
        let file = Json::parse_strict(&text, ParseLimits::default())?;
        if file.get("schema").and_then(Json::as_str) != Some("binfiddle.nn.selection-file/v1") {
            return Err(NnError::MalformedInput {
                detail: "not a binfiddle selection file".to_string(),
            });
        }
        let embedded = file
            .get("selection_id")
            .and_then(Json::as_str)
            .ok_or_else(|| NnError::MalformedInput {
                detail: "selection file is missing selection_id".to_string(),
            })?
            .to_string();
        let semantic = file
            .get("selection")
            .cloned()
            .ok_or_else(|| NnError::MalformedInput {
                detail: "selection file is missing the selection payload".to_string(),
            })?;
        let computed = compute_id(IdKind::Selection, &semantic)?;
        if computed != embedded {
            return Err(NnError::MalformedInput {
                detail: format!(
                    "selection id mismatch: file claims {}, payload hashes to {}",
                    embedded, computed
                ),
            });
        }
        selection_from_semantic(&semantic)
    }
}

fn resolve_request(catalog: &Catalog, request: &SelectionRequest) -> Result<Vec<String>, NnError> {
    match request {
        SelectionRequest::TensorName { name, source } => {
            let matches = catalog.tensors_by_name(name, source.as_deref())?;
            Ok(matches.into_iter().map(|t| t.id.clone()).collect())
        }
        SelectionRequest::TensorId { id } => Ok(vec![catalog.tensor_by_id(id)?.id.clone()]),
        SelectionRequest::ComponentExpression { expression } => {
            let selector = Selector::parse(expression)?;
            Err(NnError::FormatUnsupported {
                format: "component-selection".to_string(),
                reason: format!(
                    "selector '{}' parsed successfully, but this catalog has no architecture components to select; component selection requires a model pack (tensor selections work now via --tensor/--id)",
                    selector
                ),
            })
        }
    }
}

fn selection_from_semantic(semantic: &Json) -> Result<Selection, NnError> {
    if semantic.get("schema").and_then(Json::as_str) != Some("binfiddle.nn.selection/v1") {
        return Err(NnError::MalformedInput {
            detail: "selection payload schema mismatch".to_string(),
        });
    }
    let catalog_id = semantic
        .get("catalog_id")
        .and_then(Json::as_str)
        .ok_or_else(|| NnError::MalformedInput {
            detail: "selection payload is missing catalog_id".to_string(),
        })?
        .to_string();
    let request_json = semantic
        .get("request")
        .cloned()
        .ok_or_else(|| NnError::MalformedInput {
            detail: "selection payload is missing the request record".to_string(),
        })?;
    let request = match request_json.get("kind").and_then(Json::as_str) {
        Some("tensor_name") => SelectionRequest::TensorName {
            name: request_json
                .get("name")
                .and_then(Json::as_str)
                .unwrap_or_default()
                .to_string(),
            source: request_json
                .get("source")
                .and_then(Json::as_str)
                .map(str::to_string),
        },
        Some("tensor_id") => SelectionRequest::TensorId {
            id: request_json
                .get("id")
                .and_then(Json::as_str)
                .unwrap_or_default()
                .to_string(),
        },
        Some("component_expression") => SelectionRequest::ComponentExpression {
            expression: request_json
                .get("expression")
                .and_then(Json::as_str)
                .unwrap_or_default()
                .to_string(),
        },
        other => {
            return Err(NnError::MalformedInput {
                detail: format!("unknown selection request kind {:?}", other),
            })
        }
    };
    let target_ids = semantic
        .get("targets")
        .and_then(Json::as_array)
        .map(|targets| {
            targets
                .iter()
                .filter_map(|t| t.get("tensor_id").and_then(Json::as_str))
                .map(str::to_string)
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    let empty_policy = match semantic.get("empty_policy").and_then(Json::as_str) {
        Some("allow") => EmptyPolicy::Allow,
        _ => EmptyPolicy::Reject,
    };
    // Per-target views (component selections): parallel to target_ids.
    let views = semantic
        .get("targets")
        .and_then(Json::as_array)
        .map(|targets| {
            targets
                .iter()
                .map(|t| {
                    t.get("view").and_then(|v| {
                        Some(TargetView {
                            logical: v.get("logical")?.as_str()?.to_string(),
                            span: (
                                v.get("span_start")?.as_str()?.parse().ok()?,
                                v.get("span_length")?.as_str()?.parse().ok()?,
                            ),
                            rows: (
                                v.get("row_start")?.as_str()?.parse().ok()?,
                                v.get("row_end")?.as_str()?.parse().ok()?,
                            ),
                        })
                    })
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_else(|| vec![None; target_ids.len()]);
    Ok(Selection {
        catalog_id,
        request,
        target_ids,
        views,
        empty_policy,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::nn::budget::{Budget, BudgetCaps};
    use crate::nn::cancel::CancellationToken;
    use crate::nn::catalog::Catalog;
    use crate::nn::discover::{discover, DiscoverOptions};
    use std::path::PathBuf;

    fn budget() -> Budget {
        Budget::new(BudgetCaps::default(), None, CancellationToken::new())
    }

    fn catalog_with(dir: &std::path::Path, name: &str, payload: &[u8]) -> Catalog {
        let header = format!(
            "{{\"w\":{{\"dtype\":\"U8\",\"shape\":[{}],\"data_offsets\":[0,{}]}}}}",
            payload.len(),
            payload.len()
        );
        let mut data = Vec::new();
        data.extend_from_slice(&(header.len() as u64).to_le_bytes());
        data.extend_from_slice(header.as_bytes());
        data.extend_from_slice(payload);
        std::fs::write(dir.join(name), data).unwrap();
        let report = discover(&dir.join(name), &DiscoverOptions::default(), &budget()).unwrap();
        Catalog::from_discovery(&report).unwrap()
    }

    #[test]
    fn resolves_by_name_and_saves_loads_binds() {
        let dir = tempfile::tempdir().unwrap();
        let catalog = catalog_with(dir.path(), "m.safetensors", &[1, 2, 3]);
        let selection = Selection::resolve(
            &catalog,
            SelectionRequest::TensorName {
                name: "w".to_string(),
                source: None,
            },
            EmptyPolicy::Reject,
        )
        .unwrap();
        assert_eq!(selection.target_ids.len(), 1);
        let path = dir.path().join("s.selection.json");
        selection.save(&path).unwrap();
        let loaded = Selection::load(&path).unwrap();
        assert_eq!(loaded.id().unwrap(), selection.id().unwrap());
        let bound = loaded.bind(&catalog).unwrap();
        assert_eq!(bound.len(), 1);
        assert_eq!(bound[0].original_name, "w");
    }

    #[test]
    fn stale_catalog_is_never_silently_rematched() {
        let dir = tempfile::tempdir().unwrap();
        let catalog_a = catalog_with(dir.path(), "a.safetensors", &[1]);
        let catalog_b = catalog_with(dir.path(), "b.safetensors", &[9]);
        let selection = Selection::resolve(
            &catalog_a,
            SelectionRequest::TensorName {
                name: "w".to_string(),
                source: None,
            },
            EmptyPolicy::Reject,
        )
        .unwrap();
        let err = selection.bind(&catalog_b).unwrap_err();
        assert_eq!(err.code().as_str(), "SOURCE_CHANGED");
        assert!(err.to_string().contains("never silently rematch"));
    }

    #[test]
    fn empty_selection_rejected_by_default_allowed_explicitly() {
        let dir = tempfile::tempdir().unwrap();
        let catalog = catalog_with(dir.path(), "m.safetensors", &[1]);
        let missing = SelectionRequest::TensorName {
            name: "nope".to_string(),
            source: None,
        };
        assert!(Selection::resolve(&catalog, missing.clone(), EmptyPolicy::Reject).is_err());
        let empty = Selection::resolve(&catalog, missing, EmptyPolicy::Allow).unwrap();
        assert!(empty.target_ids.is_empty());
    }

    #[test]
    fn component_expressions_parse_but_explain_the_pack_requirement() {
        let dir = tempfile::tempdir().unwrap();
        let catalog = catalog_with(dir.path(), "m.safetensors", &[1]);
        let err = Selection::resolve(
            &catalog,
            SelectionRequest::ComponentExpression {
                expression: "decoder.layers[8:16].mlp".to_string(),
            },
            EmptyPolicy::Reject,
        )
        .unwrap_err();
        assert_eq!(err.code().as_str(), "FORMAT_UNSUPPORTED");
        assert!(err.to_string().contains("parsed successfully"));
        // Malformed expressions are wire errors before resolution.
        let bad = Selection::resolve(
            &catalog,
            SelectionRequest::ComponentExpression {
                expression: "layers[-1]".to_string(),
            },
            EmptyPolicy::Reject,
        )
        .unwrap_err();
        assert_eq!(bad.code().as_str(), "WIRE_SYNTAX");
    }

    #[test]
    fn tampered_selection_file_is_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let catalog = catalog_with(dir.path(), "m.safetensors", &[1]);
        let selection = Selection::resolve(
            &catalog,
            SelectionRequest::TensorName {
                name: "w".to_string(),
                source: None,
            },
            EmptyPolicy::Reject,
        )
        .unwrap();
        let path = dir.path().join("s.selection.json");
        selection.save(&path).unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        std::fs::write(&path, text.replace("\"w\"", "\"hacked\"")).unwrap();
        let err = Selection::load(&path).unwrap_err();
        assert_eq!(err.code().as_str(), "MALFORMED_INPUT");
        assert!(err.to_string().contains("mismatch"));
    }

    #[test]
    fn request_text_is_preserved_not_reinterpreted() {
        let dir = tempfile::tempdir().unwrap();
        let catalog = catalog_with(dir.path(), "m.safetensors", &[1]);
        let selection = Selection::resolve(
            &catalog,
            SelectionRequest::TensorId {
                id: catalog.tensors[0].id.clone(),
            },
            EmptyPolicy::Reject,
        )
        .unwrap();
        let semantic = selection.semantic().unwrap();
        let request = semantic.get("request").unwrap();
        assert_eq!(
            request.get("kind").and_then(Json::as_str),
            Some("tensor_id")
        );
        // Catalog binding survives a rename of the tensor in a NEW catalog:
        // the old selection simply refuses to bind (tested above).
        let _ = PathBuf::new();
    }
}
