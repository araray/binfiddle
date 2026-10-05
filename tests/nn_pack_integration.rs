//! Integration tests for model packs: verify, architecture view, component
//! show, and contradiction retention — through the real binary with the
//! published B.4/B.5/B.6/B.7 reference values as the pack configuration.

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

/// The worked reference pack, configured with the B-vector dimensions
/// (H=2, D=3, d=4; Nk=2, Nv=4, Dk=3, Dv=2; 8 layers, interval 4).
fn write_pack(dir: &Path) {
    let yaml = r#"schema: binfiddle.nn.pack/v1
id: qwen3-next.reference
version: "1.0.0"
description: worked reference profile (B-vector configuration)
config:
  hidden_size: 4
  num_layers: 8
  num_attention_heads: 2
  head_dim: 3
  num_key_heads: 2
  num_value_heads: 4
  key_head_dim: 3
  value_head_dim: 2
bindings:
  - pattern: "model.layers.{layer}.self_attn.q_proj.weight"
    component: "decoder.layers[{layer}].attention.query_gate"
    kind: query_gate
    shape: ["2*num_attention_heads*head_dim", "hidden_size"]
  - pattern: "model.layers.{layer}.self_attn.o_proj.weight"
    component: "decoder.layers[{layer}].attention.output"
    kind: dense
    shape: ["hidden_size", "num_attention_heads*head_dim"]
  - pattern: "model.layers.{layer}.input_layernorm.weight"
    component: "decoder.layers[{layer}].input_norm"
    kind: norm_zero_centered
    shape: ["hidden_size"]
  - pattern: "model.layers.{layer}.linear_attn.in_proj.weight"
    component: "decoder.layers[{layer}].linear_attention.in_projections"
    kind: linear_qkvz_groups
    shape: ["num_key_heads*(2*key_head_dim + 2*(num_value_heads/num_key_heads)*value_head_dim)", "hidden_size"]
  - pattern: "model.layers.{layer}.linear_attn.ba_proj.weight"
    component: "decoder.layers[{layer}].linear_attention.ba"
    kind: linear_ba_groups
    shape: ["2*num_value_heads", "hidden_size"]
full_attention_interval: 4
"#;
    fs::write(dir.join("pack.yaml"), yaml).unwrap();
}

