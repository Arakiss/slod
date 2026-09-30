use anyhow::{Result, bail};
use serde_json::{Map, Value};

use crate::trace::EventKind;

/// Default source label recorded when a host hook does not name itself.
pub const DEFAULT_SOURCE: &str = "generic";

/// Maximum number of bytes of host-reported stderr kept on a `tool.result`.
/// Enough to identify why a command failed, small enough that a trace does not
/// become a second copy of every command's error stream.
pub const STDERR_CAPTURE_BYTES: usize = 500;

/// Normalize a free-form `--source` label. Slod never hardcodes the names
/// of specific agent harnesses: the label is an opaque string the host chooses,
/// trimmed and stored verbatim on every mapped event. An empty label falls back
/// to [`DEFAULT_SOURCE`].
pub fn normalize_source(input: &str) -> String {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        DEFAULT_SOURCE.to_string()
    } else {
        trimmed.to_string()
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct HookEvent {
    pub kind: EventKind,
    pub payload: Value,
}

/// Extract the host session id from a hook payload, if present. Looks at the
/// same fields `insert_host_context` records, plus the camelCase `sessionId`
/// variant some hosts emit.
pub fn session_id(payload: &Value) -> Option<String> {
    string_field(payload, &["session_id", "sessionId"])
}

/// Derive a stable, per-session run id from a hook payload. When the host
/// provides a session id we use `run-<session_id>` so every event from the
/// same session lands in the same trace. When it does not, we fall back to a
/// deterministic id derived from the payload's stable identity fields (cwd,
/// hook event, tool, command) so repeated hooks in the same context still
/// share one trace. The fallback is content-addressed and free of randomness,
/// so the same payload context always yields the same run id.
pub fn derive_run_id(payload: &Value) -> String {
    if let Some(session) = session_id(payload) {
        return format!("run-{session}");
    }

    let cwd = string_field(payload, &["cwd", "workspace"]).unwrap_or_default();
    let hook_event = string_field(
        payload,
        &["hook_event_name", "event_name", "event", "type", "name"],
    )
    .unwrap_or_default();
    let tool = infer_tool(payload).unwrap_or_default();
    let command = infer_command(payload).unwrap_or_default();
    let seed = format!("{cwd}\u{1f}{hook_event}\u{1f}{tool}\u{1f}{command}");
    format!("run-{:016x}", fnv1a64(seed.as_bytes()))
}

/// 64-bit FNV-1a hash. Deterministic and dependency-free; used only to build a
/// stable fallback run id, never for security.
fn fnv1a64(bytes: &[u8]) -> u64 {
    const OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
    const PRIME: u64 = 0x0000_0100_0000_01b3;
    let mut hash = OFFSET;
    for &byte in bytes {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(PRIME);
    }
    hash
}

pub fn map_hook_payload(source: &str, payload: &Value) -> Result<Vec<HookEvent>> {
    let Some(object) = payload.as_object() else {
        bail!("hook payload must be a JSON object");
    };
    if object.is_empty() {
        bail!("hook payload must not be empty");
    }

    let hook_event = string_field(
        payload,
        &["hook_event_name", "event_name", "event", "type", "name"],
    )
    .unwrap_or_else(|| "hook.event".to_string());
    let normalized = hook_event.to_ascii_lowercase();

    if let Some(event) = map_permission_decision(source, &hook_event, payload) {
        return Ok(vec![event]);
    }

    if is_tool_result_hook(&normalized, payload) {
        return Ok(vec![map_tool_result(source, &hook_event, payload)]);
    }

    if is_error_hook(&normalized, payload) {
        return Ok(vec![map_error(source, &hook_event, payload)]);
    }

    if is_tool_call_hook(&normalized, payload) {
        return Ok(vec![map_tool_call(source, &hook_event, payload)]);
    }

    bail!(
        "unsupported hook payload: expected tool call, tool result, permission decision, or error"
    )
}

fn map_permission_decision(source: &str, hook_event: &str, payload: &Value) -> Option<HookEvent> {
    let decision = string_field(
        payload,
        &[
            "decision",
            "permission_decision",
            "permissionDecision",
            "permission.decision",
            "tool_decision",
        ],
    )?;

    let mut out = base_payload(source, hook_event);
    insert_string(
        &mut out,
        "capability",
        string_field(payload, &["capability", "permission.capability"])
            .or_else(|| infer_capability(payload)),
    );
    insert_string(&mut out, "decision", Some(decision));
    insert_string(
        &mut out,
        "reason",
        string_field(payload, &["reason", "permission.reason"]),
    );
    insert_string(&mut out, "tool", infer_tool(payload));
    insert_string(&mut out, "command", infer_command(payload));
    insert_host_context(&mut out, payload);

    Some(HookEvent {
        kind: EventKind::PermissionDecision,
        payload: Value::Object(out),
    })
}

fn map_tool_call(source: &str, hook_event: &str, payload: &Value) -> HookEvent {
    let mut out = base_payload(source, hook_event);
    insert_string(&mut out, "tool", infer_tool(payload));
    insert_string(&mut out, "command", infer_command(payload));
    insert_value(
        &mut out,
        "input",
        value_field(
            payload,
            &[
                "tool_input",
                "toolInput",
                "input",
                "arguments",
                "args",
                "parameters",
                "tool_call.input",
            ],
        ),
    );
    insert_host_context(&mut out, payload);

    HookEvent {
        kind: EventKind::ToolCall,
        payload: Value::Object(out),
    }
}

fn map_tool_result(source: &str, hook_event: &str, payload: &Value) -> HookEvent {
    let outcome = tool_outcome(payload);

    let mut out = base_payload(source, hook_event);
    insert_string(&mut out, "tool", infer_tool(payload));
    insert_string(&mut out, "command", infer_command(payload));
    // Three-state outcome. `null` means the host reported nothing we can read,
    // and is recorded as such: a trace that claims success because no field
    // said otherwise is worse than one that admits it does not know.
    out.insert(
        "success".to_string(),
        match outcome.success {
            Some(success) => Value::Bool(success),
            None => Value::Null,
        },
    );
    out.insert(
        "outcome_source".to_string(),
        Value::String(outcome.source.to_string()),
    );
    insert_value(&mut out, "exit_code", outcome.exit_code);
    insert_value(
        &mut out,
        "duration_ms",
        value_field(
            payload,
            &[
                "duration_ms",
                "durationMs",
                "tool_response.duration_ms",
                "tool_response.durationMs",
                "tool_response.metadata.duration_ms",
                "result.duration_ms",
            ],
        ),
    );
    // A host that renders the result as text instead of a structured object
    // puts everything the reader needs — including why a command failed — in
    // that text, so it is the output when no structured one is offered.
    insert_value(
        &mut out,
        "output",
        non_empty_value_field(
            payload,
            &[
                "output",
                "stdout",
                "tool_response.output",
                // A stream reported as `{ "text": … }` is recorded as its
                // text, not as the wrapper object a reader would have to
                // unwrap, so the leaf is probed before the container.
                "tool_response.stdout.text",
                "tool_response.stdout",
                "tool_response.aggregated_output.text",
                "tool_response.aggregated_output",
                "result.output",
                "result.stdout",
            ],
        )
        .or_else(|| response_text(payload).map(Value::String)),
    );
    insert_string(&mut out, "stderr", stderr_capture(payload));
    insert_string(&mut out, "error", error_text(payload));
    insert_host_context(&mut out, payload);

    HookEvent {
        kind: EventKind::ToolResult,
        payload: Value::Object(out),
    }
}

/// A tool outcome as the host actually reported it.
///
/// `success` is three-state on purpose. `Some(true)`/`Some(false)` mean a field
/// in the payload said so; `None` means no field did, and the caller must
/// record `null` rather than invent a verdict. `source` names the field that
/// decided it so a reader can tell a measured outcome from an absent one.
#[derive(Debug, Clone, PartialEq)]
pub struct ToolOutcome {
    pub success: Option<bool>,
    pub exit_code: Option<Value>,
    pub source: &'static str,
}

const SUCCESS_PATHS: &[&str] = &[
    "success",
    "tool_response.success",
    "result.success",
    "ok",
    "status.success",
];
const IS_ERROR_PATHS: &[&str] = &[
    "is_error",
    "isError",
    "tool_response.is_error",
    "tool_response.isError",
    "result.is_error",
];
const INTERRUPTED_PATHS: &[&str] = &[
    "interrupted",
    "tool_response.interrupted",
    "result.interrupted",
];
const TIMED_OUT_PATHS: &[&str] = &[
    "timed_out",
    "timedOut",
    "tool_response.timed_out",
    "tool_response.timedOut",
];
const EXIT_CODE_PATHS: &[&str] = &[
    "exit_code",
    "exitCode",
    "tool_response.exit_code",
    "tool_response.exitCode",
    "tool_response.metadata.exit_code",
    "result.exit_code",
];
const ERROR_PATHS: &[&str] = &["error", "tool_response.error", "result.error"];
const RESPONSE_TEXT_PATHS: &[&str] = &["tool_response", "toolResponse", "result"];

/// Resolve the outcome of a tool result payload without ever defaulting to
/// success.
///
/// Negative signals are read first: a host that says a call errored, was
/// interrupted, or timed out has settled the question, whatever else the
/// payload claims. Only then do we read an explicit success flag, an exit
/// code, an exit code embedded in a textual response, or an error message.
/// Anything else is unknown.
///
/// Deliberately *not* a failure signal: a non-empty stderr. Well-behaved
/// commands write progress and warnings to stderr and exit 0, so stderr is
/// captured as evidence (see [`STDERR_CAPTURE_BYTES`]) but never used as a
/// verdict.
pub fn tool_outcome(payload: &Value) -> ToolOutcome {
    let exit_code = value_field(payload, EXIT_CODE_PATHS);

    for (paths, source) in [
        (IS_ERROR_PATHS, "is_error"),
        (INTERRUPTED_PATHS, "interrupted"),
        (TIMED_OUT_PATHS, "timed_out"),
    ] {
        if bool_field(payload, paths) == Some(true) {
            return ToolOutcome {
                success: Some(false),
                exit_code,
                source,
            };
        }
    }

    if let Some(success) = bool_field(payload, SUCCESS_PATHS) {
        return ToolOutcome {
            success: Some(success),
            exit_code,
            source: "success",
        };
    }

    // An explicit `is_error: false` is the host asserting the call did not
    // error. `interrupted: false` is not: it only rules out an interruption,
    // and says nothing about the exit status.
    if bool_field(payload, IS_ERROR_PATHS) == Some(false) {
        return ToolOutcome {
            success: Some(true),
            exit_code,
            source: "is_error",
        };
    }

    if let Some(success) = exit_code.as_ref().and_then(value_exit_success) {
        return ToolOutcome {
            success: Some(success),
            exit_code,
            source: "exit_code",
        };
    }

    if let Some(code) = response_text(payload)
        .as_deref()
        .and_then(exit_code_in_text)
    {
        return ToolOutcome {
            success: Some(code == 0),
            exit_code: Some(Value::from(code)),
            source: "response_text",
        };
    }

    if error_text(payload).is_some() {
        return ToolOutcome {
            success: Some(false),
            exit_code,
            source: "error",
        };
    }

    ToolOutcome {
        success: None,
        exit_code,
        source: "none",
    }
}

fn error_text(payload: &Value) -> Option<String> {
    string_field(payload, ERROR_PATHS)
}

/// The tool response when the host hands back rendered text instead of a
/// structured object.
fn response_text(payload: &Value) -> Option<String> {
    RESPONSE_TEXT_PATHS
        .iter()
        .find_map(|path| match value_at(payload, path) {
            Some(Value::String(text)) if !text.trim().is_empty() => Some(text.to_string()),
            _ => None,
        })
}

/// Read an exit status out of a textual tool response.
///
/// Hosts that render a result for the model still put the exit status in the
/// first line of it; `Error: Exit code 127` and `Exit code: 127` are both in
/// use. Only the first non-empty line is scanned, so the phrase appearing
/// later inside captured output cannot be mistaken for the status.
fn exit_code_in_text(text: &str) -> Option<i64> {
    const NEEDLE: &str = "exit code";

    let line = text.lines().find(|line| !line.trim().is_empty())?;
    let position = line.to_ascii_lowercase().find(NEEDLE)?;
    let rest = line.get(position + NEEDLE.len()..)?;
    let digits = rest
        .chars()
        .skip_while(|ch| *ch == ':' || ch.is_whitespace())
        .take_while(|ch| ch.is_ascii_digit() || *ch == '-')
        .collect::<String>();
    digits.parse::<i64>().ok()
}

/// The first [`STDERR_CAPTURE_BYTES`] of whatever the host reported on stderr.
fn stderr_capture(payload: &Value) -> Option<String> {
    let text = string_field(
        payload,
        &[
            "stderr",
            "tool_response.stderr",
            "tool_response.stderr.text",
            "tool_response.metadata.stderr",
            "result.stderr",
        ],
    )?;
    if text.trim().is_empty() {
        return None;
    }
    Some(truncate_bytes(&text, STDERR_CAPTURE_BYTES))
}

/// Truncate on a character boundary at or below `limit` bytes, marking the cut
/// so a reader never mistakes a clipped message for the whole one.
fn truncate_bytes(text: &str, limit: usize) -> String {
    if text.len() <= limit {
        return text.to_string();
    }
    let mut end = limit;
    while end > 0 && !text.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}\n[truncated]", &text[..end])
}

