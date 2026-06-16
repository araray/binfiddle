//! Integration tests for the streaming `search --block-size` path.

use std::io::Write;
use std::process::Command;

fn binfiddle() -> std::path::PathBuf {
    std::env::var_os("CARGO_BIN_EXE_binfiddle")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| {
            std::env::current_exe()
                .unwrap()
                .parent()
                .unwrap()
                .parent()
                .unwrap()
                .join("binfiddle")
        })
}

#[test]
fn streaming_search_finds_exact_match() {
    let input = tempfile::NamedTempFile::new().unwrap();
    std::fs::write(input.path(), [0xDE, 0xAD, 0xBE, 0xEF, 0x00]).unwrap();

    let output = Command::new(binfiddle())
        .arg("--input")
        .arg(input.path())
        .arg("search")
        .arg("DEADBEEF")
        .arg("--all")
        .arg("--block-size")
        .arg("2")
        .output()
        .expect("failed to run binfiddle search");

    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("0x00000000"),
        "Expected match at offset 0, got: {}",
        stdout
    );
}

#[test]
fn streaming_search_finds_boundary_match() {
    // Pattern straddles the boundary between 4-byte blocks.
    let mut data = vec![0x00u8; 10];
    data[3..7].copy_from_slice(&[0xCA, 0xFE, 0xBA, 0xBE]);

    let input = tempfile::NamedTempFile::new().unwrap();
    std::fs::write(input.path(), &data).unwrap();

    let output = Command::new(binfiddle())
        .arg("--input")
        .arg(input.path())
        .arg("search")
        .arg("CAFEBABE")
        .arg("--all")
        .arg("--offsets-only")
        .arg("--block-size")
        .arg("4")
        .output()
        .expect("failed to run binfiddle search");

    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert_eq!(
        stdout.trim(),
        "0x00000003",
        "Expected boundary match at offset 3, got: {}",
        stdout
    );
}

#[test]
fn streaming_search_finds_ascii_pattern() {
    let input = tempfile::NamedTempFile::new().unwrap();
    std::fs::write(input.path(), b"hello world").unwrap();

    let output = Command::new(binfiddle())
        .arg("--input")
        .arg(input.path())
        .arg("search")
        .arg("world")
        .arg("--input-format")
        .arg("ascii")
        .arg("--all")
        .arg("--block-size")
        .arg("3")
        .output()
        .expect("failed to run binfiddle search");

    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("0x00000006"),
        "Expected match at offset 6, got: {}",
        stdout
    );
}

#[test]
fn streaming_search_from_stdin() {
    let mut data = vec![0x00u8; 10];
    data[7..9].copy_from_slice(&[0xAA, 0xBB]);

    let mut child = Command::new(binfiddle())
        .arg("--input")
        .arg("-")
        .arg("search")
        .arg("AABB")
        .arg("--all")
        .arg("--block-size")
        .arg("3")
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .expect("failed to spawn binfiddle search");

    let mut stdin = child.stdin.take().expect("failed to open stdin");
    std::thread::spawn(move || {
        stdin.write_all(&data).unwrap();
    });

    let output = child
        .wait_with_output()
        .expect("failed to wait on binfiddle search");

    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("0x00000007"),
        "Expected match at offset 7, got: {}",
        stdout
    );
}

#[test]
fn streaming_search_rejects_regex() {
    let input = tempfile::NamedTempFile::new().unwrap();
    std::fs::write(input.path(), b"hello").unwrap();

    let output = Command::new(binfiddle())
        .arg("--input")
        .arg(input.path())
        .arg("search")
        .arg(".*")
        .arg("--input-format")
        .arg("regex")
        .arg("--all")
        .arg("--block-size")
        .arg("4")
        .output()
        .expect("failed to run binfiddle search");

    assert!(
        !output.status.success(),
        "Expected regex + --block-size to fail"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("not supported"),
        "Expected unsupported error, got: {}",
        stderr
    );
}
