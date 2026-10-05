//! Integration tests for `nn impact`, `nn validate`, and `nn profile
//! lint/scaffold` through the real binary.

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

/// One Q4_0 [1,32] GGUF tensor (18-byte block) + helper to find its span.
fn write_q4_model(dir: &Path) {
    let mut out = Vec::new();
    out.extend_from_slice(b"GGUF");
    out.extend_from_slice(&3u32.to_le_bytes());
    out.extend_from_slice(&1u64.to_le_bytes());
    out.extend_from_slice(&1u64.to_le_bytes());
    let key = b"general.architecture";
    out.extend_from_slice(&(key.len() as u64).to_le_bytes());
    out.extend_from_slice(key);
    out.extend_from_slice(&8u32.to_le_bytes());
    out.extend_from_slice(&1u64.to_le_bytes());
    out.push(b't');
    out.extend_from_slice(&1u64.to_le_bytes());
    out.push(b'w');
    out.extend_from_slice(&2u32.to_le_bytes());
    out.extend_from_slice(&1u64.to_le_bytes());
    out.extend_from_slice(&32u64.to_le_bytes());
    out.extend_from_slice(&2u32.to_le_bytes());
    out.extend_from_slice(&0u64.to_le_bytes());
    while out.len() % 32 != 0 {
        out.push(0);
    }
    out.extend_from_slice(&[0x00, 0x38, 0xA3]);
    out.extend(std::iter::repeat_n(0u8, 15));
    fs::write(dir.join("q.gguf"), out).unwrap();
}

#[test]
fn impact_distinguishes_code_and_scale_spans() {
    let dir = tempfile::tempdir().unwrap();
    write_q4_model(dir.path());
    run_in(
        dir.path(),
        &[
            "-i",
            "q.gguf",
            "nn",
            "discover",
            "--out-catalog",
            "c.nn.json",
        ],
    );
    // Payload span from show.
    let (_, show_out, _) = run_in(
        dir.path(),
        &["nn", "show", "--catalog", "c.nn.json", "--tensor", "w"],
    );
    let start: u64 = show_out
        .lines()
        .find(|l| l.contains("payload:"))
        .unwrap()
        .split_whitespace()
        .nth(1)
        .unwrap()
        .split("..")
        .next()
        .unwrap()
        .parse()
        .unwrap();

    // Scale byte: 32 influenced elements.
    let (code, out, _) = run_in(
        dir.path(),
        &[
            "nn",
            "impact",
            "--catalog",
            "c.nn.json",
            "--span",
            &format!("{}..{}", start, start + 1),
        ],
    );
    assert_eq!(code, 0, "out: {out}");
    assert!(out.contains("32 influenced elements"), "out: {out}");
    assert!(out.contains("shared F16 scale"), "out: {out}");
    assert!(out.contains("NOT predicted"), "out: {out}");

    // Code byte: 2 influenced elements (nibbles 0 and 16).
    let (code, out, _) = run_in(
        dir.path(),
        &[
            "nn",
            "impact",
            "--catalog",
            "c.nn.json",
            "--offset",
            &format!("{}", start + 2),
        ],
    );
    assert_eq!(code, 0, "out: {out}");
    assert!(out.contains("2 influenced elements"), "out: {out}");
    assert!(out.contains("exactly one element"), "out: {out}");

    // Header bytes: honestly unowned.
    let (code, out, _) = run_in(
        dir.path(),
        &["nn", "impact", "--catalog", "c.nn.json", "--offset", "3"],
    );
    assert_eq!(code, 0);
    assert!(out.contains("no exact-extent tensor owns"), "out: {out}");
}

