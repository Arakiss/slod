use std::{
    collections::BTreeSet,
    fs,
    io::Write,
    path::Path,
    process::{Command as StdCommand, Stdio},
    sync::{Arc, Barrier},
    thread,
};

use predicates::prelude::*;
use slod::trace::{Event, EventKind};
use tempfile::tempdir;

#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;

mod common;
use common::*;

#[test]
fn hook_ingest_can_initialize_missing_trace_and_record_tool_events() {
    let dir = tempdir().unwrap();
    let trace_path = dir.path().join("hook.slod");

    slod()
        .args([
            "hook",
            "ingest",
            "--source",
            "generic",
            "--run-id",
            "hook-run",
            "--init-if-missing",
            "--file",
        ])
        .arg(&trace_path)
        .write_stdin(
            r#"{"hook_event_name":"PreToolUse","tool_name":"Bash","tool_input":{"command":"cargo test"},"session_id":"host-session"}"#,
        )
        .assert()
        .success()
        .stdout(predicate::str::contains("slod hook ingest"))
        .stdout(predicate::str::contains("tool.call#1"));

    slod()
        .args(["hook", "ingest", "--source", "generic", "--file"])
        .arg(&trace_path)
        .write_stdin(
            r#"{"hook_event_name":"PostToolUse","tool_name":"Bash","tool_response":{"success":true,"exit_code":0,"stdout":"ok"}}"#,
        )
        .assert()
        .success()
        .stdout(predicate::str::contains("tool.result#2"));

    slod()
        .args(["summary", "--file"])
        .arg(&trace_path)
        .assert()
        .success()
        .stdout(predicate::str::contains("run_id: hook-run"))
        .stdout(predicate::str::contains("tool_calls: 1"))
        .stdout(predicate::str::contains("tool_results: 1"));

    slod()
        .args(["finish", "--file"])
        .arg(&trace_path)
        .args(["--status", "success"])
        .assert()
        .success();

    slod()
        .args(["verify", "--file"])
        .arg(&trace_path)
        .assert()
        .success();

    let trace = fs::read_to_string(trace_path).unwrap();
    assert!(trace.contains(r#""source":"generic""#));
    assert!(trace.contains(r#""hook_event":"PreToolUse""#));
    assert!(trace.contains(r#""session_id":"host-session""#));
}

#[test]
fn hook_ingest_stores_free_form_source_label_verbatim() {
    let dir = tempdir().unwrap();
    let trace_path = dir.path().join("hook.slod");

    slod()
        .args([
            "hook",
            "ingest",
            "--source",
            "my-harness",
            "--run-id",
            "hook-run",
            "--init-if-missing",
            "--file",
        ])
        .arg(&trace_path)
        .write_stdin(
            r#"{"hook_event_name":"PreToolUse","tool_name":"Bash","tool_input":{"command":"cargo test"}}"#,
        )
        .assert()
        .success()
        .stdout(predicate::str::contains("source"))
        .stdout(predicate::str::contains("my-harness"));

    let trace = fs::read_to_string(trace_path).unwrap();
    assert!(trace.contains(r#""source":"my-harness""#));
}

#[test]
fn hook_ingest_records_permission_and_error_events() {
    let dir = tempdir().unwrap();
    let trace_path = dir.path().join("hook.slod");

    slod()
        .args(["init", "--file"])
        .arg(&trace_path)
        .args(["--run-id", "hook-run"])
        .assert()
        .success();

    slod()
        .args(["hook", "ingest", "--source", "policy", "--file"])
        .arg(&trace_path)
        .write_stdin(
            r#"{"hook_event_name":"PreToolUse","tool_name":"Write","tool_input":{"command":"edit README.md"},"decision":"allow"}"#,
        )
        .assert()
        .success()
        .stdout(predicate::str::contains("permission.decision#1"));

    slod()
        .args(["hook", "ingest", "--source", "generic", "--file"])
        .arg(&trace_path)
        .write_stdin(
            r#"{"hook_event_name":"HookError","tool_name":"Bash","error":{"message":"hook failed"}}"#,
        )
        .assert()
        .success()
        .stdout(predicate::str::contains("error#2"));

    slod()
        .args(["inspect", "--file"])
        .arg(&trace_path)
        .assert()
        .success()
        .stdout(predicate::str::contains("permission.decision"))
        .stdout(predicate::str::contains("error"));
}

#[test]
fn hook_ingest_rejects_missing_trace_without_init_flag() {
    let dir = tempdir().unwrap();
    let trace_path = dir.path().join("missing.slod");

    slod()
        .args(["hook", "ingest", "--file"])
        .arg(&trace_path)
        .write_stdin(r#"{"hook_event_name":"PreToolUse","tool_name":"Bash"}"#)
        .assert()
        .failure()
        .stderr(predicate::str::contains("trace does not exist"));
}

#[test]
fn hook_ingest_rejects_empty_stdin() {
    let dir = tempdir().unwrap();
    let trace_path = dir.path().join("hook.slod");

    slod()
        .args(["init", "--file"])
        .arg(&trace_path)
        .args(["--run-id", "hook-run"])
        .assert()
        .success();

    slod()
        .args(["hook", "ingest", "--file"])
        .arg(&trace_path)
        .write_stdin("")
        .assert()
        .failure()
        .stderr(predicate::str::contains("missing hook JSON on stdin"));
}

#[test]
fn hook_ingest_does_not_initialize_trace_for_invalid_payload() {
    let dir = tempdir().unwrap();
    let trace_path = dir.path().join("invalid.slod");

    slod()
        .args([
            "hook",
            "ingest",
            "--run-id",
            "invalid-run",
            "--init-if-missing",
            "--file",
        ])
        .arg(&trace_path)
        .write_stdin("{not-json")
        .assert()
        .failure()
        .stderr(predicate::str::contains("invalid JSON payload"));

    assert!(!trace_path.exists());
}

#[test]
fn hook_ingest_does_not_echo_sensitive_input_in_parse_errors() {
    let dir = tempdir().unwrap();
    let trace_path = dir.path().join("invalid.slod");
    let synthetic_credential = "synthetic-error-api-key";
    let malformed =
        format!(r#"{{"hook_event_name":"PreToolUse","api_key":"{synthetic_credential}""#);

    slod()
        .args([
            "hook",
            "ingest",
            "--run-id",
            "invalid-run",
            "--init-if-missing",
            "--file",
        ])
        .arg(&trace_path)
        .write_stdin(malformed)
        .assert()
        .failure()
        .stderr(predicate::str::contains("invalid JSON payload"))
        .stderr(predicate::str::contains(synthetic_credential).not());

    assert!(!trace_path.exists());
}

#[test]
fn hook_ingest_redacts_nested_credentials_before_persisting() {
    let dir = tempdir().unwrap();
    let runs = dir.path().join("runs");
    let trace_path = runs.join("run-redaction-session.slod");
    let sensitive_values = [
        "synthetic-client-secret",
        "synthetic-access-token",
        "synthetic-password",
        "synthetic-cookie",
        "synthetic-authorization",
        "synthetic-api-key",
    ];

    slod()
        .args(["hook", "ingest", "--source", "generic", "--dir"])
        .arg(&runs)
        .write_stdin(
            r#"{"hook_event_name":"PreToolUse","tool_name":"Http","session_id":"redaction-session","tool_input":{"command":"request synthetic fixture","client_secret":"synthetic-client-secret","accessToken":"synthetic-access-token","password":"synthetic-password","headers":{"Cookie":"synthetic-cookie","Authorization":"synthetic-authorization","X-API-Key":"synthetic-api-key","Accept":"application/json"},"metadata":{"fixture":"kept"}}}"#,
        )
        .assert()
        .success();

    let trace = fs::read_to_string(trace_path).unwrap();
    for sensitive in sensitive_values {
        assert!(
            !trace.contains(sensitive),
            "persisted trace contains a synthetic credential"
        );
    }
    assert!(trace.contains("[REDACTED]"));
    assert!(trace.contains("application/json"));
    assert!(trace.contains(r#""fixture":"kept""#));
}

#[cfg(unix)]
#[test]
fn storage_creation_stays_private_with_a_permissive_umask() {
    let dir = tempdir().unwrap();
    let workspace = dir.path().join("workspace");
    fs::create_dir(&workspace).unwrap();
    let data_dir = workspace.join(".slod");
    let runs = data_dir.join("runs");
    let trace_path = runs.join("run-private-session.slod");
    let lock_path = runs.join("run-private-session.slod.lock");

    let mut child = StdCommand::new("sh")
        .arg("-c")
        .arg(
            r#"umask 000
exec "$SLOD_BIN" hook ingest --source generic --dir "$RUNS""#,
        )
        .env("SLOD_BIN", env!("CARGO_BIN_EXE_slod"))
        .env("RUNS", &runs)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(
            br#"{"hook_event_name":"PreToolUse","tool_name":"Bash","tool_input":{"command":"synthetic"},"session_id":"private-session"}"#,
        )
        .unwrap();
    let output = child.wait_with_output().unwrap();
    assert!(
        output.status.success(),
        "hook ingest failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let catalog = data_dir.join("catalog");
    let ledger = catalog.join("ledger.slod");
    let output = StdCommand::new("sh")
        .arg("-c")
        .arg(
            r#"umask 000
exec "$SLOD_BIN" ledger rebuild --dir "$RUNS" --out "$LEDGER""#,
        )
        .env("SLOD_BIN", env!("CARGO_BIN_EXE_slod"))
        .env("RUNS", &runs)
        .env("LEDGER", &ledger)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "ledger rebuild failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    assert_eq!(unix_mode(&data_dir), 0o700);
    assert_eq!(unix_mode(&runs), 0o700);
    assert_eq!(unix_mode(&catalog), 0o700);
    assert_eq!(unix_mode(&trace_path), 0o600);
    assert_eq!(unix_mode(&lock_path), 0o600);
    assert_eq!(unix_mode(&ledger), 0o600);
}

#[cfg(unix)]
fn unix_mode(path: &Path) -> u32 {
    fs::metadata(path).unwrap().permissions().mode() & 0o777
}

#[test]
fn hook_install_print_emits_snippet_and_writes_nothing() {
    let dir = tempdir().unwrap();
    let hooks_file = dir.path().join(".agent").join("hooks.json");

    slod()
        .current_dir(dir.path())
        .args(["hook", "install", "--print"])
        .assert()
        .success()
        .stdout(predicate::str::contains("hook install (print)"))
        .stdout(predicate::str::contains("slod hook ingest"))
        .stdout(predicate::str::contains("PreToolUse"))
        .stdout(predicate::str::contains("PostToolUse"))
        .stdout(predicate::str::contains("--source generic"))
        .stdout(predicate::str::contains("\"matcher\": \"Bash\""))
        .stdout(predicate::str::contains("paste this into the host"));

    // --print must never create the local hooks file.
    assert!(!hooks_file.exists());
}

#[test]
fn hook_install_writes_preserves_existing_entries_and_is_idempotent() {
    let dir = tempdir().unwrap();
    let hooks_dir = dir.path().join(".agent");
    fs::create_dir(&hooks_dir).unwrap();
    let hooks_file = hooks_dir.join("hooks.json");

    // Seed a pre-existing, foreign hook entry that must be preserved. It lives
    // under the top-level `hooks` object a host actually reads.
    fs::write(
        &hooks_file,
        r#"{"hooks":{"PreToolUse":[{"matcher":"Bash","hooks":[{"type":"command","command":"existing-tool"}]}]}}"#,
    )
    .unwrap();

    slod()
        .current_dir(dir.path())
        .args(["hook", "install"])
        .assert()
        .success()
        .stdout(predicate::str::contains("hook install"))
        .stdout(predicate::str::contains("wired"));

    let after_first = fs::read_to_string(&hooks_file).unwrap();
    // Existing foreign entry preserved.
    assert!(after_first.contains("existing-tool"));
    // Slod entries added under both events, nested under `hooks`.
    assert!(after_first.contains("slod hook ingest"));
    assert!(after_first.contains("PostToolUse"));
    let parsed: serde_json::Value = serde_json::from_str(&after_first).unwrap();
    assert!(
        parsed["hooks"]["PreToolUse"].is_array(),
        "entries must be nested under the top-level `hooks` object a host reads"
    );
    assert!(parsed["hooks"]["PostToolUse"].is_array());

    // Second run must be idempotent: no duplicate slod entries.
    slod()
        .current_dir(dir.path())
        .args(["hook", "install"])
        .assert()
        .success()
        .stdout(predicate::str::contains("already wired"));

    let after_second = fs::read_to_string(&hooks_file).unwrap();
    assert_eq!(
        after_second.matches("slod hook ingest").count(),
        2,
        "expected exactly one slod entry per event after re-running install"
    );
    assert!(after_second.contains("existing-tool"));
}

#[test]
fn hook_install_creates_hooks_file_when_missing() {
    let dir = tempdir().unwrap();
    let hooks_file = dir.path().join(".agent").join("hooks.json");

    slod()
        .current_dir(dir.path())
        .args(["hook", "install", "--run-id", "demo-run"])
        .assert()
        .success();

    assert!(hooks_file.exists());
    let content = fs::read_to_string(&hooks_file).unwrap();
    assert!(content.contains("slod hook ingest"));
    assert!(content.contains("--run-id demo-run"));
    // The wiring must sit under the top-level `hooks` object a host discovers,
    // with the `Bash` matcher that captures shell tool calls.
    let parsed: serde_json::Value = serde_json::from_str(&content).unwrap();
    assert_eq!(parsed["hooks"]["PreToolUse"][0]["matcher"], "Bash");
    assert_eq!(parsed["hooks"]["PostToolUse"][0]["matcher"], "Bash");
}

#[test]
fn hook_install_honors_custom_source_and_file() {
    let dir = tempdir().unwrap();
    let hooks_file = dir.path().join("hooks.json");

    slod()
        .current_dir(dir.path())
        .args(["hook", "install", "--source", "my-harness", "--file"])
        .arg(&hooks_file)
        .assert()
        .success();

    let content = fs::read_to_string(&hooks_file).unwrap();
    assert!(content.contains("--source my-harness"));
}

#[test]
fn hook_ingest_dir_derives_per_session_trace_without_run_id_or_init() {
    let dir = tempdir().unwrap();
    let runs = dir.path().join(".slod").join("runs");

    // No --run-id and no --init-if-missing: the wired-command shape.
    slod()
        .args(["hook", "ingest", "--source", "generic", "--dir"])
        .arg(&runs)
        .write_stdin(
            r#"{"hook_event_name":"PreToolUse","tool_name":"Bash","tool_input":{"command":"cargo test"},"session_id":"sess-A"}"#,
        )
        .assert()
        .success()
        .stdout(predicate::str::contains("tool.call#1"));

    let trace_path = runs.join("run-sess-A.slod");
    assert!(
        trace_path.exists(),
        "expected per-session trace to be created"
    );
    let trace = fs::read_to_string(&trace_path).unwrap();
    assert!(trace.contains(r#""run_id":"run-sess-A""#));
    assert!(trace.contains(r#""command":"cargo test""#));
}

#[test]
fn hook_ingest_dir_separates_sessions_and_appends_same_session() {
    let dir = tempdir().unwrap();
    let runs = dir.path().join("runs");

    let ingest = |payload: &str| {
        slod()
            .args(["hook", "ingest", "--source", "generic", "--dir"])
            .arg(&runs)
            .write_stdin(payload.to_string())
            .assert()
            .success();
    };

    // Two different sessions => two separate trace files.
    ingest(
        r#"{"hook_event_name":"PreToolUse","tool_name":"Bash","tool_input":{"command":"a"},"session_id":"sess-1"}"#,
    );
    ingest(
        r#"{"hook_event_name":"PreToolUse","tool_name":"Bash","tool_input":{"command":"b"},"session_id":"sess-2"}"#,
    );

    let trace_1 = runs.join("run-sess-1.slod");
    let trace_2 = runs.join("run-sess-2.slod");
    assert!(trace_1.exists());
    assert!(trace_2.exists());

    // Same session again => appended to the same file (seq increments).
    slod()
        .args(["hook", "ingest", "--source", "generic", "--dir"])
        .arg(&runs)
        .write_stdin(
            r#"{"hook_event_name":"PostToolUse","tool_name":"Bash","tool_response":{"success":true,"exit_code":0},"session_id":"sess-1"}"#,
        )
        .assert()
        .success()
        .stdout(predicate::str::contains("tool.result#2"));

    // sess-1 now has two events; the open trace must still verify.
    slod()
        .args(["verify", "--allow-open", "--file"])
        .arg(&trace_1)
        .assert()
        .success();

    let content_1 = fs::read_to_string(&trace_1).unwrap();
    assert_eq!(
        content_1.lines().filter(|l| !l.trim().is_empty()).count(),
        3,
        "sess-1 should hold run.started + tool.call + tool.result"
    );
}

#[test]
fn hook_ingest_serializes_concurrent_creation_and_existing_trace_appends() {
    const CONCURRENT_INGESTS: usize = 24;

    let dir = tempdir().unwrap();
    let runs = dir.path().join("runs");
    let trace_path = runs.join("run-concurrent-session.slod");

    let call_payloads = (0..CONCURRENT_INGESTS)
        .map(|index| {
            format!(
                r#"{{"hook_event_name":"PreToolUse","tool_name":"Bash","tool_input":{{"command":"command-{index}"}},"session_id":"concurrent-session"}}"#
            )
        })
        .collect();
    run_concurrent_hook_ingests(&runs, call_payloads);

    slod()
        .args(["verify", "--allow-open", "--file"])
        .arg(&trace_path)
        .assert()
        .success();

    let result_payloads = (0..CONCURRENT_INGESTS)
        .map(|index| {
            format!(
                r#"{{"hook_event_name":"PostToolUse","tool_name":"Bash","tool_input":{{"command":"command-{index}"}},"tool_response":{{"success":true,"exit_code":0}},"session_id":"concurrent-session"}}"#
            )
        })
        .collect();
    run_concurrent_hook_ingests(&runs, result_payloads);

    slod()
        .args(["finish", "--file"])
        .arg(&trace_path)
        .args(["--status", "success"])
        .assert()
        .success();
    slod()
        .args(["verify", "--file"])
        .arg(&trace_path)
        .assert()
        .success();

    let content = fs::read_to_string(&trace_path).unwrap();
    assert!(
        content.ends_with('\n'),
        "trace must remain newline terminated"
    );
    let events = content
        .lines()
        .map(|line| serde_json::from_str::<Event>(line).expect("every JSONL row must be valid"))
        .collect::<Vec<_>>();

    assert_eq!(
        events.len(),
        2 * CONCURRENT_INGESTS + 2,
        "run.started + calls + results + run.finished"
    );
    assert_eq!(
        events
            .iter()
            .filter(|event| event.kind == EventKind::RunStarted)
            .count(),
        1
    );
    assert_eq!(
        events
            .iter()
            .filter(|event| event.kind == EventKind::RunFinished)
            .count(),
        1
    );
    assert_eq!(
        events.iter().map(|event| event.seq).collect::<Vec<_>>(),
        (0..events.len() as u64).collect::<Vec<_>>(),
        "sequence numbers must be unique and contiguous"
    );

    let expected_commands = (0..CONCURRENT_INGESTS)
        .map(|index| format!("command-{index}"))
        .collect::<BTreeSet<_>>();
    for kind in [EventKind::ToolCall, EventKind::ToolResult] {
        let actual_commands = events
            .iter()
            .filter(|event| event.kind == kind)
            .filter_map(|event| {
                event
                    .payload
                    .get("command")
                    .and_then(|value| value.as_str())
            })
            .map(str::to_string)
            .collect::<BTreeSet<_>>();
        assert_eq!(actual_commands, expected_commands, "missing {kind} events");
    }

    assert!(
        trace_path.with_extension("slod.lock").exists(),
        "the persistent sidecar lock should live beside the trace"
    );
}

#[test]
fn hook_ingest_dir_uses_deterministic_fallback_without_session_id() {
    let dir = tempdir().unwrap();
    let runs = dir.path().join("runs");

    slod()
        .args(["hook", "ingest", "--source", "generic", "--dir"])
        .arg(&runs)
        .write_stdin(
            r#"{"hook_event_name":"PreToolUse","tool_name":"Bash","tool_input":{"command":"cargo build"},"cwd":"/tmp/ws"}"#,
        )
        .assert()
        .success()
        .stdout(predicate::str::contains("tool.call#1"));

    // Exactly one trace file was created, named run-<hash>.slod.
    let entries: Vec<_> = fs::read_dir(&runs)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().to_string())
        .filter(|name| name.ends_with(".slod"))
        .collect();
    assert_eq!(
        entries.len(),
        1,
        "expected one fallback trace, got {entries:?}"
    );
    assert!(entries[0].starts_with("run-"));
    assert!(entries[0].ends_with(".slod"));
}

fn run_concurrent_hook_ingests(runs: &Path, payloads: Vec<String>) {
    assert!(!payloads.is_empty());

    let mut children = Vec::with_capacity(payloads.len());
    for payload in payloads {
        let mut child = StdCommand::new(env!("CARGO_BIN_EXE_slod"))
            .args(["hook", "ingest", "--source", "generic", "--dir"])
            .arg(runs)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("spawn concurrent slod hook ingest");
        let stdin = child.stdin.take().expect("child stdin");
        children.push((child, stdin, payload));
    }

    let barrier = Arc::new(Barrier::new(children.len()));
    let workers = children
        .into_iter()
        .map(|(child, mut stdin, payload)| {
            let barrier = Arc::clone(&barrier);
            thread::spawn(move || {
                barrier.wait();
                stdin
                    .write_all(payload.as_bytes())
                    .expect("write hook payload");
                drop(stdin);

                let output = child.wait_with_output().expect("wait for hook ingest");
                assert!(
                    output.status.success(),
                    "hook ingest failed\nstdout: {}\nstderr: {}",
                    String::from_utf8_lossy(&output.stdout),
                    String::from_utf8_lossy(&output.stderr)
                );
            })
        })
        .collect::<Vec<_>>();

    for worker in workers {
        worker.join().expect("hook ingest worker panicked");
    }
}

#[test]
fn hook_ingest_file_with_init_no_run_id_derives_instead_of_failing() {
    let dir = tempdir().unwrap();
    let trace_path = dir.path().join("session.slod");

    // The old bug: --file --init-if-missing without --run-id failed. Now it
    // derives the run id from the payload and creates the trace.
    slod()
        .args([
            "hook",
            "ingest",
            "--source",
            "generic",
            "--init-if-missing",
            "--file",
        ])
        .arg(&trace_path)
        .write_stdin(
            r#"{"hook_event_name":"PreToolUse","tool_name":"Bash","tool_input":{"command":"cargo test"},"session_id":"sess-file"}"#,
        )
        .assert()
        .success()
        .stdout(predicate::str::contains("tool.call#1"));

    assert!(trace_path.exists());
    let trace = fs::read_to_string(&trace_path).unwrap();
    assert!(trace.contains(r#""run_id":"run-sess-file""#));
}

#[test]
fn hook_ingest_requires_exactly_one_of_file_or_dir() {
    let dir = tempdir().unwrap();

    // Neither --file nor --dir.
    slod()
        .args(["hook", "ingest", "--source", "generic"])
        .current_dir(dir.path())
        .write_stdin(r#"{"hook_event_name":"PreToolUse","tool_name":"Bash"}"#)
        .assert()
        .failure()
        .stderr(predicate::str::contains(
            "pass exactly one of --file or --dir",
        ));
}

/// End-to-end: take the EXACT command line that `hook install` wires into the
/// generated hooks file, run it with a real payload on stdin, and prove it
/// creates a valid per-session trace under `.slod/runs/`. This is the
/// regression guard for the bug where the wired command itself failed at
/// runtime.
#[test]
fn wired_install_command_actually_runs_and_creates_a_trace() {
    let dir = tempdir().unwrap();
    let workspace = dir.path();

    // 1) Generate the real hooks file (non-print mode).
    slod()
        .current_dir(workspace)
        .args(["hook", "install"])
        .assert()
        .success();

    let hooks_file = workspace.join(".agent").join("hooks.json");
    let hooks: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(&hooks_file).unwrap()).unwrap();

    // 2) Extract the exact wired command string from the nested `hooks` object.
    let command = hooks["hooks"]["PreToolUse"][0]["hooks"][0]["command"]
        .as_str()
        .expect("wired command present")
        .to_string();
    assert!(command.contains("--dir .slod/runs"));
    assert!(!command.contains("--init-if-missing"));
    assert!(!command.contains("--file"));

    // 3) Tokenize and run it as the host would, with the payload on stdin,
    //    from the workspace dir. Replace the bare `slod` program with the
    //    test binary so we exercise the same code path the host would invoke.
    let mut tokens = command.split_whitespace();
    assert_eq!(tokens.next(), Some("slod"));
    let args: Vec<&str> = tokens.collect();

    slod()
        .current_dir(workspace)
        .args(&args)
        .write_stdin(
            r#"{"hook_event_name":"PreToolUse","tool_name":"Bash","tool_input":{"command":"cargo test"},"session_id":"wired-session"}"#,
        )
        .assert()
        .success()
        .stdout(predicate::str::contains("tool.call#1"));

    // 4) The trace exists under .slod/runs/ and verifies (open).
    let trace_path = workspace
        .join(".slod")
        .join("runs")
        .join("run-wired-session.slod");
    assert!(
        trace_path.exists(),
        "wired command did not create {}",
        trace_path.display()
    );

    slod()
        .args(["verify", "--allow-open", "--file"])
        .arg(&trace_path)
        .assert()
        .success();
}

/// A realistic host hook payload shape (sanitized). Many agent harnesses surface
/// every shell command to hooks as the canonical tool `"Bash"` with the command
/// under `tool_input.command` (PreToolUse) and the result under `tool_response`
/// (`output` + `exit_code`, PostToolUse), alongside host-specific
/// `turn_id`/`tool_use_id`/`permission_mode` fields. This proves the ingest
/// adapter maps a PreToolUse/PostToolUse pair into a tool.call and a
/// tool.result, with the command, output, and exit code preserved.
#[test]
fn hook_ingest_maps_pre_and_post_tool_use_payloads() {
    let dir = tempdir().unwrap();
    let runs = dir.path().join(".slod").join("runs");

    // PreToolUse as a host actually emits it for a shell command.
    slod()
        .args(["hook", "ingest", "--source", "generic", "--dir"])
        .arg(&runs)
        .write_stdin(
            r#"{"session_id":"00000000-0000-0000-0000-000000000000","turn_id":"11111111-1111-1111-1111-111111111111","transcript_path":"/tmp/ws/transcript.jsonl","cwd":"/tmp/ws","hook_event_name":"PreToolUse","permission_mode":"default","tool_name":"Bash","tool_input":{"command":"cargo test"},"tool_use_id":"call-aaaa"}"#,
        )
        .assert()
        .success()
        .stdout(predicate::str::contains("tool.call#1"));

    // PostToolUse for the same session: result nested under tool_response.
    slod()
        .args(["hook", "ingest", "--source", "generic", "--dir"])
        .arg(&runs)
        .write_stdin(
            r#"{"session_id":"00000000-0000-0000-0000-000000000000","turn_id":"11111111-1111-1111-1111-111111111111","transcript_path":"/tmp/ws/transcript.jsonl","cwd":"/tmp/ws","hook_event_name":"PostToolUse","permission_mode":"default","tool_name":"Bash","tool_input":{"command":"cargo test"},"tool_response":{"output":"test result: ok. 12 passed","exit_code":0},"tool_use_id":"call-aaaa"}"#,
        )
        .assert()
        .success()
        .stdout(predicate::str::contains("tool.result#2"));

    // Both events landed in one per-session trace and carry the real fields.
    let trace_path = runs.join("run-00000000-0000-0000-0000-000000000000.slod");
    assert!(
        trace_path.exists(),
        "expected per-session trace for the host session id"
    );
    let trace = fs::read_to_string(&trace_path).unwrap();
    assert!(trace.contains(r#""hook_event":"PreToolUse""#));
    assert!(trace.contains(r#""hook_event":"PostToolUse""#));
    assert!(trace.contains(r#""tool":"Bash""#));
    assert!(trace.contains(r#""command":"cargo test""#));
    // The PostToolUse result fields survived the tool_response.* extraction.
    assert!(trace.contains(r#""exit_code":0"#));
    assert!(trace.contains("test result: ok. 12 passed"));
    assert!(trace.contains(r#""success":true"#));

    slod()
        .args(["summary", "--file"])
        .arg(&trace_path)
        .assert()
        .success()
        .stdout(predicate::str::contains("tool_calls: 1"))
        .stdout(predicate::str::contains("tool_results: 1"));
}
