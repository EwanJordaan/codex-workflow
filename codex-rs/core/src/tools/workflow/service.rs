use std::collections::BTreeMap;
use std::sync::atomic::AtomicU64;
use std::sync::atomic::Ordering;
use std::time::Instant;

use codex_code_mode::CellId;
use serde::Serialize;
use tokio::sync::Mutex;

use crate::tools::code_mode::CodeCellStatus;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum WorkflowRunStatus {
    Running,
    Completed,
    Failed,
    Terminated,
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
    started_at: Instant,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub(crate) struct WorkflowRunSummary {
    pub(crate) id: String,
    pub(crate) name: String,
    pub(crate) cell_id: String,
    pub(crate) status: WorkflowRunStatus,
    pub(crate) elapsed_ms: u64,
}

#[derive(Default)]
pub(crate) struct WorkflowService {
    next_id: AtomicU64,
    runs: Mutex<BTreeMap<String, WorkflowRun>>,
}

impl WorkflowService {
    pub(crate) async fn register(
        &self,
        name: String,
        cell_id: CellId,
        status: CodeCellStatus,
    ) -> WorkflowRunSummary {
        let sequence = self.next_id.fetch_add(1, Ordering::Relaxed) + 1;
        let id = format!("workflow-{sequence}");
        let run = WorkflowRun {
            id: id.clone(),
            name,
            cell_id,
            status: status.into(),
            started_at: Instant::now(),
        };
        let summary = summary(&run);
        self.runs.lock().await.insert(id, run);
        summary
    }

    pub(crate) async fn list(&self) -> Vec<WorkflowRunSummary> {
        self.runs.lock().await.values().rev().map(summary).collect()
    }

    pub(crate) async fn update_cell(&self, cell_id: &CellId, status: CodeCellStatus) {
        if let Some(run) = self
            .runs
            .lock()
            .await
            .values_mut()
            .find(|run| &run.cell_id == cell_id)
        {
            run.status = status.into();
        }
    }

    pub(crate) async fn cell_for_run(&self, run_id: &str) -> Option<CellId> {
        self.runs
            .lock()
            .await
            .get(run_id)
            .map(|run| run.cell_id.clone())
    }
}

fn summary(run: &WorkflowRun) -> WorkflowRunSummary {
    WorkflowRunSummary {
        id: run.id.clone(),
        name: run.name.clone(),
        cell_id: run.cell_id.to_string(),
        status: run.status,
        elapsed_ms: u64::try_from(run.started_at.elapsed().as_millis()).unwrap_or(u64::MAX),
    }
}

#[cfg(test)]
#[path = "service_tests.rs"]
mod tests;
