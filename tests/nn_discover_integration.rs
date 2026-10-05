//! Integration tests for `nn discover`: real binary, real files, envelope and
//! exit-code contracts.

use std::fs;
use std::process::Command;

fn binfiddle() -> Command {
    Command::new(env!("CARGO_BIN_EXE_binfiddle"))
}

fn run_in(dir: &std::path::Path, args: &[&str]) -> (i32, String, String) {
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

fn write_safetensors(dir: &std::path::Path, name: &str, tensors: &[(&str, &str, &[u64], &[u8])]) {
    let mut body: Vec<u8> = Vec::new();
    let mut spans = Vec::new();
    for (name, dtype, shape, payload) in tensors {
        let begin = body.len();
        body.extend_from_slice(payload);
        spans.push((name, dtype, shape, begin, begin + payload.len()));
    }
    let shape_text = |dims: &[u64]| -> String {
        dims.iter()
            .map(|d| d.to_string())
            .collect::<Vec<_>>()
            .join(",")
    };
    let entries: Vec<String> = spans
        .iter()
        .map(|(n, dt, sh, b, e)| {
            format!(
                "\"{}\":{{\"dtype\":\"{}\",\"shape\":[{}],\"data_offsets\":[{},{}]}}",
                n,
                dt,
                shape_text(sh),
                b,
                e
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

fn write_gguf(dir: &std::path::Path, name: &str, tensor_name: &str, dims: &[u64]) {
    // One F32 tensor; payload zero-filled to the exact element count.
    let elements: u64 = dims.iter().product();
    let mut out = Vec::new();
    out.extend_from_slice(b"GGUF");
    out.extend_from_slice(&3u32.to_le_bytes());
    out.extend_from_slice(&1u64.to_le_bytes()); // tensor count
    out.extend_from_slice(&1u64.to_le_bytes()); // metadata count
                                                // metadata: general.architecture = "t" (string)
    let key = "general.architecture";
    out.extend_from_slice(&(key.len() as u64).to_le_bytes());
    out.extend_from_slice(key.as_bytes());
    out.extend_from_slice(&8u32.to_le_bytes()); // string
    out.extend_from_slice(&1u64.to_le_bytes());
    out.push(b't');
    // tensor directory
    out.extend_from_slice(&(tensor_name.len() as u64).to_le_bytes());
    out.extend_from_slice(tensor_name.as_bytes());
    out.extend_from_slice(&(dims.len() as u32).to_le_bytes());
    for dim in dims {
        out.extend_from_slice(&dim.to_le_bytes());
    }
    out.extend_from_slice(&0u32.to_le_bytes()); // F32
    out.extend_from_slice(&0u64.to_le_bytes()); // offset 0
                                                // align to 32 and append payload
    while out.len() % 32 != 0 {
        out.push(0);
    }
    out.extend(std::iter::repeat_n(0u8, elements as usize * 4));
    fs::write(dir.join(name), out).unwrap();
}

#[test]
fn discover_safetensors_file_reports_tensor_inventory() {
    let dir = tempfile::tempdir().unwrap();
    write_safetensors(
        dir.path(),
        "model.safetensors",
        &[
            ("weight", "F32", &[2, 3], &[0u8; 24]),
            ("bias", "U8", &[3], &[1, 2, 3]),
        ],
    );
    let (code, out, _) = run_in(dir.path(), &["-i", "model.safetensors", "nn", "discover"]);
    assert_eq!(code, 0);
    assert!(out.contains("model.safetensors: parsed"), "out: {out}");
    assert!(out.contains("format: safetensors 1"));
    assert!(out.contains("tensors: 2"));
    assert!(out.contains("tensor weight [2x3] 6 elements, encoding safetensors.F32"));

    let (code, out, _) = run_in(
        dir.path(),
        &[
            "-i",
            "model.safetensors",
            "nn",
            "discover",
            "--report-format",
            "json",
        ],
    );
    assert_eq!(code, 0);
    assert!(out.contains("\"operation\":\"discover\""));
    assert!(out.contains("\"schema\":\"binfiddle.nn.result/v1\""));
    assert!(out.contains("\"name\":\"weight\""));
    assert!(out.contains("\"payload_start\":\""));
    assert!(out.contains("\"complete\":true"));
}

#[test]
fn discover_gguf_file_reports_tensor_inventory() {
    let dir = tempfile::tempdir().unwrap();
    write_gguf(dir.path(), "model.gguf", "tok_embd.weight", &[2, 4]);
    let (code, out, _) = run_in(dir.path(), &["-i", "model.gguf", "nn", "discover"]);
    assert_eq!(code, 0);
    assert!(out.contains("model.gguf: parsed"), "out: {out}");
    assert!(out.contains("format: gguf v3"));
    assert!(out.contains("tensor tok_embd.weight [2x4] 8 elements, encoding ggml.f32"));
}

#[test]
fn discover_directory_scans_and_classifies() {
    let dir = tempfile::tempdir().unwrap();
    write_safetensors(dir.path(), "a.safetensors", &[("w", "U8", &[1], &[9])]);
    write_gguf(dir.path(), "b.gguf", "w", &[1]);
    fs::write(dir.path().join("config.json"), b"{}").unwrap();
    fs::write(dir.path().join("notes.md"), b"# hi").unwrap();
    let (code, out, _) = run_in(dir.path(), &["-i", ".", "nn", "discover"]);
    assert_eq!(code, 0);
    assert!(out.contains("a.safetensors: parsed"));
    assert!(out.contains("b.gguf: parsed"));
    assert!(out.contains("config.json: asset (json-configuration)"));
    assert!(out.contains("notes.md: asset (documentation)"));
    assert!(out.contains("coverage: 2 of 2 sources parsed"));
}

#[test]
fn discover_reports_unrecognized_and_require_complete_exits_eight() {
    let dir = tempfile::tempdir().unwrap();
    fs::write(dir.path().join("mystery.bin"), b"some random bytes").unwrap();
    let (code, out, _) = run_in(dir.path(), &["-i", "mystery.bin", "nn", "discover"]);
    assert_eq!(code, 0, "plain discover reports and exits zero");
    assert!(out.contains("mystery.bin: unrecognized"));

    let (code, _, err) = run_in(
        dir.path(),
        &["-i", "mystery.bin", "nn", "discover", "--require-complete"],
    );
    assert_eq!(code, 8);
    assert!(err.contains("incomplete result rejected"), "stderr: {err}");
}

#[test]
fn discover_missing_input_is_usage_error() {
    let dir = tempfile::tempdir().unwrap();
    let (code, _, err) = run_in(dir.path(), &["nn", "discover"]);
    assert_eq!(code, 2);
    assert!(err.contains("requires --input"), "stderr: {err}");
}

#[test]
fn discover_stdin_spools_the_stream() {
    // Stdin discovery is now supported via a bounded private spool; a model
    // piped in parses and reports the spool provenance note. (The deep
    // assertions live in nn_backlog_integration.)
    let dir = tempfile::tempdir().unwrap();
    write_safetensors(dir.path(), "m.safetensors", &[("w", "U8", &[1], &[7])]);
    let model = std::fs::read(dir.path().join("m.safetensors")).unwrap();
    use std::io::Write;
    use std::process::Stdio;
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
}

#[test]
fn discover_missing_file_is_source_missing() {
    let dir = tempfile::tempdir().unwrap();
    let (code, _, err) = run_in(dir.path(), &["-i", "nope.gguf", "nn", "discover"]);
    assert_eq!(code, 5);
    assert!(err.contains("source missing"), "stderr: {err}");
}

#[test]
fn discover_verify_content_adds_digest() {
    let dir = tempfile::tempdir().unwrap();
    write_safetensors(dir.path(), "m.safetensors", &[("w", "U8", &[1], &[7])]);
    let (code, out, _) = run_in(
        dir.path(),
        &[
            "-i",
            "m.safetensors",
            "nn",
            "discover",
            "--verify-content",
            "--report-format",
            "json",
        ],
    );
    assert_eq!(code, 0);
    assert!(out.contains("\"consistency\":\"content_verified\""));
    assert!(out.contains("\"content_digest\":\""));
}

#[test]
fn discover_reports_malformed_safetensors() {
    let dir = tempfile::tempdir().unwrap();
    // Header claims 40 bytes of JSON but the file is truncated.
    let mut data = Vec::new();
    data.extend_from_slice(&40u64.to_le_bytes());
    data.extend_from_slice(b"{\"a\":{\"dtype\":\"U8\"");
    fs::write(dir.path().join("trunc.safetensors"), data).unwrap();
    let (code, out, _) = run_in(dir.path(), &["-i", "trunc.safetensors", "nn", "discover"]);
    assert_eq!(code, 0);
    assert!(out.contains("trunc.safetensors: malformed"), "out: {out}");
}

#[test]
fn nn_capabilities_lists_discover_as_implemented() {
    let (code, out, _) = run_in(std::path::Path::new("/"), &["nn", "capabilities"]);
    assert_eq!(code, 0);
    assert!(out.contains("nn.discover"));
    assert!(!out.contains("nn.discover    artifact inventory is not implemented"));
}
