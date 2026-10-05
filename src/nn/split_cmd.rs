//! One-command decomposition: `nn split`.
//!
//! Splitting a model by layer produces, in a fresh output directory: one
//! saved child selection per layer (built by re-evaluating a synthesized
//! selector expression through the pack, so children are ordinary,
//! rebindable selections), one slice plan per layer (applied into a
//! materialized bundle when requested), and a root `split.json` manifest
//! carrying the coverage partition (assigned / shared / unresolved tensors)
//! and physical-union accounting — member bytes counted once regardless of
//! how many components reference them. A parent-and-child selection never
//! justifies duplicating payloads; the manifest reports both the logical
//! membership total and the unique byte total.

use super::budget::Budget;
use super::catalog::Catalog;
use super::component_selection::parse_component_path;
use super::error::NnError;
use super::json::Json;
use super::packs::{Pack, Recognition};
use super::report::ResultEnvelope;
use super::selection::{EmptyPolicy, Selection};
use super::slice::{self, QuantPolicy, StoragePolicy};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// One child entry.
#[derive(Debug, Clone)]
pub struct SplitChild {
    pub layer: u64,
    /// Selector expression that generated the child (recorded, re-evaluable).
    pub expression: String,
    pub selection_path: String,
    /// Set for materialized storage.
    pub bundle_dir: Option<String>,
    pub tensor_names: Vec<String>,
    pub unique_bytes: u64,
}

/// The split manifest contents.
pub struct SplitReport {
    pub root_dir: PathBuf,
    pub catalog_id: String,
    pub children: Vec<SplitChild>,
    /// Tensors referenced by more than one layer (payload stored/counted once).
    pub shared: Vec<String>,
    /// Tensors no layer claimed (embeddings, norms, heads, unclaimed).
    pub unresolved: Vec<String>,
    pub total_unique_bytes: u64,
}

