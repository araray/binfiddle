# Binfiddle NN Workbench — User Guide

This guide covers the `nn` command family: a static workbench for neural-network
artifact files (SafeTensors, GGUF, ONNX) and the directories that package them.
Every command runs offline, reads bounded amounts of data, never executes model
code, and never modifies an input file — all outputs are fresh files.

> For the classic byte-level commands (`read`, `write`, `search`, …) see
> [USAGE.md](USAGE.md). A one-page cheat sheet lives in
> [NN_QUICK_REFERENCE.md](NN_QUICK_REFERENCE.md). For complete real-model
> walkthroughs, see the deep dives:
> [NN_DEEPDIVE_KOKORO.md](NN_DEEPDIVE_KOKORO.md) (ONNX export, pickle boundary),
> [NN_DEEPDIVE_GEMMA.md](NN_DEEPDIVE_GEMMA.md) (16 GB SafeTensors, model packs,
> transactional editing at scale),
> [NN_DEEPDIVE_QWEN.md](NN_DEEPDIVE_QWEN.md) (131-shard package, hybrid
> attention, fused head views),
> [NN_DEEPDIVE_GLM_EXL3.md](NN_DEEPDIVE_GLM_EXL3.md) (EXL3 quantized
> storage on GLM-5.3-Flash — variant-profiled packs at
> [packs/](../packs/)), and
> [NN_DEEPDIVE_GLM_LAB.md](NN_DEEPDIVE_GLM_LAB.md) (a small, reproducible
> GLM expert specimen, independent decoding, and a two-byte intervention).

**Contents**

