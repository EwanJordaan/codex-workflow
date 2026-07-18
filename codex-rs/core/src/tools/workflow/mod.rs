mod service;
mod source;
mod spec;

pub(crate) use service::DEFAULT_WORKFLOW_LIST_LIMIT;
pub(crate) use service::MAX_WORKFLOW_LIST_LIMIT;
pub(crate) use service::WorkflowAgentVersion;
pub(crate) use service::WorkflowOwnedAgent;
pub(crate) use service::WorkflowRunStatus;
pub(crate) use service::WorkflowService;

use codex_protocol::models::FunctionCallOutputContentItem;
use codex_tools::ToolName;
use codex_tools::ToolSpec;
use serde::Deserialize;
use serde_json::Value;
use tracing::warn;

use crate::function_tool::FunctionCallError;
use crate::session::session::Session;
use crate::tools::code_mode::CodeCellStatus;
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
use self::source::workflow_agent_api;
use self::spec::create_control_workflow_tool;
use self::spec::create_list_workflows_tool;
use self::spec::create_run_workflow_tool;
use self::spec::create_wait_workflow_tool;

pub(crate) const CONTROL_WORKFLOW_TOOL_NAME: &str = "control_workflow";
pub(crate) const LIST_WORKFLOWS_TOOL_NAME: &str = "list_workflows";
pub(crate) const RUN_WORKFLOW_TOOL_NAME: &str = "run_workflow";
pub(crate) const WAIT_WORKFLOW_TOOL_NAME: &str = "wait_workflow";
const MAX_WORKFLOW_OUTPUT_TOKENS: usize = 10_000;
const MAX_WORKFLOW_ARGUMENT_BYTES: usize = 16 * 1024;
const MAX_WORKFLOW_NAME_BYTES: usize = 256;
const MAX_WORKFLOW_YIELD_TIME_MS: u64 = 30_000;

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
            if arguments.len() > MAX_WORKFLOW_ARGUMENT_BYTES {
                return Err(FunctionCallError::RespondToModel(format!(
                    "run_workflow arguments exceed the {MAX_WORKFLOW_ARGUMENT_BYTES}-byte limit"
                )));
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
            if args
                .yield_time_ms
                .is_some_and(|yield_time_ms| yield_time_ms > MAX_WORKFLOW_YIELD_TIME_MS)
            {
                return Err(FunctionCallError::RespondToModel(format!(
                    "run_workflow yield_time_ms cannot exceed {MAX_WORKFLOW_YIELD_TIME_MS}"
                )));
            }
            if args
                .name
                .as_ref()
                .is_some_and(|name| name.len() > MAX_WORKFLOW_NAME_BYTES)
            {
                return Err(FunctionCallError::RespondToModel(format!(
                    "run_workflow name cannot exceed {MAX_WORKFLOW_NAME_BYTES} bytes"
                )));
            }
            let session = std::sync::Arc::clone(&invocation.session);
            let turn = std::sync::Arc::clone(&invocation.turn);
            let origin_sub_id = turn.sub_id.clone();
            let call_id = invocation.call_id.clone();
            if turn.environments.starting().next().is_some() {
                return Err(FunctionCallError::RespondToModel(
                    "run_workflow cannot start while a turn environment is still starting"
                        .to_string(),
                ));
            }
            let environments = turn.environments.turn_environments().collect::<Vec<_>>();
            let [environment] = environments.as_slice() else {
                return Err(FunctionCallError::RespondToModel(
                    "run_workflow requires exactly one turn environment".to_string(),
                ));
            };
            let cwd = environment.cwd();
            let file_system = environment.environment.get_filesystem();
            let sandbox =
                turn.file_system_sandbox_context(/*additional_permissions*/ None, environment);
            let personal_workflow_root = codex_utils_path_uri::PathUri::from_host_native_path(
                turn.config.codex_home.join("workflows"),
            )
            .ok();
            let workflow_name = args.name.clone().unwrap_or_else(|| {
                args.path
                    .as_deref()
                    .and_then(|path| std::path::Path::new(path).file_stem())
                    .and_then(|name| name.to_str())
                    .unwrap_or("ephemeral")
                    .to_string()
            });
            let enabled_tools =
                codex_tools::collect_code_mode_tool_definitions(&self.nested_tool_specs);
            let agent_api =
                workflow_agent_api(&enabled_tools).map_err(FunctionCallError::RespondToModel)?;
            let source = match (&args.path, &args.source) {
                (Some(path), None) => {
                    load_and_compile_workflow(
                        file_system.as_ref(),
                        cwd,
                        path,
                        args.args.as_ref(),
                        &agent_api,
                        personal_workflow_root.as_ref(),
                        Some(&sandbox),
                    )
                    .await
                }
                (None, Some(source)) => {
                    compile_inline_workflow(source, args.args.as_ref(), &agent_api)
                }
                (Some(_), Some(_)) => Err(
                    "run_workflow accepts exactly one of `path` or `source`, not both".to_string(),
                ),
                (None, None) => {
                    Err("run_workflow requires exactly one of `path` or `source`".to_string())
                }
            }
            .map_err(FunctionCallError::RespondToModel)?;
            let started_at = std::time::Instant::now();
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
                .register(
                    workflow_name,
                    execution.cell_id.clone(),
                    execution.status,
                    origin_sub_id,
                    started_at,
                    (!matches!(execution.status, CodeCellStatus::Running))
                        .then_some(&execution.response),
                )
                .await;
            if matches!(execution.status, CodeCellStatus::Running) {
                watch_workflow_completion(
                    std::sync::Arc::clone(&session),
                    execution.cell_id.clone(),
                );
            } else {
                cleanup_workflow_agents(&session, &execution.cell_id).await;
            }
            execution.output.body.insert(
                0,
                FunctionCallOutputContentItem::InputText {
                    text: format!(
                        "Managed workflow run {} ({}).\n{}\n",
                        run.id,
                        run.status.as_str(),
                        WorkflowService::persistence_marker(&run)
                            .map_err(FunctionCallError::RespondToModel)?
                    ),
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
            if args
                .yield_time_ms
                .is_some_and(|yield_time_ms| yield_time_ms > MAX_WORKFLOW_YIELD_TIME_MS)
            {
                return Err(FunctionCallError::RespondToModel(format!(
                    "wait_workflow yield_time_ms cannot exceed {MAX_WORKFLOW_YIELD_TIME_MS}"
                )));
            }
            let exec = ExecContext {
                session: std::sync::Arc::clone(&invocation.session),
                turn: std::sync::Arc::clone(&invocation.turn),
            };
            let cell_id = codex_code_mode::CellId::new(args.cell_id);
            let started_at = std::time::Instant::now();
            let Some(wait_lock) = exec
                .session
                .services
                .workflow_service
                .wait_lock(&cell_id)
                .await
            else {
                return Err(FunctionCallError::RespondToModel(format!(
                    "workflow cell {cell_id} was not found in the managed run registry"
                )));
            };
            let _wait_guard = wait_lock.lock().await;
            if !args.terminate.unwrap_or(false)
                && let Some(response) = exec
                    .session
                    .services
                    .workflow_service
                    .terminal_response(&cell_id)
                    .await
            {
                return handle_runtime_response(&exec, response, args.max_tokens, started_at)
                    .await
                    .map(boxed_tool_output)
                    .map_err(FunctionCallError::RespondToModel);
            }
            if args.terminate.unwrap_or(false) {
                cleanup_workflow_agents(&exec.session, &cell_id).await;
            }
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
            if !matches!(status, CodeCellStatus::Running) {
                finish_workflow_cell(
                    &exec.session,
                    &cell_id,
                    response,
                    matches!(&outcome, codex_code_mode::WaitOutcome::LiveCell(_)),
                )
                .await;
            } else {
                exec.session
                    .services
                    .workflow_service
                    .update_cell(&cell_id, status, Some(response))
                    .await;
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

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ListWorkflowsArgs {
    limit: Option<usize>,
}

impl ToolExecutor<ToolInvocation> for ListWorkflowsHandler {
    fn tool_name(&self) -> ToolName {
        ToolName::plain(LIST_WORKFLOWS_TOOL_NAME)
    }

    fn spec(&self) -> ToolSpec {
        create_list_workflows_tool()
    }

    fn handle(&self, invocation: ToolInvocation) -> codex_tools::ToolExecutorFuture<'_> {
        Box::pin(async move {
            let ToolPayload::Function { arguments } = &invocation.payload else {
                return Err(FunctionCallError::RespondToModel(
                    "list_workflows expects JSON arguments".to_string(),
                ));
            };
            let args: ListWorkflowsArgs = serde_json::from_str(arguments).map_err(|err| {
                FunctionCallError::RespondToModel(format!(
                    "failed to parse list_workflows arguments: {err}"
                ))
            })?;
            let limit = args.limit.unwrap_or(DEFAULT_WORKFLOW_LIST_LIMIT);
            if limit == 0 || limit > MAX_WORKFLOW_LIST_LIMIT {
                return Err(FunctionCallError::RespondToModel(format!(
                    "list_workflows limit must be from 1 through {MAX_WORKFLOW_LIST_LIMIT}"
                )));
            }
            let runs = invocation
                .session
                .services
                .workflow_service
                .list(limit)
                .await;
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
            let Some(run) = invocation
                .session
                .services
                .workflow_service
                .run(&args.run_id)
                .await
            else {
                return Err(FunctionCallError::RespondToModel(format!(
                    "workflow run {} was not found",
                    args.run_id
                )));
            };
            match args.action {
                WorkflowControlAction::Terminate => {
                    if run.status.is_terminal() {
                        return Ok(boxed_tool_output(FunctionToolOutput::from_text(
                            format!(
                                "Workflow run {} is already {}.",
                                args.run_id,
                                run.status.as_str()
                            ),
                            Some(true),
                        )));
                    }
                    let Some(wait_lock) = invocation
                        .session
                        .services
                        .workflow_service
                        .wait_lock(&run.cell_id)
                        .await
                    else {
                        return Err(FunctionCallError::RespondToModel(format!(
                            "workflow run {} no longer has a live runtime cell",
                            args.run_id
                        )));
                    };
                    let _wait_guard = wait_lock.lock().await;
                    if let Some(current) = invocation
                        .session
                        .services
                        .workflow_service
                        .run(&args.run_id)
                        .await
                        && current.status.is_terminal()
                    {
                        return Ok(boxed_tool_output(FunctionToolOutput::from_text(
                            format!(
                                "Workflow run {} is already {}.",
                                args.run_id,
                                current.status.as_str()
                            ),
                            Some(true),
                        )));
                    }
                    cleanup_workflow_agents(&invocation.session, &run.cell_id).await;
                    let outcome = invocation
                        .session
                        .services
                        .code_mode_service
                        .terminate(run.cell_id.clone())
                        .await
                        .map_err(FunctionCallError::RespondToModel)?;
                    let response = match &outcome {
                        codex_code_mode::WaitOutcome::LiveCell(response)
                        | codex_code_mode::WaitOutcome::MissingCell(response) => response,
                    };
                    let status = code_cell_status(response);
                    finish_workflow_cell(
                        &invocation.session,
                        &run.cell_id,
                        response,
                        matches!(&outcome, codex_code_mode::WaitOutcome::LiveCell(_)),
                    )
                    .await;
                    return Ok(boxed_tool_output(FunctionToolOutput::from_text(
                        format!(
                            "Workflow run {} is {}.",
                            args.run_id,
                            WorkflowRunStatus::from(status).as_str()
                        ),
                        Some(true),
                    )));
                }
            }
        })
    }
}

impl CoreToolRuntime for ControlWorkflowHandler {}

fn watch_workflow_completion(session: std::sync::Arc<Session>, cell_id: codex_code_mode::CellId) {
    tokio::spawn(async move {
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        loop {
            let Some(wait_lock) = session.services.workflow_service.wait_lock(&cell_id).await
            else {
                warn!(cell_id = %cell_id, "workflow completion watcher lost its registry entry");
                return;
            };
            let _wait_guard = wait_lock.lock().await;
            if session
                .services
                .workflow_service
                .terminal_response(&cell_id)
                .await
                .is_some()
            {
                return;
            }
            let outcome = session
                .services
                .code_mode_service
                .wait(codex_code_mode::WaitRequest {
                    cell_id: cell_id.clone(),
                    yield_time_ms: MAX_WORKFLOW_YIELD_TIME_MS,
                })
                .await;
            let outcome = match outcome {
                Ok(outcome) => outcome,
                Err(err) => {
                    warn!(%err, cell_id = %cell_id, "workflow completion watcher failed");
                    let response = codex_code_mode::RuntimeResponse::Result {
                        cell_id: cell_id.clone(),
                        content_items: Vec::new(),
                        error_text: Some(format!("workflow completion watcher failed: {err}")),
                    };
                    finish_workflow_cell(&session, &cell_id, &response, /*live_cell*/ true).await;
                    return;
                }
            };
            let response = match &outcome {
                codex_code_mode::WaitOutcome::LiveCell(response)
                | codex_code_mode::WaitOutcome::MissingCell(response) => response,
            };
            if matches!(code_cell_status(response), CodeCellStatus::Running) {
                continue;
            }
            finish_workflow_cell(
                &session,
                &cell_id,
                response,
                matches!(&outcome, codex_code_mode::WaitOutcome::LiveCell(_)),
            )
            .await;
            return;
        }
    });
}

async fn finish_workflow_cell(
    session: &std::sync::Arc<Session>,
    cell_id: &codex_code_mode::CellId,
    response: &codex_code_mode::RuntimeResponse,
    live_cell: bool,
) {
    let status = code_cell_status(response);
    let changed = session
        .services
        .workflow_service
        .update_cell(cell_id, status, Some(response))
        .await;
    if live_cell && let Some(run) = changed {
        session
            .services
            .rollout_thread_trace
            .code_cell_trace_context(run.origin_sub_id.as_str(), cell_id.as_str())
            .record_ended(response);
        session
            .services
            .code_mode_service
            .finish_cell_dispatch(cell_id);
    }
    cleanup_workflow_agents(session, cell_id).await;
}

async fn cleanup_workflow_agents(
    session: &std::sync::Arc<Session>,
    cell_id: &codex_code_mode::CellId,
) {
    let agents = session.services.workflow_service.take_agents(cell_id).await;
    for agent in agents {
        let result = tokio::time::timeout(std::time::Duration::from_secs(5), async {
            match agent.version {
                WorkflowAgentVersion::V1 => {
                    session
                        .services
                        .agent_control
                        .close_agent(agent.thread_id)
                        .await
                }
                WorkflowAgentVersion::V2 => {
                    session
                        .services
                        .agent_control
                        .interrupt_agent(agent.thread_id)
                        .await
                }
            }
        })
        .await;
        match result {
            Ok(Ok(_)) => {}
            Ok(Err(err)) => {
                warn!(%err, agent_id = %agent.thread_id, cell_id = %cell_id, "failed to stop workflow agent");
            }
            Err(_) => {
                warn!(agent_id = %agent.thread_id, cell_id = %cell_id, "timed out stopping workflow agent");
            }
        }
    }
}

#[cfg(test)]
#[path = "workflow_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "workflow_runtime_tests.rs"]
mod runtime_tests;
