//! Reviewed evidence sidecars (BF-009 / BF-010 enabler).
//!
//! A sidecar is a canonical, strict-JSON document that associates reviewed
//! human knowledge — an operation description, an observation timeline,
//! capture notes — with a binfiddle artifact (a file, a catalog, a
//! selection, a bundle manifest). The sidecar records the subject's
//! content identity at review time; verification recomputes it from the
//! artifact and reports agreement. Sidecars are data: nothing in them
//! executes, and their claims line states what they do not establish.

use super::error::NnError;
use super::json::{Json, ParseLimits};
use super::report::ResultEnvelope;
use std::path::Path;

/// Subject kinds a sidecar can bind to.
pub const SUBJECT_KINDS: [&str; 4] = ["file", "catalog", "selection", "bundle"];

/// One parsed sidecar.
pub struct Sidecar {
    pub subject_kind: String,
    pub subject_path: String,
    pub identity_form: String,
    pub identity_value: String,
    pub role: String,
    pub reviewed_by: String,
    pub claims: String,
    pub semantic: Json,
}

fn required_str(json: &Json, key: &str) -> Result<String, NnError> {
    json.get(key)
        .and_then(Json::as_str)
        .map(str::to_string)
        .ok_or_else(|| NnError::MalformedInput {
            detail: format!("sidecar is missing {key}"),
        })
}

/// Parse and structurally validate a sidecar document.
pub fn parse_sidecar(text: &str) -> Result<Sidecar, NnError> {
    let file = Json::parse_strict(text, ParseLimits::for_input_len(text.len()))?;
    if file.get("schema").and_then(Json::as_str) != Some("binfiddle.nn.evidence-sidecar/v1") {
        return Err(NnError::MalformedInput {
            detail: "not a binfiddle evidence sidecar".to_string(),
        });
    }
    let subject = file
        .get("subject")
        .cloned()
        .ok_or_else(|| NnError::MalformedInput {
            detail: "sidecar is missing subject".to_string(),
        })?;
    let subject_kind = required_str(&subject, "kind")?;
    if !SUBJECT_KINDS.contains(&subject_kind.as_str()) {
        return Err(NnError::MalformedInput {
            detail: format!(
                "subject kind {subject_kind} must be one of {}",
                SUBJECT_KINDS.join(", ")
            ),
        });
    }
    let subject_path = required_str(&subject, "path")?;
    let identity =
        file.get("subject_identity")
            .cloned()
            .ok_or_else(|| NnError::MalformedInput {
                detail: "sidecar is missing subject_identity".to_string(),
            })?;
    let identity_form = required_str(&identity, "form")?;
    if !["sha256", "catalog_id", "selection_id"].contains(&identity_form.as_str()) {
        return Err(NnError::MalformedInput {
            detail: format!(
                "identity form {identity_form} must be sha256, catalog_id, or selection_id"
            ),
        });
    }
    let identity_value = required_str(&identity, "value")?;
    let role = required_str(&file, "role")?;
    if role.is_empty() {
        return Err(NnError::MalformedInput {
            detail: "sidecar role must not be empty".to_string(),
        });
    }
    let reviewed_by = required_str(&file, "reviewed_by")?;
    let claims = required_str(&file, "claims")?;
    Ok(Sidecar {
        subject_kind,
        subject_path,
        identity_form,
        identity_value,
        role,
        reviewed_by,
        claims,
        semantic: file,
    })
}

/// Compute the content identity of a subject artifact.
pub fn subject_identity(sidecar: &Sidecar) -> Result<(String, String), NnError> {
    let path = Path::new(&sidecar.subject_path);
    match sidecar.subject_kind.as_str() {
        "file" | "bundle" => {
            // Bundle subjects bind to their manifest file (slice.json /
            // split.json / undo.json); the binding is over that file's
            // bytes, exactly as for a plain file subject.
            let bytes = std::fs::read(path).map_err(|_| NnError::SourceMissing {
                detail: format!("cannot read subject {}", path.display()),
            })?;
            use sha2::Digest;
            Ok((
                "sha256".to_string(),
                hex::encode(sha2::Sha256::digest(&bytes)),
            ))
        }
        "catalog" => {
            let catalog = super::catalog::Catalog::load(path)?;
            Ok(("catalog_id".to_string(), catalog.id()?))
        }
        "selection" => {
            let selection = super::selection::Selection::load(path)?;
            Ok(("selection_id".to_string(), selection.id()?))
        }
        other => Err(NnError::InvalidRequest {
            message: format!("unsupported subject kind {other}"),
        }),
    }
}

pub struct VerifyReport {
    pub role: String,
    pub reviewed_by: String,
    pub claims: String,
    pub bound: bool,
    pub expected: String,
    pub observed: String,
}

impl VerifyReport {
    pub fn text(&self) -> String {
        let mut out = String::from("evidence sidecar\n");
        out.push_str(&format!("  role:       {}\n", self.role));
        out.push_str(&format!("  reviewed_by: {}\n", self.reviewed_by));
        let binding = if self.bound {
            "identity matches the subject artifact"
        } else {
            "IDENTITY MISMATCH — the artifact changed after review (or the sidecar names another artifact)"
        };
        out.push_str(&format!("  binding:    {binding}\n"));
        if !self.bound {
            out.push_str(&format!("  sidecar:    {}\n", self.expected));
            out.push_str(&format!("  artifact:   {}\n", self.observed));
        }
        out.push_str(&format!("  claims:     {}\n", self.claims));
        out.push_str(
            "  limits: sidecars are reviewed data bound to artifact identity; they execute nothing and establish no runtime behavior\n",
        );
        out
    }

