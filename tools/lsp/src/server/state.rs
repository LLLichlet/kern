use super::lifecycle::TraceValue;
use crate::analysis::{AnalysisEngine, AnalysisOutcome};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};

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
    pub(super) pending_diagnostics_targets: BTreeMap<String, ScheduledDiagnosticsTask>,
    pub(super) pending_workspace_refresh_reason: Option<String>,
    pub(super) pending_diagnostics: BTreeMap<String, ScheduledDiagnosticsPublish>,
    pub(super) published_by_target: BTreeMap<String, BTreeSet<String>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(super) struct AnalysisGeneration(pub(super) u64);

#[derive(Debug, Clone)]
pub(super) struct RequestContext {
    pub(super) id: Value,
    pub(super) target_uri: Option<String>,
    pub(super) generation: Option<AnalysisGeneration>,
}

#[derive(Debug)]
pub(super) struct DocumentRequestTaskResult {
    pub(super) request: RequestContext,
    pub(super) target_uri: String,
    pub(super) lane: SchedulerLane,
    pub(super) method: String,
    pub(super) elapsed_ms: u128,
    pub(super) response: DocumentRequestResponse,
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

    pub(super) fn should_force_drain_for_pending_work(self, state: &ServerState) -> bool {
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
            pending_diagnostics_targets: BTreeMap::new(),
            pending_workspace_refresh_reason: None,
            pending_diagnostics: BTreeMap::new(),
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
        }
    }

    pub(super) fn cancel_request(&mut self, id: Value) {
        if self
            .canceled_request_ids
            .iter()
            .any(|canceled| canceled == &id)
        {
            return;
        }
        self.canceled_request_ids.push(id);
    }

    pub(super) fn should_skip_request(&mut self, request: &RequestContext) -> bool {
        self.should_drop_response(request)
    }

    pub(super) fn should_drop_response(&mut self, request: &RequestContext) -> bool {
        if let Some(index) = self
            .canceled_request_ids
            .iter()
            .position(|canceled| canceled == &request.id)
        {
            self.canceled_request_ids.swap_remove(index);
            return true;
        }

        match (&request.target_uri, request.generation) {
            (Some(target_uri), Some(generation)) => {
                !self.is_current_generation(target_uri, generation)
            }
            _ => false,
        }
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
    }

    pub(super) fn should_drain_scheduler_after(&self, method: &str) -> bool {
        self.has_pending_diagnostics_work()
            && (self.diagnostics_flush_policy.decide_after_message(method)
                == SchedulerDrainDecision::Drain
                || self
                    .diagnostics_flush_policy
                    .should_force_drain_for_pending_work(self))
    }
}
