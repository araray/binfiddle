//! Integration tests for `nn partition` (static planning) and `nn carve`
//! (artifact carving) through the real binary.

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

/// A pack with layered bindings: 4 layers, per-layer gate/up/down (6 bytes
/// each... F32 [6,4]=96B gate/up, [4,6]=96B down) plus an embedding tensor.
fn write_pack(dir: &Path) {
    let yaml = r#"schema: binfiddle.nn.pack/v1
id: partition.test
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
  - pattern: "model.embed_tokens.weight"
    component: "embeddings.tokens"
    kind: dense
    shape: ["hidden_size", "hidden_size"]
"#;
    fs::write(dir.join("pack.yaml"), yaml).unwrap();
}

fn write_model(dir: &Path) {
    let mut tensors: Vec<(String, Vec<u64>)> =
        vec![("model.embed_tokens.weight".to_string(), vec![4, 4])];
    for layer in 0..4 {
        let l = layer.to_string();
        tensors.push((format!("model.layers.{l}.mlp.gate_proj.weight"), vec![6, 4]));
        tensors.push((format!("model.layers.{l}.mlp.up_proj.weight"), vec![6, 4]));
        tensors.push((format!("model.layers.{l}.mlp.down_proj.weight"), vec![4, 6]));
    }
    let mut body: Vec<u8> = Vec::new();
    let mut spans = Vec::new();
    for (name, shape) in &tensors {
        let begin = body.len();
        let bytes: usize = shape.iter().product::<u64>() as usize * 4;
        body.extend(std::iter::repeat_n(0u8, bytes));
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
fn partition_plans_balanced_contiguous_stages_with_stated_limits() {
    let dir = tempfile::tempdir().unwrap();
    write_pack(dir.path());
    write_model(dir.path());
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
            "partition",
            "--catalog",
            "c.nn.json",
            "--pack",
            "pack.yaml",
            "--stages",
            "2",
        ],
    );
    assert_eq!(code, 0, "out: {out}");
    // 4 identical layers over 2 stages: [0,1] and [2,3].
    assert!(out.contains("stage 0: layers [0,1]"), "out: {out}");
    assert!(out.contains("stage 1: layers [2,3]"), "out: {out}");
    // Per-stage bytes: 3 tensors x 96 bytes x 2 layers = 576.
    assert!(out.contains("576 weight bytes"), "out: {out}");
    // The embedding is unlayered and explicitly listed, never distributed.
    assert!(out.contains("unlayered: 1 tensors"), "out: {out}");
    // The estimate boundary is stated.
    assert!(
        out.contains("activations, workspace, state, and transfers are excluded"),
        "out: {out}"
    );
    assert!(
        out.contains("no performance or speedup claims"),
        "out: {out}"
    );

    // JSON envelope includes the estimates section.
    let (code, out, _) = run_in(
        dir.path(),
        &[
            "nn",
            "partition",
            "--catalog",
            "c.nn.json",
            "--pack",
            "pack.yaml",
            "--stages",
            "2",
            "--report-format",
            "json",
        ],
    );
    assert_eq!(code, 0);
    assert!(out.contains("\"layer_range\":[\"0\",\"1\"]"), "out: {out}");
    assert!(out.contains("runtime evidence"), "out: {out}");

    // More stages than layers clamps to one stage per layer.
    let (code, out, _) = run_in(
        dir.path(),
        &[
            "nn",
            "partition",
            "--catalog",
            "c.nn.json",
            "--pack",
            "pack.yaml",
            "--stages",
            "9",
        ],
    );
    assert_eq!(code, 0);
    assert!(out.contains("stage 3: layers [3]"), "out: {out}");

    // Zero stages is a usage error.
    let (code, _, err) = run_in(
        dir.path(),
        &[
            "nn",
            "partition",
            "--catalog",
            "c.nn.json",
            "--pack",
            "pack.yaml",
            "--stages",
            "0",
        ],
    );
    assert_eq!(code, 2);
    assert!(err.contains("at least one stage"), "stderr: {err}");
}

#[test]
fn carve_finds_embedded_containers_in_a_raw_image() {
    let dir = tempfile::tempdir().unwrap();
    // A real safetensors model surrounded by junk.
    let header = r#"{"w":{"dtype":"F32","shape":[2,2],"data_offsets":[0,16]}}"#;
    let mut model = Vec::new();
    model.extend_from_slice(&(header.len() as u64).to_le_bytes());
    model.extend_from_slice(header.as_bytes());
    model.extend_from_slice(&[0u8; 16]);
    let mut image = Vec::new();
    image.extend(std::iter::repeat_n(0x41u8, 700));
    let model_start = image.len();
    image.extend_from_slice(&model);
    image.extend(std::iter::repeat_n(0x42u8, 200));
    fs::write(dir.path().join("image.bin"), &image).unwrap();

    let (code, out, _) = run_in(dir.path(), &["nn", "carve", "--target", "image.bin"]);
    assert_eq!(code, 0, "out: {out}");
    assert!(
        out.contains(&format!(
            "safetensors at [{}, {})",
            model_start,
            model_start + model.len()
        )),
        "out: {out}"
    );
    assert!(out.contains("[verified] 1 tensors"), "out: {out}");
    assert!(out.contains("asserts model validity"), "out: {out}");

    // JSON form.
    let (code, out, _) = run_in(
        dir.path(),
        &[
            "nn",
            "carve",
            "--target",
            "image.bin",
            "--report-format",
            "json",
        ],
    );
    assert_eq!(code, 0);
    assert!(out.contains("\"confidence\":\"verified\""), "out: {out}");

    // A junk file reports nothing, honestly.
    fs::write(dir.path().join("junk.bin"), vec![0x13u8; 8192]).unwrap();
    let (code, out, _) = run_in(dir.path(), &["nn", "carve", "--target", "junk.bin"]);
    assert_eq!(code, 0);
    assert!(
        out.contains("no model-container signatures found"),
        "out: {out}"
    );
}
