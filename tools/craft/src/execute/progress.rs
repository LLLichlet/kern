use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU8, AtomicUsize, Ordering};
use std::sync::mpsc;
use std::thread;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

const LONG_ACTION_REPORT_DELAY: Duration = Duration::from_secs(15);
const LONG_ACTION_REPORT_INTERVAL: Duration = Duration::from_secs(30);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ExecutionProgressPlan {
    pub staged_actions: usize,
    pub compile_actions: usize,
    pub link_actions: usize,
}

impl ExecutionProgressPlan {
    pub fn total_steps(self) -> usize {
        self.staged_actions + self.compile_actions + self.link_actions
    }

    pub fn is_empty(self) -> bool {
        self.total_steps() == 0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ExecutionPhase {
    #[default]
    Bootstrap,
    Stage,
    Compile,
    Link,
}

impl ExecutionPhase {
    fn encode(self) -> u8 {
        match self {
            Self::Bootstrap => 0,
            Self::Stage => 1,
            Self::Compile => 2,
            Self::Link => 3,
        }
    }

    fn decode(value: u8) -> Self {
        match value {
            1 => Self::Stage,
            2 => Self::Compile,
            3 => Self::Link,
            _ => Self::Bootstrap,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExecutionProgressSnapshot {
    pub phase: ExecutionPhase,
    pub plan: ExecutionProgressPlan,
    pub staged_done: usize,
    pub compile_done: usize,
    pub link_done: usize,
    pub elapsed: Duration,
    pub detail: String,
}

impl ExecutionProgressSnapshot {
    pub fn completed_steps(&self) -> usize {
        self.staged_done.min(self.plan.staged_actions)
            + self.compile_done.min(self.plan.compile_actions)
            + self.link_done.min(self.plan.link_actions)
    }

    pub fn total_steps(&self) -> usize {
        self.plan.total_steps()
    }
}

#[derive(Debug, Clone)]
pub struct ProgressReporter {
    state: Arc<ProgressState>,
}

#[derive(Debug)]
pub(crate) struct ProgressSuspendGuard {
    state: Arc<ProgressState>,
}

#[derive(Debug)]
pub(crate) struct LongActionReport {
    stop: Option<mpsc::Sender<()>>,
    worker: Option<JoinHandle<()>>,
}

#[derive(Debug)]
struct ProgressState {
    plan: ExecutionProgressPlan,
    phase: AtomicU8,
    staged_done: AtomicUsize,
    compile_done: AtomicUsize,
    link_done: AtomicUsize,
    suspended: AtomicUsize,
    started_at: Instant,
    detail: Mutex<String>,
}

impl ProgressReporter {
    pub fn new(plan: ExecutionProgressPlan) -> Self {
        Self {
            state: Arc::new(ProgressState {
                plan,
                phase: AtomicU8::new(ExecutionPhase::Bootstrap.encode()),
                staged_done: AtomicUsize::new(0),
                compile_done: AtomicUsize::new(0),
                link_done: AtomicUsize::new(0),
                suspended: AtomicUsize::new(0),
                started_at: Instant::now(),
                detail: Mutex::new(String::new()),
            }),
        }
    }

    pub fn snapshot(&self) -> ExecutionProgressSnapshot {
        ExecutionProgressSnapshot {
            phase: ExecutionPhase::decode(self.state.phase.load(Ordering::Relaxed)),
            plan: self.state.plan,
            staged_done: self.state.staged_done.load(Ordering::Relaxed),
            compile_done: self.state.compile_done.load(Ordering::Relaxed),
            link_done: self.state.link_done.load(Ordering::Relaxed),
            elapsed: self.state.started_at.elapsed(),
            detail: self.state.detail.lock().unwrap().clone(),
        }
    }

    pub(crate) fn set_phase(&self, phase: ExecutionPhase) {
        self.state.phase.store(phase.encode(), Ordering::Relaxed);
    }

    pub(crate) fn record_staged_action(&self) {
        self.state.staged_done.fetch_add(1, Ordering::Relaxed);
    }

    pub(crate) fn record_compile_action(&self) {
        self.state.compile_done.fetch_add(1, Ordering::Relaxed);
    }

    pub(crate) fn record_link_action(&self) {
        self.state.link_done.fetch_add(1, Ordering::Relaxed);
    }

    pub(crate) fn set_detail(&self, detail: impl Into<String>) {
        *self.state.detail.lock().unwrap() = detail.into();
    }

    pub(crate) fn suspend_terminal(&self) -> ProgressSuspendGuard {
        self.state.suspended.fetch_add(1, Ordering::Relaxed);
        ProgressSuspendGuard {
            state: self.state.clone(),
        }
    }

    pub(crate) fn terminal_suspended(&self) -> bool {
        self.state.suspended.load(Ordering::Relaxed) != 0
    }

    pub(crate) fn report_long_action(
        &self,
        verb: &'static str,
        detail: impl Into<String>,
    ) -> LongActionReport {
        LongActionReport::spawn(verb, detail.into())
    }
}

impl Drop for ProgressSuspendGuard {
    fn drop(&mut self) {
        self.state.suspended.fetch_sub(1, Ordering::Relaxed);
    }
}

impl LongActionReport {
    fn spawn(verb: &'static str, detail: String) -> Self {
        let (tx, rx) = mpsc::channel();
        let started = Instant::now();
        let worker = thread::spawn(move || {
            if rx.recv_timeout(LONG_ACTION_REPORT_DELAY).is_ok() {
                return;
            }
            loop {
                let elapsed = started.elapsed();
                eprintln!("craft: still {verb} after {}s: {detail}", elapsed.as_secs());
                if rx.recv_timeout(LONG_ACTION_REPORT_INTERVAL).is_ok() {
                    return;
                }
            }
        });

        Self {
            stop: Some(tx),
            worker: Some(worker),
        }
    }
}

impl Drop for LongActionReport {
    fn drop(&mut self) {
        if let Some(stop) = self.stop.take() {
            let _ = stop.send(());
        }
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}
