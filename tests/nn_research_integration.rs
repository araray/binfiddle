//! Integration tests for `nn research align` (M11 research track) through
//! the real binary.

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

/// Build a safetensors model with an F32 [rows, cols] tensor `w` whose rows
/// are identifiable: row r = [1000 + r, 1000 + r, ...].
fn write_model(dir: &Path, file: &str, rows: usize, cols: usize, rotate: Option<usize>) {
    let mut matrix: Vec<Vec<f32>> = (0..rows).map(|r| vec![(1000 + r) as f32; cols]).collect();
    if let Some(shift) = rotate {
        let shifted: Vec<Vec<f32>> = (0..rows)
            .map(|r| matrix[(r + shift) % rows].clone())
            .collect();
        matrix = shifted;
    }
    let mut body: Vec<u8> = Vec::new();
    let mut spans = Vec::new();
    {
        let begin = body.len();
        for row in &matrix {
            for v in row {
                body.extend_from_slice(&v.to_le_bytes());
            }
        }
        spans.push(("w", vec![rows as u64, cols as u64], begin, body.len()));
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
fn research_align_detects_permutations_and_rejects_independent_content() {
    let dir = tempfile::tempdir().unwrap();
    write_model(dir.path(), "base.safetensors", 6, 4, None);
    write_model(dir.path(), "rotated.safetensors", 6, 4, Some(2));

    // Independent content: rows differ in VALUES, not just order.
    let header = r#"{"w":{"dtype":"F32","shape":[6,4],"data_offsets":[0,96]}}"#;
    let mut data = Vec::new();
    data.extend_from_slice(&(header.len() as u64).to_le_bytes());
    data.extend_from_slice(header.as_bytes());
    for r in 0..6u32 {
        for _ in 0..4 {
            data.extend_from_slice(&(5_000 + r).to_le_bytes());
        }
    }
    fs::write(dir.path().join("other.safetensors"), data).unwrap();

    for (model, cat) in [
        ("base.safetensors", "b.nn.json"),
        ("rotated.safetensors", "r.nn.json"),
        ("other.safetensors", "o.nn.json"),
    ] {
        run_in(
            dir.path(),
            &["-i", model, "nn", "discover", "--out-catalog", cat],
        );
    }

    // Rotated: a real permutation (rows moved, none fixed under shift 2 mod 6).
    let (code, out, _) = run_in(
        dir.path(),
        &[
            "nn",
            "research",
            "align",
            "--left",
            "b.nn.json",
            "--right",
            "r.nn.json",
        ],
    );
    assert_eq!(code, 0, "out: {out}");
    assert!(
        out.contains("w: permutation (6 rows moved, 0 fixed"),
        "out: {out}"
    );
    assert!(
        out.contains("computational equivalence is NOT claimed"),
        "out: {out}"
    );
    assert!(out.contains("experimental"), "out: {out}");

    // Independent values: not a permutation.
    let (code, out, _) = run_in(
        dir.path(),
        &[
            "nn",
            "research",
            "align",
            "--left",
            "b.nn.json",
            "--right",
            "o.nn.json",
        ],
    );
    assert_eq!(code, 0);
    assert!(out.contains("w: not a row permutation"), "out: {out}");

    // Self-alignment: identity.
    let (code, out, _) = run_in(
        dir.path(),
        &[
            "nn",
            "research",
            "align",
            "--left",
            "b.nn.json",
            "--right",
            "b.nn.json",
        ],
    );
    assert_eq!(code, 0);
    assert!(
        out.contains("w: identical rows (identity permutation"),
        "out: {out}"
    );

    // JSON form carries counts + method.
    let (code, out, _) = run_in(
        dir.path(),
        &[
            "nn",
            "research",
            "align",
            "--left",
            "b.nn.json",
            "--right",
            "r.nn.json",
            "--report-format",
            "json",
        ],
    );
    assert_eq!(code, 0);
    assert!(out.contains("\"permutation\":\"1\""), "out: {out}");
    assert!(
        out.contains("\"method\":\"row-permutation-multiset/v1\""),
        "out: {out}"
    );
}
