use super::*;
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

/// The Bash result Claude Code actually delivers on PostToolUse: stdout and a
/// couple of flags, no exit code and no error marker. Nothing in it says the
/// command succeeded, so nothing may claim it did.
#[test]
fn claude_bash_result_without_a_verdict_is_unknown_not_success() {
    let events = map_hook_payload(
        "claude-code",
        &json!({
            "session_id": "00000000-0000-0000-0000-000000000000",
            "cwd": "/tmp/ws",
            "hook_event_name": "PostToolUse",
            "tool_name": "Bash",
            "tool_input": {"command": "rg -n mani"},
            "tool_response": {
                "stdout": "settings.json:624:  mani",
                "stderr": "",
                "interrupted": false,
                "isImage": false,
                "noOutputExpected": false
            }
        }),
    )
    .unwrap();

    assert_eq!(events[0].kind, EventKind::ToolResult);
    assert_eq!(events[0].payload["success"], Value::Null);
    assert_eq!(events[0].payload["outcome_source"], "none");
    assert_eq!(events[0].payload["output"], "settings.json:624:  mani");
    assert!(events[0].payload.get("stderr").is_none());
}

/// A command can write to stderr and still exit 0, so stderr is captured as
/// evidence but must never be read as a failure.
#[test]
fn stderr_is_captured_without_becoming_a_verdict() {
    let events = map_hook_payload(
        "claude-code",
        &json!({
            "hook_event_name": "PostToolUse",
            "tool_name": "Bash",
            "tool_response": {
                "stdout": "",
                "stderr": "warning: 2 files skipped",
                "interrupted": false
            }
        }),
    )
    .unwrap();

    assert_eq!(events[0].payload["stderr"], "warning: 2 files skipped");
    assert_eq!(events[0].payload["success"], Value::Null);
    assert_eq!(events[0].payload["outcome_source"], "none");
}

#[test]
fn stderr_capture_is_truncated_on_a_character_boundary() {
    let long = "é".repeat(STDERR_CAPTURE_BYTES);
    let events = map_hook_payload(
        "generic",
        &json!({
            "hook_event_name": "PostToolUse",
            "tool_name": "Bash",
            "tool_response": {"stderr": long, "exit_code": 1}
        }),
    )
    .unwrap();

    let captured = events[0].payload["stderr"].as_str().unwrap();
    assert!(captured.ends_with("\n[truncated]"));
    assert!(captured.len() <= STDERR_CAPTURE_BYTES + "\n[truncated]".len());
}

/// When a tool fails, some hosts hand back a rendered string instead of an
/// object. The exit status is still in its first line.
#[test]
fn reads_the_exit_status_out_of_a_textual_response() {
    for (response, code) in [
        ("Error: Exit code 127\nzsh: command not found: nope", 127),
        (
            "Exit code: 1\nWall time: 0.012 seconds\nTotal output lines: 1\n\nls: no such file",
            1,
        ),
    ] {
        let events = map_hook_payload(
            "generic",
            &json!({
                "hook_event_name": "PostToolUse",
                "tool_name": "Bash",
                "tool_response": response
            }),
        )
        .unwrap();

        assert_eq!(events[0].payload["success"], false, "{response}");
        assert_eq!(events[0].payload["exit_code"], code, "{response}");
        assert_eq!(events[0].payload["outcome_source"], "response_text");
    }
}

#[test]
fn textual_response_reporting_a_zero_exit_is_a_success() {
    let events = map_hook_payload(
        "generic",
        &json!({
            "hook_event_name": "PostToolUse",
            "tool_name": "Bash",
            "tool_response": "Exit code: 0\nWall time: 0.004 seconds\n\nhola"
        }),
    )
    .unwrap();

    assert_eq!(events[0].payload["success"], true);
    assert_eq!(events[0].payload["exit_code"], 0);
}

