use serde_json::Value;

const META_PREFIX: &str = "export const meta";

#[derive(Debug, PartialEq, Eq)]
struct WorkflowMeta {
    name: String,
    description: String,
}

/// Validates a workflow file and wraps its body in the isolated code-mode runtime prelude.
pub fn compile_workflow_source(source: &str, args: Option<&Value>) -> Result<String, String> {
    let (body, meta) = parse_workflow_source(source)?;
    let args = args.map_or_else(
        || Ok("undefined".to_string()),
        |args| {
            serde_json::to_string(args)
                .map_err(|err| format!("failed to serialize workflow args: {err}"))
        },
    )?;
    let validated_meta = serde_json::to_string(&serde_json::json!({
        "name": meta.name,
        "description": meta.description,
    }))
    .map_err(|err| format!("failed to serialize workflow metadata: {err}"))?;
    let body = serde_json::to_string(body)
        .map_err(|err| format!("failed to serialize workflow body: {err}"))?;

    Ok(format!(
        r#"const args = {args};
const meta = Object.freeze({validated_meta});
const __validatedMeta = {validated_meta};
const __workflowBody = {body};
const __workflowTools = tools;
const __workflowText = text;
const __WorkflowPromise = Promise;
const __objectEntries = Object.entries;
const __objectValues = Object.values;
const __hasOwn = Function.call.bind(Object.prototype.hasOwnProperty);
const __now = Date.now.bind(Date);
let __workflowAgentCount = 0;
let __workflowActiveAgents = 0;
const __workflowAgentWaiters = [];
const __workflowMaxAgents = 64;
const __workflowMaxConcurrency = 4;
const __workflowMaxPromptBytes = 9 * 1024;
const __workflowMaxAgentTimeMs = 30 * 60 * 1000;

for (const name of [
  "tools", "ALL_TOOLS", "text", "image", "generatedImage", "store", "load",
  "notify", "yield_control", "exit", "clearTimeout", "setTimeout",
]) {{
  delete globalThis[name];
}}

function __utf8Length(value) {{
  let length = 0;
  for (const character of value) {{
    const codePoint = character.codePointAt(0);
    length += codePoint <= 0x7f ? 1 : codePoint <= 0x7ff ? 2 : codePoint <= 0xffff ? 3 : 4;
  }}
  return length;
}}

async function __withAgentPermit(work) {{
  if (__workflowActiveAgents >= __workflowMaxConcurrency) {{
    await new __WorkflowPromise(resolve => __workflowAgentWaiters.push(resolve));
  }}
  __workflowActiveAgents += 1;
  try {{
    return await work();
  }} finally {{
    __workflowActiveAgents -= 1;
    const next = __workflowAgentWaiters.shift();
    if (next) next();
  }}
}}

function __agentStatus(status) {{
  if (status && typeof status === "object" && "completed" in status) {{
    return {{ kind: "completed", value: status.completed }};
  }}
  if (status && typeof status === "object" && "errored" in status) {{
    return {{ kind: "errored", value: status.errored }};
  }}
  return {{ kind: String(status), value: undefined }};
}}

function __validateSchema(value, schema, path = "result") {{
  if (!schema || typeof schema !== "object") return;
  const type = schema.type;
  const valid = type === undefined ||
    (type === "array" && Array.isArray(value)) ||
    (type === "object" && value !== null && typeof value === "object" && !Array.isArray(value)) ||
    (type === "string" && typeof value === "string") ||
    (type === "number" && typeof value === "number") ||
    (type === "integer" && Number.isInteger(value)) ||
    (type === "boolean" && typeof value === "boolean") ||
    (type === "null" && value === null);
  if (!valid) throw new Error(`${{path}} does not match schema type ${{type}}`);
  if (type === "object") {{
    for (const key of schema.required || []) {{
      if (!__hasOwn(value, key)) throw new Error(`${{path}} is missing required property ${{key}}`);
    }}
    for (const [key, child] of __objectEntries(schema.properties || {{}})) {{
      if (__hasOwn(value, key)) __validateSchema(value[key], child, `${{path}}.${{key}}`);
    }}
  }}
  if (type === "array" && schema.items) {{
    value.forEach((item, index) => __validateSchema(item, schema.items, `${{path}}[${{index}}]`));
  }}
}}

async function agent(prompt, options = {{}}) {{
  if (typeof prompt !== "string" || !prompt.trim()) throw new Error("agent prompt must be non-empty");
  __workflowAgentCount += 1;
  if (__workflowAgentCount > __workflowMaxAgents) throw new Error("workflow exceeded the 64-agent limit");
  const schemaInstruction = options.schema
    ? `\n\nReturn only JSON matching this schema (no markdown fences): ${{JSON.stringify(options.schema)}}`
    : "";
  const modelOverride = options.model !== undefined || options.reasoningEffort !== undefined;
  const forkTurns = options.forkTurns === undefined
    ? (modelOverride ? "none" : "all")
    : String(options.forkTurns);
  if (forkTurns !== "none" && modelOverride) {{
    throw new Error("model and reasoningEffort overrides require forkTurns: \"none\"");
  }}
  const labelPrefix = options.label === undefined
    ? ""
    : `[Workflow task: ${{String(options.label)}}]\n\n`;
  const message = labelPrefix + prompt + schemaInstruction;
  if (__utf8Length(message) > __workflowMaxPromptBytes) {{
    throw new Error("agent prompt exceeds the 9216-byte limit");
  }}
  const timeoutMs = options.timeoutMs ?? __workflowMaxAgentTimeMs;
  if (!Number.isInteger(timeoutMs) || timeoutMs < 1000 || timeoutMs > __workflowMaxAgentTimeMs) {{
    throw new Error("agent timeoutMs must be an integer from 1000 through 1800000");
  }}
  return __withAgentPermit(async () => {{
    const spawned = await __workflowTools.multi_agent_v1__spawn_agent({{
      message,
      agent_type: undefined,
      fork_context: forkTurns !== "none",
      model: options.model,
      reasoning_effort: options.reasoningEffort,
    }});
    const deadline = __now() + timeoutMs;
    while (true) {{
      const remainingMs = deadline - __now();
      if (remainingMs <= 0) {{
        await __workflowTools.multi_agent_v1__close_agent({{ target: spawned.agent_id }});
        throw new Error(`agent timed out after ${{timeoutMs}} ms`);
      }}
      const waited = await __workflowTools.multi_agent_v1__wait_agent({{
        targets: [spawned.agent_id],
        timeout_ms: Math.min(30000, Math.max(1000, remainingMs)),
      }});
      if (waited.timed_out) continue;
      const rawStatus = __objectValues(waited.status)[0];
      const status = __agentStatus(rawStatus);
      if (status.kind === "completed") {{
        const output = status.value ?? "";
        if (!options.schema) return output;
        let parsed;
        try {{ parsed = JSON.parse(output); }}
        catch (error) {{ throw new Error(`agent returned invalid JSON: ${{error}}`); }}
        __validateSchema(parsed, options.schema);
        return parsed;
      }}
      if (status.kind === "errored") throw new Error(`agent failed: ${{status.value}}`);
      throw new Error(`agent ended with status ${{status.kind}}`);
    }}
  }});
}}

async function pipeline(items, worker, options = {{}}) {{
  if (!Array.isArray(items)) throw new Error("pipeline items must be an array");
  if (typeof worker !== "function") throw new Error("pipeline worker must be a function");
  const concurrency = options.concurrency ?? __workflowMaxConcurrency;
  if (!Number.isInteger(concurrency) || concurrency < 1 || concurrency > __workflowMaxConcurrency) {{
    throw new Error("pipeline concurrency must be an integer from 1 through 4");
  }}
  const results = new Array(items.length);
  let nextIndex = 0;
  async function runWorker() {{
    while (true) {{
      const index = nextIndex++;
      if (index >= items.length) return;
      results[index] = await worker(items[index], index);
    }}
  }}
  await Promise.all(Array.from({{ length: Math.min(concurrency, items.length) }}, runWorker));
  return results;
}}

async function parallel(tasks, options = {{}}) {{
  if (!Array.isArray(tasks) || tasks.some(task => typeof task !== "function")) {{
    throw new Error("parallel tasks must be an array of functions");
  }}
  return pipeline(tasks, task => task(), options);
}}

async function phase(name, work) {{
  if (typeof name !== "string" || !name.trim()) throw new Error("phase name must be non-empty");
  if (typeof work !== "function") throw new Error("phase work must be a function");
  return work();
}}

const __AsyncFunction = (async function () {{}}).constructor;
const __workflowMain = new __AsyncFunction(
  "args", "meta", "agent", "parallel", "pipeline", "phase", __workflowBody,
);
const __workflowResult = await __workflowMain(args, meta, agent, parallel, pipeline, phase);
__workflowText(JSON.stringify({{ meta: __validatedMeta, result: __workflowResult }}, null, 2));
"#
    ))
}

