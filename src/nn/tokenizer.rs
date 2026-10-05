//! Tokenizer asset inspection and comparison.
//!
//! Tokenizer assets are part of a package's input contract. This module
//! classifies the standard asset files in a package, inspects the
//! HuggingFace-style `tokenizer.json` structure (model type, vocabulary
//! size, merges, added tokens), and compares two tokenizer files at the
//! vocabulary and added-token level — reporting what changed without
//! claiming anything about behavior. Restricted rendering of chat templates
//! is deliberately NOT attempted here: template engines are execution
//! surfaces, and unsupported features are reported rather than run.

use super::error::NnError;
use super::json::{Json, ParseLimits};
use super::report::ResultEnvelope;
use std::path::Path;

/// One asset file classification.
#[derive(Debug, Clone)]
pub struct TokenizerAsset {
    pub path: String,
    pub bytes: u64,
    pub classification: &'static str,
}

/// Classify the standard tokenizer asset files present in a directory.
pub fn classify_assets(dir: &Path) -> Result<Vec<TokenizerAsset>, NnError> {
    let entries = std::fs::read_dir(dir).map_err(|_| NnError::SourceMissing {
        detail: format!("cannot read directory {}", dir.display()),
    })?;
    let mut assets = Vec::new();
    for entry in entries {
        let entry = entry.map_err(NnError::Io)?;
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        let classification = match name.as_str() {
            "tokenizer.json" => "tokenizer_json",
            "tokenizer_config.json" => "tokenizer_config",
            "vocab.json" => "vocab_json",
            "merges.txt" => "merges",
            "special_tokens_map.json" => "special_tokens_map",
            "tokenizer.model" => "sentencepiece_model",
            _ => continue,
        };
        let bytes = entry.metadata().map_err(NnError::Io)?.len();
        assets.push(TokenizerAsset {
            path: name,
            bytes,
            classification,
        });
    }
    assets.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(assets)
}

/// Structural summary of one `tokenizer.json`.
#[derive(Debug, Clone, PartialEq)]
pub struct TokenizerSummary {
    pub model_type: String,
    pub vocab_size: u64,
    pub merge_count: Option<u64>,
    pub added_tokens: u64,
    pub unk_token: Option<String>,
}

/// Inspect a `tokenizer.json` file.
pub fn inspect_tokenizer_json(path: &Path) -> Result<TokenizerSummary, NnError> {
    let text = std::fs::read_to_string(path).map_err(|_| NnError::SourceMissing {
        detail: format!("cannot read {}", path.display()),
    })?;
    let root = Json::parse_foreign(&text, ParseLimits::for_input_len(text.len()))?;
    let model = root.get("model").cloned().unwrap_or(Json::Null);
    let model_type = model
        .get("type")
        .and_then(Json::as_str)
        .unwrap_or("unknown")
        .to_string();
    let vocab_size = match model.get("vocab") {
        Some(Json::Object(members)) => members.len() as u64,
        Some(Json::Array(items)) => items.len() as u64,
        _ => 0,
    };
    let merge_count = model
        .get("merges")
        .and_then(Json::as_array)
        .map(|merges| merges.len() as u64);
    let added_tokens = root
        .get("added_tokens")
        .and_then(Json::as_array)
        .map(|tokens| tokens.len() as u64)
        .unwrap_or(0);
    let unk_token = model
        .get("unk_token")
        .and_then(Json::as_str)
        .map(str::to_string);
    Ok(TokenizerSummary {
        model_type,
        vocab_size,
        merge_count,
        added_tokens,
        unk_token,
    })
}

/// Vocabulary-level difference between two `tokenizer.json` files.
#[derive(Debug, Clone, PartialEq)]
pub struct TokenizerDiff {
    pub left_vocab_size: u64,
    pub right_vocab_size: u64,
    pub left_added_tokens: u64,
    pub right_added_tokens: u64,
    /// Token strings present only on the left (bounded sample in text).
    pub removed: Vec<String>,
    /// Token strings present only on the right (bounded sample in text).
    pub added: Vec<String>,
    pub model_types: (String, String),
    pub same_vocab: bool,
}

