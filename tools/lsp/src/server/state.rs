use super::lifecycle::TraceValue;
use crate::analysis::{AnalysisEngine, AnalysisOutcome, AnalysisTier};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
    mpsc,
};
use std::thread;

const DEFAULT_WORKER_COUNT: usize = 4;

pub(super) struct ServerState {
    pub(super) initialized: bool,
    pub(super) shutdown_requested: bool,
    pub(super) trace: TraceValue,
    pub(super) diagnostics_flush_policy: DiagnosticsFlushPolicy,
    pub(super) request_budget_policy: RequestBudgetPolicy,
    pub(super) analysis: AnalysisEngine,
    pub(super) next_analysis_generation: u64,
    pub(super) latest_generation_by_target: BTreeMap<String, AnalysisGeneration>,
    pub(super) canceled_request_ids: Vec<Value>,
    active_request_cancellations: Vec<ActiveRequestCancellation>,
    pub(super) pending_diagnostics_targets: BTreeMap<String, ScheduledDiagnosticsTask>,
    pub(super) pending_workspace_refresh_reason: Option<String>,
    pub(super) pending_diagnostics: BTreeMap<String, ScheduledDiagnosticsPublish>,
    pub(super) document_request_results_rx: mpsc::Receiver<DocumentRequestTaskResult>,
    pub(super) diagnostics_results_rx: mpsc::Receiver<DiagnosticsTaskResult>,
    pub(super) workspace_refresh_results_rx: mpsc::Receiver<WorkspaceRefreshTaskResult>,
    pub(super) worker_task_tx: mpsc::SyncSender<LspWorkerTask>,
    pub(super) pending_document_request_tasks: usize,
    pub(super) pending_diagnostics_worker_tasks: usize,
    pub(super) pending_workspace_refresh_tasks: usize,
    pub(super) published_by_target: BTreeMap<String, BTreeSet<String>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(super) struct AnalysisGeneration(pub(super) u64);

#[derive(Debug, Clone)]
pub(super) struct RequestContext {
    pub(super) id: Value,
    pub(super) target_uri: Option<String>,
    pub(super) generation: Option<AnalysisGeneration>,
    pub(super) cancellation: Option<CancellationToken>,
}

#[derive(Debug, Clone)]
pub(super) struct CancellationToken {
    canceled: Arc<AtomicBool>,
}

#[derive(Debug)]
struct ActiveRequestCancellation {
    id: Value,
    token: CancellationToken,
}

impl CancellationToken {
    fn new() -> Self {
        Self {
            canceled: Arc::new(AtomicBool::new(false)),
        }
    }

    pub(super) fn cancel(&self) {
        self.canceled.store(true, Ordering::SeqCst);
    }