fn parse_workflow_source(source: &str) -> Result<(&str, WorkflowMeta), String> {
    let source = source
        .strip_prefix('\u{feff}')
        .unwrap_or(source)
        .trim_start();
    let remainder = source.strip_prefix(META_PREFIX).ok_or_else(|| {
        "workflow must start with `export const meta = { name, description }`".to_string()
    })?;
    let remainder = remainder.trim_start();
    let remainder = remainder
        .strip_prefix('=')
        .ok_or_else(|| "workflow metadata must use `export const meta = { ... }`".to_string())?;
    let object_offset = remainder
        .find('{')
        .ok_or_else(|| "workflow metadata must be an object literal".to_string())?;
    if !remainder[..object_offset].trim().is_empty() {
        return Err("workflow metadata must be an object literal".to_string());
    }
    let object = &remainder[object_offset..];
    let object_end = find_object_end(object)?;
    let meta_source = &object[..=object_end];
    let mut body = &object[object_end + 1..];
    body = body.trim_start();
    if let Some(without_semicolon) = body.strip_prefix(';') {
        body = without_semicolon;
    }

    let name = find_string_property(meta_source, "name")?
        .ok_or_else(|| "workflow metadata requires a string `name`".to_string())?;
    let description = find_string_property(meta_source, "description")?
        .ok_or_else(|| "workflow metadata requires a string `description`".to_string())?;
    if name.trim().is_empty() || description.trim().is_empty() {
        return Err("workflow metadata name and description must be non-empty".to_string());
    }
    Ok((body, WorkflowMeta { name, description }))
}

