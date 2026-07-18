use std::collections::HashMap;
use std::collections::VecDeque;
use std::sync::Arc;
use std::time::Duration;
use std::time::Instant;

use codex_code_mode::CellId;
use codex_protocol::ThreadId;
use codex_protocol::models::ResponseItem;
use codex_protocol::protocol::RolloutItem;
use serde::Deserialize;
use serde::Serialize;
use tokio::sync::Mutex;

use crate::tools::code_mode::CodeCellStatus;

pub(crate) const DEFAULT_WORKFLOW_LIST_LIMIT: usize = 20;
pub(crate) const MAX_WORKFLOW_LIST_LIMIT: usize = 50;
const MAX_RETAINED_WORKFLOW_RUNS: usize = 64;
const MAX_PENDING_WORKFLOW_CELLS: usize = 64;
const WORKFLOW_RUN_MARKER: &str = "codex_workflow_run_v1=";

#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum WorkflowRunStatus {
    Running,
    Completed,
    Failed,
    Terminated,
}

impl WorkflowRunStatus {
    pub(crate) fn is_terminal(self) -> bool {
        !matches!(self, Self::Running)
    }

    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Running => "running",
            Self::Completed => "completed",
            Self::Failed => "failed",
            Self::Terminated => "terminated",
        }
    }
}

impl From<CodeCellStatus> for WorkflowRunStatus {
    fn from(status: CodeCellStatus) -> Self {
        match status {
            CodeCellStatus::Running => Self::Running,
            CodeCellStatus::Completed => Self::Completed,
            CodeCellStatus::Failed => Self::Failed,
            CodeCellStatus::Terminated => Self::Terminated,
        }
    }
}

