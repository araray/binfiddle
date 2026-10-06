//! Static execution partition planning.
//!
//! A partition plan assigns contiguous layer groups to stages, balanced by
//! encoded weight bytes — nothing more. It is a static estimate over catalog
//! spans: activation memory, workspace, runtime state, and transfer costs
//! need runtime evidence and are explicitly NOT estimated here. Every number
//! in the output states what it is and what it excludes.

use super::catalog::Catalog;
use super::component_selection::parse_component_path;
use super::error::NnError;
use super::json::Json;
use super::packs::Recognition;
use super::report::ResultEnvelope;

/// One stage assignment.
#[derive(Debug, Clone)]
pub struct StageAssignment {
    pub stage: usize,
    pub layer_range: (u64, u64),
    pub layers: Vec<u64>,
    pub tensor_count: usize,
    /// Encoded payload bytes of the stage's layer tensors.
    pub weight_bytes: u64,
}

/// Placement policy for unlayered tensors (embeddings, norms, heads).
pub enum PlacementPolicy<'a> {
    /// Default: report them, never distribute.
    Report,
    /// Place every unlayered tensor in stage 0.
    First,
    /// Place every unlayered tensor in the last stage.
    Last,
    /// Explicit tensor-name -> stage map; every unlayered tensor must be
    /// present, and every stage index must exist.
    Manual(&'a [(String, usize)]),
}

impl PlacementPolicy<'_> {
    pub fn as_str(&self) -> &'static str {
        match self {
            PlacementPolicy::Report => "report",
            PlacementPolicy::First => "first",
            PlacementPolicy::Last => "last",
            PlacementPolicy::Manual(_) => "manual",
        }
    }
}

/// A partition plan.
pub struct PartitionPlan {
    pub stages: Vec<StageAssignment>,
    /// Tensors outside every layer family (embeddings, norms, heads) —
    /// reported, never silently distributed.
    pub unlayered_bytes: u64,
    pub unlayered_tensors: Vec<String>,
    pub total_layer_bytes: u64,
    pub total_stages: usize,
    /// Placement policy applied to unlayered tensors (report | first |
    /// last | manual); `report` (the default) leaves them unplaced.
    pub unlayered_policy: &'static str,
    /// Unlayered tensors placed per stage index (policy != report).
    pub placements: Vec<(String, usize)>,
}

