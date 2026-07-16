---
name: codex-workflows
description: Create, inspect, and run reusable Codex multi-agent workflow scripts under .codex/workflows. Use when a task should orchestrate subagents from a TypeScript file, fan work out in parallel, collect intermediate results in script variables, or launch an existing workflow with run_workflow.
---

# Codex workflows

Create project workflows in `.codex/workflows/<name>.ts`. Keep the file valid JavaScript as well as TypeScript: the MVP runtime executes ECMAScript modules directly and does not erase TypeScript-only syntax.

Read [runtime-api.md](references/runtime-api.md) before writing or changing a workflow. Read [spec.md](references/spec.md) when comparing this MVP with Claude Code or deciding whether a requested behavior is supported.

## Create and run a workflow

1. Decide whether scripting adds value. Use a workflow for repeatable fan-out, branching, loops, or independent verification. Use ordinary subagent calls for one or two simple delegations.
2. Create `.codex/workflows/` if it does not exist.
3. Write `<name>.ts` with the required literal `meta` export first, followed by a top-level body that uses `agent`, `parallel`, `pipeline`, and `phase`.
4. Keep orchestration in the script. Put filesystem access, shell commands, web access, and edits in agent prompts.
5. Bound every loop and choose a pipeline concurrency no greater than 4. The runtime also caps a run at 64 agents.
6. Return the final serializable value from the top-level body.
7. Call `run_workflow` with the project-relative path and structured `args` when needed.
8. If the result says the workflow is still running, call `wait_workflow` with its `cell_id` until it completes. Do not busy-poll.
9. Inspect agent errors and the final value. Revise the workflow rather than silently reproducing failed work in the parent agent.

## Minimal shape

```ts
export const meta = {
  name: "review-files",
  description: "Review a list of files and consolidate the findings",
};

const files = Array.isArray(args) ? args : [];
const reviews = await phase("review", () =>
  pipeline(
    files,
    (file) => agent(`Review ${file} for correctness bugs.`, { label: file }),
    { concurrency: 4 },
  ),
);

return agent(
  `Deduplicate and rank these findings:\n${JSON.stringify(reviews)}`,
  { label: "synthesize" },
);
```

Treat workflow files as code: review them before launching because their agents inherit the current session's capabilities and can edit the workspace.
