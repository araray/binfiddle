//! Legacy CLI regression suite.
//!
//! These tests pin the exact observable behavior of the pre-NN command
//! surface (stdout bytes, exit codes, output-file contents) so that NN-layer
//! changes cannot silently disturb the existing toolkit. Expected values were
//! captured from the baseline binary and cross-checked against independent
//! tools (sha256sum, md5sum, sha1sum, xxh64sum, crc32) where applicable.
//!
//! Note on option placement: the legacy CLI accepts root options only before
//! the subcommand (e.g. `binfiddle -i file --output out write ...`).

use std::fs;
use std::io::Write;
use std::process::{Command, Stdio};

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

fn run_with_stdin(
    dir: &std::path::Path,
    args: &[&str],
    stdin_data: &[u8],
) -> (i32, String, String) {
    let mut child = binfiddle()
        .args(args)
        .current_dir(dir)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("failed to spawn binfiddle");
    child
        .stdin
        .as_mut()
        .expect("stdin")
        .write_all(stdin_data)
        .expect("failed to write stdin");
    let output = child.wait_with_output().expect("wait");
    (
        output.status.code().unwrap_or(-1),
        String::from_utf8_lossy(&output.stdout).into_owned(),
        String::from_utf8_lossy(&output.stderr).into_owned(),
    )
}

fn scratch(tag: &str) -> tempfile::TempDir {
    let dir = tempfile::tempdir().expect("tempdir");
    let _ = tag;
    dir
}

fn write_file(dir: &std::path::Path, name: &str, data: &[u8]) {
    fs::write(dir.join(name), data).expect("write fixture");
}

fn read_file(dir: &std::path::Path, name: &str) -> Vec<u8> {
    fs::read(dir.join(name)).expect("read output")
}

// ---- read ----

#[test]
fn read_hex_range() {
    let dir = scratch("read_hex");
    write_file(dir.path(), "dead.bin", &[0xde, 0xad, 0xbe, 0xef]);
    let (code, out, _) = run_in(dir.path(), &["-i", "dead.bin", "read", "0..4"]);
    assert_eq!(code, 0);
    assert_eq!(out, "de ad be ef\n");
}

#[test]
fn read_formats() {
    let dir = scratch("read_fmt");
    write_file(dir.path(), "ab.bin", b"AB");
    let cases = [
        ("dec", "65 66\n"),
        ("oct", "101 102\n"),
        ("bin", "01000001 01000010\n"),
        ("ascii", "AB\n"),
    ];
    for (format, expected) in cases {
        let (code, out, _) = run_in(
            dir.path(),
            &["-i", "ab.bin", "--format", format, "read", ".."],
        );
        assert_eq!(code, 0, "format {format}");
        assert_eq!(out, expected, "format {format}");
    }
}

#[test]
fn read_raw_outputs_exact_bytes() {
    let dir = scratch("read_raw");
    write_file(
        dir.path(),
        "data.bin",
        &[0x00, 0x01, 0x02, 0xfd, 0xfe, 0xff],
    );
    let output = binfiddle()
        .args(["-i", "data.bin", "--format", "raw", "read", "0..6"])
        .current_dir(dir.path())
        .output()
        .expect("spawn");
    assert!(output.status.success());
    assert_eq!(output.stdout, vec![0x00, 0x01, 0x02, 0xfd, 0xfe, 0xff]);
}

#[test]
fn read_single_index() {
    let dir = scratch("read_one");
    write_file(dir.path(), "four.bin", &[0x00, 0x01, 0x02, 0x03]);
    let (code, out, _) = run_in(dir.path(), &["-i", "four.bin", "read", "2"]);
    assert_eq!(code, 0);
    assert_eq!(out, "02\n");
}

#[test]
fn read_hex_range_offsets() {
    let dir = scratch("read_off");
    write_file(dir.path(), "dead.bin", &[0xde, 0xad, 0xbe, 0xef]);
    let (code, out, _) = run_in(
        dir.path(),
        &["-i", "dead.bin", "--show-offset", "read", "0..4"],
    );
    assert_eq!(code, 0);
    assert_eq!(out, "0x0000: de ad be ef\n");
    let (code, out, _) = run_in(
        dir.path(),
        &["-i", "dead.bin", "--show-ascii", "read", "0..4"],
    );
    assert_eq!(code, 0);
    assert_eq!(
        out,
        "0x0000: de ad be ef                                      |....|\n"
    );
}

#[test]
fn read_from_stdin() {
    let dir = scratch("read_stdin");
    let (code, out, _) = run_with_stdin(dir.path(), &["read", "0..2"], &[0xde, 0xad]);
    assert_eq!(code, 0);
    assert_eq!(out, "de ad\n");
}

