//! Integration tests for `nn ls` / `nn show` / `nn select` and catalog
//! persistence: real binary, catalog round-trips, ambiguity handling, and
//! stale-selection rejection (library-level bind, since consuming saved
//! selections arrives with extraction).

use binfiddle::nn::budget::{Budget, BudgetCaps};
use binfiddle::nn::cancel::CancellationToken;
use binfiddle::nn::catalog::Catalog;
use binfiddle::nn::discover::{discover, DiscoverOptions};
use binfiddle::nn::selection::{EmptyPolicy, Selection, SelectionRequest};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

fn binfiddle() -> Command {
    Command::new(env!("CARGO_BIN_EXE_binfiddle"))
}

fn run_in(dir: &Path, args: &[&str]) -> (i32, String, String) {
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

fn write_safetensors(path: &Path, tensors: &[(&str, &str, &[u64], &[u8])]) {
    let mut body: Vec<u8> = Vec::new();
    let mut spans = Vec::new();
    for (name, dtype, shape, payload) in tensors {
        let begin = body.len();
        body.extend_from_slice(payload);
        spans.push((name, dtype, shape, begin, begin + payload.len()));
    }
    let entries: Vec<String> = spans
        .iter()
        .map(|(n, dt, sh, b, e)| {
            let dims: Vec<String> = sh.iter().map(|d| d.to_string()).collect();
            format!(
                "\"{}\":{{\"dtype\":\"{}\",\"shape\":[{}],\"data_offsets\":[{},{}]}}",
                n,
                dt,
                dims.join(","),
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
    fs::write(path, data).unwrap();
}

fn lib_catalog(path: &Path) -> Catalog {
    let budget = Budget::new(BudgetCaps::default(), None, CancellationToken::new());
    let report = discover(path, &DiscoverOptions::default(), &budget).unwrap();
    Catalog::from_discovery(&report).unwrap()
}

#[test]
fn catalog_persists_and_lists_through_the_cli() {
    let dir = tempfile::tempdir().unwrap();
    write_safetensors(
        &dir.path().join("model.safetensors"),
        &[
            ("model.alpha", "F32", &[2, 3], &[0u8; 24]),
            ("model.beta", "U8", &[64], &[7u8; 64]),
        ],
    );
    let (code, _, err) = run_in(
        dir.path(),
        &[
            "-i",
            "model.safetensors",
            "nn",
            "discover",
            "--out-catalog",
            "model.nn.json",
        ],
    );
    assert_eq!(code, 0, "stderr: {err}");

    // Tensor ids in the persisted catalog match what ls displays. (The
    // catalog file is the authority here: fresh discovery through a different
    // path spelling yields a different observation-scoped identity.)
    let persisted = Catalog::load(&dir.path().join("model.nn.json")).unwrap();
    let (code, out, _) = run_in(dir.path(), &["nn", "ls", "--catalog", "model.nn.json"]);
    assert_eq!(code, 0);
    for tensor in &persisted.tensors {
        let prefix = &tensor.id[..24.min(tensor.id.len())];
        assert!(out.contains(prefix), "missing id {prefix} in ls output");
    }

    // Filters and sorting.
    let (code, out, _) = run_in(
        dir.path(),
        &[
            "nn",
            "ls",
            "--catalog",
            "model.nn.json",
            "--encoding",
            "safetensors.F32",
        ],
    );
    assert_eq!(code, 0);
    assert!(out.contains("model.alpha"));
    assert!(!out.contains("model.beta"));
    assert!(out.contains("1 of 1 tensors"));

    let (code, out, _) = run_in(
        dir.path(),
        &["nn", "ls", "--catalog", "model.nn.json", "--sort", "bytes"],
    );
    assert_eq!(code, 0);
    let alpha = out.find("model.alpha").unwrap();
    let beta = out.find("model.beta").unwrap();
    assert!(
        beta < alpha,
        "larger tensor must sort first under --sort bytes"
    );

    // Pagination with honest totals.
    let (code, out, _) = run_in(
        dir.path(),
        &[
            "nn",
            "ls",
            "--catalog",
            "model.nn.json",
            "--limit",
            "1",
            "--offset",
            "1",
        ],
    );
    assert_eq!(code, 0);
    assert!(out.contains("1 of 2 tensors (offset 1, limit 1)"));

    // Sources view.
    let (code, out, _) = run_in(
        dir.path(),
        &[
            "nn",
            "ls",
            "--catalog",
            "model.nn.json",
            "--view",
            "sources",
        ],
    );
    assert_eq!(code, 0);
    assert!(out.contains("model.safetensors"));
    assert!(out.contains("safetensors 1"));

    // JSON envelope shape.
    let (code, out, _) = run_in(
        dir.path(),
        &[
            "nn",
            "ls",
            "--catalog",
            "model.nn.json",
            "--report-format",
            "json",
        ],
    );
    assert_eq!(code, 0);
    assert!(out.contains("\"operation\":\"ls\""));
    assert!(out.contains("\"total_matches\":\"2\""));
}

#[test]
fn show_rejects_ambiguity_and_missing_names() {
    let dir = tempfile::tempdir().unwrap();
    write_safetensors(
        &dir.path().join("a.safetensors"),
        &[("shared.w", "U8", &[1], &[1])],
    );
    write_safetensors(
        &dir.path().join("b.safetensors"),
        &[("shared.w", "U8", &[1], &[2])],
    );
    let (code, _, _) = run_in(
        dir.path(),
        &["-i", ".", "nn", "discover", "--out-catalog", "c.nn.json"],
    );
    assert_eq!(code, 0);

    let (code, _, err) = run_in(
        dir.path(),
        &[
            "nn",
            "show",
            "--catalog",
            "c.nn.json",
            "--tensor",
            "shared.w",
        ],
    );
    assert_eq!(code, 3);
    assert!(err.contains("ambiguous binding"), "stderr: {err}");

    let (code, out, _) = run_in(
        dir.path(),
        &[
            "nn",
            "show",
            "--catalog",
            "c.nn.json",
            "--tensor",
            "shared.w",
            "--source",
            "a.safetensors",
            "--explain",
        ],
    );
    assert_eq!(code, 0);
    assert!(out.contains("tensor shared.w"));
    assert!(out.contains("a.safetensors"));

    let (code, _, err) = run_in(
        dir.path(),
        &["nn", "show", "--catalog", "c.nn.json", "--tensor", "absent"],
    );
    assert_eq!(code, 5);
    assert!(err.contains("source missing"), "stderr: {err}");
}

#[test]
fn select_saves_and_rejects_empty_by_default() {
    let dir = tempfile::tempdir().unwrap();
    write_safetensors(
        &dir.path().join("m.safetensors"),
        &[("w", "F32", &[2, 2], &[0u8; 16])],
    );
    let (code, out, _) = run_in(
        dir.path(),
        &[
            "-i",
            "m.safetensors",
            "nn",
            "select",
            "--tensor",
            "w",
            "--out-selection",
            "w.selection.json",
        ],
    );
    assert_eq!(code, 0);
    assert!(out.contains("1 tensors"));
    assert!(Path::new(&dir.path().join("w.selection.json")).exists());

    let (code, _, err) = run_in(
        dir.path(),
        &["-i", "m.safetensors", "nn", "select", "--tensor", "absent"],
    );
    assert_eq!(code, 2);
    assert!(err.contains("matched no tensors"), "stderr: {err}");

    let (code, out, _) = run_in(
        dir.path(),
        &[
            "-i",
            "m.safetensors",
            "nn",
            "select",
            "--tensor",
            "absent",
            "--allow-empty",
            "--report-format",
            "json",
        ],
    );
    assert_eq!(code, 0);
    assert!(out.contains("\"target_count\":\"0\""));
}

#[test]
fn component_selector_parses_then_explains_pack_requirement() {
    let dir = tempfile::tempdir().unwrap();
    write_safetensors(
        &dir.path().join("m.safetensors"),
        &[("w", "F32", &[2, 2], &[0u8; 16])],
    );
    let (code, _, err) = run_in(
        dir.path(),
        &[
            "-i",
            "m.safetensors",
            "nn",
            "select",
            "--select",
            "decoder.layers[8:16].mlp",
        ],
    );
    assert_eq!(code, 3);
    assert!(err.contains("parsed successfully"), "stderr: {err}");
    assert!(err.contains("model pack"), "stderr: {err}");

    // Grammar violations are usage errors before any resolution.
    let (code, _, err) = run_in(
        dir.path(),
        &[
            "-i",
            "m.safetensors",
            "nn",
            "select",
            "--select",
            "layers[-1]",
        ],
    );
    assert_eq!(code, 4);
    assert!(err.contains("selector parse error"), "stderr: {err}");
}

#[test]
fn saved_selection_never_silently_rematches_a_different_catalog() {
    let dir = tempfile::tempdir().unwrap();
    write_safetensors(
        &dir.path().join("v1.safetensors"),
        &[("w", "F32", &[2, 2], &[0u8; 16])],
    );
    write_safetensors(
        &dir.path().join("v2.safetensors"),
        &[("w", "F32", &[2, 2], &[1u8; 16])],
    );
    let catalog_v1 = lib_catalog(&dir.path().join("v1.safetensors"));
    let catalog_v2 = lib_catalog(&dir.path().join("v2.safetensors"));
    assert_ne!(catalog_v1.id().unwrap(), catalog_v2.id().unwrap());

    let selection = Selection::resolve(
        &catalog_v1,
        SelectionRequest::TensorName {
            name: "w".to_string(),
            source: None,
        },
        EmptyPolicy::Reject,
    )
    .unwrap();
    let path: PathBuf = dir.path().join("w.selection.json");
    selection.save(&path).unwrap();

    // Same catalog: binds exactly.
    let loaded = Selection::load(&path).unwrap();
    let bound = loaded.bind(&catalog_v1).unwrap();
    assert_eq!(bound.len(), 1);
    assert_eq!(bound[0].original_name, "w");

    // Different catalog (same names!): refused as stale.
    let err = loaded.bind(&catalog_v2).unwrap_err();
    assert_eq!(err.code().as_str(), "SOURCE_CHANGED");
    assert!(err.to_string().contains("never silently rematch"));
}

#[test]
fn conflicting_source_routes_are_usage_errors() {
    let dir = tempfile::tempdir().unwrap();
    write_safetensors(
        &dir.path().join("m.safetensors"),
        &[("w", "F32", &[2, 2], &[0u8; 16])],
    );
    run_in(
        dir.path(),
        &[
            "-i",
            "m.safetensors",
            "nn",
            "discover",
            "--out-catalog",
            "c.nn.json",
        ],
    );
    let (code, _, err) = run_in(
        dir.path(),
        &["-i", "m.safetensors", "nn", "ls", "--catalog", "c.nn.json"],
    );
    assert_eq!(code, 2);
    assert!(err.contains("exactly one source route"), "stderr: {err}");

    let (code, _, err) = run_in(dir.path(), &["nn", "ls"]);
    assert_eq!(code, 2);
    assert!(err.contains("a source is required"), "stderr: {err}");
}

#[test]
fn tampered_catalog_file_fails_to_load_through_cli() {
    let dir = tempfile::tempdir().unwrap();
    write_safetensors(
        &dir.path().join("m.safetensors"),
        &[("w", "F32", &[2, 2], &[0u8; 16])],
    );
    run_in(
        dir.path(),
        &[
            "-i",
            "m.safetensors",
            "nn",
            "discover",
            "--out-catalog",
            "c.nn.json",
        ],
    );
    let text = fs::read_to_string(dir.path().join("c.nn.json")).unwrap();
    fs::write(
        dir.path().join("c.nn.json"),
        text.replace("\"w\"", "\"hacked\""),
    )
    .unwrap();
    let (code, _, err) = run_in(dir.path(), &["nn", "ls", "--catalog", "c.nn.json"]);
    assert_eq!(code, 4);
    assert!(err.contains("mismatch"), "stderr: {err}");
}
