//! Command chaining for binfiddle.
//!
//! Provides a `chain` command that executes multiple binfiddle subcommands
//! sequentially, passing the output of each step as the input to the next.
//! This avoids shell pipes and escaping issues while reusing the existing CLI.
//!
//! Internally, v1 uses subprocesses connected by temporary files. Each step
//! is a normal binfiddle invocation. This keeps the implementation simple and
//! ensures every command behaves exactly as it does when run standalone.

use crate::error::{BinfiddleError, Result};
use std::io::{self, Read, Write};
use std::process::Command;

/// Executes a chain of binfiddle commands.
pub struct ChainExecutor;

impl ChainExecutor {
    /// Executes the given steps in order using the current executable.
    ///
    /// - `input_path`: optional initial input file. If `None`, stdin is read.
    /// - `output_path`: optional final output file. If `None`, final step stdout is forwarded.
    /// - `silent`: whether to suppress informational stderr from intermediate steps.
    pub fn execute(
        steps: &[String],
        input_path: Option<&str>,
        output_path: Option<&str>,
        silent: bool,
    ) -> Result<()> {
        let exe = std::env::current_exe().map_err(|e| {
            BinfiddleError::Io(std::io::Error::new(
                e.kind(),
                "Failed to determine current executable path".to_string(),
            ))
        })?;
        Self::execute_with_exe(steps, input_path, output_path, silent, &exe)
    }

    /// Executes the chain using a specific executable path.
    pub fn execute_with_exe(
        steps: &[String],
        input_path: Option<&str>,
        output_path: Option<&str>,
        silent: bool,
        exe: &std::path::Path,
    ) -> Result<()> {
        if steps.is_empty() {
            return Err(BinfiddleError::InvalidInput(
                "Chain requires at least one --step".to_string(),
            ));
        }

        // Create the initial temporary input file.
        let output_path = normalize_output_path(output_path);
        let mut current_input = create_initial_temp(input_path)?;

        for (i, step) in steps.iter().enumerate() {
            let step_num = i + 1;
            let is_last = i == steps.len() - 1;

            // Parse the step into arguments, respecting shell-style quoting.
            let args = shell_words::split(step).map_err(|e| {
                BinfiddleError::Parse(format!(
                    "Failed to parse step {} '{}': {}",
                    step_num, step, e
                ))
            })?;

            // Prepare the next temp file for intermediate steps.
            let next_temp = if !is_last {
                Some(tempfile::NamedTempFile::new()?)
            } else {
                None
            };

            // Build the subprocess command.
            let mut cmd = Command::new(exe);
            cmd.arg("--input").arg(current_input.path());

            if let Some(next) = &next_temp {
                cmd.arg("--output").arg(next.path());
            } else if let Some(out) = output_path {
                cmd.arg("--output").arg(out);
            }

            if silent {
                cmd.arg("--silent");
            }

            cmd.args(&args);

            // Run the step and capture output.
            let output = cmd.output()?;

            if !output.status.success() {
                let stderr = String::from_utf8_lossy(&output.stderr).to_string();
                return Err(BinfiddleError::ChainStepFailed {
                    step: step_num,
                    command: step.clone(),
                    stderr,
                });
            }

            // Forward stderr from every step (diagnostic output).
            if !silent && !output.stderr.is_empty() {
                io::stderr().write_all(&output.stderr)?;
            }

            // Forward stdout only from the final step when no explicit output file.
            if is_last && output_path.is_none() && !output.stdout.is_empty() {
                io::stdout().write_all(&output.stdout)?;
            }

            if let Some(next) = next_temp {
                // Validate that the intermediate step actually wrote output.
                let metadata = std::fs::metadata(next.path())?;
                if metadata.len() == 0 {
                    return Err(BinfiddleError::InvalidInput(format!(
                        "Chain step {} ('{}') produced no byte output; intermediate steps must produce bytes (e.g., read, write, edit, convert, patch)",
                        step_num, step
                    )));
                }
                current_input = next;
            }
        }

        Ok(())
    }
}

/// Creates the initial temporary file for the chain.
///
/// `-` is treated as stdin, matching the normal CLI input convention.
fn create_initial_temp(input_path: Option<&str>) -> Result<tempfile::NamedTempFile> {
    let mut temp = tempfile::NamedTempFile::new()?;

    match input_path {
        Some("-") | None => {
            let mut stdin = Vec::new();
            io::stdin().read_to_end(&mut stdin)?;
            temp.write_all(&stdin)?;
        }
        Some(path) => {
            let data = std::fs::read(path)?;
            temp.write_all(&data)?;
        }
    }

    temp.flush()?;
    Ok(temp)
}

