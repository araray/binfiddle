//! Integration tests for the experimental `--process-self` feature.

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
fn process_self_requires_address_and_size() {
    let output = Command::new(binfiddle())
        .args(["--process-self", "read", "0..1"])
        .output()
        .expect("failed to run binfiddle");

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("--address") && stderr.contains("--size"),
        "Expected --address and --size to be required, got: {}",
        stderr
    );
}

#[test]
fn process_self_invalid_address_fails_gracefully() {
    let output = Command::new(binfiddle())
        .args([
            "--process-self",
            "--address",
            "0x1",
            "--size",
            "1",
            "read",
            "0..1",
        ])
        .output()
        .expect("failed to run binfiddle");

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("Process memory error") || stderr.contains("Failed to read process memory"),
        "Expected process memory error, got: {}",
        stderr
    );
}
