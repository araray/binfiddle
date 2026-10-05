//! Integration tests for `nn edit set/apply/undo`: the corpus B.3 masked-write
//! vector, stale rejection, preservation proof, and undo round trip through
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

/// GGUF with one Q4_0 [1,32] tensor whose block is the corpus fixture
/// (scale `00 38`, first code byte `A3`).
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

/// SafeTensors with an F32 [2,2] tensor `w` = [1, 2, 3, 4].
fn write_f32_model(dir: &Path) {
    let header = r#"{"w":{"dtype":"F32","shape":[2,2],"data_offsets":[0,16]}}"#;
    let mut data = Vec::new();
    data.extend_from_slice(&(header.len() as u64).to_le_bytes());
    data.extend_from_slice(header.as_bytes());
    for v in [1.0f32, 2.0, 3.0, 4.0] {
        data.extend_from_slice(&v.to_le_bytes());
    }
    fs::write(dir.join("m.safetensors"), data).unwrap();
}

/// The corpus B.3 vector through the real binary: value −2.0 at element
/// [0,0] rewrites the low nibble 3→4 (A3→A4), value[0] −2.5→−2.0, and the
/// neighbor at element 16 (+1.0) is preserved by the mask.
#[test]
fn q4_0_masked_write_reproduces_the_corpus_vector() {
    let dir = tempfile::tempdir().unwrap();
    write_q4_model(dir.path());
    run_in(
        dir.path(),
        &[
            "-i",
            "q.gguf",
            "nn",
            "discover",
            "--verify-content",
            "--out-catalog",
            "c.nn.json",
        ],
    );
    let (code, out, _) = run_in(
        dir.path(),
        &[
            "nn",
            "edit",
            "set",
            "--catalog",
            "c.nn.json",
            "--tensor",
            "w",
            "--index",
            "0,0",
            "--value=-2.0",
            "--save-plan",
            "p.json",
        ],
    );
    assert_eq!(code, 0, "out: {out}");
    assert!(out.contains("masked 0x0f shift 0"), "out: {out}");
    assert!(out.contains("-2.5 -> -2"), "out: {out}");
    assert!(out.contains("a3 -> a4"), "out: {out}");
    assert!(out.contains("fixed_parameters"), "out: {out}");

    let (code, out, _) = run_in(
        dir.path(),
        &[
            "nn",
            "edit",
            "apply",
            "--catalog",
            "c.nn.json",
            "--plan",
            "p.json",
            "--out-model",
            "edited.gguf",
            "--undo-bundle",
            "undo",
        ],
    );
    assert_eq!(code, 0, "out: {out}");
    assert!(out.contains("preserved: all bytes outside"), "out: {out}");
    assert!(
        out.contains("reparsed: container reparse valid"),
        "out: {out}"
    );

    let original = fs::read(dir.path().join("q.gguf")).unwrap();
    let edited = fs::read(dir.path().join("edited.gguf")).unwrap();
    let diffs: Vec<usize> = (0..original.len())
        .filter(|&i| original[i] != edited[i])
        .collect();
    assert_eq!(diffs, vec![130], "exactly the code byte may change");
    let payload = edited.len() - 18;
    assert_eq!(
        &edited[payload..payload + 2],
        &[0x00, 0x38],
        "scale untouched"
    );
    assert_eq!(edited[payload + 2], 0xA4, "low nibble 3 -> 4");
    // Neighbor value: analyze the edited file and confirm element 16 is +1.0.
    run_in(
        dir.path(),
        &[
            "-i",
            "edited.gguf",
            "nn",
            "discover",
            "--out-catalog",
            "e.nn.json",
        ],
    );
    let (code, out, _) = run_in(
        dir.path(),
        &["nn", "analyze", "--catalog", "e.nn.json", "--tensor", "w"],
    );
    assert_eq!(code, 0);
    assert!(
        out.contains("max:      1 at [0, 16]"),
        "neighbor preserved: {out}"
    );

    // Undo round trip is byte-identical to the original.
    let (code, _, _) = run_in(
        dir.path(),
        &[
            "nn",
            "edit",
            "undo",
            "--bundle",
            "undo",
            "--target",
            "edited.gguf",
            "--out-model",
            "restored.gguf",
        ],
    );
    assert_eq!(code, 0);
    assert_eq!(
        fs::read(dir.path().join("restored.gguf")).unwrap(),
        original,
        "undo must restore the exact original bytes"
    );
}

