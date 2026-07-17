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
use codex_code_mode::compile_workflow_source;

const SIMPLE_WORKFLOW: &str = r#"
export const meta = {
  name: "simple",
  description: "Return structured input",
};

await Promise.resolve();
return { received: args };
"#;

#[test]
fn inline_workflow_enforces_size_limit() {
    assert!(compile_inline_workflow(SIMPLE_WORKFLOW, None).is_ok());
    assert_eq!(
        compile_inline_workflow(&"x".repeat(128 * 1024 + 1), None),
        Err("workflow source exceeds the 131072-byte limit".to_string())
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
        load_and_compile_workflow(&file_system, &cwd, ".codex/workflows/simple.ts", None, None,)
            .await
            .is_ok()
    );
    assert_eq!(
        load_and_compile_workflow(&file_system, &cwd, "outside.ts", None, None).await,
        Err("workflow path must stay below .codex/workflows".to_string())
    );
    assert_eq!(
        load_and_compile_workflow(&file_system, &cwd, "../outside.ts", None, None).await,
        Err("workflow path must be project-relative and cannot contain `..`".to_string())
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
            None,
            None,
        )
        .await,
        Err("workflow path must end in .ts or .js".to_string())
    );
    assert_eq!(
        load_and_compile_workflow(&file_system, &cwd, ".codex/workflows/large.ts", None, None,)
            .await,
        Err("workflow file exceeds the 131072-byte limit".to_string())
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
            None,
            None,
        )
        .await,
        Err("workflow directory must stay below the project .codex directory".to_string())
    );
}

#[tokio::test]
async fn compiled_workflow_runs_in_code_mode_runtime() {
    let compiled = compile_workflow_source(SIMPLE_WORKFLOW, Some(&json!(["a", "b"])))
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