/// The phrase appearing inside captured output must not be mistaken for the
/// status of the call itself.
#[test]
fn exit_status_is_only_read_from_the_first_line() {
    let events = map_hook_payload(
        "generic",
        &json!({
            "hook_event_name": "PostToolUse",
            "tool_name": "Bash",
            "tool_response": "grep results:\nscript.sh: exit code 3 was returned"
        }),
    )
    .unwrap();

    assert_eq!(events[0].payload["success"], Value::Null);
    assert_eq!(events[0].payload["outcome_source"], "none");
}

/// A structured exec result, as a host that reports one emits it.
#[test]
fn maps_a_structured_exec_result_with_exit_code_and_streams() {
    let events = map_hook_payload(
        "codex",
        &json!({
            "session_id": "01a0eed8-e8f8-7390-a065-684e3fba11a1",
            "cwd": "/tmp/ws",
            "hook_event_name": "PostToolUse",
            "model": "gpt-6.1-sol",
            "permission_mode": "default",
            "tool_name": "Bash",
            "tool_input": {"command": "ls /nope"},
            "tool_use_id": "call-1",
            "tool_response": {
                "exit_code": 1,
                "stdout": {"text": ""},
                "stderr": {"text": "ls: /nope: No such file or directory"},
                "aggregated_output": {"text": "ls: /nope: No such file or directory"},
                "timed_out": false
            }
        }),
    )
    .unwrap();

    assert_eq!(events[0].payload["success"], false);
    assert_eq!(events[0].payload["exit_code"], 1);
    assert_eq!(events[0].payload["outcome_source"], "exit_code");
    assert_eq!(
        events[0].payload["stderr"],
        "ls: /nope: No such file or directory"
    );
    // A `{ "text": … }` stream lands as its text, not as the wrapper object.
    assert_eq!(
        events[0].payload["output"],
        "ls: /nope: No such file or directory"
    );
}

#[test]
fn interrupted_and_timed_out_are_failures() {
    for (field, source) in [("interrupted", "interrupted"), ("timed_out", "timed_out")] {
        let events = map_hook_payload(
            "generic",
            &json!({
                "hook_event_name": "PostToolUse",
                "tool_name": "Bash",
                "tool_response": {field: true, "stdout": "partial"}
            }),
        )
        .unwrap();

        assert_eq!(events[0].payload["success"], false, "{field}");
        assert_eq!(events[0].payload["outcome_source"], source);
    }
}

/// A host asserting the call did not error settles the question; the same host
/// asserting it was not interrupted does not.
#[test]
fn is_error_settles_the_outcome_and_interrupted_alone_does_not() {
    let with_is_error = map_hook_payload(
        "generic",
        &json!({
            "hook_event_name": "PostToolUse",
            "tool_name": "Bash",
            "tool_response": {"is_error": false, "stdout": "ok"}
        }),
    )
    .unwrap();
    assert_eq!(with_is_error[0].payload["success"], true);
    assert_eq!(with_is_error[0].payload["outcome_source"], "is_error");

    let is_error_true = map_hook_payload(
        "generic",
        &json!({
            "hook_event_name": "PostToolUse",
            "tool_name": "Bash",
            "tool_response": {"is_error": true, "stdout": ""}
        }),
    )
    .unwrap();
    assert_eq!(is_error_true[0].payload["success"], false);
}

/// The old behavior: no field said anything, so the mapper said success. That
/// is the defect this module exists to prevent.
#[test]
fn an_error_message_is_a_failure_and_its_absence_is_not_a_success() {
    let failed = map_hook_payload(
        "generic",
        &json!({
            "hook_event_name": "PostToolUse",
            "tool_name": "Bash",
            "tool_response": {"error": "tool crashed"}
        }),
    )
    .unwrap();
    assert_eq!(failed[0].payload["success"], false);
    assert_eq!(failed[0].payload["outcome_source"], "error");

    let silent = map_hook_payload(
        "generic",
        &json!({"hook_event_name": "PostToolUse", "tool_name": "Bash"}),
    )
    .unwrap();
    assert_eq!(silent[0].payload["success"], Value::Null);
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
