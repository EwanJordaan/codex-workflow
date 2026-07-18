use anyhow::Result;
use codex_exec_server::CreateDirectoryOptions;
use codex_features::Feature;
use codex_utils_path_uri::PathUri;
use core_test_support::responses::ev_assistant_message;
use core_test_support::responses::ev_completed;
use core_test_support::responses::ev_function_call;
use core_test_support::responses::ev_response_created;
use core_test_support::responses::mount_response_once_match;
use core_test_support::responses::mount_sse_once;
use core_test_support::responses::mount_sse_once_match;
use core_test_support::responses::sse;
use core_test_support::responses::sse_response;
use core_test_support::responses::start_mock_server;
use core_test_support::test_codex::test_codex;
use serde_json::Value;
use serde_json::json;

const PARENT_PROMPT: &str = "Run the test workflow";
const CHILD_PROMPT: &str = "Inspect the parser and return one finding.";
const CHILD_RESULT: &str = "The parser handles the tested edge case.";

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn workflow_runs_ephemeral_source_without_a_project_file() -> Result<()> {
    let server = start_mock_server().await;
    let source = r#"export const meta = {
  name: "ephemeral",
  description: "Run generated source",
};
return { received: args };
"#;

    mount_sse_once(
        &server,
        sse(vec![
            ev_response_created("parent-1"),
            ev_function_call(
                "workflow-call",
                "run_workflow",
                &serde_json::to_string(&json!({
                    "source": source,
                    "args": { "value": 7 },
                    "yield_time_ms": 5_000,
                }))?,
            ),
            ev_completed("parent-1"),
        ]),
    )
    .await;

    let list_call = mount_sse_once_match(
        &server,
        |request: &wiremock::Request| {
            body_contains(request, "workflow-call")
                && body_contains(request, "ephemeral")
                && body_contains(request, "received")
        },
        sse(vec![
            ev_response_created("parent-2"),
            ev_function_call("workflow-list", "list_workflows", "{}"),
            ev_completed("parent-2"),
        ]),
    )
    .await;

    let followup = mount_sse_once_match(
        &server,
        |request: &wiremock::Request| {
            body_contains(request, "workflow-list")
                && body_contains(request, "workflow-")
                && body_contains(request, "completed")
        },
        sse(vec![
            ev_response_created("parent-3"),
            ev_assistant_message("parent-message", "workflow complete"),
            ev_completed("parent-3"),
        ]),
    )
    .await;

    let test = test_codex()
        .with_model("test-gpt-5.1-codex")
        .with_config(|config| {
            config
                .features
                .enable(Feature::Collab)
                .expect("enable collaboration");
            config
                .features
                .enable(Feature::MultiAgentV2)
                .expect("enable multi-agent v2");
        })
        .build_with_auto_env(&server)
        .await?;
    test.submit_turn(PARENT_PROMPT).await?;

    tokio::time::timeout(std::time::Duration::from_secs(30), async {
        while followup.requests().is_empty() {
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("parent workflow follow-up should arrive");

    assert_eq!(list_call.requests().len(), 1);
    assert_eq!(followup.requests().len(), 1);
    Ok(())
}

fn body_contains(request: &wiremock::Request, needle: &str) -> bool {
    let is_zstd = request
        .headers
        .get("content-encoding")
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| {
            value
                .split(',')
                .any(|entry| entry.trim().eq_ignore_ascii_case("zstd"))
        });
    let body = if is_zstd {
        zstd::stream::decode_all(std::io::Cursor::new(&request.body)).ok()
    } else {
        Some(request.body.clone())
    };
    body.and_then(|body| String::from_utf8(body).ok())
        .is_some_and(|body| body.contains(needle))
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn workflow_launches_subagent_and_returns_its_result() -> Result<()> {
    let server = start_mock_server().await;
    let workflow_source = format!(
        r#"export const meta = {{
  name: "integration-test",
  description: "Exercise workflow agent orchestration",
}};

const finding = await agent({CHILD_PROMPT:?}, {{ label: "parser-review", forkTurns: "none" }});
return {{ finding }};
"#
    );

    mount_sse_once(
        &server,
        sse(vec![
            ev_response_created("parent-1"),
            ev_function_call(
                "workflow-call",
                "run_workflow",
                &serde_json::to_string(&json!({
                    "path": ".codex/workflows/integration.ts",
                    "yield_time_ms": 1,
                }))?,
            ),
            ev_completed("parent-1"),
        ]),
    )
    .await;

    mount_response_once_match(
        &server,
        |request: &wiremock::Request| body_contains(request, CHILD_PROMPT),
        sse_response(sse(vec![
            ev_response_created("child-1"),
            ev_assistant_message("child-message", CHILD_RESULT),
            ev_completed("child-1"),
        ]))
        .set_delay(std::time::Duration::from_millis(250)),
    )
    .await;

    let parent_wait = mount_sse_once_match(
        &server,
        |request: &wiremock::Request| body_contains(request, "workflow-call"),
        sse(vec![
            ev_response_created("parent-2"),
            ev_function_call(
                "workflow-wait",
                "wait_workflow",
                &serde_json::to_string(&json!({
                    "cell_id": "1",
                    "yield_time_ms": 30_000,
                }))
                .expect("wait arguments should serialize"),
            ),
            ev_completed("parent-2"),
        ]),
    )
    .await;

    let parent_followup = mount_sse_once_match(
        &server,
        |request: &wiremock::Request| {
            body_contains(request, "workflow-wait") && body_contains(request, CHILD_RESULT)
        },
        sse(vec![
            ev_response_created("parent-3"),
            ev_assistant_message("parent-message", "workflow complete"),
            ev_completed("parent-3"),
        ]),
    )
    .await;

    let mut builder = test_codex()
        .with_model("test-gpt-5.1-codex")
        .with_workspace_setup(move |cwd, file_system| async move {
            let workflow_dir = cwd.join(".codex/workflows");
            let workflow_dir_uri = PathUri::from_host_native_path(&workflow_dir)?;
            file_system
                .create_directory(
                    &workflow_dir_uri,
                    CreateDirectoryOptions { recursive: true },
                    /*sandbox*/ None,
                )
                .await?;
            let workflow_uri = PathUri::from_host_native_path(workflow_dir.join("integration.ts"))?;
            file_system
                .write_file(
                    &workflow_uri,
                    workflow_source.into_bytes(),
                    /*sandbox*/ None,
                )
                .await?;
            Ok::<(), anyhow::Error>(())
        })
        .with_config(|config| {
            config
                .features
                .enable(Feature::Collab)
                .expect("enable collaboration");
            config
                .features
                .enable(Feature::MultiAgentV2)
                .expect("enable multi-agent v2");
        });
    let test = builder.build_with_auto_env(&server).await?;
    test.submit_turn(PARENT_PROMPT).await?;
    tokio::time::timeout(std::time::Duration::from_secs(30), async {
        while parent_followup.requests().is_empty() {
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("parent workflow follow-up should arrive");

    assert_eq!(parent_wait.requests().len(), 1);
    let request = parent_followup.single_request();
    let output = request.function_call_output("workflow-wait");
    let output_text = output
        .get("output")
        .and_then(|output| match output {
            Value::String(text) => Some(text.clone()),
            Value::Array(items) => Some(
                items
                    .iter()
                    .filter_map(|item| item.get("text").and_then(Value::as_str))
                    .collect::<String>(),
            ),
            _ => None,
        })
        .expect("workflow output should contain text");
    assert!(output_text.contains(CHILD_RESULT));
    assert!(output_text.contains("integration-test"));
    Ok(())
}
