//! Integration tests for approximate fingerprints and the evidence graph
//! (BFNN-REQ-026) through the real binary.

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

/// Build a safetensors file with one F32 tensor `w` of `values` plus an
/// unrelated small tensor `b` (constant across models).
fn write_model(dir: &Path, file: &str, values: &[f32]) {
    let tensors: Vec<(String, Vec<u64>, Vec<f32>)> = vec![
        ("w".to_string(), vec![values.len() as u64], values.to_vec()),
        ("b".to_string(), vec![1], vec![7.0]),
    ];
    let mut body: Vec<u8> = Vec::new();
    let mut spans = Vec::new();
    for (name, shape, vals) in &tensors {
        let begin = body.len();
        for v in vals {
            body.extend_from_slice(&v.to_le_bytes());
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
    fs::write(dir.join(file), data).unwrap();
}

#[test]
fn evidence_graph_distinguishes_exact_structural_and_similar() {
    let dir = tempfile::tempdir().unwrap();
    // Left: w = [1..16], b. Right: w differs ONLY in its last value
    // (one sampled block differs on this small payload: a single block),
    // b identical.
    let left_values: Vec<f32> = (1..=16).map(|i| i as f32).collect();
    let mut right_values = left_values.clone();
    *right_values.last_mut().unwrap() = 99.0;
    write_model(dir.path(), "left.safetensors", &left_values);
    write_model(dir.path(), "right.safetensors", &right_values);
    for (model, cat) in [
        ("left.safetensors", "l.nn.json"),
        ("right.safetensors", "r.nn.json"),
    ] {
        run_in(
            dir.path(),
            &[
                "-i",
                model,
                "nn",
                "discover",
                "--verify-content",
                "--out-catalog",
                cat,
            ],
        );
    }

    let (code, out, _) = run_in(
        dir.path(),
        &[
            "nn",
            "fingerprint",
            "--catalog",
            "l.nn.json",
            "--compare",
            "r.nn.json",
        ],
    );
    assert_eq!(code, 0, "out: {out}");
    // b is byte-identical -> exact edge.
    assert!(out.contains("b exact_payload_match"), "out: {out}");
    // w: payload differs, same structure, and the small payload is a single
    // sampled block whose digest changed -> structural edge only (score 0.0
    // below threshold; no similar edge).
    assert!(out.contains("w same_structure"), "out: {out}");
    assert!(!out.contains("w similar_under_mapping"), "out: {out}");
    assert!(out.contains("NOT claimed"), "out: {out}");
    assert!(out.contains("experimental"), "out: {out}");

    // JSON form: method/version/threshold records on similarity edges. The
    // similar case needs a multi-block payload: 8 blocks of 64 KiB, one
    // value perturbed -> 7/8 blocks match = 0.875 >= 0.75.
    let big_values: Vec<f32> = (0..8 * 16384).map(|i| (i % 97) as f32).collect();
    write_model(dir.path(), "big.safetensors", &big_values);
    let mut mid_values = big_values.clone();
    mid_values[100_000] = -1.0; // perturb inside one sampled block
    write_model(dir.path(), "mid.safetensors", &mid_values);
    run_in(
        dir.path(),
        &[
            "-i",
            "big.safetensors",
            "nn",
            "discover",
            "--verify-content",
            "--out-catalog",
            "big.nn.json",
        ],
    );
    run_in(
        dir.path(),
        &[
            "-i",
            "mid.safetensors",
            "nn",
            "discover",
            "--verify-content",
            "--out-catalog",
            "m.nn.json",
        ],
    );
    let (code, out, _) = run_in(
        dir.path(),
        &[
            "nn",
            "fingerprint",
            "--catalog",
            "big.nn.json",
            "--compare",
            "m.nn.json",
            "--report-format",
            "json",
        ],
    );
    assert_eq!(code, 0);
    assert!(
        out.contains("\"method\":\"sampled-block-digest/v1\""),
        "out: {out}"
    );
    assert!(out.contains("\"threshold\":\"0.7500\""), "out: {out}");
    assert!(out.contains("\"score\":\"0.8750\""), "out: {out}");
    assert!(
        out.contains("unsampled bytes are NOT certified"),
        "out: {out}"
    );
}

#[test]
fn identical_models_produce_exact_edges_only() {
    let dir = tempfile::tempdir().unwrap();
    let values: Vec<f32> = (0..32).map(|i| i as f32 * 0.5).collect();
    write_model(dir.path(), "a.safetensors", &values);
    write_model(dir.path(), "b.safetensors", &values);
    for (model, cat) in [
        ("a.safetensors", "a.nn.json"),
        ("b.safetensors", "b.nn.json"),
    ] {
        run_in(
            dir.path(),
            &[
                "-i",
                model,
                "nn",
                "discover",
                "--verify-content",
                "--out-catalog",
                cat,
            ],
        );
    }
    let (code, out, _) = run_in(
        dir.path(),
        &[
            "nn",
            "fingerprint",
            "--catalog",
            "a.nn.json",
            "--compare",
            "b.nn.json",
            "--report-format",
            "json",
        ],
    );
    assert_eq!(code, 0);
    assert!(out.contains("\"exact_payload_match\":\"2\""), "out: {out}");
    assert!(
        out.contains("\"similar_under_mapping\":\"0\""),
        "out: {out}"
    );
}

#[test]
fn invalid_thresholds_are_rejected() {
    let dir = tempfile::tempdir().unwrap();
    write_model(dir.path(), "m.safetensors", &[1.0, 2.0]);
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
    let (code, _, err) = run_in(
        dir.path(),
        &[
            "nn",
            "fingerprint",
            "--catalog",
            "c.nn.json",
            "--compare",
            "c.nn.json",
            "--threshold",
            "1.5",
        ],
    );
    assert_eq!(code, 2);
    assert!(err.contains("must be within"), "stderr: {err}");
}