fn find_object_end(source: &str) -> Result<usize, String> {
    let bytes = source.as_bytes();
    if bytes.first() != Some(&b'{') {
        return Err("workflow metadata must be an object literal".to_string());
    }
    let mut depth = 0usize;
    let mut index = 0usize;
    while index < bytes.len() {
        match bytes[index] {
            b'\'' | b'"' | b'`' => index = skip_string(bytes, index)?,
            b'/' if bytes.get(index + 1) == Some(&b'/') => index = skip_line_comment(bytes, index),
            b'/' if bytes.get(index + 1) == Some(&b'*') => {
                index = skip_block_comment(bytes, index)?
            }
            b'{' => depth += 1,
            b'}' => {
                depth = depth
                    .checked_sub(1)
                    .ok_or_else(|| "unbalanced workflow metadata".to_string())?;
                if depth == 0 {
                    return Ok(index);
                }
            }
            _ => {}
        }
        index += 1;
    }
    Err("unterminated workflow metadata object".to_string())
}

fn find_string_property(source: &str, property: &str) -> Result<Option<String>, String> {
    let bytes = source.as_bytes();
    let mut depth = 0usize;
    let mut index = 0usize;
    while index < bytes.len() {
        match bytes[index] {
            b'{' => depth += 1,
            b'}' => depth = depth.saturating_sub(1),
            b'/' if bytes.get(index + 1) == Some(&b'/') => index = skip_line_comment(bytes, index),
            b'/' if bytes.get(index + 1) == Some(&b'*') => {
                index = skip_block_comment(bytes, index)?
            }
            b'\'' | b'"' if depth == 1 => {
                let end = skip_string(bytes, index)?;
                let key = decode_js_string(&source[index..=end])?;
                if key == property
                    && let Some(value) = string_property_value(source, end + 1)?
                {
                    return Ok(Some(value));
                }
                index = end;
            }
            byte if depth == 1 && (byte.is_ascii_alphabetic() || byte == b'_') => {
                let start = index;
                while bytes
                    .get(index + 1)
                    .is_some_and(|byte| byte.is_ascii_alphanumeric() || *byte == b'_')
                {
                    index += 1;
                }
                if &source[start..=index] == property
                    && let Some(value) = string_property_value(source, index + 1)?
                {
                    return Ok(Some(value));
                }
            }
            b'\'' | b'"' | b'`' => index = skip_string(bytes, index)?,
            _ => {}
        }
        index += 1;
    }
    Ok(None)
}

