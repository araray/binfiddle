# Deep Dive: Dissecting Qwen3.8-Flash-Next with the `nn` Workbench

Third real-model walkthrough ([Kokoro](NN_DEEPDIVE_KOKORO.md) — ONNX and the
pickle boundary; [Gemma-4 E4B](NN_DEEPDIVE_GEMMA.md) — 16 GB SafeTensors and
model packs). This one is the homecoming: **Qwen/Qwen3.8-Flash-Next** is a
Qwen3-Next-family architecture — the lineage the workbench's most specialized
machinery (fused query-gate head views, grouped linear-attention projections,
zero-centered norms, interval schedules) was designed around. It is also a
**131-shard, 360 GB release** — the sharded-package discipline at full scale.

Every output below is a real capture from the actual artifacts.

## The subject

| Artifact | Role | Size |
|---|---|---|
| `model-*-of-00131.safetensors` | The weights, sharded 131 ways (BF16) | 360 GB total |
| `model.safetensors.index.json` | The shard index: tensor → shard map | 170 KB |
| `config.json` | Architecture (`qwen4_exp`) | 4.7 KB |
| `tokenizer.json` | BPE vocabulary | 12.8 MB |

From `config.json` before touching a payload byte: 48 decoder layers on a
**4:1 linear/full hybrid** (`full_attention_interval: 4`), hidden 2560, 24
attention heads of 256, 2 KV heads, GatedDeltaNet-style linear attention
(16 key heads, 48 value heads, 128-wide), a **512-expert MoE** with fused
`gate_up` stacks, hyper-connections, an n-gram indexer, and an MTP layer.

Nobody needs 360 GB on disk to dissect that honestly: the index maps every
tensor to its shard, so this walkthrough works from a **curated 9-shard
subset (19.3 GB)** — the visual tower, one complete linear-attention layer,
one complete full-attention layer with its MoE block, the embeddings, and
the output head. `nn shard-map` plans exactly such a subset; Part 1 shows
what the engine says about it.

## Part 1 — A partial package must say so

The split-GGUF machinery verifies shard-group completeness. Sharded
SafeTensors packages carry the same information in
`model.safetensors.index.json`, and it is honored the same way: a directory
with 9 of 131 shards never discovers "cleanly" as if silently reduced.

```bash
binfiddle -i Qwen3.8-Flash-Next/ nn discover | grep -E "sharded|coverage"
```

```text
coverage: 9 of 9 sources parsed, 0 problematic, 0 symlinks skipped
sharded-package note: model.safetensors.index.json declares shards absent here (122 of them); coverage is incomplete
```

Every present shard carries the finding:

```text
note: [SAFETENSORS_SHARD_INDEX_INCOMPLETE] model.safetensors.index.json declares
131 shards; 122 absent from this directory (model-00002-of-00131.safetensors, …)
```

And the finding has teeth — `--require-complete` rejects partial downloads
with exit 8 instead of passing vacuously.

