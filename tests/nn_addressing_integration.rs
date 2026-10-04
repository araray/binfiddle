//! Integration tests for `nn where` / `nn locate` and the codec layer:
//! real binary, real files, corpus reference vectors.

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

/// Build a safetensors file with an F32 [2,2] tensor `w`.
fn write_f32_model(dir: &Path) {
    let header = r#"{"w":{"dtype":"F32","shape":[2,2],"data_offsets":[0,16]}}"#;
    let mut data = Vec::new();
    data.extend_from_slice(&(header.len() as u64).to_le_bytes());
    data.extend_from_slice(header.as_bytes());
    data.extend_from_slice(&[0u8; 16]);
    fs::write(dir.join("m.safetensors"), data).unwrap();
}

/// Build a GGUF file with one Q4_0 [1,32] tensor whose payload is the corpus
/// fixture block: scale `00 38` (F16 0.5) and first code byte `A3`.
fn write_q4_model(dir: &Path) {
    let mut out = Vec::new();
    out.extend_from_slice(b"GGUF");
    out.extend_from_slice(&3u32.to_le_bytes());
    out.extend_from_slice(&1u64.to_le_bytes()); // tensor count
    out.extend_from_slice(&1u64.to_le_bytes()); // metadata count
    let key = "general.architecture";
    out.extend_from_slice(&(key.len() as u64).to_le_bytes());
    out.extend_from_slice(key.as_bytes());
    out.extend_from_slice(&8u32.to_le_bytes()); // string
    out.extend_from_slice(&1u64.to_le_bytes());
    out.push(b't');
    out.extend_from_slice(&1u64.to_le_bytes()); // name length
    out.push(b'w');
    out.extend_from_slice(&2u32.to_le_bytes()); // dims
    out.extend_from_slice(&1u64.to_le_bytes());
    out.extend_from_slice(&32u64.to_le_bytes());
    out.extend_from_slice(&2u32.to_le_bytes()); // Q4_0
    out.extend_from_slice(&0u64.to_le_bytes()); // offset
    while out.len() % 32 != 0 {
        out.push(0);
    }
    let payload_start = out.len();
    out.push(0x00);
    out.push(0x38);
    out.push(0xA3);
    out.extend(std::iter::repeat_n(0u8, 15));
    let total = out.len();
    fs::write(dir.join("q.gguf"), &out).unwrap();
    assert_eq!(total, payload_start + 18);
}

#[test]
fn where_locate_f32_round_trip() {
    let dir = tempfile::tempdir().unwrap();
    write_f32_model(dir.path());
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
    // Map element [1,0], parse its span, and confirm locate resolves the
    // span start back to the same element (no hard-coded offsets).
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
            "1,0",
        ],
    );
    assert_eq!(code, 0);
    assert!(out.contains("precision: exact_contiguous"), "out: {out}");
    let span_line = out
        .lines()
        .find(|l| l.contains("file span:"))
        .expect("span line");
    let mut parts = span_line.split(['[', ',']);
    let _ = parts.next();
    let start: u64 = parts.next().expect("start").trim().parse().expect("number");
    let end: u64 = parts
        .next()
        .expect("end")
        .trim()
        .trim_end_matches(')')
        .parse()
        .expect("number");
    assert_eq!(end - start, 4, "F32 element must span four bytes");

    let (code, out, _) = run_in(
        dir.path(),
        &[
            "nn",
            "locate",
            "--catalog",
            "c.nn.json",
            "--offset",
            &start.to_string(),
        ],
    );
    assert_eq!(code, 0);
    assert!(out.contains("w: element [1,0]"), "out: {out}");
}

#[test]
fn where_q4_0_reports_bits_and_dependencies() {
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
            "c.nn.json",
        ],
    );
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
            "0,0",
        ],
    );
    assert_eq!(code, 0);
    assert!(out.contains("precision: exact_bits"), "out: {out}");
    assert!(out.contains("mask 0x0f shift 0"), "out: {out}");
    assert!(out.contains("lsb0"), "out: {out}");
    // Decode dependencies include the 2-byte scale before the code byte.
    let dependencies: Vec<&str> = out
        .lines()
        .filter(|l| l.trim_start().starts_with('['))
        .map(|l| l.trim())
        .collect();
    assert!(dependencies.len() >= 2, "out: {out}");

    // Element 16 shares the code byte at the high nibble.
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
            "0,16",
        ],
    );
    assert_eq!(code, 0);
    assert!(out.contains("mask 0xf0 shift 4"), "out: {out}");
}

