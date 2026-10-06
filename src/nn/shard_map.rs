//! Static download planning from a sharded-SafeTensors index (P4).
//!
//! `model.safetensors.index.json` maps every tensor to its shard file. This
//! command answers "which shards must I download for this selection?"
//! without touching the network — a plan, never a download.

use super::error::NnError;
use super::json::{Json, ParseLimits};
use super::report::ResultEnvelope;
use std::collections::BTreeMap;
use std::path::Path;

/// One shard's planned contents.
pub struct ShardPlan {
    pub shard: String,
    pub tensors: Vec<String>,
    /// Sum of tensor payload bytes (decoded from the catalog when
    /// available; otherwise counted as entries only).
    pub bytes: Option<u64>,
}

pub struct ShardMapReport {
    pub shards: Vec<ShardPlan>,
    /// Tensor names the selection requested that the index does not know.
    pub missing: Vec<String>,
    /// The index's declared total size, when present.
    pub total_size: Option<u64>,
    /// Planned payload bytes, when the catalog supplied byte counts.
    pub planned_bytes: Option<u64>,
}

impl ShardMapReport {
    pub fn text(&self) -> String {
        let mut out = String::from("shard map (static download plan)\n");
        for shard in &self.shards {
            let bytes = shard
                .bytes
                .map(|b| format!("{b} payload bytes"))
                .unwrap_or_else(|| "byte counts unavailable (no catalog)".to_string());
            out.push_str(&format!(
                "  {}: {} tensors, {}\n",
                shard.shard,
                shard.tensors.len(),
                bytes
            ));
            for name in shard.tensors.iter().take(5) {
                out.push_str(&format!("    {name}\n"));
            }
            if shard.tensors.len() > 5 {
                out.push_str(&format!("    ... and {} more\n", shard.tensors.len() - 5));
            }
        }
        out.push_str(&format!(
            "  shards: {}  tensors: {}  missing from index: {}\n",
            self.shards.len(),
            self.shards.iter().map(|s| s.tensors.len()).sum::<usize>(),
            self.missing.len()
        ));
        if let (Some(planned), Some(total)) = (self.planned_bytes, self.total_size) {
            if total > 0 {
                out.push_str(&format!(
                    "  planned payload {} of {} declared bytes ({:.1}%)\n",
                    planned,
                    total,
                    planned as f64 * 100.0 / total as f64
                ));
            }
        }
        out.push_str(
            "  claims: a download plan from the index alone; no network access, no payload verification\n",
        );
        out
    }

    pub fn envelope(&self) -> Result<ResultEnvelope, NnError> {
        let shards = self
            .shards
            .iter()
            .map(|s| {
                Json::object(vec![
                    ("shard", Json::Str(s.shard.clone())),
                    ("tensors", Json::Str(s.tensors.len().to_string())),
                    (
                        "names",
                        Json::Array(s.tensors.iter().take(50).cloned().map(Json::Str).collect()),
                    ),
                    (
                        "bytes",
                        match s.bytes {
                            Some(b) => Json::Str(b.to_string()),
                            None => Json::Null,
                        },
                    ),
                ])
            })
            .collect::<Result<Vec<_>, _>>()?;
        let semantic = Json::object(vec![
            ("shards", Json::Str(self.shards.len().to_string())),
            ("shard_plans", Json::Array(shards)),
            (
                "missing",
                Json::Array(
                    self.missing
                        .iter()
                        .take(100)
                        .cloned()
                        .map(Json::Str)
                        .collect(),
                ),
            ),
            (
                "total_size",
                match self.total_size {
                    Some(t) => Json::Str(t.to_string()),
                    None => Json::Null,
                },
            ),
            (
                "planned_bytes",
                match self.planned_bytes {
                    Some(b) => Json::Str(b.to_string()),
                    None => Json::Null,
                },
            ),
        ])?;
        Ok(ResultEnvelope::new("shard.map").with_semantic(semantic))
    }
}