    pub(super) fn is_canceled(&self) -> bool {
        self.canceled.load(Ordering::SeqCst)
    }
}

impl RequestContext {
    pub(super) fn is_canceled(&self) -> bool {
        self.cancellation
            .as_ref()
            .is_some_and(CancellationToken::is_canceled)
    }
}

#[derive(Debug)]
pub(super) struct DocumentRequestTaskResult {
    pub(super) request: RequestContext,
    pub(super) target_uri: String,
    pub(super) lane: SchedulerLane,
    pub(super) method: String,
    pub(super) queue_wait_ms: u128,
    pub(super) elapsed_ms: u128,
    pub(super) analysis_tier: Option<AnalysisTier>,
    pub(super) canceled: bool,
    pub(super) response: DocumentRequestResponse,
}

#[derive(Debug)]
pub(super) struct ScheduledDocumentRequestTask {
    pub(super) request: RequestContext,
    pub(super) target_uri: String,
    pub(super) lane: SchedulerLane,
    pub(super) method: String,
    pub(super) queued_at: std::time::Instant,
}

pub(super) struct DiagnosticsTaskResult {
    pub(super) target_uri: String,
    pub(super) generation: AnalysisGeneration,
    pub(super) mode: DiagnosticsAnalysisMode,
    pub(super) queue_wait_ms: u128,
    pub(super) elapsed_ms: u128,
    pub(super) analysis_tier: Option<AnalysisTier>,
    pub(super) outcome: AnalysisOutcome,
}

pub(super) struct WorkspaceRefreshTaskResult {
    pub(super) reason: String,
    pub(super) queue_wait_ms: u128,
    pub(super) elapsed_ms: u128,
    pub(super) targets: Result<Vec<(String, DiagnosticsAnalysisMode)>, String>,
}

pub(super) enum LspWorkerTask {
    DocumentRequest(Box<dyn FnOnce() -> DocumentRequestTaskResult + Send + 'static>),
    Diagnostics(Box<dyn FnOnce() -> DiagnosticsTaskResult + Send + 'static>),
    WorkspaceRefresh(Box<dyn FnOnce() -> WorkspaceRefreshTaskResult + Send + 'static>),
}

#[derive(Debug)]
pub(super) enum DocumentRequestResponse {
    Success(Value),
    Null,
    Error { code: i64, message: String },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum SchedulerLane {
    Interactive,
    Diagnostics,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum RequestBudgetKind {
    Interactive,
    Diagnostics,
    WorkspaceRefresh,
}

pub(super) struct ScheduledDiagnosticsPublish {
    pub(super) generation: AnalysisGeneration,
    pub(super) outcome: AnalysisOutcome,
}

pub(super) struct ScheduledDiagnosticsTask {
    pub(super) generation: AnalysisGeneration,
    pub(super) mode: DiagnosticsAnalysisMode,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum DiagnosticsAnalysisMode {
    Structure,
    Full,
}

impl DiagnosticsAnalysisMode {
    fn merge(self, other: Self) -> Self {
        match (self, other) {
            (Self::Full, _) | (_, Self::Full) => Self::Full,
            _ => Self::Structure,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct DiagnosticsFlushPolicy {
    pub(super) target_task_budget: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct RequestBudgetPolicy {
    pub(super) interactive_ms: u128,
    pub(super) diagnostics_ms: u128,
    pub(super) workspace_refresh_ms: u128,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum SchedulerDrainDecision {
    Drain,
    Defer,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum RequestBudgetStatus {
    Ok,
    Exceeded,
}

impl DiagnosticsFlushPolicy {
    pub(super) fn new() -> Self {
        Self {
            target_task_budget: 2,
        }
    }

    pub(super) fn decide_after_message(self, method: &str) -> SchedulerDrainDecision {
        match method {
            "textDocument/didOpen" | "textDocument/didClose" | "textDocument/didSave" => {
                SchedulerDrainDecision::Drain
            }
            "textDocument/didChange"
            | "workspace/didChangeConfiguration"
            | "workspace/didChangeWatchedFiles" => SchedulerDrainDecision::Defer,
            _ => SchedulerDrainDecision::Defer,
        }
    }

    pub(super) fn should_force_drain_after_message(
        self,
        method: &str,
        state: &ServerState,
    ) -> bool {
        if !matches!(
            method,
            "textDocument/didChange"
                | "workspace/didChangeConfiguration"
                | "workspace/didChangeWatchedFiles"
        ) {
            return false;
        }
        state.pending_workspace_refresh_reason.is_some()
            || state.pending_diagnostics_targets.len() >= self.target_task_budget
    }
}

impl RequestBudgetPolicy {
    pub(super) fn new() -> Self {
        Self {
            interactive_ms: 100,
            diagnostics_ms: 500,
            workspace_refresh_ms: 2_000,
        }
    }

    pub(super) fn budget_ms(self, kind: RequestBudgetKind) -> u128 {
        match kind {
            RequestBudgetKind::Interactive => self.interactive_ms,
            RequestBudgetKind::Diagnostics => self.diagnostics_ms,
            RequestBudgetKind::WorkspaceRefresh => self.workspace_refresh_ms,
        }
    }

    pub(super) fn status(self, kind: RequestBudgetKind, elapsed_ms: u128) -> RequestBudgetStatus {
        if elapsed_ms >= self.budget_ms(kind) {
            RequestBudgetStatus::Exceeded
        } else {
            RequestBudgetStatus::Ok
        }
    }
}

impl RequestBudgetStatus {
    pub(super) fn as_str(self) -> &'static str {
        match self {
            Self::Ok => "ok",
            Self::Exceeded => "exceeded",
        }
    }
}

impl ServerState {
    #[cfg(test)]
    pub(super) fn new() -> Self {
        Self::with_analysis(AnalysisEngine::default())
    }

    pub(super) fn with_analysis(analysis: AnalysisEngine) -> Self {
        let (document_request_results_tx, document_request_results_rx) = mpsc::channel();
        let (diagnostics_results_tx, diagnostics_results_rx) = mpsc::channel();
        let (workspace_refresh_results_tx, workspace_refresh_results_rx) = mpsc::channel();
        let worker_task_tx = spawn_worker_threads(
            DEFAULT_WORKER_COUNT,
            document_request_results_tx,
            diagnostics_results_tx,
            workspace_refresh_results_tx,
        );
        Self {
            initialized: false,
            shutdown_requested: false,
            trace: TraceValue::Off,
            diagnostics_flush_policy: DiagnosticsFlushPolicy::new(),
            request_budget_policy: RequestBudgetPolicy::new(),
            analysis,
            next_analysis_generation: 0,
            latest_generation_by_target: BTreeMap::new(),
            canceled_request_ids: Vec::new(),
            active_request_cancellations: Vec::new(),
            pending_diagnostics_targets: BTreeMap::new(),
            pending_workspace_refresh_reason: None,
            pending_diagnostics: BTreeMap::new(),
            document_request_results_rx,
            diagnostics_results_rx,
            workspace_refresh_results_rx,
            worker_task_tx,
            pending_document_request_tasks: 0,
            pending_diagnostics_worker_tasks: 0,
            pending_workspace_refresh_tasks: 0,
            published_by_target: BTreeMap::new(),
        }
    }

    pub(super) fn begin_target_analysis(&mut self, target_uri: &str) -> AnalysisGeneration {
        self.next_analysis_generation += 1;
        let generation = AnalysisGeneration(self.next_analysis_generation);
        self.latest_generation_by_target
            .insert(target_uri.to_string(), generation);
        generation
    }

    pub(super) fn is_current_generation(
        &self,
        target_uri: &str,
        generation: AnalysisGeneration,
    ) -> bool {
        self.latest_generation_by_target.get(target_uri).copied() == Some(generation)
    }

    pub(super) fn request_context(&self, id: Value) -> RequestContext {
        RequestContext {
            id,
            target_uri: None,
            generation: None,
            cancellation: None,
        }
    }

    pub(super) fn request_context_for_document(
        &self,
        id: Value,
        target_uri: &str,
    ) -> RequestContext {
        RequestContext {
            id,
            target_uri: Some(target_uri.to_string()),
            generation: self.latest_generation_by_target.get(target_uri).copied(),
            cancellation: None,
        }
    }

    pub(super) fn cancel_request(&mut self, id: Value) {
        let mut canceled_active = false;
        for active in &self.active_request_cancellations {
            if active.id == id {
                active.token.cancel();
                canceled_active = true;
            }
        }
        if canceled_active {
            return;
        }
        if self
            .canceled_request_ids
            .iter()
            .any(|canceled| canceled == &id)
        {
            return;
        }
        self.canceled_request_ids.push(id);
    }

    pub(super) fn register_request_cancellation(&mut self, request: &mut RequestContext) {
        if request.cancellation.is_some() {
            return;
        }
        let token = CancellationToken::new();
        if self
            .canceled_request_ids
            .iter()
            .any(|canceled| canceled == &request.id)
        {
            token.cancel();
        }
        self.active_request_cancellations
            .push(ActiveRequestCancellation {
                id: request.id.clone(),
                token: token.clone(),
            });
        request.cancellation = Some(token);
    }

    pub(super) fn finish_request_cancellation(&mut self, id: &Value) {
        self.active_request_cancellations
            .retain(|active| &active.id != id);
    }

    pub(super) fn should_skip_request(&mut self, request: &RequestContext) -> bool {
        self.should_drop_response(request)
    }

    pub(super) fn should_drop_response(&mut self, request: &RequestContext) -> bool {
        let was_canceled = request.is_canceled();
        let had_pending_cancel = if let Some(index) = self
            .canceled_request_ids
            .iter()
            .position(|canceled| canceled == &request.id)
        {
            self.canceled_request_ids.swap_remove(index);
            true
        } else {
            false
        };

        let is_stale = match (&request.target_uri, request.generation) {
            (Some(target_uri), Some(generation)) => {
                !self.is_current_generation(target_uri, generation)
            }
            _ => false,
        };
        self.finish_request_cancellation(&request.id);
        was_canceled || had_pending_cancel || is_stale
    }

    pub(super) fn queue_diagnostics_publish(
        &mut self,
        target_uri: String,
        generation: AnalysisGeneration,
        outcome: AnalysisOutcome,
    ) {
        self.pending_diagnostics.insert(
            target_uri,
            ScheduledDiagnosticsPublish {
                generation,
                outcome,
            },
        );
    }

    pub(super) fn queue_target_diagnostics_task(
        &mut self,
        target_uri: String,
        generation: AnalysisGeneration,
        mode: DiagnosticsAnalysisMode,
    ) {
        if self.pending_workspace_refresh_reason.is_some()
            || !self.is_current_generation(&target_uri, generation)
        {
            return;
        }
        self.pending_diagnostics_targets
            .entry(target_uri)
            .and_modify(|existing| {
                existing.generation = generation;
                existing.mode = existing.mode.merge(mode);
            })
            .or_insert(ScheduledDiagnosticsTask { generation, mode });
    }

    pub(super) fn queue_workspace_refresh_task(&mut self, reason: String) {
        self.pending_workspace_refresh_reason = Some(reason);
        self.pending_diagnostics_targets.clear();
    }

    pub(super) fn has_pending_diagnostics_work(&self) -> bool {
        !self.pending_diagnostics_targets.is_empty()
            || self.pending_workspace_refresh_reason.is_some()
            || !self.pending_diagnostics.is_empty()
            || self.pending_diagnostics_worker_tasks > 0
            || self.pending_workspace_refresh_tasks > 0
    }

    pub(super) fn should_drain_scheduler_after(&self, method: &str) -> bool {
        (self.has_pending_diagnostics_work() || self.has_pending_document_request_work())
            && (self.diagnostics_flush_policy.decide_after_message(method)
                == SchedulerDrainDecision::Drain
                || self
                    .diagnostics_flush_policy
                    .should_force_drain_after_message(method, self))
    }

    pub(super) fn queue_document_request_task(&mut self) {
        self.pending_document_request_tasks += 1;
    }

    pub(super) fn complete_document_request_task(&mut self) {
        self.pending_document_request_tasks = self.pending_document_request_tasks.saturating_sub(1);
    }

    pub(super) fn has_pending_document_request_work(&self) -> bool {
        self.pending_document_request_tasks > 0
    }

    pub(super) fn has_pending_worker_work(&self) -> bool {
        self.has_pending_document_request_work()
            || self.pending_workspace_refresh_reason.is_some()
            || !self.pending_diagnostics_targets.is_empty()
            || self.pending_diagnostics_worker_tasks > 0
            || self.pending_workspace_refresh_tasks > 0
            || !self.pending_diagnostics.is_empty()
    }

    pub(super) fn queue_diagnostics_worker_task(&mut self) {
        self.pending_diagnostics_worker_tasks += 1;
    }

    pub(super) fn complete_diagnostics_worker_task(&mut self) {
        self.pending_diagnostics_worker_tasks =
            self.pending_diagnostics_worker_tasks.saturating_sub(1);
    }

    pub(super) fn queue_workspace_refresh_worker_task(&mut self) {
        self.pending_workspace_refresh_tasks += 1;
    }

    pub(super) fn complete_workspace_refresh_worker_task(&mut self) {
        self.pending_workspace_refresh_tasks =
            self.pending_workspace_refresh_tasks.saturating_sub(1);
    }
}

fn spawn_worker_threads(
    worker_count: usize,
    document_request_results_tx: mpsc::Sender<DocumentRequestTaskResult>,
    diagnostics_results_tx: mpsc::Sender<DiagnosticsTaskResult>,
    workspace_refresh_results_tx: mpsc::Sender<WorkspaceRefreshTaskResult>,
) -> mpsc::SyncSender<LspWorkerTask> {
    let (task_tx, task_rx) = mpsc::sync_channel::<LspWorkerTask>(worker_count.max(1) * 2);
    let task_rx = std::sync::Arc::new(std::sync::Mutex::new(task_rx));
    for _ in 0..worker_count.max(1) {
        let task_rx = task_rx.clone();
        let document_request_results_tx = document_request_results_tx.clone();
        let diagnostics_results_tx = diagnostics_results_tx.clone();
        let workspace_refresh_results_tx = workspace_refresh_results_tx.clone();
        thread::spawn(move || {
            loop {
                let task = {
                    let task_rx = task_rx.lock().unwrap();
                    task_rx.recv()
                };
                let Ok(task) = task else {
                    break;
                };
                match task {
                    LspWorkerTask::DocumentRequest(task) => {
                        let result = task();
                        let _ = document_request_results_tx.send(result);
                    }
                    LspWorkerTask::Diagnostics(task) => {
                        let result = task();
                        let _ = diagnostics_results_tx.send(result);
                    }
                    LspWorkerTask::WorkspaceRefresh(task) => {
                        let result = task();
                        let _ = workspace_refresh_results_tx.send(result);
                    }
                }
            }
        });
    }
    task_tx
}
