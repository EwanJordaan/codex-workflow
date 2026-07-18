use std::fs;

use codex_code_mode::ExecuteRequest;
use codex_code_mode::FunctionCallOutputContentItem;
use codex_code_mode::InProcessCodeModeSession;
use codex_code_mode::RuntimeResponse;
use codex_exec_server::LocalFileSystem;
use codex_utils_path_uri::PathUri;
use pretty_assertions::assert_eq;
use serde_json::json;
use tempfile::tempdir;

use super::source::compile_inline_workflow;
use super::source::load_and_compile_workflow;
use codex_code_mode::WorkflowAgentApi;
use codex_code_mode::compile_workflow_source;

const SIMPLE_WORKFLOW: &str = r#"
export const meta = {
  name: "simple",
  description: "Return structured input",
};

await Promise.resolve();
return { received: args };
"#;

fn agent_api() -> WorkflowAgentApi {
    WorkflowAgentApi::V1 {
        spawn_tool: "multi_agent_v1__spawn_agent".to_string(),
        wait_tool: "multi_agent_v1__wait_agent".to_string(),
        close_tool: "multi_agent_v1__close_agent".to_string(),
    }
}

#[test]
fn inline_workflow_enforces_size_limit() {
    assert!(compile_inline_workflow(SIMPLE_WORKFLOW, /*args*/ None, &agent_api()).is_ok());
    assert_eq!(
        compile_inline_workflow(&"x".repeat(4 * 1024 + 1), /*args*/ None, &agent_api()),
        Err("inline workflow source exceeds the 4096-byte limit; save larger workflows under .codex/workflows".to_string())
    );
}

#[tokio::test]
async fn loader_restricts_files_to_workflow_directory() {
    let root = tempdir().expect("tempdir");
    let cwd = PathUri::from_host_native_path(root.path()).expect("temporary path URI");
    let file_system = LocalFileSystem::unsandboxed();
    let workflow_dir = root.path().join(".codex/workflows");
    fs::create_dir_all(&workflow_dir).expect("create workflow directory");
    fs::write(workflow_dir.join("simple.ts"), SIMPLE_WORKFLOW).expect("write workflow");
    fs::write(root.path().join("outside.ts"), SIMPLE_WORKFLOW).expect("write outside file");

    assert!(
        load_and_compile_workflow(
            &file_system,
            &cwd,
            ".codex/workflows/simple.ts",
            /*args*/ None,
            &agent_api(),
            /*personal_workflow_root*/ None,
            /*sandbox*/ None,
        )
        .await
        .is_ok()
    );
    assert_eq!(
        load_and_compile_workflow(
            &file_system,
            &cwd,
            "outside.ts",
            /*args*/ None,
            &agent_api(),
            /*personal_workflow_root*/ None,
            /*sandbox*/ None
        )
        .await,
        Err("workflow path must stay below a project or personal workflows directory".to_string())
    );
    assert_eq!(
        load_and_compile_workflow(
            &file_system,
            &cwd,
            "../outside.ts",
            /*args*/ None,
            &agent_api(),
            /*personal_workflow_root*/ None,
            /*sandbox*/ None
        )
        .await,
        Err("workflow path cannot contain `..`".to_string())
    );
}

#[tokio::test]
async fn loader_enforces_extension_and_size_limits() {
    let root = tempdir().expect("tempdir");
    let cwd = PathUri::from_host_native_path(root.path()).expect("temporary path URI");
    let file_system = LocalFileSystem::unsandboxed();
    let workflow_dir = root.path().join(".codex/workflows");
    fs::create_dir_all(&workflow_dir).expect("create workflow directory");
    fs::write(workflow_dir.join("simple.txt"), SIMPLE_WORKFLOW).expect("write text workflow");
    fs::write(workflow_dir.join("large.ts"), "x".repeat(128 * 1024 + 1))
        .expect("write large workflow");

    assert_eq!(
        load_and_compile_workflow(
            &file_system,
            &cwd,
            ".codex/workflows/simple.txt",
            /*args*/ None,
            &agent_api(),
            /*personal_workflow_root*/ None,
            /*sandbox*/ None,
        )
        .await,
        Err("workflow path must end in .ts or .js".to_string())
    );
    assert_eq!(
        load_and_compile_workflow(
            &file_system,
            &cwd,
            ".codex/workflows/large.ts",
            /*args*/ None,
            &agent_api(),
            /*personal_workflow_root*/ None,
            /*sandbox*/ None,
        )
        .await,
        Err("workflow file exceeds the 131072-byte limit".to_string())
    );
}