- [What the workbench does — and never does](#what-the-workbench-does--and-never-does)
- [Core concepts](#core-concepts)
- [A five-minute walkthrough](#a-five-minute-walkthrough)
- [Command reference](#command-reference)
  - [nn capabilities](#nn-capabilities) · [nn discover](#nn-discover) · [nn ls](#nn-ls) ·
    [nn show](#nn-show) · [nn select](#nn-select) · [nn where](#nn-where) ·
    [nn locate](#nn-locate) · [nn impact](#nn-impact) · [nn slice](#nn-slice) ·
    [nn assemble](#nn-assemble) · [nn split](#nn-split) · [nn analyze](#nn-analyze) ·
    [nn edit](#nn-edit) · [nn pack](#nn-pack) · [nn diff](#nn-diff) ·
    [nn fingerprint](#nn-fingerprint) · [nn partition](#nn-partition) ·
    [nn carve](#nn-carve) · [nn validate](#nn-validate) · [nn adapter](#nn-adapter) ·
    [nn tokenizer](#nn-tokenizer) · [nn research](#nn-research)
- [Component selector grammar](#component-selector-grammar)
- [Exit codes and error codes](#exit-codes-and-error-codes)
- [The JSON result envelope](#the-json-result-envelope)
- [Model pack authoring](#model-pack-authoring)
- [Howtos](#howtos)
- [Boundaries](#boundaries)

---

## What the workbench does — and never does

The workbench answers static questions about model files:

- **Inventory** — what tensors exist, with which shapes and encodings, where in
  the file, and how reliably we know that.
- **Addressing** — where element `[123,456]` of a tensor lives on disk, down to
  the bit for packed encodings, and the reverse question of which tensor owns a
  byte offset.
- **Extraction** — pull selected tensors (or components, or layers) into
  bundles, then reassemble their content.
- **Editing** — plan a small, typed change; verify everything about the source;
  write one fresh output file; keep a verified undo.
- **Analysis** — statistics, distributions, quantization blocks, and comparison
  against reference values.
- **Storage codecs** — qualified decode of exotic quantization storage: EXL3
  trellis fields into Hadamard-domain values.
- **Comparison** — layered diffs, content fingerprints, evidence graphs,
  adapter inspection, tokenizer comparison.
- **Research** — exact, reproducible experiments (row-permutation alignment).
- **Planning** — static plans with byte accounting: which shards to download,
  how layers group into stages, and whether stages fit supplied per-device
  capacity observations.

It deliberately never does three things:

1. **Never executes model code.** No inference, no Python, no framework
   imports. ONNX files are read as descriptors, not run.
2. **Never overclaims.** Every report ends with a `claims:` line stating what
   it proves — and what it does not. Similarity is never lineage; a static
   byte estimate is never a speedup prediction.
3. **Never modifies an input.** Mutating commands write a fresh output file
   (refusing to overwrite) and verify every byte outside the change.

Unknown formats, unknown encodings, and malformed files never erase evidence:
they are reported as findings with precise severities.

## Core concepts

| Concept | What it is |
|---|---|
| **Source** | One file (or a stdin capture) considered for discovery. Directories are scanned one level deep; symlinks are never followed. |
| **Catalog** | A saved record of a discovery (`*.nn.json`). Tensor and source identities are digests of what was **observed** — metadata and descriptors by default, the full payload only when discovered with `--verify-content`. A descriptor-only catalog binds the observed metadata and the source observation (`consistency: observation`, `content_digest: null`); it proves nothing about payload bytes you did not hash. Identity strength is explicit everywhere: `observation` versus `content_verified`. |
| **Tensor id** | `tensor:<sha256>` — stable across re-discovery of identical content. Any unique prefix works where a command takes `--id`. |
| **Selection** | A saved, resolvable set of tensors (`*.sel.json`), bound to the catalog it came from. Selections never silently rematch against different bytes. |
| **Plan** | A saved, id-verified recipe (slice plan, edit plan). Applying re-verifies the catalog, the full source digest, and recorded preimage bytes — stale or tampered plans abort. |
| **Bundle** | The output directory of a slice or split: payloads plus a manifest describing exactly what is stored and under which guarantee. |
| **Model pack** | A pure-data YAML file mapping tensor-name patterns to named components (`decoder.layers[3].mlp.gate`, …). Packs describe; they contain no executable content. |
| **Result envelope** | Every command's machine-readable output (`--report-format json`): schema `binfiddle.nn.result/v1`, with status, coverage, diagnostics, publication, and the semantic payload. Integers are decimal strings. |

Content-verified discovery (`nn discover --verify-content`) hashes full file
contents. Some operations (all `nn edit` commands) require it, because they
re-verify sources before touching anything.

## A five-minute walkthrough

These commands run against any SafeTensors/GGUF file. Replace the names with
yours; the captured outputs below come from a small demo model (a 2×2 F32
tensor named `w` whose payload starts at byte 632).

```bash
# 1. What is in this file? (no payload bytes are read)
binfiddle -i model.safetensors nn discover

# 2. Save a catalog; hashing contents strengthens identity
binfiddle -i model-dir/ nn discover --verify-content --out-catalog model.nn.json

# 3. Browse tensors, biggest first
binfiddle nn ls --catalog model.nn.json --sort bytes --limit 20

# 4. Inspect one tensor and the evidence behind it
binfiddle nn show --catalog model.nn.json --tensor model.layers.0.weight --explain

# 5. Where does element [1,1] live in the file?
binfiddle nn where --catalog model.nn.json --tensor w --index 1,1
# → precision: exact_contiguous, file span [644, 648)

# 6. And the reverse: which tensor owns offset 0x27c?
binfiddle nn locate --catalog model.nn.json --offset 0x27c
# → w: element [0,1] (payload [632, 648))

# 7. Machine-readable everything
binfiddle nn ls --catalog model.nn.json --report-format json
```

## Command reference

Every command accepts `--report-format text|json` (default `text`). Commands
that operate on catalogs take `--catalog <file>` **or** discover on the fly
through the root input option (`binfiddle -i <file-or-dir> nn ls …`).

### `nn capabilities`

Reports exactly which workbench capabilities this build implements — the honest
self-description of the tool. Unimplemented capabilities are listed as
unavailable, never advertised as supported.

| Option | Values | Default | Description |
|---|---|---|---|
| `--report-format` | `text`, `json` | `text` | Output format |

```bash
binfiddle nn capabilities
binfiddle nn capabilities --report-format json
```

### `nn discover`

Inventory supported model artifacts — descriptor-only, no payload reads.
Recognizes SafeTensors, GGUF v2/v3, ONNX (protobuf, descriptor tier),
PyTorch `.pth`/`.pt`/`.ckpt` (ZIP container + a data-only pickle opcode
reader: tensor names, shapes, dtypes, and exact storage spans without
executing a single opcode — the tier is read-only, and `nn edit` refuses
`torch.*` tensors), and NumPy captures `.npy`/`.npz` (member names,
dtypes, shapes, byte order, layout; STORED members get exact payload
spans, DEFLATE members report exact archive spans with logical
decompressed coordinates — never a fabricated file-byte address; object
arrays are never unpickled); scans package directories (unknown files stay visible,
classified as assets or opaque); GGUF split-shard groups
(`name-00001-of-00002.gguf`) get completeness and cross-shard uniqueness
checks, and sharded-SafeTensors packages get the same discipline from
`model.safetensors.index.json` (missing shards are visible findings and
fail `--require-complete`; index-vs-shard consistency and duplicate tensor
names across shards are checked). On ONNX: initializers with `raw_data` get
exact spans, external-data tensors stay visible with unresolved extents,
packed protobuf fields are reported as non-contiguous storage.

With `-i -` the input is read from stdin and spooled to a bounded private
temporary file whose digest becomes a content-verified identity.

| Option | Values | Default | Description |
|---|---|---|---|
| `--report-format` | `text`, `json` | `text` | Output format |
| `--verify-content` | flag | off | Hash full contents; upgrade identity strength to content-verified |
| `--require-complete` | flag | off | Fail with exit 8 when coverage is incomplete |
| `--out-catalog` | path | — | Save the catalog for later commands |

```bash
binfiddle -i model.safetensors nn discover
binfiddle -i model-dir/ nn discover --verify-content --out-catalog model.nn.json
binfiddle -i model-dir/ nn discover --require-complete
cat model.gguf | binfiddle -i - nn discover
```

Example (text):

```text
demo.safetensors: parsed
  format: safetensors 1
  validity: valid
  tensors: 4
  tensor model.layers.0.mlp.gate_proj.weight [6x4] 24 elements, encoding safetensors.F32
  ...
coverage: 1 of 1 sources parsed, 0 problematic, 0 symlinks skipped
```

### `nn ls`

List tensors, sources, or (with a pack) architecture components. Bounded
pagination keeps huge models comfortable.

| Option | Values | Default | Description |
|---|---|---|---|
| `--catalog` | path | — | Saved catalog (or use root `-i` to discover) |
| `--view` | `tensors`, `sources`, `architecture` | `tensors` | What to list (`architecture` needs `--pack`) |
| `--pack` | path | — | Model pack file or directory (`pack.yaml`) |
| `--encoding` | e.g. `safetensors.F32`, `ggml.q4_0` | — | Exact encoding filter |
| `--source` | id prefix or path | — | Scope to one source |
| `--name-regex` | bounded regex | — | Filter over tensor names |
| `--sort` | `name`, `bytes` | `name` | Order |
| `--limit` | 1–10000 | — | Max entries per page |
| `--offset` | count | `0` | Entries to skip |
| `--report-format` | `text`, `json` | `text` | Output format |

```bash
binfiddle nn ls --catalog model.nn.json --sort bytes --limit 20
binfiddle nn ls --catalog model.nn.json --encoding ggml.q4_0
binfiddle nn ls --catalog model.nn.json --name-regex 'gate|down' --sort bytes
binfiddle nn ls --catalog model.nn.json --view sources
binfiddle nn ls --catalog model.nn.json --view architecture --pack demo.pack.yaml
```

### `nn show`

Show one tensor's full record — by exact name, unique id prefix, or (with a
pack) component path. `--explain` adds the evidence statement behind every
field.

| Option | Description |
|---|---|
| `--catalog` | Saved catalog (or root `-i`) |
| `--pack` | Model pack (enables `--component`) |
| `--component` | Component path, e.g. `decoder.layers[3].attention` |
| `--tensor` | Exact original tensor name |
| `--id` | Tensor identifier (full or unique digest prefix) |
| `--source` | Scope for `--tensor`: unique source id prefix or exact path |
| `--explain` | Include the evidence explanation |
| `--report-format` | `text` or `json` |

```bash
binfiddle nn show --catalog model.nn.json --tensor w --explain
binfiddle nn show --catalog model.nn.json --id tensor:dbc045ef
binfiddle nn show --catalog model.nn.json --pack demo.pack.yaml \
    --component 'decoder.layers[0].mlp.gate'
```

### `nn select`

Resolve a tensor selection and optionally save it. Three resolution modes:
exact tensor name (optionally scoped to a source), tensor id, or a component
selector expression (requires a pack). `--rebind` re-evaluates a saved
selection's request against a different catalog and reports additions and
removals of resolved tensor identities — the original selection file is never
touched.

| Option | Description |
|---|---|
| `--catalog` | Saved catalog (or root `-i`) |
| `--tensor` | Exact original tensor name |
| `--id` | Tensor identifier (full or unique digest prefix) |
| `--select` | Component selector expression (requires `--pack`) |
| `--rebind` | Saved selection to rebind against `--catalog` |
| `--pack` | Model pack for component resolution |
| `--source` | Scope for `--tensor` |
| `--allow-empty` | Permit an empty selection instead of rejecting it |
| `--out-selection` | Save the resolved selection to this file |
| `--report-format` | `text` or `json` |

```bash
binfiddle nn select --catalog model.nn.json --tensor w --out-selection w.sel.json
binfiddle nn select --catalog model.nn.json --pack demo.pack.yaml \
    --select 'decoder.layers[*].mlp' --out-selection mlp.sel.json
binfiddle nn select --catalog model-v2.nn.json --rebind w.sel.json \
    --out-selection w-v2.sel.json
```

See [Component selector grammar](#component-selector-grammar) for the
expression syntax, including the `heads[N]` virtual family.

### `nn where`

Forward address mapping: a tensor, or one element coordinate, to its
file-qualified location. The answer carries a **precision classification** —
`exact_contiguous` (dense scalars), `exact_bits` (packed encodings, with bit
mask, shift, and numbering), `no_payload`, or `unresolved` — plus the full
**decode dependency** set: for Q4_0, an element's nibble *and* its block
scale.

| Option | Values | Default | Description |
|---|---|---|---|
| `--catalog` | path | — | Saved catalog (or root `-i`) |
| `--tensor` | name | — | Exact tensor name |
| `--id` | id | — | Tensor identifier (full or unique prefix) |
| `--source` | id/path | — | Scope for `--tensor` |
| `--index` | `i,j,…` | — | Element coordinate (comma-separated decimal) |
| `--space` | `file` | `file` | Address space (only file addresses exist) |
| `--report-format` | `text`, `json` | `text` | Output format |

```bash
binfiddle nn where --catalog model.nn.json --tensor w
binfiddle nn where --catalog model.nn.json --tensor w --index 1,1
# Q4_0: exact bit addressing plus the shared scale dependency
binfiddle nn where --catalog gguf.nn.json --tensor q4w --index 0,31
```

Q4_0 element answer (real output):

```text
q4w [0,31] (ggml.q4_0)
  precision: exact_bits
  file span: [177, 178)
  bits:      mask 0xf0 shift 4 (lsb0 within the byte)
  note:      the other nibble of this byte is a different logical element
  decode dependencies:
    [160, 162)
    [177, 178)
```

### `nn locate`

Reverse lookup: which tensors own a given file offset, with the owning
element coordinate and quantization-block role where applicable.

| Option | Description |
|---|---|
| `--catalog` | Saved catalog (or root `-i`) |
| `--offset` | File offset (decimal or `0x` hex) — required |
| `--report-format` | `text` or `json` |

```bash
binfiddle nn locate --catalog model.nn.json --offset 0x27c
# offset 636 (0x27c):
#   w: element [0,1] (payload [632, 648))
```

### `nn impact`

What does a byte span (or a saved edit plan) touch? For each owning tensor it
reports two **distinct** sets: *decode dependencies* (bytes you must read) and
the *numerical influence set* (elements whose values change if those bytes
change). In Q4_0 a scale byte influences all 32 elements of its block
(`shared_scale`); a code nibble influences exactly one element. Behavioral
consequences are never predicted.

| Option | Description |
|---|---|
| `--catalog` | Saved catalog — required |
| `--span` | File span `START..END` (decimal or `0x` hex) |
| `--offset` | Single file offset (decimal or `0x` hex) |
| `--plan` | Saved edit plan file |
| `--report-format` | `text` or `json` |

```bash
binfiddle nn impact --catalog model.nn.json --span 0x27c..0x280
binfiddle nn impact --catalog model.nn.json --offset 0x27c
binfiddle nn impact --catalog model.nn.json --plan w-edit.plan.json
```

### `nn slice`

Extract the tensors of a selection into a bundle. Plans are id-verified files:
applying re-verifies the catalog and full source digests — a changed source
aborts instead of extracting stale bytes. Reference bundles keep payloads in
their content-verified sources; materialized bundles copy exact spans and hash
every member. The `decode` policy materializes a numeric representation and
records the loss of the original encoding identity.

| Option | Values | Default | Description |
|---|---|---|---|
| `--catalog` | path | — | Saved catalog (or root `-i`) |
| `--selection` | path | — | Saved selection (plan-generation mode) |
| `--plan` | path | — | Saved plan (apply mode) |
| `--kind` | `weights` | `weights` | Slice kind (component/executable need packs) |
| `--storage` | `reference`, `materialized` | `materialized` | Storage policy |
| `--quant` | `preserve_encoding`, `cover_blocks`, `decode` | `preserve_encoding` | Quantization policy |
| `--dry-run` | flag | off | Resolve and print the plan without writing |
| `--save-plan` | path | — | Save the resolved plan |
| `--out-dir` | path | — | Bundle output directory (apply mode) |
| `--report-format` | `text`, `json` | `text` | Output format |

```bash
# Preview what a slice would do
binfiddle nn slice --catalog model.nn.json --selection w.sel.json --dry-run

# Plan now, apply later (both steps re-verify sources)
binfiddle nn slice --catalog model.nn.json --selection w.sel.json \
    --save-plan w-slice.plan.json
binfiddle nn slice --catalog model.nn.json --plan w-slice.plan.json \
    --out-dir slices/w/

# Quantization-aware variants
binfiddle nn slice --catalog model.nn.json --selection q.sel.json \
    --storage reference --quant preserve_encoding --out-dir slices/ref/
binfiddle nn slice --catalog model.nn.json --selection q.sel.json \
    --quant decode --out-dir slices/decoded/
```

### `nn assemble`

Reconstruct tensor content from a materialized bundle. Every member digest is
verified; the output carries the `tensor_content` guarantee (and only that
guarantee — original file bytes and executability are different claims).

| Option | Description |
|---|---|
| `--bundle` | Materialized bundle directory (contains `slice.json`) — required |
| `--out-dir` | Output directory for reconstructed payloads — required |
| `--report-format` | `text` or `json` |

```bash
binfiddle nn assemble --bundle slices/w --out-dir rebuilt/w
```

### `nn split`

One-command layer decomposition: per-layer child selections (synthesized,
rebindable selector expressions), per-layer reference plans or materialized
bundles via the slice machinery, and a root `split.json` with the coverage
partition (assigned / shared / unresolved) and member-vs-unique byte
accounting — so shared tensors are counted once, never double-billed.

| Option | Values | Default | Description |
|---|---|---|---|
| `--catalog` | path | — | Saved catalog — required |
| `--pack` | path | — | Model pack with layered components — required |
| `--by` | `layer` | `layer` | Decomposition axis |
| `--storage` | `reference`, `materialized` | `reference` | Plans only, or per-layer bundles |
| `--out-dir` | path | — | Fresh output directory for the split tree — required |
| `--report-format` | `text`, `json` | `text` | Output format |

```bash
binfiddle nn split --catalog model.nn.json --pack demo.pack.yaml \
    --storage materialized --out-dir split/
```

### `nn analyze`

Numerical inspection of one tensor — or every tensor of a selection — with
honest coverage. NumPy capture members analyze directly, including
DEFLATE-compressed `.npz` members (bounded whole-member decompression; the
scan states that extents resolved to exact decompressed coordinates). `metadata` reads no payload bytes; `sample` examines a
seeded deterministic selection; `full` scans everything within its budget.
Statistics: Welford mean/variance (population and sample named separately),
min/max with coordinates and tie counts, non-finite value category counts,
an overflow-safe L2 norm, optional histograms, optional reference-error
metrics, and quantization-block views. With `--selection`, one uniform mode
runs over every tensor of a saved selection in a single table with robust
cross-tensor outlier flags (median/MAD, |z| ≥ 4 → `STAT_OUTLIER` — a
statistical observation, never a defect verdict).

| Option | Values | Default | Description |
|---|---|---|---|
| `--catalog` | path | — | Saved catalog (or root `-i`) |
| `--tensor` | name | — | Exact tensor name |
| `--id` | id | — | Tensor identifier (full or unique prefix) |
| `--source` | id/path | — | Scope for `--tensor` |
| `--selection` | path | — | Analyze every tensor of a saved selection (batch mode; exclusive with `--tensor`/`--id`) |
| `--mode` | `metadata`, `sample`, `full` | `full` | Access mode |
| `--seed` | integer | `17` | Seed for deterministic sampling |
| `--sample-size` | elements | `10000` | Sample size (sample mode) |
| `--histogram-bins` | count | `0` | Histogram bins (0 = none) |
| `--blocks` | count | `0` | Leading quantization blocks to display (0 = none) |
| `--reference` | path | — | Reference values file (raw little-endian f32/f64) |
| `--reference-width` | `4`, `8` | `4` | Reference element width |
| `--report-format` | `text`, `json` | `text` | Output format |

```bash
binfiddle nn analyze --catalog model.nn.json --tensor w
binfiddle nn analyze --catalog model.nn.json --tensor w --mode sample \
    --seed 17 --sample-size 4096 --histogram-bins 32
binfiddle nn analyze --catalog model.nn.json --tensor w --mode metadata
binfiddle nn analyze --catalog model.nn.json --tensor w \
    --reference ref.f32 --reference-width 4
binfiddle nn analyze --catalog gguf.nn.json --tensor q4w --blocks 2

# Batch: every gate projection in one table, outliers flagged
binfiddle nn analyze --catalog model.nn.json --selection gates.sel.json \
    --mode sample --sample-size 2000
```

The reference comparison reports MAE / RMSE / maxAE / relative L2 with
explicit zero-denominator policies — a perfect match prints zeros, not silence:

```text
  reference: 4 pairs compared
    MAE:  0
    RMSE: 0
    maxAE: 0
    relative L2: 0
```

### `nn edit`

Transactional fixed-size edits. A **plan** records the exact write unit, the
observed bytes (preimage), and the computed replacement. **Apply** re-verifies
the catalog, the full source digest, and the preimage, then writes a fresh
output file — the original is never modified, `--out-model` must not exist.
Every byte outside the planned span is verified unchanged and the patched
container must reparse. Sub-byte writes (Q4_0 nibbles) preserve the
neighboring value by mask. **Undo** bundles reverse an edit against its exact
edited revision. All edit commands require content-verified discovery
(`nn discover --verify-content`).

> **Negative values:** pass them with `=` — `--value=-0.5` — because a
> bare `-0.5` would be parsed as a flag.
>
> **Raw bits are hex byte pairs:** `a4` is one byte; a single nibble for
> sub-byte units is padded — `--raw-bits 07`, not `7`.

#### `nn edit set`

| Option | Values | Default | Description |
|---|---|---|---|
| `--catalog` | path | — | Content-verified catalog |
| `--tensor` | name | — | Exact tensor name |
| `--id` | id | — | Tensor identifier |
| `--source` | id/path | — | Scope for `--tensor` |
| `--index` | `i,j,…` | — | Element coordinate — required |
| `--value` | number | — | Requested numeric value — required (with `--raw-bits`) |
| `--raw-bits` | hex | — | Requested raw bits (nibble for sub-byte units) |
| `--policy` | `auto`, `exact_only`, `nearest`, `fixed_parameters` | `auto` | Value policy |
| `--clamp` | flag | off | Allow saturating out-of-range quantized codes |
| `--save-plan` | path | — | Save the plan to this file |
| `--session` | dir | — | Stage the operation into a multi-write session instead |
| `--report-format` | `text`, `json` | `text` | Output format |

```bash
# Typed value change (policy auto: fixed_parameters for Q4_0)
binfiddle nn edit set --catalog model.nn.json --tensor w \
    --index 0,0 --value 9 --save-plan w-edit.plan.json

# A Q4_0 nibble edit: masked write, neighbor preserved, scale untouched
binfiddle nn edit set --catalog gguf.nn.json --tensor q4w \
    --index 0,3 --value=-0.5 --save-plan q4.plan.json
# → write: bytes [133, 134) masked 0x0f shift 0 (lsb0)
#   change: -4 -> -0.5 (requested value -0.5)
#   bytes:  00 -> 07

# Raw-bit edits for exact bit patterns — hex byte pairs, so pad a single
# nibble with a leading zero (07, not 7)
binfiddle nn edit set --catalog gguf.nn.json --tensor q4w \
    --index 0,3 --raw-bits 07 --save-plan q4raw.plan.json
```

#### Multi-write edit sessions

Many planned values, one copy pass. `edit set --session DIR` stages each
operation (preimage captured at staging, overlapping write units refused
at staging time, source digest re-verified between stagings); `edit apply
--session DIR` patches every staged span in a single pass over the source
and writes an undo bundle that reverses all of them atomically. One
session binds one catalog and one source file — stage a session per shard
or checkpoint.

```bash
for spec in "w 0,0 101" "w 3,3 202" "v 1,1 303"; do
    set -- $spec
    binfiddle nn edit set --catalog m.nn.json --tensor "$1" \
        --index "$2" --value "$3" --session sess/
done
binfiddle nn edit apply --catalog m.nn.json --session sess/ \
    --out-model edited.safetensors --undo-bundle undo/
binfiddle nn edit undo --bundle undo/ --target edited.safetensors \
    --out-model restored.safetensors   # byte-identical to the original
```

#### `nn edit apply`

| Option | Description |
|---|---|
| `--catalog` | Saved catalog (must match the plan) — required |
| `--plan` | Saved edit plan (exclusive with `--session`) |
| `--session` | Apply every staged operation of an edit session in one pass |
| `--out-model` | Fresh output file (must not exist) — required |
| `--undo-bundle` | Directory for the undo bundle |
| `--report-format` | `text` or `json` |

```bash
binfiddle nn edit apply --catalog model.nn.json --plan w-edit.plan.json \
    --out-model w-edited.safetensors --undo-bundle undo/w
# → written:   4 bytes
#   preserved: all bytes outside the planned span verified identical
#   reparsed:  container reparse valid, spans unchanged
```

#### `nn edit undo`

| Option | Description |
|---|---|
| `--bundle` | Undo bundle directory (from `edit apply`) — required |
| `--target` | The edited file to reverse — required |
| `--out-model` | Fresh output file (must not exist) — required |
| `--report-format` | `text` or `json` |

```bash
binfiddle nn edit undo --bundle undo/w --target w-edited.safetensors \
    --out-model w-restored.safetensors
# cmp model.safetensors w-restored.safetensors → identical
```

Undo binds to the exact edited revision: applying it to any other file fails
with `SOURCE_CHANGED` rather than patching wrong bytes.

#### `nn edit prune`

Structural recipe: remove intermediate MLP channels (gate/up rows plus down
columns) into a fresh SafeTensors file with updated shapes and digest-verified
untouched payloads. Channels must be sorted, unique, in range, and not all of
them.

| Option | Description |
|---|---|
| `--catalog` | Content-verified catalog — required |
| `--pack` | Pack with `mlp_gate`/`mlp_up`/`mlp_down` bindings — required |
| `--channels` | Sorted unique channel indices, comma-separated — required |
| `--out-model` | Fresh output SafeTensors file — required |
| `--report-format` | `text` or `json` |

```bash
binfiddle nn edit prune --catalog model.nn.json --pack demo.pack.yaml \
    --channels 0 --out-model pruned.safetensors
# → model.layers.0.mlp.up_proj.weight:   [6, 4] -> [5, 4]
#   model.layers.0.mlp.down_proj.weight: [4, 6] -> [4, 5]
#   untouched tensors: 1 (payloads digest-verified)
```

### `nn pack`

Model pack operations. A pack is a declarative YAML manifest mapping
tensor-name patterns to component roles; see
[Model pack authoring](#model-pack-authoring) for the file format.

#### `nn pack verify`

| Option | Description |
|---|---|
| `--pack` | Pack file or directory (`pack.yaml`) — required |
| `--report-format` | `text` or `json` |

```bash
binfiddle nn pack verify --pack demo.pack.yaml
# → pack verified: demo.mlp v1.0.0 (3 bindings, id pack:7a51f8e6…)
```

#### `nn pack lint`

Static lint — no model involved. Checks duplicate components, shape-expression
evaluation against the pack's own config, layer-schedule arity, near-no-literal
capture patterns, and more. Errors exit 7.

```bash
binfiddle nn pack lint --pack demo.pack.yaml
# → [warning] NO_LAYER_SCHEDULE: bindings reference layers[..] but neither
#   layer_types nor full_attention_interval is declared
```

#### `nn pack scaffold`

Derive a provisional pack draft from an observed catalog. Every suggestion is
heuristic-labeled; drafts lint cleanly but are starting points, never trusted
profiles. With `--config config.json`, known dimension keys from the model
configuration become parameters (heuristic-labeled), and any dimension
observed in three or more tensors becomes an inferred symbol — shape
expressions then reference symbols instead of literals. Output goes to
stdout — review, rename, and save it yourself.

```bash
binfiddle nn pack scaffold --catalog model.nn.json > draft.pack.yaml
binfiddle nn pack scaffold --catalog model.nn.json --config config.json \
    > draft.pack.yaml
```

### `nn diff`

Layered comparison of two catalogs. Layers are never confused: package
members (added/removed), descriptor changes (shape/encoding), encoded-content
equality by payload digest, and the **repack distinction** — identical bytes
at different offsets is a repack, not a content change. Missing tensors stay
visible as unmatched; a missing tensor is never a zero tensor.

`--precision` switches to a **precision audit**: for every name-matched,
same-shape tensor pair, both sides are decoded under their own declared
codecs (fp32 vs fp16 exports, quantized variants) and the table reports
per-tensor MAE / RMSE / maxAE / relative-L2 with finite-mismatch counts and
a worst-by-relative-L2 rollup. Error size is never a behavioral claim.

| Option | Values | Default | Description |
|---|---|---|---|
| `--left` | path | — | Left catalog — required |
| `--right` | path | — | Right catalog — required |
| `--decoded` | flag | off | Also compare decoded values of content-changed scalar tensors |
| `--policy` | `exact_bits`, `lenient` | `exact_bits` | Decoded comparison policy (NaN/signed-zero handling) |
| `--precision` | flag | off | Precision audit: per-tensor decoded error metrics |
| `--precision-sample` | elements | `0` | Sample stride for the audit (0 = every element) |
| `--report-format` | `text`, `json` | `text` | Output format |

```bash
binfiddle nn diff --left v1.nn.json --right v2.nn.json
binfiddle nn diff --left v1.nn.json --right v2.nn.json --decoded --policy exact_bits

# What did fp16 conversion cost, per tensor?
binfiddle nn diff --left fp32.nn.json --right fp16.nn.json --precision
```

Real output excerpt:

```text
layered diff
  package:
    - demo.safetensors
    + demo-v2.safetensors
  model.layers.0.mlp.gate_proj.weight: content changed
  model.layers.0.mlp.up_proj.weight: repacked (same bytes; offsets 440 vs 440)
  ...
  summary: 0 identical, 1 content-changed, 3 repacked, 0 descriptor changes, 0 unmatched, 2 member diffs
  claims: exact, layered; no lineage or behavior claims
```

### `nn fingerprint`

Exact content fingerprints for every tensor: a canonical digest over
name + shape + encoding + payload. Stable across re-discovery of the same
bytes; evidence, never lineage claims.

With `--compare`, an **experimental evidence graph** is added: exact-payload,
structural, and sampled-block similarity edges between the two catalogs, each
carrying method/version/threshold/score records. Sampled fingerprints digest
evenly spaced 64 KiB blocks — they find candidate relationships cheaply but
never certify unsampled bytes.

| Option | Values | Default | Description |
|---|---|---|---|
| `--catalog` | path | — | Catalog file — required |
| `--compare` | path | — | Second catalog: build the evidence graph |
| `--threshold` | 0.0–1.0 | `0.75` | Similarity threshold for `similar_under_mapping` edges |
| `--report-format` | `text`, `json` | `text` | Output format |

```bash
binfiddle nn fingerprint --catalog v1.nn.json
binfiddle nn fingerprint --catalog v1.nn.json --compare v2.nn.json --threshold 0.75
```

### `nn partition`

Plan contiguous layer groups per stage, balanced by encoded weight bytes —
a static estimate that states exactly what it includes (layer weight bytes)
and excludes (activations, workspace, state, transfers — all runtime
behavior). Unlayered tensors (embeddings, norms, heads) are reported, never
silently distributed.

| Option | Description |
|---|---|
| `--catalog` | Saved catalog — required |
| `--pack` | Model pack with layered components — required |
| `--stages` | Number of stages (default 2) |
| `--unlayered-policy` | `report` (default), `first`, `last`, or `manual` placement of unlayered tensors |
| `--placement-file` | JSON object of tensor name → stage (for `manual`) |
| `--report-format` | `text` or `json` |

```bash
binfiddle nn partition --catalog model.nn.json --pack demo.pack.yaml --stages 2
# → stage 0: layers [0] (3 tensors, 288 weight bytes)
#   unlayered: 1 tensors, 16 bytes (embeddings/norms/heads; never silently distributed)

# Place them explicitly instead of reporting only
binfiddle nn partition --catalog model.nn.json --pack demo.pack.yaml \
    --stages 4 --unlayered-policy manual --placement-file places.json
```

### `nn ledger`

Per-device memory accounting for a partition plan: takes the JSON report
of an `nn partition` run plus a supplied devices file, places stage *i*
on device *i % N*, and reports per-device fit verdicts. Capacity is
never assumed — each device carries supplied observations
(`nominal` / `nvml_total` / `cuda_visible` / `free_snapshot`) with
provenance (source, timestamp, ECC state, units; unknowns preserved),
and the verdict uses the **selected** observation only; a device with no
selected observation fails rather than guessing. Weight replication
multiplies encoded bytes, overhead line items are accounted separately
from weight bytes, and unlayered tensors stay visible. The per-rank rule
stands on its own: one device that does not fit is a failure even when
the nominal aggregate would; `--reject-on-aggregate` adds the aggregate
check on top. Any device that does not fit exits 7.

| Option | Description |
|---|---|
| `--stages-json` | `nn partition --report-format json` output — required |
| `--devices` | Devices file: `[{name, uuid?, selected?, observations: [{kind, bytes, source?, observed_at?, ecc?, units?}]}]` — required |
| `--replication` | Weight replication per hosting device (default 1) |
| `--overheads` | Runtime overhead line items `[{device, bytes, source?}]` — supplied separately from weight bytes |
| `--reject-on-aggregate` | Also evaluate the nominal-aggregate check (per-rank verdicts stand regardless) |
| `--report-format` | `text` or `json` |

```bash
binfiddle nn ledger --stages-json plan.json --devices devices.json --replication 2
# → gpu-0: stages [0] encoded 640000000000 x2 replication, overheads 0,
#   selected nvml_total (48305799168) — DOES NOT FIT
#   reason: total 640000000000 exceeds selected nvml_total capacity 48305799168
# → gpu-1: stages [1] encoded 480000000000 x2 replication, …
# → unplaced: 4 tensors, 16000000000 bytes (never silently distributed)
# exit 7: one or more devices exceed their selected capacity
```

### `nn carve`

Scan a raw file for embedded model containers (SafeTensors/GGUF), validate
candidates structurally, and report spans with confidence labels (`verified`
spans parse structurally — nothing asserts model validity or recoverability).

| Option | Description |
|---|---|
| `--target` | Raw file to scan — required |
| `--report-format` | `text` or `json` |

```bash
binfiddle nn carve --target disk.img
# → safetensors at [64, 712) [verified] 4 tensors — header 336 bytes, validity valid
```

### `nn validate`

Structural artifact validation with precise per-source verdicts:
`structurally_valid_for_reader`, `unsupported_feature`, `invalid`,
`incomplete`. Error-severity findings demote a parsed source to invalid;
`behavior_not_evaluated` is always stated. Exit 7 when anything fails.

| Option | Description |
|---|---|
| `--catalog` | Saved catalog (or root `-i`) |
| `--report-format` | `text` or `json` |

```bash
binfiddle -i model-dir/ nn validate
# → ./demo.safetensors: structurally_valid_for_reader
#   note: all structural checks passed for this reader
```

### `nn shard-map`

Static download planning from a sharded-SafeTensors index: which shard
files hold the tensors you care about, with per-shard counts and payload
bytes, planned-versus-declared coverage, and any requested names the index
does not know. Works from explicit names, a saved selection (resolved
through a catalog for byte accounting), or the whole package. **A plan,
never a download** — there is no network access anywhere in binfiddle.

| Option | Description |
|---|---|
| `--index` | Path to `model.safetensors.index.json` — required |
| `--catalog` | Saved catalog (resolves selections; adds byte accounting) |
| `--selection` | Saved selection file naming the tensors of interest (needs `--catalog`) |
| `--tensor` | Exact tensor names to plan for (repeatable; omit for everything) |
| `--report-format` | `text` or `json` |

```bash
binfiddle nn shard-map --index model.safetensors.index.json \
    --tensor model.layers.3.self_attn.q_norm.weight
# → model-00082-of-00131.safetensors: 1 tensors, …

# A selection with byte accounting against a partial catalog
binfiddle nn shard-map --index model.safetensors.index.json \
    --catalog q.nn.json --selection experts.sel.json
# → planned payload 3355443200 of 359999963128 declared bytes (0.9%)
```

### `nn derive`

Data-only derived numerical views over catalog tensors (typically NumPy
capture members): a closed expression language (`+ - * /`, unary `-`,
and `log`/`exp`/`sqrt`/`abs`) evaluated in float64 over arrays decoded
under their declared dtypes. Nothing executes; the output is a fresh
`.npy` plus a canonical provenance sidecar recording the expression,
every input's tensor id/encoding/shape, evaluation and output dtypes,
the output's sha256, and domain-violation/NaN counts. Domain violations
(log of a non-positive value, sqrt of a negative, division by zero)
refuse by default with exact counts; `--allow-domain-violations` writes
NaN at those positions and records the count. Derived values are
labeled derived — never observed device values.

| Option | Description |
|---|---|
| `--catalog` | Saved catalog naming the input tensors — required |
| `--expression` | Expression over tensor names (identifiers must match catalog tensor names; a bare `.npy` catalogs its array as `array`) — required |
| `--out-npy` | Fresh output .npy file — required |
| `--out-sidecar` | Fresh provenance JSON — required |
| `--f32-output` | Write float32 instead of float64 |
| `--allow-domain-violations` | Write NaN at violations instead of refusing |
| `--report-format` | `text` or `json` |

```bash
# Observed minus expected, from a capture archive
binfiddle nn derive --catalog mhcpre.nn.json \
    --expression 'post - expected_post' \
    --out-npy error.npy --out-sidecar error.json

# The consumer-inverse diagnostic transform (refuses outside (0, 2))
binfiddle nn derive --catalog p.nn.json \
    --expression 'log(p) - log(2 - p)' \
    --out-npy logit.npy --out-sidecar logit.json
```

### `nn exl3`

Decode EXL3 trellis storage — the quantization scheme behind EXL3 model
releases — into Hadamard-domain values, statically and exactly. The four
fields of an EXL3 projection (`.trellis`, `.suh`, `.svh`, `.mcg`) are
resolved from a saved catalog by trellis name; the selector must carry
the mcg codebook magic or the variant is refused as not decodable by
this codec. Each weight's code is the 16-bit window ending at its last
code bit inside the 16×16 tile (256 times the bitrate in bits). Codes
are packed MSB-first in little-endian u32 words and use a tensor-core
lane permutation. Supported variants use the mcg codebook at integer
bitrates 1–8; codebook addition rounds to fp16. In `[output,input]`
coordinates, logical reconstruction is
`W = diag(svh)·H·Wq·H·diag(suh)` with normalized 128-point Sylvester
Hadamard blocks. Reconstruction uses f32 arithmetic and does not claim
GPU intermediate-rounding parity. An independent CPU reference and
real-weight checks are included in the [GLM lab](NN_DEEPDIVE_GLM_LAB.md).
Claims stay storage-level: dequantized values, never model quality or behavior.
JSON report values and statistics are decimal strings, consistent with
the number-free wire format. Current queries load the whole projection's
trellis field even when returning one value or block.

#### `nn exl3 decode`

One Hadamard-domain storage value `Wq[n, k]`, with its decode
dependency stated (the 16-bit window inside its tile).

| Option | Description |
|---|---|
| `--catalog` | Saved catalog containing the four EXL3 fields — required |
| `--trellis` | Trellis tensor name (siblings `.suh`/`.svh`/`.mcg` resolved by name) — required |
| `--index` | Element coordinate `n,k` (comma-separated decimal) — required |
| `--report-format` | `text` or `json` |

#### `nn exl3 block`

Summary statistics over one logical 128×128 block of `W` (origin
coordinates must be multiples of 128).

| Option | Description |
|---|---|
| `--catalog` | Saved catalog containing the four EXL3 fields — required |
| `--trellis` | Trellis tensor name — required |
| `--origin` | Block origin `n0,k0` (comma-separated, multiples of 128) — required |
| `--report-format` | `text` or `json` |

```bash
binfiddle nn exl3 decode --catalog expert.nn.json \
    --trellis model.language_model.layers.12.mlp.experts.27.up_proj.trellis --index 3,5
# → logical: Wq[3, 5] (Hadamard domain, projection [2048, 4096], 4 bpw)
#   value:   0.54052734
#   decode dependencies: the 16-bit window ending at the weight's code, inside its 16x16 tile

binfiddle nn exl3 block --catalog expert.nn.json \
    --trellis model.language_model.layers.12.mlp.experts.27.up_proj.trellis --origin 0,0
# → mean: -0.000069   variance: 0.00035795   abs max: 0.080924
#   claims: f32 logical weights under diag(svh) . H . Wq . H . diag(suh); …
```

### `nn evidence`

Reviewed evidence sidecars: bind reviewed human knowledge — an operation
description, an observation timeline, capture notes — to a binfiddle
artifact's content identity. The sidecar is a canonical strict-JSON
document (`binfiddle.nn.evidence-sidecar/v1`) recording the subject
(kind: `file`, `catalog`, `selection`, or `bundle` — for bundles, bind
to the manifest file: `slice.json` / `split.json` / `undo.json`), its
computed identity at review time (sha256 / catalog_id / selection_id),
a role, the reviewer, an arbitrary reviewed payload, and a claims line.
`nn evidence verify` recomputes the artifact's identity and reports
agreement — exit 7 on mismatch (the artifact changed after review, or
the sidecar names another artifact). Sidecars are data: nothing in them
executes, and they establish no runtime behavior.

#### `nn evidence verify`

| Option | Description |
|---|---|
| `--sidecar` | Sidecar JSON file — required |
| `--report-format` | `text` or `json` |

#### `nn evidence init`

| Option | Description |
|---|---|
| `--kind` | `file`, `catalog`, `selection`, or `bundle` — required |
| `--subject` | Subject artifact path — required |
| `--role` | Sidecar role (e.g. `operation-description`, `observation-timeline`) — required |

```bash
# Draft a timeline sidecar over a capture archive, identity pre-computed
binfiddle nn evidence init --kind file --subject mhc-pre-4.npz \
    --role observation-timeline > tl.json

# Fill the payload, set reviewed_by and claims, then verify the binding
binfiddle nn evidence verify --sidecar tl.json
# → binding: identity matches the subject artifact

# If the artifact changes after review, verification fails (exit 7)
binfiddle nn evidence verify --sidecar tl.json
# → binding: IDENTITY MISMATCH — the artifact changed after review…
```

**Two worked roles.** An `operation-description` sidecar carries a
reviewed table of producer/consumer, storage dtype, arithmetic dtype,
and cast/materialization boundaries — marking each annotation as
observed, source-derived, modeled, or unknown, and never implying
numerical equivalence. An `observation-timeline` sidecar carries
ordered events (before-call snapshots, after-call copies, earlier cache
transfers) with their evidence, keeping coordinate namespaces explicit
(model-call indices vs replay serials are never interchangeable) and
never presenting a saved tensor as an instruction-time observation.

### `nn adapter`

#### `nn adapter inspect`

Inventories LoRA-style factor pairs in an adapter checkpoint: targets with
ranks and dimensions, orphan factors, rank mismatches, and extra adapter
tensors — descriptor-level claims only.

| Option | Description |
|---|---|
| `--catalog` | Catalog of the adapter checkpoint — required |
| `--report-format` | `text` or `json` |

```bash
binfiddle -i adapter.safetensors nn discover --verify-content --out-catalog adapter.nn.json
binfiddle nn adapter inspect --catalog adapter.nn.json
# → base.model.layers.0.attn.q rank 4 [8 -> 8]
#   claims: descriptor-level factor inspection; merge arithmetic and
#   base-model compatibility are NOT verified
```

### `nn tokenizer`

#### `nn tokenizer inspect`

Classifies standard tokenizer asset files in a package directory and
summarizes `tokenizer.json` structure. Does not evaluate tokenization
behavior or render templates.

| Option | Description |
|---|---|
| `--package` | Package directory — required |
| `--report-format` | `text` or `json` |

```bash
binfiddle nn tokenizer inspect --package model-dir/
# → tokenizer.json [tokenizer_json] 291 bytes
#   tokenizer.json: model WordLevel, vocab 10, merges -, added tokens 0
```

#### `nn tokenizer diff`

Compares two `tokenizer.json` files at the vocabulary level with added and
removed token lists. Vocabulary differences never imply behavioral ones.

| Option | Description |
|---|---|
| `--left` | Left `tokenizer.json` — required |
| `--right` | Right `tokenizer.json` — required |
| `--report-format` | `text` or `json` |

```bash
binfiddle nn tokenizer diff --left v1/tokenizer.json --right v2/tokenizer.json
# → vocab: 10 vs 12 (0 removed, 2 added)
#   + t10
#   + t11
```

### `nn research`

Experimental capabilities. Results state their own limits.

#### `nn research align`

Decides **exactly** whether one same-shape dense weight is a row permutation
of another, via per-row digest multisets. Recovers the mapping and counts
duplicate-row ambiguity instead of guessing. Computational equivalence is
never claimed.

| Option | Description |
|---|---|
| `--left` | Left catalog — required |
| `--right` | Right catalog — required |
| `--report-format` | `text` or `json` |

```bash
binfiddle nn research align --left v1.nn.json --right v2.nn.json
# → model.layers.0.mlp.gate_proj.weight: permutation
#   (6 rows moved, 0 fixed, 0 ambiguous rows)
```

## Component selector grammar

Component selectors resolve through a model pack into tensor sets.

```
decoder.layers[3].attention            single index
decoder.layers[8:16].attention         half-open range [8,16)
decoder.layers[1,3,5].mlp              explicit list (unique indices)
decoder.layers[*].mlp                  wildcard (whole family)
decoder.layers                         shorter path → whole subtree
decoder.layers[3].attention.query_gate.heads[1]   virtual head family
decoder.layers[19].mlp.experts.gate_up.experts[17]   virtual expert family
```

- Indices are non-negative decimals; ranges are `[start:end)` with `end > start`;
  list entries must be unique; reserved words are rejected.
- `heads[N]` exists only on query/gate (fused projection) components and
  selects the exact stored rows of one head. Slicing such a selection extracts
  those bytes with a `logical_view` statement in the bundle manifest.
- `experts[N]` exists only on `moe_expert_stack` components (`[E, A, B]`
  stacked MoE storage, fused gate_up and down-projection stacks alike) and
  selects one expert's contiguous `[A, B]` slab with an exact byte span.
  Single indices only; the two virtual families cannot combine.
- Out-of-range indices, empty selections, and family/kind mismatches are
  errors — never silent guesses.

## Exit codes and error codes

`nn` commands use their own exit codes (classic commands keep theirs):

| Exit | Error codes | Meaning |
|---|---|---|
| 0 | — | Success |
| 2 | `INVALID_REQUEST` | Malformed command arguments |
| 3 | `FORMAT_UNSUPPORTED`, `CODEC_UNSUPPORTED`, `AMBIGUOUS_BINDING`, `BOUNDARY_UNRESOLVED`, `INVERSE_UNQUALIFIED` | Unsupported format/codec, ambiguous resolution, or an edit the encoding cannot represent exactly |
| 4 | `MALFORMED_INPUT`, `WIRE_SYNTAX` | Malformed artifact or wire data (including tampered id-verified files) |
| 5 | `SOURCE_CHANGED`, `SOURCE_MISSING`, `WRITE_CONFLICT` | Precondition failure: the source changed since planning, is missing, or conflicts |
| 6 | `IO`, `BUDGET_EXCEEDED`, `PUBLICATION_INCOMPLETE` | I/O failure, resource budget exhaustion, unresolved output publication |
| 7 | `VALIDATION_FAILED` | Validation defects (`nn validate` findings, `nn pack lint` errors) |
| 8 | `INCOMPLETE_REJECTED` | `--require-complete` rejected incomplete coverage |
| 130 | `CANCELLED` | Cooperative cancellation (terminal signal) |

## The JSON result envelope

Every command emits the same envelope shape under `--report-format json`:

```json
{
  "schema": "binfiddle.nn.result/v1",
  "operation": "discover",
  "status": "complete",
  "coverage": { "complete": true, "notes": [] },
  "diagnostics": [],
  "execution": { "publication": "not_applicable" },
  "semantic": { "…": "the operation's payload" }
}
```

- Integers are **decimal strings** (`"payload_start": "632"`) — a
  number-free JSON subset that keeps huge models exact across every parser.
- The JSON is canonically serialized (RFC 8785 ordering), so identical runs
  produce byte-identical output — safe to diff, hash, or archive.
- `diagnostics` carries structured findings; `execution.publication` records
  output status for commands that write files.

Consume it with any JSON tool:

```bash
binfiddle nn ls --catalog model.nn.json --report-format json | jq -r '.semantic.tensors[].name'
```

## Model pack authoring

A pack is a versioned YAML file: configuration parameters, tensor-name
patterns with captures, and bindings that map matched tensors to components
with kinds and expected shapes written as integer expressions over the
configuration. Packs are pure data — no executable content, no imports.

```yaml
schema: binfiddle.nn.pack/v1
id: demo.mlp
version: "1.0.0"
config:
  hidden_size: 4
  intermediate_size: 6
bindings:
  - pattern: "model.layers.{layer}.mlp.gate_proj.weight"
    component: "decoder.layers[{layer}].mlp.gate"
    kind: mlp_gate
    shape: ["intermediate_size", "hidden_size"]
  - pattern: "model.layers.{layer}.mlp.up_proj.weight"
    component: "decoder.layers[{layer}].mlp.up"
    kind: mlp_up
    shape: ["intermediate_size", "hidden_size"]
  - pattern: "model.layers.{layer}.mlp.down_proj.weight"
    component: "decoder.layers[{layer}].mlp.down"
    kind: mlp_down
    shape: ["hidden_size", "intermediate_size"]
```

Authoring workflow:

```bash
# 1. Scaffold a heuristic draft from an observed catalog
binfiddle nn pack scaffold --catalog model.nn.json > draft.pack.yaml

# 2. Review every pattern/kind/shape by hand, then lint
binfiddle nn pack lint --pack draft.pack.yaml

# 3. Verify and use
binfiddle nn pack verify --pack draft.pack.yaml
binfiddle nn ls --catalog model.nn.json --view architecture --pack draft.pack.yaml
```

Recognition keeps contradictions visible: a tensor whose name matched but
whose shape disagreed is reported as a finding, not hidden. Bindings may be
scoped by declared attention kind (`layer_kinds:`) for architectures whose
shapes differ per layer kind. Component kinds understood by recipes and
views include `mlp_gate`, `mlp_up`, `mlp_down`, `moe_expert_stack` (with
the `experts[…]` family), attention projections (including fused
`query_gate` with the `heads[…]` family), grouped linear-attention
projections, and zero-centered normalization.

## Howtos

### First contact with an unknown model directory

```bash
binfiddle -i model-dir/ nn discover --verify-content --out-catalog m.nn.json
binfiddle -i model-dir/ nn validate
binfiddle nn ls --catalog m.nn.json --sort bytes --limit 25
binfiddle nn capabilities
```

`discover` shows every file — model containers, configuration assets, opaque
blobs — with per-source verdicts; `validate` gives the precise structural
verdicts; `ls` shows the inventory biggest-first.

### Verify a download

```bash
binfiddle -i model.gguf nn discover --verify-content --require-complete
binfiddle -i model.gguf nn discover --verify-content --out-catalog m.nn.json
binfiddle nn fingerprint --catalog m.nn.json
```

`--require-complete` fails (exit 8) if any source was skipped; fingerprints
give you per-tensor identity records to keep or compare later.

### Extract one component family, then rebuild it

```bash
binfiddle nn select --catalog m.nn.json --pack pack.yaml \
    --select 'decoder.layers[8:16].attention' --out-selection attn.sel.json
binfiddle nn slice --catalog m.nn.json --selection attn.sel.json \
    --out-dir slices/attn/
binfiddle nn assemble --bundle slices/attn --out-dir rebuilt/attn/
```

### Safely edit one weight value

```bash
binfiddle -i model.safetensors nn discover --verify-content --out-catalog m.nn.json
binfiddle nn edit set --catalog m.nn.json --tensor w --index 0,0 --value 9 \
    --save-plan edit.plan.json
binfiddle nn edit apply --catalog m.nn.json --plan edit.plan.json \
    --out-model edited.safetensors --undo-bundle undo/edit
binfiddle nn edit undo --bundle undo/edit --target edited.safetensors \
    --out-model restored.safetensors
```

The original is never modified; the undo restores it byte-identically.

### Compare two checkpoints

```bash
binfiddle -i v1/ nn discover --verify-content --out-catalog v1.nn.json
binfiddle -i v2/ nn discover --verify-content --out-catalog v2.nn.json
binfiddle nn diff --left v1.nn.json --right v2.nn.json
binfiddle nn fingerprint --catalog v1.nn.json --compare v2.nn.json
```

The diff separates content changes from repacks; the evidence graph adds
exact-payload and similarity edges — none of it claims lineage.

### Recover embedded models from a dump

```bash
binfiddle nn carve --target image.bin
binfiddle -i image.bin read 64..712 --format raw > carved.safetensors   # extract a span
binfiddle -i carved.safetensors nn discover
```

## Boundaries

Stated plainly, once, so no command has to whisper it:

- **No execution.** The workbench never runs model code; runtime replay,
  capture, and behavior verification are outside its scope by design.
- **No behavior claims.** `impact` explains byte dependencies, not model
  quality; `partition` estimates bytes, not speedups.
- **No lineage claims.** Fingerprints and evidence graphs record byte-level
  relationships; direction, chronology, and provenance are never inferred.
- **No template rendering.** Tokenizer assets are inspected and compared,
  never executed.
- **Descriptor-tier ONNX.** Graph structure and initializer storage are
  inventoried honestly (including external data and packed fields); nothing
  executes.
- **Budgets are limits, not promises.** Every operation runs under bounded
  memory/IO/output budgets; nothing here is a performance guarantee.

## Version highlights — v0.29

| Feature | Command | What it does |
|---|---|---|
| PyTorch descriptor tier | `nn discover` (automatic) | `.pth`/`.pt`/`.ckpt` files inventory through a data-only pickle opcode interpreter (548 tensors out of the real Kokoro checkpoint); `torch.*` encodings with exact storage spans; edits are refused (read-only v1) |
| Multi-write edit sessions | `nn edit set --session DIR` / `nn edit apply --session DIR` | Stage many operations (staging-time conflict and digest checks), apply them all in one copy pass, undo them atomically |
| Stacked-expert views | `experts[N]` selector | One expert of an `[E, A, B]` MoE stack as a contiguous slab with an exact byte span (works on real 512-expert gate_up stacks) |
| Shard download planning | `nn shard-map --index model.safetensors.index.json` (+ `--catalog`/`--selection`/`--tensor`) | Which shards hold what you selected, with byte accounting — a plan, never a download |
| Batch analysis | `nn analyze --selection s.json` | One uniform mode over every tensor of a selection with robust outlier flags (STAT_OUTLIER; statistical, not a defect verdict) |
| Precision audit | `nn diff --left a --right b --precision` (+ `--precision-sample N`) | Per-tensor decoded MAE/RMSE/maxAE/relative-L2 under each side's own codec (fp32 vs fp16 exports, quantized variants) |
| Config-aware scaffolding | `nn pack scaffold --catalog c --config config.json` | Dimension keys from the config become parameters; dims seen in 3+ tensors become inferred symbols; shapes reference symbols |
| Sharded-package findings | `nn discover` (automatic) | Missing SafeTensors shards (per the index) are visible findings and fail `--require-complete`; index-vs-shard consistency and duplicate-name checks; deduplicated reporting |
| Partition placement | `nn partition --unlayered-policy report` or `first` or `last` or `manual` | Unlayered tensors (embeddings/norms/heads) can be placed explicitly; the default still never silently distributes them |