fn map_error(source: &str, hook_event: &str, payload: &Value) -> HookEvent {
    let mut out = base_payload(source, hook_event);
    insert_string(
        &mut out,
        "message",
        string_field(payload, &["message", "error", "error.message"])
            .or_else(|| Some("host hook reported an error".to_string())),
    );
    insert_string(&mut out, "tool", infer_tool(payload));
    insert_string(&mut out, "command", infer_command(payload));
    insert_host_context(&mut out, payload);

    HookEvent {
        kind: EventKind::Error,
        payload: Value::Object(out),
    }
}

fn is_tool_call_hook(normalized: &str, payload: &Value) -> bool {
    normalized.contains("pretooluse")
        || normalized.contains("tool.call")
        || normalized.contains("tool_call")
        || normalized.contains("toolcall")
        || value_field(
            payload,
            &[
                "tool_input",
                "toolInput",
                "tool_name",
                "toolName",
                "tool_call",
                "request.tool_input",
            ],
        )
        .is_some()
}

fn is_tool_result_hook(normalized: &str, payload: &Value) -> bool {
    normalized.contains("posttooluse")
        || normalized.contains("tool.result")
        || normalized.contains("tool_result")
        || normalized.contains("toolresult")
        || value_field(
            payload,
            &[
                "tool_response",
                "toolResponse",
                "result",
                "success",
                "exit_code",
                "exitCode",
            ],
        )
        .is_some()
}

