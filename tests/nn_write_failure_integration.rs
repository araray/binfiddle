//! REQ-036 write-failure fault paths: edit apply and prune against
//! unwritable destinations must fail cleanly (exit 6, IO error) with no
//! partial output left behind.

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
    let header = r#"{"w":{"dtype":"F32","shape":[2,2],"data_offsets":[0,16]}}"#;
    let mut data = Vec::new();
    data.extend_from_slice(&(header.len() as u64).to_le_bytes());
    data.extend_from_slice(header.as_bytes());
    for v in [1.0f32, 2.0, 3.0, 4.0] {
        data.extend_from_slice(&v.to_le_bytes());
    }
    fs::write(dir.join("m.safetensors"), data).unwrap();
}

/// Create a guard path whose interior cannot receive new files, so any
/// destination under it fails at the OS on creation.
///
/// Unix: a 0o555 directory (permission failure). Windows: the readonly
/// attribute on directories does not block file creation, so the guard is a
/// regular file — a child path then fails because a path component is not a
/// directory.
fn make_unwritable_dir(parent: &Path, name: &str) -> std::path::PathBuf {
    let guard = parent.join(name);
    #[cfg(unix)]
    {
        fs::create_dir_all(&guard).unwrap();
        let mut perms = fs::metadata(&guard).unwrap().permissions();
        std::os::unix::fs::PermissionsExt::set_mode(&mut perms, 0o555);
        fs::set_permissions(&guard, perms).unwrap();
    }
    #[cfg(windows)]
    {
        fs::write(&guard, b"not a directory").unwrap();
    }
    guard
}

/// Root bypasses Unix mode bits, so the permission guard is not exercised
/// when privileged. The Windows guard fails regardless of privileges.
#[cfg(unix)]
fn running_as_root() -> bool {
    let euid = unsafe { libc::geteuid() };
    euid == 0
}

#[cfg(windows)]
fn running_as_root() -> bool {
    false
}

#[test]
fn edit_apply_to_unwritable_destination_fails_cleanly() {
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
            "edit",
            "set",
            "--catalog",
            "c.nn.json",
            "--tensor",
            "w",
            "--index",
            "0,0",
            "--value",
            "9",
            "--save-plan",
            "p.json",
        ],
    );

    // A destination root that cannot receive files: creation fails at the OS.
    let ro = make_unwritable_dir(dir.path(), "ro");

    let (code, out, err) = run_in(
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
            "ro/out.safetensors",
        ],
    );
    // Root can bypass Unix mode bits; otherwise the failure must be clean.
    if !running_as_root() {
        assert_eq!(code, 6, "out: {out} stderr: {err}");
        assert!(
            err.contains("I/O error")
                || err.contains("Permission denied")
                || err.contains("denied")
                || err.contains("invalid"),
            "stderr: {err}"
        );
        // No partial artifact left behind under the final name.
        assert!(!ro.join("out.safetensors").exists());
    }

    // The original model is untouched: 8-byte length prefix + header + the
    // exact original 16 payload bytes (1,2,3,4 as F32 LE).
    let after = fs::read(dir.path().join("m.safetensors")).unwrap();
    let header_len = u64::from_le_bytes(after[0..8].try_into().unwrap()) as usize;
    assert_eq!(after.len(), 8 + header_len + 16, "length sanity");
    assert!(after.ends_with(&[0, 0, 128, 63, 0, 0, 0, 64, 0, 0, 64, 64, 0, 0, 128, 64]));
}

#[test]
fn prune_to_unwritable_destination_fails_cleanly() {
    let dir = tempfile::tempdir().unwrap();
    // Minimal MLP model + pack (reuse the component test shapes).
    let yaml = r#"schema: binfiddle.nn.pack/v1
id: prune.test
version: "1.0.0"
config:
  hidden_size: 4
  intermediate_size: 6
bindings:
  - pattern: "model.layers.{layer}.mlp.gate_proj.weight"
    component: "decoder.layers[{layer}].mlp.gate"
    kind: mlp_gate
    shape: ["intermediate_size", "hidden_size"]
  - pattern: "model.layers.{layer}.mlp.up_proj.weight"
    component: "decoder.layers[{layer}].mlp.up"
    kind: mlp_up
    shape: ["intermediate_size", "hidden_size"]
  - pattern: "model.layers.{layer}.mlp.down_proj.weight"
    component: "decoder.layers[{layer}].mlp.down"
    kind: mlp_down
    shape: ["hidden_size", "intermediate_size"]
"#;
    fs::write(dir.path().join("pack.yaml"), yaml).unwrap();
    let mut tensors: Vec<(String, Vec<u64>)> = Vec::new();
    for layer in 0..1 {
        let l = layer.to_string();
        tensors.push((format!("model.layers.{l}.mlp.gate_proj.weight"), vec![6, 4]));
        tensors.push((format!("model.layers.{l}.mlp.up_proj.weight"), vec![6, 4]));
        tensors.push((format!("model.layers.{l}.mlp.down_proj.weight"), vec![4, 6]));
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
    fs::write(dir.path().join("m.safetensors"), data).unwrap();
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

    let ro = make_unwritable_dir(dir.path(), "ro");

    let (code, _, err) = run_in(
        dir.path(),
        &[
            "nn",
            "edit",
            "prune",
            "--catalog",
            "c.nn.json",
            "--pack",
            "pack.yaml",
            "--channels",
            "0",
            "--out-model",
            "ro/pruned.safetensors",
        ],
    );
    if !running_as_root() {
        assert_eq!(code, 6, "stderr: {err}");
        assert!(!ro.join("pruned.safetensors").exists());
    }
}
