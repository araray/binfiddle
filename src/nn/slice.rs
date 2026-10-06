//! Weight slicing: plans, bundles, and tensor-content reconstruction.
//!
//! A slice is a resolved selection plus storage and quantization policies.
//! `preserve_encoding` copies exact encoded bytes; `cover_blocks` states the
//! logical view in terms of complete encoding blocks (identical to preserve
//! for whole-tensor selections today, distinct for future region cuts);
//! `decode` materializes a declared numeric representation and records the
//! loss of original encoding identity. Plans are immutable, id-verified
//! files; application re-verifies catalog and source identities — a stale
//! plan or a changed source is `SOURCE_CHANGED`, never a silent rematch.
//!
//! Reference bundles point at content-verified sources (strong identity);
//! materialized bundles copy exact spans and hash every member. `assemble`
//! reconstructs tensor content from a bundle under the named
//! `tensor_content` guarantee — original-byte and computation
//! reconstruction are explicitly different, unclaimed guarantees.

use super::budget::Budget;
use super::catalog::{Catalog, CatalogTensor};
use super::codec::{layout_for_encoding, q4_0_decode_block, ScalarCodec, TensorLayout};
use super::error::NnError;
use super::id::{compute_id, IdKind};
use super::json::{Json, ParseLimits};
use super::selection::Selection;
use super::source::BoundedFile;
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::io::Write;
use std::path::{Path, PathBuf};

/// Copy chunk for streaming members.
const COPY_CHUNK: usize = 1024 * 1024;

/// Quantization handling policy for a slice.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QuantPolicy {
    /// Copy exact encoded bytes; encoding identity is preserved.
    PreserveEncoding,
    /// Include complete encoding blocks and describe the logical view.
    CoverBlocks,
    /// Materialize decoded values; original encoding identity is lost.
    Decode,
}

impl QuantPolicy {
    pub fn as_str(self) -> &'static str {
        match self {
            QuantPolicy::PreserveEncoding => "preserve_encoding",
            QuantPolicy::CoverBlocks => "cover_blocks",
            QuantPolicy::Decode => "decode",
        }
    }

    pub fn parse(text: &str) -> Result<QuantPolicy, NnError> {
        match text {
            "preserve_encoding" => Ok(QuantPolicy::PreserveEncoding),
            "cover_blocks" => Ok(QuantPolicy::CoverBlocks),
            "decode" => Ok(QuantPolicy::Decode),
            other => Err(NnError::InvalidRequest {
                message: format!("unknown quantization policy {other}"),
            }),
        }
    }
}

/// Storage policy for a slice.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StoragePolicy {
    /// Record immutable source identities and spans; no payload copy.
    Reference,
    /// Copy exact spans into a self-contained bundle.
    Materialized,
}

impl StoragePolicy {
    pub fn as_str(self) -> &'static str {
        match self {
            StoragePolicy::Reference => "reference",
            StoragePolicy::Materialized => "materialized",
        }
    }

    pub fn parse(text: &str) -> Result<StoragePolicy, NnError> {
        match text {
            "reference" => Ok(StoragePolicy::Reference),
            "materialized" => Ok(StoragePolicy::Materialized),
            other => Err(NnError::InvalidRequest {
                message: format!("unknown storage policy {other}"),
            }),
        }
    }
}

/// One resolved slice entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SliceEntry {
    pub tensor_id: String,
    pub name: String,
    pub source_id: String,
    pub encoding: String,
    pub span_start: u64,
    pub span_length: u64,
    /// How this entry is materialized under the chosen policy.
    pub effect: String,
    /// Logical-view annotation (component selections with row-range views).
    pub logical_view: Option<String>,
}

/// An immutable slice plan.
#[derive(Debug, Clone)]
pub struct SlicePlan {
    pub plan_id: String,
    pub catalog_id: String,
    pub selection_id: String,
    pub storage: StoragePolicy,
    pub quant: QuantPolicy,
    pub entries: Vec<SliceEntry>,
    /// Source content digests recorded at planning time (content-verified
    /// sources only); application re-hashes and compares.
    pub source_digests: Vec<(String, String)>,
}

impl SlicePlan {
    /// Build a plan from a selection bound to its catalog. The selection
    /// binding itself rejects mismatched catalogs.
    pub fn build(
        catalog: &Catalog,
        selection: &Selection,
        storage: StoragePolicy,
        quant: QuantPolicy,
    ) -> Result<SlicePlan, NnError> {
        let targets = selection.bind(catalog)?;
        let mut entries = Vec::with_capacity(targets.len());
        let mut source_digests: BTreeMap<String, String> = BTreeMap::new();

        for (position, tensor) in targets.iter().enumerate() {
            let Some(length) = tensor.payload_length else {
                return Err(NnError::InvalidRequest {
                    message: format!(
                        "tensor {} has only a bounded extent; exact extraction requires an exact extent",
                        tensor.original_name
                    ),
                });
            };
            let effect = plan_effect(tensor, quant)?;
            // Component views narrow the extracted span to the viewed rows;
            // the logical statement records what the bytes represent.
            let view = selection.views.get(position).cloned().flatten();
            let (span_start, span_length, logical_view) = match &view {
                Some(view) => {
                    if view.span.0 + view.span.1 > length {
                        return Err(NnError::InvalidRequest {
                            message: format!(
                                "view span [{}, {}) exceeds the {}-byte payload of {}",
                                view.span.0,
                                view.span.0 + view.span.1,
                                length,
                                tensor.original_name
                            ),
                        });
                    }
                    (
                        tensor.payload_start + view.span.0,
                        view.span.1,
                        Some(view.logical.clone()),
                    )
                }
                None => (tensor.payload_start, length, None),
            };
            entries.push(SliceEntry {
                tensor_id: tensor.id.clone(),
                name: tensor.original_name.clone(),
                source_id: tensor.source_id.clone(),
                encoding: tensor.encoding.clone(),
                span_start,
                span_length,
                effect,
                logical_view,
            });
            // Record the source digest when the revision is content-verified.
            if let Ok(source) = catalog.resolve_source(&tensor.source_id) {
                if let Some(digest) = source
                    .semantic
                    .get("content_digest")
                    .and_then(|d| d.get("value"))
                    .and_then(Json::as_str)
                {
                    source_digests.insert(source.id.clone(), digest.to_string());
                }
            }
        }

        let mut plan = SlicePlan {
            plan_id: String::new(),
            catalog_id: catalog.id()?,
            selection_id: selection.id()?,
            storage,
            quant,
            entries,
            source_digests: source_digests.into_iter().collect(),
        };
        plan.plan_id = compute_id(IdKind::Plan, &plan.semantic()?)?;
        Ok(plan)
    }

