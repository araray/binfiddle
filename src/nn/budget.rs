//! Hierarchical resource budgets and cooperative checkpoints.
//!
//! Every NN operation runs under explicit ceilings for metadata bytes, resident
//! decode memory, source bytes read, generated objects, expression steps,
//! output bytes, workers, and open handles, plus an optional wall-clock
//! deadline. These are hard accounting boundaries: consumption that would
//! exceed a ceiling fails with `BUDGET_EXCEEDED` instead of degrading silently.
//!
//! A parent budget shares one account table with the budgets it spawns, so
//! sibling operations can never collectively exceed the parent's ceiling even
//! though each child is granted its own cap.

use super::cancel::CancellationToken;
use super::error::NnError;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// Per-resource ceilings carried by a budget handle.
#[derive(Debug, Clone, Copy)]
pub struct BudgetCaps {
    pub metadata_bytes: u64,
    pub decode_bytes: u64,
    pub source_bytes_read: u64,
    pub generated_objects: u64,
    pub expression_steps: u64,
    pub output_bytes: u64,
    pub workers: usize,
    pub open_handles: usize,
}

impl Default for BudgetCaps {
    fn default() -> Self {
        BudgetCaps {
            metadata_bytes: 256 * 1024 * 1024,
            decode_bytes: 256 * 1024 * 1024,
            source_bytes_read: u64::MAX,
            generated_objects: 1_000_000,
            expression_steps: 1_000_000,
            output_bytes: 256 * 1024 * 1024,
            workers: std::thread::available_parallelism()
                .map(|n| n.get())
                .unwrap_or(1),
            open_handles: 64,
        }
    }
}

#[derive(Debug, Default)]
struct Accounts {
    metadata_bytes: u64,
    decode_bytes: u64,
    source_bytes_read: u64,
    generated_objects: u64,
    expression_steps: u64,
    output_bytes: u64,
    open_handles: usize,
}

/// A budget handle. Cheap to clone; clones share the same account table and
/// the same caps snapshot. The effective `output_bytes` ceiling additionally
/// honors a monotonic justified floor (see [`Budget::justify_output_bytes`]).
#[derive(Clone)]
pub struct Budget {
    accounts: Arc<Mutex<Accounts>>,
    caps: BudgetCaps,
    /// Monotonic floor for the effective output cap: exact-output operations
    /// (whole-model copies with in-place patches, materialized bundles) raise
    /// it to their provable output bound so real-world model sizes are not
    /// rejected by the generic default. It can only grow, never shrink.
    output_floor: Arc<AtomicU64>,
    deadline: Option<Instant>,
    cancel: CancellationToken,
}

impl std::fmt::Debug for Budget {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Budget")
            .field("caps", &self.caps)
            .field("deadline", &self.deadline)
            .field("cancelled", &self.cancel.is_cancelled())
            .finish()
    }
}

impl Budget {
    /// Create a root budget with the given caps, optional wall-clock deadline,
    /// and cancellation token.
    pub fn new(caps: BudgetCaps, deadline: Option<Duration>, cancel: CancellationToken) -> Self {
        Budget {
            accounts: Arc::new(Mutex::new(Accounts::default())),
            caps,
            output_floor: Arc::new(AtomicU64::new(0)),
            deadline: deadline.map(|d| Instant::now() + d),
            cancel,
        }
    }

    /// Create a root budget with default caps and no deadline or cancellation.
    pub fn with_default_caps() -> Self {
        Budget::new(BudgetCaps::default(), None, CancellationToken::new())
    }

