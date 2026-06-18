//! Progress-bar helpers for long-running commands.
//!
//! Wraps `indicatif` so commands can show progress when running in a terminal
//! and stay quiet otherwise (or when `--silent` is set).

use indicatif::{ProgressBar, ProgressDrawTarget, ProgressStyle};
use std::io::{self, Read};

/// A thin wrapper around an `indicatif` progress bar.
pub struct Progress {
    bar: ProgressBar,
}

impl Progress {
    /// Creates a new progress bar.
    ///
    /// - `total`: total number of bytes, or `None` for an indeterminate spinner.
    /// - `message`: short description shown on the left of the bar.
    /// - `silent`: if true, the bar is hidden.
    pub fn new(total: Option<u64>, message: &str, silent: bool) -> Self {
        let hidden = silent || !atty::is(atty::Stream::Stderr);
        let bar = if hidden {
            ProgressBar::hidden()
        } else {
            ProgressBar::new(total.unwrap_or(0))
        };

        if !hidden {
            let style = match total {
                Some(_) => ProgressStyle::with_template(
                    "{msg} [{bar:40.cyan/blue}] {bytes}/{total_bytes} ({bytes_per_sec}, ETA {eta})",
                )
                .unwrap()
                .progress_chars("=>-"),
                None => ProgressStyle::with_template("{spinner} {msg} {bytes} ({bytes_per_sec})")
                    .unwrap(),
            };
            bar.set_style(style);
            bar.set_message(message.to_string());
            // Draw to stderr so stdout remains clean for command output.
            bar.set_draw_target(ProgressDrawTarget::stderr());
        }

        Self { bar }
    }

    /// Advances the bar by `n` bytes.
    pub fn inc(&self, n: u64) {
        self.bar.inc(n);
    }

    /// Finishes the bar and clears it from the terminal.
    pub fn finish(&self) {
        self.bar.finish_and_clear();
    }
}

/// A [`Read`] wrapper that increments a [`Progress`] bar as bytes are read.
pub struct ProgressReader<R> {
    inner: R,
    progress: Progress,
}

impl<R> ProgressReader<R> {
    /// Wraps a reader with a progress bar.
    pub fn new(inner: R, progress: Progress) -> Self {
        Self { inner, progress }
    }
}

impl<R: Read> Read for ProgressReader<R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let n = self.inner.read(buf)?;
        self.progress.inc(n as u64);
        Ok(n)
    }
}

impl<R> Drop for ProgressReader<R> {
    fn drop(&mut self) {
        self.progress.finish();
    }
}
