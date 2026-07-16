use std::collections::HashMap;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::AtomicU64;
use std::sync::atomic::Ordering;

use codex_code_mode::CellId;
use codex_code_mode::CodeModeNestedToolCall;
use codex_code_mode::CodeModeSessionDelegate;
use codex_code_mode::NotificationFuture;
use codex_code_mode::ToolInvocationFuture;
use codex_protocol::models::FunctionCallOutputPayload;
use codex_protocol::models::ResponseItem;
use serde_json::Value as JsonValue;
use tokio::sync::watch;
use tokio_util::sync::CancellationToken;

use super::ExecContext;
use super::PUBLIC_TOOL_NAME;
use super::call_nested_tool;
use crate::session::step_context::StepContext;
use crate::tools::ToolRouter;
use crate::tools::context::SharedTurnDiffTracker;
use crate::tools::parallel::ToolCallRuntime;

type TurnHostRegistry = Arc<Mutex<HashMap<String, (u64, Arc<CoreTurnHost>)>>>;

pub(super) struct CodeModeDispatchBroker {
    cell_hosts: Mutex<HashMap<CellId, watch::Sender<Option<Arc<CoreTurnHost>>>>>,
    turn_hosts: TurnHostRegistry,
    next_host_id: AtomicU64,
}

impl CodeModeDispatchBroker {
    pub(super) fn new() -> Self {
        Self {
            cell_hosts: Mutex::new(HashMap::new()),
            turn_hosts: Arc::new(Mutex::new(HashMap::new())),
            next_host_id: AtomicU64::new(1),
        }
    }

    pub(super) fn bind_cell_to_turn(&self, cell_id: &CellId, turn_id: &str) -> Result<(), String> {
        let turn_hosts = match self.turn_hosts.lock() {
            Ok(turn_hosts) => turn_hosts,
            Err(poisoned) => poisoned.into_inner(),
        };
        let host = turn_hosts
            .get(turn_id)
            .map(|(_, host)| Arc::clone(host))
            .ok_or_else(|| format!("code mode dispatch host is unavailable for turn {turn_id}"))?;
        cell_host(&self.cell_hosts, cell_id).send_replace(Some(host));
        Ok(())
    }

    pub(super) fn start_turn_worker(
        &self,
        exec: ExecContext,
        router: Arc<ToolRouter>,
        step_context: Arc<StepContext>,
        tracker: SharedTurnDiffTracker,
    ) -> CodeModeDispatchWorker {
        let turn_id = exec.turn.sub_id.clone();
        let tool_runtime =
            ToolCallRuntime::new(router, Arc::clone(&exec.session), step_context, tracker);
        let host = Arc::new(CoreTurnHost { exec, tool_runtime });
        let host_id = self.next_host_id.fetch_add(1, Ordering::Relaxed);
        let mut turn_hosts = match self.turn_hosts.lock() {
            Ok(turn_hosts) => turn_hosts,
            Err(poisoned) => poisoned.into_inner(),
        };
        turn_hosts.insert(turn_id.clone(), (host_id, host));
        CodeModeDispatchWorker {
            turn_hosts: Arc::clone(&self.turn_hosts),
            turn_id,
            host_id,
        }
    }

    pub(super) fn close_cell(&self, cell_id: &CellId) {
        remove_cell_host(&self.cell_hosts, cell_id);
    }
}

pub(crate) struct CodeModeDispatchWorker {
    turn_hosts: TurnHostRegistry,
    turn_id: String,
    host_id: u64,
}

impl Drop for CodeModeDispatchWorker {
    fn drop(&mut self) {
        let mut turn_hosts = match self.turn_hosts.lock() {
            Ok(turn_hosts) => turn_hosts,
            Err(poisoned) => poisoned.into_inner(),
        };
        if turn_hosts
            .get(&self.turn_id)
            .is_some_and(|(host_id, _)| *host_id == self.host_id)
        {
            turn_hosts.remove(&self.turn_id);
        }
    }
}

