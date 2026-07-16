use std::collections::HashMap;
use std::sync::Arc;
use std::sync::Mutex;
use std::time::Duration;

use codex_code_mode::CellId;
use codex_code_mode::CodeModeNestedToolCall;
use codex_code_mode::CodeModeSessionDelegate;
use codex_code_mode::CodeModeToolKind;
use codex_code_mode::ExecuteRequest;
use codex_code_mode::FunctionCallOutputContentItem;
use codex_code_mode::InProcessCodeModeSession;
use codex_code_mode::NotificationFuture;
use codex_code_mode::RuntimeResponse;
use codex_code_mode::ToolDefinition;
use codex_code_mode::ToolInvocationFuture;
use codex_protocol::ToolName;
use pretty_assertions::assert_eq;
use serde_json::Map;
use serde_json::Value;
use serde_json::json;
use tokio_util::sync::CancellationToken;

use codex_code_mode::compile_workflow_source;

#[derive(Default)]
struct AgentState {
    next_id: usize,
    active: usize,
    max_active: usize,
    messages: HashMap<String, String>,
    spawn_inputs: Vec<Value>,
}

struct AgentDelegate {
    state: Mutex<AgentState>,
    wait_delay: Duration,
}

impl AgentDelegate {
    fn new(wait_delay: Duration) -> Self {
        Self {
            state: Mutex::new(AgentState::default()),
            wait_delay,
        }
    }

    fn with_state<T>(&self, read: impl FnOnce(&AgentState) -> T) -> T {
        let state = match self.state.lock() {
            Ok(state) => state,
            Err(poisoned) => poisoned.into_inner(),
        };
        read(&state)
    }
}

impl CodeModeSessionDelegate for AgentDelegate {
    fn invoke_tool<'a>(
        &'a self,
        invocation: CodeModeNestedToolCall,
        cancellation_token: CancellationToken,
    ) -> ToolInvocationFuture<'a> {
        Box::pin(async move {
            let input = invocation.input.unwrap_or_else(|| json!({}));
            match invocation.tool_name.name.as_str() {
                "spawn_agent" => {
                    let message = input
                        .get("message")
                        .and_then(Value::as_str)
                        .ok_or_else(|| "spawn_agent message is missing".to_string())?
                        .to_string();
                    let mut state = match self.state.lock() {
                        Ok(state) => state,
                        Err(poisoned) => poisoned.into_inner(),
                    };
                    state.next_id += 1;
                    state.active += 1;
                    state.max_active = state.max_active.max(state.active);
                    let agent_id = format!("agent-{}", state.next_id);
                    state.messages.insert(agent_id.clone(), message);
                    state.spawn_inputs.push(input);
                    Ok(json!({ "agent_id": agent_id }))
                }
                "wait_agent" => {
                    let agent_id = input
                        .get("targets")
                        .and_then(Value::as_array)
                        .and_then(|targets| targets.first())
                        .and_then(Value::as_str)
                        .ok_or_else(|| "wait_agent target is missing".to_string())?
                        .to_string();
                    tokio::select! {
                        _ = tokio::time::sleep(self.wait_delay) => {}
                        _ = cancellation_token.cancelled() => {
                            return Err("wait_agent cancelled".to_string());
                        }
                    }
                    let mut state = match self.state.lock() {
                        Ok(state) => state,
                        Err(poisoned) => poisoned.into_inner(),
                    };
                    state.active -= 1;
                    let message = state
                        .messages
                        .remove(&agent_id)
                        .ok_or_else(|| format!("unknown agent {agent_id}"))?;
                    let completed = if message.starts_with("structured") {
                        "{}".to_string()
                    } else if message.starts_with("fail") {
                        let mut statuses = Map::new();
                        statuses.insert(agent_id, json!({ "errored": "intentional failure" }));
                        return Ok(json!({ "status": statuses, "timed_out": false }));
                    } else {
                        format!("done:{message}")
                    };
                    let mut statuses = Map::new();
                    statuses.insert(agent_id, json!({ "completed": completed }));
                    Ok(json!({ "status": statuses, "timed_out": false }))
                }
                "close_agent" => Ok(json!({ "status": "shutdown" })),
                name => Err(format!("unexpected workflow tool {name}")),
            }
        })
    }

    fn notify<'a>(
        &'a self,
        _call_id: String,
        _cell_id: CellId,
        _text: String,
        _cancellation_token: CancellationToken,
    ) -> NotificationFuture<'a> {
        Box::pin(async { Ok(()) })
    }

    fn cell_closed(&self, _cell_id: &CellId) {}
}