#[test]
fn validate_reports_precise_verdicts_and_exit_codes() {
    let dir = tempfile::tempdir().unwrap();
    // Valid + size-mismatched (error finding) + unrecognized.
    let good = r#"{"w":{"dtype":"U8","shape":[1],"data_offsets":[0,1]}}"#;
    let mut data = Vec::new();
    data.extend_from_slice(&(good.len() as u64).to_le_bytes());
    data.extend_from_slice(good.as_bytes());
    data.push(7);
    fs::write(dir.path().join("good.safetensors"), data).unwrap();
    let bad = r#"{"b":{"dtype":"U8","shape":[2],"data_offsets":[0,3]}}"#;
    let mut data = Vec::new();
    data.extend_from_slice(&(bad.len() as u64).to_le_bytes());
    data.extend_from_slice(bad.as_bytes());
    data.extend_from_slice(&[1, 2, 3]);
    fs::write(dir.path().join("bad.safetensors"), data).unwrap();
    fs::write(dir.path().join("junk.safetensors"), b"plain text").unwrap();

    let (code, out, _) = run_in(dir.path(), &["-i", ".", "nn", "validate"]);
    assert_eq!(code, 7, "out: {out}");
    assert!(
        out.contains("good.safetensors: structurally_valid_for_reader"),
        "out: {out}"
    );
    assert!(out.contains("bad.safetensors: invalid"), "out: {out}");
    assert!(
        out.contains("junk.safetensors: unsupported_feature"),
        "out: {out}"
    );
    assert!(out.contains("behavior_not_evaluated"), "out: {out}");
    assert!(
        out.contains("not a safety or behavioral assessment"),
        "out: {out}"
    );

    // A clean single file validates with exit 0.
    let (code, out, _) = run_in(dir.path(), &["-i", "good.safetensors", "nn", "validate"]);
    assert_eq!(code, 0, "out: {out}");

    // JSON form carries the counts.
    let (_, out, _) = run_in(
        dir.path(),
        &["-i", ".", "nn", "validate", "--report-format", "json"],
    );
    assert!(out.contains("\"invalid\":\"1\""), "out: {out}");
    assert!(out.contains("\"unsupported_feature\":\"1\""), "out: {out}");
}

#[test]
fn profile_lint_and_scaffold_round_trip() {
    let dir = tempfile::tempdir().unwrap();
    // A clean pack: lints clean, exit 0.
    let clean = r#"schema: binfiddle.nn.pack/v1
id: clean.test
version: "1.0.0"
config:
  d: 4
bindings:
  - pattern: "model.layers.{layer}.self_attn.q_proj.weight"
    component: "decoder.layers[{layer}].q"
    kind: dense
    shape: ["d", "d"]
full_attention_interval: 4
"#;
    fs::write(dir.path().join("clean.yaml"), clean).unwrap();
    let (code, out, _) = run_in(dir.path(), &["nn", "pack", "lint", "--pack", "clean.yaml"]);
    assert_eq!(code, 0, "out: {out}");
    assert!(out.contains("LINT_CLEAN"), "out: {out}");

    // A broken pack: lint errors surface and exit 7.
    let broken = clean.replace("\"d\", \"d\"", "\"not_a_param\", \"d\"");
    fs::write(dir.path().join("broken.yaml"), broken).unwrap();
    let (code, out, _) = run_in(dir.path(), &["nn", "pack", "lint", "--pack", "broken.yaml"]);
    assert_eq!(code, 7, "out: {out}");
    assert!(out.contains("SHAPE_EXPRESSION"), "out: {out}");

    // Scaffold from a small catalog: PROVISIONAL labels, parses as a pack.
    let header = r#"{"model.layers.0.self_attn.q_proj.weight":{"dtype":"F32","shape":[12,4],"data_offsets":[0,192]},"model.layers.1.self_attn.q_proj.weight":{"dtype":"F32","shape":[12,4],"data_offsets":[192,384]}}"#;
    let mut data = Vec::new();
    data.extend_from_slice(&(header.len() as u64).to_le_bytes());
    data.extend_from_slice(header.as_bytes());
    data.extend(std::iter::repeat_n(0u8, 384));
    fs::write(dir.path().join("m.safetensors"), data).unwrap();
    run_in(
        dir.path(),
        &[
            "-i",
            "m.safetensors",
            "nn",
            "discover",
            "--out-catalog",
            "c.nn.json",
        ],
    );
    let (code, out, _) = run_in(
        dir.path(),
        &["nn", "pack", "scaffold", "--catalog", "c.nn.json"],
    );
    assert_eq!(code, 0, "out: {out}");
    assert!(out.contains("PROVISIONAL"), "out: {out}");
    assert!(out.contains("observed 2 name(s)"), "out: {out}");
    fs::write(dir.path().join("scaffolded.yaml"), &out).unwrap();
    // The scaffold has shapes like "12" (literal dims) so expressions
    // evaluate; lint passes or reports only non-error findings.
    let (_, lint_out, _) = run_in(
        dir.path(),
        &["nn", "pack", "lint", "--pack", "scaffolded.yaml"],
    );
    assert!(!lint_out.contains("[error]"), "lint_out: {lint_out}");
}