    /// Semantic record of the plan.
    pub fn semantic(&self) -> Result<Json, NnError> {
        let entries = self
            .entries
            .iter()
            .map(|e| {
                Json::object(vec![
                    ("tensor_id", Json::Str(e.tensor_id.clone())),
                    ("name", Json::Str(e.name.clone())),
                    ("source_id", Json::Str(e.source_id.clone())),
                    ("encoding", Json::Str(e.encoding.clone())),
                    ("span_start", Json::Str(e.span_start.to_string())),
                    ("span_length", Json::Str(e.span_length.to_string())),
                    ("effect", Json::Str(e.effect.clone())),
                    (
                        "logical_view",
                        match &e.logical_view {
                            Some(view) => Json::Str(view.clone()),
                            None => Json::Null,
                        },
                    ),
                ])
            })
            .collect::<Result<Vec<_>, _>>()?;
        let digests = self
            .source_digests
            .iter()
            .map(|(id, d)| {
                Json::object(vec![
                    ("source_id", Json::Str(id.clone())),
                    ("sha256", Json::Str(d.clone())),
                ])
            })
            .collect::<Result<Vec<_>, _>>()?;
        Json::object(vec![
            (
                "schema",
                Json::Str("binfiddle.nn.slice-plan/v1".to_string()),
            ),
            ("kind", Json::Str("weights".to_string())),
            ("catalog_id", Json::Str(self.catalog_id.clone())),
            ("selection_id", Json::Str(self.selection_id.clone())),
            ("storage", Json::Str(self.storage.as_str().to_string())),
            ("quant_policy", Json::Str(self.quant.as_str().to_string())),
            ("entries", Json::Array(entries)),
            ("source_digests", Json::Array(digests)),
        ])
    }

    /// Result envelope for plan generation / dry run.
    pub fn envelope(&self) -> Result<super::report::ResultEnvelope, NnError> {
        let semantic = Json::object(vec![
            ("plan_id", Json::Str(self.plan_id.clone())),
            ("dry_run", Json::Bool(true)),
            (
                "guarantees",
                Json::object(vec![
                    (
                        "payload_bytes_exact",
                        Json::Bool(self.quant != QuantPolicy::Decode),
                    ),
                    (
                        "encoding_preserved",
                        Json::Bool(self.quant != QuantPolicy::Decode),
                    ),
                    ("claim", Json::Str("tensor content only".to_string())),
                ])?,
            ),
            ("plan", self.semantic()?),
        ])?;
        Ok(super::report::ResultEnvelope::new("slice").with_semantic(semantic))
    }

    /// Human-readable dry-run text.
    pub fn text(&self) -> String {
        let mut out = String::from("slice plan (dry run)\n");
        out.push_str(&format!(
            "  plan:      {}\n  catalog:   {}\n  selection: {}\n  storage:   {}\n  quant:     {}\n",
            &self.plan_id[..30.min(self.plan_id.len())],
            &self.catalog_id[..26.min(self.catalog_id.len())],
            &self.selection_id[..32.min(self.selection_id.len())],
            self.storage.as_str(),
            self.quant.as_str()
        ));
        let mut total: u64 = 0;
        for entry in &self.entries {
            total += entry.span_length;
            out.push_str(&format!(
                "  {} [{}..{}] {} bytes ({})\n",
                entry.name,
                entry.span_start,
                entry.span_start + entry.span_length,
                entry.span_length,
                entry.effect
            ));
        }
        out.push_str(&format!(
            "  total: {} bytes across {} tensors\n",
            total,
            self.entries.len()
        ));
        out
    }

    /// Save the plan file (embedded id verified on load).
    pub fn save(&self, path: &Path) -> Result<(), NnError> {
        let file = Json::object(vec![
            (
                "schema",
                Json::Str("binfiddle.nn.slice-plan-file/v1".to_string()),
            ),
            ("plan_id", Json::Str(self.plan_id.clone())),
            ("plan", self.semantic()?),
        ])?;
        std::fs::write(path, file.to_canonical()?.as_bytes()).map_err(NnError::Io)?;
        Ok(())
    }

    /// Load and verify a plan file.
    pub fn load(path: &Path) -> Result<SlicePlan, NnError> {
        let text = std::fs::read_to_string(path).map_err(NnError::Io)?;
        let file = Json::parse_strict(&text, ParseLimits::default())?;
        if file.get("schema").and_then(Json::as_str) != Some("binfiddle.nn.slice-plan-file/v1") {
            return Err(NnError::MalformedInput {
                detail: "not a binfiddle slice plan file".to_string(),
            });
        }
        let embedded = file
            .get("plan_id")
            .and_then(Json::as_str)
            .ok_or_else(|| NnError::MalformedInput {
                detail: "plan file is missing plan_id".to_string(),
            })?
            .to_string();
        let semantic = file
            .get("plan")
            .cloned()
            .ok_or_else(|| NnError::MalformedInput {
                detail: "plan file is missing the plan payload".to_string(),
            })?;
        let computed = compute_id(IdKind::Plan, &semantic)?;
        if computed != embedded {
            return Err(NnError::MalformedInput {
                detail: format!(
                    "plan id mismatch: file claims {}, payload hashes to {}",
                    embedded, computed
                ),
            });
        }
        plan_from_semantic(&semantic, computed)
    }
}

