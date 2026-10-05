//! `nn show`: one tensor's full record with optional evidence explanation.

use super::catalog::{Catalog, CatalogTensor};
use super::error::NnError;
use super::json::Json;
use super::queries::tensor_record;
use super::report::ResultEnvelope;

/// Show target resolution: exact name (optionally scoped) or id prefix.
pub enum ShowTarget<'a> {
    Name {
        name: &'a str,
        source: Option<&'a str>,
    },
    Id {
        id: &'a str,
    },
}

/// Resolve exactly one tensor for `show`.
pub fn resolve_show_target<'a>(
    catalog: &'a Catalog,
    target: &ShowTarget<'_>,
) -> Result<&'a CatalogTensor, NnError> {
    match target {
        ShowTarget::Name { name, source } => {
            let matches = catalog.tensors_by_name(name, *source)?;
            match matches.as_slice() {
                [one] => Ok(one),
                [] => Err(NnError::SourceMissing {
                    detail: format!("no tensor named {}", super::error::brief(name)),
                }),
                _ => Err(NnError::AmbiguousBinding {
                    detail: format!(
                        "tensor name {} matches {} tensors across sources; add --source <id-prefix-or-path>",
                        super::error::brief(name),
                        matches.len()
                    ),
                }),
            }
        }
        ShowTarget::Id { id } => catalog.tensor_by_id(id),
    }
}

/// Evidence section: where the record came from and what is known.
fn explain_record(catalog: &Catalog, tensor: &CatalogTensor) -> Result<Json, NnError> {
    let source = catalog.resolve_source(&tensor.source_id)?;
    let consistency = source
        .semantic
        .get("consistency")
        .and_then(Json::as_str)
        .unwrap_or("observation");
    let length = source
        .semantic
        .get("length")
        .and_then(Json::as_str)
        .unwrap_or("?");
    Json::object(vec![
        ("source_id", Json::Str(source.id.clone())),
        ("source_path", Json::Str(source.path.clone())),
        (
            "source_format",
            Json::Str(source.format.clone().unwrap_or_default()),
        ),
        ("source_consistency", Json::Str(consistency.to_string())),
        ("source_length", Json::Str(length.to_string())),
        (
            "known",
            Json::Array(vec![
                Json::Str("original tensor name and stored shape".to_string()),
                Json::Str("encoding identifier from the container".to_string()),
                Json::Str("file-qualified payload span".to_string()),
                Json::Str("container structural validity".to_string()),
            ]),
        ),
        (
            "not_known",
            Json::Array(vec![
                Json::Str("architectural role (no model pack is installed)".to_string()),
                Json::Str("decoded values (numeric decoding is not part of listing)".to_string()),
            ]),
        ),
    ])
}

/// Build the show envelope.
pub fn show_envelope(
    catalog: &Catalog,
    tensor: &CatalogTensor,
    explain: bool,
) -> Result<ResultEnvelope, NnError> {
    let mut pairs = vec![
        ("catalog_id", Json::Str(catalog.id()?)),
        ("tensor", tensor_record(tensor)?),
    ];
    if explain {
        pairs.push(("explain", explain_record(catalog, tensor)?));
    }
    let semantic = Json::object(pairs)?;
    Ok(ResultEnvelope::new("show").with_semantic(semantic))
}

/// Human-readable tensor card.
pub fn show_text(catalog: &Catalog, tensor: &CatalogTensor, explain: bool) -> String {
    let mut out = String::new();
    out.push_str(&format!("tensor {}\n", tensor.original_name));
    out.push_str(&format!("  id:       {}\n", tensor.id));
    out.push_str(&format!("  encoding: {}\n", tensor.encoding));
    let shape = tensor
        .shape
        .iter()
        .map(|d| d.to_string())
        .collect::<Vec<_>>()
        .join(" x ");
    out.push_str(&format!(
        "  shape:    [{}] ({} elements)\n",
        shape, tensor.element_count
    ));
    out.push_str(&format!(
        "  payload:  {}..{} ({})\n",
        tensor.payload_start,
        tensor
            .payload_length
            .map(|l| tensor.payload_start + l)
            .map(|end| end.to_string())
            .unwrap_or_else(|| "?".to_string()),
        tensor
            .payload_length
            .map(|l| format!("{l} bytes, exact"))
            .unwrap_or_else(|| "extent only bounded".to_string())
    ));
    out.push_str(&format!(
        "  decoding: {}\n",
        if tensor.decode_supported {
            "supported (qualified numeric decoder registered)"
        } else {
            "unsupported (no qualified numeric decoder for this encoding)"
        }
    ));
    if explain {
        if let Ok(source) = catalog.resolve_source(&tensor.source_id) {
            out.push_str(&format!(
                "  source:   {} ({})\n",
                if source.path.is_empty() {
                    source.id.as_str()
                } else {
                    &source.path
                },
                source
                    .format
                    .as_deref()
                    .unwrap_or("format presentation missing")
            ));
            out.push_str(
                "  evidence: name/shape/encoding/span observed from the container; \
architectural role unknown without a model pack\n",
            );
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::nn::budget::{Budget, BudgetCaps};
    use crate::nn::cancel::CancellationToken;
    use crate::nn::catalog::Catalog;
    use crate::nn::discover::{discover, DiscoverOptions};

    fn budget() -> Budget {
        Budget::new(BudgetCaps::default(), None, CancellationToken::new())
    }

    fn sample(dir: &std::path::Path) -> Catalog {
        let header = "{\"w\":{\"dtype\":\"F32\",\"shape\":[2,2],\"data_offsets\":[0,16]}}";
        let mut data = Vec::new();
        data.extend_from_slice(&(header.len() as u64).to_le_bytes());
        data.extend_from_slice(header.as_bytes());
        data.extend_from_slice(&[0u8; 16]);
        std::fs::write(dir.join("m.safetensors"), data).unwrap();
        let report = discover(
            &dir.join("m.safetensors"),
            &DiscoverOptions::default(),
            &budget(),
        )
        .unwrap();
        Catalog::from_discovery(&report).unwrap()
    }

    #[test]
    fn show_by_name_and_id_and_ambiguity_error() {
        let dir = tempfile::tempdir().unwrap();
        let catalog = sample(dir.path());
        let tensor = resolve_show_target(
            &catalog,
            &ShowTarget::Name {
                name: "w",
                source: None,
            },
        )
        .unwrap();
        assert_eq!(tensor.original_name, "w");
        let by_id = resolve_show_target(
            &catalog,
            &ShowTarget::Id {
                id: &tensor.id[..20],
            },
        )
        .unwrap();
        assert_eq!(by_id.id, tensor.id);

        let missing = resolve_show_target(
            &catalog,
            &ShowTarget::Name {
                name: "absent",
                source: None,
            },
        )
        .unwrap_err();
        assert_eq!(missing.code().as_str(), "SOURCE_MISSING");
    }

    #[test]
    fn envelope_and_text_render() {
        let dir = tempfile::tempdir().unwrap();
        let catalog = sample(dir.path());
        let tensor = &catalog.tensors[0];
        let envelope = show_envelope(&catalog, tensor, true).unwrap();
        let text = envelope.to_json_string().unwrap();
        assert!(text.contains("\"operation\":\"show\""));
        assert!(text.contains("\"explain\""));
        assert!(text.contains("architectural role"));
        let human = show_text(&catalog, tensor, true);
        assert!(human.contains("tensor w"));
        assert!(human.contains("16 bytes, exact"));
        assert!(human.contains("m.safetensors"));
    }
}
