//! Cooperative cancellation token and terminal-signal guard.
//!
//! The token is a plain shared flag: producers call [`CancellationToken::cancel`]
//! (a signal handler, a UI action, or a deadline watcher) and long-running
//! operations observe it at checkpoints between bounded steps.
//!
//! [`SignalGuard`] installs terminal-signal handlers (SIGINT, SIGTERM on Unix;
//! SIGINT, SIGBREAK on Windows) that set a process-global flag which the guard
//! exposes. The guard restores the previous handlers on drop so the ambient
//! process behavior is unchanged outside an NN operation.

use super::error::NnError;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

/// Shared, cloneable cancellation flag.
#[derive(Clone, Default)]
pub struct CancellationToken {
    flag: Arc<AtomicBool>,
}

impl std::fmt::Debug for CancellationToken {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CancellationToken")
            .field("cancelled", &self.is_cancelled())
            .finish()
    }
}

impl CancellationToken {
    /// Create a fresh, uncancelled token.
    pub fn new() -> Self {
        CancellationToken {
            flag: Arc::new(AtomicBool::new(false)),
        }
    }

    /// Request cancellation. Idempotent.
    pub fn cancel(&self) {
        self.flag.store(true, Ordering::SeqCst);
    }

    /// Whether cancellation was requested.
    pub fn is_cancelled(&self) -> bool {
        self.flag.load(Ordering::SeqCst)
    }

    /// Return `Cancelled` if cancellation was requested.
    pub fn check(&self) -> Result<(), NnError> {
        if self.is_cancelled() {
            Err(NnError::Cancelled)
        } else {
            Ok(())
        }
    }
}

static SIGNALLED: AtomicBool = AtomicBool::new(false);

extern "C" fn on_terminal_signal(_signal: i32) {
    // Only async-signal-safe work happens here: setting an atomic flag.
    SIGNALLED.store(true, Ordering::SeqCst);
}

/// Guard that maps terminal signals to a process-global flag for the duration
/// of its lifetime, restoring prior handlers when dropped.
///
/// The NN command runner installs this for the duration of one command so that
/// legacy command behavior (default signal disposition) is untouched.
#[derive(Debug)]
pub struct SignalGuard {
    #[cfg(unix)]
    previous: Vec<(i32, libc::sigaction)>,
    #[cfg(windows)]
    previous: Vec<(i32, usize)>,
}

impl SignalGuard {
    /// Install handlers for terminal signals. Fails if the platform rejects
    /// the handler installation.
    pub fn install() -> Result<SignalGuard, NnError> {
        SIGNALLED.store(false, Ordering::SeqCst);
        let signals = terminal_signals();
        let mut previous = Vec::with_capacity(signals.len());
        for &signal in &signals {
            previous.push((signal, set_handler(signal)?));
        }
        Ok(SignalGuard { previous })
    }

    /// Whether a terminal signal arrived since installation.
    pub fn signalled(&self) -> bool {
        SIGNALLED.load(Ordering::SeqCst)
    }

    /// Propagate the signal flag into a cancellation token.
    pub fn propagate(&self, token: &CancellationToken) {
        if self.signalled() {
            token.cancel();
        }
    }
}

impl Drop for SignalGuard {
    fn drop(&mut self) {
        for (signal, handler) in self.previous.drain(..) {
            let _ = restore_handler(signal, handler);
        }
    }
}

#[cfg(unix)]
fn terminal_signals() -> Vec<i32> {
    use libc::{SIGINT, SIGTERM};
    vec![SIGINT, SIGTERM]
}

/// The CRT's SIGBREAK (Ctrl+Break proxy on Windows). The `libc` crate does
/// not export it for Windows targets; it is a stable CRT constant (21).
#[cfg(windows)]
pub(crate) const SIGBREAK: i32 = 21;

#[cfg(windows)]
fn terminal_signals() -> Vec<i32> {
    use libc::SIGINT;
    vec![SIGINT, SIGBREAK]
}

#[cfg(unix)]
fn set_handler(signal: i32) -> Result<libc::sigaction, NnError> {
    unsafe {
        let mut action: libc::sigaction = std::mem::zeroed();
        action.sa_sigaction = on_terminal_signal as *const () as usize;
        action.sa_flags = 0;
        if libc::sigemptyset(&mut action.sa_mask) != 0 {
            return Err(signal_error(signal, "sigemptyset failed"));
        }
        let mut previous: libc::sigaction = std::mem::zeroed();
        if libc::sigaction(signal, &action, &mut previous) != 0 {
            return Err(signal_error(signal, "sigaction failed"));
        }
        Ok(previous)
    }
}

#[cfg(unix)]
fn restore_handler(signal: i32, previous: libc::sigaction) -> Result<(), NnError> {
    unsafe {
        if libc::sigaction(signal, &previous, std::ptr::null_mut()) != 0 {
            return Err(signal_error(signal, "sigaction restore failed"));
        }
    }
    Ok(())
}

#[cfg(windows)]
fn set_handler(signal: i32) -> Result<usize, NnError> {
    let handler = on_terminal_signal as *const () as usize;
    let previous = unsafe { libc::signal(signal, handler) };
    if previous == libc::SIG_ERR as usize {
        return Err(signal_error(signal, "signal registration failed"));
    }
    Ok(previous)
}

#[cfg(windows)]
fn restore_handler(signal: i32, previous: usize) -> Result<(), NnError> {
    let result = unsafe { libc::signal(signal, previous) };
    if result == libc::SIG_ERR as usize {
        return Err(signal_error(signal, "signal restore failed"));
    }
    Ok(())
}

fn signal_error(signal: i32, detail: &str) -> NnError {
    NnError::Io(std::io::Error::other(format!(
        "cannot install handler for signal {signal}: {detail}"
    )))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn token_starts_uncancelled() {
        let token = CancellationToken::new();
        assert!(!token.is_cancelled());
        assert!(token.check().is_ok());
    }

    #[test]
    fn cancel_is_idempotent_and_shared() {
        let token = CancellationToken::new();
        let clone = token.clone();
        token.cancel();
        token.cancel();
        assert!(clone.is_cancelled());
        assert_eq!(clone.check().unwrap_err().code().as_str(), "CANCELLED");
    }

    #[test]
    fn independent_tokens_do_not_interfere() {
        let a = CancellationToken::new();
        let b = CancellationToken::new();
        a.cancel();
        assert!(a.is_cancelled());
        assert!(!b.is_cancelled());
    }

    #[test]
    fn guard_installs_and_restores() {
        // Install/restore only; no signal is raised here because test threads
        // share one process and handlers are process-global. Signal delivery
        // is exercised by the dedicated integration test.
        let signalled = {
            let guard = SignalGuard::install().expect("install signal guard");
            guard.signalled()
        };
        assert!(!signalled);
        // A second installation also works (previous handler was our own).
        let guard = SignalGuard::install().expect("reinstall signal guard");
        assert!(!guard.signalled());
        drop(guard);
    }
}
