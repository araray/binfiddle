//! Integration tests for streaming `analyze --block-size`.

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
fn streaming_analyze_entropy_per_block() {
    let input = tempfile::NamedTempFile::new().unwrap();
    // Two blocks: one uniform, one mixed.
    let mut data = vec![0x00u8; 4];
    data.extend(vec![0x00, 0x11, 0x22, 0x33]);
    std::fs::write(input.path(), &data).unwrap();

    let output = Command::new(binfiddle())
        .arg("--input")
        .arg(input.path())
        .arg("analyze")
        .arg("entropy")
        .arg("--block-size")
        .arg("4")
        .output()
        .expect("failed to run binfiddle analyze");

    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("Blocks: 2"),
        "Expected 2 blocks, got: {}",
        stdout
    );
    assert!(
        stdout.contains("0x00000000"),
        "Expected first block offset, got: {}",
        stdout
    );
    assert!(
        stdout.contains("0x00000004"),
        "Expected second block offset, got: {}",
        stdout
    );
}

#[test]
fn streaming_analyze_histogram_accumulates() {
    let input = tempfile::NamedTempFile::new().unwrap();
    std::fs::write(input.path(), [0xAA, 0xAA, 0xBB, 0xCC]).unwrap();

    let output = Command::new(binfiddle())
        .arg("--input")
        .arg(input.path())
        .arg("analyze")
        .arg("histogram")
        .arg("--block-size")
        .arg("2")
        .output()
        .expect("failed to run binfiddle analyze");

    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("Total bytes: 4"),
        "Expected total bytes 4, got: {}",
        stdout
    );
    assert!(
        stdout.contains("0xaa"),
        "Expected 0xaa in histogram, got: {}",
        stdout
    );
}

#[test]
fn streaming_analyze_from_stdin() {
    let mut data = vec![0x00u8; 8];
    data.extend(vec![0xFFu8; 8]);

    let mut child = Command::new(binfiddle())
        .arg("--input")
        .arg("-")
        .arg("analyze")
        .arg("entropy")
        .arg("--block-size")
        .arg("8")
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .expect("failed to spawn binfiddle analyze");

    let mut stdin = child.stdin.take().expect("failed to open stdin");
    std::thread::spawn(move || {
        stdin.write_all(&data).unwrap();
    });

    let output = child
        .wait_with_output()
        .expect("failed to wait on binfiddle analyze");

    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("Blocks: 2"),
        "Expected 2 blocks from stdin, got: {}",
        stdout
    );
}

#[test]
fn streaming_analyze_rejects_range() {
    let input = tempfile::NamedTempFile::new().unwrap();
    std::fs::write(input.path(), [0x00; 8]).unwrap();

    let output = Command::new(binfiddle())
        .arg("--input")
        .arg(input.path())
        .arg("analyze")
        .arg("entropy")
        .arg("--block-size")
        .arg("4")
        .arg("--range")
        .arg("0..4")
        .output()
        .expect("failed to run binfiddle analyze");

    assert!(
        !output.status.success(),
        "Expected --range with streaming analyze to fail"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("not supported"),
        "Expected unsupported error, got: {}",
        stderr
    );
}
