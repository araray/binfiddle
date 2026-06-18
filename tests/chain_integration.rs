//! Integration tests for the `chain` command.
//!
//! These tests invoke the compiled `binfiddle` binary via `std::process::Command`.

use std::io::Write;
use std::process::Command;

/// Returns the path to the compiled `binfiddle` binary.
fn binfiddle() -> std::path::PathBuf {
    std::env::var_os("CARGO_BIN_EXE_binfiddle")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| {
            // Fallback for running the test binary directly.
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
fn chain_two_byte_steps_produces_file() {
    let input = tempfile::NamedTempFile::new().unwrap();
    std::fs::write(input.path(), [0x00, 0x11, 0x22, 0x33]).unwrap();

    let output = tempfile::NamedTempFile::new().unwrap();

    let status = Command::new(binfiddle())
        .arg("--input")
        .arg(input.path())
        .arg("--output")
        .arg(output.path())
        .arg("chain")
        .arg("--step")
        .arg("edit replace 0..2 4142")
        .arg("--step")
        .arg("write 2 9999")
        .status()
        .expect("failed to run binfiddle chain");

    assert!(status.success());
    let result = std::fs::read(output.path()).unwrap();
    assert_eq!(result, vec![0x41, 0x42, 0x99, 0x99]);
}

#[test]
fn chain_final_text_step_prints_to_stdout() {
    let input = tempfile::NamedTempFile::new().unwrap();
    std::fs::write(input.path(), [0x41, 0x42, 0x43]).unwrap();

    let output = Command::new(binfiddle())
        .arg("--input")
        .arg(input.path())
        .arg("chain")
        .arg("--step")
        .arg("write 0 44")
        .arg("--step")
        .arg("read 0..3")
        .output()
        .expect("failed to run binfiddle chain");

    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("44"),
        "Expected stdout to contain the edited byte 44, got: {}",
        stdout
    );
}

#[test]
fn chain_intermediate_text_step_fails() {
    let input = tempfile::NamedTempFile::new().unwrap();
    std::fs::write(input.path(), [0xDE, 0xAD, 0xBE, 0xEF]).unwrap();

    let output = Command::new(binfiddle())
        .arg("--input")
        .arg(input.path())
        .arg("chain")
        .arg("--step")
        .arg("read 0..2")
        .arg("--step")
        .arg("write 0 00")
        .output()
        .expect("failed to run binfiddle chain");

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("produced no byte output"),
        "Expected intermediate text step to fail, got stderr: {}",
        stderr
    );
}

#[test]
fn chain_from_stdin_to_output_file() {
    let output_file = tempfile::NamedTempFile::new().unwrap();

    let mut child = Command::new(binfiddle())
        .arg("--input")
        .arg("-")
        .arg("--output")
        .arg(output_file.path())
        .arg("chain")
        .arg("--step")
        .arg("write 0 42")
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .expect("failed to spawn binfiddle chain");

    let mut stdin = child.stdin.take().expect("failed to open stdin");
    std::thread::spawn(move || {
        stdin.write_all(&[0x00, 0x00]).unwrap();
    });

    let output = child
        .wait_with_output()
        .expect("failed to wait on binfiddle chain");
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let result = std::fs::read(output_file.path()).unwrap();
    assert_eq!(result, vec![0x42, 0x00]);
}

#[test]
fn chain_empty_steps_rejected() {
    let input = tempfile::NamedTempFile::new().unwrap();
    std::fs::write(input.path(), [0x00]).unwrap();

    let output = Command::new(binfiddle())
        .arg("--input")
        .arg(input.path())
        .arg("chain")
        .output()
        .expect("failed to run binfiddle chain");

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("required arguments were not provided") && stderr.contains("--step"),
        "Expected rejection of empty chain, got stderr: {}",
        stderr
    );
}
