use pretty_assertions::assert_eq;
use std::time::Instant;

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
            "turn-1".to_string(),
            Instant::now(),
            /*terminal_response*/ None,
        )
        .await;
    assert!(started.id.starts_with("workflow-"));
    assert_eq!(started.status, WorkflowRunStatus::Running);

    service
        .update_cell(&cell_id, CodeCellStatus::Completed, /*response*/ None)
        .await;
    let runs = service.list(DEFAULT_WORKFLOW_LIST_LIMIT).await;
    assert_eq!(runs.len(), 1);
    assert_eq!(runs[0].status, WorkflowRunStatus::Completed);
    assert_eq!(
        service.run(&started.id).await.map(|run| run.cell_id),
        Some(cell_id)
    );
}

#[tokio::test]
async fn registry_lists_newest_runs_in_insertion_order() {
    let service = WorkflowService::default();
    for index in 0..12 {
        service
            .register(
                format!("run-{index}"),
                CellId::new(format!("cell-{index}")),
                CodeCellStatus::Completed,
                "turn-1".to_string(),
                Instant::now(),
                /*terminal_response*/ None,
            )
            .await;
    }

    assert_eq!(
        service
            .list(/*limit*/ 3)
            .await
            .into_iter()
            .map(|run| run.name)
            .collect::<Vec<_>>(),
        vec!["run-11", "run-10", "run-9"]
    );
}
