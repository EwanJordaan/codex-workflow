mod source;
mod spec;

use codex_tools::ToolName;
use codex_tools::ToolSpec;
use serde::Deserialize;
use serde_json::Value;

use crate::function_tool::FunctionCallError;
use crate::tools::code_mode::execute_code_cell;
use crate::tools::code_mode::wait_handler::WaitOutputLimit;
use crate::tools::code_mode::wait_handler::handle_wait_call;
use crate::tools::context::ToolInvocation;
use crate::tools::context::ToolPayload;
use crate::tools::context::boxed_tool_output;
use crate::tools::registry::CoreToolRuntime;
use crate::tools::registry::ToolExecutor;

use self::source::compile_inline_workflow;
use self::source::load_and_compile_workflow;
use self::spec::create_run_workflow_tool;
use self::spec::create_wait_workflow_tool;

pub(crate) const RUN_WORKFLOW_TOOL_NAME: &str = "run_workflow";
pub(crate) const WAIT_WORKFLOW_TOOL_NAME: &str = "wait_workflow";
const MAX_WORKFLOW_OUTPUT_TOKENS: usize = 10_000;

pub(crate) struct RunWorkflowHandler {
    nested_tool_specs: Vec<ToolSpec>,
}

impl RunWorkflowHandler {
    pub(crate) fn new(nested_tool_specs: Vec<ToolSpec>) -> Self {
        Self { nested_tool_specs }
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RunWorkflowArgs {
    path: Option<String>,
    source: Option<String>,
    args: Option<Value>,
    yield_time_ms: Option<u64>,
    max_output_tokens: Option<usize>,
}

impl ToolExecutor<ToolInvocation> for RunWorkflowHandler {
    fn tool_name(&self) -> ToolName {
        ToolName::plain(RUN_WORKFLOW_TOOL_NAME)
    }

    fn spec(&self) -> ToolSpec {
        create_run_workflow_tool()
    }

    fn handle(&self, invocation: ToolInvocation) -> codex_tools::ToolExecutorFuture<'_> {
        Box::pin(async move {
            let ToolPayload::Function { arguments } = &invocation.payload else {
                return Err(FunctionCallError::RespondToModel(
                    "run_workflow expects JSON arguments".to_string(),
                ));
            };
            if invocation.tool_name != ToolName::plain(RUN_WORKFLOW_TOOL_NAME) {
                return Err(FunctionCallError::RespondToModel(
                    "run_workflow received an unexpected tool name".to_string(),
                ));
            }
            let args: RunWorkflowArgs = serde_json::from_str(arguments).map_err(|err| {
                FunctionCallError::RespondToModel(format!(
                    "failed to parse run_workflow arguments: {err}"
                ))
            })?;
            if args
                .max_output_tokens
                .is_some_and(|max_output_tokens| max_output_tokens > MAX_WORKFLOW_OUTPUT_TOKENS)
            {
                return Err(FunctionCallError::RespondToModel(format!(
                    "run_workflow max_output_tokens cannot exceed {MAX_WORKFLOW_OUTPUT_TOKENS}"
                )));
            }
            let session = std::sync::Arc::clone(&invocation.session);
            let turn = std::sync::Arc::clone(&invocation.turn);
            let call_id = invocation.call_id.clone();
            if !turn.environments.starting.is_empty() {
                return Err(FunctionCallError::RespondToModel(
                    "run_workflow cannot start while a turn environment is still starting"
                        .to_string(),
                ));
            }
            let [environment] = turn.environments.turn_environments.as_slice() else {
                return Err(FunctionCallError::RespondToModel(
                    "run_workflow requires exactly one turn environment".to_string(),
                ));
            };
            let cwd = environment.cwd();
            let file_system = environment.environment.get_filesystem();
            let sandbox =
                turn.file_system_sandbox_context(/*additional_permissions*/ None, cwd);
            let source = match (&args.path, &args.source) {
                (Some(path), None) => load_and_compile_workflow(
                    file_system.as_ref(),
                    cwd,
                    path,
                    args.args.as_ref(),
                    Some(&sandbox),
                )
                .await,
                (None, Some(source)) => compile_inline_workflow(source, args.args.as_ref()),
                (Some(_), Some(_)) => Err(
                    "run_workflow accepts exactly one of `path` or `source`, not both".to_string(),
                ),
                (None, None) => Err(
                    "run_workflow requires exactly one of `path` or `source`".to_string(),
                ),
            }
            .map_err(FunctionCallError::RespondToModel)?;
            let enabled_tools =
                codex_tools::collect_code_mode_tool_definitions(&self.nested_tool_specs);
            execute_code_cell(
                session,
                turn,
                call_id,
                source,
                enabled_tools,
                Some(args.yield_time_ms.unwrap_or(1_000)),
                args.max_output_tokens,
            )
            .await
            .map(boxed_tool_output)
        })
    }
}

impl CoreToolRuntime for RunWorkflowHandler {}

pub(crate) struct WaitWorkflowHandler;

impl ToolExecutor<ToolInvocation> for WaitWorkflowHandler {
    fn tool_name(&self) -> ToolName {
        ToolName::plain(WAIT_WORKFLOW_TOOL_NAME)
    }

    fn spec(&self) -> ToolSpec {
        create_wait_workflow_tool()
    }

    fn handle(&self, invocation: ToolInvocation) -> codex_tools::ToolExecutorFuture<'_> {
        Box::pin(async move {
            handle_wait_call(
                invocation,
                WAIT_WORKFLOW_TOOL_NAME,
                WaitOutputLimit::AtMost(MAX_WORKFLOW_OUTPUT_TOKENS),
            )
            .await
        })
    }
}

impl CoreToolRuntime for WaitWorkflowHandler {}

#[cfg(test)]
#[path = "workflow_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "workflow_runtime_tests.rs"]
mod runtime_tests;