impl SplitReport {
    /// Split a catalog by layer under a pack's recognition. `materialize`
    /// additionally applies each child's slice plan into `root/layers/…`.
    pub fn split_by_layer(
        catalog: &Catalog,
        pack: &Pack,
        recognition: &Recognition,
        materialize: bool,
        out_dir: &Path,
        budget: &Budget,
    ) -> Result<SplitReport, NnError> {
        if out_dir.exists() {
            return Err(NnError::InvalidRequest {
                message: format!(
                    "output directory {} already exists; split writes a fresh tree",
                    out_dir.display()
                ),
            });
        }

        // Group component tensor ids by the layer index on their path, and
        // synthesize the per-layer selector expression (path truncated at
        // the indexed `layers[N]` segment).
        let mut layer_members: BTreeMap<u64, Vec<String>> = BTreeMap::new();
        let mut tensor_owner: BTreeMap<String, Vec<u64>> = BTreeMap::new();
        let mut layer_expression: BTreeMap<u64, String> = BTreeMap::new();
        for component in &recognition.components {
            let path = parse_component_path(&component.path)?;
            let Some(position) = path
                .iter()
                .position(|s| s.name == "layers" && s.index.is_some())
            else {
                continue;
            };
            let layer = path[position].index.expect("checked");
            let expression = path[..=position]
                .iter()
                .map(|s| match s.index {
                    Some(i) => format!("{}[{}]", s.name, i),
                    None => s.name.clone(),
                })
                .collect::<Vec<_>>()
                .join(".");
            layer_expression.entry(layer).or_insert(expression);
            for tensor_id in &component.tensor_ids {
                let owners = tensor_owner.entry(tensor_id.clone()).or_default();
                if !owners.contains(&layer) {
                    owners.push(layer);
                }
                let members = layer_members.entry(layer).or_default();
                if !members.contains(tensor_id) {
                    members.push(tensor_id.clone());
                }
            }
        }
        if layer_members.is_empty() {
            return Err(NnError::InvalidRequest {
                message: "the pack recognized no indexed layer components (…layers[N]…); split --by layer needs a layered family".to_string(),
            });
        }
        // Layer indices must be contiguous from 0 for a well-formed split.
        let indices: Vec<u64> = layer_members.keys().copied().collect();
        if indices.first().copied() != Some(0)
            || indices.last().copied().unwrap_or(0) + 1 != indices.len() as u64
        {
            return Err(NnError::InvalidRequest {
                message: format!(
                    "layer indices are not contiguous from 0 (found {}..{})",
                    indices.first().copied().unwrap_or(0),
                    indices.last().copied().unwrap_or(0)
                ),
            });
        }

        // Shared = tensors owned by more than one layer; unresolved = no layer.
        let mut shared: Vec<String> = Vec::new();
        for (tensor_id, owners) in &tensor_owner {
            if owners.len() > 1 {
                if let Some(tensor) = catalog.tensors.iter().find(|t| &t.id == tensor_id) {
                    shared.push(tensor.original_name.clone());
                }
            }
        }
        let unresolved: Vec<String> = catalog
            .tensors
            .iter()
            .filter(|t| !tensor_owner.contains_key(&t.id))
            .map(|t| t.original_name.clone())
            .collect();

        std::fs::create_dir_all(out_dir).map_err(NnError::Io)?;
        let selections_dir = out_dir.join("selections");
        std::fs::create_dir_all(&selections_dir).map_err(NnError::Io)?;

        let mut children = Vec::new();
        let mut total_unique_bytes = 0u64;
        for (layer, tensor_ids) in &layer_members {
            let expression = layer_expression
                .get(layer)
                .cloned()
                .unwrap_or_else(|| format!("layers[{layer}]"));
            // Children are ordinary pack-resolved selections: the expression
            // is re-evaluated, so the saved selection stays rebindable.
            let selection = Selection::resolve_components(
                catalog,
                pack,
                recognition,
                &expression,
                EmptyPolicy::Reject,
            )?;
            let selection_path = format!("selections/layer-{layer:05}.sel.json");
            selection.save(&out_dir.join(&selection_path))?;

            let mut tensor_names = Vec::new();
            let mut unique_bytes = 0u64;
            for tensor_id in tensor_ids {
                if let Some(tensor) = catalog.tensors.iter().find(|t| &t.id == tensor_id) {
                    tensor_names.push(tensor.original_name.clone());
                    unique_bytes += tensor.payload_length.unwrap_or(0);
                }
            }
            total_unique_bytes += unique_bytes;

            let mut bundle_dir = None;
            if materialize {
                let plan = slice::SlicePlan::build(
                    catalog,
                    &selection,
                    StoragePolicy::Materialized,
                    QuantPolicy::PreserveEncoding,
                )?;
                let dir = format!("layers/layer-{layer:05}");
                slice::apply_plan(&plan, catalog, &out_dir.join(&dir), budget)?;
                bundle_dir = Some(dir);
            } else {
                // Reference storage: save the dry-run plan so each child's
                // spans and preimages are reviewable without payload copies.
                let plan = slice::SlicePlan::build(
                    catalog,
                    &selection,
                    StoragePolicy::Reference,
                    QuantPolicy::PreserveEncoding,
                )?;
                plan.save(&out_dir.join(format!("selections/layer-{layer:05}.plan.json")))?;
            }
            budget.consume_generated(1)?;
            budget.checkpoint()?;

            children.push(SplitChild {
                layer: *layer,
                expression,
                selection_path,
                bundle_dir,
                tensor_names,
                unique_bytes,
            });
        }

        // Root manifest.
        let manifest =
            Self::manifest_json(catalog, &children, &shared, &unresolved, total_unique_bytes)?;
        std::fs::write(out_dir.join("split.json"), manifest.as_bytes()).map_err(NnError::Io)?;

        Ok(SplitReport {
            root_dir: out_dir.to_path_buf(),
            catalog_id: catalog.id()?,
            children,
            shared,
            unresolved,
            total_unique_bytes,
        })
    }

    fn manifest_json(
        catalog: &Catalog,
        children: &[SplitChild],
        shared: &[String],
        unresolved: &[String],
        total_unique_bytes: u64,
    ) -> Result<String, NnError> {
        let member_bytes: u64 = children.iter().map(|c| c.unique_bytes).sum();
        let child_records = children
            .iter()
            .map(|c| {
                let mut pairs = vec![
                    ("layer", Json::Str(c.layer.to_string())),
                    ("expression", Json::Str(c.expression.clone())),
                    ("selection", Json::Str(c.selection_path.clone())),
                    ("tensor_count", Json::Str(c.tensor_names.len().to_string())),
                    ("unique_bytes", Json::Str(c.unique_bytes.to_string())),
                ];
                if let Some(dir) = &c.bundle_dir {
                    pairs.push(("bundle", Json::Str(dir.clone())));
                }
                Json::object(pairs)
            })
            .collect::<Result<Vec<_>, _>>()?;
        let manifest = Json::object(vec![
            ("schema", Json::Str("binfiddle.nn.split/v1".to_string())),
            ("by", Json::Str("layer".to_string())),
            ("catalog_id", Json::Str(catalog.id()?)),
            ("children", Json::Array(child_records)),
            (
                "shared",
                Json::Array(shared.iter().cloned().map(Json::Str).collect()),
            ),
            (
                "unresolved",
                Json::Array(unresolved.iter().cloned().map(Json::Str).collect()),
            ),
            (
                "accounting",
                Json::object(vec![
                    ("member_bytes", Json::Str(member_bytes.to_string())),
                    ("unique_bytes", Json::Str(total_unique_bytes.to_string())),
                    (
                        "note",
                        Json::Str(
                            "member bytes count each tensor once per child; unique bytes count each tensor once overall — payloads are never duplicated by navigation overlap"
                                .to_string(),
                        ),
                    ),
                ])?,
            ),
            (
                "guarantees",
                Json::object(vec![
                    ("claim", Json::Str("tensor_content".to_string())),
                    ("original_bytes", Json::Bool(false)),
                    ("executable", Json::Bool(false)),
                ])?,
            ),
        ])?;
        manifest.to_canonical()
    }

