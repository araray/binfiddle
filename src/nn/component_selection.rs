//! Component selection: resolving the selector grammar against a pack's
//! recognized components.
//!
//! A selector matches component paths segment by segment. A segment without
//! an indexer matches any path segment of the same name; an indexer requires
//! an indexed path segment whose index it selects (single/range/list from
//! the family extent, or `*`). A selector shorter than a path selects the
//! whole subtree below it. `heads[N]` on a `query_gate` component is a
//! virtual family selecting the B.4 row range of head N.

use super::catalog::{Catalog, CatalogTensor};
use super::codec::{layout_for_encoding, TensorLayout};
use super::error::NnError;
use super::packs::{BindingKind, Pack, Recognition};
use super::selector::{IndexSpec, Selector};
use std::collections::BTreeMap;

/// One path segment of a component path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PathSegment {
    pub name: String,
    pub index: Option<u64>,
}

/// Parse `decoder.layers[3].attention.query_gate` into segments.
pub fn parse_component_path(path: &str) -> Result<Vec<PathSegment>, NnError> {
    let selector = Selector::parse(path)?;
    let mut segments = Vec::with_capacity(selector.segments.len());
    for segment in selector.segments {
        let index = match segment.index {
            None => None,
            Some(IndexSpec::Single(i)) => Some(i),
            Some(_) => {
                return Err(NnError::InvalidRequest {
                    message: format!(
                        "component path {} carries a range or wildcard; paths name exactly one component",
                        super::error::brief(path)
                    ),
                })
            }
        };
        segments.push(PathSegment {
            name: segment.name,
            index,
        });
    }
    Ok(segments)
}

/// A resolved selection target with an optional view annotation.
#[derive(Debug, Clone, PartialEq)]
pub struct ComponentTarget {
    pub component_path: String,
    pub kind: BindingKind,
    pub tensor_id: String,
    pub tensor_name: String,
    /// Virtual view over the tensor (e.g. the head row range of a fused
    /// query/gate weight).
    pub view: Option<ComponentView>,
}

/// A view annotation produced by a virtual family segment.
#[derive(Debug, Clone, PartialEq)]
pub struct ComponentView {
    /// Human statement of what the bytes represent.
    pub logical: String,
    /// Byte range within the tensor payload.
    pub span: (u64, u64),
    /// Row range within the stored weight.
    pub rows: (u64, u64),
}

/// Resolve a selector expression against a recognition result and its
/// catalog (the catalog supplies tensor encodings for view spans).
pub fn resolve_selector(
    selector: &Selector,
    recognition: &Recognition,
    pack: &Pack,
    catalog: &Catalog,
) -> Result<Vec<ComponentTarget>, NnError> {
    if recognition.components.is_empty() {
        return Err(NnError::InvalidRequest {
            message: format!(
                "pack {} recognized no components in this catalog",
                recognition.pack_name
            ),
        });
    }

    // Split a virtual `heads[N]` tail.
    let mut head_tail: Option<u64> = None;
    let mut effective = selector.clone();
    if let Some(last) = selector.segments.last() {
        if last.name == "heads" {
            match last.index {
                Some(IndexSpec::Single(h)) => {
                    head_tail = Some(h);
                    effective.segments.pop();
                }
                Some(IndexSpec::All) | None => {
                    return Err(NnError::InvalidRequest {
                        message: "heads requires one explicit index (heads[1]); the component itself already selects every head"
                            .to_string(),
                    })
                }
                Some(_) => {
                    return Err(NnError::InvalidRequest {
                        message: "heads accepts a single index; ranges and lists are not supported"
                            .to_string(),
                    })
                }
            }
        }
    }

    let parsed: Vec<(&super::packs::RecognizedComponent, Vec<PathSegment>)> = recognition
        .components
        .iter()
        .map(|c| Ok((c, parse_component_path(&c.path)?)))
        .collect::<Result<Vec<_>, NnError>>()?;

    // Family extents: for every (position, name), how many distinct indexed
    // siblings exist (max index + 1).
    let mut extents: BTreeMap<(usize, String), u64> = BTreeMap::new();
    for (_, path) in &parsed {
        for (position, segment) in path.iter().enumerate() {
            if let Some(index) = segment.index {
                let key = (position, segment.name.clone());
                let entry = extents.entry(key).or_insert(0);
                *entry = (*entry).max(index + 1);
            }
        }
    }

    let mut targets: Vec<ComponentTarget> = Vec::new();
    let mut matched_any = false;
    for (component, path) in &parsed {
        let Some(_) = match_selector(&effective, path, &extents)? else {
            continue;
        };
        matched_any = true;

        let tensor = catalog
            .tensors
            .iter()
            .find(|t| component.tensor_ids.contains(&t.id))
            .ok_or_else(|| NnError::SourceMissing {
                detail: format!(
                    "component {} references a tensor missing from the catalog",
                    component.path
                ),
            })?;

        let view = match head_tail {
            None => None,
            Some(head) => Some(head_view(component, pack, tensor, head)?),
        };
        for (i, tensor_id) in component.tensor_ids.iter().enumerate() {
            targets.push(ComponentTarget {
                component_path: component.path.clone(),
                kind: component.kind,
                tensor_id: tensor_id.clone(),
                tensor_name: component.tensor_names.get(i).cloned().unwrap_or_default(),
                view: view.clone(),
            });
        }
    }
    if !matched_any {
        return Err(NnError::InvalidRequest {
            message: format!(
                "selector '{}' matched no components under pack {}",
                selector, recognition.pack_name
            ),
        });
    }
    Ok(targets)
}

