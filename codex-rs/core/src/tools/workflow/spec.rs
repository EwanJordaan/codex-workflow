use std::collections::BTreeMap;

use codex_tools::JsonSchema;
use codex_tools::ResponsesApiTool;
use codex_tools::ToolSpec;

use super::CONTROL_WORKFLOW_TOOL_NAME;
use super::LIST_WORKFLOWS_TOOL_NAME;
use super::RUN_WORKFLOW_TOOL_NAME;
use super::WAIT_WORKFLOW_TOOL_NAME;

pub(super) fn create_run_workflow_tool() -> ToolSpec {
    let properties = BTreeMap::from([
        (
            "name".to_string(),
            JsonSchema::string(Some(
                "Optional display name for this managed workflow run.".to_string(),
            )),
        ),
        (
            "path".to_string(),
            JsonSchema::string(Some(
                "A .ts or .js file in the current or an ancestor project's .codex/workflows directory, or the personal workflows directory. Mutually exclusive with source."
                    .to_string(),
            )),
        ),
        (
            "source".to_string(),
            JsonSchema::string(Some(
                "Ephemeral JavaScript-compatible workflow source, limited to 4096 bytes. Mutually exclusive with path."
                    .to_string(),
            )),
        ),
        (
            "args".to_string(),
            JsonSchema {
                description: Some(
                    "Optional structured JSON value exposed to the workflow as global args."
                        .to_string(),
                ),
                ..Default::default()
            },
        ),
        (
            "yield_time_ms".to_string(),
            JsonSchema::number(Some(
                "Initial wait before returning a running cell. Defaults to 1000 ms.".to_string(),
            )),
        ),
        (
            "max_output_tokens".to_string(),
            JsonSchema::number(Some(
                "Maximum model-visible output tokens. Defaults to 10000.".to_string(),
            )),
        ),
    ]);

    ToolSpec::Function(ResponsesApiTool {
        name: RUN_WORKFLOW_TOOL_NAME.to_string(),
        description: concat!(
            "Launch a bounded multi-agent workflow from either a JavaScript-compatible ",
            "TypeScript file under `.codex/workflows/` or ephemeral source. Provide exactly ",
            "one of `path` or `source`. Source starts with `export const meta = { name, ",
            "description };`. The workflow receives `args` and can use `agent(prompt, ",
            "options?)`, `parallel(tasks, { concurrency })`, `pipeline(items, worker, ",
            "{ concurrency })`, and `phase(name, work)`. Keep filesystem, shell, and network ",
            "work inside agents. Concurrency is capped at 4 and each workflow at 64 agents. ",
            "If this call yields a cell ID, call `wait_workflow` until the run completes."
        )
        .to_string(),
        strict: false,
        parameters: JsonSchema::object(properties, None, Some(false.into())),
        output_schema: None,
        defer_loading: None,
    })
}

pub(super) fn create_list_workflows_tool() -> ToolSpec {
    let properties = BTreeMap::from([(
        "limit".to_string(),
        JsonSchema::number(Some(
            "Maximum number of newest runs to return, from 1 through 50. Defaults to 20."
                .to_string(),
        )),
    )]);
    ToolSpec::Function(ResponsesApiTool {
        name: LIST_WORKFLOWS_TOOL_NAME.to_string(),
        description: "List managed workflow runs in this session and their lifecycle status."
            .to_string(),
        strict: false,
        parameters: JsonSchema::object(properties, None, Some(false.into())),
        output_schema: None,
        defer_loading: None,
    })
}

pub(super) fn create_control_workflow_tool() -> ToolSpec {
    let properties = BTreeMap::from([
        (
            "run_id".to_string(),
            JsonSchema::string(Some("Managed workflow run identifier.".to_string())),
        ),
        (
            "action".to_string(),
            JsonSchema::string(Some(
                "Lifecycle action. Currently supports terminate.".to_string(),
            )),
        ),
    ]);
    ToolSpec::Function(ResponsesApiTool {
        name: CONTROL_WORKFLOW_TOOL_NAME.to_string(),
        description: "Control a managed workflow run.".to_string(),
        strict: false,
        parameters: JsonSchema::object(
            properties,
            Some(vec!["run_id".to_string(), "action".to_string()]),
            Some(false.into()),
        ),
        output_schema: None,
        defer_loading: None,
    })
}

pub(super) fn create_wait_workflow_tool() -> ToolSpec {
    let properties = BTreeMap::from([
        (
            "cell_id".to_string(),
            JsonSchema::string(Some("Running workflow cell identifier.".to_string())),
        ),
        (
            "yield_time_ms".to_string(),
            JsonSchema::number(Some(
                "Wait before yielding more output. Defaults to 10000 ms.".to_string(),
            )),
        ),
        (
            "max_tokens".to_string(),
            JsonSchema::number(Some(
                "Maximum model-visible output tokens. Defaults to 10000.".to_string(),
            )),
        ),
        (
            "terminate".to_string(),
            JsonSchema::boolean(Some(
                "Stop the workflow cell instead of waiting when true.".to_string(),
            )),
        ),
    ]);

    ToolSpec::Function(ResponsesApiTool {
        name: WAIT_WORKFLOW_TOOL_NAME.to_string(),
        description: "Wait for a yielded run_workflow cell or terminate it.".to_string(),
        strict: false,
        parameters: JsonSchema::object(
            properties,
            Some(vec!["cell_id".to_string()]),
            Some(false.into()),
        ),
        output_schema: None,
        defer_loading: None,
    })
}
