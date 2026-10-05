//! Pack authoring: lint and scaffold (Part 05 §12).
//!
//! `nn profile lint` validates a pack statically — schema, patterns,
//! expressions, multiplicities, references — without touching any model.
//! `nn profile scaffold` derives a provisional draft pack from an observed
//! catalog, with every suggestion labeled heuristic: scaffolds are starting
//! points for a human, never silently promoted to trusted profiles.

use super::catalog::Catalog;
use super::error::NnError;
use super::json::Json;
use super::packs::Pack;
use super::report::ResultEnvelope;

/// One lint finding.
#[derive(Debug, Clone)]
pub struct LintFinding {
    pub severity: &'static str, // "error" | "warning" | "info"
    pub code: &'static str,
    pub message: String,
}

/// Lint a pack: pure static checks, no model involved.
pub fn lint_pack(pack: &Pack) -> Vec<LintFinding> {
    let mut findings = Vec::new();

    // Duplicate component templates.
    let mut seen_components: Vec<&str> = Vec::new();
    for binding in &pack.bindings {
        if seen_components.contains(&binding.component.as_str()) {
            findings.push(LintFinding {
                severity: "error",
                code: "DUPLICATE_COMPONENT",
                message: format!(
                    "component template '{}' is declared by more than one binding",
                    binding.component
                ),
            });
        }
        seen_components.push(&binding.component);
    }

    // Shape arity sanity: expressions must exist and evaluate under the
    // pack's own config (unknown parameters are errors here, where the
    // config is the pack's own).
    for binding in &pack.bindings {
        if binding.shape.is_empty() {
            findings.push(LintFinding {
                severity: "warning",
                code: "EMPTY_SHAPE",
                message: format!(
                    "binding '{}' declares no shape expressions; it will match any shape",
                    binding.pattern
                ),
            });
            continue;
        }
        for axis in &binding.shape {
            match pack.eval(axis) {
                Ok(_) => {}
                Err(err) => findings.push(LintFinding {
                    severity: "error",
                    code: "SHAPE_EXPRESSION",
                    message: format!(
                        "binding '{}' axis '{}' does not evaluate: {}",
                        binding.pattern, axis, err
                    ),
                }),
            }
        }
    }

    // Layer schedule consistency.
    if let Some(flags) = &pack.layer_types_explicit {
        let declared = pack.config.get("num_layers").copied();
        if let Some(declared) = declared {
            if flags.len() as u64 != declared {
                findings.push(LintFinding {
                    severity: "error",
                    code: "LAYER_SCHEDULE_ARITY",
                    message: format!(
                        "layer_types has {} entries but config num_layers = {}",
                        flags.len(),
                        declared
                    ),
                });
            }
        }
    } else if pack.full_attention_interval.is_none() {
        let has_layers = pack
            .bindings
            .iter()
            .any(|b| b.component.contains("layers["));
        if has_layers {
            findings.push(lint_no_schedule_warning());
        }
    }

    // Catch-all safety: a binding whose pattern could match EVERY tensor
    // name (no literal content) is almost certainly a mistake.
    for binding in &pack.bindings {
        let literal: String = super::packs::compile_pattern(&binding.pattern)
            .map(|tokens| {
                tokens
                    .iter()
                    .filter_map(|t| match t {
                        super::packs::PatternToken::Literal(l) => Some(l.clone()),
                        _ => None,
                    })
                    .collect()
            })
            .unwrap_or_default();
        let literal_alpha = literal.chars().filter(|c| c.is_alphanumeric()).count();
        if literal_alpha < 2 {
            findings.push(LintFinding {
                severity: "error",
                code: "PATTERN_ALL_CAPTURE",
                message: format!(
                    "pattern '{}' has almost no literal text; it would match nearly every indexed name",
                    binding.pattern
                ),
            });
        }
    }

    if findings.is_empty() {
        findings.push(LintFinding {
            severity: "info",
            code: "LINT_CLEAN",
            message: "no static pack defects found".to_string(),
        });
    }
    findings
}

