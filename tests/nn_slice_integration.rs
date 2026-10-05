//! Integration tests for `nn slice` / `nn assemble`: real binary, full
//! workflows, stale-plan/selection rejection, and policy behavior.

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

/// F32 [2,2] tensor `w` (first element 1.0f32) plus a Q4_0-free U8 [4] `b`.
fn write_model(dir: &Path) {
    let header = r#"{"w":{"dtype":"F32","shape":[2,2],"data_offsets":[0,16]},"b":{"dtype":"U8","shape":[4],"data_offsets":[16,20]}}"#;
    let mut data = Vec::new();
    data.extend_from_slice(&(header.len() as u64).to_le_bytes());
    data.extend_from_slice(header.as_bytes());
    data.extend_from_slice(&[0x00, 0x00, 0x80, 0x3f]); // w[0,0] = 1.0f32
    data.extend(std::iter::repeat_n(0u8, 12));
    data.extend_from_slice(&[1, 2, 3, 4]);
    fs::write(dir.join("m.safetensors"), data).unwrap();
}

#[test]
fn full_slice_workflow_plan_apply_assemble() {
    let dir = tempfile::tempdir().unwrap();
    write_model(dir.path());

    // Verified discovery → catalog → selection.
    let (code, _, err) = run_in(
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
    assert_eq!(code, 0, "stderr: {err}");
    let (code, _, _) = run_in(
        dir.path(),
        &[
            "nn",
            "select",
            "--catalog",
            "c.nn.json",
            "--tensor",
            "w",
            "--out-selection",
            "w.sel.json",
        ],
    );
    assert_eq!(code, 0);

    // Dry run shows the plan.
    let (code, out, _) = run_in(
        dir.path(),
        &[
            "nn",
            "slice",
            "--catalog",
            "c.nn.json",
            "--selection",
            "w.sel.json",
            "--dry-run",
        ],
    );
    assert_eq!(code, 0);
    assert!(out.contains("slice plan (dry run)"), "out: {out}");
    assert!(out.contains("preserve_encoding"), "out: {out}");
    assert!(out.contains("w ["), "out: {out}");

    // Save the plan, apply it from the plan file, then assemble.
    let (code, _, _) = run_in(
        dir.path(),
        &[
            "nn",
            "slice",
            "--catalog",
            "c.nn.json",
            "--selection",
            "w.sel.json",
            "--save-plan",
            "w.plan.json",
        ],
    );
    assert_eq!(code, 0);
    assert!(Path::new(&dir.path().join("w.plan.json")).exists());

    let (code, out, _) = run_in(
        dir.path(),
        &[
            "nn",
            "slice",
            "--catalog",
            "c.nn.json",
            "--plan",
            "w.plan.json",
            "--out-dir",
            "bundle",
        ],
    );
    assert_eq!(code, 0);
    assert!(out.contains("storage: materialized"), "out: {out}");

    // The extracted member is exactly the tensor payload bytes.
    let source = fs::read(dir.path().join("m.safetensors")).unwrap();
    let member = fs::read(dir.path().join("bundle/weights/w")).unwrap();
    let manifest = fs::read_to_string(dir.path().join("bundle/slice.json")).unwrap();
    let start = manifest
        .split("\"span_start\":\"")
        .nth(1)
        .and_then(|rest| rest.split('"').next())
        .unwrap()
        .parse::<usize>()
        .unwrap();
    assert_eq!(&member[..], &source[start..start + 16]);
    assert!(manifest.contains("\"claim\":\"tensor_content\""));
    assert!(manifest.contains("\"encoding_preserved\":true"));

    // Assemble reconstructs identical bytes with digest verification.
    let (code, out, _) = run_in(
        dir.path(),
        &[
            "nn",
            "assemble",
            "--bundle",
            "bundle",
            "--out-dir",
            "rebuilt",
        ],
    );
    assert_eq!(code, 0);
    assert!(out.contains("digest verified"), "out: {out}");
    assert!(out.contains("NOT claimed"), "out: {out}");
    let rebuilt = fs::read(dir.path().join("rebuilt/w")).unwrap();
    assert_eq!(rebuilt, member);
}

#[test]
fn stale_selection_and_plan_are_rejected() {
    let dir = tempfile::tempdir().unwrap();
    write_model(dir.path());
    // Catalog v1 + selection + plan.
    run_in(
        dir.path(),
        &[
            "-i",
            "m.safetensors",
            "nn",
            "discover",
            "--verify-content",
            "--out-catalog",
            "v1.nn.json",
        ],
    );
    run_in(
        dir.path(),
        &[
            "nn",
            "select",
            "--catalog",
            "v1.nn.json",
            "--tensor",
            "w",
            "--out-selection",
            "w.sel.json",
        ],
    );
    run_in(
        dir.path(),
        &[
            "nn",
            "slice",
            "--catalog",
            "v1.nn.json",
            "--selection",
            "w.sel.json",
            "--save-plan",
            "w.plan.json",
        ],
    );

    // Mutate the model: new discovery yields a different catalog id.
    let mut data = fs::read(dir.path().join("m.safetensors")).unwrap();
    let last = data.len() - 1;
    data[last] ^= 0xFF;
    fs::write(dir.path().join("m.safetensors"), &data).unwrap();
    run_in(
        dir.path(),
        &[
            "-i",
            "m.safetensors",
            "nn",
            "discover",
            "--verify-content",
            "--out-catalog",
            "v2.nn.json",
        ],
    );

    // The old selection refuses to plan against the new catalog.
    let (code, _, err) = run_in(
        dir.path(),
        &[
            "nn",
            "slice",
            "--catalog",
            "v2.nn.json",
            "--selection",
            "w.sel.json",
            "--dry-run",
        ],
    );
    assert_eq!(code, 5);
    assert!(err.contains("never silently rematch"), "stderr: {err}");

    // The old plan refuses to apply against the new catalog.
    let (code, _, err) = run_in(
        dir.path(),
        &[
            "nn",
            "slice",
            "--catalog",
            "v2.nn.json",
            "--plan",
            "w.plan.json",
            "--out-dir",
            "b1",
        ],
    );
    assert_eq!(code, 5);
    assert!(err.contains("source changed"), "stderr: {err}");

    // Same catalog id but mutated source bytes: the recorded source digest
    // check fires during apply.
    run_in(
        dir.path(),
        &[
            "-i",
            "m.safetensors",
            "nn",
            "discover",
            "--verify-content",
            "--out-catalog",
            "v3.nn.json",
        ],
    );
    run_in(
        dir.path(),
        &[
            "nn",
            "select",
            "--catalog",
            "v3.nn.json",
            "--tensor",
            "w",
            "--out-selection",
            "w3.sel.json",
        ],
    );
    run_in(
        dir.path(),
        &[
            "nn",
            "slice",
            "--catalog",
            "v3.nn.json",
            "--selection",
            "w3.sel.json",
            "--save-plan",
            "w3.plan.json",
        ],
    );
    let mut data = fs::read(dir.path().join("m.safetensors")).unwrap();
    let last = data.len() - 1;
    data[last] ^= 0xFF;
    fs::write(dir.path().join("m.safetensors"), &data).unwrap();
    let (code, _, err) = run_in(
        dir.path(),
        &[
            "nn",
            "slice",
            "--catalog",
            "v3.nn.json",
            "--plan",
            "w3.plan.json",
            "--out-dir",
            "b2",
        ],
    );
    assert_eq!(code, 5);
    assert!(err.contains("content changed"), "stderr: {err}");
}

#[test]
fn reference_storage_requires_content_verified_sources() {
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
            "weak.nn.json",
        ],
    );
    run_in(
        dir.path(),
        &[
            "nn",
            "select",
            "--catalog",
            "weak.nn.json",
            "--tensor",
            "w",
            "--out-selection",
            "w.sel.json",
        ],
    );
    let (code, _, err) = run_in(
        dir.path(),
        &[
            "nn",
            "slice",
            "--catalog",
            "weak.nn.json",
            "--selection",
            "w.sel.json",
            "--storage",
            "reference",
            "--out-dir",
            "refbundle",
        ],
    );
    assert_eq!(code, 2);
    assert!(err.contains("content-verified"), "stderr: {err}");

    // With verification the reference bundle applies (no payload copy).
    run_in(
        dir.path(),
        &[
            "-i",
            "m.safetensors",
            "nn",
            "discover",
            "--verify-content",
            "--out-catalog",
            "strong.nn.json",
        ],
    );
    run_in(
        dir.path(),
        &[
            "nn",
            "select",
            "--catalog",
            "strong.nn.json",
            "--tensor",
            "w",
            "--out-selection",
            "ws.sel.json",
        ],
    );
    let (code, out, _) = run_in(
        dir.path(),
        &[
            "nn",
            "slice",
            "--catalog",
            "strong.nn.json",
            "--selection",
            "ws.sel.json",
            "--storage",
            "reference",
            "--out-dir",
            "refbundle",
        ],
    );
    assert_eq!(code, 0);
    assert!(out.contains("storage: reference"), "out: {out}");
    assert!(Path::new(&dir.path().join("refbundle/slice.json")).exists());
    assert!(!Path::new(&dir.path().join("refbundle/weights")).exists());
}

