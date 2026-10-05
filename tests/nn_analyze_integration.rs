//! Integration tests for `nn analyze`: real binary, reference vectors,
//! coverage modes, histograms, reference metrics, and Q4_0 block views.

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

/// F32 [2,2] tensor `w` holding 1, 2, 3, 4.
fn write_model(dir: &Path) {
    let header = r#"{"w":{"dtype":"F32","shape":[2,2],"data_offsets":[0,16]}}"#;
    let mut data = Vec::new();
    data.extend_from_slice(&(header.len() as u64).to_le_bytes());
    data.extend_from_slice(header.as_bytes());
    for v in [1.0f32, 2.0, 3.0, 4.0] {
        data.extend_from_slice(&v.to_le_bytes());
    }
    fs::write(dir.join("m.safetensors"), data).unwrap();
}

/// GGUF with one Q4_0 [1,32] tensor = the corpus fixture block.
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
    out.push(0x00);
    out.push(0x38);
    out.push(0xA3);
    out.extend(std::iter::repeat_n(0u8, 15));
    fs::write(dir.join("q.gguf"), out).unwrap();
}

#[test]
fn full_scan_reproduces_reference_statistics() {
    let dir = tempfile::tempdir().unwrap();
    write_model(dir.path());
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
        &["nn", "analyze", "--catalog", "c.nn.json", "--tensor", "w"],
    );
    assert_eq!(code, 0);
    // The published reference vector: mean 2.5, population variance 1.25,
    // sample variance 5/3.
    assert!(out.contains("4 of 4 elements examined"), "out: {out}");
    assert!(out.contains("mean:     2.5"), "out: {out}");
    assert!(out.contains("variance: 1.25 (population)"), "out: {out}");
    assert!(out.contains("1.6666666666666667 (sample)"), "out: {out}");
    assert!(out.contains("min:      1 at [0, 0]"), "out: {out}");
    assert!(out.contains("max:      4 at [1, 1]"), "out: {out}");
    // L2 norm sqrt(30).
    assert!(out.contains("L2 norm:  5.477225575051661"), "out: {out}");

    // JSON envelope carries coverage + the same values.
    let (code, out, _) = run_in(
        dir.path(),
        &[
            "nn",
            "analyze",
            "--catalog",
            "c.nn.json",
            "--tensor",
            "w",
            "--report-format",
            "json",
        ],
    );
    assert_eq!(code, 0);
    assert!(out.contains("\"mode\":\"full\""));
    assert!(out.contains("\"examined_elements\":\"4\""));
    assert!(out.contains("\"population_variance\":\"1.25\""));
}

#[test]
fn sample_mode_is_deterministic_and_honest_about_coverage() {
    let dir = tempfile::tempdir().unwrap();
    write_model(dir.path());
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
    let args = &[
        "nn",
        "analyze",
        "--catalog",
        "c.nn.json",
        "--tensor",
        "w",
        "--mode",
        "sample",
        "--seed",
        "17",
        "--sample-size",
        "2",
    ];
    let (code, first, _) = run_in(dir.path(), args);
    assert_eq!(code, 0);
    assert!(first.contains("mode sample"), "out: {first}");
    assert!(first.contains("2 of 4 elements examined"), "out: {first}");
    // Determinism: identical invocation, identical statistics.
    let (_, second, _) = run_in(dir.path(), args);
    assert_eq!(first, second);
    // A different seed changes the sample.
    let (_, other, _) = run_in(
        dir.path(),
        &[
            "nn",
            "analyze",
            "--catalog",
            "c.nn.json",
            "--tensor",
            "w",
            "--mode",
            "sample",
            "--seed",
            "99",
            "--sample-size",
            "2",
        ],
    );
    assert_ne!(first, other);
}

#[test]
fn metadata_mode_reads_nothing_and_refuses_payload_features() {
    let dir = tempfile::tempdir().unwrap();
    write_model(dir.path());
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
        &[
            "nn",
            "analyze",
            "--catalog",
            "c.nn.json",
            "--tensor",
            "w",
            "--mode",
            "metadata",
        ],
    );
    assert_eq!(code, 0);
    assert!(out.contains("0 of 4 elements examined"), "out: {out}");
    assert!(out.contains("0 bytes read"), "out: {out}");
    assert!(out.contains("no numerical observations"), "out: {out}");

    let (code, _, err) = run_in(
        dir.path(),
        &[
            "nn",
            "analyze",
            "--catalog",
            "c.nn.json",
            "--tensor",
            "w",
            "--mode",
            "metadata",
            "--histogram-bins",
            "4",
        ],
    );
    assert_eq!(code, 2);
    assert!(
        err.contains("metadata mode reads no payload"),
        "stderr: {err}"
    );
}

