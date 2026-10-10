# GLM-5.3-Flash: a 175 GB model, a 13 MB specimen, and a two-byte experiment

What does it mean to change a weight in a quantized mixture-of-experts model?
Sometimes the interesting object is a two-byte scale shared by thousands of
logical coefficients. Sometimes an integer that looks like a weight is really
part of a compressed code stream. And sometimes a convincing histogram is
answering the wrong question entirely.

This lab follows **one complete GLM-5.3-Flash expert** from a public shard index
to its physical bytes, decoded weights, and a reversible intervention. You will:

- Read the architecture before downloading weights, including an apparent
  extra-layer mystery in the index.
- Acquire 12 authentic fields with bounded HTTP ranges: about **28.4 MB** of
  successful response bodies, including metadata, from a **175.6 GB** release.
- Explain the difference between stored integers, quantized coefficients, and
  logical matrix entries.
- Zero one input scale, predict the affected weight column, measure the result,
  and restore the original file byte for byte.
- Cross-check binfiddle against a small independent CPU implementation.

No GPU, model server, PyTorch, or inference API is needed. This is a lab for
understanding **LLM artifacts and weight representations**. Text generation and
behavioral evaluations are follow-up experiments, not measurements made here.

It complements [the original GLM storage deep dive](NN_DEEPDIVE_GLM_EXL3.md),
which introduces the four-field profile and variant contradictions. All
numerical captures here use the corrected EXL3 decoder shipped with this lab;
the older decoder's numerical examples are superseded.

## 1. Make the experiment small enough to repeat

