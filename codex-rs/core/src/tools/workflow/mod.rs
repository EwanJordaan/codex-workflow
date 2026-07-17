mod service;
mod source;
mod spec;

pub(crate) use service::WorkflowService;

use codex_protocol::models::FunctionCallOutputContentItem;
use codex_tools::ToolName;
use codex_tools::ToolSpec;
use serde::Deserialize;
use serde_json::Value;

use crate::function_tool::FunctionCallError;
use crate::tools::code_mode::ExecContext;
use crate::tools::code_mode::code_cell_status;
use crate::tools::code_mode::execute_code_cell_with_info;
use crate::tools::code_mode::handle_runtime_response;
use crate::tools::context::FunctionToolOutput;
use crate::tools::context::ToolInvocation;
use crate::tools::context::ToolPayload;
use crate::tools::context::boxed_tool_output;
use crate::tools::registry::CoreToolRuntime;
use crate::tools::registry::ToolExecutor;

use self::source::compile_inline_workflow;
use self::source::load_and_compile_workflow;
use self::spec::create_control_workflow_tool;
use self::spec::create_list_workflows_tool;
use self::spec::create_run_workflow_tool;
use self::spec::create_wait_workflow_tool;

pub(crate) const CONTROL_WORKFLOW_TOOL_NAME: &str = "control_workflow";
pub(crate) const LIST_WORKFLOWS_TOOL_NAME: &str = "list_workflows";
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
    name: Option<String>,
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
            let workflow_name = args.name.clone().unwrap_or_else(|| {
                args.path
                    .as_deref()
                    .and_then(|path| std::path::Path::new(path).file_stem())
                    .and_then(|name| name.to_str())
                    .unwrap_or("ephemeral")
                    .to_string()
            });
            let source = match (&args.path, &args.source) {
                (Some(path), None) => {
                    load_and_compile_workflow(
                        file_system.as_ref(),
                        cwd,
                        path,
                        args.args.as_ref(),
                        Some(&sandbox),
                    )
                    .await
                }
                (None, Some(source)) => compile_inline_workflow(source, args.args.as_ref()),
                (Some(_), Some(_)) => Err(
                    "run_workflow accepts exactly one of `path` or `source`, not both".to_string(),
                ),
                (None, None) => {
                    Err("run_workflow requires exactly one of `path` or `source`".to_string())
                }
            }
            .map_err(FunctionCallError::RespondToModel)?;
            let enabled_tools =
                codex_tools::collect_code_mode_tool_definitions(&self.nested_tool_specs);
            let mut execution = execute_code_cell_with_info(
                std::sync::Arc::clone(&session),
                turn,
                call_id,
                source,
                enabled_tools,
                Some(args.yield_time_ms.unwrap_or(1_000)),
                args.max_output_tokens,
            )
            .await
            .map_err(FunctionCallError::RespondToModel)?;
            let run = session
                .services
                .workflow_service
                .register(workflow_name, execution.cell_id, execution.status)
                .await;
            execution.output.body.insert(
                0,
                FunctionCallOutputContentItem::InputText {
                    text: format!("Managed workflow run {} ({:?}).\n", run.id, run.status),
                },
            );
            Ok(boxed_tool_output(execution.output))
        })
    }
}

impl CoreToolRuntime for RunWorkflowHandler {}

pub(crate) struct WaitWorkflowHandler;

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct WaitWorkflowArgs {
    cell_id: String,
    yield_time_ms: Option<u64>,
    max_tokens: Option<usize>,
    terminate: Option<bool>,
}

impl ToolExecutor<ToolInvocation> for WaitWorkflowHandler {
    fn tool_name(&self) -> ToolName {
        ToolName::plain(WAIT_WORKFLOW_TOOL_NAME)
    }

    fn spec(&self) -> ToolSpec {
        create_wait_workflow_tool()
    }

