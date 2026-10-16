//! Failure budgets for edits the sweep retries. A write the shell keeps refusing (a property
//! that no longer applies on a new build, an element that changed type) must not be retried for
//! the life of Explorer: after [`LIMIT`] consecutive failures the feature logs once and turns
//! itself off until the next stand-down.

use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};

use crate::log::logf;

/// Consecutive failures before giving up: 10 s at the fast sweep pace.
pub const LIMIT: u32 = 40;

pub struct Budget {
    what: &'static str,
    failures: AtomicU32,
    spent: AtomicBool,
}

impl Budget {
    pub const fn new(what: &'static str) -> Self {
        Self {
            what,
            failures: AtomicU32::new(0),
            spent: AtomicBool::new(false),
        }
    }

    /// Whether the edit may still be tried.
    pub fn allowed(&self) -> bool {
        !self.spent.load(Ordering::SeqCst)
    }

    /// Records one attempt's outcome; a success refills the budget.
    pub fn record(&self, ok: bool) {
        if ok {
            self.failures.store(0, Ordering::SeqCst);
            return;
        }
        if self.failures.fetch_add(1, Ordering::SeqCst) + 1 == LIMIT {
            self.spent.store(true, Ordering::SeqCst);
            logf!(
                "{}: {LIMIT} consecutive failures — disabled until the next stand-down",
                self.what
            );
        }
    }

    /// Gives up at once: for a failure that retrying cannot fix.
    pub fn spend(&self) {
        self.failures.store(LIMIT, Ordering::SeqCst);
        self.spent.store(true, Ordering::SeqCst);
    }

    pub fn reset(&self) {
        self.failures.store(0, Ordering::SeqCst);
        self.spent.store(false, Ordering::SeqCst);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spends_after_the_limit_and_a_success_refills() {
        let budget = Budget::new("test");
        for _ in 0..LIMIT - 1 {
            budget.record(false);
        }
        budget.record(true);
        for _ in 0..LIMIT - 1 {
            budget.record(false);
        }
        assert!(budget.allowed());
        budget.record(false);
        assert!(!budget.allowed());
        budget.reset();
        assert!(budget.allowed());
    }
}
