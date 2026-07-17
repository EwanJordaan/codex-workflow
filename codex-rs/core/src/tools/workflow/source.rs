use std::path::Component;
use std::path::Path;

use codex_file_system::ExecutorFileSystem;
use codex_file_system::FileSystemSandboxContext;
use codex_utils_path_uri::PathUri;
use serde_json::Value;

const MAX_WORKFLOW_BYTES: u64 = 128 * 1024;

pub(super) fn compile_inline_workflow(
    source: &str,
    args: Option<&Value>,
) -> Result<String, String> {
    if source.len() as u64 > MAX_WORKFLOW_BYTES {
        return Err(format!(
            "workflow source exceeds the {MAX_WORKFLOW_BYTES}-byte limit"
        ));
    }
    codex_code_mode::compile_workflow_source(source, args)
}

pub(super) async fn load_and_compile_workflow(
    file_system: &dyn ExecutorFileSystem,
    cwd: &PathUri,
    relative_path: &str,
    args: Option<&Value>,
    sandbox: Option<&FileSystemSandboxContext>,
) -> Result<String, String> {
    let path = resolve_workflow_path(file_system, cwd, relative_path, sandbox).await?;
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
    codex_code_mode::compile_workflow_source(&source, args)
}

async fn resolve_workflow_path(
    file_system: &dyn ExecutorFileSystem,
    cwd: &PathUri,
    relative_path: &str,
    sandbox: Option<&FileSystemSandboxContext>,
) -> Result<PathUri, String> {
    let native_path = Path::new(relative_path);
    if native_path.is_absolute()
        || native_path
            .components()
            .any(|component| !matches!(component, Component::Normal(_) | Component::CurDir))
    {
        return Err("workflow path must be project-relative and cannot contain `..`".to_string());
    }
    let extension = native_path
        .extension()
        .and_then(|extension| extension.to_str())
        .unwrap_or_default();
    if !matches!(extension, "ts" | "js") {
        return Err("workflow path must end in .ts or .js".to_string());
    }

    let canonical_cwd = file_system
        .canonicalize(cwd, sandbox)
        .await
        .map_err(|err| format!("failed to resolve project directory: {err}"))?;
    let codex_root = cwd
        .join(".codex")
        .map_err(|err| format!("failed to resolve .codex directory: {err}"))?;
    let canonical_codex_root = file_system
        .canonicalize(&codex_root, sandbox)
        .await
        .map_err(|err| format!("failed to open .codex directory {codex_root}: {err}"))?;
    if !canonical_codex_root.starts_with(&canonical_cwd) {
        return Err(".codex directory must stay below the project directory".to_string());
    }
    let workflow_root = cwd
        .join(".codex/workflows")
        .map_err(|err| format!("failed to resolve workflow directory: {err}"))?;
    let canonical_root = file_system
        .canonicalize(&workflow_root, sandbox)
        .await
        .map_err(|err| format!("failed to open workflow directory {workflow_root}: {err}"))?;
    if !canonical_root.starts_with(&canonical_codex_root) {
        return Err("workflow directory must stay below the project .codex directory".to_string());
    }
    let candidate = cwd
        .join(relative_path)
        .map_err(|err| format!("failed to resolve workflow path: {err}"))?;
    let candidate = file_system
        .canonicalize(&candidate, sandbox)
        .await
        .map_err(|err| format!("failed to resolve workflow path: {err}"))?;
    if !candidate.starts_with(&canonical_root) {
        return Err("workflow path must stay below .codex/workflows".to_string());
    }
    Ok(candidate)
}