    pub fn envelope(&self) -> Result<ResultEnvelope, NnError> {
        let semantic = Json::object(vec![
            ("role", Json::Str(self.role.clone())),
            ("reviewed_by", Json::Str(self.reviewed_by.clone())),
            ("bound", Json::Bool(self.bound)),
            ("sidecar_identity", Json::Str(self.expected.clone())),
            ("artifact_identity", Json::Str(self.observed.clone())),
            ("claims", Json::Str(self.claims.clone())),
        ])?;
        let envelope = ResultEnvelope::new("evidence.verify").with_semantic(semantic);
        Ok(if self.bound {
            envelope
        } else {
            envelope.with_status(super::report::CompletionStatus::Partial)
        })
    }
}

/// Verify a sidecar against the artifact it names: recompute the subject
/// identity and compare with the reviewed value.
pub fn verify(sidecar_path: &Path) -> Result<VerifyReport, NnError> {
    let text = std::fs::read_to_string(sidecar_path).map_err(|_| NnError::SourceMissing {
        detail: format!("cannot read sidecar {}", sidecar_path.display()),
    })?;
    let sidecar = parse_sidecar(&text)?;
    let (form, observed) = subject_identity(&sidecar)?;
    let bound = form == sidecar.identity_form && observed == sidecar.identity_value;
    Ok(VerifyReport {
        role: sidecar.role,
        reviewed_by: sidecar.reviewed_by,
        claims: sidecar.claims,
        bound,
        expected: format!("{}:{}", sidecar.identity_form, sidecar.identity_value),
        observed: format!("{form}:{observed}"),
    })
}

/// Emit a skeleton sidecar for an artifact, identity pre-computed, for a
/// human to fill with reviewed content. Drafts carry status "draft".
pub fn init_skeleton(
    subject_kind: &str,
    subject_path: &Path,
    role: &str,
) -> Result<String, NnError> {
    let probe = Sidecar {
        subject_kind: subject_kind.to_string(),
        subject_path: subject_path.display().to_string(),
        identity_form: String::new(),
        identity_value: String::new(),
        role: role.to_string(),
        reviewed_by: String::new(),
        claims: String::new(),
        semantic: Json::Null,
    };
    let (form, value) = subject_identity(&probe)?;
    let doc = Json::object(vec![
        (
            "schema",
            Json::Str("binfiddle.nn.evidence-sidecar/v1".to_string()),
        ),
        (
            "subject",
            Json::object(vec![
                ("kind", Json::Str(subject_kind.to_string())),
                ("path", Json::Str(subject_path.display().to_string())),
            ])?,
        ),
        (
            "subject_identity",
            Json::object(vec![("form", Json::Str(form)), ("value", Json::Str(value))])?,
        ),
        ("role", Json::Str(role.to_string())),
        ("status", Json::Str("draft".to_string())),
        ("reviewed_by", Json::Str("".to_string())),
        (
            "payload",
            Json::object(vec![(
                "note",
                Json::Str("fill with reviewed content".to_string()),
            )])?,
        ),
        (
            "claims",
            Json::Str(
                "DRAFT — unreviewed; states nothing until reviewed_by and claims are completed"
                    .to_string(),
            ),
        ),
    ])?;
    doc.to_canonical()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn skeleton_json() -> String {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("artifact.bin");
        std::fs::write(&path, b"evidence bytes").unwrap();
        init_skeleton("file", &path, "operation-description").unwrap()
    }

    #[test]
    fn skeleton_round_trips_through_parse() {
        let text = skeleton_json();
        let sidecar = parse_sidecar(&text).unwrap();
        assert_eq!(sidecar.subject_kind, "file");
        assert_eq!(sidecar.identity_form, "sha256");
        assert_eq!(sidecar.identity_value.len(), 64);
        assert_eq!(sidecar.role, "operation-description");
    }

    #[test]
    fn schema_rejects_missing_claims_and_bad_kinds() {
        let text = skeleton_json().replace("\"claims\":\"", "\"not_claims\":\"");
        assert!(parse_sidecar(&text).is_err());
        let text = skeleton_json().replace("\"kind\":\"file\"", "\"kind\":\"gpu\"");
        assert!(parse_sidecar(&text).is_err());
        let text = skeleton_json().replace("binfiddle.nn.evidence-sidecar/v1", "something.else/v9");
        assert!(parse_sidecar(&text).is_err());
    }

    #[test]
    fn verify_detects_mutation() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a.bin");
        std::fs::write(&path, b"v1 bytes").unwrap();
        let sidecar_path = dir.path().join("a.sidecar.json");
        std::fs::write(
            &sidecar_path,
            init_skeleton("file", &path, "timeline").unwrap(),
        )
        .unwrap();
        // Unmodified: bound.
        let report = verify(&sidecar_path).unwrap();
        assert!(report.bound);
        // Mutated subject: mismatch.
        std::fs::write(&path, b"v2 bytes").unwrap();
        let report = verify(&sidecar_path).unwrap();
        assert!(!report.bound);
        assert!(report.observed != report.expected);
    }
}