fn string_property_value(source: &str, mut index: usize) -> Result<Option<String>, String> {
    let bytes = source.as_bytes();
    while bytes.get(index).is_some_and(u8::is_ascii_whitespace) {
        index += 1;
    }
    if bytes.get(index) != Some(&b':') {
        return Ok(None);
    }
    index += 1;
    while bytes.get(index).is_some_and(u8::is_ascii_whitespace) {
        index += 1;
    }
    if !matches!(bytes.get(index), Some(b'\'' | b'"')) {
        return Ok(None);
    }
    let end = skip_string(bytes, index)?;
    decode_js_string(&source[index..=end]).map(Some)
}

fn skip_string(bytes: &[u8], start: usize) -> Result<usize, String> {
    let quote = bytes[start];
    let mut index = start + 1;
    while index < bytes.len() {
        if bytes[index] == b'\\' {
            index += 2;
            continue;
        }
        if bytes[index] == quote {
            return Ok(index);
        }
        index += 1;
    }
    Err("unterminated string in workflow metadata".to_string())
}

fn skip_line_comment(bytes: &[u8], start: usize) -> usize {
    bytes[start + 2..]
        .iter()
        .position(|byte| *byte == b'\n')
        .map_or(bytes.len(), |offset| start + 2 + offset)
}

fn skip_block_comment(bytes: &[u8], start: usize) -> Result<usize, String> {
    bytes[start + 2..]
        .windows(2)
        .position(|window| window == b"*/")
        .map(|offset| start + 3 + offset)
        .ok_or_else(|| "unterminated comment in workflow metadata".to_string())
}

fn decode_js_string(source: &str) -> Result<String, String> {
    let quote = source
        .as_bytes()
        .first()
        .copied()
        .ok_or_else(|| "empty workflow metadata string".to_string())?;
    let content = &source[1..source.len() - 1];
    if quote == b'"' {
        return serde_json::from_str(source)
            .map_err(|err| format!("invalid workflow metadata string: {err}"));
    }

    let mut decoded = String::with_capacity(content.len());
    let mut chars = content.chars();
    while let Some(character) = chars.next() {
        if character != '\\' {
            decoded.push(character);
            continue;
        }
        let escaped = chars
            .next()
            .ok_or_else(|| "invalid trailing escape in workflow metadata".to_string())?;
        decoded.push(match escaped {
            '\\' => '\\',
            '\'' => '\'',
            '"' => '"',
            'n' => '\n',
            'r' => '\r',
            't' => '\t',
            other => other,
        });
    }
    Ok(decoded)
}

#[cfg(test)]
#[path = "workflow_tests.rs"]
mod tests;
