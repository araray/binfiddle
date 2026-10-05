//! Integration tests for the final backlog slices: select rebind, GGUF
//! split-shard detection, the ONNX edit tier, and stdin spooling — through
//! the real binary.

use std::fs;
use std::io::Write as _;
use std::path::Path;
use std::process::{Command, Stdio};

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
fn select_rebind_reevaluates_requests_and_reports_additions_removals() {
    let dir = tempfile::tempdir().unwrap();
    // v1 has tensors a and b; v2 keeps a, drops b, adds c.
    write_safetensors(
        dir.path(),
        "v1.safetensors",
        &[("a", vec![2, 2]), ("b", vec![4])],
    );
    write_safetensors(
        dir.path(),
        "v2.safetensors",
        &[("a", vec![2, 2]), ("c", vec![8])],
    );
    run_in(
        dir.path(),
        &[
            "-i",
            "v1.safetensors",
            "nn",
            "discover",
            "--out-catalog",
            "v1.nn.json",
        ],
    );
    run_in(
        dir.path(),
        &[
            "-i",
            "v2.safetensors",
            "nn",
            "discover",
            "--out-catalog",
            "v2.nn.json",
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
            "a",
            "--out-selection",
            "a.sel.json",
        ],
    );

    // Rebind against v2: 'a' still exists (same name, new tensor id).
    let (code, out, _) = run_in(
        dir.path(),
        &[
            "nn",
            "select",
            "--catalog",
            "v2.nn.json",
            "--rebind",
            "a.sel.json",
            "--out-selection",
            "a2.sel.json",
        ],
    );
    assert_eq!(code, 0, "out: {out}");
    assert!(out.contains("(1 tensors)"), "out: {out}");
    // Rebind compares resolved tensor identities: the same name over new
    // bytes yields a new tensor id, reported as one addition + one removal.
    assert!(out.contains("additions: 1"), "out: {out}");
    assert!(out.contains("removals: 1"), "out: {out}");

    // The rebound selection is bound to the NEW catalog (old one refuses).
    let (code, _, err) = run_in(
        dir.path(),
        &[
            "nn",
            "slice",
            "--catalog",
            "v1.nn.json",
            "--selection",
            "a2.sel.json",
            "--dry-run",
        ],
    );
    assert_eq!(code, 5);
    assert!(err.contains("never silently rematch"), "stderr: {err}");
    let (code, _, _) = run_in(
        dir.path(),
        &[
            "nn",
            "slice",
            "--catalog",
            "v2.nn.json",
            "--selection",
            "a2.sel.json",
            "--dry-run",
        ],
    );
    assert_eq!(code, 0);

    // Rebinding a vanished tensor is an honest empty rejection.
    run_in(
        dir.path(),
        &[
            "nn",
            "select",
            "--catalog",
            "v1.nn.json",
            "--tensor",
            "b",
            "--out-selection",
            "b.sel.json",
        ],
    );
    let (code, _, err) = run_in(
        dir.path(),
        &[
            "nn",
            "select",
            "--catalog",
            "v2.nn.json",
            "--rebind",
            "b.sel.json",
        ],
    );
    assert_eq!(code, 2);
    assert!(err.contains("matched no tensors"), "stderr: {err}");

    // Rebinding against the same catalog is a usage error.
    let (code, _, err) = run_in(
        dir.path(),
        &[
            "nn",
            "select",
            "--catalog",
            "v1.nn.json",
            "--rebind",
            "a.sel.json",
        ],
    );
    assert_eq!(code, 2);
    assert!(err.contains("already bound"), "stderr: {err}");
}

/// Build a minimal GGUF shard with one F32 tensor named `w{tag}`.
fn write_gguf_shard(dir: &Path, file: &str, tensor_name: &str) {
    let mut out = Vec::new();
    out.extend_from_slice(b"GGUF");
    out.extend_from_slice(&3u32.to_le_bytes());
    out.extend_from_slice(&1u64.to_le_bytes()); // tensors
    out.extend_from_slice(&1u64.to_le_bytes()); // metadata
    let key = b"general.architecture";
    out.extend_from_slice(&(key.len() as u64).to_le_bytes());
    out.extend_from_slice(key);
    out.extend_from_slice(&8u32.to_le_bytes());
    out.extend_from_slice(&1u64.to_le_bytes());
    out.push(b't');
    out.extend_from_slice(&(tensor_name.len() as u64).to_le_bytes());
    out.extend_from_slice(tensor_name.as_bytes());
    out.extend_from_slice(&2u32.to_le_bytes()); // dims
    out.extend_from_slice(&1u64.to_le_bytes());
    out.extend_from_slice(&4u64.to_le_bytes());
    out.extend_from_slice(&0u32.to_le_bytes()); // F32
    out.extend_from_slice(&0u64.to_le_bytes()); // offset
    while out.len() % 32 != 0 {
        out.push(0);
    }
    out.extend_from_slice(&[0u8; 16]);
    fs::write(dir.join(file), out).unwrap();
}

