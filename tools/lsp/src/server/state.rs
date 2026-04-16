use super::lifecycle::TraceValue;
use crate::analysis::{AnalysisEngine, AnalysisOutcome};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};

pub(super) struct ServerState {
    pub(super) initialized: bool,
    pub(super) shutdown_requested: bool,
    pub(super) trace: TraceValue,
    pub(super) diagnostics_flush_policy: DiagnosticsFlushPolicy,
    pub(super) analysis: AnalysisEngine,
    pub(super) next_analysis_generation: u64,
    pub(super) latest_generation_by_target: BTreeMap<String, AnalysisGeneration>,
    pub(super) canceled_request_ids: Vec<Value>,
    pub(super) pending_diagnostics_targets: BTreeSet<String>,
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum SchedulerLane {
    Interactive,
    Diagnostics,
}

pub(super) struct ScheduledDiagnosticsPublish {
    pub(super) generation: AnalysisGeneration,
    pub(super) outcome: AnalysisOutcome,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct DiagnosticsFlushPolicy {
    pub(super) target_task_budget: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum SchedulerDrainDecision {
    Drain,
    Defer,
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
            analysis,
            next_analysis_generation: 0,
            latest_generation_by_target: BTreeMap::new(),
            canceled_request_ids: Vec::new(),
            pending_diagnostics_targets: BTreeSet::new(),
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

    pub(super) fn begin_workspace_refresh(&mut self) -> BTreeMap<String, AnalysisGeneration> {
        self.analysis
            .document_uris()
            .into_iter()
            .map(|target_uri| {
                let generation = self.begin_target_analysis(&target_uri);
                (target_uri, generation)
            })
            .collect()
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

    pub(super) fn queue_target_diagnostics_task(&mut self, target_uri: String) {
        if self.pending_workspace_refresh_reason.is_some() {
            return;
        }
        self.pending_diagnostics_targets.insert(target_uri);
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