    fn handle(&self, invocation: ToolInvocation) -> codex_tools::ToolExecutorFuture<'_> {
        Box::pin(async move {
            let ToolPayload::Function { arguments } = &invocation.payload else {
                return Err(FunctionCallError::RespondToModel(
                    "wait_workflow expects JSON arguments".to_string(),
                ));
            };
            let args: WaitWorkflowArgs = serde_json::from_str(arguments).map_err(|err| {
                FunctionCallError::RespondToModel(format!(
                    "failed to parse wait_workflow arguments: {err}"
                ))
            })?;
            if args
                .max_tokens
                .is_some_and(|max_tokens| max_tokens > MAX_WORKFLOW_OUTPUT_TOKENS)
            {
                return Err(FunctionCallError::RespondToModel(format!(
                    "wait_workflow max_tokens cannot exceed {MAX_WORKFLOW_OUTPUT_TOKENS}"
                )));
            }
            let exec = ExecContext {
                session: std::sync::Arc::clone(&invocation.session),
                turn: std::sync::Arc::clone(&invocation.turn),
            };
            let cell_id = codex_code_mode::CellId::new(args.cell_id);
            let started_at = std::time::Instant::now();
            let outcome = if args.terminate.unwrap_or(false) {
                exec.session
                    .services
                    .code_mode_service
                    .terminate(cell_id.clone())
                    .await
            } else {
                exec.session
                    .services
                    .code_mode_service
                    .wait(codex_code_mode::WaitRequest {
                        cell_id: cell_id.clone(),
                        yield_time_ms: args.yield_time_ms.unwrap_or(10_000),
                    })
                    .await
            }
            .map_err(FunctionCallError::RespondToModel)?;
            let response = match &outcome {
                codex_code_mode::WaitOutcome::LiveCell(response)
                | codex_code_mode::WaitOutcome::MissingCell(response) => response,
            };
            let status = code_cell_status(response);
            exec.session
                .services
                .workflow_service
                .update_cell(&cell_id, status)
                .await;
            if matches!(&outcome, codex_code_mode::WaitOutcome::LiveCell(_))
                && !matches!(status, crate::tools::code_mode::CodeCellStatus::Running)
            {
                exec.session
                    .services
                    .rollout_thread_trace
                    .code_cell_trace_context(exec.turn.sub_id.as_str(), cell_id.as_str())
                    .record_ended(response);
                exec.session
                    .services
                    .code_mode_service
                    .finish_cell_dispatch(&cell_id);
            }
            exec.session.services.elicitations.wait_until_clear().await;
            handle_runtime_response(&exec, outcome.into(), args.max_tokens, started_at)
                .await
                .map(boxed_tool_output)
                .map_err(FunctionCallError::RespondToModel)
        })
    }
}

impl CoreToolRuntime for WaitWorkflowHandler {}

pub(crate) struct ListWorkflowsHandler;

impl ToolExecutor<ToolInvocation> for ListWorkflowsHandler {
    fn tool_name(&self) -> ToolName {
        ToolName::plain(LIST_WORKFLOWS_TOOL_NAME)
    }

    fn spec(&self) -> ToolSpec {
        create_list_workflows_tool()
    }

    fn handle(&self, invocation: ToolInvocation) -> codex_tools::ToolExecutorFuture<'_> {
        Box::pin(async move {
            let runs = invocation.session.services.workflow_service.list().await;
            let text = serde_json::to_string_pretty(&runs).map_err(|err| {
                FunctionCallError::RespondToModel(format!(
                    "failed to serialize workflow runs: {err}"
                ))
            })?;
            Ok(boxed_tool_output(FunctionToolOutput::from_text(
                text,
                Some(true),
            )))
        })
    }
}

impl CoreToolRuntime for ListWorkflowsHandler {}

pub(crate) struct ControlWorkflowHandler;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
enum WorkflowControlAction {
    Terminate,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ControlWorkflowArgs {
    run_id: String,
    action: WorkflowControlAction,
}

impl ToolExecutor<ToolInvocation> for ControlWorkflowHandler {
    fn tool_name(&self) -> ToolName {
        ToolName::plain(CONTROL_WORKFLOW_TOOL_NAME)
    }

    fn spec(&self) -> ToolSpec {
        create_control_workflow_tool()
    }

    fn handle(&self, invocation: ToolInvocation) -> codex_tools::ToolExecutorFuture<'_> {
        Box::pin(async move {
            let ToolPayload::Function { arguments } = &invocation.payload else {
                return Err(FunctionCallError::RespondToModel(
                    "control_workflow expects JSON arguments".to_string(),
                ));
            };
            let args: ControlWorkflowArgs = serde_json::from_str(arguments).map_err(|err| {
                FunctionCallError::RespondToModel(format!(
                    "failed to parse control_workflow arguments: {err}"
                ))
            })?;
            let Some(cell_id) = invocation
                .session
                .services
                .workflow_service
                .cell_for_run(&args.run_id)
                .await
            else {
                return Err(FunctionCallError::RespondToModel(format!(
                    "workflow run {} was not found",
                    args.run_id
                )));
            };
            match args.action {
                WorkflowControlAction::Terminate => {
                    let outcome = invocation
                        .session
                        .services
                        .code_mode_service
                        .terminate(cell_id.clone())
                        .await
                        .map_err(FunctionCallError::RespondToModel)?;
                    let response = match &outcome {
                        codex_code_mode::WaitOutcome::LiveCell(response)
                        | codex_code_mode::WaitOutcome::MissingCell(response) => response,
                    };
                    let status = code_cell_status(response);
                    invocation
                        .session
                        .services
                        .workflow_service
                        .update_cell(&cell_id, status)
                        .await;
                    invocation
                        .session
                        .services
                        .code_mode_service
                        .finish_cell_dispatch(&cell_id);
                }
            }
            Ok(boxed_tool_output(FunctionToolOutput::from_text(
                format!("Workflow run {} terminated.", args.run_id),
                Some(true),
            )))
        })
    }
}

impl CoreToolRuntime for ControlWorkflowHandler {}

#[cfg(test)]
#[path = "workflow_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "workflow_runtime_tests.rs"]
mod runtime_tests;