#[test]
fn gguf_split_shard_groups_are_checked_for_completeness_and_duplicates() {
    let dir = tempfile::tempdir().unwrap();
    // Complete group (2 of 2) with distinct tensor names.
    write_gguf_shard(dir.path(), "model-00001-of-00002.gguf", "wa");
    write_gguf_shard(dir.path(), "model-00002-of-00002.gguf", "wb");
    // Incomplete group: 2 of 3 with only shard 1 and 3 present.
    write_gguf_shard(dir.path(), "other-00001-of-00003.gguf", "x1");
    write_gguf_shard(dir.path(), "other-00003-of-00003.gguf", "x3");

    let (code, out, _) = run_in(dir.path(), &["-i", ".", "nn", "discover"]);
    assert_eq!(code, 0, "out: {out}");
    // Complete group: hints noted, no group findings.
    assert!(
        out.contains("split shard 1 of 2 in group 'model'"),
        "out: {out}"
    );
    assert!(
        !out.contains("[GGUF_SHARD_GROUP_INCOMPLETE] group 'model'"),
        "out: {out}"
    );
    // Incomplete group: completeness finding on both present shards.
    assert!(
        out.contains(
            "[GGUF_SHARD_GROUP_INCOMPLETE] group 'other' declares 3 shards; missing indices 2"
        ),
        "out: {out}"
    );

    // Duplicate tensor names across shards of one group.
    let dir2 = tempfile::tempdir().unwrap();
    write_gguf_shard(dir2.path(), "m-00001-of-00002.gguf", "dupe");
    write_gguf_shard(dir2.path(), "m-00002-of-00002.gguf", "dupe");
    let (code, out, _) = run_in(dir2.path(), &["-i", ".", "nn", "discover"]);
    assert_eq!(code, 0);
    assert!(
        out.contains(
            "[GGUF_SHARD_DUPLICATE_TENSOR] group 'm' repeats tensor names across shards: dupe"
        ),
        "out: {out}"
    );
}

// ONNX protobuf builders (same helpers as the onnx integration tests).
fn varint(mut value: u64) -> Vec<u8> {
    let mut out = Vec::new();
    loop {
        let byte = (value & 0x7f) as u8;
        value >>= 7;
        if value == 0 {
            out.push(byte);
            break;
        }
        out.push(byte | 0x80);
    }
    out
}

fn pb_tag(field: u32, wire: u32) -> Vec<u8> {
    varint(((field as u64) << 3) | wire as u64)
}

fn pb_len(field: u32, payload: &[u8]) -> Vec<u8> {
    let mut out = pb_tag(field, 2);
    out.extend(varint(payload.len() as u64));
    out.extend_from_slice(payload);
    out
}

fn pb_varint(field: u32, value: u64) -> Vec<u8> {
    let mut out = pb_tag(field, 0);
    out.extend(varint(value));
    out
}

#[test]
fn onnx_edit_tier_applies_typed_edits_with_revalidation() {
    let dir = tempfile::tempdir().unwrap();
    // Model: one Add node + one F32 [2] raw-data initializer `w` = [1, 2].
    let payload: Vec<u8> = [1.0f32, 2.0].iter().flat_map(|v| v.to_le_bytes()).collect();
    let mut tensor = Vec::new();
    tensor.extend(pb_len(1, &varint(2))); // packed dims [2]
    tensor.extend(pb_varint(2, 1)); // dtype FLOAT
    tensor.extend(pb_len(8, b"w")); // name
    tensor.extend(pb_len(9, &payload)); // raw_data
    let mut graph = Vec::new();
    let mut node = Vec::new();
    node.extend(pb_len(1, b"x"));
    node.extend(pb_len(2, b"y"));
    node.extend(pb_len(4, b"Add"));
    graph.extend(pb_len(1, &node));
    graph.extend(pb_len(2, b"g"));
    graph.extend(pb_len(5, &tensor));
    let mut model = Vec::new();
    model.extend(pb_varint(1, 9));
    model.extend(pb_len(7, &graph));
    fs::write(dir.path().join("m.onnx"), model).unwrap();

    run_in(
        dir.path(),
        &[
            "-i",
            "m.onnx",
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
            "1",
            "--value",
            "9.5",
            "--save-plan",
            "p.json",
        ],
    );
    assert_eq!(code, 0, "out: {out}");
    assert!(out.contains("2 -> 9.5"), "out: {out}");
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
            "m2.onnx",
        ],
    );
    assert_eq!(code, 0, "out: {out}");
    assert!(
        out.contains("reparsed: container reparse valid"),
        "out: {out}"
    );

    // The edited model re-discovered shows the new value.
    run_in(
        dir.path(),
        &[
            "-i",
            "m2.onnx",
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
    assert_eq!(code, 0, "out: {out}");
    assert!(out.contains("max:      9.5"), "out: {out}");
}

#[test]
fn discover_reads_stdin_through_a_bounded_spool() {
    let dir = tempfile::tempdir().unwrap();
    write_safetensors(dir.path(), "m.safetensors", &[("w", vec![2, 2])]);
    let model = fs::read(dir.path().join("m.safetensors")).unwrap();

    let mut child = binfiddle()
        .args(["-i", "-", "nn", "discover"])
        .current_dir(dir.path())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn");
    child.stdin.as_mut().unwrap().write_all(&model).unwrap();
    let output = child.wait_with_output().expect("wait");
    assert_eq!(output.status.code(), Some(0));
    let out = String::from_utf8_lossy(&output.stdout);
    assert!(out.contains("parsed"), "out: {out}");
    assert!(out.contains("captured stdin stream"), "out: {out}");

    // JSON form carries the spooled source's content-verified identity.
    let mut child = binfiddle()
        .args(["-i", "-", "nn", "discover", "--report-format", "json"])
        .current_dir(dir.path())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn");
    child.stdin.as_mut().unwrap().write_all(&model).unwrap();
    let output = child.wait_with_output().expect("wait");
    assert_eq!(output.status.code(), Some(0));
    let out = String::from_utf8_lossy(&output.stdout);
    assert!(
        out.contains("\"consistency\":\"content_verified\""),
        "out: {out}"
    );
}