struct WorkflowRun {
    id: String,
    name: String,
    cell_id: CellId,
    status: WorkflowRunStatus,
    origin_sub_id: String,
    started_at: Instant,
    terminal_elapsed: Option<Duration>,
    terminal_response: Option<codex_code_mode::RuntimeResponse>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub(crate) struct WorkflowRunSummary {
    pub(crate) id: String,
    pub(crate) name: String,
    pub(crate) cell_id: String,
    pub(crate) status: WorkflowRunStatus,
    pub(crate) elapsed_ms: u64,
}

#[derive(Deserialize, Serialize)]
struct PersistedWorkflowRun {
    id: String,
    name: String,
    cell_id: String,
    status: WorkflowRunStatus,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct WorkflowRunHandle {
    pub(crate) cell_id: CellId,
    pub(crate) status: WorkflowRunStatus,
    pub(crate) origin_sub_id: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum WorkflowAgentVersion {
    V1,
    V2,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct WorkflowOwnedAgent {
    pub(crate) thread_id: ThreadId,
    pub(crate) version: WorkflowAgentVersion,
}

#[derive(Default)]
pub(crate) struct WorkflowService {
    runs: Mutex<VecDeque<WorkflowRun>>,
    owned_agents: Mutex<HashMap<CellId, Vec<WorkflowOwnedAgent>>>,
    wait_locks: Mutex<HashMap<CellId, Arc<Mutex<()>>>>,
}

impl WorkflowService {
    pub(crate) fn from_rollout_items(items: &[RolloutItem]) -> Self {
        let mut restored = VecDeque::new();
        for item in items.iter().rev() {
            let RolloutItem::ResponseItem(ResponseItem::FunctionCallOutput { output, .. }) = item
            else {
                continue;
            };
            let Some(text) = output.body.to_text() else {
                continue;
            };
            for line in text.lines().rev() {
                let Some(marker) = line.strip_prefix(WORKFLOW_RUN_MARKER) else {
                    continue;
                };
                let Ok(persisted) = serde_json::from_str::<PersistedWorkflowRun>(marker) else {
                    continue;
                };
                if restored
                    .iter()
                    .any(|run: &WorkflowRun| run.id == persisted.id)
                {
                    continue;
                }
                let status = if matches!(persisted.status, WorkflowRunStatus::Running) {
                    WorkflowRunStatus::Terminated
                } else {
                    persisted.status
                };
                restored.push_front(WorkflowRun {
                    id: persisted.id,
                    name: persisted.name,
                    cell_id: CellId::new(persisted.cell_id),
                    status,
                    origin_sub_id: String::new(),
                    started_at: Instant::now(),
                    terminal_elapsed: Some(Duration::ZERO),
                    terminal_response: None,
                });
                if restored.len() == MAX_RETAINED_WORKFLOW_RUNS {
                    break;
                }
            }
            if restored.len() == MAX_RETAINED_WORKFLOW_RUNS {
                break;
            }
        }
        Self {
            runs: Mutex::new(restored),
            owned_agents: Mutex::new(HashMap::new()),
            wait_locks: Mutex::new(HashMap::new()),
        }
    }

    pub(crate) async fn register(
        &self,
        name: String,
        cell_id: CellId,
        status: CodeCellStatus,
        origin_sub_id: String,
        started_at: Instant,
        terminal_response: Option<&codex_code_mode::RuntimeResponse>,
    ) -> WorkflowRunSummary {
        let status = WorkflowRunStatus::from(status);
        let terminal_elapsed = status.is_terminal().then(|| started_at.elapsed());
        let wait_cell_id = cell_id.clone();
        let run = WorkflowRun {
            id: format!("workflow-{}", uuid::Uuid::now_v7()),
            name,
            cell_id,
            status,
            origin_sub_id,
            started_at,
            terminal_elapsed,
            terminal_response: terminal_response.cloned(),
        };
        let summary = summary(&run);
        let removed_cells = {
            let mut runs = self.runs.lock().await;
            runs.push_back(run);
            let mut removed_cells = Vec::new();
            while runs.len() > MAX_RETAINED_WORKFLOW_RUNS {
                if let Some(removed) = runs.pop_front() {
                    removed_cells.push(removed.cell_id);
                }
            }
            removed_cells
        };
        {
            let mut owned_agents = self.owned_agents.lock().await;
            for cell_id in &removed_cells {
                owned_agents.remove(cell_id);
            }
        }
        let mut wait_locks = self.wait_locks.lock().await;
        for cell_id in removed_cells {
            wait_locks.remove(&cell_id);
        }
        wait_locks
            .entry(wait_cell_id)
            .or_insert_with(|| Arc::new(Mutex::new(())));
        summary
    }

    pub(crate) async fn list(&self, limit: usize) -> Vec<WorkflowRunSummary> {
        self.runs
            .lock()
            .await
            .iter()
            .rev()
            .take(limit.min(MAX_WORKFLOW_LIST_LIMIT))
            .map(summary)
            .collect()
    }

    pub(crate) fn persistence_marker(summary: &WorkflowRunSummary) -> Result<String, String> {
        let persisted = PersistedWorkflowRun {
            id: summary.id.clone(),
            name: summary.name.clone(),
            cell_id: summary.cell_id.clone(),
            status: summary.status,
        };
        serde_json::to_string(&persisted)
            .map(|json| format!("{WORKFLOW_RUN_MARKER}{json}"))
            .map_err(|err| format!("failed to serialize workflow run metadata: {err}"))
    }

    pub(crate) async fn update_cell(
        &self,
        cell_id: &CellId,
        status: CodeCellStatus,
        response: Option<&codex_code_mode::RuntimeResponse>,
    ) -> Option<WorkflowRunHandle> {
        let mut runs = self.runs.lock().await;
        let run = runs.iter_mut().find(|run| &run.cell_id == cell_id)?;
        let previous = run.status;
        run.status = status.into();
        if run.status.is_terminal() && run.terminal_elapsed.is_none() {
            run.terminal_elapsed = Some(run.started_at.elapsed());
        }
        if run.status.is_terminal()
            && let Some(response) = response
        {
            run.terminal_response = Some(response.clone());
        }
        (previous != run.status).then(|| handle(run))
    }

    pub(crate) async fn terminal_response(
        &self,
        cell_id: &CellId,
    ) -> Option<codex_code_mode::RuntimeResponse> {
        self.runs
            .lock()
            .await
            .iter()
            .find(|run| &run.cell_id == cell_id)
            .and_then(|run| run.terminal_response.clone())
    }

    pub(crate) async fn run(&self, run_id: &str) -> Option<WorkflowRunHandle> {
        self.runs
            .lock()
            .await
            .iter()
            .find(|run| run.id == run_id)
            .map(handle)
    }

    pub(crate) async fn track_agent(&self, cell_id: CellId, agent: WorkflowOwnedAgent) {
        let mut owned_agents = self.owned_agents.lock().await;
        if !owned_agents.contains_key(&cell_id)
            && owned_agents.len() >= MAX_PENDING_WORKFLOW_CELLS
            && let Some(oldest) = owned_agents.keys().next().cloned()
        {
            owned_agents.remove(&oldest);
        }
        let agents = owned_agents.entry(cell_id).or_default();
        if !agents.contains(&agent) {
            agents.push(agent);
        }
    }

    pub(crate) async fn untrack_agent(&self, cell_id: &CellId, thread_id: ThreadId) {
        let mut owned_agents = self.owned_agents.lock().await;
        if let Some(agents) = owned_agents.get_mut(cell_id) {
            agents.retain(|agent| agent.thread_id != thread_id);
            if agents.is_empty() {
                owned_agents.remove(cell_id);
            }
        }
    }

    pub(crate) async fn take_agents(&self, cell_id: &CellId) -> Vec<WorkflowOwnedAgent> {
        self.owned_agents
            .lock()
            .await
            .remove(cell_id)
            .unwrap_or_default()
    }

    pub(crate) async fn wait_lock(&self, cell_id: &CellId) -> Option<Arc<Mutex<()>>> {
        self.wait_locks.lock().await.get(cell_id).cloned()
    }
}

fn handle(run: &WorkflowRun) -> WorkflowRunHandle {
    WorkflowRunHandle {
        cell_id: run.cell_id.clone(),
        status: run.status,
        origin_sub_id: run.origin_sub_id.clone(),
    }
}

fn summary(run: &WorkflowRun) -> WorkflowRunSummary {
    let elapsed = run
        .terminal_elapsed
        .unwrap_or_else(|| run.started_at.elapsed());
    WorkflowRunSummary {
        id: run.id.clone(),
        name: run.name.clone(),
        cell_id: run.cell_id.to_string(),
        status: run.status,
        elapsed_ms: u64::try_from(elapsed.as_millis()).unwrap_or(u64::MAX),
    }
}

#[cfg(test)]
#[path = "service_tests.rs"]
mod tests;