fn workflow_tool(name: &str) -> ToolDefinition {
    ToolDefinition {
        name: format!("multi_agent_v1__{name}"),
        tool_name: ToolName::namespaced("multi_agent_v1", name),
        description: String::new(),
        kind: CodeModeToolKind::Function,
        input_schema: None,
        output_schema: None,
    }
}

async fn execute_workflow(source: &str, delegate: Arc<AgentDelegate>) -> RuntimeResponse {
    let compiled = compile_workflow_source(source, None).expect("workflow should compile");
    InProcessCodeModeSession::with_delegate(delegate)
        .execute(ExecuteRequest {
            tool_call_id: "workflow-runtime-test".to_string(),
            enabled_tools: ["spawn_agent", "wait_agent", "close_agent"]
                .map(workflow_tool)
                .into(),
            source: compiled,
            yield_time_ms: Some(5_000),
            max_output_tokens: None,
        })
        .await
        .expect("workflow runtime should start")
        .initial_response()
        .await
        .expect("workflow runtime should finish")
}

fn response_json(response: RuntimeResponse) -> Value {
    let RuntimeResponse::Result {
        content_items,
        error_text: None,
        ..
    } = response
    else {
        panic!("expected successful workflow response")
    };
    let [FunctionCallOutputContentItem::InputText { text }] = content_items.as_slice() else {
        panic!("expected one text output")
    };
    serde_json::from_str(text).expect("workflow output should be JSON")
}

fn response_error(response: RuntimeResponse) -> String {
    let RuntimeResponse::Result {
        error_text: Some(error),
        ..
    } = response
    else {
        panic!("expected failed workflow response")
    };
    error
}

#[tokio::test]
async fn pipeline_preserves_order_caps_agents_and_hides_raw_tools() {
    let delegate = Arc::new(AgentDelegate::new(Duration::from_millis(20)));
    let response = execute_workflow(
        r#"export const meta = { name: "pipeline", description: "exercise helpers" };
const rawTools = [typeof tools, typeof globalThis.tools, typeof globalThis.ALL_TOOLS];
const results = await pipeline(
  [0, 1, 2, 3, 4, 5, 6, 7],
  (item) => agent(`job-${item}`),
  { concurrency: 4 },
);
return { rawTools, results };
"#,
        Arc::clone(&delegate),
    )
    .await;

    assert_eq!(
        response_json(response),
        json!({
            "meta": { "name": "pipeline", "description": "exercise helpers" },
            "result": {
                "rawTools": ["undefined", "undefined", "undefined"],
                "results": [
                    "done:job-0", "done:job-1", "done:job-2", "done:job-3",
                    "done:job-4", "done:job-5", "done:job-6", "done:job-7",
                ],
            },
        })
    );
    assert_eq!(delegate.with_state(|state| state.max_active), 4);
    assert_eq!(delegate.with_state(|state| state.spawn_inputs.len()), 8);
}

#[tokio::test]
async fn agent_enforces_prompt_schema_and_failure_boundaries() {
    let prompt_error = response_error(
        execute_workflow(
            r#"export const meta = { name: "prompt", description: "bound prompt" };
return agent("x".repeat(9217));
"#,
            Arc::new(AgentDelegate::new(Duration::ZERO)),
        )
        .await,
    );
    assert!(prompt_error.contains("agent prompt exceeds the 9216-byte limit"));

    let schema_error = response_error(
        execute_workflow(
            r#"export const meta = { name: "schema", description: "validate schema" };
return agent("structured", {
  schema: { type: "object", required: ["constructor"] },
});
"#,
            Arc::new(AgentDelegate::new(Duration::ZERO)),
        )
        .await,
    );
    assert!(schema_error.contains("missing required property constructor"));

    let agent_error = response_error(
        execute_workflow(
            r#"export const meta = { name: "failure", description: "surface failure" };
return agent("fail intentionally");
"#,
            Arc::new(AgentDelegate::new(Duration::ZERO)),
        )
        .await,
    );
    assert!(agent_error.contains("agent failed: intentional failure"));
}