#[test]
fn read_out_of_range_is_error() {
    let dir = scratch("read_oob");
    write_file(dir.path(), "four.bin", &[0x00, 0x01, 0x02, 0x03]);
    let (code, _, err) = run_in(dir.path(), &["-i", "four.bin", "read", "0..99"]);
    assert_eq!(code, 1);
    assert!(
        err.contains("End index 99 exceeds data length 4"),
        "stderr: {err}"
    );
}

// ---- write / edit ----

#[test]
fn write_overwrites_at_position() {
    let dir = scratch("write");
    write_file(dir.path(), "zeros8.bin", &[0u8; 8]);
    let (code, out, _) = run_in(
        dir.path(),
        &[
            "-i",
            "zeros8.bin",
            "--output",
            "w1.bin",
            "write",
            "2",
            "DEADBEEF",
        ],
    );
    assert_eq!(code, 0);
    assert_eq!(out, "Previous: 00000000\nNew:     deadbeef\n");
    assert_eq!(
        read_file(dir.path(), "w1.bin"),
        vec![0x00, 0x00, 0xde, 0xad, 0xbe, 0xef, 0x00, 0x00]
    );
}

#[test]
fn write_in_place() {
    let dir = scratch("in_file");
    write_file(dir.path(), "t.bin", &[0x00, 0x01, 0x02, 0x03]);
    let (code, _, _) = run_in(
        dir.path(),
        &["-i", "t.bin", "--in-file", "write", "0", "FF"],
    );
    assert_eq!(code, 0);
    assert_eq!(read_file(dir.path(), "t.bin"), vec![0xff, 0x01, 0x02, 0x03]);
}

#[test]
fn edit_insert_remove_replace() {
    let dir = scratch("edit");
    write_file(dir.path(), "four.bin", &[0x00, 0x01, 0x02, 0x03]);

    let (code, _, _) = run_in(
        dir.path(),
        &[
            "-i", "four.bin", "--output", "i1.bin", "edit", "insert", "1", "AABB",
        ],
    );
    assert_eq!(code, 0);
    assert_eq!(
        read_file(dir.path(), "i1.bin"),
        vec![0x00, 0xaa, 0xbb, 0x01, 0x02, 0x03]
    );

    let (code, _, _) = run_in(
        dir.path(),
        &[
            "-i", "four.bin", "--output", "r1.bin", "edit", "remove", "1..3",
        ],
    );
    assert_eq!(code, 0);
    assert_eq!(read_file(dir.path(), "r1.bin"), vec![0x00, 0x03]);

    let (code, _, _) = run_in(
        dir.path(),
        &[
            "-i", "four.bin", "--output", "p1.bin", "edit", "replace", "0..2", "CAFEBABE",
        ],
    );
    assert_eq!(code, 0);
    assert_eq!(
        read_file(dir.path(), "p1.bin"),
        vec![0xca, 0xfe, 0xba, 0xbe, 0x02, 0x03]
    );
}

// ---- search ----

#[test]
fn search_all_count_and_offsets() {
    let dir = scratch("search");
    write_file(dir.path(), "s.bin", &[0xde, 0xad, 0x00, 0xde, 0xad, 0xde]);
    let (code, out, _) = run_in(
        dir.path(),
        &["-i", "s.bin", "search", "DE AD", "--all", "--count"],
    );
    assert_eq!(code, 0);
    assert_eq!(out, "2\n");
    let (code, out, _) = run_in(
        dir.path(),
        &["-i", "s.bin", "search", "DE AD", "--all", "--offsets-only"],
    );
    assert_eq!(code, 0);
    assert_eq!(out, "0x00000000\n0x00000003\n");
    let (code, out, _) = run_in(dir.path(), &["-i", "s.bin", "search", "DE AD", "--all"]);
    assert_eq!(code, 0);
    assert_eq!(out, "0x00000000: de ad\n0x00000003: de ad\n");
}

#[test]
fn search_ascii_input_format() {
    let dir = scratch("search_ascii");
    write_file(dir.path(), "pw.bin", b"PASSWORD now PASSWORD then");
    let (code, out, _) = run_in(
        dir.path(),
        &[
            "-i",
            "pw.bin",
            "search",
            "PASSWORD",
            "--input-format",
            "ascii",
            "--all",
        ],
    );
    assert_eq!(code, 0);
    assert_eq!(
        out,
        "0x00000000: 50 41 53 53 57 4f 52 44\n0x0000000d: 50 41 53 53 57 4f 52 44\n"
    );
}

// ---- hash ----