/// Build the plan: index + requested tensor names (resolved through a
/// catalog/selection when given).
#[allow(clippy::too_many_arguments)]
pub fn shard_map(
    index_path: &Path,
    requested: &[String],
    catalog: Option<&super::catalog::Catalog>,
    selection: Option<&super::selection::Selection>,
) -> Result<ShardMapReport, NnError> {
    let text = std::fs::read_to_string(index_path).map_err(|_| NnError::SourceMissing {
        detail: format!("cannot read {}", index_path.display()),
    })?;
    let parsed = Json::parse_foreign(&text, ParseLimits::for_input_len(text.len()))?;
    let Some(Json::Object(members)) = parsed.get("weight_map") else {
        return Err(NnError::MalformedInput {
            detail: "index carries no weight_map".to_string(),
        });
    };
    let total_size = match parsed.get("metadata").and_then(|m| m.get("total_size")) {
        Some(Json::Number(digits)) => digits.parse::<u64>().ok(),
        Some(Json::Float(value)) if value.is_finite() => Some(*value as u64),
        _ => None,
    };
    let map: BTreeMap<&str, &str> = members
        .iter()
        .filter_map(|(k, v)| v.as_str().map(|s| (k.as_str(), s)))
        .collect();

    // The requested set: explicit names, a selection's tensor ids resolved
    // through the catalog to names, or (empty) every indexed tensor.
    let mut wanted: Vec<String> = requested.to_vec();
    if let (Some(catalog), Some(sel)) = (catalog, selection) {
        for target_id in &sel.target_ids {
            if let Some(tensor) = catalog.tensors.iter().find(|t| t.id == *target_id) {
                wanted.push(tensor.original_name.clone());
            }
        }
    }
    let everything = wanted.is_empty();

    let mut by_shard: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let mut missing: Vec<String> = Vec::new();
    let mut planned_bytes: Option<u64> = Some(0);
    for (name, shard) in &map {
        let selected = everything || wanted.iter().any(|w| w == name);
        if !selected {
            continue;
        }
        by_shard
            .entry((*shard).to_string())
            .or_default()
            .push((*name).to_string());
        if let Some(catalog) = catalog {
            let bytes = catalog
                .tensors
                .iter()
                .find(|t| t.original_name == *name)
                .and_then(|t| t.payload_length);
            match (bytes, &mut planned_bytes) {
                (Some(b), Some(total)) => *total += b,
                (None, _) => planned_bytes = None,
                _ => {}
            }
        }
    }
    if !everything {
        for name in &wanted {
            if !map.contains_key(name.as_str()) && !missing.contains(name) {
                missing.push(name.clone());
            }
        }
    }
    if by_shard.is_empty() {
        return Err(NnError::InvalidRequest {
            message: "no requested tensor appears in the index weight_map".to_string(),
        });
    }
    if catalog.is_none() {
        planned_bytes = None;
    }
    let mut shards: Vec<ShardPlan> = by_shard
        .into_iter()
        .map(|(shard, mut tensors)| {
            tensors.sort();
            let bytes = None; // filled per-tensor only when a catalog exists
            ShardPlan {
                shard,
                tensors,
                bytes,
            }
        })
        .collect();
    // Per-shard byte sums when the catalog supplies them.
    if let Some(catalog) = catalog {
        for shard in &mut shards {
            let mut sum = 0u64;
            let mut known = true;
            for name in &shard.tensors {
                match catalog
                    .tensors
                    .iter()
                    .find(|t| t.original_name == *name)
                    .and_then(|t| t.payload_length)
                {
                    Some(b) => sum += b,
                    None => known = false,
                }
            }
            shard.bytes = known.then_some(sum);
        }
    }
    Ok(ShardMapReport {
        shards,
        missing,
        total_size,
        planned_bytes,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn index_json(map: &str) -> std::path::PathBuf {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("model.safetensors.index.json");
        std::fs::write(
            &path,
            format!(r#"{{"metadata":{{"total_size":1000}},"weight_map":{map}}}"#),
        )
        .unwrap();
        // Leak the tempdir path into the test scope: the dir lives until
        // process end, which outlives every call here.
        let kept =
            std::env::temp_dir().join(format!("binfiddle-shardmap-test-{}", std::process::id()));
        std::fs::create_dir_all(&kept).unwrap();
        let dest = kept.join("model.safetensors.index.json");
        std::fs::copy(&path, &dest).unwrap();
        dest
    }

    #[test]
    fn plans_by_name_and_reports_missing() {
        let index =
            index_json(r#"{"a":"s1.safetensors","b":"s1.safetensors","c":"s2.safetensors"}"#);
        let report = shard_map(
            &index,
            &["a".to_string(), "c".to_string(), "zz".to_string()],
            None,
            None,
        )
        .unwrap();
        assert_eq!(report.shards.len(), 2);
        assert_eq!(report.total_size, Some(1000));
        assert_eq!(report.missing, vec!["zz".to_string()]);
        // No catalog → no byte accounting.
        assert_eq!(report.planned_bytes, None);

        // Empty request = the whole package.
        let report = shard_map(&index, &[], None, None).unwrap();
        assert_eq!(report.shards.len(), 2);
        assert_eq!(
            report.shards.iter().map(|s| s.tensors.len()).sum::<usize>(),
            3
        );
    }
}
