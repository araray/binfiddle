//! Integration tests for process memory access (`--process-self` and `--pid`).

use std::io::{BufRead, BufReader, Write};
use std::process::{Command, Stdio};

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
        stderr.contains("ProcessMemoryError")
            || stderr.contains("Process memory error")
            || stderr.contains("Failed to read process memory"),
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

#[test]
fn force_writable_requires_allow_write() {
    let output = Command::new(binfiddle())
        .args([
            "--process-self",
            "--address",
            "0x1000",
            "--size",
            "1",
            "--force-writable",
            "write",
            "0",
            "FF",
        ])
        .output()
        .expect("failed to run binfiddle");

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("--allow-write") || stderr.contains("allow_write"),
        "Expected --force-writable to require --allow-write, got: {}",
        stderr
    );
}

const FORCE_WRITE_HELPER_SRC: &str = r#"
use std::io::{self, Read, Write};

static TARGET: [u8; 8] = *b"CHANGEME";

fn main() {
    println!("{} {}", std::process::id(), &TARGET as *const _ as usize);
    io::stdout().flush().unwrap();
    // Wait for the parent to signal us to exit.
    let mut buf = [0u8; 1];
    let _ = io::stdin().read_exact(&mut buf);
}
"#;

fn compile_force_write_helper() -> (std::path::PathBuf, tempfile::TempDir) {
    let dir = tempfile::tempdir().expect("failed to create temp dir");
    let src = dir.path().join("helper.rs");
    let bin = dir.path().join("helper");
    std::fs::write(&src, FORCE_WRITE_HELPER_SRC).expect("failed to write helper source");

    let rustc = std::env::var_os("RUSTC").unwrap_or_else(|| "rustc".into());
    let status = Command::new(rustc)
        .args(["--edition", "2021", "-C", "opt-level=0", "-o"])
        .arg(&bin)
        .arg(&src)
        .status()
        .expect("failed to run rustc");
    assert!(
        status.success(),
        "rustc failed to compile force-write helper"
    );

    (bin, dir)
}

fn ptrace_scope_permits_parent_child() -> bool {
    match std::fs::read_to_string("/proc/sys/kernel/yama/ptrace_scope") {
        Ok(content) => content
            .trim()
            .parse::<i32>()
            .map(|value| value <= 1)
            .unwrap_or(true),
        Err(_) => true,
    }
}

#[test]
#[ignore = "requires ptrace access (run with --ignored --test-threads=1)"]
fn cross_process_force_writable_modifies_readonly_page() {
    if !ptrace_scope_permits_parent_child() {
        return;
    }

    let (helper_path, _helper_dir) = compile_force_write_helper();
    let mut child = Command::new(&helper_path)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .expect("failed to spawn force-write helper");

    let stdout = child.stdout.take().expect("helper stdout missing");
    let mut reader = BufReader::new(stdout);
    let mut line = String::new();
    reader
        .read_line(&mut line)
        .expect("failed to read helper announcement");

    let mut parts = line.split_whitespace();
    let pid: u32 = parts
        .next()
        .expect("missing pid")
        .parse()
        .expect("invalid pid");
    let address = parts.next().expect("missing address");

    // Temporarily make the helper's read-only static writable and overwrite it.
    let write_output = Command::new(binfiddle())
        .args([
            "--pid",
            &pid.to_string(),
            "--address",
            address,
            "--size",
            "8",
            "--allow-write",
            "--force-writable",
            "write",
            "0",
            "464f524345575254",
        ])
        .output()
        .expect("failed to run binfiddle write");
    if !write_output.status.success() {
        let stderr = String::from_utf8_lossy(&write_output.stderr);
        if stderr.contains("Operation not permitted") || stderr.contains("Permission denied") {
            // The environment (e.g. seccomp, capabilities) blocks cross-process
            // vm access even though Yama allows it. Skip the rest of the test.
            let _ = child.kill();
            let _ = child.wait();
            return;
        }
        panic!("write failed: {}", stderr);
    }

    // Read back the modified bytes as raw output.
    let read_output = Command::new(binfiddle())
        .args([
            "--pid",
            &pid.to_string(),
            "--address",
            address,
            "--size",
            "8",
            "read",
            "..",
            "--format",
            "raw",
        ])
        .output()
        .expect("failed to run binfiddle read");
    if !read_output.status.success() {
        let stderr = String::from_utf8_lossy(&read_output.stderr);
        if stderr.contains("Operation not permitted") || stderr.contains("Permission denied") {
            let _ = child.kill();
            let _ = child.wait();
            return;
        }
        panic!("read failed: {}", stderr);
    }
    assert_eq!(read_output.stdout, b"FORCEWRT");

    // Signal the helper to exit.
    let mut stdin = child.stdin.take().expect("helper stdin missing");
    stdin.write_all(b"\n").expect("failed to write to helper");
    let status = child.wait().expect("helper did not exit");
    assert!(status.success(), "helper exited with non-zero status");
}