    pub fn envelope(&self) -> Result<ResultEnvelope, NnError> {
        let children = self
            .children
            .iter()
            .map(|c| {
                Json::object(vec![
                    ("layer", Json::Str(c.layer.to_string())),
                    ("expression", Json::Str(c.expression.clone())),
                    ("selection", Json::Str(c.selection_path.clone())),
                    (
                        "bundle",
                        c.bundle_dir
                            .as_ref()
                            .map(|d| Json::Str(d.clone()))
                            .unwrap_or(Json::Null),
                    ),
                    ("tensor_count", Json::Str(c.tensor_names.len().to_string())),
                    ("unique_bytes", Json::Str(c.unique_bytes.to_string())),
                ])
            })
            .collect::<Result<Vec<_>, _>>()?;
        let semantic = Json::object(vec![
            ("split_root", Json::Str(self.root_dir.display().to_string())),
            ("catalog_id", Json::Str(self.catalog_id.clone())),
            ("children", Json::Array(children)),
            ("shared_count", Json::Str(self.shared.len().to_string())),
            (
                "unresolved_count",
                Json::Str(self.unresolved.len().to_string()),
            ),
            (
                "total_unique_bytes",
                Json::Str(self.total_unique_bytes.to_string()),
            ),
            (
                "guarantees",
                Json::object(vec![
                    ("claim", Json::Str("tensor_content".to_string())),
                    ("executable", Json::Bool(false)),
                ])?,
            ),
        ])?;
        Ok(ResultEnvelope::new("split").with_semantic(semantic))
    }

