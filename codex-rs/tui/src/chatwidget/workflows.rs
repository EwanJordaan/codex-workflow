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
        let workflows = discover_saved_workflows(cwd);
        if workflows.is_empty() {
            self.add_info_message(
                "No saved workflows found.".to_string(),
                Some(
                    "Create one in .codex/workflows/<name>.ts, then run /workflows again."
                        .to_string(),
                ),
            );
            return;
        }

        let items = workflows
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
            .collect();

        self.bottom_pane.show_selection_view(SelectionViewParams {
            title: Some("Saved workflows".to_string()),
            subtitle: Some("Select a workflow to run it in this session.".to_string()),
            items,
            ..Default::default()
        });
        self.request_redraw();
    }

    pub(crate) fn run_saved_workflow(&mut self, path: PathBuf, args: Option<String>) {
        let prompt = saved_workflow_prompt(&path, args.as_deref());
        self.submit_user_message(prompt.into());
    }

    pub(super) fn run_saved_workflow_by_name(&mut self, input: &str) {
        let (name, args) = input
            .trim()
            .split_once(char::is_whitespace)
            .map_or((input.trim(), None), |(name, args)| {
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
        let Some(workflow) = discover_saved_workflows(cwd)
            .into_iter()
            .find(|workflow| workflow.name == name)
        else {
            self.add_error_message(format!(
                "Saved workflow `{name}` was not found under .codex/workflows."
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

fn discover_saved_workflows(cwd: &Path) -> Vec<SavedWorkflow> {
    let relative_root = Path::new(".codex").join("workflows");
    let root = cwd.join(&relative_root);
    let Ok(entries) = std::fs::read_dir(root) else {
        return Vec::new();
    };
    let mut workflows = entries
        .filter_map(Result::ok)
        .filter_map(|entry| {
            let absolute_path = entry.path();
            let extension = absolute_path.extension()?.to_str()?;
            if !matches!(extension, "js" | "ts") || !entry.file_type().ok()?.is_file() {
                return None;
            }
            let file_name = absolute_path.file_name()?;
            let name = absolute_path.file_stem()?.to_str()?.to_string();
            let path = relative_root.join(file_name);
            (!name.is_empty()).then_some(SavedWorkflow { name, path })
        })
        .collect::<Vec<_>>();
    workflows.sort_by(|left, right| left.name.cmp(&right.name));
    workflows
}

#[cfg(test)]
#[path = "workflows_tests.rs"]
mod tests;