fn lint_no_schedule_warning() -> LintFinding {
    LintFinding {
        severity: "warning",
        code: "NO_LAYER_SCHEDULE",
        message: "bindings reference layers[..] but neither layer_types nor full_attention_interval is declared; schedule tools will be unavailable".to_string(),
    }
}

/// Scaffold a provisional pack from an observed catalog: group tensor names
/// by their numeric segments, propose pattern templates, and label every
/// suggestion heuristic.
pub fn scaffold_pack(catalog: &Catalog) -> Result<String, NnError> {
    // Group by the name with numeric runs replaced by {layer}.
    use std::collections::BTreeMap;
    let mut groups: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for tensor in &catalog.tensors {
        let mut templated = String::new();
        let mut chars = tensor.original_name.chars().peekable();
        while let Some(ch) = chars.next() {
            if ch.is_ascii_digit() {
                let mut run = String::new();
                run.push(ch);
                while let Some(&next) = chars.peek() {
                    if next.is_ascii_digit() {
                        run.push(next);
                        chars.next();
                    } else {
                        break;
                    }
                }
                templated.push_str("{layer}");
            } else {
                templated.push(ch);
            }
        }
        groups
            .entry(templated)
            .or_default()
            .push(tensor.original_name.clone());
    }

    let mut yaml = String::from(
        "# scaffolded by binfiddle — PROVISIONAL, every suggestion is heuristic\nschema: binfiddle.nn.pack/v1\nid: scaffold.draft\nversion: 0.1.0\ndescription: >-\n  Auto-derived draft from an observed catalog. Review every pattern,\n  kind, and shape before use; this file is a starting point, not a\n  trusted profile.\nconfig: {}\nbindings:\n",
    );
    for (template, names) in &groups {
        if names.is_empty() {
            continue;
        }
        let example = &names[0];
        let example_tensor = catalog.tensors.iter().find(|t| t.original_name == *example);
        let shape: Vec<String> = example_tensor
            .map(|t| {
                t.shape
                    .iter()
                    .map(|d| format!("\"{}\"", placeholder_for(*d)))
                    .collect()
            })
            .unwrap_or_default();
        yaml.push_str(&format!(
            "  - pattern: \"{template}\"\n    component: \"draft.{}\"\n    kind: dense\n    shape: [{}]\n    # observed {} name(s), e.g. {example}\n",
            template.replace(['{', '}'], "").replace('.', "_"),
            shape.join(", "),
            names.len(),
        ));
    }
    yaml.push_str("# NOTE: kinds, axes, and config parameters are NOT inferred; fill them in.\n");
    Ok(yaml)
}

fn placeholder_for(dim: u64) -> String {
    // Literal observed dimension: provisional packs must lint cleanly, and a
    // literal always evaluates; named parameters are the author's upgrade.
    dim.to_string()
}

// ---- rendering ----

pub fn lint_envelope(pack: &Pack, findings: &[LintFinding]) -> Result<ResultEnvelope, NnError> {
    let items = findings
        .iter()
        .map(|f| {
            Json::object(vec![
                ("severity", Json::Str(f.severity.to_string())),
                ("code", Json::Str(f.code.to_string())),
                ("message", Json::Str(f.message.clone())),
            ])
        })
        .collect::<Result<Vec<_>, _>>()?;
    let errors = findings.iter().filter(|f| f.severity == "error").count();
    let semantic = Json::object(vec![
        ("pack_id", Json::Str(pack.pack_id()?)),
        ("pack", Json::Str(pack.id_name.clone())),
        ("findings", Json::Array(items)),
        ("error_count", Json::Str(errors.to_string())),
    ])?;
    let mut envelope = ResultEnvelope::new("profile lint").with_semantic(semantic);
    if errors > 0 {
        envelope = envelope.with_coverage(false, vec![format!("{errors} error-severity findings")]);
    }
    Ok(envelope)
}

