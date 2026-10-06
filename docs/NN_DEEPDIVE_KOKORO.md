# Deep Dive: Dissecting Kokoro-82M with the `nn` Workbench

A complete, real-model walkthrough of the `nn` command family. Every command
below was executed against the actual public artifacts; every output shown is
a real capture. Nothing was executed from the model itself — that is the
point.

> Companion guides: [NN_USAGE.md](NN_USAGE.md) (reference for every command),
> [NN_QUICK_REFERENCE.md](NN_QUICK_REFERENCE.md) (cheat sheet).

## The subject

[Kokoro-82M](https://huggingface.co/hexgrad/Kokoro-82M) is a popular
82-million-parameter text-to-speech model (Apache-2.0, derived from
StyleTTS2). Its release package is a great dissection target because it is
*typical*: a PyTorch checkpoint, a JSON config, a swarm of small voice files
— and, alongside it, a faithful ONNX export family in
[onnx-community/Kokoro-82M-v1.0-ONNX](https://huggingface.co/onnx-community/Kokoro-82M-v1.0-ONNX)
with fp32, fp16, and quantized variants of the same weights.

| Artifact | Role | Size |
|---|---|---|
| `kokoro-v1_0.pth` | The model checkpoint (PyTorch pickle inside a ZIP) | 312 MiB |
| `config.json` | Model/package configuration | 2.3 KiB |
| `voices/*.pt` | 54 per-speaker style vectors | ~0.5 KiB each |
| `onnx/model.onnx` | Full-precision export | 311 MiB |
| `onnx/model_fp16.onnx` | Half-precision export | 156 MiB |
| `onnx/model_q8f16.onnx` | 8-bit + fp16 quantized export | 82 MiB |
| `tokenizer.json` | Phoneme vocabulary | 3.4 KiB |

## Part 1 — First contact: what is in this package?

Point discovery at the directory. Descriptor-only: no payload bytes are read,
nothing is executed.

```bash
binfiddle -i kokoro-base/ nn discover
```

```text
kokoro-base/README.md: asset (documentation)
kokoro-base/config.json: asset (json-configuration)
kokoro-base/kokoro-v1_0.pth: asset (opaque)
kokoro-base/voices/af_heart.pt: asset (opaque)
...
coverage: 0 of 0 sources parsed, 0 problematic, 0 symlinks skipped
```

Everything is visible and classified — and zero model sources were parsed,
because **`.pth` is a pickle and the workbench never executes pickle**.
Executing deserialization code found inside a downloaded file is exactly the
attack surface a static tool must not have.

> **Since v0.29:** `.pth`/`.pt`/`.ckpt` checkpoints inventory natively
> through a **data-only pickle opcode reader** (see
> [NN_USAGE.md](NN_USAGE.md) — PyTorch descriptor tier). The stream is read
> as pure data — strings, integers, containers; `REDUCE` is recognized for
> tensor rebuild functions and *read as data*, never called. This very
> checkpoint inventories as 548 tensors with exact storage spans, and every
> voice pack as its single style vector. The boundary below is unchanged:
> nothing executes, ever.

Validation draws the same line, loudly:

```bash
binfiddle -i kokoro-base/ nn validate
```

```text
artifact validation
  behavior: behavior_not_evaluated
  claims: structural verdicts per reader only; not a safety or behavioral assessment
error: validation failed: one or more sources failed structural validation
```

Exit code 7. A package with **zero readable model sources is a validation
failure by design** — "nothing was validated" must never look like "all
valid."

## Part 2 — The pickle boundary: opaque does not mean unmeasurable

Classic binfiddle never left. A PyTorch checkpoint is a ZIP archive, and ZIP
is a *format* — measurable without executing anything. This is also how the
descriptor tier above works underneath; here it is by hand, with the classic
byte-level tools.

```bash
binfiddle -i kokoro-v1_0.pth search "504b0304" --all --count
```

```text
510
```

510 ZIP local-file headers. The first entry's name is right there in the
header:

```text
00000000: 504b 0304 0000 0808 ...  6d6f 6465 6c2f 6461 7461  PK......model/data
00000020: 2e70 6b6c ...             .pkl
```

`model/data.pkl` — the manifest that tells PyTorch which tensor lives at
which storage offset. We will not run it, but we can *read it as text*,
because pickle stores its strings verbatim:

```bash
binfiddle -i kokoro-v1_0.pth --format ascii read 0x48..0xd0
```

```text
...bertq.ccollec
tions.OrderedDic
t.q.)Rq.(X(...mo
dule.embeddings.
word_embeddings.
weightq.ctorch._
utils._rebuild_t
ensor_v2...
```

Tensor names, in the clear: the state dictionary starts at
`bert.embeddings.word_embeddings.weight`, rebuilt via
`torch._utils._rebuild_tensor_v2`. Counting subsystem prefixes across the
manifest:

```bash
for term in bert text_encoder decoder predictor; do
  binfiddle -i kokoro-v1_0.pth search "$term" --input-format ascii --all --count
done
```

```text
bert           66
text_encoder   42
decoder        2
predictor      1
```

And the archive's own end-of-central-directory confirms the entry count
statically — `fe 01` little-endian is 510, matching the 510 local headers:

```bash
binfiddle -i kokoro-v1_0.pth --show-offset read 327212180..327212226
```

```text
0x1380dc94: 00 00 00 00 50 4b 06 07 ... 50 4b 05 06 00 00 00 00
0x1380dcb4: fe 01 fe 01 28 77 00 00 38 65 80 13 00 00
```

The voice packs get the same treatment — `voices/af_heart.pt` is a tiny ZIP
whose pickle names `torch.FloatStorage`: one f32 style vector per speaker,
readable without running a line of it. Finally, pin the artifact's identity
the classic way:

```bash
binfiddle -i kokoro-v1_0.pth hash blake3
```

```text
03a6ebddf6b50c9969e87aa0e04dc912c85d6ad87c13b66f2b5bbfa88b89fc93
```

That is the whole honest story of an unsupported container: structure,
names, counts, and identity — measured, never executed.

## Part 3 — The same model in a supported container

The ONNX export carries the same weights in a format the workbench reads at
its descriptor tier. Now the full machinery applies:

```bash
binfiddle -i onnx/model.onnx nn discover --verify-content --out-catalog fp32.nn.json
```

```text
onnx/model.onnx: parsed
  format: onnx ir9
  validity: valid
  tensors: 554
  tensor encoder.bert.embeddings.word_embeddings.weight [178x128] 22784 elements, encoding onnx.float
  ...
coverage: 1 of 1 sources parsed, 0 problematic, 0 symlinks skipped
```

554 tensors — and the very first name confirms the pickle evidence from
Part 2: `encoder.bert.embeddings.word_embeddings.weight` is exactly the
`bert.embeddings.word_embeddings.weight` we read out of the ZIP.

What dominates the model?

```bash
binfiddle nn ls --catalog fp32.nn.json --sort bytes --limit 6
```

```text
#    name                         encoding    elements            bytes
1    /decoder/decoder/decode.0/M… onnx.float  3348480 [1024x1090x3] 13393920
2    /decoder/decoder/decode.1/M… onnx.float  3348480 [1024x1090x3] 13393920
3    /decoder/decoder/decode.2/M… onnx.float  3348480 [1024x1090x3] 13393920
4    /decoder/decoder/decode.0/M… onnx.float  3145728 [1024x1024x3] 12582912
...
```

The biggest weights are the decoder's LSTM projection stacks —
1024×1090 fused matmuls, three blocks deep. A single record with evidence:

```bash
binfiddle nn show --catalog fp32.nn.json --tensor encoder.bert.embeddings.word_embeddings.weight --explain
```

```text
tensor encoder.bert.embeddings.word_embeddings.weight
  id:       tensor:5c967f075236997547f790fcb62cb0952b71857d72af4867b7f8928d805aca82
  encoding: onnx.float
  shape:    [178 x 128] (22784 elements)
  payload:  660386..751522 (91136 bytes, exact)
  decoding: supported (qualified numeric decoder registered)
  source:   onnx/model.onnx (onnx ir9)
  evidence: name/shape/encoding/span observed from the container; architectural role unknown without a model pack
```

### Addressing, down to the byte

```bash
binfiddle nn where --catalog fp32.nn.json \
    --tensor encoder.bert.embeddings.word_embeddings.weight --index 177,127
```

```text
  precision: exact_contiguous
  file span: [751518, 751522)
```

And the reverse — offset `0x10240` lands in the protobuf graph region, owned
by no tensor, and the answer says so:

```bash
binfiddle nn locate --catalog fp32.nn.json --offset 0x10240
```

```text
offset 66112 (0x10240):
  no exact-extent tensor owns this offset
```

### Reading the numbers

```bash
binfiddle nn analyze --catalog fp32.nn.json \
    --tensor encoder.bert.embeddings.word_embeddings.weight --histogram-bins 8
```

```text
analyze encoder.bert.embeddings.word_embeddings.weight (onnx.float)
  coverage: mode full — 22784 of 22784 elements examined, 91136 bytes read (completed)
  finite:   22784
  mean:     0.00007190023695107188
  variance: 0.003689844424327504 (population)
  min:      -0.3370845317840576 at [62, 46] (+127 ties)
  max:      0.32849422097206116 at [29, 72] (+127 ties)
  L2 norm:  9.168943949498773
  histogram: 8 bins over [-0.3370845317840576, 0.32849422097206116]
    [-0.337085, -0.253887): 21
    [-0.087492, -0.004295): 8375
    [-0.004295, 0.078902): 10794
    ...
```

A trained embedding table: near-zero-centered, tightly packed. And one real
finding hiding in the ties: **+127 ties on both the minimum and the maximum**
— one full row of 128 identical minima and another row of identical maxima.
Two constant embedding rows is classic padding/bias-row behavior, visible
without running the model. (Sample mode on a big LSTM weight works the same
way with `--mode sample --seed 17 --sample-size 5000`, stating its
`SAMPLE_INCOMPLETE` coverage honestly.)

## Part 4 — Extract and edit for real

Extraction round-trips on the ONNX like any supported container:

```bash
binfiddle nn select --catalog fp32.nn.json \
    --tensor encoder.bert.embeddings.word_embeddings.weight --out-selection bert.sel.json
binfiddle nn slice --catalog fp32.nn.json --selection bert.sel.json --out-dir slices-bert
binfiddle nn assemble --bundle slices-bert --out-dir rebuilt-bert
```

```text
  encoder.bert.embeddings.word_embeddings.weight (91136 bytes, digest verified)
  guarantee: tensor_content (original bytes and executability are NOT claimed)
```

A transactional edit on the 311 MiB model — plan, apply to a fresh file,
undo back to byte-identical:

```bash
binfiddle nn edit set --catalog fp32.nn.json \
    --tensor encoder.bert.embeddings.word_embeddings.weight \
    --index 0,0 --value 0.5 --save-plan emb.plan.json
```

```text
edit plan
  tensor:    encoder.bert.embeddings.word_embeddings.weight [0,0]
  encoding:  onnx.float
  write:     bytes [660386, 660390)
  change:    0 -> 0.5 (requested value 0.5)
  bytes:     00000000 -> 0000003f
```

```bash
binfiddle nn edit apply --catalog fp32.nn.json --plan emb.plan.json \
    --out-model model-edited.onnx --undo-bundle undo/
binfiddle nn edit undo --bundle undo --target model-edited.onnx \
    --out-model model-restored.onnx
cmp onnx/model.onnx model-restored.onnx && echo identical
```

```text
  written:   4 bytes
  preserved: all bytes outside the planned span verified identical
  reparsed: container reparse valid, spans unchanged
identical
```

What does a planned edit touch? Impact keeps dependencies and influence
separate:

```bash
binfiddle nn impact --catalog fp32.nn.json --span 660386..660390
```

```text
impact of [660386, 660390)
  encoder.bert.embeddings.word_embeddings.weight (onnx.float): 1 decode deps,
  1 influenced elements — dense scalar storage: each element owns its 4 bytes;
  read and influence sets coincide
  claims: structural and encoding dependencies only; behavioral consequences are NOT predicted
```

## Part 5 — One model, three precisions

The export family gives a comparison set no fixture can fake. Catalog all
three, then diff fp32 against fp16:

```bash
binfiddle nn diff --left fp32.nn.json --right fp16.nn.json | tail -4
```

```text
  summary: 0 identical, 0 content-changed, 55 repacked, 499 descriptor changes, 0 unmatched, 2 member diffs
  claims: exact, layered; no lineage or behavior claims
```

Two exact layers, no confusion between them:

- **55 repacked** — byte-identical tensors (the int64 constants) sitting at
  different offsets. Same bytes elsewhere is a repack, not a content change.
- **499 descriptor changes** — `encoding onnx.float -> onnx.float16`: the
  weights were converted, so the layer is *descriptor-level* different, and
  the diff never pretends to know how *numerically* different (add
  `--decoded` for scalar content-changed comparisons under strict NaN and
  signed-zero policies).

The evidence graph adds the byte-level relationships:

```bash
binfiddle nn fingerprint --catalog fp32.nn.json --compare fp16.nn.json | head -4
```

```text
fingerprint evidence graph (experimental)
  /decoder/Constant_1_output_0 exact_payload_match
  /decoder/decoder/decode.0/norm1/Mul_1_output_0 exact_payload_match
  ...
  claims: edges assert byte-level relationships only; direction, chronology, and lineage are NOT claimed
```

The quantized export tells its own structural story — `model_q8f16.onnx`
inventories **775 tensors, not 554**: quantization adds machinery, and the
catalog shows it directly (185 `onnx.float16` scale tensors and 45
`onnx.uint8` zero-point tensors named after the ops they calibrate).

Research tooling keeps its honesty on real data too. Row-permutation
alignment only ever answers for dense 2-axis weights:

```bash
binfiddle nn research align --left fp32.nn.json --right fp16.nn.json | head -3
```

```text
row-permutation alignment (experimental)
  /encoder/Constant_3_output_0: ineligible (not dense 2-axis)
  ...
```

And the tokenizer asset is inspected, not executed — including the honesty
of *unknown*:

```bash
binfiddle nn tokenizer inspect --package kokoro-onnx/
```

```text
tokenizer assets
  tokenizer.json [tokenizer_json] 3497 bytes
  tokenizer.json: model unknown, vocab 115, merges -, added tokens 0
  claims: static asset and structure inspection; tokenization behavior and template rendering are NOT evaluated
```

Kokoro's phoneme vocabulary uses a custom model type; the tool reports
`unknown` with the count instead of guessing.

## What the workbench refused to claim

Every step bounded itself, out loud:

- The `.pth` was **measured, never executed** — structure, names, counts,
  digest; the pickle stream is read as data, and no deserialization code
  ever runs.
- Validation of an unreadable package **failed loudly** instead of passing
  vacuously.
- The ONNX was inventoried at the **descriptor tier**; nothing ran.
- Statistics stated their **coverage** (full, or sample with counts).
- Diff separated **content change from repack from descriptor change**;
  fingerprints asserted **byte-level relationships only** — never lineage.
- Editing was **transactional**: preimage-verified, fresh outputs, verified
  undo — with output budgets justified by each operation's provable output
  bound, so models larger than any flat cap still edit cleanly.

That is the whole philosophy in one model: *say what you prove, prove what
you say, and stop there.*

## Reproducing

Commands assume this layout (any equivalent works):

```
kokoro-base/    kokoro-v1_0.pth, config.json, README.md, voices/*.pt
kokoro-onnx/    onnx/{model,model_fp16,model_q8f16}.onnx, tokenizer.json
```

Download from the pinned upstream repositories
(`hexgrad/Kokoro-82M`, `onnx-community/Kokoro-82M-v1.0-ONNX`) — this guide
was written against upstream revisions `f3ff3571` and `1939ad2a`
respectively; re-verification against later revisions is exactly what the
content-verified catalogs are for.