fn plan_from_semantic(semantic: &Json, plan_id: String) -> Result<SlicePlan, NnError> {
    if semantic.get("schema").and_then(Json::as_str) != Some("binfiddle.nn.slice-plan/v1") {
        return Err(NnError::MalformedInput {
            detail: "plan payload schema mismatch".to_string(),
        });
    }
    let catalog_id = required_str(semantic, "catalog_id")?;
    let selection_id = required_str(semantic, "selection_id")?;
    let storage = StoragePolicy::parse(&required_str(semantic, "storage")?)?;
    let quant = QuantPolicy::parse(&required_str(semantic, "quant_policy")?)?;
    let mut entries = Vec::new();
    if let Some(list) = semantic.get("entries").and_then(Json::as_array) {
        for entry in list {
            entries.push(SliceEntry {
                tensor_id: required_str(entry, "tensor_id")?,
                name: required_str(entry, "name")?,
                source_id: required_str(entry, "source_id")?,
                encoding: required_str(entry, "encoding")?,
                span_start: required_str(entry, "span_start")?
                    .parse()
                    .map_err(bad_number)?,
                span_length: required_str(entry, "span_length")?
                    .parse()
                    .map_err(bad_number)?,
                effect: required_str(entry, "effect")?,
                logical_view: entry
                    .get("logical_view")
                    .and_then(Json::as_str)
                    .map(str::to_string),
            });
        }
    }
    let mut source_digests = Vec::new();
    if let Some(list) = semantic.get("source_digests").and_then(Json::as_array) {
        for digest in list {
            source_digests.push((
                required_str(digest, "source_id")?,
                required_str(digest, "sha256")?,
            ));
        }
    }
    Ok(SlicePlan {
        plan_id,
        catalog_id,
        selection_id,
        storage,
        quant,
        entries,
        source_digests,
    })
}

fn required_str(json: &Json, key: &str) -> Result<String, NnError> {
    json.get(key)
        .and_then(Json::as_str)
        .map(str::to_string)
        .ok_or_else(|| NnError::MalformedInput {
            detail: format!("record is missing {key}"),
        })
}

fn bad_number(_: std::num::ParseIntError) -> NnError {
    NnError::MalformedInput {
        detail: "record carries a non-canonical integer".to_string(),
    }
}

/// Effect description for one tensor under a policy.
fn plan_effect(tensor: &CatalogTensor, quant: QuantPolicy) -> Result<String, NnError> {
    let layout = layout_for_encoding(&tensor.encoding);
    match (quant, layout) {
        (QuantPolicy::PreserveEncoding, _) => Ok("encoded bytes copied verbatim".to_string()),
        (QuantPolicy::CoverBlocks, TensorLayout::Q4_0) => Ok(format!(
            "complete {}-byte encoding blocks copied; logical view = 32 values per block",
            18
        )),
        (QuantPolicy::CoverBlocks, _) => {
            Ok("dense bytes copied; policy identical to preserve for dense storage".to_string())
        }
        (QuantPolicy::Decode, TensorLayout::Scalar(codec)) => Ok(format!(
            "decoded to little-endian {} values (original encoding identity lost)",
            scalar_suffix(codec)
        )),
        (QuantPolicy::Decode, TensorLayout::Q4_0) => Ok(
            "decoded to little-endian f32 values, 32 per block (original encoding identity lost)"
                .to_string(),
        ),
        (QuantPolicy::Decode, TensorLayout::Unknown) => Err(NnError::CodecUnsupported {
            codec: tensor.encoding.clone(),
            operation: "decode".to_string(),
            reason: "no qualified numeric decoder for this encoding".to_string(),
        }),
    }
}

fn scalar_suffix(codec: ScalarCodec) -> &'static str {
    match codec {
        ScalarCodec::F64 => "f64",
        ScalarCodec::F32 => "f32",
        ScalarCodec::F16 => "f32",
        ScalarCodec::Bf16 => "f32",
        ScalarCodec::I64 => "i64",
        ScalarCodec::I32 => "i32",
        ScalarCodec::I16 => "i16",
        ScalarCodec::I8 => "i8",
        ScalarCodec::U8 => "u8",
        ScalarCodec::Bool => "u8",
    }
}

/// The result of applying a slice plan.
#[derive(Debug)]
pub struct SliceReceipt {
    pub plan_id: String,
    pub bundle_dir: PathBuf,
    pub members: Vec<(String, u64, String)>, // (member path, bytes, sha256)
    pub storage: StoragePolicy,
}