#[test]
fn hash_reference_vectors() {
    let dir = scratch("hash");
    write_file(dir.path(), "abc.bin", b"abc");
    write_file(dir.path(), "empty.bin", b"");
    // Digests below were produced by sha256sum, md5sum, sha1sum, xxh64sum,
    // crc32, and the canonical BLAKE3 empty-string vector.
    let cases = [
        ("abc.bin", "md5", "900150983cd24fb0d6963f7d28e17f72"),
        (
            "abc.bin",
            "sha1",
            "a9993e364706816aba3e25717850c26c9cd0d89d",
        ),
        (
            "abc.bin",
            "sha256",
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad",
        ),
        (
            "abc.bin",
            "blake3",
            "6437b3ac38465133ffb63b75273a8db548c558465d79db03fd359c6cd5bd9d85",
        ),
        ("abc.bin", "crc32", "352441c2"),
        ("abc.bin", "xxhash64", "44bc2cf5ad770999"),
        ("empty.bin", "md5", "d41d8cd98f00b204e9800998ecf8427e"),
        (
            "empty.bin",
            "sha1",
            "da39a3ee5e6b4b0d3255bfef95601890afd80709",
        ),
        (
            "empty.bin",
            "sha256",
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855",
        ),
        (
            "empty.bin",
            "blake3",
            "af1349b9f5f9a1a6a0404dea36dcc9499bcb25c9adc112b7cc9a93cae41f3262",
        ),
        ("empty.bin", "crc32", "00000000"),
        ("empty.bin", "xxhash64", "ef46db3751d8e999"),
    ];
    for (file, algorithm, expected) in cases {
        let (code, out, _) = run_in(dir.path(), &["-i", file, "hash", algorithm]);
        assert_eq!(code, 0, "{algorithm} on {file}");
        assert_eq!(out, format!("{expected}\n"), "{algorithm} on {file}");
    }
}

#[test]
fn hash_check_verifies_and_fails() {
    let dir = scratch("check");
    write_file(dir.path(), "abc.bin", b"abc");
    write_file(
        dir.path(),
        "SHA256SUMS",
        b"ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad  abc.bin\n",
    );
    let (code, out, _) = run_in(dir.path(), &["hash", "sha256", "--check", "SHA256SUMS"]);
    assert_eq!(code, 0);
    assert_eq!(out, "abc.bin: OK\n1 passed, 0 failed\n");

    write_file(
        dir.path(),
        "BADSUMS",
        b"0000000000000000000000000000000000000000000000000000000000000000  abc.bin\n",
    );
    let (code, out, _) = run_in(dir.path(), &["hash", "sha256", "--check", "BADSUMS"]);
    assert_eq!(code, 1);
    assert_eq!(out, "abc.bin: FAILED\n0 passed, 1 failed\n");
}

// ---- analyze ----

#[test]
fn analyze_entropy_constant_data_is_zero() {
    let dir = scratch("entropy");
    write_file(dir.path(), "c16.bin", &[b'A'; 16]);
    let (code, out, _) = run_in(dir.path(), &["-i", "c16.bin", "analyze", "entropy"]);
    assert_eq!(code, 0);
    assert_eq!(
        out,
        "=== Entropy Analysis ===\nSize: 16 bytes\nEntropy: 0.0000 bits/byte\nInterpretation: highly repetitive/uniform\n\n"
    );
}

#[test]
fn analyze_index_of_coincidence_constant_is_one() {
    let dir = scratch("ic");
    write_file(dir.path(), "c16.bin", &[b'A'; 16]);
    let (code, out, _) = run_in(dir.path(), &["-i", "c16.bin", "analyze", "ic"]);
    assert_eq!(code, 0);
    assert!(out.contains("IC: 1.000000"), "output: {out}");
}

// ---- diff / patch ----

#[test]
fn diff_simple_output() {
    let dir = scratch("diff");
    write_file(dir.path(), "orig.bin", &[0x01, 0x02, 0x03, 0x04]);
    write_file(dir.path(), "mod.bin", &[0x01, 0xff, 0x03, 0x04]);
    let (code, out, _) = run_in(dir.path(), &["diff", "orig.bin", "mod.bin"]);
    assert_eq!(code, 0);
    assert_eq!(
        out,
        "--- orig.bin\n+++ mod.bin\n@@ -0x0,0x4 +0x0,0x4 @@\n-0x00000000: 01 02 03 04  |....|\n+0x00000000: 01 ff 03 04  |....|\n"
    );
}

#[test]
fn diff_patch_roundtrip_reconstructs_modified_file() {
    let dir = scratch("patch");
    write_file(dir.path(), "orig.bin", &[0x01, 0x02, 0x03, 0x04]);
    write_file(dir.path(), "mod.bin", &[0x01, 0xff, 0x03, 0x04]);
    let (code, patch_text, _) = run_in(
        dir.path(),
        &["diff", "orig.bin", "mod.bin", "--diff-format", "patch"],
    );
    assert_eq!(code, 0);
    assert_eq!(
        patch_text,
        "# binfiddle patch file\n# source: orig.bin\n# target: mod.bin\n# format: OFFSET:OLD_HEX:NEW_HEX\n# differences: 1\n#\n0x00000001:02:ff\n"
    );
    write_file(dir.path(), "ch.patch", patch_text.as_bytes());
    let (code, _, _) = run_in(
        dir.path(),
        &["--output", "rec.bin", "patch", "orig.bin", "ch.patch"],
    );
    assert_eq!(code, 0);
    assert_eq!(
        read_file(dir.path(), "rec.bin"),
        read_file(dir.path(), "mod.bin")
    );
}