fn is_error_hook(normalized: &str, payload: &Value) -> bool {
    normalized.contains("error")
        || value_field(payload, &["error", "error.message", "failure"]).is_some()
}

fn base_payload(source: &str, hook_event: &str) -> Map<String, Value> {
    let mut out = Map::new();
    out.insert("source".to_string(), Value::String(source.to_string()));
    out.insert(
        "hook_event".to_string(),
        Value::String(hook_event.to_string()),
    );
    out
}

fn insert_host_context(out: &mut Map<String, Value>, payload: &Value) {
    let mut host = Map::new();
    for key in [
        "session_id",
        "transcript_path",
        "cwd",
        "workspace",
        "model",
        "hook_event_name",
    ] {
        insert_value(&mut host, key, value_field(payload, &[key]));
    }
    if !host.is_empty() {
        out.insert("host".to_string(), Value::Object(host));
    }
}

fn infer_capability(payload: &Value) -> Option<String> {
    let tool = infer_tool(payload)?;
    let command = infer_command(payload);
    Some(match command {
        Some(command) => format!("tool:{tool}:{command}"),
        None => format!("tool:{tool}"),
    })
}

fn infer_tool(payload: &Value) -> Option<String> {
    string_field(
        payload,
        &[
            "tool_name",
            "toolName",
            "tool",
            "tool.name",
            "tool_call.tool",
            "tool_call.name",
            "request.tool_name",
            "tool_response.tool",
            "result.tool",
        ],
    )
}

