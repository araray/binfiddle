//! Domain-separated persistent identifiers.
//!
//! `id = kind + ":" + hex(SHA256(UTF8("binfiddle.nn." + kind + "/v1") + NUL + JCS(semantic)))`
//!
//! The identifier names a semantic record, not a file. Human output may
//! abbreviate it, but machine-readable records carry the complete digest.

use super::error::NnError;
use super::json::Json;

/// Registered identifier domains.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum IdKind {
    Source,
    Tensor,
    View,
    Component,
    Catalog,
    Selection,
    Plan,
    Receipt,
}

impl IdKind {
    /// Wire spelling used inside the identity prefix and the ID itself.
    pub fn as_str(self) -> &'static str {
        match self {
            IdKind::Source => "src",
            IdKind::Tensor => "tensor",
            IdKind::View => "view",
            IdKind::Component => "component",
            IdKind::Catalog => "catalog",
            IdKind::Selection => "selection",
            IdKind::Plan => "plan",
            IdKind::Receipt => "receipt",
        }
    }

    /// Parse a domain name.
    pub fn from_str_kind(kind: &str) -> Result<IdKind, NnError> {
        Ok(match kind {
            "src" => IdKind::Source,
            "tensor" => IdKind::Tensor,
            "view" => IdKind::View,
            "component" => IdKind::Component,
            "catalog" => IdKind::Catalog,
            "selection" => IdKind::Selection,
            "plan" => IdKind::Plan,
            "receipt" => IdKind::Receipt,
            other => {
                return Err(NnError::WireSyntax {
                    detail: format!("unknown identifier domain {}", super::error::brief(other)),
                })
            }
        })
    }
}

/// Compute the complete identifier for a semantic record.
///
/// The semantic payload must be an object; its canonical serialization is
/// hashed between the domain prefix and the digest.
pub fn compute_id(kind: IdKind, semantic: &Json) -> Result<String, NnError> {
    if !matches!(semantic, Json::Object(_)) {
        return Err(NnError::WireSyntax {
            detail: "semantic payload of an identified record must be an object".to_string(),
        });
    }
    let canonical = semantic.to_canonical()?;
    let prefix = format!("binfiddle.nn.{}/v1", kind.as_str());
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(prefix.as_bytes());
    hasher.update([0u8]);
    hasher.update(canonical.as_bytes());
    Ok(format!(
        "{}:{}",
        kind.as_str(),
        hex::encode(hasher.finalize())
    ))
}

/// Validate the complete form `kind:hex64` and split it.
pub fn validate_id(id: &str) -> Result<(IdKind, &str), NnError> {
    let (kind, digest) = id.split_once(':').ok_or_else(|| NnError::WireSyntax {
        detail: format!(
            "identifier must be 'kind:hex64': {}",
            super::error::brief(id)
        ),
    })?;
    let kind = IdKind::from_str_kind(kind)?;
    if digest.len() != 64
        || !digest
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    {
        return Err(NnError::WireSyntax {
            detail: format!(
                "identifier digest must be 64 lowercase hex characters: {}",
                super::error::brief(id)
            ),
        });
    }
    Ok((kind, digest))
}

#[cfg(test)]
mod tests {
    use super::*;

    // The four vectors below are the published workbench fixture
    // `examples/q4_0_source_map.json`. Reproducing them proves the identity
    // algorithm and canonicalization agree with the reference generator.

