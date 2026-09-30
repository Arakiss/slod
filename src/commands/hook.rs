//! Hook commands: ingest host hook payloads and install host wiring.

use std::{
    fs,
    io::{self, Read},
    path::{Path, PathBuf},
};

use anyhow::{Context, Result, bail};
use serde_json::{Map, Value, json};
use slod::{
    hook::{derive_run_id, map_hook_payload, normalize_source},
    trace::{EventKind, Trace},
};

use super::{parse_payload, print_action};

pub(crate) fn ingest(
    file: Option<&Path>,
    dir: Option<&Path>,
    source: &str,
    run_id: Option<&str>,
    init_if_missing: bool,
) -> Result<()> {
    // The source is a free-form label the host chooses; slod stores it
    // verbatim and never special-cases any harness name.
    let source = normalize_source(source);
    let mut input = String::new();
    io::stdin()
        .read_to_string(&mut input)
        .context("failed to read hook payload from stdin")?;
    if input.trim().is_empty() {
        bail!("missing hook JSON on stdin");
    }

    let payload = parse_payload(&input)?;
    let events = map_hook_payload(&source, &payload)?;

    // The effective run id is the explicit --run-id, or one derived from the
    // payload's session so a wired hook never has to know the run id up front.
    let effective_run_id = run_id
        .map(str::to_string)
        .unwrap_or_else(|| derive_run_id(&payload));

    // Resolve the trace file and whether a missing one should be created.
    // `--dir` is per-session: it computes the path and always initializes the
    // session trace on first use. `--file` keeps the explicit `--init-if-missing`
    // gate but no longer fails for a missing `--run-id` (it derives one).
    let (trace_file, create_if_missing): (PathBuf, bool) = match (dir, file) {
        (Some(dir), None) => {
            fs::create_dir_all(dir)
                .with_context(|| format!("failed to create {}", dir.display()))?;
            (dir.join(format!("{effective_run_id}.slod")), true)
        }
        (None, Some(file)) => (file.to_path_buf(), init_if_missing),
        (Some(_), Some(_)) => bail!("pass exactly one of --file or --dir, not both"),
        (None, None) => bail!("pass exactly one of --file or --dir"),
    };

    for event in &events {
        if matches!(event.kind, EventKind::RunStarted | EventKind::RunFinished) {
            bail!("hook ingest cannot create lifecycle events directly");
        }
    }

    let recorded = Trace::with_exclusive(&trace_file, |trace| {
        if !trace.exists() {
            if !create_if_missing {
                bail!(
                    "trace does not exist: {}; pass --init-if-missing to create it (or use --dir)",
                    trace_file.display()
                );
            }
            trace.init(&effective_run_id, false)?;
        }

        let mut recorded = Vec::with_capacity(events.len());
        for event in events {
            let event = trace.append(event.kind, event.payload)?;
            recorded.push(format!("{}#{}", event.kind, event.seq));
        }
        Ok(recorded)
    })?;

    print_action(
        "hook ingest",
        &[
            ("file", trace_file.display().to_string()),
            ("source", source),
            ("events", recorded.join(", ")),
        ],
    );
    Ok(())
}

