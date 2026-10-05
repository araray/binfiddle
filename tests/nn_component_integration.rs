//! Integration tests for M8: component selection through packs, head-view
//! slicing (B.4 rows), and the MLP channel prune recipe — real binary.

use std::fs;
use std::path::Path;
use std::process::Command;

fn binfiddle() -> Command {
    Command::new(env!("CARGO_BIN_EXE_binfiddle"))
}

fn run_in(dir: &Path, args: &[&str]) -> (i32, String, String) {
    let output = binfiddle()
        .args(args)
        .current_dir(dir)
        .output()
        .expect("failed to spawn binfiddle");
    (
        output.status.code().unwrap_or(-1),
        String::from_utf8_lossy(&output.stdout).into_owned(),
        String::from_utf8_lossy(&output.stderr).into_owned(),
    )
}

/// Pack with attention bindings + MLP bindings (B-vector attention config;
/// MLP intermediate width 6 for prune tests).
fn write_pack(dir: &Path) {
    let yaml = r#"schema: binfiddle.nn.pack/v1
id: reference.test
version: "1.0.0"
config:
  hidden_size: 4
  num_layers: 8
  num_attention_heads: 2
  head_dim: 3
  intermediate_size: 6
bindings:
  - pattern: "model.layers.{layer}.self_attn.q_proj.weight"
    component: "decoder.layers[{layer}].attention.query_gate"
    kind: query_gate
    shape: ["2*num_attention_heads*head_dim", "hidden_size"]
  - pattern: "model.layers.{layer}.self_attn.o_proj.weight"
    component: "decoder.layers[{layer}].attention.output"
    kind: dense
    shape: ["hidden_size", "num_attention_heads*head_dim"]
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
full_attention_interval: 4
"#;
    fs::write(dir.join("pack.yaml"), yaml).unwrap();
}

/// Model with attention + MLP tensors per layer, values = row index + j.
fn write_model(dir: &Path, layers: usize) {
    let mut tensors: Vec<(String, Vec<u64>)> = Vec::new();
    for layer in 0..layers {
        let l = layer.to_string();
        tensors.push((
            format!("model.layers.{l}.self_attn.q_proj.weight"),
            vec![12, 4],
        ));
        tensors.push((
            format!("model.layers.{l}.self_attn.o_proj.weight"),
            vec![4, 6],
        ));
        tensors.push((format!("model.layers.{l}.mlp.gate_proj.weight"), vec![6, 4]));
        tensors.push((format!("model.layers.{l}.mlp.up_proj.weight"), vec![6, 4]));
        tensors.push((format!("model.layers.{l}.mlp.down_proj.weight"), vec![4, 6]));
    }
    // Distinctive values: element (r, c) of tensor k = 1000*k + 10*r + c.
    let mut body: Vec<u8> = Vec::new();
    let mut spans = Vec::new();
    for (k, (name, shape)) in tensors.iter().enumerate() {
        let begin = body.len();
        let rows = shape[0];
        let cols = shape[1];
        for r in 0..rows {
            for c in 0..cols {
                let value = (1000 * k + 10 * r as usize + c as usize) as f32;
                body.extend_from_slice(&value.to_le_bytes());
            }
        }
        spans.push((name, shape, begin, body.len()));
    }
    let entries: Vec<String> = spans
        .iter()
        .map(|(n, sh, b, e)| {
            let dims: Vec<String> = sh.iter().map(|d| d.to_string()).collect();
            format!(
                "\"{n}\":{{\"dtype\":\"F32\",\"shape\":[{}],\"data_offsets\":[{b},{e}]}}",
                dims.join(",")
            )
        })
        .collect();
    let header = format!("{{{}}}", entries.join(","));
    let mut data = Vec::new();
    data.extend_from_slice(&(header.len() as u64).to_le_bytes());
    data.extend_from_slice(header.as_bytes());
    data.extend_from_slice(&body);
    fs::write(dir.join("model.safetensors"), data).unwrap();
}

