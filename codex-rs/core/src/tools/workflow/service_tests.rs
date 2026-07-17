use pretty_assertions::assert_eq;

use super::*;

#[tokio::test]
async fn registry_tracks_and_updates_workflow_runs() {
    let service = WorkflowService::default();
    let cell_id = CellId::new("cell-1".to_string());

    let started = service
        .register(
            "audit".to_string(),
            cell_id.clone(),
            CodeCellStatus::Running,
        )
        .await;
    assert_eq!(started.id, "workflow-1");
    assert_eq!(started.status, WorkflowRunStatus::Running);

    service
        .update_cell(&cell_id, CodeCellStatus::Completed)
        .await;
    let runs = service.list().await;
    assert_eq!(runs.len(), 1);
    assert_eq!(runs[0].status, WorkflowRunStatus::Completed);
    assert_eq!(service.cell_for_run("workflow-1").await, Some(cell_id));
}
