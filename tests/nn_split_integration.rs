//! Integration tests for `nn split` (REQ-016 decomposition) through the
//! real binary.

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

fn write_pack(dir: &Path) {
    let yaml = r#"schema: binfiddle.nn.pack/v1
id: split.test
version: "1.0.0"
config:
  hidden_size: 4
  num_attention_heads: 2
  head_dim: 3
bindings:
  - pattern: "model.layers.{layer}.self_attn.q_proj.weight"
    component: "decoder.layers[{layer}].attention.query_gate"
    kind: query_gate
    shape: ["2*num_attention_heads*head_dim", "hidden_size"]
  - pattern: "model.layers.{layer}.self_attn.o_proj.weight"
    component: "decoder.layers[{layer}].attention.output"
    kind: dense
    shape: ["hidden_size", "num_attention_heads*head_dim"]
  - pattern: "model.embed_tokens.weight"
    component: "embeddings.tokens"
    kind: dense
    shape: ["hidden_size", "hidden_size"]
"#;
    fs::write(dir.join("pack.yaml"), yaml).unwrap();
}

fn write_model(dir: &Path, layers: usize) {
    let mut tensors: Vec<(String, Vec<u64>)> =
        vec![("model.embed_tokens.weight".to_string(), vec![4, 4])];
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
    fs::write(dir.join("m.safetensors"), data).unwrap();
}

#[test]
fn split_by_layer_reference_and_materialized() {
    let dir = tempfile::tempdir().unwrap();
    write_pack(dir.path());
    write_model(dir.path(), 3);
    run_in(
        dir.path(),
        &[
            "-i",
            "m.safetensors",
            "nn",
            "discover",
            "--verify-content",
            "--out-catalog",
            "c.nn.json",
        ],
    );

    // Reference split: child selections + plans + root manifest.
    let (code, out, _) = run_in(
        dir.path(),
        &[
            "nn",
            "split",
            "--catalog",
            "c.nn.json",
            "--pack",
            "pack.yaml",
            "--by",
            "layer",
            "--storage",
            "reference",
            "--out-dir",
            "split",
        ],
    );
    assert_eq!(code, 0, "out: {out}");
    assert!(
        out.contains("layer 0: 2 tensors, 288 bytes — reference (plan saved)"),
        "out: {out}"
    );
    assert!(out.contains("unresolved: 1 tensors"), "out: {out}");
    assert!(out.contains("unique bytes: 864"), "out: {out}");
    assert!(out.contains("NOT claimed"), "out: {out}");
    assert!(dir.path().join("split/split.json").exists());
    assert!(dir
        .path()
        .join("split/selections/layer-00001.sel.json")
        .exists());

    // Child selections are ordinary selections: slice works from them.
    let (code, out, _) = run_in(
        dir.path(),
        &[
            "nn",
            "slice",
            "--catalog",
            "c.nn.json",
            "--selection",
            "split/selections/layer-00001.sel.json",
            "--out-dir",
            "layer1-bundle",
        ],
    );
    assert_eq!(code, 0, "out: {out}");

    // Root manifest carries the accounting and coverage partition.
    let manifest = fs::read_to_string(dir.path().join("split/split.json")).unwrap();
    assert!(manifest.contains("\"member_bytes\":\"864\""), "{manifest}");
    assert!(manifest.contains("\"unique_bytes\":\"864\""), "{manifest}");
    assert!(
        manifest.contains("\"unresolved\":[\"model.embed_tokens.weight\"]"),
        "{manifest}"
    );
    assert!(
        manifest.contains("never duplicated by navigation overlap"),
        "{manifest}"
    );

    // Materialized split: per-layer bundles with digest-verified members.
    let (code, out, _) = run_in(
        dir.path(),
        &[
            "nn",
            "split",
            "--catalog",
            "c.nn.json",
            "--pack",
            "pack.yaml",
            "--by",
            "layer",
            "--storage",
            "materialized",
            "--out-dir",
            "split-mat",
        ],
    );
    assert_eq!(code, 0, "out: {out}");
    assert!(
        out.contains("materialized (layers/layer-00002)"),
        "out: {out}"
    );
    for layer in 0..3 {
        let bundle = dir
            .path()
            .join(format!("split-mat/layers/layer-{layer:05}"));
        assert!(bundle.join("slice.json").exists());
        assert_eq!(
            std::fs::read_dir(bundle.join("weights")).unwrap().count(),
            2,
            "layer {layer}"
        );
    }

    // Fresh-directory enforcement.
    let (code, _, err) = run_in(
        dir.path(),
        &[
            "nn",
            "split",
            "--catalog",
            "c.nn.json",
            "--pack",
            "pack.yaml",
            "--out-dir",
            "split",
        ],
    );
    assert_eq!(code, 2);
    assert!(err.contains("already exists"), "stderr: {err}");
}