#[test]
fn decode_policy_materializes_numbers_and_records_loss() {
    let dir = tempfile::tempdir().unwrap();
    write_model(dir.path());
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
    run_in(
        dir.path(),
        &[
            "nn",
            "select",
            "--catalog",
            "c.nn.json",
            "--tensor",
            "w",
            "--out-selection",
            "w.sel.json",
        ],
    );
    let (code, _, _) = run_in(
        dir.path(),
        &[
            "nn",
            "slice",
            "--catalog",
            "c.nn.json",
            "--selection",
            "w.sel.json",
            "--quant",
            "decode",
            "--out-dir",
            "decoded",
        ],
    );
    assert_eq!(code, 0);
    let decoded = fs::read(dir.path().join("decoded/weights/w")).unwrap();
    assert_eq!(decoded.len(), 16); // 4 f32 values
    assert_eq!(f32::from_le_bytes(decoded[0..4].try_into().unwrap()), 1.0);
    let manifest = fs::read_to_string(dir.path().join("decoded/slice.json")).unwrap();
    assert!(manifest.contains("\"encoding_preserved\":false"));
}

#[test]
fn usage_errors_are_rejected() {
    let dir = tempfile::tempdir().unwrap();
    write_model(dir.path());
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
    run_in(
        dir.path(),
        &[
            "nn",
            "select",
            "--catalog",
            "c.nn.json",
            "--tensor",
            "w",
            "--out-selection",
            "w.sel.json",
        ],
    );
    // Neither --selection nor --plan.
    let (code, _, err) = run_in(dir.path(), &["nn", "slice", "--catalog", "c.nn.json"]);
    assert_eq!(code, 2);
    assert!(err.contains("exactly one"), "stderr: {err}");
    // Both at once.
    let (code, _, _) = run_in(
        dir.path(),
        &[
            "nn",
            "slice",
            "--catalog",
            "c.nn.json",
            "--selection",
            "w.sel.json",
            "--plan",
            "x",
        ],
    );
    assert_eq!(code, 2);
    // Plan preview without --out-dir succeeds (no bundle written).
    let (code, out, _) = run_in(
        dir.path(),
        &[
            "nn",
            "slice",
            "--catalog",
            "c.nn.json",
            "--selection",
            "w.sel.json",
        ],
    );
    assert_eq!(code, 0);
    assert!(out.contains("slice plan (dry run)"), "out: {out}");
    // Apply mode without --out-dir is a usage error.
    run_in(
        dir.path(),
        &[
            "nn",
            "slice",
            "--catalog",
            "c.nn.json",
            "--selection",
            "w.sel.json",
            "--save-plan",
            "p.json",
        ],
    );
    let (code, _, err) = run_in(
        dir.path(),
        &["nn", "slice", "--catalog", "c.nn.json", "--plan", "p.json"],
    );
    assert_eq!(code, 2);
    assert!(err.contains("--out-dir"), "stderr: {err}");
    // Nonempty output directory.
    fs::create_dir_all(dir.path().join("occupied")).unwrap();
    fs::write(dir.path().join("occupied/keep.txt"), b"x").unwrap();
    let (code, _, err) = run_in(
        dir.path(),
        &[
            "nn",
            "slice",
            "--catalog",
            "c.nn.json",
            "--selection",
            "w.sel.json",
            "--out-dir",
            "occupied",
        ],
    );
    assert_eq!(code, 2);
    assert!(err.contains("not empty"), "stderr: {err}");
}

