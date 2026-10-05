//! Integration tests for the ONNX reader tier: real binary, synthetic
//! protobuf fixtures built byte-by-byte.

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

// Minimal protobuf builders (mirrors the unit-test helpers).
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

fn tag(field: u32, wire: u32) -> Vec<u8> {
    varint(((field as u64) << 3) | wire as u64)
}

fn len_delim(field: u32, payload: &[u8]) -> Vec<u8> {
    let mut out = tag(field, 2);
    out.extend(varint(payload.len() as u64));
    out.extend_from_slice(payload);
    out
}

fn varint_field(field: u32, value: u64) -> Vec<u8> {
    let mut out = tag(field, 0);
    out.extend(varint(value));
    out
}

fn tensor_raw(name: &str, data_type: u64, dims: &[u64], payload: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    let mut packed_dims = Vec::new();
    for &d in dims {
        packed_dims.extend(varint(d));
    }
    if !packed_dims.is_empty() {
        out.extend(len_delim(1, &packed_dims));
    }
    out.extend(varint_field(2, data_type));
    out.extend(len_delim(8, name.as_bytes()));
    out.extend(len_delim(9, payload));
    out
}

fn tensor_external(name: &str, data_type: u64, dims: &[u64], location: &str) -> Vec<u8> {
    let mut out = Vec::new();
    let mut packed_dims = Vec::new();
    for &d in dims {
        packed_dims.extend(varint(d));
    }
    out.extend(len_delim(1, &packed_dims));
    out.extend(varint_field(2, data_type));
    out.extend(len_delim(8, name.as_bytes()));
    let entry =
        |k: &str, v: &str| [len_delim(1, k.as_bytes()), len_delim(2, v.as_bytes())].concat();
    out.extend(len_delim(13, &entry("location", location)));
    out.extend(len_delim(13, &entry("offset", "0")));
    out.extend(len_delim(13, &entry("length", "64")));
    out.extend(varint_field(14, 1));
    out
}

fn node(op_type: &str) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend(len_delim(1, b"x"));
    out.extend(len_delim(2, b"y"));
    out.extend(len_delim(4, op_type.as_bytes()));
    out
}

fn graph(name: &str, nodes: &[Vec<u8>], tensors: &[Vec<u8>]) -> Vec<u8> {
    let mut out = Vec::new();
    for n in nodes {
        out.extend(len_delim(1, n));
    }
    out.extend(len_delim(2, name.as_bytes()));
    for t in tensors {
        out.extend(len_delim(5, t));
    }
    out
}

fn model(graph_bytes: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend(varint_field(1, 9));
    out.extend(len_delim(7, graph_bytes));
    out
}

#[test]
fn onnx_discover_ls_where_round_trip() {
    let dir = tempfile::tempdir().unwrap();
    // Model: MatMul + Add nodes; F32 [2,2] raw-data initializer `w` filled
    // with 1,2,3,4.
    let mut payload = Vec::new();
    for v in [1.0f32, 2.0, 3.0, 4.0] {
        payload.extend_from_slice(&v.to_le_bytes());
    }
    let graph = graph(
        "g",
        &[node("MatMul"), node("Add")],
        &[tensor_raw("w", 1, &[2, 2], &payload)],
    );
    fs::write(dir.path().join("m.onnx"), model(&graph)).unwrap();

    let (code, out, _) = run_in(
        dir.path(),
        &[
            "-i",
            "m.onnx",
            "nn",
            "discover",
            "--out-catalog",
            "c.nn.json",
        ],
    );
    assert_eq!(code, 0, "out: {out}");
    assert!(out.contains("m.onnx: parsed"), "out: {out}");
    assert!(out.contains("format: onnx ir9"), "out: {out}");

    // Catalog listing shows the tensor with the onnx encoding.
    let (code, out, _) = run_in(dir.path(), &["nn", "ls", "--catalog", "c.nn.json"]);
    assert_eq!(code, 0);
    assert!(out.contains("onnx.float"), "out: {out}");

    // Scalar addressing works through the codec registry: element [1,1] is
    // the fourth float.
    let (code, out, _) = run_in(
        dir.path(),
        &[
            "nn",
            "where",
            "--catalog",
            "c.nn.json",
            "--tensor",
            "w",
            "--index",
            "1,1",
        ],
    );
    assert_eq!(code, 0, "out: {out}");
    assert!(out.contains("precision: exact_contiguous"), "out: {out}");

    // Numerical inspection works: mean of [1,2,3,4] = 2.5.
    let (code, out, _) = run_in(
        dir.path(),
        &["nn", "analyze", "--catalog", "c.nn.json", "--tensor", "w"],
    );
    assert_eq!(code, 0, "out: {out}");
    assert!(out.contains("mean:     2.5"), "out: {out}");

    // Fingerprints cover onnx tensors like any other.
    let (code, out, _) = run_in(dir.path(), &["nn", "fingerprint", "--catalog", "c.nn.json"]);
    assert_eq!(code, 0);
    assert!(out.contains("onnx.float"), "out: {out}");
}

#[test]
fn external_data_tensors_stay_visible_with_unresolved_extents() {
    let dir = tempfile::tempdir().unwrap();
    let graph = graph(
        "g",
        &[],
        &[
            tensor_raw("small", 1, &[2], &[0u8; 8]),
            tensor_external("big", 1, &[4, 4], "weights.bin"),
        ],
    );
    fs::write(dir.path().join("m.onnx"), model(&graph)).unwrap();
    run_in(
        dir.path(),
        &[
            "-i",
            "m.onnx",
            "nn",
            "discover",
            "--out-catalog",
            "c.nn.json",
        ],
    );
    let (code, out, _) = run_in(dir.path(), &["-i", "m.onnx", "nn", "discover"]);
    assert_eq!(code, 0);
    assert!(out.contains("ONNX_EXTERNAL_DATA"), "out: {out}");
    assert!(out.contains("weights.bin"), "out: {out}");

    // The tensor is visible with a bounded extent; exact extraction of it is
    // refused honestly.
    let (code, out, _) = run_in(
        dir.path(),
        &["nn", "show", "--catalog", "c.nn.json", "--tensor", "big"],
    );
    assert_eq!(code, 0);
    assert!(out.contains("extent only bounded"), "out: {out}");

    run_in(
        dir.path(),
        &[
            "nn",
            "select",
            "--catalog",
            "c.nn.json",
            "--tensor",
            "big",
            "--out-selection",
            "b.sel.json",
        ],
    );
    let (code, _, err) = run_in(
        dir.path(),
        &[
            "nn",
            "slice",
            "--catalog",
            "c.nn.json",
            "--selection",
            "b.sel.json",
            "--dry-run",
        ],
    );
    assert_eq!(code, 2);
    assert!(err.contains("bounded extent"), "stderr: {err}");
}

#[test]
fn non_onnx_files_stay_unrecognized() {
    let dir = tempfile::tempdir().unwrap();
    fs::write(dir.path().join("junk.onnx"), b"certainly not protobuf").unwrap();
    let (code, out, _) = run_in(dir.path(), &["-i", "junk.onnx", "nn", "discover"]);
    assert_eq!(code, 0);
    assert!(out.contains("junk.onnx: unrecognized"), "out: {out}");
}