#[test]
fn histogram_declares_edges() {
    let dir = tempfile::tempdir().unwrap();
    write_model(dir.path());
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
        &[
            "nn",
            "analyze",
            "--catalog",
            "c.nn.json",
            "--tensor",
            "w",
            "--histogram-bins",
            "2",
        ],
    );
    assert_eq!(code, 0);
    assert!(out.contains("histogram: 2 bins over [1, 4]"), "out: {out}");
    assert!(out.contains("[1.000000, 2.500000): 2"), "out: {out}");
    assert!(out.contains("[2.500000, 4.000000)]: 2"), "out: {out}");
}

#[test]
fn reference_metrics_reproduce_the_published_vector() {
    let dir = tempfile::tempdir().unwrap();
    write_model(dir.path());
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
    // Reference [0,2,?,?] aligned to w=[1,2,3,4]: pairs (0,1),(2,2)...
    // Use a clean 4-element reference: x=[0,2,0,2], y=w=[1,2,3,4].
    let mut reference = Vec::new();
    for v in [0.0f32, 2.0, 0.0, 2.0] {
        reference.extend_from_slice(&v.to_le_bytes());
    }
    fs::write(dir.path().join("ref.f32"), reference).unwrap();
    let (code, out, _) = run_in(
        dir.path(),
        &[
            "nn",
            "analyze",
            "--catalog",
            "c.nn.json",
            "--tensor",
            "w",
            "--reference",
            "ref.f32",
            "--reference-width",
            "4",
        ],
    );
    assert_eq!(code, 0);
    // e = [1,0,3,2]: MAE 1.5, maxAE 3.
    assert!(out.contains("MAE:  1.5"), "out: {out}");
    assert!(out.contains("maxAE: 3"), "out: {out}");
    assert!(out.contains("4 pairs compared"), "out: {out}");

    // Length mismatch is a usage error.
    fs::write(dir.path().join("short.f32"), [0u8; 4]).unwrap();
    let (code, _, err) = run_in(
        dir.path(),
        &[
            "nn",
            "analyze",
            "--catalog",
            "c.nn.json",
            "--tensor",
            "w",
            "--reference",
            "short.f32",
        ],
    );
    assert_eq!(code, 2);
    assert!(err.contains("reference has"), "stderr: {err}");
}

#[test]
fn q4_0_scan_and_block_view_use_the_reference_decoder() {
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
            "q.nn.json",
        ],
    );
    let (code, out, _) = run_in(
        dir.path(),
        &[
            "nn",
            "analyze",
            "--catalog",
            "q.nn.json",
            "--tensor",
            "w",
            "--blocks",
            "1",
        ],
    );
    assert_eq!(code, 0);
    assert!(out.contains("32 of 32 elements examined"), "out: {out}");
    // The corpus fixture values: max is +1.0 at element [0,16]; the bulk is
    // -4.0 (zero codes with scale 0.5).
    assert!(out.contains("max:      1 at [0, 16]"), "out: {out}");
    assert!(
        out.contains("min:      -4 at [0, 1] (+29 ties)"),
        "out: {out}"
    );
    assert!(
        out.contains("block 0: scale 0.5 (raw 0038), first values -2.5, -4, 1"),
        "out: {out}"
    );
}

#[test]
fn nonfinite_values_are_counted_not_averaged() {
    let dir = tempfile::tempdir().unwrap();
    // F32 [4] = [1, NaN, +Inf, 2].
    let header = r#"{"w":{"dtype":"F32","shape":[4],"data_offsets":[0,16]}}"#;
    let mut data = Vec::new();
    data.extend_from_slice(&(header.len() as u64).to_le_bytes());
    data.extend_from_slice(header.as_bytes());
    data.extend_from_slice(&1.0f32.to_le_bytes());
    data.extend_from_slice(&f32::NAN.to_le_bytes());
    data.extend_from_slice(&f32::INFINITY.to_le_bytes());
    data.extend_from_slice(&2.0f32.to_le_bytes());
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
        &["nn", "analyze", "--catalog", "c.nn.json", "--tensor", "w"],
    );
    assert_eq!(code, 0);
    assert!(out.contains("finite:   2"), "out: {out}");
    assert!(
        out.contains("non-finite: NaN 1, +Inf 1, -Inf 0"),
        "out: {out}"
    );
    assert!(out.contains("mean:     1.5"), "out: {out}");
    assert!(out.contains("NONFINITE_OBSERVED"), "out: {out}");
}
