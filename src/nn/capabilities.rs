//! Capability reporting for the NN workbench surface.
//!
//! The report lists what this build actually implements and what it does not.
//! Unimplemented commands are listed as unavailable so that help output and
//! machine queries never advertise more than the binary can do.

use super::error::NnError;
use super::json::Json;
use super::report::ResultEnvelope;

/// Availability of one capability in this build.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CapabilityStatus {
    Implemented,
    Unavailable,
}

impl CapabilityStatus {
    fn as_str(self) -> &'static str {
        match self {
            CapabilityStatus::Implemented => "implemented",
            CapabilityStatus::Unavailable => "unavailable",
        }
    }
}

/// One capability record.
#[derive(Debug, Clone)]
pub struct Capability {
    pub name: String,
    pub status: CapabilityStatus,
    pub detail: String,
}

impl Capability {
    fn implemented(name: &str, detail: &str) -> Self {
        Capability {
            name: name.to_string(),
            status: CapabilityStatus::Implemented,
            detail: detail.to_string(),
        }
    }

    fn unavailable(name: &str, detail: &str) -> Self {
        Capability {
            name: name.to_string(),
            status: CapabilityStatus::Unavailable,
            detail: detail.to_string(),
        }
    }

    fn to_json(&self) -> Result<Json, NnError> {
        Json::object(vec![
            ("name", Json::Str(self.name.clone())),
            ("status", Json::Str(self.status.as_str().to_string())),
            ("detail", Json::Str(self.detail.clone())),
        ])
    }
}

/// The capability matrix of this build.
pub fn capabilities() -> Vec<Capability> {
    vec![
        Capability::implemented(
            "nn.capabilities",
            "report implemented and unavailable NN workbench capabilities",
        ),
        Capability::implemented(
            "nn.wire",
            "wire contracts: canonical decimal-string integers, strict JSON subset, canonical serialization, domain-separated record identifiers",
        ),
        Capability::implemented(
            "nn.envelopes",
            "versioned result envelopes with completion, coverage, diagnostics, and publication status",
        ),
        Capability::implemented(
            "nn.budgets",
            "hierarchical resource budgets with cancellation checkpoints",
        ),
        Capability::implemented(
            "nn.discover",
            "descriptor-only inventory of SafeTensors and GGUF files and directories; optional --out-catalog persistence",
        ),
        Capability::implemented(
            "nn.ls",
            "tensor and source listing with encoding/source/name filters, name/byte ordering, and bounded pagination",
        ),
        Capability::implemented(
            "nn.show",
            "one tensor's full record (exact name, scoped name, or unique id prefix) with optional --explain evidence",
        ),
        Capability::implemented(
            "nn.select",
            "tensor selection by exact name, scoped name, or id; saved selections bind their catalog and never silently rematch",
        ),
        Capability::unavailable("nn.where", "address mapping is not implemented yet"),
        Capability::unavailable("nn.locate", "reverse address lookup is not implemented yet"),
        Capability::unavailable("nn.impact", "impact analysis is not implemented yet"),
        Capability::unavailable("nn.slice", "extraction is not implemented yet"),
        Capability::unavailable("nn.split", "decomposition is not implemented yet"),
        Capability::unavailable("nn.assemble", "reassembly is not implemented yet"),
        Capability::unavailable("nn.analyze", "numerical inspection is not implemented yet"),
        Capability::unavailable("nn.quant", "quantization inspection is not implemented yet"),
        Capability::unavailable("nn.edit", "transactional editing is not implemented yet"),
        Capability::unavailable("nn.diff", "model comparison is not implemented yet"),
        Capability::unavailable("nn.adapter", "adapter inspection is not implemented yet"),
        Capability::unavailable("nn.fingerprint", "fingerprinting is not implemented yet"),
        Capability::unavailable("nn.tokenizer", "tokenizer inspection is not implemented yet"),
        Capability::unavailable("nn.pack", "model-pack management is not implemented yet"),
        Capability::unavailable("nn.replay", "runtime replay is not implemented yet"),
        Capability::unavailable("nn.capture", "runtime capture is not implemented yet"),
        Capability::unavailable("nn.validate", "artifact validation is not implemented yet"),
    ]
}

/// Build the capabilities result envelope.
pub fn capabilities_envelope() -> Result<ResultEnvelope, NnError> {
    let caps = capabilities();
    let implemented = caps
        .iter()
        .filter(|c| c.status == CapabilityStatus::Implemented)
        .count();
    let unavailable = caps.len() - implemented;
    let semantic = Json::object(vec![
        (
            "capabilities",
            Json::Array(
                caps.iter()
                    .map(Capability::to_json)
                    .collect::<Result<Vec<_>, _>>()?,
            ),
        ),
        ("implemented", Json::Str(implemented.to_string())),
        ("unavailable", Json::Str(unavailable.to_string())),
    ])?;
    Ok(ResultEnvelope::new("capabilities").with_semantic(semantic))
}

/// Render the capabilities report as human-readable text.
pub fn capabilities_text() -> String {
    let caps = capabilities();
    let mut out = String::from("binfiddle nn workbench capabilities\n\n");
    out.push_str("implemented:\n");
    for cap in caps
        .iter()
        .filter(|c| c.status == CapabilityStatus::Implemented)
    {
        out.push_str(&format!("  {:<18} {}\n", cap.name, cap.detail));
    }
    out.push_str("\nunavailable:\n");
    for cap in caps
        .iter()
        .filter(|c| c.status == CapabilityStatus::Unavailable)
    {
        out.push_str(&format!("  {:<18} {}\n", cap.name, cap.detail));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn envelope_is_valid_and_counts_match() {
        let envelope = capabilities_envelope().unwrap();
        let json = envelope.to_json().unwrap();
        let caps = json.get("semantic").unwrap().get("capabilities");
        match caps {
            Some(Json::Array(items)) => assert_eq!(items.len(), capabilities().len()),
            other => panic!("unexpected capabilities array: {other:?}"),
        }
        let implemented: usize = capabilities()
            .iter()
            .filter(|c| c.status == CapabilityStatus::Implemented)
            .count();
        assert_eq!(
            json.get("semantic").unwrap().get("implemented"),
            Some(&Json::Str(implemented.to_string()))
        );
        assert!(envelope.to_json_string().is_ok());
    }

    #[test]
    fn text_report_lists_every_capability() {
        let text = capabilities_text();
        for cap in capabilities() {
            assert!(
                text.contains(&cap.name),
                "missing {} in text report",
                cap.name
            );
        }
    }

    #[test]
    fn names_are_unique() {
        let caps = capabilities();
        let mut names: Vec<&str> = caps.iter().map(|c| c.name.as_str()).collect();
        names.sort_unstable();
        let count = names.len();
        names.dedup();
        assert_eq!(names.len(), count);
    }
}