#[test]
fn assemble_rejects_corrupted_and_reference_bundles() {
    let dir = tempfile::tempdir().unwrap();
    write_model(dir.path());
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
    run_in(
        dir.path(),
        &[
            "nn",
            "select",
            "--catalog",
            "c.nn.json",
            "--tensor",
            "w",
            "--out-selection",
            "w.sel.json",
        ],
    );
    run_in(
        dir.path(),
        &[
            "nn",
            "slice",
            "--catalog",
            "c.nn.json",
            "--selection",
            "w.sel.json",
            "--out-dir",
            "bundle",
        ],
    );
    // Corrupt the member: assemble must fail digest validation.
    let member = dir.path().join("bundle/weights/w");
    let mut data = fs::read(&member).unwrap();
    data[0] ^= 0xFF;
    fs::write(&member, &data).unwrap();
    let (code, _, err) = run_in(
        dir.path(),
        &["nn", "assemble", "--bundle", "bundle", "--out-dir", "out"],
    );
    assert_eq!(code, 7);
    assert!(err.contains("digest"), "stderr: {err}");

    // Reference bundle: reconstruction is not applicable.
    fs::remove_dir_all(dir.path().join("bundle")).unwrap();
    run_in(
        dir.path(),
        &[
            "nn",
            "slice",
            "--catalog",
            "c.nn.json",
            "--selection",
            "w.sel.json",
            "--storage",
            "reference",
            "--out-dir",
            "refbundle",
        ],
    );
    let (code, _, err) = run_in(
        dir.path(),
        &[
            "nn",
            "assemble",
            "--bundle",
            "refbundle",
            "--out-dir",
            "out2",
        ],
    );
    assert_eq!(code, 2);
    assert!(err.contains("materialized"), "stderr: {err}");
}