#[test]
fn stale_sources_and_preimages_are_rejected() {
    let dir = tempfile::tempdir().unwrap();
    write_q4_model(dir.path());
    run_in(
        dir.path(),
        &[
            "-i",
            "q.gguf",
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
            "edit",
            "set",
            "--catalog",
            "c.nn.json",
            "--tensor",
            "w",
            "--index",
            "0,0",
            "--value=-2.0",
            "--save-plan",
            "p.json",
        ],
    );

    // Mutate the source after planning: apply must refuse.
    let mut data = fs::read(dir.path().join("q.gguf")).unwrap();
    let last = data.len() - 1;
    data[last] ^= 0xFF;
    fs::write(dir.path().join("q.gguf"), &data).unwrap();
    let (code, _, err) = run_in(
        dir.path(),
        &[
            "nn",
            "edit",
            "apply",
            "--catalog",
            "c.nn.json",
            "--plan",
            "p.json",
            "--out-model",
            "x.gguf",
        ],
    );
    assert_eq!(code, 5);
    assert!(
        err.contains("source changed since the plan"),
        "stderr: {err}"
    );

    // Restore, apply once, then apply the SAME plan again: the preimage
    // assertion must catch the changed write unit.
    let mut data = fs::read(dir.path().join("q.gguf")).unwrap();
    let last = data.len() - 1;
    data[last] ^= 0xFF;
    fs::write(dir.path().join("q.gguf"), &data).unwrap();
    let (code, _, _) = run_in(
        dir.path(),
        &[
            "nn",
            "edit",
            "apply",
            "--catalog",
            "c.nn.json",
            "--plan",
            "p.json",
            "--out-model",
            "first.gguf",
        ],
    );
    assert_eq!(code, 0);
    // Re-apply against a catalog built on the EDITED file: different catalog
    // id is the first rejection.
    run_in(
        dir.path(),
        &[
            "-i",
            "first.gguf",
            "nn",
            "discover",
            "--verify-content",
            "--out-catalog",
            "c2.nn.json",
        ],
    );
    let (code, _, err) = run_in(
        dir.path(),
        &[
            "nn",
            "edit",
            "apply",
            "--catalog",
            "c2.nn.json",
            "--plan",
            "p.json",
            "--out-model",
            "second.gguf",
        ],
    );
    assert_eq!(code, 5);
    assert!(err.contains("plan was built against"), "stderr: {err}");
}

#[test]
fn scalar_edits_policies_and_raw_bits() {
    let dir = tempfile::tempdir().unwrap();
    write_f32_model(dir.path());
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
    // Typed F32 edit: 4-byte write, exact by construction.
    let (code, out, _) = run_in(
        dir.path(),
        &[
            "nn",
            "edit",
            "set",
            "--catalog",
            "c.nn.json",
            "--tensor",
            "w",
            "--index",
            "1,0",
            "--value",
            "9.5",
            "--save-plan",
            "p.json",
        ],
    );
    assert_eq!(code, 0, "out: {out}");
    assert!(out.contains("3 -> 9.5"), "out: {out}");
    assert!(out.contains("policy:    exact_only"), "out: {out}");
    let (code, _, _) = run_in(
        dir.path(),
        &[
            "nn",
            "edit",
            "apply",
            "--catalog",
            "c.nn.json",
            "--plan",
            "p.json",
            "--out-model",
            "m2.safetensors",
        ],
    );
    assert_eq!(code, 0);
    let edited = fs::read(dir.path().join("m2.safetensors")).unwrap();
    // Verify the patched value directly.
    run_in(
        dir.path(),
        &[
            "-i",
            "m2.safetensors",
            "nn",
            "discover",
            "--out-catalog",
            "e.nn.json",
        ],
    );
    let (code, out, _) = run_in(
        dir.path(),
        &["nn", "analyze", "--catalog", "e.nn.json", "--tensor", "w"],
    );
    assert_eq!(code, 0);
    assert!(out.contains("max:      9.5 at [1, 0]"), "out: {out}");
    let _ = edited;

    // Out-of-range integer edit is rejected.
    let (code, _, err) = run_in(
        dir.path(),
        &[
            "nn",
            "edit",
            "set",
            "--catalog",
            "c.nn.json",
            "--tensor",
            "w",
            "--index",
            "0,0",
            "--value",
            "1e40",
            "--save-plan",
            "p2.json",
        ],
    );
    assert_eq!(code, 3);
    assert!(err.contains("not exactly representable"), "stderr: {err}");

    // Raw-bits edit writes exact bytes (7.0f32 = 0000e040 at [0,0]).
    let (code, out, _) = run_in(
        dir.path(),
        &[
            "nn",
            "edit",
            "set",
            "--catalog",
            "c.nn.json",
            "--tensor",
            "w",
            "--index",
            "0,0",
            "--raw-bits",
            "0000e040",
            "--save-plan",
            "p3.json",
        ],
    );
    assert_eq!(code, 0, "out: {out}");
    assert!(
        out.contains("bytes:     0000803f -> 0000e040"),
        "out: {out}"
    );
    let (code, _, _) = run_in(
        dir.path(),
        &[
            "nn",
            "edit",
            "apply",
            "--catalog",
            "c.nn.json",
            "--plan",
            "p3.json",
            "--out-model",
            "m3.safetensors",
        ],
    );
    assert_eq!(code, 0);
    run_in(
        dir.path(),
        &[
            "-i",
            "m3.safetensors",
            "nn",
            "discover",
            "--out-catalog",
            "e3.nn.json",
        ],
    );
    let (code, out, _) = run_in(
        dir.path(),
        &["nn", "analyze", "--catalog", "e3.nn.json", "--tensor", "w"],
    );
    assert_eq!(code, 0);
    assert!(
        out.contains("min:      2 at [0, 1]"),
        "1.0 replaced by 7.0: {out}"
    );
    assert!(out.contains("max:      7 at [0, 0]"), "out: {out}");

    // A raw-bits edit equal to the current bytes is a no-op and refused.
    let (code, _, err) = run_in(
        dir.path(),
        &[
            "nn",
            "edit",
            "set",
            "--catalog",
            "c.nn.json",
            "--tensor",
            "w",
            "--index",
            "0,0",
            "--raw-bits",
            "0000803f",
        ],
    );
    assert_eq!(code, 2);
    assert!(err.contains("no-op"), "stderr: {err}");
}