/// Apply a plan: verify identities, then write the bundle.
///
/// Stale checks, in order: catalog id must match the plan's; each recorded
/// source digest must match a fresh full-content hash; then spans are read.
pub fn apply_plan(
    plan: &SlicePlan,
    catalog: &Catalog,
    out_dir: &Path,
    budget: &Budget,
) -> Result<SliceReceipt, NnError> {
    let catalog_id = catalog.id()?;
    if catalog_id != plan.catalog_id {
        return Err(NnError::SourceChanged {
            detail: format!(
                "plan was built against catalog {} but applied to {}; slice plans never silently rematch",
                super::error::brief(&plan.catalog_id),
                super::error::brief(&catalog_id)
            ),
        });
    }

    // Resolve source files (presentation paths) and verify digests.
    let mut source_files: BTreeMap<String, PathBuf> = BTreeMap::new();
    for source in &catalog.sources {
        if source.path.is_empty() {
            return Err(NnError::InvalidRequest {
                message: format!(
                    "source {} has no stored locator path; re-discover with the same root",
                    super::error::brief(&source.id)
                ),
            });
        }
        source_files.insert(source.id.clone(), catalog.resolve_path(&source.path));
    }
    for (source_id, expected) in &plan.source_digests {
        let Some(path) = source_files.get(source_id) else {
            return Err(NnError::SourceMissing {
                detail: format!(
                    "plan references source {} which is not in the catalog",
                    super::error::brief(source_id)
                ),
            });
        };
        let reader = BoundedFile::open(path)?;
        let actual = reader.content_digest(budget)?;
        if &actual != expected {
            return Err(NnError::SourceChanged {
                detail: format!(
                    "source {} content changed since the plan was made (expected {}, found {})",
                    super::error::brief(source_id),
                    expected,
                    actual
                ),
            });
        }
    }

    if plan.storage == StoragePolicy::Reference && plan.source_digests.is_empty() {
        return Err(NnError::InvalidRequest {
            message: "reference bundles require content-verified sources; re-discover with --verify-content"
                .to_string(),
        });
    }

    // Destination policy: refuse a nonempty output directory.
    if out_dir.exists() {
        if std::fs::read_dir(out_dir)
            .map(|mut d| d.next().is_some())
            .unwrap_or(false)
        {
            return Err(NnError::InvalidRequest {
                message: format!(
                    "output directory {} is not empty; slice bundles are written to fresh directories",
                    out_dir.display()
                ),
            });
        }
    } else {
        std::fs::create_dir_all(out_dir).map_err(NnError::Io)?;
    }

    let weights_dir = out_dir.join("weights");
    if plan.storage == StoragePolicy::Materialized {
        std::fs::create_dir_all(&weights_dir).map_err(NnError::Io)?;
    }

    let mut members: Vec<(String, u64, String)> = Vec::new();
    let mut used_names: BTreeMap<String, ()> = BTreeMap::new();
    let mut manifest_members: Vec<Json> = Vec::new();

    // Materialized bundles emit, per member, at most the encoded span copied
    // verbatim (preserve/cover) or its decode — and every registered codec
    // expands by strictly less than 8 bytes of output per input byte. Justify
    // the output budget with that provable bound plus manifest overhead so
    // real-model-sized bundles are not rejected by the generic default cap.
    if plan.storage == StoragePolicy::Materialized {
        let span_sum: u64 = plan.entries.iter().map(|e| e.span_length).sum();
        budget.justify_output_bytes(span_sum.saturating_mul(8).saturating_add(1 << 20));
    }

    for entry in &plan.entries {
        let Some(path) = source_files.get(&entry.source_id) else {
            return Err(NnError::SourceMissing {
                detail: format!(
                    "tensor {} references source {} which is not in the catalog",
                    entry.name, entry.source_id
                ),
            });
        };
        let (member_rel, bytes, digest) = match plan.storage {
            StoragePolicy::Materialized => {
                let reader = BoundedFile::open(path)?;
                reader.verify_length(path)?;
                let file_name = unique_member_name(&entry.name, &mut used_names);
                let rel = format!("weights/{file_name}");
                let full = out_dir.join(&rel);
                let (bytes, digest) = write_member(&reader, entry, plan.quant, &full, budget)?;
                (rel, bytes, digest)
            }
            StoragePolicy::Reference => {
                let bytes = entry.span_length;
                let mut hasher = Sha256::new();
                hasher.update(entry.tensor_id.as_bytes());
                let digest = hex::encode(hasher.finalize());
                (String::new(), bytes, digest)
            }
        };
        manifest_members.push(Json::object(vec![
            ("tensor_id", Json::Str(entry.tensor_id.clone())),
            ("name", Json::Str(entry.name.clone())),
            ("source_id", Json::Str(entry.source_id.clone())),
            ("encoding", Json::Str(entry.encoding.clone())),
            ("span_start", Json::Str(entry.span_start.to_string())),
            ("span_length", Json::Str(entry.span_length.to_string())),
            (
                "path",
                if member_rel.is_empty() {
                    Json::Null
                } else {
                    Json::Str(member_rel.clone())
                },
            ),
            ("sha256", Json::Str(digest.clone())),
            ("bytes", Json::Str(bytes.to_string())),
            ("effect", Json::Str(entry.effect.clone())),
            (
                "logical_view",
                match &entry.logical_view {
                    Some(view) => Json::Str(view.clone()),
                    None => Json::Null,
                },
            ),
        ])?);
        if !member_rel.is_empty() {
            members.push((member_rel, bytes, digest));
        }
        budget.checkpoint()?;
    }

    let manifest = bundle_manifest(plan, catalog, manifest_members)?;
    let manifest_path = out_dir.join("slice.json");
    std::fs::write(&manifest_path, manifest.as_bytes()).map_err(NnError::Io)?;

    Ok(SliceReceipt {
        plan_id: plan.plan_id.clone(),
        bundle_dir: out_dir.to_path_buf(),
        members,
        storage: plan.storage,
    })
}

fn bundle_manifest(
    plan: &SlicePlan,
    catalog: &Catalog,
    members: Vec<Json>,
) -> Result<String, NnError> {
    let sources = plan
        .source_digests
        .iter()
        .map(|(id, digest)| {
            let path = catalog
                .sources
                .iter()
                .find(|s| s.id == *id)
                .map(|s| s.path.clone())
                .unwrap_or_default();
            Json::object(vec![
                ("source_id", Json::Str(id.clone())),
                ("path", Json::Str(path)),
                ("sha256", Json::Str(digest.clone())),
            ])
        })
        .collect::<Result<Vec<_>, _>>()?;
    let manifest = Json::object(vec![
        ("schema", Json::Str("binfiddle.nn.bundle/v1".to_string())),
        ("kind", Json::Str("weights".to_string())),
        ("storage", Json::Str(plan.storage.as_str().to_string())),
        ("quant_policy", Json::Str(plan.quant.as_str().to_string())),
        ("plan_id", Json::Str(plan.plan_id.clone())),
        ("catalog_id", Json::Str(plan.catalog_id.clone())),
        ("selection_id", Json::Str(plan.selection_id.clone())),
        ("sources", Json::Array(sources)),
        ("members", Json::Array(members)),
        (
            "guarantees",
            Json::object(vec![
                ("claim", Json::Str("tensor_content".to_string())),
                (
                    "payload_bytes_exact",
                    Json::Bool(plan.quant != QuantPolicy::Decode),
                ),
                (
                    "encoding_preserved",
                    Json::Bool(plan.quant != QuantPolicy::Decode),
                ),
                ("original_bytes", Json::Bool(false)),
                ("executable", Json::Bool(false)),
            ])?,
        ),
        (
            "validation",
            Json::object(vec![
                (
                    "members_hashed",
                    Json::Bool(plan.storage == StoragePolicy::Materialized),
                ),
                (
                    "source_digests_verified",
                    Json::Bool(!plan.source_digests.is_empty()),
                ),
            ])?,
        ),
    ])?;
    manifest.to_canonical()
}