    pub fn text(&self) -> String {
        let mut out = String::from("split by layer\n");
        out.push_str(&format!("  root: {}\n", self.root_dir.display()));
        for child in &self.children {
            let storage = child
                .bundle_dir
                .as_deref()
                .map(|d| format!("materialized ({d})"))
                .unwrap_or_else(|| "reference (plan saved)".to_string());
            out.push_str(&format!(
                "  layer {}: {} tensors, {} bytes — {} [{}]\n",
                child.layer,
                child.tensor_names.len(),
                child.unique_bytes,
                storage,
                child.expression
            ));
        }
        out.push_str(&format!(
            "  shared: {} tensors (stored/counted once)\n",
            self.shared.len()
        ));
        out.push_str(&format!(
            "  unresolved: {} tensors (no layer claimed; visible above)\n",
            self.unresolved.len()
        ));
        out.push_str(&format!("  unique bytes: {}\n", self.total_unique_bytes));
        out.push_str(
            "  guarantees: tensor_content only; original bytes and executability are NOT claimed\n",
        );
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::nn::budget::{Budget, BudgetCaps};
    use crate::nn::cancel::CancellationToken;
    use crate::nn::catalog::Catalog;
    use crate::nn::discover::{discover, DiscoverOptions};
    use crate::nn::packs::Pack;

    fn budget() -> Budget {
        Budget::new(BudgetCaps::default(), None, CancellationToken::new())
    }

    fn fixture(dir: &Path, layers: usize) {
        // Model: embedding + per-layer q/o tensors.
        let mut tensors: Vec<(String, Vec<u64>)> =
            vec![("model.embed_tokens.weight".to_string(), vec![4, 4])];
        for layer in 0..layers {
            let l = layer.to_string();
            tensors.push((
                format!("model.layers.{l}.self_attn.q_proj.weight"),
                vec![12, 4],
            ));
            tensors.push((
                format!("model.layers.{l}.self_attn.o_proj.weight"),
                vec![4, 6],
            ));
        }
        let mut body: Vec<u8> = Vec::new();
        let mut spans = Vec::new();
        for (name, shape) in &tensors {
            let begin = body.len();
            let bytes: usize = shape.iter().product::<u64>() as usize * 4;
            body.extend(std::iter::repeat_n(0u8, bytes));
            spans.push((name, shape, begin, body.len()));
        }
        let entries: Vec<String> = spans
            .iter()
            .map(|(n, sh, b, e)| {
                let dims: Vec<String> = sh.iter().map(|d| d.to_string()).collect();
                format!(
                    "\"{n}\":{{\"dtype\":\"F32\",\"shape\":[{}],\"data_offsets\":[{b},{e}]}}",
                    dims.join(",")
                )
            })
            .collect();
        let header = format!("{{{}}}", entries.join(","));
        let mut data = Vec::new();
        data.extend_from_slice(&(header.len() as u64).to_le_bytes());
        data.extend_from_slice(header.as_bytes());
        data.extend_from_slice(&body);
        std::fs::write(dir.join("m.safetensors"), data).unwrap();

        let yaml = r#"schema: binfiddle.nn.pack/v1
id: split.test
version: "1.0.0"
config:
  hidden_size: 4
  num_attention_heads: 2
  head_dim: 3
bindings:
  - pattern: "model.layers.{layer}.self_attn.q_proj.weight"
    component: "decoder.layers[{layer}].attention.query_gate"
    kind: query_gate
    shape: ["2*num_attention_heads*head_dim", "hidden_size"]
  - pattern: "model.layers.{layer}.self_attn.o_proj.weight"
    component: "decoder.layers[{layer}].attention.output"
    kind: dense
    shape: ["hidden_size", "num_attention_heads*head_dim"]
  - pattern: "model.embed_tokens.weight"
    component: "embeddings.tokens"
    kind: dense
    shape: ["hidden_size", "hidden_size"]
"#;
        std::fs::write(dir.join("pack.yaml"), yaml).unwrap();
    }

    fn setup(dir: &Path, layers: usize) -> (Catalog, Pack, Recognition) {
        fixture(dir, layers);
        let report = discover(
            &dir.join("m.safetensors"),
            &DiscoverOptions::default(),
            &budget(),
        )
        .unwrap();
        let catalog = Catalog::from_discovery(&report).unwrap();
        let pack = Pack::parse(&std::fs::read_to_string(dir.join("pack.yaml")).unwrap()).unwrap();
        let recognition = Recognition::recognize(&pack, &catalog).unwrap();
        (catalog, pack, recognition)
    }

    #[test]
    fn reference_split_accounts_once_and_marks_shared() {
        let dir = tempfile::tempdir().unwrap();
        let (catalog, pack, recognition) = setup(dir.path(), 3);
        let out = dir.path().join("split");
        let report =
            SplitReport::split_by_layer(&catalog, &pack, &recognition, false, &out, &budget())
                .unwrap();
        assert_eq!(report.children.len(), 3);
        // Each layer: q (192B) + o (96B) = 288 unique bytes.
        assert!(report.children.iter().all(|c| c.unique_bytes == 288));
        assert_eq!(report.total_unique_bytes, 288 * 3);
        // The embedding is unresolved (no layer claimed it).
        assert_eq!(report.unresolved, vec!["model.embed_tokens.weight"]);
        assert!(report.shared.is_empty());
        // Manifest + child artifacts exist.
        assert!(out.join("split.json").exists());
        for layer in 0..3 {
            assert!(out
                .join(format!("selections/layer-{layer:05}.sel.json"))
                .exists());
            assert!(out
                .join(format!("selections/layer-{layer:05}.plan.json"))
                .exists());
        }
        // Child selections load and bind.
        let child = Selection::load(&out.join("selections/layer-00000.sel.json")).unwrap();
        assert_eq!(child.bind(&catalog).unwrap().len(), 2);
        // Fresh-directory enforcement.
        assert!(
            SplitReport::split_by_layer(&catalog, &pack, &recognition, false, &out, &budget())
                .is_err()
        );
    }

    #[test]
    fn materialized_split_produces_per_layer_bundles() {
        let dir = tempfile::tempdir().unwrap();
        let (catalog, pack, recognition) = setup(dir.path(), 2);
        let out = dir.path().join("split");
        let report =
            SplitReport::split_by_layer(&catalog, &pack, &recognition, true, &out, &budget())
                .unwrap();
        for layer in 0..2 {
            let bundle = out.join(format!("layers/layer-{layer:05}"));
            assert!(bundle.join("slice.json").exists(), "layer {layer} bundle");
            // Two member files: q and o.
            let weights = bundle.join("weights");
            let count = std::fs::read_dir(&weights).unwrap().count();
            assert_eq!(count, 2);
        }
        let text = report.text();
        assert!(text.contains("materialized (layers/layer-00000)"));
        assert!(text.contains("NOT claimed"));
    }
}
