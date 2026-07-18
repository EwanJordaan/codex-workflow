use std::collections::HashSet;
use std::path::Path;
use std::path::PathBuf;

use crate::app_event::AppEvent;
use crate::bottom_pane::SelectionItem;
use crate::bottom_pane::SelectionViewParams;

use super::ChatWidget;

#[derive(Clone, Debug, PartialEq, Eq)]
struct SavedWorkflow {
    name: String,
    path: PathBuf,
}

impl ChatWidget {
    pub(super) fn open_workflows_menu(&mut self) {
        let cwd = self
            .current_cwd
            .as_deref()
            .unwrap_or(self.config.cwd.as_path());
        let workflows = discover_saved_workflows(cwd, &self.config.codex_home);
        let mut items = vec![SelectionItem {
            name: "Managed runs".to_string(),
            description: Some("View active, completed, and terminated workflow runs".to_string()),
            actions: vec![Box::new(|tx| {
                tx.send(AppEvent::InspectWorkflowRuns);
            })],
            dismiss_on_select: true,
            ..Default::default()
        }];
        items.extend(
            workflows
                .into_iter()
                .map(|workflow| {
                    let path = workflow.path.clone();
                    SelectionItem {
                        name: workflow.name,
                        description: Some(workflow.path.display().to_string()),
                        actions: vec![Box::new(move |tx| {
                            tx.send(AppEvent::RunSavedWorkflow { path: path.clone() });
                        })],
                        dismiss_on_select: true,
                        ..Default::default()
                    }
                })
                .collect::<Vec<_>>(),
        );

        self.bottom_pane.show_selection_view(SelectionViewParams {
            title: Some("Saved workflows".to_string()),
            subtitle: Some("Inspect managed runs or launch a saved workflow.".to_string()),
            items,
            ..Default::default()
        });
        self.request_redraw();
    }

    pub(crate) fn run_saved_workflow(&mut self, path: PathBuf, args: Option<String>) {
        let prompt = saved_workflow_prompt(&path, args.as_deref());
        self.submit_user_message(prompt.into());
    }

    pub(crate) fn inspect_workflow_runs(&mut self) {
        self.submit_user_message(
            "Call `list_workflows` and present the managed workflow runs with their IDs, names, statuses, and elapsed times. Do not infer runs from conversation history."
                .to_string()
                .into(),
        );
    }

    pub(super) fn run_saved_workflow_by_name(&mut self, input: &str) {
        let trimmed = input.trim();
        if matches!(trimmed, "list" | "status") {
            self.inspect_workflow_runs();
            return;
        }
        if let Some(run_id) = trimmed
            .strip_prefix("stop ")
            .or_else(|| trimmed.strip_prefix("terminate "))
            .map(str::trim)
            .filter(|run_id| !run_id.is_empty())
        {
            self.submit_user_message(
                format!(
                    "Call `control_workflow` for managed run `{run_id}` with action `terminate`, then report its resulting status."
                )
                .into(),
            );
            return;
        }
        let (name, args) = trimmed
            .split_once(char::is_whitespace)
            .map_or((trimmed, None), |(name, args)| {
                (name, Some(args.to_string()))
            });
        if name.is_empty() {
            self.open_workflows_menu();
            return;
        }

        let cwd = self
            .current_cwd
            .as_deref()
            .unwrap_or(self.config.cwd.as_path());
        let Some(workflow) = discover_saved_workflows(cwd, &self.config.codex_home)
            .into_iter()
            .find(|workflow| workflow.name == name)
        else {
            self.add_error_message(format!(
                "Saved workflow `{name}` was not found in project or personal workflow scopes."
            ));
            return;
        };
        self.run_saved_workflow(workflow.path, args);
    }
}

fn saved_workflow_prompt(path: &Path, args: Option<&str>) -> String {
    let path = path.to_string_lossy();
    let args = args
        .filter(|args| !args.trim().is_empty())
        .map(|args| format!(" Pass this user input as structured workflow args: {args}"))
        .unwrap_or_default();
    format!(
        "Run the saved workflow at `{path}` with `run_workflow` and wait for it to complete.{args}"
    )
}

fn discover_saved_workflows(cwd: &Path, codex_home: &Path) -> Vec<SavedWorkflow> {
    const MAX_SAVED_WORKFLOWS: usize = 100;

    let relative_root = Path::new(".codex").join("workflows");
    let mut roots = cwd
        .ancestors()
        .map(|ancestor| ancestor.join(&relative_root))
        .collect::<Vec<_>>();
    roots.push(codex_home.join("workflows"));

    let current_root = cwd.join(&relative_root);
    let mut names = HashSet::new();
    let mut workflows = Vec::new();
    for root in roots {
        let mut pending = vec![root.clone()];
        while let Some(directory) = pending.pop() {
            let Ok(entries) = std::fs::read_dir(&directory) else {
                continue;
            };
            for entry in entries.filter_map(Result::ok) {
                if workflows.len() == MAX_SAVED_WORKFLOWS {
                    break;
                }
                let absolute_path = entry.path();
                let Ok(file_type) = entry.file_type() else {
                    continue;
                };
                if file_type.is_dir() {
                    pending.push(absolute_path);
                    continue;
                }
                let Some(extension) = absolute_path.extension().and_then(|value| value.to_str())
                else {
                    continue;
                };
                if !file_type.is_file() || !matches!(extension, "js" | "ts") {
                    continue;
                }
                let Ok(relative_path) = absolute_path.strip_prefix(&root) else {
                    continue;
                };
                let name = relative_path
                    .with_extension("")
                    .to_string_lossy()
                    .replace('\\', "/");
                if name.is_empty() || !names.insert(name.clone()) {
                    continue;
                }
                let path = if root == current_root {
                    relative_root.join(relative_path)
                } else {
                    absolute_path
                };
                workflows.push(SavedWorkflow { name, path });
            }
        }
        if workflows.len() == MAX_SAVED_WORKFLOWS {
            break;
        }
    }
    workflows.sort_by(|left, right| left.name.cmp(&right.name));
    workflows
}

#[cfg(test)]
#[path = "workflows_tests.rs"]
mod tests;
