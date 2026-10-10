# Deep Dive: EXL3 Storage on GLM-5.3-Flash with the `nn` Workbench

Fourth real-model walkthrough ([Kokoro](NN_DEEPDIVE_KOKORO.md) — the pickle
boundary; [Gemma-4 E4B](NN_DEEPDIVE_GEMMA.md) — packs at scale;
[Qwen3.8-Flash-Next](NN_DEEPDIVE_QWEN.md) — sharded hybrid). This one
dissects **EXL3 quantized storage** on a real GLM-5.3-Flash checkpoint —
the format where a logical projection weight no longer *is* one tensor
but a group of coded-storage fields, and where inferring a contract from
tensor names is exactly the wrong move.

Every output below is a real capture from the actual artifacts.

> **Companion lab and codec correction (2026-10-09):**
> [The GLM expert lab](NN_DEEPDIVE_GLM_LAB.md) supplies a pinned acquisition
> helper, a complete expert specimen, independent decode checks, and a
> reversible two-byte experiment. It exposed errors in the earlier numerical
> decoder. The storage observations below remain useful; Part 5 now points to
> the corrected, reproducible numerical results.

## The subject

| Artifact | Role | Size |
|---|---|---|
| `model-00020-of-00083.safetensors` | One shard of the GLM-5.3-Flash EXL3 4bpw (TensorFold) release | 2.15 GB |
| `model.safetensors.index.json` | The 83-shard index (150,226 tensors, 175.6 GB total) | 15.6 MB |
| `config.json` | Architecture (`glm5_next`) + `quantization_config` (`exl3`, bits 4, codebook `mcg`, scope routed experts only) | 6.1 KB |

From `config.json` before touching payload bytes: 45 decoder layers, 288
routed experts (+1 shared), 8 active per token, hidden 4096, MoE
intermediate 2048, mHC attention with hyper-connections — and the
quantization declaration that makes this checkpoint a *specific EXL3
variant*: **bits 4, codebook mcg, routed-experts-only scope**.

Each quantized projection decomposes into four storage fields:

| Field | Dtype | Role (observed) |
|---|---|---|
| `trellis` | I16 `[a, b, c]` | coded storage; the inner extent tracks the variant (bits/codebook) |
| `mcg` | I32 `[1]` | codebook selector |
| `suh` | F16 `[ext]` | head scales along the INPUT extent |
| `svh` | F16 `[ext]` | head scales along the OUTPUT extent |

The gate/up orientation stores trellis `[256, 128, 64]`; the transposed
down orientation stores `[128, 256, 64]` — and **suh/svh swap lengths**
between the orientations. That asymmetry is why a profile must bind each
field per projection, not one pattern for all.

## Part 1 — A sharded release, planned before downloaded

```bash
binfiddle nn shard-map --index model.safetensors.index.json \
    --catalog tf20.nn.json --selection e27up.sel.json
```

```text
  model-00020-of-00083.safetensors: 4 tensors, 4206596 payload bytes
    …up_proj.mcg / suh / svh / trellis
  shards: 1  tensors: 4  missing from index: 0
  planned payload 4206596 of 175622979576 declared bytes (0.0%)
```

One shard of 83 holds everything for the target projection — planned
statically, before any of the 175.6 GB moves.

## Part 2 — The storage profile (packs/EXL3, step 1)

The profile ships in-repo as
[packs/exl3-glm53-tensorfold-4bpw.yaml](../packs/exl3-glm53-tensorfold-4bpw.yaml)
— a pure-data model pack, explicitly versioned to the variant:

```yaml
id: exl3.glm53flash.tensorfold.4bpw
version: "0.1.0"
description: >-
  … The LOGICAL matrix shape of each projection is deliberately NOT
  asserted: resolve it from the model config and the checkpoint's
  declared quantization variant …, never from tensor names. Other EXL3
  variants (different bits or codebooks) yield different trellis inner
  extents — they must bind to their own profile version, producing
  visible contradictions here rather than silent mismatches.
```

Twelve bindings group the four fields under one component per projection:

```text
decoder.layers[12].mlp.experts[27].up.mcg   <- …up_proj.mcg
decoder.layers[12].mlp.experts[27].up.suh   <- …up_proj.suh
decoder.layers[12].mlp.experts[27].up.svh   <- …up_proj.svh
decoder.layers[12].mlp.experts[27].up.trellis <- …up_proj.trellis
```

```bash
binfiddle nn pack verify --pack packs/exl3-glm53-tensorfold-4bpw.yaml
# → pack verified: exl3.glm53flash.tensorfold.4bpw v0.1.0 (12 bindings, …)
```

Selecting the whole projection — one selector, four fields:

```bash
binfiddle nn select --catalog tf20.nn.json --pack packs/exl3-glm53-tensorfold-4bpw.yaml \
    --select 'decoder.layers[12].mlp.experts[27].up' --out-selection e27up.sel.json
# → selection … (4 tensors)
```

## Part 3 — The fields, one by one

**trellis — the coded storage.** The generic I16 view stays exact:

```bash
binfiddle nn where --catalog tf20.nn.json \
    --tensor …up_proj.trellis --index 12,34,7
# → precision: exact_contiguous
#   file span: [1148580022, 1148580024)
```

What that element *decodes to* as a weight value is a separate step from
the storage profile: `nn exl3 decode` is the qualified codec that answers
it (see Part 5). The storage view stays preserved and addressable, and
`nn analyze` reports the raw I16 population.

**suh — head scales, decodable today.** The F16 fields decode through
the existing codec:

```text
analyze …up_proj.suh (safetensors.F16)
  coverage: mode sample — 1598 of 4096 elements examined (completed)
  mean:     0.000084009
  min/max:  -0.017227 / 0.017212
```

**mcg — the codebook selector.** A single I32; the config declares it as
the codebook (`quantization_config.codebook: mcg`). The generic scalar view
reports the stored integer; the specialized EXL3 decoder verifies the selector
magic before interpreting the trellis.

## Part 4 — Variant mismatch stays visible

The profile is bound to one variant. Recognizing a **different** EXL3
variant against it must produce retained contradictions, not silent
bindings. The authentic K2-quantized sample of the same projection
(2,109,444 payload bytes, sha-verified against its acquisition manifest;
trellis inner extent 32 instead of the TensorFold's 64):

```text
architecture view (pack exl3.glm53flash.tensorfold.4bpw)
  components:
    decoder.layers[12].mlp.experts[27].up.mcg  …bound
    decoder.layers[12].mlp.experts[27].up.suh  …bound
    decoder.layers[12].mlp.experts[27].up.svh  …bound
  contradictions:
    …up_proj.trellis matched …up_proj.trellis but shape
    [256, 128, 32] != [256, 128, 64]
```

The three fields whose geometry coincides bind; the one that proves a
different variant refuses — exactly the discipline: a name match is not
a contract.

## Part 5 — Decoding the trellis

The codec lives in the workbench as `nn exl3 decode` / `nn exl3 block`.
The original version of this article presented K2 numerical captures whose
qualification was insufficient: the decoder used incorrect bit ordering and
matrix-to-lane mapping, missed fp16 codebook rounding, repeated tiles across
block columns, and applied scales in the wrong transform order. Those numerical
captures are withdrawn; they are not reference values for current builds.

The corrected reconstruction, in `[output,input]` coordinates, is
`W = diag(svh)·H·Wq·H·diag(suh)`. The scales belong outside the normalized
128-point Hadamard transforms. Block calculations use f32 and do not claim
the intermediate rounding of an inference kernel.

Use [Part 6 of the companion lab](NN_DEEPDIVE_GLM_LAB.md#6-cross-the-boundary-from-bytes-to-weights)
for pinned four-bpw captures, an independent bit-stream/butterfly reference,
and boundary probes. It supplies a downloadable specimen instead of depending
on the historical local K2 fixture. The profile mismatch in Part 4 remains a
descriptor-level observation, independent of the numerical decoder.

## Engine features this walkthrough exercises

1. **Multi-capture packs**: `{layer}` and `{expert}` numeric captures in
   one pattern, producing doubly-indexed component families
   (`layers[12].mlp.experts[27]`) addressable by the full selector
   grammar.
2. **Contradiction retention** across storage variants — shape mismatch
   on a matched name is a finding, never a guess.
3. **Static shard planning** (`nn shard-map`) over a 150k-tensor index.
4. The **generic scalar views** (I16/F16 addressing, statistics) that
   stay exact regardless of the storage scheme layered above them.
5. The **qualified EXL3 codec** (`nn exl3 decode`/`nn exl3 block`):
   trellis decode with lane mapping and suh/svh rescaling, now checked
   against the independent reference in the companion lab.

## Reproducing

Sources: `Mia-AiLab/GLM-5.3-Flash-EXL3-4bpw-TensorFold` (config, index,
shard 20). The variant-mismatch fixture is the independently acquired
K2 sample of `layers.12.mlp.experts.27.up_proj` (four payload files +
manifest, 2,109,444 bytes) wrapped in a constructed SafeTensors container
— the payloads are authentic and sha-verified; only the container is
constructed. For a self-contained reproduction using a pinned public
four-bpw release and current numerical results, use the
[GLM expert lab](NN_DEEPDIVE_GLM_LAB.md).