The upstream [GLM-5.3-Flash model card](https://huggingface.co/zai-org/GLM-5.3-Flash)
describes a multimodal model with 320B total and 18B active parameters. Our
specimen comes from the public
[Mia-AiLab EXL3 4bpw TensorFold conversion](https://huggingface.co/Mia-AiLab/GLM-5.3-Flash-EXL3-4bpw-TensorFold/tree/76c0b5173166d2795dd48860f45d8224817f894c).
That conversion is the object measured here; it is not interchangeable with the
upstream FP8 release or another EXL3 bitrate.

| Item | Pinned measurement |
|---|---|
| Repository | `Mia-AiLab/GLM-5.3-Flash-EXL3-4bpw-TensorFold` |
| Revision | `76c0b5173166d2795dd48860f45d8224817f894c` |
| Full index | 150,226 fields across 83 shards |
| Index `metadata.total_size` | 175,622,979,576 bytes of declared tensor payload |
| Target | Decoder layer 12, routed expert 27; indices are zero-based |
| Original containing shard | `model-00020-of-00083.safetensors`, 2,146,623,400 bytes |
| Selected payload | 12,619,788 bytes across 12 fields |
| Reconstructed specimen file | 12,621,516 bytes, including its new header |
| Successful acquisition response bodies | 28,443,135 bytes; retries can add traffic |

The selected payload is about **0.00719%** of the index's declared payload.
The resulting specimen is a new SafeTensors container holding unchanged
selected payload bytes. It is neither the original shard nor a runnable model.

### Setup

Use this checkout, Rust/Cargo, Python 3.10+, and curl 8.4+. The acquisition
helper uses curl's enforced transfer-size limit, including for responses without
a declared length. Its Python dependencies are all in the standard library.
Allow roughly 60 MB of disk for the successful specimen, metadata, edited copy,
and restored copy. Compilation has its own normal Cargo storage requirements.

Run the following from the **repository root**, in one shell. Change `LAB` to
another fresh scratch directory if needed. The examples deliberately put
downloads, catalogs, plans, and outputs outside the repository.

```bash
cargo build --locked --release
export BF="$PWD/target/release/binfiddle"
export LAB=/media/araray/merlin/sandbox/binfiddle/glm53-lab-reproduction
mkdir -p "$LAB"

"$BF" --build-info > "$LAB/build-info.json"
git rev-parse HEAD > "$LAB/checkout.txt"
git diff --binary > "$LAB/worktree.patch"
sha256sum "$BF" > "$LAB/binary.sha256"

python3 examples/glm53_expert_sample.py --output "$LAB/specimen"

export SAMPLE="$LAB/specimen/expert.safetensors"
export CAT="$LAB/expert.nn.json"
export PACK="$PWD/packs/exl3-glm53-tensorfold-4bpw.yaml"
export UP=model.language_model.layers.12.mlp.experts.27.up_proj
```

The helper refuses an existing output directory. After an interrupted download,
use a new directory; only a completed acquisition has `acquisition.json`. It
retries individual transfers at most three times and checks the status, exact
`Content-Range`, response length, field shapes, and codebook selector. Payload
requests are split into at most 1 MiB ranges. A server ignoring `Range` cannot
turn this into a whole-shard download.

**Everything after acquisition works offline.** The helper is the network
component; `binfiddle nn` itself does not download anything.

## 2. Read the model as a set of hypotheses

First inspect the pinned configuration and the full index you just downloaded:

```bash
python3 - <<'PY'
import collections, json, os
from pathlib import Path
p = Path(os.environ["LAB"]) / "specimen"
c = json.loads((p / "config.json").read_text())["text_config"]
w = json.loads((p / "model.safetensors.index.json").read_text())["weight_map"]
print("hidden / expert width:", c["hidden_size"], c["moe_intermediate_size"])
print("attention:", dict(collections.Counter(c["layer_types"])))
print("MLP:", dict(collections.Counter(c["mlp_layer_types"])))
print("stored / selected experts:", c["n_routed_experts"], c["num_experts_per_tok"])
layers = sorted({int(n.split(".")[3]) for n in w if ".mlp.experts." in n})
print("indexed routed-expert layers:", layers)
print("routed-expert fields:", sum(".mlp.experts." in n for n in w))
print("next-token prediction layers:", c["num_nextn_predict_layers"])
PY
```

The [pinned configuration](https://huggingface.co/Mia-AiLab/GLM-5.3-Flash-EXL3-4bpw-TensorFold/blob/76c0b5173166d2795dd48860f45d8224817f894c/config.json)
declares 45 decoder layers, hidden size 4096, expert intermediate size 2048,
288 routed experts, one shared expert, and eight routed experts selected per
token. Its schedules contain **34 linear-attention layers and 11 sparse-attention
layers**, independently of an MLP schedule with **three dense and 42 sparse
layers**. Layer 12 combines linear attention with a sparse MLP.

There are two independent kinds of sparsity to investigate: attention and expert
selection. An expert is part of the feed-forward computation; it is not an
attention head. A layer's attention type does not tell you whether its MLP is
dense or routed.

### The extra-layer mystery

The index contains routed-expert names for **layers 3 through 45 inclusive**:
43 layer namespaces, even though the ordinary decoder schedule has 45 layers
numbered 0 through 44. It contains 148,608 routed-expert storage fields:

```text
43 layer namespaces × 288 experts × 3 projections × 4 fields = 148,608
150,226 total fields − 148,608 expert fields = 1,618 other fields
```

The configuration also declares one next-token prediction layer. That makes the
extra namespace consistent with an auxiliary prediction layer. **The index and
config establish the discrepancy and support that interpretation; they do not
show which modules a particular inference path executes.** Trace a runtime's
architecture implementation before counting layer 45 as an ordinary decoder
block. A tensor-name range is not a forward-pass trace.

### Why eight active experts do not imply an eight-expert download

The simple ratio `8 / 288 = 1 / 36` describes selected routed experts per token
within a sparse layer. The stored pool still contains all 288. Different tokens
can choose different experts; a batch can need a much larger union of them.

For the ordinary 42 sparse decoder layers, the three matrices per routed expert
account for this many logical coefficients:

```text
one projection:     4096 × 2048                 =     8,388,608
one expert:        3 × 4096 × 2048             =    25,165,824
stored pool:       42 × 288 × 25,165,824       = 304,405,807,104
selected per token:42 ×   8 × 25,165,824       =   8,455,716,864
```

These are counts for **those routed matrices only**. They exclude shared
experts, dense MLPs, attention, embeddings, vision, and auxiliary prediction
modules. They are neither a replacement for the model card's total/active
counts nor a FLOP, RAM, or latency estimate. Expert sparsity primarily reduces
which computations a token needs; residency and transfer costs depend on the
runtime's placement and offloading policy.

## 3. A complete expert is three matrices, stored as twelve fields

An expert's gate and up projections expand a hidden vector into the expert
intermediate space; its down projection returns to the hidden space. A useful
schematic is:

```text
hidden vector x: 4096 entries
gate = W_gate x: 2048 entries
up   = W_up   x: 2048 entries
expert output ≈ W_down (SiLU(gate) ⊙ up): 4096 entries
```

This is the familiar gated-MLP shape explanation. It omits GLM's configured
clipping and the surrounding routing, shared-expert, and residual computations.
It is not executable GLM forward-pass code.

In this conversion each matrix is represented by four fields. Using the
workbench's `[output, input] = [N, K]` matrix convention:

| Projection | Logical matrix `[N,K]` | `trellis` I16 shape | `suh` F16 length | `svh` F16 length |
|---|---|---|---|---|
| gate | `[2048,4096]` | `[256,128,64]` | 4096 | 2048 |
| up | `[2048,4096]` | `[256,128,64]` | 4096 | 2048 |
| down | `[4096,2048]` | `[128,256,64]` | 2048 | 4096 |

Each also has one I32 `mcg` selector. The configuration specifies EXL3, four
bits per weight, the `mcg` codebook, and **routed-experts-only** quantization.
Do not extrapolate this storage scheme to every other field in the package.

For one projection the observed storage budget is:

```text
trellis: 256 × 128 × 64 × 2 bytes   = 4,194,304
suh:     4096 × 2 bytes            =     8,192
svh:     2048 × 2 bytes            =     4,096
mcg:     1 × 4 bytes               =         4
                                     ---------
total payload                      = 4,206,596 bytes
```

The trellis alone costs exactly four bits per logical coefficient. Including
these scales and the selector gives **4.01172256 bits per coefficient**, before
container metadata. Three projections total 12,619,788 payload bytes.

Notice another counting trap: the up trellis contains **2,097,152 I16 elements**
but represents **8,388,608 coefficients**. Counting SafeTensors elements in a
quantized release is not the same operation as counting model parameters.

## 4. Prove what you acquired

A SafeTensors file starts with an eight-byte little-endian header length,
followed by a JSON header and tensor payload. A descriptor's `data_offsets` are
relative to the payload region. The acquisition helper computes:

```text
original file offset = 8 + original header length + descriptor offset
specimen file offset = 8 + specimen header length + new descriptor offset
```

For the source shard, the payload region begins at byte **240,528**. In our
specimen it begins at byte **1,728**. The two files therefore have different
coordinate systems even when individual tensor payloads are identical. See the
[SafeTensors format specification](https://github.com/huggingface/safetensors#format).

`acquisition.json` records the pinned revision, source shard sizes and header
hashes, both coordinate systems, each field's SHA-256, and the specimen digest.
Verify the reconstructed payloads against that record:

```bash
python3 - <<'PY'
import hashlib, json, os
from pathlib import Path
p = Path(os.environ["LAB"]) / "specimen"
m = json.loads((p / "acquisition.json").read_text())
data = (p / "expert.safetensors").read_bytes()
assert hashlib.sha256(data).hexdigest() == m["sample"]["sha256"]
for field in m["fields"]:
    start, end = field["sample_file_span"]
    assert hashlib.sha256(data[start:end]).hexdigest() == field["sha256"]
print("12 payload hashes and the specimen hash match")
print(m["sample"]["sha256"])
PY
```

Expected specimen SHA-256 for the default helper invocation:

```text
db74518d744851ec5dbd094d33d9694db9dce64f6005c02b49e07d863c4dfcef
```

These hashes identify the bytes acquired and let you detect later changes. We
did not download and hash the entire original shard; a range acquisition does
not establish its full-file digest. The new container's digest also says
nothing about model quality or the equivalence of different quantizations.

### Catalog the specimen, then give the fields architectural names

```bash
"$BF" -i "$SAMPLE" nn discover --verify-content --out-catalog "$CAT"
"$BF" nn ls --catalog "$CAT" --sort name
"$BF" nn pack verify --pack "$PACK"
"$BF" nn ls --catalog "$CAT" --view architecture --pack "$PACK"

"$BF" nn select --catalog "$CAT" --pack "$PACK" \
  --select 'decoder.layers[12].mlp.experts[27].up' \
  --out-selection "$LAB/up.sel.json"
```

The catalog contains 12 tensors. The selection contains four, covering the
entire up projection. Content verification hashes this small file and supplies
the stronger source identity required by the edit workflow.

**Discover the specimen file explicitly.** Its directory also contains the
original 83-shard index for research. Discovering that directory asks a different
question about an incomplete package. Likewise, complete coverage of the
specified specimen file does not mean complete coverage of GLM.

Now connect the local selection back to the original package:

```bash
"$BF" nn shard-map --index "$LAB/specimen/model.safetensors.index.json" \
  --catalog "$CAT" --selection "$LAB/up.sel.json"
```

The relevant captured lines are:

```text
model-00020-of-00083.safetensors: 4 tensors, 4206596 payload bytes
shards: 1  tensors: 4  missing from index: 0
planned payload 4206596 of 175622979576 declared bytes (0.0%)
```

The percentage rounds to one decimal place; zero displayed here does not mean
zero bytes. Also distinguish **selected payload bytes**, **whole-shard download
bytes**, and **range-download traffic including metadata**. They answer three
different capacity-planning questions. Use the release build for this large
index; strict parsing in a debug build can be substantially slower.

## 5. Ask a statistic what it is measuring

```bash
"$BF" nn analyze --catalog "$CAT" --tensor "$UP.trellis" \
  --mode sample --seed 17 --sample-size 4096

"$BF" nn analyze --catalog "$CAT" --tensor "$UP.suh" --mode full
```

| Observation | Trellis, sampled I16 storage | Input scales, full F16 field |
|---|---:|---:|
| Elements examined | 4,091 of 2,097,152 | 4,096 of 4,096 |
| Payload bytes read | 8,182 | 8,192 |
| Mean | −125.0337326 | 0.00004911423 |
| Population variance | 353,178,781.8370 | 0.00029113928 |
| Minimum | −32,756 | −0.01724243164 |
| Maximum | 32,737 | 0.01722717285 |

The trellis values are **signed interpretations of packed bit patterns**. Their
range near the limits of I16 does not indicate exploding neural-network weights.
The negative mean does not indicate a negatively biased expert. You have
measured a storage representation, before its codebook, permutation, transforms,
and scales.

The sample requested 4,096 draws but examined 4,091 unique elements. Read the
reported coverage instead of assuming the requested sample size was achieved.
The report explicitly limits its statements to the observed sample.

The F16 scales are directly decodable numeric values. Their small mean also
does not make them unimportant: positive and negative scales can cancel in an
average while every one participates in reconstruction. Use magnitude, spread,
and the role of the field to decide what to investigate next.

## 6. Cross the boundary from bytes to weights

There are three coordinate spaces in this experiment:

| Question | Coordinates | Command |
|---|---|---|
| Where is one stored I16 word? | `[k_group,n_group,word]` | `nn where` |
| What is a decoded coefficient in the rotated basis? | `Wq[n,k]` | `nn exl3 decode` |
| What are logical coefficients after transforms and scales? | `W[n,k]`, aligned 128×128 block | `nn exl3 block` |

For example:

```bash
"$BF" nn where --catalog "$CAT" --tensor "$UP.trellis" --index 12,34,7
"$BF" nn exl3 decode --catalog "$CAT" --trellis "$UP.trellis" --index 3,5
"$BF" nn exl3 block --catalog "$CAT" --trellis "$UP.trellis" --origin 0,0
```

Captured results:

```text
stored word [12,34,7]: specimen file span [8628186, 8628188)
Wq[3,5]:              0.54052734
logical block (0,0):  mean -0.000069, variance 0.00035795, abs max 0.080924
```

These are deliberately different queries. `nn where` reports dependencies for
the scalar I16 storage element; it does not translate that element into an
EXL3 logical-weight influence set.

### What the decoder has to know

A 16×16 tile represents 256 coefficients with 128 payload bytes at four bpw.
Its codes form a circular stream, packed most-significant-bit first within
little-endian 32-bit words. Each coefficient's codebook input is a **16-bit
window**, overlapping neighboring codes. At four bpw, a window spans four
successive four-bit code contributions. Physical storage also uses a tensor-core
lane permutation, so consecutive codes are not ordinary consecutive matrix
columns. These details come from the pinned upstream
[packer](https://github.com/turboderp-org/exllamav3/blob/151539c77abc7ab7425d30da7a4e8e3c5c154e7b/exllamav3/exllamav3_ext/quant/pack.cu),
[window decoder](https://github.com/turboderp-org/exllamav3/blob/151539c77abc7ab7425d30da7a4e8e3c5c154e7b/exllamav3/exllamav3_ext/quant/exl3_dq.cuh),
and [lane permutation](https://github.com/turboderp-org/exllamav3/blob/151539c77abc7ab7425d30da7a4e8e3c5c154e7b/exllamav3/modules/quant/exl3_lib/quantize.py#L21-L49).

The `mcg` field contains the selector `0xCBAC1FED`. Its codebook multiplies a
window by that constant modulo 2³², transforms the resulting bits into two
binary16 values, and adds them with binary16 rounding. Thus a four-bit storage
rate does not mean that every weight independently selects one of only 16
ordinary scalar levels. The overlapping trellis state matters. See the
[codebook implementation](https://github.com/turboderp-org/exllamav3/blob/151539c77abc7ab7425d30da7a4e8e3c5c154e7b/exllamav3/exllamav3_ext/quant/codebook.cuh).

Decoded `Wq` still lives in a rotated basis. For our `[N,K]` convention, logical
weights are reconstructed as:

```text
W = diag(svh) · H_N · Wq · H_K · diag(suh)
```

Each `H` is block diagonal with normalized 128-point Sylvester Hadamard blocks:
entries are `±1/√128`, with signs determined by the parity of `i & j`. The
upstream weight getter uses `[K,N]`; transposing its operation order gives the
formula above. **The scale vectors are outside the transforms.** Moving a
diagonal scale through a Hadamard transform changes the result. See
[the upstream reconstruction order](https://github.com/turboderp-org/exllamav3/blob/151539c77abc7ab7425d30da7a4e8e3c5c154e7b/exllamav3/modules/quant/exl3.py#L239-L249).

The workbench computes block arithmetic in f32. It does not reproduce every
intermediate fp16 rounding decision of a GPU kernel. Also, a block query in the
current implementation loads the projection's **whole trellis field**, not just
the 64 tiles contributing to the output block. Small output does not necessarily
mean a correspondingly small read.

### A second implementation should disagree when the first is wrong

Run the included reference checker:

```bash
python3 examples/glm53_reference.py "$SAMPLE" \
  --binfiddle "$BF" --catalog "$CAT" > "$LAB/reference-before.json"
```

It uses a literal bit string, the forward lane permutation, and f64
Walsh-Hadamard butterflies. Binfiddle uses u32 window extraction, the inverse
permutation, and f32 matrix multiplication. The checker compares seven `Wq`
probes exactly and the three block statistics within explicit f32 tolerances
(`rel_tol=5e-5`, `abs_tol=1e-8`). Decimal values in binfiddle's JSON envelopes
are strings; the checker converts them explicitly.

Some exact reference values from this specimen:

| Coordinate | `Wq` |
|---|---:|
| `[0,0]` | 0.32373046875 |
| `[3,5]` | 0.54052734375 |
| `[0,16]` | 0.3583984375 |
| `[16,0]` | −0.93017578125 |
| `[127,127]` | −1.904296875 |
| `[128,128]` | 0.74853515625 |
| `[2047,4095]` | 0.947265625 |

Boundary probes matter: repeating the first 16-column tile across a 128-column
block can produce plausible statistics while being wrong. Uniform all-zero or
all-one test tiles also hide bit-order and lane-permutation mistakes. This lab
exposed those gaps in the earlier decoder; the correction adds asymmetric
packed-stream tests for integer bitrates 1–8 and full-block comparisons with
nonuniform signed scales at both zero and nonzero origins.

## 7. Predict a two-byte intervention before applying it

We will set `up_proj.suh[0]` to zero. In
`W = diag(svh) H_N Wq H_K diag(suh)`, that scalar multiplies the **final input
column zero**. The prediction is precise:

1. One F16 storage element changes; every trellis byte stays the same.
2. `Wq` stays the same.
3. The 2,048 logical entries of `W_up[:,0]` become zero; other columns stay the
   same under this reconstruction.
4. For a hypothetical input vector `x`, the up projection's preactivation
   changes by `Δ(W_up x) = −W_up[:,0] x[0]`.

The fourth statement is linear algebra, not a measured GLM activation. If
`x[0]` is zero, this projection sees no change from the edit for that input.
Otherwise the effect still has to pass through the expert's other operations
and the model's routing. Two changed bytes do not determine a behavioral effect.

### Plan and inspect

```bash
"$BF" nn where --catalog "$CAT" --tensor "$UP.suh" --index 0

"$BF" nn edit set --catalog "$CAT" --tensor "$UP.suh" \
  --index 0 --value 0 --policy exact_only \
  --save-plan "$LAB/zero-input.plan.json"
```

Captured plan:

```text
tensor:    model.language_model.layers.12.mlp.experts.27.up_proj.suh [0]
encoding:  safetensors.F16
write:     bytes [8414924, 8414926)
change:    0.0169525146484375 -> 0 (requested value 0)
bytes:     5724 -> 0000
policy:    exact_only
```

`5724` here is the two bytes `57 24` in file order, representing the
little-endian F16 word `0x2457`. The same scale begins at source-shard offset
**201,736,104**, not specimen offset 8,414,924. The acquisition manifest retains
that relationship; an edit plan intentionally addresses only its bound local
source revision.

### Apply to a fresh file, then measure both representations

```bash
"$BF" nn edit apply --catalog "$CAT" --plan "$LAB/zero-input.plan.json" \
  --out-model "$LAB/edited.safetensors" --undo-bundle "$LAB/undo"

"$BF" -i "$LAB/edited.safetensors" nn discover --verify-content \
  --out-catalog "$LAB/edited.nn.json"

"$BF" nn diff --left "$CAT" --right "$LAB/edited.nn.json" --decoded

"$BF" nn exl3 block --catalog "$LAB/edited.nn.json" \
  --trellis "$UP.trellis" --origin 0,0

python3 examples/glm53_reference.py "$SAMPLE" \
  --binfiddle "$BF" --catalog "$CAT" --compare "$LAB/edited.safetensors" \
  > "$LAB/reference-change.json"
```

The storage diff reports **one content-changed field** and **one unequal value
among 4,096 decoded scales**. Its other 11 fields are reported as `repacked
(same bytes; offsets …)`: their source file identity changed, even though this
edit kept their bytes and offsets. Read the detailed comparison, not just the
category name.

The independent reference examines every entry of logical block `(0,0)`:

| Measurement | Before | After |
|---|---:|---:|
| Logical block mean | −0.00006863443 | −0.00007965945 |
| Population variance | 0.00035795020 | 0.00035503788 |
| Absolute maximum | 0.08092387748 | 0.08092387748 |
| Changed entries in this 128×128 block | — | 128 |
| Changed input columns | — | `[0]` |
| Maximum absolute entry change | — | 0.04587439127 |

The absolute maximum stayed unchanged despite a real column intervention. A
single summary statistic can miss a localized, structured change. The explicit
coordinate comparison is what verifies the prediction.

The table shows **f64 reference** statistics; binfiddle's f32 text report rounds
them further. This precision distinction is part of the evidence, not a reason
to treat differing last digits as a model difference.

### Restore and verify the whole file

```bash
"$BF" nn edit undo --bundle "$LAB/undo" --target "$LAB/edited.safetensors" \
  --out-model "$LAB/restored.safetensors"

cmp "$SAMPLE" "$LAB/restored.safetensors"
sha256sum "$SAMPLE" "$LAB/restored.safetensors"
```

`cmp` exits successfully with no output. Both hashes are
`db74518d744851ec5dbd094d33d9694db9dce64f6005c02b49e07d863c4dfcef`.
This verifies the full 12,621,516-byte specimen, not just the edited scale.
Undo is bound to the exact edited revision; an independently modified target
must not be silently accepted as the same experiment.

## 8. Turn observations into better experiments

The most interesting next question is usually one level above the evidence you
already have. Make the missing evidence explicit:

| Question | What this lab establishes | What the next experiment needs |
|---|---|---|
| Is expert 27 a “coding expert”? | Its stored matrices and their statistics | Token-to-expert routing traces across a labeled corpus, compared with other experts |
| Did the two-byte edit hurt quality? | Exact storage change and logical column intervention | A compatible runtime, fixed prompts or dataset, baseline, routing evidence, and a defined metric |
| Is four bpw accurate? | The decoded representation and its footprint | Matched higher-precision reference weights or outputs, with layout and revision alignment |
| Can this model fit in memory? | Selected payload and original shard sizes | All resident weights, runtime buffers, attention state, context length, placement, and batch policy |
| Are two conversions equivalent? | Equality or difference for acquired fields | Matched logical coordinates and transforms; then runtime comparison if behavior is the claim |

Expert specialization is an empirical question. A useful research lead is
[Park and Park, *How Many Experts Are Enough?*](https://arxiv.org/abs/2512.19765),
which studies semantic differentiation and adaptive expert allocation. This was
checked through LibSearch, **file_id 344264, pages 1–2**. Its method is not a
description of GLM's training or router; it suggests questions to ask rather
than labels to assign from a tensor's name.

### Five concrete extensions

1. **Cross a transform boundary.** Run the reference checker with
   `--origin 128,128`. Repeat with `--projection gate` and `--projection down`.
   The down projection reverses input/output extents. Check that your reasoning
   follows the matrix axes rather than the words “row” and “column” alone.
2. **Inspect another expert.** Acquire `--expert 28 --output "$LAB/expert28"`.
   Compare corresponding full scale fields and decoded blocks. State whether
   differences are between experts, projections, or quantization variants;
   those are different experiments. Each invocation downloads its own metadata.
3. **Compare raw and logical sparsity.** Count zeros in the packed I16 field,
   then in a decoded block. Explain why either count alone cannot establish how
   often this expert is selected or how sparse its activations are.
4. **Try a trellis-bit intervention on a fresh copy.** Predict the affected
   overlapping windows and undo the lane permutation before making a logical
   claim. At four bpw, one stream bit participates in four codebook windows;
   codebook collisions can reduce the number of changed decoded values. The
   Hadamard transforms can spread a local `Wq` change within its logical block.
   Verify the coordinates, not just a histogram.
5. **Plan an activation experiment before renting hardware.** Define the
   projection input capture, token positions, selected-expert IDs, baseline
   output, precision, and error metric you would need. For this scale ablation,
   check `Δz = −W[:,0] x[0]` at the linear projection first. Only then ask about
   downstream logits or task accuracy. This lab does not provision a runtime.

For the first extension, these commands are ready to run:

```bash
python3 examples/glm53_reference.py "$SAMPLE" \
  --binfiddle "$BF" --catalog "$CAT" --origin 128,128
python3 examples/glm53_reference.py "$SAMPLE" \
  --binfiddle "$BF" --catalog "$CAT" --projection gate
python3 examples/glm53_reference.py "$SAMPLE" \
  --binfiddle "$BF" --catalog "$CAT" --projection down
```

## Reproduction record and implementation links

The default acquisition and all command excerpts were exercised on 2026-10-09.
The sample, edited/restored copies, source snapshots, manifests, and detailed
captures used to write this article live in the author's scratch workspace,
`/media/araray/merlin/sandbox/binfiddle/glm53-lab/`. They are not required local
fixtures: the pinned acquisition script reconstructs the specimen for readers.
No model payload is checked into this repository.

- [Acquisition helper](../examples/glm53_expert_sample.py): bounded transfers,
  pinned revision, descriptor checks, and field-level provenance.
- [Independent reference](../examples/glm53_reference.py): inspectable CPU
  decode, transform, comparison, and optional CLI cross-check.
- [EXL3 implementation and regression tests](../src/nn/exl3.rs): bit windows,
  lane mapping, scale order, block coverage, and canonical JSON reports.
- [Acquisition transport tests](../tests/test_glm53_expert_sample.py): exact
  ranges, ignored ranges, truncation, compression, and length mismatches.
- [Variant-specific model pack](../packs/exl3-glm53-tensorfold-4bpw.yaml) and
  [full NN manual](NN_USAGE.md) for selector, catalog, and transactional-edit
  contracts.

Keep the specimen hash, acquisition manifest, binary digest, source revision and
local patch, command arguments, and numerical comparison policy with your own
results. That makes a small exploratory experiment something another person can
actually repeat.
