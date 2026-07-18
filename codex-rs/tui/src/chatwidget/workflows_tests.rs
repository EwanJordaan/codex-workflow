use std::fs;

use pretty_assertions::assert_eq;
use tempfile::TempDir;

use crate::chatwidget::tests::helpers::make_chatwidget_manual;
use crate::chatwidget::tests::helpers::render_bottom_popup;

use super::SavedWorkflow;
use super::discover_saved_workflows;
use super::saved_workflow_prompt;

#[test]
fn saved_workflow_prompt_preserves_user_input_as_structured_args() {
    assert_eq!(
        saved_workflow_prompt(
            ".codex/workflows/audit.ts".as_ref(),
            Some("src/routes --strict"),
        ),
        concat!(
            "Run the saved workflow at `.codex/workflows/audit.ts` with `run_workflow` and ",
            "wait for it to complete. Pass this user input as structured workflow args: ",
            "src/routes --strict"
        )
    );
}

#[test]
fn discovers_javascript_workflows_in_name_order() {
    let project = TempDir::new().expect("create project");
    let workflow_dir = project.path().join(".codex/workflows");
    fs::create_dir_all(&workflow_dir).expect("create workflow directory");
    fs::write(workflow_dir.join("zeta.js"), "return 1;").expect("write workflow");
    fs::write(workflow_dir.join("alpha.ts"), "return 2;").expect("write workflow");
    fs::write(workflow_dir.join("ignored.md"), "ignored").expect("write ignored file");

    assert_eq!(
        discover_saved_workflows(project.path(), project.path()),
        vec![
            SavedWorkflow {
                name: "alpha".to_string(),
                path: ".codex/workflows/alpha.ts".into(),
            },
            SavedWorkflow {
                name: "zeta".to_string(),
                path: ".codex/workflows/zeta.js".into(),
            },
        ]
    );
}

#[test]
fn missing_workflow_directory_is_empty() {
    let project = TempDir::new().expect("create project");

    assert_eq!(
        discover_saved_workflows(project.path(), project.path()),
        Vec::new()
    );
}

#[test]
fn discovers_nested_ancestor_and_personal_scopes_with_nearest_precedence() {
    let project = TempDir::new().expect("create project");
    let personal = TempDir::new().expect("create personal home");
    let child = project.path().join("child");
    let cwd = child.join("deep");
    fs::create_dir_all(&cwd).expect("create nested project");
    let project_workflows = project.path().join(".codex/workflows");
    let child_workflows = child.join(".codex/workflows");
    let personal_workflows = personal.path().join("workflows");
    fs::create_dir_all(project_workflows.join("nested")).expect("create project workflows");
    fs::create_dir_all(&child_workflows).expect("create child workflows");
    fs::create_dir_all(&personal_workflows).expect("create personal workflows");
    fs::write(project_workflows.join("shared.ts"), "return 'root';").expect("write root workflow");
    fs::write(project_workflows.join("nested/a.js"), "return 'nested';")
        .expect("write nested workflow");
    fs::write(child_workflows.join("shared.ts"), "return 'child';").expect("write child workflow");
    fs::write(personal_workflows.join("personal.ts"), "return 'personal';")
        .expect("write personal workflow");

    assert_eq!(
        discover_saved_workflows(&cwd, personal.path()),
        vec![
            SavedWorkflow {
                name: "nested/a".to_string(),
                path: project_workflows.join("nested/a.js"),
            },
            SavedWorkflow {
                name: "personal".to_string(),
                path: personal_workflows.join("personal.ts"),
            },
            SavedWorkflow {
                name: "shared".to_string(),
                path: child_workflows.join("shared.ts"),
            },
        ]
    );
}

#[tokio::test]
async fn workflows_picker_snapshot() {
    let project = TempDir::new().expect("create project");
    let workflow_dir = project.path().join(".codex/workflows");
    fs::create_dir_all(&workflow_dir).expect("create workflow directory");
    fs::write(workflow_dir.join("audit.ts"), "return [];").expect("write workflow");
    fs::write(workflow_dir.join("review.js"), "return [];").expect("write workflow");
    let (mut chat, _rx, _op_rx) = make_chatwidget_manual(/*model_override*/ None).await;
    chat.current_cwd = Some(project.path().to_path_buf());

    chat.open_workflows_menu();

    insta::assert_snapshot!("workflows_picker", render_bottom_popup(&chat, /*width*/ 80));
}