fn cell_host(
    cell_hosts: &Mutex<HashMap<CellId, watch::Sender<Option<Arc<CoreTurnHost>>>>>,
    cell_id: &CellId,
) -> watch::Sender<Option<Arc<CoreTurnHost>>> {
    let mut cell_hosts = match cell_hosts.lock() {
        Ok(cell_hosts) => cell_hosts,
        Err(poisoned) => poisoned.into_inner(),
    };
    cell_hosts
        .entry(cell_id.clone())
        .or_insert_with(|| watch::channel(None).0)
        .clone()
}

fn remove_cell_host(
    cell_hosts: &Mutex<HashMap<CellId, watch::Sender<Option<Arc<CoreTurnHost>>>>>,
    cell_id: &CellId,
) {
    let mut cell_hosts = match cell_hosts.lock() {
        Ok(cell_hosts) => cell_hosts,
        Err(poisoned) => poisoned.into_inner(),
    };
    cell_hosts.remove(cell_id);
}

async fn wait_for_cell_host(
    cell_hosts: &Mutex<HashMap<CellId, watch::Sender<Option<Arc<CoreTurnHost>>>>>,
    cell_id: &CellId,
    cancellation_token: &CancellationToken,
) -> Result<Arc<CoreTurnHost>, String> {
    if cancellation_token.is_cancelled() {
        return Err("code mode nested dispatch cancelled".to_string());
    }
    let mut host_rx = cell_host(cell_hosts, cell_id).subscribe();
    loop {
        if let Some(host) = host_rx.borrow_and_update().as_ref() {
            return Ok(Arc::clone(host));
        }
        tokio::select! {
            changed = host_rx.changed() => {
                if changed.is_err() {
                    return Err("code mode cell dispatch host closed".to_string());
                }
            }
            _ = cancellation_token.cancelled() => {
                return Err("code mode nested dispatch cancelled".to_string());
            },
        }
    }
}

impl CodeModeSessionDelegate for CodeModeDispatchBroker {
    fn invoke_tool<'a>(
        &'a self,
        invocation: CodeModeNestedToolCall,
        cancellation_token: CancellationToken,
    ) -> ToolInvocationFuture<'a> {
        Box::pin(async move {
            let host =
                wait_for_cell_host(&self.cell_hosts, &invocation.cell_id, &cancellation_token)
                    .await?;
            tokio::select! {
                response = host.invoke_tool(invocation, cancellation_token.clone()) => response,
                _ = cancellation_token.cancelled() => {
                    Err("code mode nested tool call cancelled".to_string())
                }
            }
        })
    }

    fn notify<'a>(
        &'a self,
        call_id: String,
        cell_id: CellId,
        text: String,
        cancellation_token: CancellationToken,
    ) -> NotificationFuture<'a> {
        Box::pin(async move {
            let host = wait_for_cell_host(&self.cell_hosts, &cell_id, &cancellation_token).await?;
            tokio::select! {
                response = host.notify(call_id, cell_id, text) => response,
                _ = cancellation_token.cancelled() => {
                    Err("code mode notification cancelled".to_string())
                }
            }
        })
    }

    fn cell_closed(&self, cell_id: &CellId) {
        self.close_cell(cell_id);
    }
}

struct CoreTurnHost {
    exec: ExecContext,
    tool_runtime: ToolCallRuntime,
}

impl CoreTurnHost {
    async fn invoke_tool(
        &self,
        invocation: CodeModeNestedToolCall,
        cancellation_token: CancellationToken,
    ) -> Result<JsonValue, String> {
        call_nested_tool(
            self.exec.clone(),
            self.tool_runtime.clone(),
            invocation,
            cancellation_token,
        )
        .await
        .map_err(|error| error.to_string())
    }

    async fn notify(&self, call_id: String, cell_id: CellId, text: String) -> Result<(), String> {
        if text.trim().is_empty() {
            return Ok(());
        }
        self.exec
            .session
            .inject_if_running(vec![ResponseItem::CustomToolCallOutput {
                id: None,
                call_id,
                name: Some(PUBLIC_TOOL_NAME.to_string()),
                output: FunctionCallOutputPayload::from_text(text),
                internal_chat_message_metadata_passthrough: None,
            }])
            .await
            .map_err(|_| {
                format!("failed to inject exec notify message for cell {cell_id}: no active turn")
            })
    }
}