fn infer_command(payload: &Value) -> Option<String> {
    string_field(
        payload,
        &[
            "command",
            "tool_input.command",
            "toolInput.command",
            "input.command",
            "arguments.command",
            "args.command",
            "parameters.command",
            "tool_call.command",
            "tool_response.command",
            "result.command",
        ],
    )
}

fn insert_string(out: &mut Map<String, Value>, key: &str, value: Option<String>) {
    if let Some(value) = value {
        out.insert(key.to_string(), Value::String(value));
    }
}

fn insert_value(out: &mut Map<String, Value>, key: &str, value: Option<Value>) {
    if let Some(value) = value {
        out.insert(key.to_string(), value);
    }
}

fn string_field(value: &Value, paths: &[&str]) -> Option<String> {
    for path in paths {
        let Some(value) = value_at(value, path) else {
            continue;
        };
        match value {
            Value::String(text) if !text.trim().is_empty() => return Some(text.to_string()),
            Value::Number(number) => return Some(number.to_string()),
            Value::Bool(flag) => return Some(flag.to_string()),
            Value::Object(object) => {
                for nested in ["name", "tool", "command", "message"] {
                    if let Some(Value::String(text)) = object.get(nested)
                        && !text.trim().is_empty()
                    {
                        return Some(text.to_string());
                    }
                }
            }
            _ => {}
        }
    }
    None
}

