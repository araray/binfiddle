//! Integration tests for `nn adapter inspect` and `nn tokenizer inspect/diff`
//! through the real binary.

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

fn write_safetensors(dir: &Path, file: &str, tensors: &[(&str, Vec<u64>)]) {
    let mut body: Vec<u8> = Vec::new();
    let mut spans = Vec::new();
    for (name, shape) in tensors {
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
    fs::write(dir.join(file), data).unwrap();
}

#[test]
fn adapter_inspect_reports_pairs_and_findings() {
    let dir = tempfile::tempdir().unwrap();
    write_safetensors(
        dir.path(),
        "adapter.safetensors",
        &[
            ("m1.lora_A.weight", vec![4, 8]),
            ("m1.lora_B.weight", vec![6, 4]),
            ("m2.lora_A.weight", vec![4, 8]),
            ("m2.lora_B.weight", vec![6, 5]),
            ("m3.lora_B.weight", vec![6, 4]),
        ],
    );
    run_in(
        dir.path(),
        &[
            "-i",
            "adapter.safetensors",
            "nn",
            "discover",
            "--out-catalog",
            "a.nn.json",
        ],
    );
    let (code, out, _) = run_in(
        dir.path(),
        &["nn", "adapter", "inspect", "--catalog", "a.nn.json"],
    );
    assert_eq!(code, 0, "out: {out}");
    assert!(out.contains("m1 rank 4 [8 -> 6]"), "out: {out}");
    assert!(
        out.contains("finding [RANK_MISMATCH]: target m2"),
        "out: {out}"
    );
    assert!(
        out.contains("orphan B factor: m3.lora_B.weight"),
        "out: {out}"
    );
    assert!(out.contains("NOT verified"), "out: {out}");

    // JSON form.
    let (code, out, _) = run_in(
        dir.path(),
        &[
            "nn",
            "adapter",
            "inspect",
            "--catalog",
            "a.nn.json",
            "--report-format",
            "json",
        ],
    );
    assert_eq!(code, 0);
    assert!(out.contains("\"pair_count\":\"1\""), "out: {out}");
    assert!(out.contains("\"rank\":\"4\""), "out: {out}");
}

#[test]
fn adapter_inspect_reports_non_adapters_honestly() {
    let dir = tempfile::tempdir().unwrap();
    write_safetensors(dir.path(), "model.safetensors", &[("w", vec![2, 2])]);
    run_in(
        dir.path(),
        &[
            "-i",
            "model.safetensors",
            "nn",
            "discover",
            "--out-catalog",
            "m.nn.json",
        ],
    );
    let (code, out, _) = run_in(
        dir.path(),
        &["nn", "adapter", "inspect", "--catalog", "m.nn.json"],
    );
    assert_eq!(code, 0);
    assert!(out.contains("no complete factor pairs found"), "out: {out}");
    assert!(out.contains("NO_FACTOR_PAIRS"), "out: {out}");
}

#[test]
fn tokenizer_inspect_and_diff_round_trip() {
    let dir = tempfile::tempdir().unwrap();
    let pkg = dir.path().join("pkg");
    fs::create_dir_all(&pkg).unwrap();
    let tokenizer_json = r#"{"version":"1.0","added_tokens":[{"id":0,"content":"<s>","special":true}],"model":{"type":"BPE","unk_token":"<unk>","vocab":{"<unk>":0,"a":1,"b":2},"merges":["a b"]}}"#.to_string();
    fs::write(pkg.join("tokenizer.json"), &tokenizer_json).unwrap();
    fs::write(pkg.join("tokenizer_config.json"), b"{}").unwrap();
    fs::write(pkg.join("vocab.json"), b"{}").unwrap();
    fs::write(pkg.join("notes.md"), b"ignored").unwrap();

    let (code, out, _) = run_in(
        dir.path(),
        &["nn", "tokenizer", "inspect", "--package", "pkg"],
    );
    assert_eq!(code, 0, "out: {out}");
    assert!(
        out.contains("tokenizer.json [tokenizer_json]"),
        "out: {out}"
    );
    assert!(
        out.contains("tokenizer_config.json [tokenizer_config]"),
        "out: {out}"
    );
    assert!(out.contains("vocab.json [vocab_json]"), "out: {out}");
    assert!(!out.contains("notes.md"), "out: {out}");
    assert!(
        out.contains("tokenizer.json: model BPE, vocab 3, merges 1, added tokens 1"),
        "out: {out}"
    );
    assert!(out.contains("NOT evaluated"), "out: {out}");

    // Diff: right tokenizer adds tokens c/d and drops b.
    let right_json = r#"{"version":"1.0","added_tokens":[],"model":{"type":"BPE","unk_token":"<unk>","vocab":{"<unk>":0,"a":1,"c":2,"d":3},"merges":[]}}"#.to_string();
    fs::write(dir.path().join("right.json"), &right_json).unwrap();
    let (code, out, _) = run_in(
        dir.path(),
        &[
            "nn",
            "tokenizer",
            "diff",
            "--left",
            "pkg/tokenizer.json",
            "--right",
            "right.json",
        ],
    );
    assert_eq!(code, 0, "out: {out}");
    assert!(
        out.contains("vocab: 3 vs 4 (1 removed, 2 added)"),
        "out: {out}"
    );
    assert!(out.contains("- b"), "out: {out}");
    assert!(out.contains("+ c"), "out: {out}");
    assert!(out.contains("+ d"), "out: {out}");
    assert!(out.contains("NOT inferred"), "out: {out}");

    // Identical tokenizers compare equal.
    let (code, out, _) = run_in(
        dir.path(),
        &[
            "nn",
            "tokenizer",
            "diff",
            "--left",
            "pkg/tokenizer.json",
            "--right",
            "pkg/tokenizer.json",
            "--report-format",
            "json",
        ],
    );
    assert_eq!(code, 0);
    assert!(out.contains("\"same_vocab\":true"), "out: {out}");
}

#[test]
fn tokenizer_inspect_on_empty_directory_is_honest() {
    let dir = tempfile::tempdir().unwrap();
    let pkg = dir.path().join("empty");
    fs::create_dir_all(&pkg).unwrap();
    let (code, out, _) = run_in(
        dir.path(),
        &["nn", "tokenizer", "inspect", "--package", "empty"],
    );
    assert_eq!(code, 0);
    assert!(
        out.contains("no standard tokenizer asset files found"),
        "out: {out}"
    );
}