    fn fixture_source_semantic() -> Json {
        Json::parse_strict(
            r#"{
              "schema": "binfiddle.nn.source/v1",
              "kind": "file",
              "length": "18",
              "content_digest": {
                "algorithm": "sha256",
                "value": "4a014529a4848a42c18165a78215609b2eabf05c78cbd494aef2e3451a5f5a61"
              },
              "observation": {"method": "synthetic_fixture"},
              "consistency": "content_verified",
              "acquisition": {
                "method": "generated_static_fixture",
                "point_in_time_claim": false
              }
            }"#,
            super::super::json::ParseLimits::default(),
        )
        .unwrap()
    }

    #[test]
    fn reproduces_fixture_source_id() {
        let id = compute_id(IdKind::Source, &fixture_source_semantic()).unwrap();
        assert_eq!(
            id,
            "src:2899e4ac26eef66ea17858f9eb7ff41ba605363595b6b6eff3e23ee870baccc1"
        );
    }

    #[test]
    fn reproduces_fixture_tensor_id() {
        let semantic = Json::parse_strict(
            r#"{
              "schema": "binfiddle.nn.tensor/v1",
              "source_id": "src:2899e4ac26eef66ea17858f9eb7ff41ba605363595b6b6eff3e23ee870baccc1",
              "original_name": "fixture.weight",
              "shape": ["32"],
              "encoding_id": "ggml.q4_0.reference",
              "payload_start": "0",
              "payload_length": "18"
            }"#,
            super::super::json::ParseLimits::default(),
        )
        .unwrap();
        let id = compute_id(IdKind::Tensor, &semantic).unwrap();
        assert_eq!(
            id,
            "tensor:0bc3e882826ea1b21528e859177e398be614e26ebdddbfcdce7869210b01533b"
        );
    }

    #[test]
    fn reproduces_fixture_catalog_id() {
        let semantic = Json::parse_strict(
            r#"{
              "schema": "binfiddle.nn.catalog-fixture/v1",
              "source_id": "src:2899e4ac26eef66ea17858f9eb7ff41ba605363595b6b6eff3e23ee870baccc1",
              "tensor_id": "tensor:0bc3e882826ea1b21528e859177e398be614e26ebdddbfcdce7869210b01533b",
              "coverage": {"complete": true}
            }"#,
            super::super::json::ParseLimits::default(),
        )
        .unwrap();
        let id = compute_id(IdKind::Catalog, &semantic).unwrap();
        assert_eq!(
            id,
            "catalog:443106218a0a8ac89fb8dfcd172bea25f11347b348f97eddeed074cc3cbce445"
        );
    }

    #[test]
    fn reproduces_fixture_selection_id() {
        let semantic = Json::parse_strict(
            r#"{
              "schema": "binfiddle.nn.selection/v1",
              "catalog_id": "catalog:443106218a0a8ac89fb8dfcd172bea25f11347b348f97eddeed074cc3cbce445",
              "request": {
                "kind": "tensor_index",
                "name": "fixture.weight",
                "index": ["0"]
              },
              "targets": [
                {
                  "tensor_id": "tensor:0bc3e882826ea1b21528e859177e398be614e26ebdddbfcdce7869210b01533b",
                  "index": ["0"]
                }
              ],
              "ordering": "request_order",
              "empty_policy": "reject"
            }"#,
            super::super::json::ParseLimits::default(),
        )
        .unwrap();
        let id = compute_id(IdKind::Selection, &semantic).unwrap();
        assert_eq!(
            id,
            "selection:84e5383a4fe93665bc6d9841d706ae49b0c8fc90f1d142fb0f0b327d53a4fe86"
        );
    }

    #[test]
    fn rejects_non_object_semantic() {
        assert!(compute_id(IdKind::Source, &Json::Null).is_err());
    }

    #[test]
    fn validate_id_round_trip() {
        let id = compute_id(IdKind::Source, &fixture_source_semantic()).unwrap();
        let (kind, digest) = validate_id(&id).unwrap();
        assert_eq!(kind, IdKind::Source);
        assert_eq!(digest.len(), 64);
    }

    #[test]
    fn validate_id_rejects_malformed() {
        assert!(validate_id("src").is_err());
        assert!(validate_id("nope:abc").is_err());
        assert!(validate_id("src:ABC").is_err());
        assert!(validate_id("src:zz63").is_err());
        // Uppercase hex is rejected.
        let id = compute_id(IdKind::Source, &fixture_source_semantic()).unwrap();
        let upper = id.to_uppercase().replace("SRC:", "src:");
        assert!(validate_id(&upper).is_err());
    }

    #[test]
    fn ids_are_domain_separated() {
        let semantic = fixture_source_semantic();
        let a = compute_id(IdKind::Source, &semantic).unwrap();
        let b = compute_id(IdKind::Catalog, &semantic).unwrap();
        assert_ne!(a, b);
    }
}
