# Deep Dive: Dissecting Gemma-4 E4B with the `nn` Workbench

A second real-model walkthrough — the mirror image of the
[Kokoro deep dive](NN_DEEPDIVE_KOKORO.md). Kokoro was an ONNX export beside an
unreadable pickle; **google/gemma-4-E4B-it** is a native SafeTensors release:
one 15.99 GB `model.safetensors`, a real 262k-vocabulary tokenizer, and a
multimodal architecture (text + vision + audio) that stresses packs, selectors,
splitting, and transactional editing at a scale no fixture can reach.

Every output below is a real capture. And exactly as with the first deep dive,
the real artifact immediately found things to fix — three improvements to the
engine shipped alongside this document (see
[What the dive changed](#what-the-dive-changed-in-the-engine)).

## The subject

| Artifact | Role | Size |
|---|---|---|
| `model.safetensors` | The whole model: text stack + vision tower + audio encoder, BF16 | 15.99 GB (14.9 GiB) |
| `tokenizer.json` | BPE vocabulary | 32.2 MB |
| `config.json` | Architecture configuration | 5.1 KB |
| `chat_template.jinja` | Chat template | 18.6 KB |

From `config.json` alone, before reading a single payload byte: 42 decoder
layers on a 5:1 sliding/full attention schedule, hidden 2560, MLP intermediate
10240, 8 attention heads of 256 (sliding) / 512 (full) dimensions, 2 KV heads,
tied embeddings, a 16-layer vision tower and a 12-layer audio encoder.

## Part 1 — First contact

```bash
binfiddle -i gemma-4-E4B-it/ nn discover --verify-content --out-catalog g.nn.json
```

```text
./model.safetensors: parsed
  format: safetensors 1
  validity: valid
  tensors: 2130
  tensor model.audio_tower.layers.0.feed_forward1.ffw_layer_1.input_max [] 1 elements, encoding safetensors.BF16
  ...
./tokenizer.json: asset (json-configuration)
./chat_template.jinja: asset (opaque)
...
coverage: 1 of 1 sources parsed, 0 problematic, 0 symlinks skipped
```

2130 tensors, content-verified (the full 15.99 GB hashed for identity), and
every side file classified. Validation is clean:

```text
./model.safetensors: structurally_valid_for_reader
  behavior: behavior_not_evaluated
```

The tokenizer is a real sentencepiece-family asset — and inspecting it is the
first thing this dive changed in the engine: the initial run failed with
`node count exceeds limit`, because the strict JSON parser's flat 1M-node
guard could not admit a 262k-entry vocabulary (≈4.5M JSON nodes). Node
allowances now scale with the bounded input length — a well-formed document
cannot exceed `len/2` nodes, so real assets parse while small-input expansion
bombs stay impossible:

```bash
binfiddle nn tokenizer inspect --package gemma-4-E4B-it/
```

```text
tokenizer assets
  tokenizer.json [tokenizer_json] 32169626 bytes
  tokenizer_config.json [tokenizer_config] 3082 bytes
  tokenizer.json: model BPE, vocab 262144, merges 514906, added tokens 24
  claims: static asset and structure inspection; tokenization behavior and template rendering are NOT evaluated
```

## Part 2 — What 16 GB of weights looks like

```bash
binfiddle nn ls --catalog g.nn.json --sort bytes --limit 4
```

```text
#    name                              encoding          elements                 bytes
1    model.language_model.embed_toke… safetensors.BF16  2818572288 [262144x10752] 5637144576
2    model.language_model.embed_toke… safetensors.BF16  671088640 [262144x2560]   1342177280
3    model.language_model.per_layer_… safetensors.BF16  27525120 [10752x2560]     55050240
4    model.language_model.layers.0.m… safetensors.BF16  26214400 [2560x10240]    52428800
```

The two giants explain the "E4B" (effective-parameters) trick: a 5.6 GB
`[262144 × 10752]` slab — the per-layer input embeddings for all 42 layers
fused into one table (10752 = 42 × 256) — beside the 1.3 GB main embedding.
Addressing works at that scale without ceremony:

```bash
binfiddle nn locate --catalog g.nn.json --offset 0x150000000
```

```text
offset 5637144576 (0x150000000):
  model.language_model.embed_tokens_per_layer.weight: element [170816,6164]
  (payload [1963904984, 7601049560))
```

Statistics stay honest on tensors with billions of elements — sample mode
states exactly what it covered:

```bash
binfiddle nn analyze --catalog g.nn.json \
    --tensor model.language_model.embed_tokens.weight --mode sample --sample-size 20000
```

```text
  coverage: mode sample — 20000 of 671088640 elements examined, 40000 bytes read (completed)
  mean:     -0.00008728658501058834
  min:      -0.10791015625 at [175854, 1613] (+0 ties)
  max:      0.1435546875 at [250574, 634] (+1 ties)
  finding [info] SAMPLE_INCOMPLETE: observations cover 20000 of 671088640 elements; statements apply to the sample only
```

And one genuinely interesting full scan: Gemma's RMSNorm weights are famously
large-scale, and the numbers show it — mean ≈ 26.2, maximum 290, on a tensor
of 2560 values that a fixture-writer would never have produced:

```text
analyze model.language_model.layers.5.input_layernorm.weight (safetensors.BF16)
  coverage: mode full — 2560 of 2560 elements examined (completed)
  mean:     26.201657009124762
  max:      290 at [340] (+0 ties)
```

## Part 3 — A pack for a real architecture

The pack vocabulary is deliberately closed — unknown binding kinds are
rejected, not guessed (the first draft of this dive's pack used
`attention_q` and was told so). Authoring against `config.json`:

```yaml
schema: binfiddle.nn.pack/v1
id: gemma4.e4b.text
version: "1.1.0"
config:
  hidden_size: 2560
  intermediate_size: 10240
  num_attention_heads: 8
  head_dim: 256
  num_key_value_heads: 2
  global_head_dim: 512
layer_types: [sliding_attention, ..., full_attention]   # the 5:1 schedule
bindings:
  - pattern: "model.language_model.layers.{layer}.self_attn.q_proj.weight"
    component: "decoder.layers[{layer}].attention.q"
    kind: dense
    shape: ["num_attention_heads * head_dim", "hidden_size"]
    layer_kinds: [sliding_attention, linear_attention]
  - pattern: "model.language_model.layers.{layer}.self_attn.q_proj.weight"
    component: "decoder.layers[{layer}].attention.q"
    kind: dense
    shape: ["num_attention_heads * global_head_dim", "hidden_size"]
    layer_kinds: [full_attention]
  ...
```

The story inside that YAML is the dive's second engine change. The first pack
draft assumed one head dimension for all 42 layers; recognition refused to
pretend that was true — the seven **full-attention layers** (5, 11, 17, 23,
29, 35, 41) have genuinely larger projections (`q [4096×2560]` vs
`[2048×2560]`), and they surfaced as visible contradictions instead of
bindings. Real Gemma generations all share this dual head-dimension scheme,
so the pack schema grew what the architecture needs: a third layer kind
(`sliding_attention`, never mislabeled as linear attention) and
`layer_kinds:` binding scoping — one component path, different expected
shapes per layer kind. With it:

```bash
binfiddle nn pack verify --pack gemma4-text.pack.yaml
binfiddle nn ls --catalog g.nn.json --view architecture --pack gemma4-text.pack.yaml | head -3
```

```text
pack verified: gemma4.e4b.text v1.1.0 (12 bindings, id pack:291cff23…)
architecture view (pack gemma4.e4b.text)
  full-attention layers: [5, 11, 17, 23, 29, 35, 41]
```

— the schedule read out of the pack matches `config.json` exactly. Every
selector now resolves across both layer kinds:

```text
decoder.layers[5].attention     → 4 tensors      (full-attention layer)
decoder.layers[41].attention    → 4 tensors      (full-attention layer)
decoder.layers[*].attention     → 168 tensors    (42 layers × q/k/v/o)
decoder.layers[*].mlp.gate      → 42 tensors
```

Decomposition and planning on the 16 GB model are cheap with reference
storage (plans and selections only — no copying):

```bash
binfiddle nn split --catalog g.nn.json --pack gemma4-text.pack.yaml \
    --storage reference --out-dir split/
binfiddle nn partition --catalog g.nn.json --pack gemma4-text.pack.yaml --stages 4
```

```text
  layer 41: 8 tensors, 209720320 bytes — reference (plan saved) [decoder.layers[41]]
  unresolved: 1794 tensors (no layer claimed; visible above)
  unique bytes: 7890749440
stage 0: layers [0..11]  (88 tensors, 2044779520 weight bytes)
stage 1: layers [11..21] (80 tensors, 1887488000 weight bytes)
stage 2: layers [21..31] (80 tensors, 1887488000 weight bytes)
stage 3: layers [31..42] (88 tensors, 2070993920 weight bytes)
  unlayered: 1794 tensors, 8101565396 bytes (embeddings/norms/heads; never silently distributed)
```

Note the accounting honesty: the four stages balance ~2 GB of layer weights,
and the 8.1 GB of unlayered tensors (the two embedding giants, the vision and
audio towers) is reported, never silently distributed across stages.

## Part 4 — Editing 16 GB, transactionally

```bash
binfiddle nn edit set --catalog g.nn.json \
    --tensor model.language_model.layers.5.input_layernorm.weight \
    --index 0 --value 26.25 --save-plan norm-edit.plan.json
```

```text
edit plan
  tensor:    model.language_model.layers.5.input_layernorm.weight [0]
  encoding:  safetensors.BF16
  write:     bytes [14645850146, 14645850148)
  change:    69 -> 26.25 (requested value 26.25)
  bytes:     8a42 -> d241
  policy:    exact_only
```

A two-byte BF16 write at offset 14.6 billion, planned against the
content-verified catalog. Applying writes a fresh 15.99 GB output (the
original is never touched), verifies **every byte outside the two-byte span
is identical**, and revalidates the container:

```bash
binfiddle nn edit apply --catalog g.nn.json --plan norm-edit.plan.json \
    --out-model model-edited.safetensors
```

```text
  written:   2 bytes
  preserved: all bytes outside the planned span verified identical
  reparsed: container reparse valid, spans unchanged
```

The layered diff then sees exactly what happened. Diff speaks only about
verified bytes, so both sides need content-verified discovery — re-hashing
the 15.99 GB output is the price of certainty:

```bash
binfiddle -i model-edited.safetensors nn discover --verify-content --out-catalog edited.nn.json
binfiddle nn diff --left g.nn.json --right edited.nn.json | tail -2
```

```text
  model.language_model.layers.5.input_layernorm.weight: content changed
  summary: 0 identical, 1 content-changed, 2129 repacked, 0 descriptor changes, 0 unmatched, 2 member diffs
```

Read the summary carefully — it encodes exactly what happened. One tensor's
bytes changed. The other 2129 report as **repacked**, not identical: their
payloads are byte-for-byte the same, but they live in a *new container* (a
different file with its own identity). "Identical" is reserved for tensors
that are the same bytes in the same source; a fresh output file with matching
tensor bytes is a repack. The layers never blur: content change, repack, and
descriptor change are three different statements, and two of them appear here
for the price of one two-byte edit.

## What the dive changed in the engine

Real artifacts are the best test suite. This dive shipped three changes:

1. **Proportional JSON node limits.** A flat 1M-node parser limit rejected
   every real tokenizer vocabulary. `ParseLimits::for_input_len` scales the
   allowance with the (budget-bounded) input length; expansion bombs from
   small inputs remain impossible by construction.
2. **`sliding_attention` as a first-class layer kind**, plus
   **`layer_kinds:` binding scoping** for architectures whose shapes differ
   per layer kind (the Gemma dual head-dimension scheme). The lint rule
   learned that same-template bindings are clean when their kind scopes are
   disjoint. Pack ids are unchanged for packs that do not use the new
   vocabulary.
3. **Catalog-relative source paths.** Catalogs record paths as discovered;
   running a command from a different working directory no longer breaks
   source resolution (the catalog's own directory is preferred for relative
   paths, with the old behavior as fallback).

Plus the reminder that matters most: when the first pack draft was wrong
about the model, the engine said so — contradictions stayed visible until the
pack told the truth.

## Reproducing

Layout assumed (any equivalent works):

```
gemma-4-E4B-it/   model.safetensors, tokenizer.json, config.json, …
gemma4-text.pack.yaml
```

Sources: `google/gemma-4-E4B-it` (upstream revision `ee0ef602` at the time of
writing). The pack in this guide covers the text stack; the vision and audio
towers are left as an exercise in reading `config.json` — which is rather the
point.
