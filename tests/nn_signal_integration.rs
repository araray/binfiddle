//! Signal-guard integration: terminal signals set the guard flag and
//! propagate to cancellation tokens. Lives in its own test binary because
//! signal handlers are process-global; the checks run sequentially in one
//! test to avoid racing handler installation between threads.

use binfiddle::nn::{CancellationToken, SignalGuard};

#[test]
fn terminal_signals_set_guard_flag_and_cancel_token() {
    // SIGINT: flag + token propagation.
    {
        let guard = SignalGuard::install().expect("install signal guard");
        let token = CancellationToken::new();
        unsafe { libc::raise(libc::SIGINT) };
        assert!(guard.signalled());
        guard.propagate(&token);
        assert!(token.is_cancelled());
        assert_eq!(token.check().unwrap_err().code().as_str(), "CANCELLED");
    }

    // Second terminal signal: flag. install() resets the flag first.
    // Unix guards SIGINT+SIGTERM; Windows guards SIGINT+SIGBREAK (the CRT
    // proxy for Ctrl+Break), and raise(SIGTERM) would terminate the process
    // under the default disposition on Windows.
    #[cfg(unix)]
    {
        let guard = SignalGuard::install().expect("install signal guard");
        unsafe { libc::raise(libc::SIGTERM) };
        assert!(guard.signalled());
    }
    #[cfg(windows)]
    {
        let guard = SignalGuard::install().expect("install signal guard");
        // The libc crate does not export SIGBREAK on Windows; the CRT
        // constant is 21 (Ctrl+Break proxy).
        unsafe { libc::raise(21) };
        assert!(guard.signalled());
    }

    // After restore, the ambient (default) disposition is back in place; a
    // fresh guard still starts un-signalled.
    {
        let guard = SignalGuard::install().expect("install signal guard");
        assert!(!guard.signalled());
    }
}