/// Compare two tokenizer.json files at the vocabulary and added-token level.
pub fn diff_tokenizer_json(left_path: &Path, right_path: &Path) -> Result<TokenizerDiff, NnError> {
    let load = |path: &Path| -> Result<(Json, TokenizerSummary), NnError> {
        let text = std::fs::read_to_string(path).map_err(|_| NnError::SourceMissing {
            detail: format!("cannot read {}", path.display()),
        })?;
        let root = Json::parse_foreign(&text, ParseLimits::for_input_len(text.len()))?;
        let summary = inspect_tokenizer_json(path)?;
        Ok((root, summary))
    };
    let (left_root, left) = load(left_path)?;
    let (right_root, right) = load(right_path)?;

    let vocab_keys = |root: &Json| -> Vec<String> {
        let model = root.get("model").cloned().unwrap_or(Json::Null);
        match model.get("vocab") {
            Some(Json::Object(members)) => members.iter().map(|(k, _)| k.clone()).collect(),
            _ => Vec::new(),
        }
    };
    let left_keys = vocab_keys(&left_root);
    let right_keys = vocab_keys(&right_root);
    let removed: Vec<String> = left_keys
        .iter()
        .filter(|k| !right_keys.contains(k))
        .cloned()
        .collect();
    let added: Vec<String> = right_keys
        .iter()
        .filter(|k| !left_keys.contains(k))
        .cloned()
        .collect();

    Ok(TokenizerDiff {
        left_vocab_size: left.vocab_size,
        right_vocab_size: right.vocab_size,
        left_added_tokens: left.added_tokens,
        right_added_tokens: right.added_tokens,
        same_vocab: left.vocab_size == right.vocab_size && removed.is_empty() && added.is_empty(),
        removed,
        added,
        model_types: (left.model_type.clone(), right.model_type.clone()),
    })
}

// ---- rendering ----

pub fn inspect_envelope(
    dir: &Path,
    assets: &[TokenizerAsset],
    summary: Option<&TokenizerSummary>,
) -> Result<ResultEnvelope, NnError> {
    let asset_records = assets
        .iter()
        .map(|a| {
            Json::object(vec![
                ("path", Json::Str(a.path.clone())),
                ("bytes", Json::Str(a.bytes.to_string())),
                ("classification", Json::Str(a.classification.to_string())),
            ])
        })
        .collect::<Result<Vec<_>, _>>()?;
    let summary_json = match summary {
        Some(s) => Json::object(vec![
            ("model_type", Json::Str(s.model_type.clone())),
            ("vocab_size", Json::Str(s.vocab_size.to_string())),
            (
                "merge_count",
                match s.merge_count {
                    Some(c) => Json::Str(c.to_string()),
                    None => Json::Null,
                },
            ),
            ("added_tokens", Json::Str(s.added_tokens.to_string())),
            (
                "unk_token",
                s.unk_token.clone().map(Json::Str).unwrap_or(Json::Null),
            ),
        ])?,
        None => Json::Null,
    };
    let semantic = Json::object(vec![
        ("package", Json::Str(dir.display().to_string())),
        ("assets", Json::Array(asset_records)),
        ("tokenizer_json", summary_json),
        (
            "claims",
            Json::Str(
                "static asset and structure inspection; tokenization behavior and template rendering are NOT evaluated"
                    .to_string(),
            ),
        ),
    ])?;
    Ok(ResultEnvelope::new("tokenizer inspect").with_semantic(semantic))
}

pub fn inspect_text(assets: &[TokenizerAsset], summary: Option<&TokenizerSummary>) -> String {
    let mut out = String::from("tokenizer assets\n");
    if assets.is_empty() {
        out.push_str("  no standard tokenizer asset files found\n");
    }
    for asset in assets {
        out.push_str(&format!(
            "  {} [{}] {} bytes\n",
            asset.path, asset.classification, asset.bytes
        ));
    }
    if let Some(summary) = summary {
        out.push_str(&format!(
            "  tokenizer.json: model {}, vocab {}, merges {}, added tokens {}\n",
            summary.model_type,
            summary.vocab_size,
            summary
                .merge_count
                .map(|c| c.to_string())
                .unwrap_or_else(|| "-".into()),
            summary.added_tokens
        ));
    }
    out.push_str(
        "  claims: static asset and structure inspection; tokenization behavior and template rendering are NOT evaluated\n",
    );
    out
}

pub fn diff_envelope(diff: &TokenizerDiff) -> Result<ResultEnvelope, NnError> {
    let semantic = Json::object(vec![
        (
            "vocab_sizes",
            Json::Array(vec![
                Json::Str(diff.left_vocab_size.to_string()),
                Json::Str(diff.right_vocab_size.to_string()),
            ]),
        ),
        (
            "added_tokens",
            Json::Array(vec![
                Json::Str(diff.left_added_tokens.to_string()),
                Json::Str(diff.right_added_tokens.to_string()),
            ]),
        ),
        (
            "model_types",
            Json::Array(vec![
                Json::Str(diff.model_types.0.clone()),
                Json::Str(diff.model_types.1.clone()),
            ]),
        ),
        ("same_vocab", Json::Bool(diff.same_vocab)),
        ("removed_count", Json::Str(diff.removed.len().to_string())),
        ("added_count", Json::Str(diff.added.len().to_string())),
        (
            "claims",
            Json::Str(
                "vocabulary-level comparison; tokenization behavior differences are NOT inferred"
                    .to_string(),
            ),
        ),
    ])?;
    Ok(ResultEnvelope::new("tokenizer diff").with_semantic(semantic))
}