#[test]
fn component_select_resolves_families_and_ranges() {
    let dir = tempfile::tempdir().unwrap();
    write_pack(dir.path());
    write_model(dir.path(), 8);
    run_in(
        dir.path(),
        &[
            "-i",
            "model.safetensors",
            "nn",
            "discover",
            "--verify-content",
            "--out-catalog",
            "c.nn.json",
        ],
    );
    // Range over layers + subtree under attention.
    let (code, out, _) = run_in(
        dir.path(),
        &[
            "nn",
            "select",
            "--catalog",
            "c.nn.json",
            "--pack",
            "pack.yaml",
            "--select",
            "decoder.layers[0:2].attention",
            "--out-selection",
            "attn.sel.json",
        ],
    );
    assert_eq!(code, 0, "out: {out}");
    // 2 layers x 2 attention components x 1 tensor each.
    assert!(out.contains("(4 tensors)"), "out: {out}");

    // The saved selection reloads and binds (slice dry-run through it).
    let (code, out, _) = run_in(
        dir.path(),
        &[
            "nn",
            "slice",
            "--catalog",
            "c.nn.json",
            "--selection",
            "attn.sel.json",
            "--dry-run",
        ],
    );
    assert_eq!(code, 0, "out: {out}");
    assert!(out.contains("4 tensors"), "out: {out}");

    // Out-of-range single index is an error.
    let (code, _, err) = run_in(
        dir.path(),
        &[
            "nn",
            "select",
            "--catalog",
            "c.nn.json",
            "--pack",
            "pack.yaml",
            "--select",
            "decoder.layers[8].attention",
        ],
    );
    assert_eq!(code, 2);
    assert!(err.contains("out of range"), "stderr: {err}");

    // Wildcard selects all layers.
    let (code, out, _) = run_in(
        dir.path(),
        &[
            "nn",
            "select",
            "--catalog",
            "c.nn.json",
            "--pack",
            "pack.yaml",
            "--select",
            "decoder.layers[*].mlp",
            "--out-selection",
            "mlp.sel.json",
        ],
    );
    assert_eq!(code, 0);
    assert!(out.contains("(24 tensors)"), "out: {out}"); // 8 layers x 3 mlp tensors
}

#[test]
fn head_view_slice_extracts_b4_rows_exactly() {
    let dir = tempfile::tempdir().unwrap();
    write_pack(dir.path());
    write_model(dir.path(), 8);
    run_in(
        dir.path(),
        &[
            "-i",
            "model.safetensors",
            "nn",
            "discover",
            "--verify-content",
            "--out-catalog",
            "c.nn.json",
        ],
    );
    // Select head 1 of layer 3's fused query/gate weight.
    let (code, out, _) = run_in(
        dir.path(),
        &[
            "nn",
            "select",
            "--catalog",
            "c.nn.json",
            "--pack",
            "pack.yaml",
            "--select",
            "decoder.layers[3].attention.query_gate.heads[1]",
            "--out-selection",
            "h1.sel.json",
        ],
    );
    assert_eq!(code, 0, "out: {out}");
    let (code, out, _) = run_in(
        dir.path(),
        &[
            "nn",
            "slice",
            "--catalog",
            "c.nn.json",
            "--selection",
            "h1.sel.json",
            "--out-dir",
            "bundle",
        ],
    );
    assert_eq!(code, 0, "out: {out}");
    // Manifest carries the logical view.
    let manifest = fs::read_to_string(dir.path().join("bundle/slice.json")).unwrap();
    assert!(
        manifest.contains("query rows [6, 9) and gate rows [9, 12)"),
        "manifest: {manifest}"
    );
    // The member bytes are exactly rows 6..12 of the fused weight. The
    // payload span comes from the product itself (nn show), not hand math.
    let (_, show_out, _) = run_in(
        dir.path(),
        &[
            "nn",
            "show",
            "--catalog",
            "c.nn.json",
            "--tensor",
            "model.layers.3.self_attn.q_proj.weight",
        ],
    );
    let span_line = show_out
        .lines()
        .find(|l| l.contains("payload:"))
        .expect("payload line");
    let start: usize = span_line
        .split_whitespace()
        .nth(1)
        .unwrap()
        .split("..")
        .next()
        .unwrap()
        .parse()
        .unwrap();
    let model = fs::read(dir.path().join("model.safetensors")).unwrap();
    let expected = &model[start + 6 * 4 * 4..start + 12 * 4 * 4];
    let member = fs::read(
        dir.path()
            .join("bundle/weights/model.layers.3.self_attn.q_proj.weight"),
    )
    .unwrap();
    assert_eq!(member.len(), 96); // 6 rows x 4 cols x 4 bytes
    assert_eq!(&member[..], expected);
    // First member value equals the original element (6,0) of that weight:
    // the layer-3 q_proj is tensor index 15 in the fixture ordering.
    let v = f32::from_le_bytes(member[0..4].try_into().unwrap());
    assert_eq!(v, 1000.0 * 15.0 + 10.0 * 6.0);
}