/// Build a safetensors model with exactly the B-vector shapes for `layers`
/// layers (F32; payload bytes are always shape-product × 4).
fn write_model(dir: &Path, name: &str, layers: usize, q_proj_rows: Option<u64>) {
    let q_rows = q_proj_rows.unwrap_or(12); // 2*H*D = 2*2*3
    let mut tensors: Vec<(String, Vec<u64>)> = Vec::new();
    for layer in 0..layers {
        let l = layer.to_string();
        tensors.push((
            format!("model.layers.{l}.self_attn.q_proj.weight"),
            vec![q_rows, 4],
        ));
        tensors.push((
            format!("model.layers.{l}.self_attn.o_proj.weight"),
            vec![4, 6],
        ));
        tensors.push((format!("model.layers.{l}.input_layernorm.weight"), vec![4]));
        tensors.push((
            format!("model.layers.{l}.linear_attn.in_proj.weight"),
            vec![28, 4], // Nk * P = 2 * 14
        ));
        tensors.push((
            format!("model.layers.{l}.linear_attn.ba_proj.weight"),
            vec![8, 4], // 2*Nv
        ));
    }
    let mut body: Vec<u8> = Vec::new();
    let mut spans = Vec::new();
    for (tensor_name, shape) in &tensors {
        let begin = body.len();
        let bytes: usize = shape.iter().product::<u64>() as usize * 4;
        body.extend(std::iter::repeat_n(0u8, bytes));
        spans.push((tensor_name, shape, begin, begin + bytes));
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
    fs::write(dir.join(name), data).unwrap();
}

#[test]
fn pack_verify_reports_identity() {
    let dir = tempfile::tempdir().unwrap();
    write_pack(dir.path());
    let (code, out, _) = run_in(dir.path(), &["nn", "pack", "verify", "--pack", "pack.yaml"]);
    assert_eq!(code, 0, "out: {out}");
    assert!(
        out.contains("pack verified: qwen3-next.reference v1.0.0"),
        "out: {out}"
    );
    assert!(out.contains("5 bindings"), "out: {out}");

    let (code, out, _) = run_in(
        dir.path(),
        &[
            "nn",
            "pack",
            "verify",
            "--pack",
            "pack.yaml",
            "--report-format",
            "json",
        ],
    );
    assert_eq!(code, 0);
    assert!(out.contains("\"operation\":\"pack verify\""));
    assert!(out.contains("pack:"));

    // Malformed schema is rejected.
    fs::write(dir.path().join("bad.yaml"), "schema: something.else").unwrap();
    let (code, _, err) = run_in(dir.path(), &["nn", "pack", "verify", "--pack", "bad.yaml"]);
    assert_eq!(code, 4);
    assert!(err.contains("schema mismatch"), "stderr: {err}");
}

#[test]
fn architecture_view_recognizes_components_and_schedule() {
    let dir = tempfile::tempdir().unwrap();
    write_pack(dir.path());
    write_model(dir.path(), "model.safetensors", 8, None);
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
    let (code, out, _) = run_in(
        dir.path(),
        &[
            "nn",
            "ls",
            "--catalog",
            "c.nn.json",
            "--view",
            "architecture",
            "--pack",
            "pack.yaml",
        ],
    );
    assert_eq!(code, 0, "out: {out}");
    // B.6: the interval fallback marks layers [3, 7] full-attention.
    assert!(out.contains("full-attention layers: [3, 7]"), "out: {out}");
    // B.4-shaped components resolved.
    assert!(
        out.contains("decoder.layers[3].attention.query_gate [query_gate]"),
        "out: {out}"
    );
    assert!(
        out.contains("decoder.layers[0].linear_attention.in_projections [linear_qkvz_groups]"),
        "out: {out}"
    );
    // No contradictions or unassigned tensors in the happy path.
    assert!(!out.contains("contradictions:"), "out: {out}");
    assert!(!out.contains("unassigned tensors"), "out: {out}");

    // JSON envelope shape.
    let (code, out, _) = run_in(
        dir.path(),
        &[
            "nn",
            "ls",
            "--catalog",
            "c.nn.json",
            "--view",
            "architecture",
            "--pack",
            "pack.yaml",
            "--report-format",
            "json",
        ],
    );
    assert_eq!(code, 0);
    assert!(
        out.contains("\"full_attention_layers\":[\"3\",\"7\"]"),
        "out: {out}"
    );
    assert!(out.contains("\"contradictions\":\"0\""));
    assert!(out.contains("\"unassigned\":\"0\""));
}

#[test]
fn component_show_prints_reference_layout_maps() {
    let dir = tempfile::tempdir().unwrap();
    write_pack(dir.path());
    write_model(dir.path(), "model.safetensors", 8, None);
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
    // B.4: head row maps of the fused query/gate projection.
    let (code, out, _) = run_in(
        dir.path(),
        &[
            "nn",
            "show",
            "--catalog",
            "c.nn.json",
            "--pack",
            "pack.yaml",
            "--component",
            "decoder.layers[3].attention.query_gate",
        ],
    );
    assert_eq!(code, 0, "out: {out}");
    assert!(
        out.contains("head 0: query rows [0, 3), gate rows [3, 6)"),
        "out: {out}"
    );
    assert!(
        out.contains("head 1: query rows [6, 9), gate rows [9, 12)"),
        "out: {out}"
    );

    // B.5: GatedDeltaNet group maps and the convolution distinction note.
    let (code, out, _) = run_in(
        dir.path(),
        &[
            "nn",
            "show",
            "--catalog",
            "c.nn.json",
            "--pack",
            "pack.yaml",
            "--component",
            "decoder.layers[0].linear_attention.in_projections",
        ],
    );
    assert_eq!(code, 0, "out: {out}");
    assert!(
        out.contains("group 0: q [0, 3) k [3, 6) v [6, 10) z [10, 14)"),
        "out: {out}"
    );
    assert!(
        out.contains("group 1: q [14, 17) k [17, 20) v [20, 24) z [24, 28)"),
        "out: {out}"
    );
    assert!(
        out.contains("do NOT describe convolution weights"),
        "out: {out}"
    );

    // B.5: ba groups.
    let (code, out, _) = run_in(
        dir.path(),
        &[
            "nn",
            "show",
            "--catalog",
            "c.nn.json",
            "--pack",
            "pack.yaml",
            "--component",
            "decoder.layers[0].linear_attention.ba",
        ],
    );
    assert_eq!(code, 0, "out: {out}");
    assert!(
        out.contains("group 0: b rows [0, 2), a rows [2, 4)"),
        "out: {out}"
    );
    assert!(
        out.contains("group 1: b rows [4, 6), a rows [6, 8)"),
        "out: {out}"
    );

    // B.7: the zero-centered norm lens.
    let (code, out, _) = run_in(
        dir.path(),
        &[
            "nn",
            "show",
            "--catalog",
            "c.nn.json",
            "--pack",
            "pack.yaml",
            "--component",
            "decoder.layers[0].input_norm",
        ],
    );
    assert_eq!(code, 0, "out: {out}");
    assert!(
        out.contains("effective scale = 1 + stored weight"),
        "out: {out}"
    );

    // Unknown component path is a clean miss.
    let (code, _, err) = run_in(
        dir.path(),
        &[
            "nn",
            "show",
            "--catalog",
            "c.nn.json",
            "--pack",
            "pack.yaml",
            "--component",
            "decoder.layers[99].attention",
        ],
    );
    assert_eq!(code, 5);
    assert!(err.contains("no component"), "stderr: {err}");
}

#[test]
fn contradictions_are_retained_not_hidden() {
    let dir = tempfile::tempdir().unwrap();
    write_pack(dir.path());
    // q_proj with a WRONG shape (13 rows instead of 12): the name matches,
    // the shape contradicts. The tensor must stay visible as a contradiction.
    write_model(dir.path(), "model.safetensors", 2, Some(13));
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
    let (code, out, _) = run_in(
        dir.path(),
        &[
            "nn",
            "ls",
            "--catalog",
            "c.nn.json",
            "--view",
            "architecture",
            "--pack",
            "pack.yaml",
        ],
    );
    assert_eq!(code, 0);
    assert!(
        out.contains("model.layers.0.self_attn.q_proj.weight matched"),
        "out: {out}"
    );
    assert!(out.contains("[13, 4] != [12, 4]"), "out: {out}");

    // The tensor view still lists it; nothing disappeared. (The text view
    // truncates long names, so match the distinctive fragment.)
    let (code, out, _) = run_in(dir.path(), &["nn", "ls", "--catalog", "c.nn.json"]);
    assert_eq!(code, 0);
    assert!(out.contains("self_attn.q_"), "out: {out}");

    // JSON coverage honestly reports the contradiction.
    let (code, out, _) = run_in(
        dir.path(),
        &[
            "nn",
            "ls",
            "--catalog",
            "c.nn.json",
            "--view",
            "architecture",
            "--pack",
            "pack.yaml",
            "--report-format",
            "json",
        ],
    );
    assert_eq!(code, 0);
    assert!(out.contains("\"contradictions\":\"2\""), "out: {out}");
    assert!(out.contains("\"complete\":false"), "out: {out}");
}

#[test]
fn architecture_view_requires_a_pack() {
    let dir = tempfile::tempdir().unwrap();
    write_model(dir.path(), "model.safetensors", 1, None);
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
            "ls",
            "--catalog",
            "c.nn.json",
            "--view",
            "architecture",
        ],
    );
    assert_eq!(code, 2);
    assert!(err.contains("requires --pack"), "stderr: {err}");
}
