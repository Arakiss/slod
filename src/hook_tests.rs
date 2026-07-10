use super::*;
use crate::redaction::REDACTED;
use serde_json::json;

#[test]
fn normalize_source_trims_and_defaults() {
    assert_eq!(normalize_source("acme"), "acme");
    assert_eq!(normalize_source("  acme  "), "acme");
    // An empty or whitespace-only label falls back to the generic default.
    assert_eq!(normalize_source(""), DEFAULT_SOURCE);
    assert_eq!(normalize_source("   "), DEFAULT_SOURCE);
    // Any free-form label is preserved verbatim; no harness names are special.
    assert_eq!(normalize_source("my-harness"), "my-harness");
}

#[test]
fn maps_pre_tool_use_to_tool_call() {
    let events = map_hook_payload(
        "generic",
        &json!({
            "hook_event_name": "PreToolUse",
            "tool_name": "Bash",
            "tool_input": {"command": "cargo test"},
            "session_id": "session-demo"
        }),
    )
    .unwrap();

    assert_eq!(events.len(), 1);
    assert_eq!(events[0].kind, EventKind::ToolCall);
    assert_eq!(events[0].payload["source"], "generic");
    assert_eq!(events[0].payload["tool"], "Bash");
    assert_eq!(events[0].payload["command"], "cargo test");
    assert_eq!(events[0].payload["host"]["session_id"], "session-demo");
}

#[test]
fn source_label_is_stored_verbatim() {
    let events = map_hook_payload(
        "my-harness",
        &json!({
            "hook_event_name": "PreToolUse",
            "tool_name": "Bash",
            "tool_input": {"command": "cargo test"}
        }),
    )
    .unwrap();

    assert_eq!(events[0].payload["source"], "my-harness");
}

#[test]
fn maps_post_tool_use_to_tool_result() {
    let events = map_hook_payload(
        "generic",
        &json!({
            "hook_event_name": "PostToolUse",
            "tool_name": "Bash",
            "tool_response": {
                "success": true,
                "exit_code": 0,
                "stdout": "ok"
            }
        }),
    )
    .unwrap();

    assert_eq!(events[0].kind, EventKind::ToolResult);
    assert_eq!(events[0].payload["success"], true);
    assert_eq!(events[0].payload["exit_code"], 0);
    assert_eq!(events[0].payload["output"], "ok");
}

#[test]
fn maps_hook_payloads_with_recursive_structural_redaction() {
    let events = map_hook_payload(
        "generic",
        &json!({
            "hook_event_name": "PreToolUse",
            "tool_name": "Http",
            "tool_input": {
                "command": "request synthetic fixture",
                "credentials": {
                    "clientSecret": "synthetic-client-secret",
                    "access_token": "synthetic-access-token"
                },
                "headers": {
                    "Authorization": "Bearer synthetic-authorization",
                    "Accept": "application/json"
                },
                "metadata": {"fixture": true}
            }
        }),
    )
    .unwrap();

    let input = &events[0].payload["input"];
    assert_eq!(input["credentials"]["clientSecret"], REDACTED);
    assert_eq!(input["credentials"]["access_token"], REDACTED);
    assert_eq!(input["headers"]["Authorization"], REDACTED);
    assert_eq!(input["headers"]["Accept"], "application/json");
    assert_eq!(input["metadata"]["fixture"], true);
}

#[test]
fn maps_permission_decision() {
    let events = map_hook_payload(
        "generic",
        &json!({
            "hook_event_name": "PreToolUse",
            "tool_name": "Write",
            "tool_input": {"command": "edit README.md"},
            "decision": "allow"
        }),
    )
    .unwrap();

    assert_eq!(events[0].kind, EventKind::PermissionDecision);
    assert_eq!(events[0].payload["decision"], "allow");
    assert_eq!(events[0].payload["capability"], "tool:Write:edit README.md");
}

#[test]
fn maps_error_payload() {
    let events = map_hook_payload(
        "generic",
        &json!({
            "hook_event_name": "HookError",
            "error": {"message": "permission hook failed"},
            "tool_name": "Bash"
        }),
    )
    .unwrap();

    assert_eq!(events[0].kind, EventKind::Error);
    assert_eq!(events[0].payload["message"], "permission hook failed");
}

#[test]
fn session_id_reads_snake_and_camel_case() {
    assert_eq!(
        session_id(&json!({"session_id": "abc"})),
        Some("abc".to_string())
    );
    assert_eq!(
        session_id(&json!({"sessionId": "xyz"})),
        Some("xyz".to_string())
    );
    assert_eq!(session_id(&json!({"cwd": "/tmp"})), None);
}

#[test]
fn derive_run_id_uses_session_when_present() {
    let payload = json!({"session_id": "host-session", "cwd": "/tmp"});
    assert_eq!(derive_run_id(&payload), "run-host-session");
}

#[test]
fn derive_run_id_fallback_is_deterministic_without_session() {
    let payload = json!({
        "hook_event_name": "PreToolUse",
        "tool_name": "Bash",
        "tool_input": {"command": "cargo test"},
        "cwd": "/tmp/workspace"
    });
    let first = derive_run_id(&payload);
    let second = derive_run_id(&payload);
    assert_eq!(first, second, "fallback run id must be stable");
    assert!(first.starts_with("run-"));
    assert!(session_id(&payload).is_none());

    // A different context yields a different fallback id.
    let other = json!({
        "hook_event_name": "PreToolUse",
        "tool_name": "Bash",
        "tool_input": {"command": "cargo build"},
        "cwd": "/tmp/workspace"
    });
    assert_ne!(derive_run_id(&other), first);
}

#[test]
fn rejects_empty_or_unsupported_payloads() {
    assert!(map_hook_payload("generic", &json!({})).is_err());
    assert!(map_hook_payload("generic", &json!("nope")).is_err());
    assert!(map_hook_payload("generic", &json!({"event":"UserPromptSubmit"})).is_err());
}
