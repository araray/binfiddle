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
            "descriptor-only inventory of SafeTensors, GGUF, and ONNX files and directories; GGUF split-shard group completeness and cross-shard uniqueness checks; stdin discovery via a bounded private spool; optional --out-catalog persistence",
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
            "nn.where",
            "forward address mapping with precision classifications (exact_contiguous/exact_bits/no_payload/unresolved); scalar codecs and the Q4_0 block layout",
        ),
        Capability::implemented(
            "nn.locate",
            "reverse lookup: file offset to owning tensors with coordinate families and quantization-block roles",
        ),
        Capability::implemented(
            "nn.slice",
            "weight extraction over saved selections: dry-run plans, id-verified plan files, reference and materialized bundles with per-member digests, preserve_encoding/cover_blocks/decode policies, component annotations and logical-view sections for row-range views",
        ),
        Capability::implemented(
            "nn.assemble",
            "tensor-content reconstruction from materialized bundles with digest verification (original-byte and executable claims are explicitly not made)",
        ),
        Capability::implemented(
            "nn.edit",
            "transactional fixed-size edits: preimage-recording plans, typed scalar writes through codecs (exact/nearest policies), masked sub-byte writes for Q4_0 nibbles, verification ladder (catalog + source digest + preimage), fresh-output application with preservation proof and container reparse (SafeTensors, GGUF, and ONNX raw_data spans), undo bundles bound to the exact edited revision, and an MLP channel-prune structural recipe (gate/up rows + down columns with shape updates)",
        ),
        Capability::implemented(
            "nn.analyze",
            "bounded numerical inspection: metadata/sample/full scans with coverage records, Welford statistics, non-finite categories, overflow-safe L2 norms, declared-edge histograms, reference-error metrics with zero-denominator policies, and Q4_0 block views",
        ),
        Capability::implemented(
            "nn.impact",
            "span/edit-plan impact analysis: owning tensors, decode dependencies, and numerical influence sets (a Q4_0 scale byte influences all 32 elements of its block; a code nibble exactly one); behavioral consequences are NOT predicted",
        ),
        Capability::implemented(
            "nn.split",
            "one-command layer decomposition: per-layer child selections (synthesized, rebindable selector expressions), reference plans or materialized per-layer bundles, and a root split.json manifest with the coverage partition (assigned/shared/unresolved) and member-vs-unique byte accounting — payloads never duplicated by navigation overlap",
        ),
        Capability::unavailable("nn.quant", "quantization inspection is not implemented yet"),
        Capability::implemented(
            "nn.select",
            "tensor selection by exact name, scoped name, id, or component selector through a pack (families with single/range/list/wildcard indexers; heads[N] row-range views on fused query/gate weights)",
        ),
        Capability::implemented(
            "nn.diff",
            "layered comparison of two content-verified catalogs: package member sets, descriptor changes, encoded-content equality with the repack-vs-payload distinction, optional decoded comparison with exact_bits/lenient NaN and signed-zero policies, unmatched populations visible, exact claims only — never lineage",
        ),
        Capability::implemented(
            "nn.fingerprint",
            "exact content fingerprints (canonical digest of name+shape+encoding+payload) with method statements; with --compare, an experimental evidence graph adds exact_payload_match / same_structure / similar_under_mapping edges from sampled-block fingerprints (deterministic positions, method/version/threshold/score records, false-match analysis on a synthetic corpus, unsampled-bytes limitation stated); no lineage or chronology claims",
        ),
        Capability::implemented(
            "nn.adapter",
            "LoRA-style adapter inspection: factor pairing by target, rank and orientation checks, orphan and extra-tensor findings; descriptor-level claims only — merge arithmetic and base-model compatibility are not verified",
        ),
        Capability::implemented(
            "nn.tokenizer",
            "tokenizer asset classification and tokenizer.json structure inspection (model type, vocab size, merges, added tokens) plus vocabulary-level two-file diffs; tokenization behavior and template rendering are not evaluated",
        ),
        Capability::implemented(
            "nn.research",
            "experimental row-permutation alignment: exact per-row digest multisets decide whether one same-shape dense weight is a row permutation of another, recovering the mapping with duplicate-row ambiguity counts; computational equivalence is explicitly not claimed; qualified on a deterministic synthetic corpus (detection 1.0, false-match 0.0)",
        ),
        Capability::implemented(
            "nn.partition",
            "static execution partition planning: contiguous layer groups balanced by encoded weight bytes (optimal max-stage DP), unlayered tensors reported separately, estimates state exactly what they include and exclude — no runtime, transfer, or speedup claims",
        ),
        Capability::implemented(
            "nn.carve",
            "artifact carving: scan raw files for embedded SafeTensors/GGUF containers, validate candidates structurally on exact subviews, and report spans with verified/candidate confidence labels — no model validity or recoverability claims",
        ),
        Capability::implemented(
            "nn.pack",
            "declarative model packs: id-verified YAML manifests, name-pattern bindings with capture groups, shape expressions over configuration parameters, recognition with contradiction retention, architecture views in ls/show, static lint (duplicate components, expression evaluation, schedule arity, all-capture patterns; exit 7 on errors) and provisional scaffolding from observed catalogs (every suggestion labeled heuristic)",
        ),
        Capability::unavailable("nn.replay", "runtime replay is not implemented yet"),
        Capability::unavailable("nn.capture", "runtime capture is not implemented yet"),
        Capability::implemented(
            "nn.validate",
            "structural artifact validation with precise per-source verdicts (structurally_valid_for_reader / unsupported_feature / invalid / incomplete), error-severity findings surfaced, behavior_not_evaluated stated; exit 7 when anything fails",
        ),
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