fn bool_field(value: &Value, paths: &[&str]) -> Option<bool> {
    for path in paths {
        let Some(value) = value_at(value, path) else {
            continue;
        };
        match value {
            Value::Bool(flag) => return Some(*flag),
            Value::Number(number) => {
                if let Some(code) = number.as_i64() {
                    return Some(code == 0);
                }
            }
            Value::String(text) => match text.trim().to_ascii_lowercase().as_str() {
                "true" | "ok" | "success" | "allow" | "allowed" => return Some(true),
                "false" | "failed" | "error" | "deny" | "denied" => return Some(false),
                _ => {}
            },
            _ => {}
        }
    }
    None
}

fn value_exit_success(value: &Value) -> Option<bool> {
    match value {
        Value::Number(number) => number.as_i64().map(|code| code == 0),
        Value::String(text) => text.parse::<i64>().ok().map(|code| code == 0),
        _ => None,
    }
}

/// Like [`value_field`], but an empty string does not win over a later path
/// that has content. A host reporting an empty `stdout` alongside a populated
/// `aggregated_output` should record the output that exists.
fn non_empty_value_field(value: &Value, paths: &[&str]) -> Option<Value> {
    paths
        .iter()
        .filter_map(|path| value_at(value, path))
        .find(|value| match value {
            Value::Null => false,
            Value::String(text) => !text.is_empty(),
            // A stream wrapper carrying an empty `text` is an empty stream.
            Value::Object(object) => match object.get("text") {
                Some(Value::String(text)) => !text.is_empty(),
                _ => true,
            },
            _ => true,
        })
        .cloned()
}

fn value_field(value: &Value, paths: &[&str]) -> Option<Value> {
    paths
        .iter()
        .find_map(|path| value_at(value, path).cloned())
        .filter(|value| !value.is_null())
}

fn value_at<'a>(value: &'a Value, path: &str) -> Option<&'a Value> {
    let mut current = value;
    for part in path.split('.') {
        current = current.get(part)?;
    }
    Some(current)
}

#[cfg(test)]
#[path = "hook_tests.rs"]
mod tests;
