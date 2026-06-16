//! Integration tests for the `hash` command.

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
fn hash_sha256_of_known_string() {
    let input = tempfile::NamedTempFile::new().unwrap();
    std::fs::write(input.path(), b"hello").unwrap();

    let output = Command::new(binfiddle())
        .arg("--input")
        .arg(input.path())
        .arg("hash")
        .arg("sha256")
        .output()
        .expect("failed to run binfiddle hash");

    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
    assert_eq!(
        stdout,
        "2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824"
    );
}

#[test]
fn hash_md5_of_empty_file() {
    let input = tempfile::NamedTempFile::new().unwrap();

    let output = Command::new(binfiddle())
        .arg("--input")
        .arg(input.path())
        .arg("hash")
        .arg("md5")
        .output()
        .expect("failed to run binfiddle hash");

    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
    assert_eq!(stdout, "d41d8cd98f00b204e9800998ecf8427e");
}

#[test]
fn hash_blake3_of_known_string() {
    let input = tempfile::NamedTempFile::new().unwrap();
    std::fs::write(input.path(), b"hello").unwrap();

    let output = Command::new(binfiddle())
        .arg("--input")
        .arg(input.path())
        .arg("hash")
        .arg("blake3")
        .output()
        .expect("failed to run binfiddle hash");

    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
    assert_eq!(
        stdout,
        "ea8f163db38682925e4491c5e58d4bb3506ef8c14eb78a86e908c5624a67200f"
    );
}

#[test]
fn hash_crc32_of_known_string() {
    let input = tempfile::NamedTempFile::new().unwrap();
    std::fs::write(input.path(), b"123456789").unwrap();

    let output = Command::new(binfiddle())
        .arg("--input")
        .arg(input.path())
        .arg("hash")
        .arg("crc32")
        .output()
        .expect("failed to run binfiddle hash");

    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
    assert_eq!(stdout, "cbf43926");
}

#[test]
fn hash_block_based_crc32() {
    let input = tempfile::NamedTempFile::new().unwrap();
    std::fs::write(input.path(), b"123456789").unwrap();

    let output = Command::new(binfiddle())
        .arg("--input")
        .arg(input.path())
        .arg("hash")
        .arg("crc32")
        .arg("--block-size")
        .arg("3")
        .output()
        .expect("failed to run binfiddle hash");

    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = String::from_utf8_lossy(&output.stdout);
    let lines: Vec<&str> = stdout.lines().collect();
    assert_eq!(lines.len(), 3, "Expected 3 block hashes, got: {}", stdout);
    assert!(lines[0].starts_with("0x00000000:"));
    assert!(lines[1].starts_with("0x00000003:"));
    assert!(lines[2].starts_with("0x00000006:"));
}