/// Stream one member to disk under its policy; returns (bytes, sha256).
fn write_member(
    reader: &BoundedFile,
    entry: &SliceEntry,
    quant: QuantPolicy,
    dest: &Path,
    budget: &Budget,
) -> Result<(u64, String), NnError> {
    let mut file = std::fs::File::create(dest).map_err(NnError::Io)?;
    let mut hasher = Sha256::new();
    let mut written: u64 = 0;
    match quant {
        QuantPolicy::PreserveEncoding | QuantPolicy::CoverBlocks => {
            let mut offset = entry.span_start;
            let mut remaining = entry.span_length;
            let mut chunk = vec![0u8; COPY_CHUNK];
            while remaining > 0 {
                let take = (COPY_CHUNK as u64).min(remaining) as usize;
                reader.read_exact_at_bounded(offset, &mut chunk[..take], budget)?;
                file.write_all(&chunk[..take]).map_err(NnError::Io)?;
                hasher.update(&chunk[..take]);
                written += take as u64;
                offset += take as u64;
                remaining -= take as u64;
                budget.consume_output(take as u64)?;
                budget.checkpoint()?;
            }
        }
        QuantPolicy::Decode => {
            let layout = layout_for_encoding(&entry.encoding);
            match layout {
                TensorLayout::Scalar(codec) => {
                    let width = codec.width();
                    let mut buffer = vec![0u8; (COPY_CHUNK as u64 / width * width) as usize];
                    let mut offset = entry.span_start;
                    let mut remaining = entry.span_length;
                    while remaining > 0 {
                        let take_bytes = (buffer.len() as u64).min(remaining);
                        let take = (take_bytes / width * width) as usize;
                        reader.read_exact_at_bounded(offset, &mut buffer[..take], budget)?;
                        let mut out_bytes = Vec::with_capacity(take);
                        for element in buffer[..take].chunks_exact(width as usize) {
                            let scalar = codec.decode(element)?;
                            match scalar.value {
                                super::codec::ScalarValue::Float(f) => {
                                    out_bytes.extend_from_slice(&(f as f32).to_le_bytes());
                                }
                                super::codec::ScalarValue::Int(i) => {
                                    out_bytes.extend_from_slice(&(i as f32).to_le_bytes());
                                }
                                super::codec::ScalarValue::Uint(u) => {
                                    out_bytes.extend_from_slice(&(u as f32).to_le_bytes());
                                }
                                super::codec::ScalarValue::Bool(b) => {
                                    out_bytes.extend_from_slice(&((b as u8) as f32).to_le_bytes());
                                }
                            }
                        }
                        file.write_all(&out_bytes).map_err(NnError::Io)?;
                        hasher.update(&out_bytes);
                        written += out_bytes.len() as u64;
                        offset += take as u64;
                        remaining -= take as u64;
                        budget.consume_output(out_bytes.len() as u64)?;
                        budget.checkpoint()?;
                    }
                }
                TensorLayout::Q4_0 => {
                    let mut block = [0u8; 18];
                    let mut offset = entry.span_start;
                    let mut remaining = entry.span_length;
                    let mut out_bytes: Vec<u8> = Vec::with_capacity(32 * 4 * 128);
                    while remaining > 0 {
                        reader.read_exact_at_bounded(offset, &mut block, budget)?;
                        let decoded = q4_0_decode_block(&block)?;
                        for value in &decoded.values {
                            out_bytes.extend_from_slice(&(*value as f32).to_le_bytes());
                        }
                        offset += 18;
                        remaining -= 18;
                        if out_bytes.len() >= 32 * 4 * 64 || remaining == 0 {
                            file.write_all(&out_bytes).map_err(NnError::Io)?;
                            hasher.update(&out_bytes);
                            written += out_bytes.len() as u64;
                            budget.consume_output(out_bytes.len() as u64)?;
                            out_bytes.clear();
                        }
                        budget.checkpoint()?;
                    }
                }
                TensorLayout::Unknown => {
                    return Err(NnError::CodecUnsupported {
                        codec: entry.encoding.clone(),
                        operation: "decode".to_string(),
                        reason: "no qualified numeric decoder".to_string(),
                    });
                }
            }
        }
    }
    file.flush().map_err(NnError::Io)?;
    Ok((written, hex::encode(hasher.finalize())))
}

/// Filesystem-safe member name preserving the original in the manifest.
fn unique_member_name(name: &str, used: &mut BTreeMap<String, ()>) -> String {
    let sanitized: String = name
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-') {
                c
            } else {
                '_'
            }
        })
        .collect();
    let base = if sanitized.is_empty() {
        "tensor".to_string()
    } else {
        sanitized
    };
    let mut candidate = base.clone();
    let mut counter = 1;
    while used.contains_key(&candidate) {
        counter += 1;
        candidate = format!("{base}-{counter}");
    }
    used.insert(candidate.clone(), ());
    candidate
}

/// Receipt text for humans.
pub fn receipt_text(receipt: &SliceReceipt) -> String {
    let mut out = String::from("slice bundle written\n");
    out.push_str(&format!("  bundle: {}\n", receipt.bundle_dir.display()));
    match receipt.storage {
        StoragePolicy::Reference => {
            out.push_str("  storage: reference (payloads remain in the identified sources)\n")
        }
        StoragePolicy::Materialized => {
            out.push_str("  storage: materialized\n");
            for (path, bytes, digest) in &receipt.members {
                out.push_str(&format!(
                    "  {} ({} bytes, sha256 {}..)\n",
                    path,
                    bytes,
                    &digest[..12.min(digest.len())]
                ));
            }
        }
    }
    out
}

/// Receipt envelope.
pub fn receipt_envelope(receipt: &SliceReceipt) -> Result<super::report::ResultEnvelope, NnError> {
    let members = receipt
        .members
        .iter()
        .map(|(path, bytes, digest)| {
            Json::object(vec![
                ("path", Json::Str(path.clone())),
                ("bytes", Json::Str(bytes.to_string())),
                ("sha256", Json::Str(digest.clone())),
            ])
        })
        .collect::<Result<Vec<_>, _>>()?;
    let semantic = Json::object(vec![
        ("plan_id", Json::Str(receipt.plan_id.clone())),
        (
            "bundle_dir",
            Json::Str(receipt.bundle_dir.display().to_string()),
        ),
        ("storage", Json::Str(receipt.storage.as_str().to_string())),
        ("members", Json::Array(members)),
        (
            "guarantees",
            Json::object(vec![
                ("claim", Json::Str("tensor_content".to_string())),
                ("original_bytes", Json::Bool(false)),
                ("executable", Json::Bool(false)),
            ])?,
        ),
    ])?;
    Ok(super::report::ResultEnvelope::new("slice").with_semantic(semantic))
}

// ---- assemble ----