#[test]
fn mlp_channel_prune_rewrites_shapes_and_preserves_the_rest() {
    let dir = tempfile::tempdir().unwrap();
    write_pack(dir.path());
    write_model(dir.path(), 2);
    run_in(
        dir.path(),
        &[
            "-i",
            "model.safetensors",
            "nn",
            "discover",
            "--verify-content",
            "--out-catalog",
            "c.nn.json",
        ],
    );
    let (code, out, _) = run_in(
        dir.path(),
        &[
            "nn",
            "edit",
            "prune",
            "--catalog",
            "c.nn.json",
            "--pack",
            "pack.yaml",
            "--channels",
            "1,3",
            "--out-model",
            "pruned.safetensors",
        ],
    );
    assert_eq!(code, 0, "out: {out}");
    for layer in 0..2 {
        for name in ["gate", "up"] {
            assert!(
                out.contains(&format!(
                    "model.layers.{layer}.mlp.{name}_proj.weight: [6, 4] -> [4, 4]"
                )),
                "out: {out}"
            );
        }
        assert!(
            out.contains(&format!(
                "model.layers.{layer}.mlp.down_proj.weight: [4, 6] -> [4, 4]"
            )),
            "out: {out}"
        );
    }
    assert!(out.contains("untouched tensors: 4"), "out: {out}"); // 2 layers x 2 attention

    // Output reparses with the new shapes.
    run_in(
        dir.path(),
        &[
            "-i",
            "pruned.safetensors",
            "nn",
            "discover",
            "--out-catalog",
            "p.nn.json",
        ],
    );
    let (code, out, _) = run_in(
        dir.path(),
        &[
            "nn",
            "analyze",
            "--catalog",
            "p.nn.json",
            "--tensor",
            "model.layers.0.mlp.gate_proj.weight",
        ],
    );
    assert_eq!(code, 0, "out: {out}");
    assert!(out.contains("16 of 16"), "out: {out}"); // 4x4 elements

    // Kept gate values: rows 0,2,4,5 of the original. The payload span
    // comes from the product itself (nn show on the pruned catalog).
    let (_, show_out, _) = run_in(
        dir.path(),
        &[
            "nn",
            "show",
            "--catalog",
            "p.nn.json",
            "--tensor",
            "model.layers.0.mlp.gate_proj.weight",
        ],
    );
    let span_line = show_out
        .lines()
        .find(|l| l.contains("payload:"))
        .expect("payload line");
    let gate: usize = span_line
        .split_whitespace()
        .nth(1)
        .unwrap()
        .split("..")
        .next()
        .unwrap()
        .parse()
        .unwrap();
    let pruned = fs::read(dir.path().join("pruned.safetensors")).unwrap();
    let first = f32::from_le_bytes(pruned[gate..gate + 4].try_into().unwrap());
    assert_eq!(first, 2000.0); // original row 0 kept (gate = tensor index 2)
    let second_row = f32::from_le_bytes(pruned[gate + 16..gate + 20].try_into().unwrap());
    assert_eq!(second_row, 2020.0); // row 2 kept (row 1 removed)

    // Untouched q_proj of layer 0 is byte-identical; spans again from show.
    let (_, show_orig, _) = run_in(
        dir.path(),
        &[
            "nn",
            "show",
            "--catalog",
            "c.nn.json",
            "--tensor",
            "model.layers.0.self_attn.q_proj.weight",
        ],
    );
    let orig_span = show_orig
        .lines()
        .find(|l| l.contains("payload:"))
        .unwrap()
        .split_whitespace()
        .nth(1)
        .unwrap()
        .split("..")
        .next()
        .unwrap()
        .parse::<usize>()
        .unwrap();
    let (_, show_pruned, _) = run_in(
        dir.path(),
        &[
            "nn",
            "show",
            "--catalog",
            "p.nn.json",
            "--tensor",
            "model.layers.0.self_attn.q_proj.weight",
        ],
    );
    let pruned_span = show_pruned
        .lines()
        .find(|l| l.contains("payload:"))
        .unwrap()
        .split_whitespace()
        .nth(1)
        .unwrap()
        .split("..")
        .next()
        .unwrap()
        .parse::<usize>()
        .unwrap();
    let model = fs::read(dir.path().join("model.safetensors")).unwrap();
    let pruned_bytes = fs::read(dir.path().join("pruned.safetensors")).unwrap();
    assert_eq!(
        pruned_bytes[pruned_span..pruned_span + 12 * 4 * 4],
        model[orig_span..orig_span + 12 * 4 * 4]
    );
}