    /// Create a root budget with no caps (every limit at its maximum). For
    /// tests and internal paths that impose their own bounds; command entry
    /// points use [`Budget::with_default_caps`] so generic runaway output is
    /// still bounded by default.
    pub fn unrestricted() -> Self {
        Budget::new(
            BudgetCaps {
                metadata_bytes: u64::MAX,
                decode_bytes: u64::MAX,
                source_bytes_read: u64::MAX,
                generated_objects: u64::MAX,
                expression_steps: u64::MAX,
                output_bytes: u64::MAX,
                workers: std::thread::available_parallelism()
                    .map(|n| n.get())
                    .unwrap_or(1),
                open_handles: usize::MAX,
            },
            None,
            CancellationToken::new(),
        )
    }

    /// Raise the effective `output_bytes` ceiling to at least `bound`.
    ///
    /// Callers must pass a provable upper bound on the bytes they will emit
    /// (for example, the exact length of a whole-source copy). The floor is
    /// monotonic across clones and threads; it can never lower a configured
    /// cap.
    pub fn justify_output_bytes(&self, bound: u64) {
        let mut current = self.output_floor.load(Ordering::Relaxed);
        while bound > current {
            match self.output_floor.compare_exchange_weak(
                current,
                bound,
                Ordering::Relaxed,
                Ordering::Relaxed,
            ) {
                Ok(_) => break,
                Err(observed) => current = observed,
            }
        }
    }

    /// Effective output ceiling: the configured cap or the justified floor,
    /// whichever is larger.
    fn effective_output_cap(&self) -> u64 {
        self.caps
            .output_bytes
            .max(self.output_floor.load(Ordering::Relaxed))
    }

    /// Spawn a child budget. The child receives its own caps (which the caller
    /// is expected to derive from the parent's remaining allowance) but shares
    /// the parent's account table and justified output floor, so total
    /// consumption across the tree stays within the root ceilings.
    pub fn child(&self, caps: BudgetCaps, deadline: Option<Duration>) -> Budget {
        Budget {
            accounts: Arc::clone(&self.accounts),
            caps,
            output_floor: Arc::clone(&self.output_floor),
            deadline: deadline.map(|d| Instant::now() + d),
            cancel: self.cancel.clone(),
        }
    }

    /// The caps snapshot of this handle.
    pub fn caps(&self) -> BudgetCaps {
        self.caps
    }

    /// The cancellation token observed by checkpoints (shared with children).
    pub fn cancel_token(&self) -> &CancellationToken {
        &self.cancel
    }

    fn charge(
        &self,
        resource: &'static str,
        cap: u64,
        used: u64,
        amount: u64,
    ) -> Result<(), NnError> {
        if amount > cap {
            return Err(NnError::BudgetExceeded {
                resource,
                limit: cap,
                requested: amount,
            });
        }
        let total = used.checked_add(amount).ok_or(NnError::BudgetExceeded {
            resource,
            limit: cap,
            requested: u64::MAX,
        })?;
        if total > cap {
            return Err(NnError::BudgetExceeded {
                resource,
                limit: cap,
                // Report the cumulative requirement, not just this call's
                // slice, so the message states what the operation needs.
                requested: total,
            });
        }
        Ok(())
    }

    /// Record consumption of metadata bytes (headers, indexes, descriptors).
    pub fn consume_metadata(&self, bytes: u64) -> Result<(), NnError> {
        self.checkpoint()?;
        let mut accounts = self.lock_accounts()?;
        self.charge(
            "metadata_bytes",
            self.caps.metadata_bytes,
            accounts.metadata_bytes,
            bytes,
        )?;
        accounts.metadata_bytes += bytes;
        Ok(())
    }

    /// Record source bytes read.
    pub fn consume_source_read(&self, bytes: u64) -> Result<(), NnError> {
        self.checkpoint()?;
        let mut accounts = self.lock_accounts()?;
        self.charge(
            "source_bytes_read",
            self.caps.source_bytes_read,
            accounts.source_bytes_read,
            bytes,
        )?;
        accounts.source_bytes_read += bytes;
        Ok(())
    }