#[tokio::test]
async fn loader_accepts_ancestor_and_personal_workflow_scopes() {
    let root = tempdir().expect("tempdir");
    let personal = tempdir().expect("personal tempdir");
    let cwd_path = root.path().join("project/child");
    let ancestor_workflows = root.path().join("project/.codex/workflows");
    let personal_workflows = personal.path().join("workflows");
    fs::create_dir_all(&cwd_path).expect("create child project");
    fs::create_dir_all(&ancestor_workflows).expect("create ancestor workflows");
    fs::create_dir_all(&personal_workflows).expect("create personal workflows");
    let ancestor_workflow = ancestor_workflows.join("ancestor.ts");
    let personal_workflow = personal_workflows.join("personal.ts");
    fs::write(&ancestor_workflow, SIMPLE_WORKFLOW).expect("write ancestor workflow");
    fs::write(&personal_workflow, SIMPLE_WORKFLOW).expect("write personal workflow");
    let cwd = PathUri::from_host_native_path(&cwd_path).expect("child path URI");
    let personal_root =
        PathUri::from_host_native_path(&personal_workflows).expect("personal path URI");
    let file_system = LocalFileSystem::unsandboxed();

    assert!(
        load_and_compile_workflow(
            &file_system,
            &cwd,
            &ancestor_workflow.to_string_lossy(),
            /*args*/ None,
            &agent_api(),
            /*personal_workflow_root*/ Some(&personal_root),
            /*sandbox*/ None,
        )
        .await
        .is_ok()
    );
    assert!(
        load_and_compile_workflow(
            &file_system,
            &cwd,
            &personal_workflow.to_string_lossy(),
            /*args*/ None,
            &agent_api(),
            /*personal_workflow_root*/ Some(&personal_root),
            /*sandbox*/ None,
        )
        .await
        .is_ok()
    );
}

#[cfg(unix)]
#[tokio::test]
async fn loader_rejects_symlinked_workflow_root_outside_project() {
    use std::os::unix::fs::symlink;

    let root = tempdir().expect("tempdir");
    let outside = tempdir().expect("outside tempdir");
    let cwd = PathUri::from_host_native_path(root.path()).expect("temporary path URI");
    let file_system = LocalFileSystem::unsandboxed();
    fs::create_dir_all(root.path().join(".codex")).expect("create .codex directory");
    fs::write(outside.path().join("outside.ts"), SIMPLE_WORKFLOW).expect("write workflow");
    symlink(outside.path(), root.path().join(".codex/workflows"))
        .expect("symlink workflow directory");

    assert_eq!(
        load_and_compile_workflow(
            &file_system,
            &cwd,
            ".codex/workflows/outside.ts",
            /*args*/ None,
            &agent_api(),
            /*personal_workflow_root*/ None,
            /*sandbox*/ None,
        )
        .await,
        Err("workflow path must stay below a project or personal workflows directory".to_string())
    );
}

#[tokio::test]
async fn compiled_workflow_runs_in_code_mode_runtime() {
    let compiled = compile_workflow_source(SIMPLE_WORKFLOW, Some(&json!(["a", "b"])), &agent_api())
        .expect("workflow should compile");
    let session = InProcessCodeModeSession::new();
    let response = session
        .execute(ExecuteRequest {
            tool_call_id: "workflow-test".to_string(),
            enabled_tools: Vec::new(),
            source: compiled,
            yield_time_ms: Some(5_000),
            max_output_tokens: None,
        })
        .await
        .expect("workflow runtime should start")
        .initial_response()
        .await
        .expect("workflow runtime should finish");

    assert_eq!(
        response,
        RuntimeResponse::Result {
            cell_id: codex_code_mode::CellId::new("1".to_string()),
            content_items: vec![FunctionCallOutputContentItem::InputText {
                text: concat!(
                    "{\n",
                    "  \"meta\": {\n",
                    "    \"description\": \"Return structured input\",\n",
                    "    \"name\": \"simple\"\n",
                    "  },\n",
                    "  \"result\": {\n",
                    "    \"received\": [\n",
                    "      \"a\",\n",
                    "      \"b\"\n",
                    "    ]\n",
                    "  }\n",
                    "}"
                )
                .to_string(),
            }],
            error_text: None,
        }
    );
}