#[test]
fn prune_rejects_bad_channels_and_existing_outputs() {
    let dir = tempfile::tempdir().unwrap();
    write_pack(dir.path());
    write_model(dir.path(), 1);
    run_in(
        dir.path(),
        &[
            "-i",
            "model.safetensors",
            "nn",
            "discover",
            "--verify-content",
            "--out-catalog",
            "c.nn.json",
        ],
    );
    // Out-of-range channel.
    let (code, _, err) = run_in(
        dir.path(),
        &[
            "nn",
            "edit",
            "prune",
            "--catalog",
            "c.nn.json",
            "--pack",
            "pack.yaml",
            "--channels",
            "9",
            "--out-model",
            "x.safetensors",
        ],
    );
    assert_eq!(code, 2);
    assert!(err.contains("out of range"), "stderr: {err}");
    // Unsorted.
    let (code, _, err) = run_in(
        dir.path(),
        &[
            "nn",
            "edit",
            "prune",
            "--catalog",
            "c.nn.json",
            "--pack",
            "pack.yaml",
            "--channels",
            "3,1",
            "--out-model",
            "x.safetensors",
        ],
    );
    assert_eq!(code, 2);
    assert!(err.contains("sorted and unique"), "stderr: {err}");
    // Removing every channel.
    let (code, _, err) = run_in(
        dir.path(),
        &[
            "nn",
            "edit",
            "prune",
            "--catalog",
            "c.nn.json",
            "--pack",
            "pack.yaml",
            "--channels",
            "0,1,2,3,4,5",
            "--out-model",
            "x.safetensors",
        ],
    );
    assert_eq!(code, 2);
    assert!(err.contains("cannot remove all"), "stderr: {err}");
    // Existing output.
    fs::write(dir.path().join("exists.bin"), b"x").unwrap();
    let (code, _, err) = run_in(
        dir.path(),
        &[
            "nn",
            "edit",
            "prune",
            "--catalog",
            "c.nn.json",
            "--pack",
            "pack.yaml",
            "--channels",
            "0",
            "--out-model",
            "exists.bin",
        ],
    );
    assert_eq!(code, 2);
    assert!(err.contains("already exists"), "stderr: {err}");
}

#[test]
fn heads_requires_query_gate_components() {
    let dir = tempfile::tempdir().unwrap();
    write_pack(dir.path());
    write_model(dir.path(), 1);
    run_in(
        dir.path(),
        &[
            "-i",
            "model.safetensors",
            "nn",
            "discover",
            "--out-catalog",
            "c.nn.json",
        ],
    );
    let (code, _, err) = run_in(
        dir.path(),
        &[
            "nn",
            "select",
            "--catalog",
            "c.nn.json",
            "--pack",
            "pack.yaml",
            "--select",
            "decoder.layers[0].mlp.gate.heads[0]",
        ],
    );
    assert_eq!(code, 2);
    assert!(err.contains("query_gate"), "stderr: {err}");
    // Head out of range.
    let (code, _, err) = run_in(
        dir.path(),
        &[
            "nn",
            "select",
            "--catalog",
            "c.nn.json",
            "--pack",
            "pack.yaml",
            "--select",
            "decoder.layers[0].attention.query_gate.heads[2]",
        ],
    );
    assert_eq!(code, 2);
    assert!(err.contains("num_attention_heads 2"), "stderr: {err}");
}
