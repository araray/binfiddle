//! Multi-write edit sessions (P2): stage several operations, apply them in
//! one copy pass, undo them atomically against the exact edited revision.

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

fn write_model(dir: &Path) {
    let header = r#"{"w":{"dtype":"F32","shape":[4,4],"data_offsets":[0,64]},"v":{"dtype":"F32","shape":[2,2],"data_offsets":[64,80]}}"#;
    let mut data = Vec::new();
    data.extend_from_slice(&(header.len() as u64).to_le_bytes());
    data.extend_from_slice(header.as_bytes());
    for i in 0..20u32 {
        data.extend_from_slice(&(i as f32 / 4.0).to_le_bytes());
    }
    fs::write(dir.join("m.safetensors"), data).unwrap();
}

#[test]
fn session_stages_applies_and_undoes_multiple_ops() {
    let dir = tempfile::tempdir().unwrap();
    write_model(dir.path());
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
    assert_eq!(code, 0, "{err}");

    let session = dir.path().join("sess");
    for (tensor, index, value) in [
        ("w", "0,0", "101"),
        ("w", "3,3", "202"),
        ("v", "1,1", "303"),
    ] {
        let (code, out, err) = run_in(
            dir.path(),
            &[
                "nn",
                "edit",
                "set",
                "--catalog",
                "c.nn.json",
                "--tensor",
                tensor,
                "--index",
                index,
                "--value",
                value,
                "--session",
                session.to_str().unwrap(),
            ],
        );
        assert_eq!(code, 0, "stage {tensor}: {out} {err}");
    }
    // Three staged operations live in one id-verified session file.
    let session_text = fs::read_to_string(session.join("session.json")).unwrap();
    assert_eq!(session_text.matches("\"tensor_name\"").count(), 3);

    let (code, out, err) = run_in(
        dir.path(),
        &[
            "nn",
            "edit",
            "apply",
            "--catalog",
            "c.nn.json",
            "--session",
            session.to_str().unwrap(),
            "--out-model",
            "edited.safetensors",
            "--undo-bundle",
            "undo",
        ],
    );
    assert_eq!(code, 0, "{out} {err}");
    assert!(out.contains("ops:      3"), "{out}");
    assert!(
        out.contains("preserved: all bytes outside the planned spans"),
        "{out}"
    );

    // Every planned change landed; nothing else moved.
    let original = fs::read(dir.path().join("m.safetensors")).unwrap();
    let edited = fs::read(dir.path().join("edited.safetensors")).unwrap();
    assert_eq!(original.len(), edited.len());
    let header_len = u64::from_le_bytes(original[0..8].try_into().unwrap()) as usize;
    let w0 = u32::from_le_bytes(
        edited[8 + header_len..8 + header_len + 4]
            .try_into()
            .unwrap(),
    );
    assert_eq!(f32::from_bits(w0), 101.0);
    let v11 = u32::from_le_bytes(
        edited[8 + header_len + 76..8 + header_len + 80]
            .try_into()
            .unwrap(),
    );
    assert_eq!(f32::from_bits(v11), 303.0);
    // Only the three planned units changed: every other byte is identical.
    let w_unit = 8 + header_len;
    for (i, (a, b)) in original.iter().zip(edited.iter()).enumerate() {
        let in_w00 = i >= w_unit && i < w_unit + 4;
        let in_w33 = i >= w_unit + 60 && i < w_unit + 64;
        let in_v11 = i >= w_unit + 76 && i < w_unit + 80;
        if !in_w00 && !in_w33 && !in_v11 {
            assert_eq!(a, b, "byte {i} outside every planned unit changed");
        }
    }

    // Undo reverses all three atomically.
    let (code, _, err) = run_in(
        dir.path(),
        &[
            "nn",
            "edit",
            "undo",
            "--bundle",
            "undo",
            "--target",
            "edited.safetensors",
            "--out-model",
            "restored.safetensors",
        ],
    );
    assert_eq!(code, 0, "{err}");
    assert_eq!(
        fs::read(dir.path().join("m.safetensors")).unwrap(),
        fs::read(dir.path().join("restored.safetensors")).unwrap(),
        "undo must restore the original byte-for-byte"
    );
}

#[test]
fn sessions_refuse_overlapping_and_foreign_sources() {
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
    let session = dir.path().join("sess");
    // Same write unit staged twice → conflict at staging time.
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
            "11",
            "--session",
            session.to_str().unwrap(),
        ],
    );
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
            "12",
            "--session",
            session.to_str().unwrap(),
        ],
    );
    assert_eq!(code, 5, "write conflict: {err}");
    assert!(err.contains("overlap"), "{err}");

    // A tampered session file is never silently replaced.
    let mut tampered = fs::read_to_string(session.join("session.json")).unwrap();
    tampered.push(' ');
    fs::write(session.join("session.json"), "{broken").unwrap();
    let (code, _, err) = run_in(
        dir.path(),
        &[
            "nn",
            "edit",
            "set",
            "--catalog",
            "c.nn.json",
            "--tensor",
            "v",
            "--index",
            "0,0",
            "--value",
            "1",
            "--session",
            session.to_str().unwrap(),
        ],
    );
    assert_ne!(code, 0, "tampered session must fail loudly: {err}");
    let _ = tampered;
}