    /// Record generated output bytes.
    pub fn consume_output(&self, bytes: u64) -> Result<(), NnError> {
        self.checkpoint()?;
        let mut accounts = self.lock_accounts()?;
        self.charge(
            "output_bytes",
            self.effective_output_cap(),
            accounts.output_bytes,
            bytes,
        )?;
        accounts.output_bytes += bytes;
        Ok(())
    }

    /// Record generated catalog objects.
    pub fn consume_generated(&self, count: u64) -> Result<(), NnError> {
        self.checkpoint()?;
        let mut accounts = self.lock_accounts()?;
        self.charge(
            "generated_objects",
            self.caps.generated_objects,
            accounts.generated_objects,
            count,
        )?;
        accounts.generated_objects += count;
        Ok(())
    }

    /// Record expression-evaluation steps.
    pub fn consume_steps(&self, count: u64) -> Result<(), NnError> {
        self.checkpoint()?;
        let mut accounts = self.lock_accounts()?;
        self.charge(
            "expression_steps",
            self.caps.expression_steps,
            accounts.expression_steps,
            count,
        )?;
        accounts.expression_steps += count;
        Ok(())
    }

    /// Reserve resident decode memory. The reservation is released when the
    /// returned guard drops.
    pub fn reserve_decode(&self, bytes: u64) -> Result<DecodeReservation<'_>, NnError> {
        self.checkpoint()?;
        let mut accounts = self.lock_accounts()?;
        self.charge(
            "decode_bytes",
            self.caps.decode_bytes,
            accounts.decode_bytes,
            bytes,
        )?;
        accounts.decode_bytes += bytes;
        Ok(DecodeReservation {
            accounts: &self.accounts,
            bytes: Some(bytes),
        })
    }

    /// Acquire one open-handle slot. Released when the guard drops.
    pub fn acquire_handle(&self) -> Result<HandleGuard<'_>, NnError> {
        self.checkpoint()?;
        let mut accounts = self.lock_accounts()?;
        if accounts.open_handles >= self.caps.open_handles {
            return Err(NnError::BudgetExceeded {
                resource: "open_handles",
                limit: self.caps.open_handles as u64,
                requested: (accounts.open_handles + 1) as u64,
            });
        }
        accounts.open_handles += 1;
        Ok(HandleGuard {
            accounts: &self.accounts,
        })
    }

    /// Check cancellation, deadline, and return a `Cancelled` or
    /// `BudgetExceeded` error when the operation must stop. Call between
    /// bounded steps of any long-running operation.
    pub fn checkpoint(&self) -> Result<(), NnError> {
        if self.cancel.is_cancelled() {
            return Err(NnError::Cancelled);
        }
        if let Some(deadline) = self.deadline {
            if Instant::now() >= deadline {
                return Err(NnError::BudgetExceeded {
                    resource: "wall_clock",
                    limit: 0,
                    requested: 0,
                });
            }
        }
        Ok(())
    }

    /// Current consumption snapshot (for reports).
    pub fn consumed(&self) -> BudgetConsumed {
        let accounts = self.lock_accounts().expect("accounts mutex poisoned");
        BudgetConsumed {
            metadata_bytes: accounts.metadata_bytes,
            decode_bytes: accounts.decode_bytes,
            source_bytes_read: accounts.source_bytes_read,
            generated_objects: accounts.generated_objects,
            expression_steps: accounts.expression_steps,
            output_bytes: accounts.output_bytes,
            open_handles: accounts.open_handles,
        }
    }

    fn lock_accounts(&self) -> Result<std::sync::MutexGuard<'_, Accounts>, NnError> {
        self.accounts
            .lock()
            .map_err(|_| NnError::Io(std::io::Error::other("budget accounts mutex poisoned")))
    }
}

/// Consumption snapshot for reporting.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BudgetConsumed {
    pub metadata_bytes: u64,
    pub decode_bytes: u64,
    pub source_bytes_read: u64,
    pub generated_objects: u64,
    pub expression_steps: u64,
    pub output_bytes: u64,
    pub open_handles: usize,
}

