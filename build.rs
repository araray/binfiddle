//! Embed build provenance so a binary can identify its own origin
//! without trusting the current source checkout.

use std::process::Command;

fn main() {
    // Re-run when the HEAD moves so the embedded commit stays truthful at
    // build time (never at run time — the checkout cannot change it later).
    println!("cargo:rerun-if-changed=.git/HEAD");
    println!("cargo:rerun-if-changed=build.rs");

    let commit = Command::new("git")
        .args(["rev-parse", "HEAD"])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "unknown".to_string());

    let dirty = Command::new("git")
        .args(["status", "--porcelain"])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| !o.stdout.is_empty())
        .unwrap_or(false);

    let rustc = std::env::var("RUSTC").unwrap_or_else(|_| "rustc".to_string());
    let rustc_version = Command::new(&rustc)
        .arg("--version")
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .unwrap_or_else(|| "unknown".to_string());

    let target = std::env::var("TARGET").unwrap_or_else(|_| "unknown".to_string());
    let profile = std::env::var("PROFILE").unwrap_or_else(|_| "unknown".to_string());

    println!("cargo:rustc-env=BINFIDDLE_BUILD_COMMIT={commit}");
    println!(
        "cargo:rustc-env=BINFIDDLE_BUILD_DIRTY={}",
        if dirty { "true" } else { "false" }
    );
    println!("cargo:rustc-env=BINFIDDLE_BUILD_RUSTC={rustc_version}");
    println!("cargo:rustc-env=BINFIDDLE_BUILD_TARGET={target}");
    println!("cargo:rustc-env=BINFIDDLE_BUILD_PROFILE={profile}");
}
