use std::path::Component;
use std::path::Path;

use codex_file_system::ExecutorFileSystem;
use codex_file_system::FileSystemSandboxContext;
use codex_utils_path_uri::PathUri;
use serde_json::Value;

const MAX_WORKFLOW_BYTES: u64 = 128 * 1024;
const MAX_INLINE_WORKFLOW_BYTES: usize = 4 * 1024;

pub(super) fn compile_inline_workflow(
    source: &str,
    args: Option<&Value>,
    agent_api: &codex_code_mode::WorkflowAgentApi,
) -> Result<String, String> {
    if source.len() > MAX_INLINE_WORKFLOW_BYTES {
        return Err(format!(
            "inline workflow source exceeds the {MAX_INLINE_WORKFLOW_BYTES}-byte limit; save larger workflows under .codex/workflows"
        ));
    }
    codex_code_mode::compile_workflow_source(source, args, agent_api)
}

pub(super) async fn load_and_compile_workflow(
    file_system: &dyn ExecutorFileSystem,
    cwd: &PathUri,
    relative_path: &str,
    args: Option<&Value>,
    agent_api: &codex_code_mode::WorkflowAgentApi,
    personal_workflow_root: Option<&PathUri>,
    sandbox: Option<&FileSystemSandboxContext>,
) -> Result<String, String> {
    let path = resolve_workflow_path(
        file_system,
        cwd,
        relative_path,
        personal_workflow_root,
        sandbox,
    )
    .await?;
    let metadata = file_system
        .get_metadata(&path, sandbox)
        .await
        .map_err(|err| format!("failed to read workflow metadata: {err}"))?;
    if metadata.size > MAX_WORKFLOW_BYTES {
        return Err(format!(
            "workflow file exceeds the {MAX_WORKFLOW_BYTES}-byte limit"
        ));
    }
    let source = file_system
        .read_file_text(&path, sandbox)
        .await
        .map_err(|err| format!("failed to read workflow as UTF-8: {err}"))?;
    if source.len() as u64 > MAX_WORKFLOW_BYTES {
        return Err(format!(
            "workflow file exceeds the {MAX_WORKFLOW_BYTES}-byte limit"
        ));
    }
    codex_code_mode::compile_workflow_source(&source, args, agent_api)
}

pub(super) fn workflow_agent_api(
    tools: &[codex_code_mode::ToolDefinition],
) -> Result<codex_code_mode::WorkflowAgentApi, String> {
    let find = |name: &str, namespace: Option<&str>| {
        tools
            .iter()
            .find(|tool| {
                tool.tool_name.name == name
                    && namespace.is_none_or(|namespace| {
                        tool.tool_name.namespace.as_deref() == Some(namespace)
                    })
            })
            .map(|tool| tool.name.clone())
    };
    if let (Some(spawn_tool), Some(wait_tool), Some(close_tool)) = (
        find("spawn_agent", Some("multi_agent_v1")),
        find("wait_agent", Some("multi_agent_v1")),
        find("close_agent", Some("multi_agent_v1")),
    ) {
        return Ok(codex_code_mode::WorkflowAgentApi::V1 {
            spawn_tool,
            wait_tool,
            close_tool,
        });
    }

    let v2_tool = |name: &str| {
        tools
            .iter()
            .find(|tool| {
                tool.tool_name.name == name
                    && tool.tool_name.namespace.as_deref() != Some("multi_agent_v1")
            })
            .map(|tool| tool.name.clone())
    };
    match (
        v2_tool("spawn_agent"),
        v2_tool("wait_agent"),
        v2_tool("list_agents"),
        v2_tool("interrupt_agent"),
    ) {
        (Some(spawn_tool), Some(wait_tool), Some(list_tool), Some(interrupt_tool)) => {
            Ok(codex_code_mode::WorkflowAgentApi::V2 {
                spawn_tool,
                wait_tool,
                list_tool,
                interrupt_tool,
            })
        }
        _ => {
            Err("workflow agent helpers require a complete collaboration tool surface".to_string())
        }
    }
}

async fn resolve_workflow_path(
    file_system: &dyn ExecutorFileSystem,
    cwd: &PathUri,
    relative_path: &str,
    personal_workflow_root: Option<&PathUri>,
    sandbox: Option<&FileSystemSandboxContext>,
) -> Result<PathUri, String> {
    let native_path = Path::new(relative_path);
    if native_path
        .components()
        .any(|component| matches!(component, Component::ParentDir))
    {
        return Err("workflow path cannot contain `..`".to_string());
    }
    let extension = native_path
        .extension()
        .and_then(|extension| extension.to_str())
        .unwrap_or_default();
    if !matches!(extension, "ts" | "js") {
        return Err("workflow path must end in .ts or .js".to_string());
    }

    let candidate = if native_path.is_absolute() {
        PathUri::from_host_native_path(native_path)
            .map_err(|err| format!("failed to resolve workflow path: {err}"))?
    } else {
        cwd.join(relative_path)
            .map_err(|err| format!("failed to resolve workflow path: {err}"))?
    };
    let candidate = file_system
        .canonicalize(&candidate, sandbox)
        .await
        .map_err(|err| format!("failed to resolve workflow path: {err}"))?;

    let mut scope = Some(cwd.clone());
    while let Some(directory) = scope {
        let codex_root = directory
            .join(".codex")
            .map_err(|err| format!("failed to resolve .codex directory: {err}"))?;
        if let Ok(canonical_codex_root) = file_system.canonicalize(&codex_root, sandbox).await
            && let Ok(canonical_directory) = file_system.canonicalize(&directory, sandbox).await
            && canonical_codex_root.starts_with(&canonical_directory)
        {
            let workflow_root = directory
                .join(".codex/workflows")
                .map_err(|err| format!("failed to resolve workflow directory: {err}"))?;
            if let Ok(canonical_root) = file_system.canonicalize(&workflow_root, sandbox).await
                && canonical_root.starts_with(&canonical_codex_root)
                && candidate.starts_with(&canonical_root)
            {
                return Ok(candidate);
            }
        }
        scope = directory.parent();
    }

    if let Some(personal_workflow_root) = personal_workflow_root
        && let Ok(canonical_root) = file_system
            .canonicalize(personal_workflow_root, sandbox)
            .await
        && candidate.starts_with(&canonical_root)
    {
        return Ok(candidate);
    }
    Err("workflow path must stay below a project or personal workflows directory".to_string())
}