#[test]
fn no_op_edits_are_refused_and_outputs_never_overwritten() {
    let dir = tempfile::tempdir().unwrap();
    write_f32_model(dir.path());
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
    // Requesting the current value is a no-op → rejected.
    let (code, _, err) = run_in(
        dir.path(),
        &[
            "nn",
            "edit",
            "set",
            "--catalog",
            "c.nn.json",
            "--tensor",
            "w",
            "--index",
            "0,0",
            "--value",
            "1",
        ],
    );
    assert_eq!(code, 2);
    assert!(err.contains("no-op"), "stderr: {err}");

    // Apply refuses to overwrite an existing output.
    run_in(
        dir.path(),
        &[
            "nn",
            "edit",
            "set",
            "--catalog",
            "c.nn.json",
            "--tensor",
            "w",
            "--index",
            "0,0",
            "--value",
            "7",
            "--save-plan",
            "p.json",
        ],
    );
    fs::write(dir.path().join("exists.bin"), b"x").unwrap();
    let (code, _, err) = run_in(
        dir.path(),
        &[
            "nn",
            "edit",
            "apply",
            "--catalog",
            "c.nn.json",
            "--plan",
            "p.json",
            "--out-model",
            "exists.bin",
        ],
    );
    assert_eq!(code, 2);
    assert!(err.contains("already exists"), "stderr: {err}");

    // Edits require content-verified catalogs.
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
            "edit",
            "set",
            "--catalog",
            "weak.nn.json",
            "--tensor",
            "w",
            "--index",
            "0,0",
            "--value",
            "7",
        ],
    );
    assert_eq!(code, 2);
    assert!(err.contains("content-verified"), "stderr: {err}");
}

#[test]
fn undo_refuses_anything_but_the_exact_edited_revision() {
    let dir = tempfile::tempdir().unwrap();
    write_q4_model(dir.path());
    run_in(
        dir.path(),
        &[
            "-i",
            "q.gguf",
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
            "edit",
            "set",
            "--catalog",
            "c.nn.json",
            "--tensor",
            "w",
            "--index",
            "0,0",
            "--value=-2.0",
            "--save-plan",
            "p.json",
        ],
    );
    run_in(
        dir.path(),
        &[
            "nn",
            "edit",
            "apply",
            "--catalog",
            "c.nn.json",
            "--plan",
            "p.json",
            "--out-model",
            "edited.gguf",
            "--undo-bundle",
            "undo",
        ],
    );
    // Undoing the ORIGINAL (not the edited revision) must fail.
    let (code, _, err) = run_in(
        dir.path(),
        &[
            "nn",
            "edit",
            "undo",
            "--bundle",
            "undo",
            "--target",
            "q.gguf",
            "--out-model",
            "bad.gguf",
        ],
    );
    assert_eq!(code, 5);
    assert!(err.contains("exact edited revision"), "stderr: {err}");
}
