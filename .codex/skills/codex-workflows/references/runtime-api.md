# Codex workflow runtime API

## Contents

- File contract
- Globals
- Helper API
- Launch tools
- Patterns
- Errors and limits

## File contract

Save project workflows under `.codex/workflows/` with a `.ts` or `.js` extension. The first executable declaration must be a literal metadata export:

```ts
export const meta = {
  name: "unique-kebab-case-name",
  description: "One sentence describing the orchestration",
};
```

Both fields must be non-empty strings. The MVP accepts a TypeScript file but executes it directly as JavaScript. Do not use interfaces, enums, type-only imports, parameter annotations, `as` assertions, or other syntax V8 cannot execute.

The rest of the file is wrapped in an async function, so top-level `await` and a top-level `return` are supported. Imports are not supported. The returned value must be JSON-serializable.

## Globals

### `args`

The structured JSON value passed to `run_workflow`. It is `undefined` when omitted.

### `meta`

The validated metadata object declared at the top of the file.

## Helper API

### `agent(prompt, options?)`

Spawn one subagent and wait for its final message.

```ts
const result = await agent("Inspect the parser for edge cases.", {
  label: "parser-review",
  forkTurns: "all",
  model: "optional-model-id",
  reasoningEffort: "high",
  schema: {
    type: "object",
    required: ["findings"],
    properties: {
      findings: { type: "array", items: { type: "string" } },
    },
  },
});
```

Options:

- `label`: readable task-name seed. The runtime sanitizes and uniquifies it.
- `forkTurns`: `"none"`, `"all"`, or a positive integer string. Defaults to `"all"`.
- `model`: optional model override.
- `reasoningEffort`: optional reasoning-effort override.
- `schema`: JSON Schema for structured output. The runtime instructs the agent to return only matching JSON, parses the final message, and validates the supported subset: `type`, `required`, `properties`, and `items`.

Without `schema`, `agent` returns the final assistant message as a string. An interrupted, errored, shut down, or missing agent rejects the promise.

### `parallel(tasks, options?)`

Run an array of zero-argument async callbacks through the same bounded scheduler as `pipeline`.

```ts
const results = await parallel(
  [
    () => agent("Review correctness", { label: "correctness" }),
    () => agent("Review tests", { label: "tests" }),
  ],
  { concurrency: 2 },
);
```

Pass callbacks, not already-started promises, so the concurrency bound can take effect.

### `pipeline(items, worker, options?)`

Map an async worker over an array with bounded concurrency while preserving input order.

```ts
const results = await pipeline(paths, async (path, index) => {
  return agent(`Inspect ${path}`, { label: `inspect-${index}` });
}, { concurrency: 4 });
```

`concurrency` defaults to 4 and must be an integer from 1 through 4.

### `phase(name, work)`

Run a named stage. The MVP validates the name and invokes `work`; phase-specific UI and token accounting are future work.

## Launch tools

Call:

```json
{
  "path": ".codex/workflows/review-files.ts",
  "args": ["src/a.rs", "src/b.rs"],
  "yield_time_ms": 1000,
  "max_output_tokens": 10000
}
```

`run_workflow` only accepts files below the current workspace's `.codex/workflows/` directory. It rejects traversal, symlinks that escape the directory, files larger than 128 KiB, unsupported extensions, invalid metadata, and TypeScript syntax that V8 cannot parse.

If the call yields, use:

```json
{
  "cell_id": "1",
  "yield_time_ms": 30000,
  "max_output_tokens": 10000
}
```

with `wait_workflow`. Set `terminate` to `true` to stop the runtime cell. Stopping a cell does not currently guarantee cancellation of every already-spawned subagent; inspect live agents afterward.

## Patterns

### Discover, fan out, synthesize

Use one structured agent to discover bounded work items, a pipeline to process them, and a final agent to consolidate results.

### Independent review

Use `parallel` for independent opinions, then pass all returned strings to a final verifier. Do not let reviewers see each other's output if independence matters.

### Bounded iteration

Use a `for` loop with an explicit maximum round count and an early-stop condition. Never use an unbounded `while` loop.

## Errors and limits

- Maximum workflow file size: 128 KiB.
- Maximum agents per run: 64.
- Maximum pipeline concurrency: 4.
- No imports, direct filesystem API, shell API, network API, or mid-run user input in the script.
- Workflow state and completed-agent caching do not survive a process restart.
- The MVP has no `/workflows` UI, save dialog, cost view, pause/resume UI, or bundled deep-research workflow.
