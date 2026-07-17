use std::collections::BTreeMap;

use codex_tools::JsonSchema;
use codex_tools::ResponsesApiTool;
use codex_tools::ToolSpec;

use super::RUN_WORKFLOW_TOOL_NAME;
use super::WAIT_WORKFLOW_TOOL_NAME;

pub(super) fn create_run_workflow_tool() -> ToolSpec {
    let properties = BTreeMap::from([
        (
            "path".to_string(),
            JsonSchema::string(Some(
                "Project-relative .ts or .js file below .codex/workflows/. Mutually exclusive with source."
                    .to_string(),
            )),
        ),
        (
            "source".to_string(),
            JsonSchema::string(Some(
                "Ephemeral JavaScript-compatible workflow source. Mutually exclusive with path."
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
            "Launch a reusable multi-agent workflow from either a JavaScript-compatible ",
            "TypeScript file under `.codex/workflows/` or ephemeral source. Provide exactly ",
            "one of `path` or `source`. If this call yields a cell ID, call ",
            "`wait_workflow` until the run completes."
        )
        .to_string(),
        strict: false,
        parameters: JsonSchema::object(
            properties,
            None,
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