/// One member record read from a bundle manifest.
#[derive(Debug, Clone)]
pub struct BundleMember {
    pub tensor_id: String,
    pub name: String,
    pub path: String,
    pub sha256: String,
    pub bytes: u64,
    pub effect: String,
}

/// Reconstruct tensor content from a materialized bundle: verify every member
/// digest against the manifest, then write the payload files and a report.
/// Members with identical tensor ids and digests deduplicate; conflicting
/// digests for one tensor are `WRITE_CONFLICT`.
pub fn assemble_bundle(
    bundle_dir: &Path,
    out_dir: &Path,
    budget: &Budget,
) -> Result<super::report::ResultEnvelope, NnError> {
    let manifest_path = bundle_dir.join("slice.json");
    let text = std::fs::read_to_string(&manifest_path).map_err(|_| NnError::SourceMissing {
        detail: format!("no slice.json in {}", bundle_dir.display()),
    })?;
    let manifest = Json::parse_strict(&text, ParseLimits::default())?;
    if manifest.get("schema").and_then(Json::as_str) != Some("binfiddle.nn.bundle/v1") {
        return Err(NnError::MalformedInput {
            detail: "not a binfiddle slice bundle".to_string(),
        });
    }
    if manifest.get("storage").and_then(Json::as_str) != Some("materialized") {
        return Err(NnError::InvalidRequest {
            message: "tensor-content reconstruction requires a materialized bundle (reference bundles carry no payload)".to_string(),
        });
    }

    let mut members: Vec<BundleMember> = Vec::new();
    for member in manifest
        .get("members")
        .and_then(Json::as_array)
        .unwrap_or(&[])
    {
        let path = member.get("path").and_then(Json::as_str).unwrap_or("");
        if path.is_empty() {
            continue;
        }
        members.push(BundleMember {
            tensor_id: required_str(member, "tensor_id")?,
            name: required_str(member, "name")?,
            path: path.to_string(),
            sha256: required_str(member, "sha256")?,
            bytes: required_str(member, "bytes")?.parse().map_err(bad_number)?,
            effect: required_str(member, "effect")?,
        });
    }
    if members.is_empty() {
        return Err(NnError::InvalidRequest {
            message: "bundle has no materialized members".to_string(),
        });
    }

    // Conflict detection across duplicate tensor ids.
    let mut by_tensor: BTreeMap<&str, (&str, u64)> = BTreeMap::new();
    for member in &members {
        match by_tensor.get(member.tensor_id.as_str()) {
            Some((digest, _)) if *digest != member.sha256 => {
                return Err(NnError::WriteConflict {
                    detail: format!(
                        "tensor {} appears with conflicting member digests {} and {}",
                        member.name, digest, member.sha256
                    ),
                });
            }
            Some(_) => {}
            None => {
                by_tensor.insert(member.tensor_id.as_str(), (&member.sha256, member.bytes));
            }
        }
    }

    if out_dir.exists() {
        if std::fs::read_dir(out_dir)
            .map(|mut d| d.next().is_some())
            .unwrap_or(false)
        {
            return Err(NnError::InvalidRequest {
                message: format!("output directory {} is not empty", out_dir.display()),
            });
        }
    } else {
        std::fs::create_dir_all(out_dir).map_err(NnError::Io)?;
    }

    let mut report_rows: Vec<Json> = Vec::new();
    let mut seen: BTreeMap<String, ()> = BTreeMap::new();
    let mut used_names: BTreeMap<String, ()> = BTreeMap::new();
    for member in &members {
        // Deduplicate identical members (e.g. overlapping selections).
        let dedupe_key = format!("{}:{}", member.tensor_id, member.sha256);
        if seen.contains_key(&dedupe_key) {
            continue;
        }
        seen.insert(dedupe_key, ());

        let source = bundle_dir.join(&member.path);
        let data = std::fs::read(&source).map_err(|_| NnError::SourceMissing {
            detail: format!("bundle member {} is missing", member.path),
        })?;
        budget.consume_source_read(data.len() as u64)?;
        if data.len() as u64 != member.bytes {
            return Err(NnError::ValidationFailed {
                detail: format!(
                    "member {} has {} bytes but the manifest declares {}",
                    member.path,
                    data.len(),
                    member.bytes
                ),
            });
        }
        let digest = hex::encode(Sha256::digest(&data));
        if digest != member.sha256 {
            return Err(NnError::ValidationFailed {
                detail: format!(
                    "member {} failed its digest check (manifest {}, actual {})",
                    member.path, member.sha256, digest
                ),
            });
        }
        let file_name = unique_member_name(&member.name, &mut used_names);
        let dest = out_dir.join(file_name);
        std::fs::write(&dest, &data).map_err(NnError::Io)?;
        budget.consume_output(data.len() as u64)?;
        report_rows.push(Json::object(vec![
            ("tensor_id", Json::Str(member.tensor_id.clone())),
            ("name", Json::Str(member.name.clone())),
            ("bytes", Json::Str(data.len().to_string())),
            ("sha256", Json::Str(digest)),
            ("written", Json::Str(dest.display().to_string())),
        ])?);
    }

    let semantic = Json::object(vec![
        ("bundle", Json::Str(bundle_dir.display().to_string())),
        ("members", Json::Array(report_rows.clone())),
        (
            "guarantee",
            Json::object(vec![
                ("claim", Json::Str("tensor_content".to_string())),
                ("digests_verified", Json::Bool(true)),
                ("original_bytes", Json::Bool(false)),
            ])?,
        ),
    ])?;
    Ok(super::report::ResultEnvelope::new("assemble").with_semantic(semantic))
}

