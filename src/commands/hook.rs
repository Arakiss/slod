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
    redaction::redact,
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

    let payload = redact(&parse_payload(&input)?);
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
        (Some(dir), None) => (dir.join(format!("{effective_run_id}.slod")), true),
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