/// RAII release of reserved decode memory.
#[derive(Debug)]
pub struct DecodeReservation<'a> {
    accounts: &'a Mutex<Accounts>,
    bytes: Option<u64>,
}

impl Drop for DecodeReservation<'_> {
    fn drop(&mut self) {
        if let Some(bytes) = self.bytes.take() {
            if let Ok(mut accounts) = self.accounts.lock() {
                accounts.decode_bytes = accounts.decode_bytes.saturating_sub(bytes);
            }
        }
    }
}

/// RAII release of an open-handle slot.
#[derive(Debug)]
pub struct HandleGuard<'a> {
    accounts: &'a Mutex<Accounts>,
}

impl Drop for HandleGuard<'_> {
    fn drop(&mut self) {
        if let Ok(mut accounts) = self.accounts.lock() {
            accounts.open_handles = accounts.open_handles.saturating_sub(1);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tight_caps() -> BudgetCaps {
        BudgetCaps {
            metadata_bytes: 100,
            decode_bytes: 100,
            source_bytes_read: 100,
            generated_objects: 10,
            expression_steps: 10,
            output_bytes: 100,
            workers: 2,
            open_handles: 2,
        }
    }

    #[test]
    fn charges_within_limits_succeed() {
        let budget = Budget::new(tight_caps(), None, CancellationToken::new());
        budget.consume_metadata(60).unwrap();
        budget.consume_metadata(40).unwrap();
        budget.consume_output(100).unwrap();
        budget.consume_generated(10).unwrap();
        budget.consume_steps(10).unwrap();
        let used = budget.consumed();
        assert_eq!(used.metadata_bytes, 100);
        assert_eq!(used.output_bytes, 100);
    }

    #[test]
    fn exceeding_a_limit_fails_with_budget_exceeded() {
        let budget = Budget::new(tight_caps(), None, CancellationToken::new());
        budget.consume_metadata(60).unwrap();
        let err = budget.consume_metadata(41).unwrap_err();
        assert_eq!(err.code().as_str(), "BUDGET_EXCEEDED");
        assert!(err.to_string().contains("metadata_bytes"));
        // The failed charge must not be partially applied.
        assert_eq!(budget.consumed().metadata_bytes, 60);
    }

    /// Regression: the error must state the cumulative requirement, not the
    /// size of the failing slice (a 1 MiB chunk over a 256 MiB cap used to
    /// print "requested 1048576, available 268435456" — contradictory).
    #[test]
    fn budget_errors_report_cumulative_requirements() {
        let budget = Budget::new(tight_caps(), None, CancellationToken::new());
        budget.consume_output(60).unwrap();
        let err = budget.consume_output(41).unwrap_err();
        match err {
            NnError::BudgetExceeded {
                resource,
                limit,
                requested,
            } => {
                assert_eq!(resource, "output_bytes");
                assert_eq!(limit, 100);
                assert_eq!(requested, 101, "60 already used + 41 requested");
            }
            other => panic!("unexpected error: {other:?}"),
        }
    }

    /// Exact-output operations (whole-model copies) justify their output cap
    /// with a provable bound; the floor is monotonic and shared across clones
    /// but can never lower the configured cap.
    #[test]
    fn justified_output_floor_raises_and_shares_monotonically() {
        let budget = Budget::new(tight_caps(), None, CancellationToken::new());
        let clone = budget.clone();
        // Raise through the clone; the original sees the floor.
        clone.justify_output_bytes(500);
        budget.consume_output(100).unwrap(); // exactly the configured cap
        budget.consume_output(400).unwrap(); // only allowed by the floor
        assert!(budget.consume_output(1).is_err(), "500 floor reached");

        // A smaller justification must not lower the floor.
        budget.justify_output_bytes(10);
        assert!(budget.consume_output(1).is_err());
    }

    #[test]
    fn single_charge_above_cap_fails() {
        let budget = Budget::new(tight_caps(), None, CancellationToken::new());
        let err = budget.consume_metadata(101).unwrap_err();
        assert_eq!(err.code().as_str(), "BUDGET_EXCEEDED");
    }

    #[test]
    fn children_share_the_account_table() {
        let parent = Budget::new(tight_caps(), None, CancellationToken::new());
        let child_a = parent.child(tight_caps(), None);
        let child_b = parent.child(tight_caps(), None);
        child_a.consume_metadata(60).unwrap();
        child_b.consume_metadata(40).unwrap();
        // A third charge anywhere in the tree exceeds the shared ceiling.
        assert_eq!(
            parent.consume_metadata(1).unwrap_err().code().as_str(),
            "BUDGET_EXCEEDED"
        );
        assert_eq!(child_a.consumed().metadata_bytes, 100);
    }

    #[test]
    fn child_caps_are_enforced_per_handle() {
        let parent = Budget::unrestricted();
        let child_caps = BudgetCaps {
            metadata_bytes: 10,
            ..BudgetCaps::default()
        };
        let child = parent.child(child_caps, None);
        assert_eq!(
            child.consume_metadata(11).unwrap_err().code().as_str(),
            "BUDGET_EXCEEDED"
        );
        // Parent ceiling is far higher, so the parent itself is fine.
        parent.consume_metadata(11).unwrap();
    }

    #[test]
    fn decode_reservations_release_on_drop() {
        let budget = Budget::new(tight_caps(), None, CancellationToken::new());
        {
            let _guard = budget.reserve_decode(100).unwrap();
            assert_eq!(
                budget.reserve_decode(1).unwrap_err().code().as_str(),
                "BUDGET_EXCEEDED"
            );
        }
        assert_eq!(budget.consumed().decode_bytes, 0);
        let _again = budget.reserve_decode(100).unwrap();
    }

    #[test]
    fn handle_slots_release_on_drop() {
        let budget = Budget::new(tight_caps(), None, CancellationToken::new());
        let h1 = budget.acquire_handle().unwrap();
        let _h2 = budget.acquire_handle().unwrap();
        assert_eq!(
            budget.acquire_handle().unwrap_err().code().as_str(),
            "BUDGET_EXCEEDED"
        );
        drop(h1);
        let _h3 = budget.acquire_handle().unwrap();
        assert_eq!(budget.consumed().open_handles, 2);
    }

    #[test]
    fn cancellation_propagates_to_checkpoints() {
        let cancel = CancellationToken::new();
        let budget = Budget::new(BudgetCaps::default(), None, cancel.clone());
        budget.consume_metadata(1).unwrap();
        cancel.cancel();
        assert_eq!(
            budget.checkpoint().unwrap_err().code().as_str(),
            "CANCELLED"
        );
        assert_eq!(
            budget.consume_metadata(1).unwrap_err().code().as_str(),
            "CANCELLED"
        );
    }

    #[test]
    fn deadline_expires() {
        let budget = Budget::new(
            BudgetCaps::default(),
            Some(Duration::from_millis(0)),
            CancellationToken::new(),
        );
        std::thread::sleep(Duration::from_millis(2));
        let err = budget.checkpoint().unwrap_err();
        assert_eq!(err.code().as_str(), "BUDGET_EXCEEDED");
        assert!(err.to_string().contains("wall_clock"));
    }

    #[test]
    fn child_inherits_parent_cancellation() {
        let parent = Budget::new(BudgetCaps::default(), None, CancellationToken::new());
        let child = parent.child(BudgetCaps::default(), None);
        child.consume_steps(1).unwrap();
        assert!(child.checkpoint().is_ok());
        parent.cancel_token().cancel();
        assert_eq!(child.checkpoint().unwrap_err().code().as_str(), "CANCELLED");
    }
}