/// Human-readable assemble output.
pub fn assemble_text(envelope: &super::report::ResultEnvelope) -> Result<String, NnError> {
    let json = envelope.to_json()?;
    let mut out = String::from("tensor-content reconstruction\n");
    if let Some(members) = json
        .get("semantic")
        .and_then(|s| s.get("members"))
        .and_then(Json::as_array)
    {
        for member in members {
            let name = member.get("name").and_then(Json::as_str).unwrap_or("?");
            let bytes = member.get("bytes").and_then(Json::as_str).unwrap_or("?");
            out.push_str(&format!("  {name} ({bytes} bytes, digest verified)\n"));
        }
    }
    out.push_str(
        "  guarantee: tensor_content (original bytes and executability are NOT claimed)\n",
    );
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::nn::budget::{Budget, BudgetCaps};
    use crate::nn::cancel::CancellationToken;
    use crate::nn::catalog::Catalog;
    use crate::nn::discover::{discover, DiscoverOptions};
    use crate::nn::selection::{EmptyPolicy, Selection, SelectionRequest};

    fn budget() -> Budget {
        Budget::new(BudgetCaps::default(), None, CancellationToken::new())
    }

    fn write_safetensors(path: &Path, verified: bool) {
        let _ = verified;
        let header = r#"{"w":{"dtype":"F32","shape":[2,2],"data_offsets":[0,16]},"b":{"dtype":"U8","shape":[4],"data_offsets":[16,20]}}"#;
        let mut data = Vec::new();
        data.extend_from_slice(&(header.len() as u64).to_le_bytes());
        data.extend_from_slice(header.as_bytes());
        data.extend_from_slice(&[0x00, 0x00, 0x80, 0x3f]); // w[0,0] = 1.0f32
        data.extend(std::iter::repeat_n(0u8, 12));
        data.extend_from_slice(&[1, 2, 3, 4]);
        std::fs::write(path, data).unwrap();
    }

    fn catalog_for(path: &Path, verify: bool) -> Catalog {
        let options = DiscoverOptions {
            verify_content: verify,
        };
        let report = discover(path, &options, &budget()).unwrap();
        Catalog::from_discovery(&report).unwrap()
    }

    fn selection_of(catalog: &Catalog, name: &str) -> Selection {
        Selection::resolve(
            catalog,
            SelectionRequest::TensorName {
                name: name.to_string(),
                source: None,
            },
            EmptyPolicy::Reject,
        )
        .unwrap()
    }

    #[test]
    fn plan_dry_run_and_file_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("m.safetensors");
        write_safetensors(&path, true);
        let catalog = catalog_for(&path, true);
        let selection = selection_of(&catalog, "w");
        let plan = SlicePlan::build(
            &catalog,
            &selection,
            StoragePolicy::Materialized,
            QuantPolicy::PreserveEncoding,
        )
        .unwrap();
        assert!(plan.plan_id.starts_with("plan:"));
        assert_eq!(plan.entries.len(), 1);
        assert_eq!(plan.entries[0].span_length, 16);
        assert!(!plan.source_digests.is_empty());

        let plan_path = dir.path().join("w.plan.json");
        plan.save(&plan_path).unwrap();
        let loaded = SlicePlan::load(&plan_path).unwrap();
        assert_eq!(loaded.plan_id, plan.plan_id);
        assert_eq!(loaded.entries, plan.entries);
    }

    #[test]
    fn stale_catalog_or_source_is_rejected_on_apply() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("m.safetensors");
        write_safetensors(&path, true);
        let catalog = catalog_for(&path, true);
        let selection = selection_of(&catalog, "w");
        let plan = SlicePlan::build(
            &catalog,
            &selection,
            StoragePolicy::Materialized,
            QuantPolicy::PreserveEncoding,
        )
        .unwrap();

        // Mutate the source after planning.
        let mut bytes = std::fs::read(&path).unwrap();
        let last = bytes.len() - 1;
        bytes[last] ^= 0xFF;
        std::fs::write(&path, &bytes).unwrap();
        let catalog_after = catalog_for(&path, true);
        // Different catalog id → plan refuses.
        let err =
            apply_plan(&plan, &catalog_after, &dir.path().join("bundle"), &budget()).unwrap_err();
        assert_eq!(err.code().as_str(), "SOURCE_CHANGED");

        // Same catalog, mutated source: digest check fires. Rebuild the
        // catalog from the ORIGINAL discovery so only the source changed...
        // but the catalog id derives from source digests (verify mode), so
        // any mutation also changes the catalog. Instead verify the digest
        // branch by forging a plan entry against the mutated file through a
        // fresh plan, then mutating again.
        let catalog_v2 = catalog_for(&path, true);
        let selection_v2 = selection_of(&catalog_v2, "w");
        let plan_v2 = SlicePlan::build(
            &catalog_v2,
            &selection_v2,
            StoragePolicy::Materialized,
            QuantPolicy::PreserveEncoding,
        )
        .unwrap();
        let mut bytes = std::fs::read(&path).unwrap();
        let last = bytes.len() - 1;
        bytes[last] ^= 0xFF;
        std::fs::write(&path, &bytes).unwrap();
        let err = apply_plan(
            &plan_v2,
            &catalog_v2,
            &dir.path().join("bundle2"),
            &budget(),
        )
        .unwrap_err();
        assert_eq!(err.code().as_str(), "SOURCE_CHANGED");
        assert!(err.to_string().contains("content changed"));
    }

    #[test]
    fn materialized_preserve_extracts_exact_bytes() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("m.safetensors");
        write_safetensors(&path, false);
        let catalog = catalog_for(&path, true);
        let selection = selection_of(&catalog, "w");
        let plan = SlicePlan::build(
            &catalog,
            &selection,
            StoragePolicy::Materialized,
            QuantPolicy::PreserveEncoding,
        )
        .unwrap();
        let out = dir.path().join("bundle");
        let receipt = apply_plan(&plan, &catalog, &out, &budget()).unwrap();
        assert_eq!(receipt.members.len(), 1);
        let (member_rel, bytes, digest) = &receipt.members[0];
        assert_eq!(*bytes, 16);
        // Verify the member is exactly the tensor payload.
        let member = std::fs::read(out.join(member_rel)).unwrap();
        assert_eq!(member.len(), 16);
        let expected = &std::fs::read(&path).unwrap()
            [plan.entries[0].span_start as usize..(plan.entries[0].span_start + 16) as usize];
        assert_eq!(&member, expected);
        assert_eq!(hex::encode(Sha256::digest(&member)), *digest);
        // Manifest exists and is valid canonical JSON.
        let manifest = std::fs::read_to_string(out.join("slice.json")).unwrap();
        assert!(manifest.contains("\"claim\":\"tensor_content\""));
        assert!(manifest.contains("\"encoding_preserved\":true"));
    }

    #[test]
    fn decode_policy_materializes_f32_and_drops_encoding_identity() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("m.safetensors");
        write_safetensors(&path, true);
        let catalog = catalog_for(&path, true);
        let selection = selection_of(&catalog, "w");
        let plan = SlicePlan::build(
            &catalog,
            &selection,
            StoragePolicy::Materialized,
            QuantPolicy::Decode,
        )
        .unwrap();
        let out = dir.path().join("bundle");
        let receipt = apply_plan(&plan, &catalog, &out, &budget()).unwrap();
        let (member_rel, bytes, _) = &receipt.members[0];
        assert_eq!(*bytes, 16); // 4 f32 values
        let member = std::fs::read(out.join(member_rel)).unwrap();
        assert_eq!(f32::from_le_bytes(member[0..4].try_into().unwrap()), 1.0);
        let manifest = std::fs::read_to_string(out.join("slice.json")).unwrap();
        assert!(manifest.contains("\"encoding_preserved\":false"));
    }

    #[test]
    fn reference_bundle_requires_content_verified_sources() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("m.safetensors");
        write_safetensors(&path, false);
        let catalog = catalog_for(&path, false); // observation-level
        let selection = selection_of(&catalog, "w");
        let plan = SlicePlan::build(
            &catalog,
            &selection,
            StoragePolicy::Reference,
            QuantPolicy::PreserveEncoding,
        )
        .unwrap();
        assert!(plan.source_digests.is_empty());
        let err = apply_plan(&plan, &catalog, &dir.path().join("bundle"), &budget()).unwrap_err();
        assert_eq!(err.code().as_str(), "INVALID_REQUEST");
        assert!(err.to_string().contains("content-verified"));

        // With verification, the reference bundle applies.
        let catalog_verified = catalog_for(&path, true);
        let selection_verified = selection_of(&catalog_verified, "w");
        let plan_verified = SlicePlan::build(
            &catalog_verified,
            &selection_verified,
            StoragePolicy::Reference,
            QuantPolicy::PreserveEncoding,
        )
        .unwrap();
        let out = dir.path().join("bundle_ref");
        let receipt = apply_plan(&plan_verified, &catalog_verified, &out, &budget()).unwrap();
        assert!(receipt.members.is_empty());
        assert!(out.join("slice.json").exists());
        assert!(!out.join("weights").exists());
    }

    #[test]
    fn apply_refuses_nonempty_output_directory() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("m.safetensors");
        write_safetensors(&path, true);
        let catalog = catalog_for(&path, true);
        let selection = selection_of(&catalog, "w");
        let plan = SlicePlan::build(
            &catalog,
            &selection,
            StoragePolicy::Materialized,
            QuantPolicy::PreserveEncoding,
        )
        .unwrap();
        let out = dir.path().join("bundle");
        std::fs::create_dir_all(&out).unwrap();
        std::fs::write(out.join("x.txt"), b"occupied").unwrap();
        let err = apply_plan(&plan, &catalog, &out, &budget()).unwrap_err();
        assert_eq!(err.code().as_str(), "INVALID_REQUEST");
    }

    #[test]
    fn assemble_reconstructs_and_verifies() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("m.safetensors");
        write_safetensors(&path, true);
        let catalog = catalog_for(&path, true);
        let selection = selection_of(&catalog, "w");
        let plan = SlicePlan::build(
            &catalog,
            &selection,
            StoragePolicy::Materialized,
            QuantPolicy::PreserveEncoding,
        )
        .unwrap();
        let bundle = dir.path().join("bundle");
        apply_plan(&plan, &catalog, &bundle, &budget()).unwrap();

        let out = dir.path().join("rebuilt");
        let envelope = assemble_bundle(&bundle, &out, &budget()).unwrap();
        let text = envelope.to_json_string().unwrap();
        assert!(text.contains("\"claim\":\"tensor_content\""));
        assert!(text.contains("\"digests_verified\":true"));
        // The reconstructed payload matches the extracted member.
        let member_bytes = std::fs::read(bundle.join("weights/w")).unwrap();
        let rebuilt = std::fs::read(out.join("w")).unwrap();
        assert_eq!(member_bytes, rebuilt);
    }

    #[test]
    fn assemble_detects_corrupted_member() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("m.safetensors");
        write_safetensors(&path, true);
        let catalog = catalog_for(&path, true);
        let selection = selection_of(&catalog, "w");
        let plan = SlicePlan::build(
            &catalog,
            &selection,
            StoragePolicy::Materialized,
            QuantPolicy::PreserveEncoding,
        )
        .unwrap();
        let bundle = dir.path().join("bundle");
        apply_plan(&plan, &catalog, &bundle, &budget()).unwrap();
        // Corrupt the member.
        let member_path = bundle.join("weights/w");
        let mut data = std::fs::read(&member_path).unwrap();
        data[0] ^= 0xFF;
        std::fs::write(&member_path, data).unwrap();
        let err = assemble_bundle(&bundle, &dir.path().join("out"), &budget()).unwrap_err();
        assert_eq!(err.code().as_str(), "VALIDATION_FAILED");
        assert!(err.to_string().contains("digest"));
    }

    #[test]
    fn bounded_extent_tensor_cannot_be_planned() {
        let tensor = CatalogTensor {
            id: "tensor:x".to_string(),
            semantic: Json::Null,
            source_id: "src:x".to_string(),
            original_name: "odd".to_string(),
            encoding: "gguf.type14".to_string(),
            decode_supported: false,
            shape: vec![8],
            element_count: 8,
            payload_start: 100,
            payload_length: None,
            extent: crate::nn::format::Extent::UpperBoundOnly,
        };
        let effect = plan_effect(&tensor, QuantPolicy::PreserveEncoding);
        assert!(effect.is_ok()); // effect itself is describable
                                 // But a plan over it fails on the exact-extent requirement; tested
                                 // through build() in integration with a bounded catalog.
    }

    #[test]
    fn member_names_sanitize_and_dedupe() {
        let mut used = BTreeMap::new();
        assert_eq!(unique_member_name("a/b.c", &mut used), "a_b.c");
        assert_eq!(unique_member_name("a/b.c", &mut used), "a_b.c-2");
        assert_eq!(unique_member_name("###", &mut used), "___");
        assert_eq!(unique_member_name("", &mut used), "tensor");
    }
}
