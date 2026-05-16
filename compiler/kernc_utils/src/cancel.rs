use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

#[derive(Debug, Clone)]
pub struct CancellationToken {
    canceled: Arc<AtomicBool>,
    check_budget: Option<Arc<AtomicUsize>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Canceled;

impl CancellationToken {
    pub fn new() -> Self {
        Self {
            canceled: Arc::new(AtomicBool::new(false)),
            check_budget: None,
        }
    }

    pub fn from_shared(canceled: Arc<AtomicBool>) -> Self {
        Self {
            canceled,
            check_budget: None,
        }
    }

    #[doc(hidden)]
    pub fn with_check_budget_for_testing(successful_checks: usize) -> Self {
        Self {
            canceled: Arc::new(AtomicBool::new(false)),
            check_budget: Some(Arc::new(AtomicUsize::new(successful_checks))),
        }
    }

    pub fn cancel(&self) {
        self.canceled.store(true, Ordering::SeqCst);
    }

    pub fn is_canceled(&self) -> bool {
        self.canceled.load(Ordering::SeqCst)
    }

    pub fn check(&self) -> Result<(), Canceled> {
        if self.is_canceled() || !self.consume_check_budget() {
            Err(Canceled)
        } else {
            Ok(())
        }
    }

    fn consume_check_budget(&self) -> bool {
        let Some(budget) = &self.check_budget else {
            return true;
        };

        loop {
            let remaining = budget.load(Ordering::SeqCst);
            if remaining == 0 {
                self.cancel();
                return false;
            }
            if budget
                .compare_exchange(remaining, remaining - 1, Ordering::SeqCst, Ordering::SeqCst)
                .is_ok()
            {
                return true;
            }
        }
    }
}

impl Default for CancellationToken {
    fn default() -> Self {
        Self::new()
    }
}
