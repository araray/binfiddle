//! Integration tests for M9: layered diff and exact fingerprints through
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

/// Build a safetensors file from (name, shape, values) entries.
fn write_model(dir: &Path, name: &str, tensors: &[(&str, Vec<u64>, Vec<f32>)]) {
    let mut body: Vec<u8> = Vec::new();
    let mut spans = Vec::new();
    for (tname, shape, values) in tensors {
        let begin = body.len();
        for v in values {
            body.extend_from_slice(&v.to_le_bytes());
        }
        spans.push((tname, shape, begin, body.len()));
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

fn catalog(dir: &Path, model: &str, out: &str) {
    let (code, _, err) = run_in(
        dir,
        &[
            "-i",
            model,
            "nn",
            "discover",
            "--verify-content",
            "--out-catalog",
            out,
        ],
    );
    assert_eq!(code, 0, "stderr: {err}");
}

#[test]
fn diff_layers_distinguish_content_repack_and_descriptors() {
    let dir = tempfile::tempdir().unwrap();
    // Left: a [2,2], b [3], c [2].
    write_model(
        dir.path(),
        "left.safetensors",
        &[
            ("a", vec![2, 2], vec![1.0, 2.0, 3.0, 4.0]),
            ("b", vec![3], vec![5.0, 6.0, 7.0]),
            ("c", vec![2], vec![8.0, 9.0]),
        ],
    );
    // Right: a same bytes, b same bytes but stored in a different position
    // (different header length shifts offsets => repack), c shape changed,
    // and d added. Changing the header length is fiddly; a simpler repack:
    // rename... no — reorder tensors so c comes before b: b's payload moves.
    // Right order: a, c, b with c [2,2] (shape change) and d new.
    write_model(
        dir.path(),
        "right.safetensors",
        &[
            ("a", vec![2, 2], vec![1.0, 2.0, 3.0, 4.0]),
            ("c", vec![2, 2], vec![8.0, 9.0, 8.0, 9.0]),
            ("b", vec![3], vec![5.0, 6.0, 7.0]),
            ("d", vec![1], vec![0.0]),
        ],
    );
    catalog(dir.path(), "left.safetensors", "l.nn.json");
    catalog(dir.path(), "right.safetensors", "r.nn.json");

    let (code, out, _) = run_in(
        dir.path(),
        &["nn", "diff", "--left", "l.nn.json", "--right", "r.nn.json"],
    );
    assert_eq!(code, 0, "out: {out}");
    // a: identical bytes; whether "identical" or "repacked" depends on
    // offsets — assert one of the two honest outcomes.
    assert!(
        out.contains("a: identical") || out.contains("a: repacked"),
        "out: {out}"
    );
    // b: same payload bytes but at a different offset => repacked, NOT a
    // content change.
    assert!(
        out.contains("b: repacked (same bytes; offsets"),
        "out: {out}"
    );
    // c: shape change [2] -> [2,2].
    assert!(out.contains("c: shape [2] -> [2, 2]"), "out: {out}");
    // d: right only; unmatched stays visible.
    assert!(out.contains("d: right only"), "out: {out}");
    assert!(out.contains("unmatched"), "out: {out}");
    // No lineage claims.
    assert!(out.contains("no lineage or behavior claims"), "out: {out}");

    // JSON envelope carries counts.
    let (code, out, _) = run_in(
        dir.path(),
        &[
            "nn",
            "diff",
            "--left",
            "l.nn.json",
            "--right",
            "r.nn.json",
            "--report-format",
            "json",
        ],
    );
    assert_eq!(code, 0);
    // Both a and b moved (the right file has a different header layout),
    // so both are honestly reported as repacked — not content changes.
    assert!(out.contains("\"repacked\":\"2\""), "out: {out}");
    assert!(out.contains("\"descriptor_changes\":\"1\""), "out: {out}");
    assert!(out.contains("\"unmatched\":\"1\""), "out: {out}");
}

#[test]
fn diff_reports_content_changes_and_decoded_metrics() {
    let dir = tempfile::tempdir().unwrap();
    write_model(
        dir.path(),
        "left.safetensors",
        &[("w", vec![4], vec![0.0, 2.0, 0.0, 2.0])],
    );
    write_model(
        dir.path(),
        "right.safetensors",
        &[("w", vec![4], vec![1.0, 0.0, 3.0, 2.0])],
    );
    catalog(dir.path(), "left.safetensors", "l.nn.json");
    catalog(dir.path(), "right.safetensors", "r.nn.json");

    // Without --decoded: content changed, no decoded section.
    let (code, out, _) = run_in(
        dir.path(),
        &["nn", "diff", "--left", "l.nn.json", "--right", "r.nn.json"],
    );
    assert_eq!(code, 0);
    assert!(out.contains("w: content changed"), "out: {out}");
    assert!(!out.contains("decoded"), "out: {out}");

    // With --decoded: unequal count + note.
    let (code, out, _) = run_in(
        dir.path(),
        &[
            "nn",
            "diff",
            "--left",
            "l.nn.json",
            "--right",
            "r.nn.json",
            "--decoded",
        ],
    );
    assert_eq!(code, 0);
    assert!(
        out.contains("w: decoded 4 values compared, 3 unequal"),
        "out: {out}"
    );
    assert!(out.contains("exact_bits"), "out: {out}");

    // Missing tensor is NOT a zero tensor: adding a tensor shows as
    // right-only, never as a content change.
    write_model(
        dir.path(),
        "right2.safetensors",
        &[
            ("w", vec![4], vec![0.0, 2.0, 0.0, 2.0]),
            ("extra", vec![1], vec![0.0]),
        ],
    );
    catalog(dir.path(), "right2.safetensors", "r2.nn.json");
    let (code, out, _) = run_in(
        dir.path(),
        &[
            "nn",
            "diff",
            "--left",
            "l.nn.json",
            "--right",
            "r2.nn.json",
            "--decoded",
        ],
    );
    assert_eq!(code, 0);
    // Same bytes; the differing header may shift offsets, so identical or
    // repacked are both honest outcomes — never a content change.
    assert!(
        out.contains("w: identical") || out.contains("w: repacked"),
        "out: {out}"
    );
    assert!(out.contains("extra: right only"), "out: {out}");
}

#[test]
fn diff_requires_content_verified_catalogs() {
    let dir = tempfile::tempdir().unwrap();
    write_model(dir.path(), "m.safetensors", &[("w", vec![1], vec![1.0])]);
    run_in(
        dir.path(),
        &[
            "-i",
            "m.safetensors",
            "nn",
            "discover",
            "--out-catalog",
            "weak.nn.json",
        ],
    );
    let (code, _, err) = run_in(
        dir.path(),
        &[
            "nn",
            "diff",
            "--left",
            "weak.nn.json",
            "--right",
            "weak.nn.json",
        ],
    );
    assert_eq!(code, 2);
    assert!(err.contains("content-verified"), "stderr: {err}");
}

#[test]
fn fingerprints_are_exact_and_stable() {
    let dir = tempfile::tempdir().unwrap();
    write_model(
        dir.path(),
        "m.safetensors",
        &[("w", vec![2, 2], vec![1.0, 2.0, 3.0, 4.0])],
    );
    catalog(dir.path(), "m.safetensors", "c.nn.json");

    let (code, out, _) = run_in(dir.path(), &["nn", "fingerprint", "--catalog", "c.nn.json"]);
    assert_eq!(code, 0, "out: {out}");
    assert!(out.contains("exact fingerprints"), "out: {out}");
    assert!(out.contains("w [2x2] safetensors.F32"), "out: {out}");
    assert!(
        out.contains("no lineage or chronology claims"),
        "out: {out}"
    );

    // Stability: same model re-discovered gives identical records.
    catalog(dir.path(), "m.safetensors", "c2.nn.json");
    let (_, out2, _) = run_in(
        dir.path(),
        &["nn", "fingerprint", "--catalog", "c2.nn.json"],
    );
    assert_eq!(out, out2);

    // Changing one value changes the payload digest.
    write_model(
        dir.path(),
        "m2.safetensors",
        &[("w", vec![2, 2], vec![1.0, 2.0, 3.0, 5.0])],
    );
    catalog(dir.path(), "m2.safetensors", "c3.nn.json");
    let (_, out3, _) = run_in(
        dir.path(),
        &["nn", "fingerprint", "--catalog", "c3.nn.json"],
    );
    assert_ne!(out, out3);

    // JSON form includes the method statement.
    let (code, out, _) = run_in(
        dir.path(),
        &[
            "nn",
            "fingerprint",
            "--catalog",
            "c.nn.json",
            "--report-format",
            "json",
        ],
    );
    assert_eq!(code, 0);
    assert!(out.contains("\"payload_sha256\""));
    assert!(out.contains("sha256(canonical("));
}