/// Normalize `-` output path to `None` so final step stdout is forwarded.
fn normalize_output_path(output_path: Option<&str>) -> Option<&str> {
    match output_path {
        Some("-") | None => None,
        Some(path) => Some(path),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    /// Locate the built binfiddle binary for use by subprocess tests.
    ///
    /// Checks, in order:
    /// - The `CARGO_BIN_EXE_binfiddle` environment variable set by Cargo for integration tests.
    /// - The `target/debug/binfiddle` and `target/release/binfiddle` paths relative to the
    ///   crate manifest directory (works for both `cargo test` and `cargo test --release`).
    /// - The `binfiddle` executable on `PATH`.
    fn find_bin_fiddle_binary() -> Option<std::path::PathBuf> {
        if let Ok(path) = std::env::var("CARGO_BIN_EXE_binfiddle") {
            let p = std::path::PathBuf::from(path);
            if p.exists() {
                return Some(p);
            }
        }

        let manifest_dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let candidates = [
            manifest_dir.join("target").join("debug").join("binfiddle"),
            manifest_dir
                .join("target")
                .join("release")
                .join("binfiddle"),
        ];
        for candidate in &candidates {
            if candidate.exists() {
                return Some(candidate.clone());
            }
        }

        // Fall back to PATH.
        std::env::var_os("PATH").and_then(|paths| {
            std::env::split_paths(&paths)
                .map(|p| p.join("binfiddle"))
                .find(|p| p.exists())
        })
    }

    #[test]
    fn test_empty_steps_rejected() {
        let result = ChainExecutor::execute(&[], None, None, true);
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("at least one --step"));
    }

    #[test]
    fn test_two_step_byte_chain() {
        let exe =
            find_bin_fiddle_binary().expect("binfiddle binary not found; run cargo build first");

        // Build a test input file and chain two byte-producing commands.
        let input = tempfile::NamedTempFile::new().unwrap();
        input
            .as_file()
            .write_all(&[0x00, 0x11, 0x22, 0x33])
            .unwrap();
        input.as_file().flush().unwrap();

        let output = tempfile::NamedTempFile::new().unwrap();

        ChainExecutor::execute_with_exe(
            &[
                "edit replace 0..2 4142".to_string(),
                "write 2 9999".to_string(),
            ],
            Some(input.path().to_str().unwrap()),
            Some(output.path().to_str().unwrap()),
            true,
            &exe,
        )
        .unwrap();

        let result = std::fs::read(output.path()).unwrap();
        assert_eq!(result, vec![0x41, 0x42, 0x99, 0x99]);
    }

    #[test]
    fn test_intermediate_step_must_produce_bytes() {
        let exe =
            find_bin_fiddle_binary().expect("binfiddle binary not found; run cargo build first");

        // The first step reads bytes, but the second step (search without raw output)
        // writes text to stdout and leaves --output empty, so the chain should fail.
        let input = tempfile::NamedTempFile::new().unwrap();
        input
            .as_file()
            .write_all(&[0xDE, 0xAD, 0xBE, 0xEF])
            .unwrap();
        input.as_file().flush().unwrap();

        let result = ChainExecutor::execute_with_exe(
            &[
                "read 0..4".to_string(),
                "search DEADBEEF --color never".to_string(),
                "read 0..2".to_string(),
            ],
            Some(input.path().to_str().unwrap()),
            None,
            true,
            &exe,
        );

        assert!(result.is_err());
        let msg = result.unwrap_err().to_string();
        assert!(msg.contains("produced no byte output"), "Got: {}", msg);
    }

    #[test]
    fn test_invalid_step_parse_fails() {
        let exe =
            find_bin_fiddle_binary().expect("binfiddle binary not found; run cargo build first");

        let input = tempfile::NamedTempFile::new().unwrap();
        input.as_file().write_all(&[0x00]).unwrap();
        input.as_file().flush().unwrap();

        let result = ChainExecutor::execute_with_exe(
            &["read --bad-flag".to_string()],
            Some(input.path().to_str().unwrap()),
            None,
            true,
            &exe,
        );

        assert!(result.is_err());
    }

    #[test]
    fn test_shell_quoting_respected() {
        // Just verify shell_words splits quoted arguments correctly.
        let args = shell_words::split(r#"search "hello world" --ascii"#).unwrap();
        assert_eq!(args, vec!["search", "hello world", "--ascii"]);
    }
}