impl PartitionPlan {
    /// Plan `stages` contiguous layer groups balanced by encoded weight
    /// bytes. Layers come from a pack's recognized components (their indexed
    /// `layers` family); layer tensors are matched by component membership.
    pub fn plan(
        catalog: &Catalog,
        recognition: &Recognition,
        stages: usize,
        placement: PlacementPolicy<'_>,
    ) -> Result<PartitionPlan, NnError> {
        if stages == 0 {
            return Err(NnError::InvalidRequest {
                message: "at least one stage is required".to_string(),
            });
        }

        // Map each recognized component to its layer index (the index on a
        // path segment named "layers"; components without one are unlayered).
        let mut layer_bytes: std::collections::BTreeMap<u64, u64> =
            std::collections::BTreeMap::new();
        let mut layer_tensors: std::collections::BTreeMap<u64, usize> =
            std::collections::BTreeMap::new();
        let mut assigned_ids: Vec<String> = Vec::new();
        let mut unlayered_bytes = 0u64;
        let mut unlayered_tensors: Vec<String> = Vec::new();

        for component in &recognition.components {
            let path = parse_component_path(&component.path)?;
            let layer = path.iter().find_map(|segment| {
                (segment.name == "layers")
                    .then_some(segment.index)
                    .flatten()
            });
            for tensor_id in &component.tensor_ids {
                let Some(tensor) = catalog.tensors.iter().find(|t| &t.id == tensor_id) else {
                    continue;
                };
                if assigned_ids.contains(tensor_id) {
                    continue;
                }
                assigned_ids.push(tensor_id.clone());
                let bytes = tensor.payload_length.unwrap_or(0);
                match layer {
                    Some(layer) => {
                        *layer_bytes.entry(layer).or_insert(0) += bytes;
                        *layer_tensors.entry(layer).or_insert(0) += 1;
                    }
                    None => {
                        unlayered_bytes += bytes;
                        unlayered_tensors.push(tensor.original_name.clone());
                    }
                }
            }
        }
        // Tensors no component claimed are unlayered too (visible evidence).
        for tensor in &catalog.tensors {
            if !assigned_ids.contains(&tensor.id) {
                unlayered_bytes += tensor.payload_length.unwrap_or(0);
                unlayered_tensors.push(tensor.original_name.clone());
            }
        }

        let layers: Vec<u64> = layer_bytes.keys().copied().collect();
        if layers.is_empty() {
            return Err(NnError::InvalidRequest {
                message: "the pack recognized no layered components (decoder.layers[...] families); partitioning needs layers".to_string(),
            });
        }
        // Layer indices must be contiguous from 0 for contiguous grouping.
        if layers[0] != 0 || layers.last().copied().unwrap_or(0) + 1 != layers.len() as u64 {
            return Err(NnError::InvalidRequest {
                message: format!(
                    "layer indices are not contiguous from 0 (found {}..{}); contiguous grouping requires a complete family",
                    layers.first().copied().unwrap_or(0),
                    layers.last().copied().unwrap_or(0)
                ),
            });
        }

        let total: u64 = layer_bytes.values().sum();
        let stage_count = stages.min(layers.len()).max(1);
        let assignments = balanced_contiguous(&layers, &layer_bytes, stage_count);

        let stages = assignments
            .into_iter()
            .enumerate()
            .map(|(stage, group)| {
                Some(StageAssignment {
                    stage,
                    layer_range: (*group.first()?, *group.last()?),
                    layers: group.clone(),
                    tensor_count: group
                        .iter()
                        .map(|l| layer_tensors.get(l).copied().unwrap_or(0))
                        .sum(),
                    weight_bytes: group
                        .iter()
                        .map(|l| layer_bytes.get(l).copied().unwrap_or(0))
                        .sum(),
                })
            })
            .collect::<Option<Vec<_>>>()
            .ok_or_else(|| NnError::InvalidRequest {
                message: "empty stage group".to_string(),
            })?;

        let placements = match placement {
            PlacementPolicy::Report => Vec::new(),
            PlacementPolicy::First => unlayered_tensors
                .iter()
                .map(|name| (name.clone(), 0))
                .collect(),
            PlacementPolicy::Last => unlayered_tensors
                .iter()
                .map(|name| (name.clone(), stage_count.saturating_sub(1)))
                .collect(),
            PlacementPolicy::Manual(map) => {
                for name in &unlayered_tensors {
                    if !map.iter().any(|(n, _)| n == name) {
                        return Err(NnError::InvalidRequest {
                            message: format!("manual placement misses unlayered tensor {name}"),
                        });
                    }
                }
                let mut placed = Vec::new();
                for name in &unlayered_tensors {
                    let stage = map
                        .iter()
                        .find(|(n, _)| n == name)
                        .map(|(_, s)| *s)
                        .unwrap_or(0);
                    if stage >= stage_count {
                        return Err(NnError::InvalidRequest {
                            message: format!(
                                "manual placement sends {name} to stage {stage} but the plan has {stage_count}"
                            ),
                        });
                    }
                    placed.push((name.clone(), stage));
                }
                placed
            }
        };
        Ok(PartitionPlan {
            stages,
            unlayered_bytes,
            unlayered_tensors,
            total_layer_bytes: total,
            total_stages: stage_count,
            unlayered_policy: placement.as_str(),
            placements,
        })
    }

    pub fn envelope(&self) -> Result<ResultEnvelope, NnError> {
        let stages = self
            .stages
            .iter()
            .map(|s| {
                Json::object(vec![
                    ("stage", Json::Str(s.stage.to_string())),
                    (
                        "layer_range",
                        Json::Array(vec![
                            Json::Str(s.layer_range.0.to_string()),
                            Json::Str(s.layer_range.1.to_string()),
                        ]),
                    ),
                    (
                        "layers",
                        Json::Array(s.layers.iter().map(|l| Json::Str(l.to_string())).collect()),
                    ),
                    ("tensor_count", Json::Str(s.tensor_count.to_string())),
                    ("weight_bytes", Json::Str(s.weight_bytes.to_string())),
                ])
            })
            .collect::<Result<Vec<_>, _>>()?;
        let semantic = Json::object(vec![
            ("stages", Json::Array(stages)),
            ("total_layer_bytes", Json::Str(self.total_layer_bytes.to_string())),
            ("stage_count", Json::Str(self.total_stages.to_string())),
            ("unlayered_bytes", Json::Str(self.unlayered_bytes.to_string())),
            (
                "unlayered_tensors",
                Json::Array(
                    self.unlayered_tensors
                        .iter()
                        .cloned()
                        .map(Json::Str)
                        .collect(),
                ),
            ),
            (
                "estimates",
                Json::object(vec![
                    (
                        "includes",
                        Json::Str("encoded weight bytes of layer tensors only".to_string()),
                    ),
                    (
                        "excludes",
                        Json::Str(
                            "activations, workspace, KV/recurrent state, transfer costs, and all runtime behavior — those require runtime evidence"
                                .to_string(),
                        ),
                    ),
                    (
                        "method",
                        Json::Str("contiguous layer groups balanced by encoded weight bytes".to_string()),
                    ),
                ])?,
            ),
            (
                "claims",
                Json::Str("static byte estimate; no performance or speedup claims".to_string()),
            ),
        ])?;
        Ok(ResultEnvelope::new("partition").with_semantic(semantic))
    }

