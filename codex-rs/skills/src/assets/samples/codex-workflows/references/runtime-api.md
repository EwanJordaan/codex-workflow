# Codex workflow runtime API

## Contents

- File contract
- Globals
- Helper API
- Launch tools
- Patterns and limits

## File contract

Save project workflows under `.codex/workflows/` with a `.ts` or `.js` extension. Start with:

```ts
export const meta = {
  name: "unique-kebab-case-name",
  description: "One sentence describing the orchestration",
};
```

Both fields must be non-empty strings. The MVP accepts TypeScript files but executes them directly as JavaScript. Do not use interfaces, enums, type-only imports, parameter annotations, `as` assertions, or other TypeScript-only syntax.

The body is wrapped in an async function, so top-level `await` and `return` work. Imports do not. Return a JSON-serializable value.

## Globals

- `args`: structured JSON passed to `run_workflow`, or `undefined` when omitted.
- `meta`: the file's validated metadata object.

## Helper API

### `agent(prompt, options?)`

Spawn one subagent and wait for its final message. Options are:

- `label`: readable annotation prepended to the subagent prompt.
- `forkTurns`: `"none"`, `"all"`, or a positive integer string. The MVP maps `none` to a fresh agent and other values to inherited context.
- `model`: optional model override.
- `reasoningEffort`: optional reasoning-effort override.
- `schema`: JSON Schema for structured output. The agent must return JSON; the runtime parses it and validates `type`, `required`, `properties`, and `items`.
- `timeoutMs`: per-agent deadline from 1,000 through 1,800,000 milliseconds; defaults to 30 minutes.

Without `schema`, the result is the final assistant message string. Errored, shut down, missing, or timed-out agents reject the promise. Composed agent prompts are capped at 9 KiB.
Model or reasoning-effort overrides use a fresh agent by default and require `forkTurns: "none"` when `forkTurns` is explicit.

### `parallel(tasks, options?)`

Run zero-argument async callbacks with bounded concurrency:

```ts
const results = await parallel([
  () => agent("Review correctness", { label: "correctness" }),
  () => agent("Review tests", { label: "tests" }),
], { concurrency: 2 });
```

Pass callbacks, not already-started promises, so the concurrency bound applies.

### `pipeline(items, worker, options?)`

Map an async worker over an array with bounded concurrency while preserving input order. `concurrency` defaults to 4 and must be from 1 through 4.

### `phase(name, work)`

Run a named stage. The MVP validates the name and invokes `work`; phase-specific UI and accounting are future work.

## Launch tools

Launch with:

```json
{
  "path": ".codex/workflows/review-files.ts",
  "args": ["src/a.rs", "src/b.rs"],
  "yield_time_ms": 1000,
  "max_output_tokens": 10000
}
```

`run_workflow` rejects traversal, symlinks escaping `.codex/workflows`, files over 128 KiB, unsupported extensions, invalid metadata, and syntax V8 cannot parse.

Workflow output limits cannot exceed 10,000 tokens for either launch or wait calls.

If it yields, call `wait_workflow` with `cell_id`, optional `yield_time_ms`, optional `max_tokens`, and optional `terminate`. Do not busy-poll. Terminating the V8 cell does not guarantee cancellation of every already-spawned agent; inspect live agents afterward.

## Patterns and limits

- Discover bounded items with one structured agent, process with `pipeline`, then synthesize with a final agent.
- Use `parallel` for independent opinions, then give all outputs to a verifier.
- Bound iterative loops with a maximum round count and an early-stop condition.
- Maximum file: 128 KiB. Maximum agents: 64. Maximum helper concurrency: 4.
- Scripts have no imports, direct filesystem, shell, network, console, or mid-run user input.
- The MVP has no `/workflows` UI, durable run cache, cost view, save dialog, or bundled workflow.
