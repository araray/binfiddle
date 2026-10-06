# Binfiddle NN Quick Reference

One-page card for the `nn` workbench. Full guide: [NN_USAGE.md](NN_USAGE.md).
All outputs are fresh files; inputs are never modified.

## Pipeline

```
discover ─→ catalog (.nn.json) ─→ ls / show / select ─→ selection (.sel.json)
                                   │                        │
                                   ↓                        ↓
                        where / locate / impact        slice ─→ bundle ─→ assemble
                                   │                    or split (by layer)
                                   ↓
                          shard-map (download planning)
analyze / diff / fingerprint / partition / carve / validate / adapter / tokenizer / research
edit set ─→ plan ─→ apply ─→ edited file + undo bundle ─→ undo
edit set --session ─→ staged ops ─→ apply --session (one pass) ─→ undo (all ops)
```

## Commands

| Command | One-liner |
|---|---|
| `nn capabilities` | What this build implements (honest self-report) |
| `nn discover` | Inventory SafeTensors/GGUF/ONNX/PyTorch + dirs (`--verify-content`, `--require-complete`, `--out-catalog`) |
| `nn ls` | List tensors/sources/components (`--view`, `--encoding`, `--name-regex`, `--sort`, `--limit/--offset`) |
| `nn show` | One tensor's record (`--tensor` / `--id` / `--component`, `--explain`) |
| `nn select` | Resolve + save a selection (`--tensor`, `--id`, `--select` expr, `--rebind`) |
| `nn where` | Tensor/element → file bytes+bits (`--index`; Q4_0 nibble masks) |
| `nn locate` | File offset → owning tensor(s) (`--offset`) |
| `nn impact` | Span/plan → decode deps vs influence set (`--span`, `--offset`, `--plan`) |
| `nn slice` | Extract selection → bundle (`--storage reference\|materialized`, `--quant preserve_encoding\|cover_blocks\|decode`) |
| `nn assemble` | Bundle → reconstructed content (`--bundle`, `--out-dir`) |
| `nn split` | Layer decomposition (`--pack`, `--storage`, `--out-dir`) |
| `nn analyze` | Stats/histogram/blocks/reference (`--mode metadata\|sample\|full`, `--blocks`, `--reference`); batch over `--selection` with outlier flags |
| `nn edit set` | Plan a value/bit change (`--index`, `--value` / `--raw-bits`, `--policy`) — single `--save-plan` or stage into `--session` |
| `nn edit apply` | Apply plan or session → fresh file (+ `--undo-bundle`; sessions patch every op in one pass) |
| `nn edit undo` | Reverse via undo bundle (exact revision; reverses all session ops) |
| `nn edit prune` | Remove MLP channels structurally (`--pack`, `--channels`) |
| `nn pack verify / lint / scaffold` | Pack lifecycle (`--pack`; scaffold from `--catalog`, dims via `--config`) |
| `nn diff` | Layered diff (`--left`, `--right`, `--decoded`, `--policy`); `--precision` = per-tensor decoded error metrics |
| `nn fingerprint` | Exact fingerprints (+ `--compare` evidence graph, `--threshold`) |
| `nn partition` | Layer groups per stage (static byte estimate; `--unlayered-policy`) |
| `nn carve` | Find embedded containers in a raw file (`--target`) |
| `nn validate` | Per-source structural verdicts (exit 7 on defects) |
| `nn shard-map` | Which shards hold your selection (`--index`, `--tensor`/`--selection`) — a plan, not a download |
| `nn adapter inspect` | LoRA factor pairs (descriptor-level) |
| `nn tokenizer inspect / diff` | Tokenizer assets / vocabulary diff |
| `nn research align` | Exact row-permutation test (experimental) |

Every command: `--report-format text|json`.

## Common incantations

```bash
binfiddle -i model-dir/ nn discover --verify-content --out-catalog m.nn.json
binfiddle nn ls --catalog m.nn.json --sort bytes --limit 20
binfiddle nn show --catalog m.nn.json --tensor w --explain
binfiddle nn where --catalog m.nn.json --tensor w --index 1,1
binfiddle nn locate --catalog m.nn.json --offset 0x27c
binfiddle nn analyze --catalog m.nn.json --tensor w --histogram-bins 32
binfiddle nn analyze --catalog m.nn.json --selection gates.sel.json --mode sample
binfiddle nn select --catalog m.nn.json --pack p.yaml --select 'decoder.layers[*].mlp' \
    --out-selection mlp.sel.json
binfiddle nn slice --catalog m.nn.json --selection mlp.sel.json --out-dir slices/
binfiddle nn edit set --catalog m.nn.json --tensor w --index 0,0 --value 9 --session sess/
binfiddle nn edit apply --catalog m.nn.json --session sess/ --out-model out.bin --undo-bundle undo/
binfiddle nn diff --left v1.nn.json --right v2.nn.json
binfiddle nn diff --left fp32.nn.json --right fp16.nn.json --precision
binfiddle nn fingerprint --catalog v1.nn.json --compare v2.nn.json
binfiddle nn shard-map --index model.safetensors.index.json --tensor lm_head.weight
```

Negative values: `--value=-0.5` (equals sign — a bare `-0.5` parses as a flag).

## Selector grammar

`decoder.layers[3].attention` · `[8:16]` half-open range · `[1,3,5]` list ·
`[*]` wildcard · shorter path = subtree · `query_gate.heads[N]` = one head's
rows · `experts[N]` = one expert's contiguous slab (moe_expert_stack).

## Exit codes

`0` ok · `2` invalid request · `3` unsupported/ambiguous · `4` malformed/wire ·
`5` source changed/missing/conflict · `6` I/O/budget/publication ·
`7` validation failed · `8` incomplete rejected · `130` cancelled.

## Never forget

No execution, no behavior claims, no lineage claims, no template rendering —
every report says what it proves and stops there. Integers in JSON are decimal
strings; output is canonically serialized and deterministic. PyTorch
checkpoints are read as pure data (never executed) and are read-only.