    pub fn text(&self) -> String {
        let mut out = String::from("partition plan (static)\n");
        for stage in &self.stages {
            let layers = stage
                .layers
                .iter()
                .map(|l| l.to_string())
                .collect::<Vec<_>>()
                .join(",");
            out.push_str(&format!(
                "  stage {}: layers [{}] ({} tensors, {} weight bytes)\n",
                stage.stage, layers, stage.tensor_count, stage.weight_bytes
            ));
        }
        out.push_str(&format!(
            "  total layer bytes: {} across {} stages\n",
            self.total_layer_bytes, self.total_stages
        ));
        if !self.unlayered_tensors.is_empty() {
            out.push_str(&format!(
                "  unlayered: {} tensors, {} bytes (embeddings/norms/heads; never silently distributed)\n",
                self.unlayered_tensors.len(),
                self.unlayered_bytes
            ));
        }
        out.push_str(
            "  estimates include encoded weight bytes of layer tensors only; activations, workspace, state, and transfers are excluded (runtime evidence required)\n",
        );
        out.push_str("  claims: static byte estimate; no performance or speedup claims\n");
        out
    }
}

/// Optimal contiguous partition minimizing the maximum stage byte total
/// (classic DP; layers and stages are small). Layers are never split.
fn balanced_contiguous(
    layers: &[u64],
    bytes: &std::collections::BTreeMap<u64, u64>,
    stages: usize,
) -> Vec<Vec<u64>> {
    if layers.is_empty() || stages == 0 {
        return Vec::new();
    }
    let n = layers.len();
    let k = stages.min(n);
    let weights: Vec<u64> = layers
        .iter()
        .map(|l| bytes.get(l).copied().unwrap_or(0))
        .collect();
    let mut prefix = vec![0u64; n + 1];
    for i in 0..n {
        prefix[i + 1] = prefix[i] + weights[i];
    }
    // dp[j][i] = minimal possible maximum stage total splitting the first i
    // layers into j stages.
    let inf = u64::MAX / 4;
    let mut dp = vec![vec![inf; n + 1]; k + 1];
    let mut cut = vec![vec![0usize; n + 1]; k + 1];
    dp[0][0] = 0;
    for j in 1..=k {
        for i in j..=n {
            for m in (j - 1)..i {
                if dp[j - 1][m] == inf {
                    continue;
                }
                let stage_bytes = prefix[i] - prefix[m];
                let candidate = dp[j - 1][m].max(stage_bytes);
                if candidate < dp[j][i] {
                    dp[j][i] = candidate;
                    cut[j][i] = m;
                }
            }
        }
    }
    // Reconstruct groups from the cut table.
    let mut groups = Vec::with_capacity(k);
    let mut i = n;
    let mut j = k;
    while j > 0 {
        let m = cut[j][i];
        groups.push(layers[m..i].to_vec());
        i = m;
        j -= 1;
    }
    groups.reverse();
    groups
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    #[test]
    fn balanced_contiguous_keeps_layers_whole() {
        let mut bytes = BTreeMap::new();
        for (layer, b) in [(0u64, 100u64), (1, 100), (2, 100), (3, 100)] {
            bytes.insert(layer, b);
        }
        let layers: Vec<u64> = bytes.keys().copied().collect();
        let groups = balanced_contiguous(&layers, &bytes, 2);
        assert_eq!(groups.len(), 2);
        assert_eq!(groups[0], vec![0, 1]);
        assert_eq!(groups[1], vec![2, 3]);
        // Every layer exactly once.
        let flat: Vec<u64> = groups.concat();
        assert_eq!(flat, layers);
    }

    #[test]
    fn balanced_contiguous_handles_uneven_layers() {
        let mut bytes = BTreeMap::new();
        for (layer, b) in [(0u64, 10u64), (1, 10), (2, 400)] {
            bytes.insert(layer, b);
        }
        let layers: Vec<u64> = bytes.keys().copied().collect();
        let groups = balanced_contiguous(&layers, &bytes, 2);
        assert_eq!(groups.len(), 2);
        // The big layer must be its own trailing group.
        assert_eq!(*groups[1].last().unwrap(), 2);
        let flat: Vec<u64> = groups.concat();
        assert_eq!(flat.len(), 3);
    }

    #[test]
    fn more_stages_than_layers_clamps() {
        let mut bytes = BTreeMap::new();
        bytes.insert(0u64, 5u64);
        let layers: Vec<u64> = vec![0];
        let groups = balanced_contiguous(&layers, &bytes, 4);
        assert_eq!(groups.len(), 1);
    }
}
