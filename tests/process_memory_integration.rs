//! Integration tests for process memory access (`--process-self` and `--pid`).

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
        stderr.contains("--address") || stderr.contains("--size"),
        "Expected missing --address/--size error, got: {}",
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
        stderr.contains("Process memory error") || stderr.contains("Failed to read process"),
        "Expected process memory error, got: {}",
        stderr
    );
}

#[test]
fn process_self_lists_regions() {
    let output = Command::new(binfiddle())
        .args(["--process-self", "--list-regions"])
        .output()
        .expect("failed to run binfiddle");

    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("Memory regions"),
        "Expected region listing header, got: {}",
        stdout
    );
    assert!(
        stdout.contains("r-xp") || stdout.contains("rw-p") || stdout.contains("r--p"),
        "Expected memory permissions in listing, got: {}",
        stdout
    );
}