pub fn lint_text(pack: &Pack, findings: &[LintFinding]) -> String {
    let mut out = format!("profile lint: {} v{}\n", pack.id_name, pack.version);
    for finding in findings {
        out.push_str(&format!(
            "  [{}] {}: {}\n",
            finding.severity, finding.code, finding.message
        ));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(yaml: &str) -> Pack {
        Pack::parse(yaml).expect("fixture pack parses")
    }

    #[test]
    fn clean_pack_lints_clean() {
        let pack = parse(
            r#"schema: binfiddle.nn.pack/v1
id: clean.test
version: "1.0.0"
config:
  d: 4
bindings:
  - pattern: "model.layers.{layer}.self_attn.q_proj.weight"
    component: "decoder.layers[{layer}].q"
    kind: dense
    shape: ["d", "d"]
full_attention_interval: 4
"#,
        );
        let findings = lint_pack(&pack);
        assert!(
            findings.iter().any(|f| f.code == "LINT_CLEAN"),
            "{findings:?}"
        );
    }

    #[test]
    fn lint_catches_defects() {
        // Bad expression + duplicate component + all-capture pattern.
        let pack = parse(
            r#"schema: binfiddle.nn.pack/v1
id: broken.test
version: "1.0.0"
config:
  d: 4
bindings:
  - pattern: "{a}.{b}"
    component: "x"
    kind: dense
    shape: ["not_a_param"]
  - pattern: "w.{layer}"
    component: "x"
    kind: dense
    shape: ["d"]
"#,
        );
        let findings = lint_pack(&pack);
        let codes: Vec<&str> = findings.iter().map(|f| f.code).collect();
        assert!(codes.contains(&"SHAPE_EXPRESSION"), "{findings:?}");
        assert!(codes.contains(&"DUPLICATE_COMPONENT"), "{findings:?}");
        assert!(codes.contains(&"PATTERN_ALL_CAPTURE"), "{findings:?}");
        assert!(findings.iter().all(|f| f.severity != "info"));
    }

    #[test]
    fn scaffold_labels_everything_heuristic() {
        let dir = tempfile::tempdir().unwrap();
        // Two [12,4] F32 tensors = 192 payload bytes each.
        let header = r#"{"model.layers.0.self_attn.q_proj.weight":{"dtype":"F32","shape":[12,4],"data_offsets":[0,192]},"model.layers.1.self_attn.q_proj.weight":{"dtype":"F32","shape":[12,4],"data_offsets":[192,384]}}"#;
        let mut data = Vec::new();
        data.extend_from_slice(&(header.len() as u64).to_le_bytes());
        data.extend_from_slice(header.as_bytes());
        data.extend(std::iter::repeat_n(0u8, 384));
        std::fs::write(dir.path().join("m.safetensors"), data).unwrap();
        let budget = crate::nn::budget::Budget::new(
            crate::nn::budget::BudgetCaps::default(),
            None,
            crate::nn::cancel::CancellationToken::new(),
        );
        let report = crate::nn::discover::discover(
            &dir.path().join("m.safetensors"),
            &crate::nn::discover::DiscoverOptions::default(),
            &budget,
        )
        .unwrap();
        let catalog = Catalog::from_discovery(&report).unwrap();
        let scaffold = scaffold_pack(&catalog).unwrap();
        assert!(scaffold.contains("PROVISIONAL"), "{scaffold}");
        assert!(scaffold.contains("heuristic"));
        // Two names collapsed into one templated pattern with the count noted.
        assert!(scaffold.contains("observed 2 name(s)"), "{scaffold}");
        assert!(scaffold.contains("model.layers.{layer}.self_attn.q_proj.weight"));
        // The scaffold itself must parse as a pack.
        let parsed = Pack::parse(&scaffold).expect("scaffold is valid YAML");
        assert_eq!(parsed.bindings.len(), 1);
    }
}