/// Close the trace of a finished host session with a `run.finished` event.
///
/// Wired on the host's session-ending hook, this is what makes `slod verify`
/// (which requires a closed trace) pass on real captured sessions. It reads the
/// same hook payload shape `ingest` does and derives the same run id, so the
/// hook needs no knowledge of where the trace lives.
///
/// Three properties matter for a hook that runs unattended:
///
/// * **Idempotent.** A trace that is already closed is left alone, and closing
///   is reported as a no-op rather than an error.
/// * **Silent on absence.** A session that never triggered an ingest has no
///   trace; there is nothing to close and that is not a failure.
/// * **Never creates.** Unlike `ingest --dir`, close never initializes a trace.
///
/// The `run.finished` payload carries an outcome census of the trace it closes,
/// including how many `tool.call` events never received a `tool.result`. That
/// asymmetry is the only failure signal available from a host that drops its
/// post-tool hook when a tool fails, so it is recorded explicitly instead of
/// being left for every reader to recompute.
pub(crate) fn close(
    file: Option<&Path>,
    dir: Option<&Path>,
    source: &str,
    run_id: Option<&str>,
    status: &str,
) -> Result<()> {
    let source = normalize_source(source);
    let mut input = String::new();
    io::stdin()
        .read_to_string(&mut input)
        .context("failed to read hook payload from stdin")?;
    if input.trim().is_empty() {
        bail!("missing hook JSON on stdin");
    }

    let payload = parse_payload(&input)?;
    if !payload.is_object() {
        bail!("hook payload must be a JSON object");
    }

    let effective_run_id = run_id
        .map(str::to_string)
        .unwrap_or_else(|| derive_run_id(&payload));

    let trace_file: PathBuf = match (dir, file) {
        (Some(dir), None) => dir.join(format!("{effective_run_id}.slod")),
        (None, Some(file)) => file.to_path_buf(),
        (Some(_), Some(_)) => bail!("pass exactly one of --file or --dir, not both"),
        (None, None) => bail!("pass exactly one of --file or --dir"),
    };

    if !trace_file.exists() {
        print_action(
            "hook close",
            &[
                ("file", trace_file.display().to_string()),
                ("result", "no trace for this session".to_string()),
            ],
        );
        return Ok(());
    }

    let outcome = Trace::with_exclusive(&trace_file, |trace| {
        if !trace.exists() {
            return Ok(None);
        }
        let existing = trace.read()?;
        if existing
            .events
            .last()
            .is_some_and(|event| event.kind == EventKind::RunFinished)
        {
            return Ok(None);
        }

        let mut finished = json!({
            "status": status,
            "source": source,
            "outcomes": census(&existing),
        });
        let object = finished
            .as_object_mut()
            .expect("run.finished payload is an object");
        if let Some(hook_event) = string_at(&payload, "hook_event_name") {
            object.insert("hook_event".to_string(), Value::String(hook_event));
        }
        if let Some(reason) = string_at(&payload, "reason") {
            object.insert("reason".to_string(), Value::String(reason));
        }

        trace.append(EventKind::RunFinished, finished).map(Some)
    })?;

    let result = match &outcome {
        Some(event) => format!("closed at {}#{}", event.kind, event.seq),
        None => "already closed".to_string(),
    };
    print_action(
        "hook close",
        &[
            ("file", trace_file.display().to_string()),
            ("source", source),
            ("status", status.to_string()),
            ("result", result),
        ],
    );
    Ok(())
}

/// Count what the trace being closed actually recorded.
///
/// `unanswered_calls` is the interesting one: a `tool.call` with no matching
/// `tool.result`. Hosts that simply do not fire their post-tool hook when a
/// tool errors leave exactly this shape behind, so the count is a lower bound
/// on failures even when every delivered result is inconclusive.
fn census(trace: &Trace) -> Value {
    let mut calls = 0_u64;
    let mut results = 0_u64;
    let mut succeeded = 0_u64;
    let mut failed = 0_u64;
    let mut unknown = 0_u64;
    let mut errors = 0_u64;

    for event in &trace.events {
        match event.kind {
            EventKind::ToolCall => calls += 1,
            EventKind::ToolResult => {
                results += 1;
                match event.payload.get("success") {
                    Some(Value::Bool(true)) => succeeded += 1,
                    Some(Value::Bool(false)) => failed += 1,
                    _ => unknown += 1,
                }
            }
            EventKind::Error => errors += 1,
            _ => {}
        }
    }

    json!({
        "tool_calls": calls,
        "tool_results": results,
        "unanswered_calls": calls.saturating_sub(results),
        "succeeded": succeeded,
        "failed": failed,
        "unknown": unknown,
        "errors": errors,
    })
}

fn string_at(payload: &Value, key: &str) -> Option<String> {
    payload
        .get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|text| !text.is_empty())
        .map(str::to_string)
}

/// Wire an agent host so it pipes hook payloads into `slod hook ingest`.
///
/// The wiring is written to a local hooks file (default `.agent/hooks.json`) in
/// the standard `PreToolUse`/`PostToolUse` shape most agent harnesses read. The
/// merge is idempotent and only ever touches the given local file. When the
/// host's settings file is global or delicate, use `--print` and paste the
/// printed snippet by hand instead of writing it.
pub(crate) fn install(file: &Path, source: &str, print: bool, run_id: Option<&str>) -> Result<()> {
    let source = normalize_source(source);
    let command = ingest_command_line(&source, run_id);

    if print {
        // Show the exact shape `install` writes (and a host reads): hook entries
        // nested under a top-level `hooks` object, keyed by lifecycle event.
        let preview = json!({
            "hooks": {
                "PreToolUse": [hook_entry(&command)],
                "PostToolUse": [hook_entry(&command)],
            },
        });
        print_action(
            "hook install (print)",
            &[
                ("file", file.display().to_string()),
                ("command", command.clone()),
                (
                    "note",
                    "paste this into the host's hooks/settings file; slod writes nothing in --print mode".to_string(),
                ),
            ],
        );
        println!("{}", serde_json::to_string_pretty(&preview)?);
        return Ok(());
    }

    let mut root = read_hooks(file)?;
    let pre_added = merge_hook(&mut root, "PreToolUse", &command);
    let post_added = merge_hook(&mut root, "PostToolUse", &command);

    if let Some(parent) = file
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        fs::create_dir_all(parent)
            .with_context(|| format!("failed to create {}", parent.display()))?;
    }
    fs::write(file, format!("{}\n", serde_json::to_string_pretty(&root)?))
        .with_context(|| format!("failed to write {}", file.display()))?;

    let status = match (pre_added, post_added) {
        (true, true) => "wired PreToolUse and PostToolUse",
        (true, false) => "wired PreToolUse (PostToolUse already present)",
        (false, true) => "wired PostToolUse (PreToolUse already present)",
        (false, false) => "already wired",
    };
    print_action(
        "hook install",
        &[
            ("file", file.display().to_string()),
            ("command", command),
            ("result", status.to_string()),
        ],
    );
    Ok(())
}