pub fn diff_text(diff: &TokenizerDiff) -> String {
    let mut out = String::from("tokenizer diff\n");
    out.push_str(&format!(
        "  vocab: {} vs {} ({} removed, {} added)\n",
        diff.left_vocab_size,
        diff.right_vocab_size,
        diff.removed.len(),
        diff.added.len()
    ));
    out.push_str(&format!(
        "  model: {} vs {}; added tokens {} vs {}\n",
        diff.model_types.0, diff.model_types.1, diff.left_added_tokens, diff.right_added_tokens
    ));
    for token in diff.removed.iter().take(8) {
        out.push_str(&format!("  - {token}\n"));
    }
    for token in diff.added.iter().take(8) {
        out.push_str(&format!("  + {token}\n"));
    }
    if diff.removed.len() > 8 {
        out.push_str(&format!(
            "  ... and {} more removed\n",
            diff.removed.len() - 8
        ));
    }
    if diff.added.len() > 8 {
        out.push_str(&format!("  ... and {} more added\n", diff.added.len() - 8));
    }
    out.push_str(
        "  claims: vocabulary-level comparison; tokenization behavior differences are NOT inferred\n",
    );
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tokenizer_json(vocab_pairs: &[(&str, u64)], merges: &[&str], added: u64) -> String {
        let vocab: Vec<String> = vocab_pairs
            .iter()
            .map(|(t, id)| format!("\"{t}\":{id}"))
            .collect();
        let merges: Vec<String> = merges.iter().map(|m| format!("\"{m}\"")).collect();
        let added_tokens: Vec<String> = (0..added)
            .map(|i| format!("{{\"id\":{i},\"content\":\"<extra_{i}>\",\"special\":true}}"))
            .collect();
        format!(
            "{{\"version\":\"1.0\",\"added_tokens\":[{}],\"model\":{{\"type\":\"BPE\",\"unk_token\":\"<unk>\",\"vocab\":{{{}}},\"merges\":[{}]}}}}",
            added_tokens.join(","),
            vocab.join(","),
            merges.join(",")
        )
    }

    #[test]
    fn inspects_tokenizer_json_structure() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("tokenizer.json");
        std::fs::write(
            &path,
            tokenizer_json(&[("<unk>", 0), ("a", 1), ("b", 2)], &["a b"], 2),
        )
        .unwrap();
        let summary = inspect_tokenizer_json(&path).unwrap();
        assert_eq!(summary.model_type, "BPE");
        assert_eq!(summary.vocab_size, 3);
        assert_eq!(summary.merge_count, Some(1));
        assert_eq!(summary.added_tokens, 2);
        assert_eq!(summary.unk_token.as_deref(), Some("<unk>"));
    }

    #[test]
    fn diffs_vocabularies_both_directions() {
        let dir = tempfile::tempdir().unwrap();
        let left = dir.path().join("left.json");
        let right = dir.path().join("right.json");
        std::fs::write(
            &left,
            tokenizer_json(&[("<unk>", 0), ("a", 1), ("b", 2)], &[], 0),
        )
        .unwrap();
        std::fs::write(
            &right,
            tokenizer_json(&[("<unk>", 0), ("a", 1), ("c", 2), ("d", 3)], &[], 1),
        )
        .unwrap();
        let diff = diff_tokenizer_json(&left, &right).unwrap();
        assert_eq!(diff.left_vocab_size, 3);
        assert_eq!(diff.right_vocab_size, 4);
        assert!(!diff.same_vocab);
        assert_eq!(diff.removed, vec!["b".to_string()]);
        assert_eq!(diff.added.len(), 2);
        assert!(diff.added.contains(&"c".to_string()));
        assert!(diff.added.contains(&"d".to_string()));
        let text = diff_text(&diff);
        assert!(text.contains("1 removed, 2 added"), "{text}");
        assert!(text.contains("NOT inferred"), "{text}");
    }

    #[test]
    fn identical_tokenizers_are_equal() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("t.json");
        let content = tokenizer_json(&[("a", 0), ("b", 1)], &["a b"], 1);
        std::fs::write(&path, &content).unwrap();
        let diff = diff_tokenizer_json(&path, &path).unwrap();
        assert!(diff.same_vocab);
        assert!(diff.removed.is_empty());
        assert!(diff.added.is_empty());
    }
}
