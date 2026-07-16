# Codex dynamic workflows MVP specification

## Contents

- Research baseline
- Required behavior
- Design
- Compatibility
- Validation

## Research baseline

Primary sources:

- [Claude Code Dynamic Workflows](https://code.claude.com/docs/en/workflows)
- [Claude TypeScript Agent SDK Workflow tool](https://code.claude.com/docs/en/agent-sdk/typescript#workflow)
- [Claude Code subagents](https://code.claude.com/docs/en/sub-agents)

Claude Code 2.1.154 introduced workflows as JavaScript orchestration scripts executed outside the conversation. A script owns branching, loops, and intermediate values so the conversation receives one consolidated result. Saved project and user scripts live in `.claude/workflows/` and `~/.claude/workflows/`; nearest project definitions take precedence. Scripts begin with a literal `meta` export, support top-level `await`, receive structured global `args`, and call `agent`, `parallel`, `pipeline`, and `phase`.

Claude's public `Workflow` tool accepts inline source, a saved name, a script path, structured arguments, or a prior run ID. Runs start in the background and return task/run identifiers plus script and transcript paths. Its `/workflows` UI reports phases, agents, tokens, status, and elapsed time and supports pause/resume, stop, restart, inspection, and saving. Completed unchanged agents can be cached when a run resumes in the same session.

Claude prevents direct filesystem and shell access from scripts, disallows mid-run user input, caps concurrency at 16 and total agents at 1,000, applies permission-mode launch consent, warns on unusually large runs, permits per-stage model routing, bundles `/deep-research`, and can generate workflows through direct requests or `ultracode` effort.

## Required behavior

The Codex MVP must:

1. Ship a discoverable `codex-workflows` system skill with an exact create/run/wait guide.
2. Load project-relative `.ts` or `.js` files only from `.codex/workflows/`.
3. Require non-empty literal `meta.name` and `meta.description`, cap source at 128 KiB, and accept JavaScript-compatible TypeScript.
4. Expose structured `args`, top-level `await`/`return`, and `agent`, `parallel`, `pipeline`, and `phase`.
5. Launch via model-callable `run_workflow`; return or yield a V8 cell; continue or terminate through `wait_workflow`.
6. Keep intermediate subagent messages in script variables and return one serialized `{ meta, result }` value.
7. Produce actionable path, metadata, syntax, limit, schema, and agent-lifecycle errors.

## Design

Keep workflow parsing and runtime-prelude compilation in `codex-code-mode`, alongside the isolated V8 runtime it targets, rather than adding that reusable logic to `codex-core`. In multi-agent v2 sessions, register the legacy ID-returning spawn/wait/close implementations as hidden dispatch primitives. The core launcher explicitly enables only those definitions in its cell, captures them privately, removes raw tool globals, and executes the user body in a separate async function.

Each cell binds to its originating turn's dispatch host for its full lifetime, including after a yield. `run_workflow` requires exactly one turn environment and reads through that environment's filesystem abstraction, so local and remote executors share the same path. It canonicalizes the project, `.codex`, workflow root, and candidate under the active sandbox policy, rejects escapes, validates metadata and the post-read size, injects the runtime prelude and structured args, and executes a traced cell. Existing Codex routing, hooks, permissions, environment selection, depth limits, and agent-control limits remain authoritative.

The runtime caps helper concurrency at 4 and total spawned agents at 64. `agent` returns the final `AgentStatus::Completed` message and optionally parses and validates a supported JSON Schema subset. `pipeline` preserves order, `parallel` schedules callbacks through `pipeline`, and `phase` is a named semantic boundary without dedicated UI in the MVP.

## Compatibility

| Capability | Claude | Codex MVP |
| --- | --- | --- |
| Location | Project and user | Project `.codex/workflows` |
| Source | JavaScript | JS-compatible TypeScript/JavaScript |
| Core helpers | Four helpers | Four helpers |
| Structured results | Yes | JSON Schema subset |
| Background | Task/run UI | Yielded cell and wait tool |
| Concurrency / total | 16 / 1,000 | 4 / 64 |
| Resume cache | Same session | No completed-call cache |
| Progress/cost UI | Yes | No |
| Saved commands | Yes | Skill-directed path launch |
| Consent | Workflow-specific | Existing Codex permissions |
| Built-in workflow | Deep research | None |

## Validation

Unit-test metadata parsing, args injection, path containment, extension/size limits, helper wrapping, and the new exposure semantics. Execute a compiled no-agent workflow in the V8 runtime. Add a core integration test that launches a file, mocks one subagent, receives its final message through private spawn/wait, and returns the consolidated result. Validate the skill, format, run scoped lint fixes and core tests, then obtain two independent full-diff reviews and resolve actionable findings.