Two more behaviors this package exercises. Real indexes carry
`"total_size": 359999963128.0` — a **fractional JSON number** — which the
foreign parser reads into a dedicated `Float` value that **cannot enter the
wire subset** (canonicalization refuses it exactly like integers; the
number-free discipline is unchanged). And `nn validate --catalog` freshly
inventories **the sources the catalog records** (paths resolved against the
catalog's own directory), not the catalog file itself:

```bash
binfiddle nn validate --catalog q.nn.json | head -3
```

```text
artifact validation
  …/model-00001-of-00131.safetensors: structurally_valid_for_reader
    note: all structural checks passed for this reader
```

## Part 2 — The hybrid anatomy, read statically

The MoE stacks dominate the inventory — and they are exactly what the config
predicted, fused per expert:

```bash
binfiddle nn ls --catalog q.nn.json --sort bytes --limit 4
```

```text
1  …  [512x1280x2560] 1677721600 elements, 3355443200 bytes   (experts.gate_up_proj)
2  …  [512x1280x2560]                                 3.3 GB   (experts.gate_up_proj)
3  …  [512x2560x640]                                  1.7 GB   (experts.down_proj)
```

512 experts × 1280 rows — 1280 = 2 × 640: gate and up **pre-fused** inside
each expert slot. A whole MoE layer's experts in two tensors.

The linear-attention stack decodes the same way, shape by shape:

| Tensor (layer 2) | Shape | Reading |
|---|---|---|
| `linear_attn.in_proj_qkv.weight` | `[10240, 2560]` | 16·128 q + 16·128 k + 48·128 v |
| `linear_attn.in_proj_z.weight` | `[6144, 2560]` | 48·128 z gate |
| `linear_attn.in_proj_a/b.weight` | `[48, 2560]` | per-value-head gates |
| `linear_attn.A_log` / `dt_bias` | `[48]` | delta-rule decay state |
| `linear_attn.norm.weight` | `[128]` | per-head-dim norm |

Statistics make the internals visible without executing anything — the
delta-rule decay rates (`A_log`: mean 1.53, spread `[-3.58, 5.06]` —
`A = -exp(A_log)`), the router (`mlp.gate.weight`: near-zero-centered, tight),
attention projections (variance ~3e-4). And one genuinely nice find: **the
model stores two norm conventions side by side** —

```text
analyze …linear_attn.norm.weight:   mean 0.9668, min 0.875,  max 1.023   (direct weights)
analyze …self_attn.q_norm.weight:   mean 0.2833, min -0.346, max 0.594   (zero-centered: effective = 1 + stored)
```

## Part 3 — The pack: the reference architecture, live

The pack speaks the corpus's native vocabulary — no explicit layer list at
all, just the interval, exactly as the config declares it:

```yaml
config:
  num_attention_heads: 24
  head_dim: 256
  linear_num_key_heads: 16
  linear_num_value_heads: 48
  …
num_layers: 48
full_attention_interval: 4
bindings:
  - pattern: "model.language_model.layers.{layer}.self_attn.q_proj.weight"
    component: "decoder.layers[{layer}].attention.query_gate"
    kind: query_gate
    shape: ["num_attention_heads * head_dim * 2", "hidden_size"]
    layer_kinds: [full_attention]
  - pattern: "model.language_model.layers.{layer}.self_attn.q_norm.weight"
    component: "decoder.layers[{layer}].attention.q_norm"
    kind: norm_zero_centered
    shape: ["head_dim"]
    layer_kinds: [full_attention]
  - pattern: "model.language_model.layers.{layer}.linear_attn.in_proj_qkv.weight"
    component: "decoder.layers[{layer}].linear_attn.qkv"
    kind: dense
    shape: ["linear_num_key_heads * linear_key_head_dim * 2 + linear_num_value_heads * linear_value_head_dim", "hidden_size"]
    layer_kinds: [linear_attention]
  …
```

The schedule falls out of the interval fallback:

```text
architecture view (pack qwen4exp.flash)
  full-attention layers: [3, 7, 11, 15, 19, 23, 27, 31, 35, 39, 43, 47]
```

— which matches `config.json` exactly, without being told a single layer
index. The `layer_kinds:` scoping carries the hybrid's two different tensor
*sets* per layer kind.

### The fused query-gate, carved by head

The full-attention `q_proj` is `[12288, 2560]` — `2·H·D`, the fused
query/gate layout with per-head interleaving that the workbench's head-view
machinery was built to address. Selecting **one head** of the real tensor:

```bash
binfiddle nn select --catalog q.nn.json --pack qwen4exp.pack.yaml \
    --select 'decoder.layers[3].attention.query_gate.heads[5]' \
    --out-selection h5.sel.json
binfiddle nn slice --catalog q.nn.json --selection h5.sel.json --out-dir slices-h5
```

```text
  model.language_model.layers.3.self_attn.q_proj.weight
    [1753857008..1756478448] 2621440 bytes (encoded bytes copied verbatim)
  guarantee: tensor_content (original bytes and executability are NOT claimed)
```

2,621,440 bytes = 2·256·2560·2 — exactly the q and gate rows of head 5,
extracted from a 12288-row fused matrix inside a 1.9 GB shard, with a
logical-view statement in the bundle manifest and a digest-verified
round-trip through `nn assemble`. The B-series vectors from the
specification, running against a live model.

## Part 4 — Editing through a 131-shard package

A transactional edit planned against the **package** catalog
(content-verified: all 19.3 present GB hashed) and applied to a fresh copy
of the affected shard:

```bash
binfiddle -i . nn discover --verify-content --out-catalog qv.nn.json
binfiddle nn edit set --catalog qv.nn.json \
    --tensor model.language_model.layers.3.self_attn.q_norm.weight \
    --index 7 --value 0.5 --save-plan qn.plan.json
binfiddle nn edit apply --catalog qv.nn.json --plan qn.plan.json \
    --out-model edited-shard.safetensors
```

The plan records the exact change in the zero-centered norm's stored space
(`0.203125 -> 0.5`, bytes `503e -> 003f` at offset 1,740,749,310 of the
shard), the apply verifies every other byte is identical, and the layered
diff confirms the blast radius:

```text
  model.language_model.layers.3.self_attn.q_norm.weight: content changed
  summary: 0 identical, 1 content-changed, 32 repacked, 0 descriptor changes, 425 unmatched, 10 member diffs
```

Read the last two numbers with care — they are the honesty budget at work.
The edited output is a **single shard**, compared here against the
**nine-shard package catalog**: the 32 tensors that live in that shard
repacked (same bytes, new container), the one planned tensor changed, and
the 425 tensors that live on the other eight shards are **unmatched** —
visible as absent, never silently treated as zeros or dropped from the
account. A missing tensor is a missing tensor.

## Engine features this walkthrough exercises

1. **Sharded-SafeTensors index awareness.** `model.safetensors.index.json`
   is honored like split-GGUF groups: absent referenced shards produce
   `SAFETENSORS_SHARD_INDEX_INCOMPLETE` findings on present shards, appear
   in the report note and envelope coverage, and make `--require-complete`
   fail (exit 8).
2. **Foreign JSON floats.** Index `total_size` values like
   `359999963128.0` parse into a `Float` value that canonicalization
   still refuses — the wire subset remains number-free.
3. **`nn validate --catalog` inventories the recorded sources** — freshly,
   with paths resolved against the catalog's directory.

And the standing rule, at its sharpest here: the catalog of a partial
package refuses to know tensors that live on absent shards
(`no tensor named …layers.0.mlp.gate.weight`) — the honest answer, not a
guess.

## Reproducing

Layout (any equivalent works): the upstream repository
`Qwen/Qwen3.8-Flash-Next` at revision `de4b8e4d`, with shards 1, 57–59,
80–82, 130, 131 downloaded beside the full small-file set, and
`qwen4exp.pack.yaml` from this guide. The MoE, hyper-connection, indexer,
and MTP bindings are left as the reader's exercise — the config has every
number needed.