// ---- convert ----

#[test]
fn convert_newlines_crlf_to_lf() {
    let dir = scratch("convert_nl");
    write_file(dir.path(), "crlf.bin", b"a\r\nb\r\n");
    let (code, _, _) = run_in(
        dir.path(),
        &[
            "-i",
            "crlf.bin",
            "--output",
            "lf.bin",
            "convert",
            "--newlines",
            "unix",
        ],
    );
    assert_eq!(code, 0);
    assert_eq!(read_file(dir.path(), "lf.bin"), b"a\nb\n");
}

#[test]
fn convert_bom_add_remove_roundtrip() {
    let dir = scratch("convert_bom");
    write_file(dir.path(), "plain.bin", b"hi");
    let (code, _, _) = run_in(
        dir.path(),
        &[
            "-i",
            "plain.bin",
            "--output",
            "bom.bin",
            "convert",
            "--bom",
            "add",
        ],
    );
    assert_eq!(code, 0);
    assert_eq!(read_file(dir.path(), "bom.bin"), b"\xef\xbb\xbfhi");
    let (code, _, _) = run_in(
        dir.path(),
        &[
            "-i", "bom.bin", "--output", "nb.bin", "convert", "--bom", "remove",
        ],
    );
    assert_eq!(code, 0);
    assert_eq!(read_file(dir.path(), "nb.bin"), b"hi");
}

// ---- chain ----

#[test]
fn chain_write_then_read() {
    let dir = scratch("chain");
    let (code, out, _) = run_with_stdin(
        dir.path(),
        &["chain", "--step", "write 0 ff", "--step", "read 0..4"],
        &[0x00, 0x11, 0x22, 0x33],
    );
    assert_eq!(code, 0);
    assert_eq!(out, "ff 11 22 33\n");
}

// ---- general ----

#[test]
fn version_reports_package_version() {
    let (code, out, _) = run_in(std::path::Path::new("/"), &["--version"]);
    assert_eq!(code, 0);
    assert_eq!(
        out.trim_end(),
        format!("binfiddle {}", env!("CARGO_PKG_VERSION"))
    );
}

#[test]
fn unknown_subcommand_exits_two() {
    let (code, _, _) = run_in(std::path::Path::new("/"), &["nope"]);
    assert_eq!(code, 2);
}

#[test]
fn missing_subcommand_is_error() {
    let (code, _, _) = run_in(std::path::Path::new("/"), &[]);
    assert_eq!(code, 1);
}

// ---- nn command surface ----

#[test]
fn nn_capabilities_text_report() {
    let (code, out, _) = run_in(std::path::Path::new("/"), &["nn", "capabilities"]);
    assert_eq!(code, 0);
    assert!(
        out.starts_with("binfiddle nn workbench capabilities"),
        "out: {out}"
    );
    assert!(out.contains("implemented:"), "out: {out}");
    assert!(out.contains("unavailable:"), "out: {out}");
    assert!(out.contains("nn.wire"), "out: {out}");
    assert!(out.contains("nn.discover"), "out: {out}");
}

#[test]
fn nn_capabilities_json_report_is_canonical_envelope() {
    let (code, out, _) = run_in(
        std::path::Path::new("/"),
        &["nn", "capabilities", "--report-format", "json"],
    );
    assert_eq!(code, 0);
    assert!(
        out.starts_with("{\"coverage\":{\"complete\":true,\"notes\":[]},\"diagnostics\":[],"),
        "out: {out}"
    );
    assert!(out.contains("\"schema\":\"binfiddle.nn.result/v1\""));
    assert!(out.contains("\"operation\":\"capabilities\""));
    assert!(out.contains("\"status\":\"complete\""));
    assert!(out.contains("\"name\":\"nn.wire\""));
    assert!(out.ends_with("}\n"));
}

#[test]
fn nn_capabilities_rejects_unknown_report_format() {
    let (code, _, _) = run_in(
        std::path::Path::new("/"),
        &["nn", "capabilities", "--report-format", "yaml"],
    );
    assert_eq!(code, 2);
}

#[test]
fn nn_rejects_process_memory_options() {
    let (code, _, err) = run_in(
        std::path::Path::new("/"),
        &["--pid", "1", "nn", "capabilities"],
    );
    assert_eq!(code, 2);
    assert!(err.contains("cannot be used with nn"), "stderr: {err}");
}