enum ViewRequest {
    None,
}

/// Returns `Some(ViewRequest)` when the selector matches the path (as a
/// prefix or in full), `None` when it does not.
fn match_selector(
    selector: &Selector,
    path: &[PathSegment],
    extents: &BTreeMap<(usize, String), u64>,
) -> Result<Option<ViewRequest>, NnError> {
    if selector.segments.len() > path.len() {
        return Ok(None);
    }
    for (position, segment) in selector.segments.iter().enumerate() {
        let path_segment = &path[position];
        if segment.name != path_segment.name {
            return Ok(None);
        }
        let Some(spec) = &segment.index else {
            continue;
        };
        let Some(index) = path_segment.index else {
            return Ok(None);
        };
        // Extent for this family; unindexed-only families have extent 1 for
        // the purposes of index validation (any index would be out of range
        // and cannot match anyway).
        let extent = extents
            .get(&(position, segment.name.clone()))
            .copied()
            .unwrap_or(1);
        let selected = match spec {
            IndexSpec::All => true,
            IndexSpec::Single(i) => {
                if *i >= extent {
                    return Err(NnError::InvalidRequest {
                        message: format!(
                            "index {i} on '{}' is out of range (family extent {extent})",
                            segment.name
                        ),
                    });
                }
                index == *i
            }
            IndexSpec::Range(a, b) => {
                if *a >= extent {
                    return Err(NnError::InvalidRequest {
                        message: format!(
                            "range start {a} on '{}' exceeds the family extent {extent}",
                            segment.name
                        ),
                    });
                }
                index >= *a && index < *b
            }
            IndexSpec::List(items) => {
                for item in items {
                    if *item >= extent {
                        return Err(NnError::InvalidRequest {
                            message: format!(
                                "list index {item} on '{}' is out of range (family extent {extent})",
                                segment.name
                            ),
                        });
                    }
                }
                items.contains(&index)
            }
        };
        if !selected {
            return Ok(None);
        }
    }
    Ok(Some(ViewRequest::None))
}

/// The B.4 head view: contiguous stored rows [2Dh, 2Dh+2D) of the fused
/// `[2HD, d]` weight.
fn head_view(
    component: &super::packs::RecognizedComponent,
    pack: &Pack,
    tensor: &CatalogTensor,
    head: u64,
) -> Result<ComponentView, NnError> {
    if component.kind != BindingKind::QueryGate {
        return Err(NnError::InvalidRequest {
            message: format!(
                "heads[...] applies to query_gate components; {} is {}",
                component.path,
                component.kind.as_str()
            ),
        });
    }
    let heads = pack.config.get("num_attention_heads").copied().unwrap_or(0);
    if head >= heads {
        return Err(NnError::InvalidRequest {
            message: format!("head index {head} is out of range (num_attention_heads {heads})"),
        });
    }
    let head_dim = pack.config.get("head_dim").copied().unwrap_or(0);
    let hidden = pack.config.get("hidden_size").copied().unwrap_or(0);
    let layout = layout_for_encoding(&tensor.encoding);
    let TensorLayout::Scalar(codec) = layout else {
        return Err(NnError::CodecUnsupported {
            codec: tensor.encoding.clone(),
            operation: "head row-range view".to_string(),
            reason: "head views require a scalar-encoded weight".to_string(),
        });
    };
    let width = codec.width();
    let ((qs, qe), (gs, ge)) = super::packs::query_gate_head_rows(head, head_dim);
    let row_bytes = hidden
        .checked_mul(width)
        .ok_or_else(|| NnError::MalformedInput {
            detail: "row byte size overflows u64".to_string(),
        })?;
    let span = (
        qs.checked_mul(row_bytes).ok_or_else(overflow)?,
        (ge - qs).checked_mul(row_bytes).ok_or_else(overflow)?,
    );
    Ok(ComponentView {
        logical: format!(
            "head {head}: query rows [{qs}, {qe}) and gate rows [{gs}, {ge}) of the fused query/gate weight (stored rows [{qs}, {ge}))"
        ),
        span,
        rows: (qs, ge),
    })
}

fn overflow() -> NnError {
    NnError::MalformedInput {
        detail: "view span arithmetic overflow".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn component_paths_parse() {
        let segments = parse_component_path("decoder.layers[3].attention.query_gate").unwrap();
        assert_eq!(segments.len(), 4);
        assert_eq!(segments[0].name, "decoder");
        assert_eq!(segments[0].index, None);
        assert_eq!(segments[1].name, "layers");
        assert_eq!(segments[1].index, Some(3));
        assert_eq!(segments[3].name, "query_gate");
        assert!(parse_component_path("decoder.layers[3:5]").is_err());
        assert!(parse_component_path("decoder.layers[*]").is_err());
    }
}