#[test]
fn zero_fill_inaccessible_fills_unmapped_self_address() {
    // Reading address 0 without --zero-fill-inaccessible fails because it is
    // not mapped; with the flag it is replaced by a zero byte.
    let output = Command::new(binfiddle())
        .args([
            "--process-self",
            "--address",
            "0",
            "--size",
            "1",
            "--zero-fill-inaccessible",
            "--format",
            "hex",
            "read",
            "0..1",
        ])
        .output()
        .expect("failed to run binfiddle");

    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("00"),
        "Expected zero-filled byte, got: {}",
        stdout
    );
}

#[test]
fn zero_fill_inaccessible_requires_process_source() {
    let output = Command::new(binfiddle())
        .args([
            "-i",
            "/dev/null",
            "--zero-fill-inaccessible",
            "read",
            "0..1",
        ])
        .output()
        .expect("failed to run binfiddle");

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("--process-self") || stderr.contains("--pid"),
        "Expected process-source requirement error, got: {}",
        stderr
    );
}

#[test]
fn skip_inaccessible_requires_read_command() {
    let output = Command::new(binfiddle())
        .args([
            "--process-self",
            "--address",
            "0x1000",
            "--size",
            "1",
            "--skip-inaccessible",
            "search",
            "00",
        ])
        .output()
        .expect("failed to run binfiddle");

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("read command") || stderr.contains("only supported with the read command"),
        "Expected read-command restriction error, got: {}",
        stderr
    );
}

#[test]
#[ignore = "requires ptrace access (run with --ignored --test-threads=1)"]
fn cross_process_search_finds_readonly_static() {
    if !ptrace_scope_permits_parent_child() {
        return;
    }

    let (helper_path, _helper_dir) = compile_force_write_helper();
    let mut child = Command::new(&helper_path)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .expect("failed to spawn force-write helper");

    let stdout = child.stdout.take().expect("helper stdout missing");
    let mut reader = BufReader::new(stdout);
    let mut line = String::new();
    reader
        .read_line(&mut line)
        .expect("failed to read helper announcement");

    let mut parts = line.split_whitespace();
    let pid: u32 = parts
        .next()
        .expect("missing pid")
        .parse()
        .expect("invalid pid");
    let address = parts.next().expect("missing address");

    let search_output = Command::new(binfiddle())
        .args([
            "--pid",
            &pid.to_string(),
            "--address",
            address,
            "--size",
            "8",
            "search",
            "CHANGEME",
            "--input-format",
            "ascii",
            "--all",
            "--offsets-only",
        ])
        .output()
        .expect("failed to run binfiddle search");
    if !search_output.status.success() {
        let stderr = String::from_utf8_lossy(&search_output.stderr);
        if stderr.contains("Operation not permitted") || stderr.contains("Permission denied") {
            let _ = child.kill();
            let _ = child.wait();
            return;
        }
        panic!("search failed: {}", stderr);
    }

    let stdout = String::from_utf8_lossy(&search_output.stdout);
    assert!(
        stdout.contains("0x"),
        "Expected search to report an offset, got: {}",
        stdout
    );

    let mut stdin = child.stdin.take().expect("helper stdin missing");
    stdin.write_all(b"\n").expect("failed to write to helper");
    let status = child.wait().expect("helper did not exit");
    assert!(status.success(), "helper exited with non-zero status");
}