#[test]
fn locate_q4_0_scale_and_code_roles() {
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
            "c.nn.json",
        ],
    );
    // Find the payload start from the whole-tensor where output.
    let (code, out, _) = run_in(
        dir.path(),
        &["nn", "where", "--catalog", "c.nn.json", "--tensor", "w"],
    );
    assert_eq!(code, 0);
    let start_line = out
        .lines()
        .find(|l| l.contains("file span:"))
        .expect("span line");
    let start: u64 = start_line
        .split(['[', ','])
        .nth(1)
        .expect("start")
        .trim()
        .parse()
        .expect("number");

    let (code, out, _) = run_in(
        dir.path(),
        &[
            "nn",
            "locate",
            "--catalog",
            "c.nn.json",
            "--offset",
            &start.to_string(),
        ],
    );
    assert_eq!(code, 0);
    assert!(out.contains("payload start"), "out: {out}");

    let (code, out, _) = run_in(
        dir.path(),
        &[
            "nn",
            "locate",
            "--catalog",
            "c.nn.json",
            "--offset",
            &(start + 1).to_string(),
        ],
    );
    assert_eq!(code, 0);
    assert!(out.contains("scale of block 0"), "out: {out}");

    let (code, out, _) = run_in(
        dir.path(),
        &[
            "nn",
            "locate",
            "--catalog",
            "c.nn.json",
            "--offset",
            &(start + 2).to_string(),
        ],
    );
    assert_eq!(code, 0);
    assert!(out.contains("low nibble = element 0"), "out: {out}");
    assert!(out.contains("high nibble = element 16"), "out: {out}");
}

#[test]
fn where_rejects_bad_coordinates_and_spaces() {
    let dir = tempfile::tempdir().unwrap();
    write_f32_model(dir.path());
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
            "where",
            "--catalog",
            "c.nn.json",
            "--tensor",
            "w",
            "--index",
            "5,0",
        ],
    );
    assert_eq!(code, 2);
    assert!(err.contains("out of bounds"), "stderr: {err}");

    let (code, _, err) = run_in(
        dir.path(),
        &[
            "nn",
            "where",
            "--catalog",
            "c.nn.json",
            "--tensor",
            "w",
            "--index",
            "x,y",
        ],
    );
    assert_eq!(code, 2);
    assert!(err.contains("decimal"), "stderr: {err}");

    let (code, _, _) = run_in(
        dir.path(),
        &[
            "nn",
            "where",
            "--catalog",
            "c.nn.json",
            "--tensor",
            "w",
            "--space",
            "device",
        ],
    );
    assert_eq!(code, 2);
}

#[test]
fn locate_unowned_offset_is_honest() {
    let dir = tempfile::tempdir().unwrap();
    write_f32_model(dir.path());
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
    // Header bytes are not owned by any tensor.
    let (code, out, _) = run_in(
        dir.path(),
        &["nn", "locate", "--catalog", "c.nn.json", "--offset", "3"],
    );
    assert_eq!(code, 0);
    assert!(
        out.contains("no exact-extent tensor owns this offset"),
        "out: {out}"
    );
}

/// The codec library layer reproduces the published corpus Q4_0 fixture
/// values: scale 0.5 (`00 38`), first code byte `A3` → element 0 = −2.5,
/// element 16 = +1.0.
#[test]
fn library_q4_0_corpus_fixture_values() {
    let mut block = [0u8; 18];
    block[0] = 0x00;
    block[1] = 0x38;
    block[2] = 0xA3;
    let decoded = binfiddle::nn::codec::q4_0_decode_block(&block).unwrap();
    assert_eq!(decoded.scale, 0.5);
    assert_eq!(decoded.values[0], -2.5);
    assert_eq!(decoded.values[16], 1.0);
}

/// The published dense addressing vector: BF16 [4096,4096] at 0x01000000,
/// coordinate [123,456] → 0x010F6390.
#[test]
fn library_bf16_address_reference_vector() {
    let offset =
        binfiddle::nn::address::dense_element_offset(&[4096, 4096], &[123, 456], 2, 0x0100_0000)
            .unwrap();
    assert_eq!(offset, 0x010F_6390);
}