/// Build the `slod hook ingest` command line a host hook should run. The
/// payload arrives on stdin. We wire `--dir .slod/runs` so each host
/// session lands in its own per-session trace (`<run_id>.slod`) that is
/// created on first use; the run id is derived from the payload's session, so
/// the wired command never has to carry `--run-id` or `--init-if-missing`.
/// An explicit `--run-id` is still pinned when the operator passes one.
fn ingest_command_line(source: &str, run_id: Option<&str>) -> String {
    let mut command = format!("slod hook ingest --source {source} --dir .slod/runs");
    if let Some(run_id) = run_id {
        command.push_str(&format!(" --run-id {run_id}"));
    }
    command
}

/// A single hook matcher group. Most agent harnesses report shell commands to
/// hooks under the canonical tool name `"Bash"`, and the `matcher` is a regex
/// applied to that `tool_name`, so `"Bash"` is the matcher that captures shell
/// tool calls.
fn hook_entry(command: &str) -> Value {
    json!({
        "matcher": "Bash",
        "hooks": [{ "type": "command", "command": command }],
    })
}

fn read_hooks(file: &Path) -> Result<Value> {
    if !file.exists() {
        return Ok(Value::Object(Map::new()));
    }
    let content =
        fs::read_to_string(file).with_context(|| format!("failed to read {}", file.display()))?;
    if content.trim().is_empty() {
        return Ok(Value::Object(Map::new()));
    }
    let value: Value = serde_json::from_str(&content)
        .with_context(|| format!("invalid JSON in {}", file.display()))?;
    if !value.is_object() {
        bail!("{} must contain a JSON object", file.display());
    }
    Ok(value)
}

/// Borrow (creating if needed) the top-level `hooks` object a host reads. Hosts
/// discover PreToolUse/PostToolUse config nested under a top-level `hooks` key,
/// so we always merge there rather than at the file root. Any unrelated
/// top-level config (e.g. a sibling `notify` block) is left untouched.
fn hooks_object(root: &mut Value) -> Option<&mut Map<String, Value>> {
    let map = root.as_object_mut().expect("hooks root is a JSON object");
    let hooks = map
        .entry("hooks".to_string())
        .or_insert_with(|| Value::Object(Map::new()));
    hooks.as_object_mut()
}

/// Insert a slod hook entry under `hooks.<event>` while preserving any
/// existing entries. Returns false (idempotent no-op) when an equivalent
/// slod entry is already present, or when foreign config blocks the merge.
fn merge_hook(root: &mut Value, event: &str, command: &str) -> bool {
    let Some(hooks) = hooks_object(root) else {
        // A non-object value under the `hooks` key is foreign config we do not
        // own; leave it untouched and report no change.
        return false;
    };
    let entries = hooks
        .entry(event.to_string())
        .or_insert_with(|| Value::Array(Vec::new()));
    let Some(entries) = entries.as_array_mut() else {
        // A non-array value under the event key is foreign config we do not
        // own; leave it untouched and report no change.
        return false;
    };

    let already_present = entries.iter().any(entry_has_slod);
    if already_present {
        return false;
    }
    entries.push(hook_entry(command));
    true
}

/// True when a hook entry already runs a `slod hook ingest` command.
fn entry_has_slod(entry: &Value) -> bool {
    entry
        .get("hooks")
        .and_then(Value::as_array)
        .map(|hooks| {
            hooks.iter().any(|hook| {
                hook.get("command")
                    .and_then(Value::as_str)
                    .is_some_and(|command| command.contains("slod hook ingest"))
            })
        })
        .unwrap_or(false)
}
