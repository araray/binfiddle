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

#[test]
fn hash_sha1_of_known_string() {
    let input = tempfile::NamedTempFile::new().unwrap();
    std::fs::write(input.path(), b"hello").unwrap();

    let output = Command::new(binfiddle())
        .arg("--input")
        .arg(input.path())
        .arg("hash")
        .arg("sha1")
        .output()
        .expect("failed to run binfiddle hash");

    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
    assert_eq!(stdout, "aaf4c61ddcc5e8a2dabede0f3b482cd9aea9434d");
}

#[test]
fn hash_xxhash64_of_known_string() {
    let input = tempfile::NamedTempFile::new().unwrap();
    std::fs::write(input.path(), b"hello").unwrap();

    let output = Command::new(binfiddle())
        .arg("--input")
        .arg(input.path())
        .arg("hash")
        .arg("xxhash64")
        .output()
        .expect("failed to run binfiddle hash");

    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
    assert_eq!(stdout, "26c7827d889f6da3");
}

#[test]
fn hash_base64_output() {
    let input = tempfile::NamedTempFile::new().unwrap();
    std::fs::write(input.path(), b"hello").unwrap();

    let output = Command::new(binfiddle())
        .arg("--input")
        .arg(input.path())
        .arg("hash")
        .arg("md5")
        .arg("--output-format")
        .arg("base64")
        .output()
        .expect("failed to run binfiddle hash");

    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
    assert_eq!(stdout, "XUFAKrxLKna5cZ2REBfFkg==");
}

#[test]
fn hash_stream_matches_non_stream() {
    let input = tempfile::NamedTempFile::new().unwrap();
    std::fs::write(input.path(), b"hello world this is a stream test").unwrap();

    let non_stream = Command::new(binfiddle())
        .arg("--input")
        .arg(input.path())
        .arg("hash")
        .arg("sha256")
        .output()
        .expect("failed to run binfiddle hash");

    let stream = Command::new(binfiddle())
        .arg("--input")
        .arg(input.path())
        .arg("hash")
        .arg("sha256")
        .arg("--stream")
        .arg("--read-block-size")
        .arg("8")
        .output()
        .expect("failed to run binfiddle hash stream");

    assert!(
        non_stream.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&non_stream.stderr)
    );
    assert!(
        stream.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&stream.stderr)
    );

    assert_eq!(
        String::from_utf8_lossy(&non_stream.stdout).trim(),
        String::from_utf8_lossy(&stream.stdout).trim()
    );
}

#[test]
fn hash_stream_block_hashing() {
    let input = tempfile::NamedTempFile::new().unwrap();
    std::fs::write(input.path(), b"123456789").unwrap();

    let output = Command::new(binfiddle())
        .arg("--input")
        .arg(input.path())
        .arg("hash")
        .arg("crc32")
        .arg("--block-size")
        .arg("3")
        .arg("--stream")
        .arg("--read-block-size")
        .arg("5")
        .output()
        .expect("failed to run binfiddle hash stream");

    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = String::from_utf8_lossy(&output.stdout);
    let lines: Vec<&str> = stdout.lines().collect();
    assert_eq!(lines.len(), 3, "Expected 3 block hashes, got: {}", stdout);
    assert!(lines[0].starts_with("0x00000000:"));
}

#[test]
fn hash_check_from_file() {
    let dir = tempfile::tempdir().unwrap();
    let file_a = dir.path().join("a.bin");
    let file_b = dir.path().join("b.bin");
    std::fs::write(&file_a, b"hello").unwrap();
    std::fs::write(&file_b, b"world").unwrap();

    // Compute known digest for a.bin.
    let good = Command::new(binfiddle())
        .arg("--input")
        .arg(&file_a)
        .arg("hash")
        .arg("sha256")
        .output()
        .unwrap()
        .stdout;
    let good = String::from_utf8_lossy(&good).trim().to_string();

    let checksum_file = dir.path().join("checksums.sha256");
    std::fs::write(
        &checksum_file,
        format!("{}  a.bin\n{}  b.bin\n", good, "0".repeat(64)),
    )
    .unwrap();

    let output = Command::new(binfiddle())
        .arg("hash")
        .arg("sha256")
        .arg("--check")
        .arg(&checksum_file)
        .output()
        .expect("failed to run binfiddle hash --check");

    assert!(
        !output.status.success(),
        "Expected checksum verification to fail"
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("a.bin: OK"), "Got: {}", stdout);
    assert!(stdout.contains("b.bin: FAILED"), "Got: {}", stdout);
    assert!(stdout.contains("1 passed, 1 failed"), "Got: {}", stdout);
}

#[test]
fn hash_check_all_pass() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("data.bin");
    std::fs::write(&file, b"check me").unwrap();

    let digest = Command::new(binfiddle())
        .arg("--input")
        .arg(&file)
        .arg("hash")
        .arg("md5")
        .output()
        .unwrap()
        .stdout;
    let digest = String::from_utf8_lossy(&digest).trim().to_string();

    let checksum_file = dir.path().join("checksums.md5");
    std::fs::write(&checksum_file, format!("{}  data.bin\n", digest)).unwrap();

    let output = Command::new(binfiddle())
        .arg("hash")
        .arg("md5")
        .arg("--check")
        .arg(&checksum_file)
        .output()
        .expect("failed to run binfiddle hash --check");

    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("data.bin: OK"), "Got: {}", stdout);
    assert!(stdout.contains("1 passed, 0 failed"), "Got: {}", stdout);
}
