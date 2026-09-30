# Agent hook ingestion

Slod can ingest host hook payloads from stdin and turn them into local
trace events. This is the first integration surface for any agent harness that
emits lifecycle hook payloads, where tools, permission decisions, and failures
happen outside a single wrapped shell command.

The command a wired host hook runs is:

```bash
slod hook ingest \
  --source generic \
  --dir .slod/runs
```

The hook JSON payload is read from stdin. With `--dir`, slod derives a
per-session run id from the payload (`run-<session_id>`, or a deterministic
fallback when the host sends no session id), writes to
`<dir>/<run_id>.slod`, and creates that trace on first use. The wired
command therefore never has to know the run id up front, and every event from
the same host session lands in the same trace while different sessions stay in
separate files.

You can still target one explicit file instead of a per-session directory:

```bash
slod hook ingest \
  --source generic \
  --file "$SLOD_FILE" \
  --init-if-missing
```

In `--file` mode, `--init-if-missing` lets the first hook event create the
trace file. The run id is taken from `--run-id` when given, or derived from the
payload otherwise. Pass exactly one of `--file` or `--dir`.

## The source label

`--source` is a free-form label the host chooses. Slod stores it verbatim
on every mapped event and never special-cases any harness name, so it stays
agnostic to the tool that produced the hook. The label defaults to `generic`
(an empty or whitespace-only value falls back to `generic`). Use it to tag
events by harness, by policy layer, or by any source you find useful, e.g.
`--source generic`, `--source policy`, or `--source my-harness`.

The adapter is intentionally tolerant. It looks for common host fields such as
`hook_event_name`, `tool_name`, `tool_input`, `tool_response`, `decision`,
`success`, `exit_code`, and `error`. Unknown host fields are not part of the
public v0.1 contract.

## Event mapping

| Host payload shape | Slod event |
| --- | --- |
| `PreToolUse`, `tool_input`, or `tool_name` | `tool.call` |
| `PostToolUse`, `tool_response`, `success`, or `exit_code` | `tool.result` |
| `decision` / `permission_decision` | `permission.decision` |
| `HookError` / `error` | `error` |

Permission decisions take precedence over tool calls because many host systems
make the decision inside a pre-tool hook.

## Shell hook pattern

Use a tiny shell wrapper from the host hook. The exact environment variables
depend on the host, but the pattern is stable:

```bash
#!/usr/bin/env sh
set -eu

runs_dir="${SLOD_DIR:-.slod/runs}"
source="${SLOD_SOURCE:-generic}"

slod hook ingest \
  --source "$source" \
  --dir "$runs_dir"
```

The host should pipe its hook JSON payload into the script:

```bash
printf '%s' "$HOOK_PAYLOAD_JSON" | ./slod-hook.sh
```

## Example payloads

These mirror the shape many agent harnesses send a hook on stdin for a shell
command: `tool_name` is the canonical `"Bash"`, the command is under
`tool_input.command`, and the result is under `tool_response`
(`output` + `exit_code`), alongside host-specific
`turn_id`/`tool_use_id`/`permission_mode` fields. Slod only reads the
fields it recognizes and ignores the rest.

Tool call (`PreToolUse`):

```json
{
  "session_id": "00000000-0000-0000-0000-000000000000",
  "turn_id": "11111111-1111-1111-1111-111111111111",
  "transcript_path": "/path/to/transcript.jsonl",
  "cwd": "/path/to/workspace",
  "hook_event_name": "PreToolUse",
  "permission_mode": "default",
  "tool_name": "Bash",
  "tool_input": {
    "command": "cargo test"
  },
  "tool_use_id": "call-aaaa"
}
```

Tool result (`PostToolUse`):

```json
{
  "session_id": "00000000-0000-0000-0000-000000000000",
  "turn_id": "11111111-1111-1111-1111-111111111111",
  "hook_event_name": "PostToolUse",
  "tool_name": "Bash",
  "tool_input": {
    "command": "cargo test"
  },
  "tool_response": {
    "output": "test result: ok. 12 passed",
    "exit_code": 0
  },
  "tool_use_id": "call-aaaa"
}
```

The adapter also accepts a `success`/`stdout`/`duration_ms` result shape from
other hosts, and a `tool_response` that is a rendered string rather than an
object.

## Outcomes are three-state

`success` on a `tool.result` is `true`, `false`, or `null`. `null` means the
host reported nothing slod can read, and it is recorded as `null` rather than
`true`: a store where every result claims success cannot answer the one
question it exists for. Every `tool.result` also carries `outcome_source`,
naming the field that decided the verdict, so a reader can tell a measured
outcome from an absent one.

The resolution order is:

| Signal read | Verdict | `outcome_source` |
| --- | --- | --- |
| `is_error`, `interrupted` or `timed_out` is true | failure | that field |
| `success` / `ok` / `status.success` | as stated | `success` |
| `is_error` is false | success | `is_error` |
| `exit_code` (any of the probed paths) | zero is success | `exit_code` |
| `Exit code: N` / `Error: Exit code N` in the first line of a textual response | zero is success | `response_text` |
| a non-empty `error` message | failure | `error` |
| nothing above | unknown (`null`) | `none` |

Two deliberate omissions:

- **A non-empty stderr is not a failure.** Well-behaved commands write progress
  and warnings to stderr and exit 0. Stderr is captured as evidence — the first
  500 bytes, truncated on a character boundary and marked `[truncated]` — but
  never used as a verdict.
- **`interrupted: false` is not a success.** It rules out an interruption and
  says nothing about the exit status. Only `is_error: false` is a host
  asserting the call did not fail.

A consequence worth knowing before reading a store: some hosts do not fire
their post-tool hook at all when a tool fails. Those failures never reach
`hook ingest` in any shape, and show up instead as a `tool.call` with no
matching `tool.result`. `hook close` counts exactly that (see below).

Permission decision:

```json
{
  "hook_event_name": "PreToolUse",
  "tool_name": "Write",
  "tool_input": {
    "command": "edit README.md"
  },
  "decision": "allow",
  "reason": "trusted repo workspace"
}
```

## Installing the wiring

`slod hook install` writes the host wiring so a harness pipes its hook
payloads into `hook ingest`. It is idempotent and opt-in.

```bash
slod hook install
```

- `--file <path>` is the local hooks file to merge into (default
  `.agent/hooks.json`, relative to the current directory).
- `--source <label>` is the free-form source label pinned into the wired
  command (default `generic`).
- `--print` prints the planned wiring without writing anything.
- `--run-id <id>` pins a run id into the generated ingest commands.

Install merges slod `PreToolUse` and `PostToolUse` entries into the local
hooks file. The entries are nested under the top-level `hooks` object a host
discovers, and each uses `"matcher": "Bash"`. Most agent harnesses surface
every shell command they run to hooks as the canonical tool name `"Bash"`, and
the matcher is a regex applied to that `tool_name`, so `"Bash"` is what captures
shell tool calls. The generated file looks like:

```json
{
  "hooks": {
    "PreToolUse": [
      {
        "matcher": "Bash",
        "hooks": [
          { "type": "command", "command": "slod hook ingest --source generic --dir .slod/runs" }
        ]
      }
    ],
    "PostToolUse": [
      {
        "matcher": "Bash",
        "hooks": [
          { "type": "command", "command": "slod hook ingest --source generic --dir .slod/runs" }
        ]
      }
    ]
  }
}
```

The wired command is `slod hook ingest --source generic --dir
.slod/runs`, so each host session gets its own per-session trace with no
`--run-id` or `--init-if-missing` to manage. Install preserves existing entries
(including unrelated config under `hooks`) and never duplicates a slod
entry, so re-running is safe. It only ever touches the local file you point it
at, never a global one.

### When hooks fire

Whether a hook fires depends on the host. Some harnesses run `PreToolUse` and
`PostToolUse` hooks only during interactive sessions and skip them in a
non-interactive batch mode; others run them in both. Slod writes the
standard hook wiring and ingests whatever the host actually sends. When a host
does not run hooks in a given mode, capture traces with the `slod run`
wrapper or by piping host payloads into `hook ingest` directly instead.

When a host's settings file is global or otherwise delicate, prefer
`hook install --print` and paste the printed snippet by hand. Slod never
writes a file in `--print` mode.

## Closing a captured session

`hook ingest` is forbidden from creating lifecycle events, so a trace built
only from tool hooks never gets its `run.finished` and `slod verify` rejects
it. `slod hook close` is the other half of the wiring:

```bash
slod hook close --source generic --dir .slod/runs
```

It reads the session-ending hook payload from stdin and derives the run id the
same way `ingest` does, so the wired command needs no knowledge of where the
trace lives. It is safe to run unattended:

- an already closed trace is left alone and reported as `already closed`;
- a session that never triggered an ingest has no trace, which is reported and
  is not an error;
- it never creates a trace.

**Wire it on a session-ending hook, never on a per-turn one.** Closing a trace
forbids further appends, so a hook that fires at the end of every turn (`Stop`
on most harnesses) would end capture at the first turn. `SessionEnd` is the
event to use.

The `run.finished` payload carries a census of the trace it closes:

```json
{
  "status": "closed",
  "source": "generic",
  "hook_event": "SessionEnd",
  "outcomes": {
    "tool_calls": 2,
    "tool_results": 1,
    "unanswered_calls": 1,
    "succeeded": 1,
    "failed": 0,
    "unknown": 0,
    "errors": 0
  }
}
```

`unanswered_calls` is the one that earns its place: it is the count of
`tool.call` events that never received a `tool.result`, which is the only
failure signal available from a host that drops its post-tool hook when a tool
fails.

## Retention

A capture store that only ever grows stops being local-first the day it fills
the disk. `slod prune` drops traces outside a retention window:

```bash
slod prune --dir .slod/runs --older-than 30
```

It is a **dry run by default**: it reports how many traces and lock sidecars it
would remove and how much that frees, then exits without touching anything.
Pass `--apply` to actually delete. It only ever considers regular files named
`*.slod` and `*.slod.lock` directly inside `--dir`; it never recurses, never
removes `ledger.slod`, and leaves foreign files alone. Lock sidecars whose
trace is already gone are removed at any age.

## Auditing a trace against policy

`slod policy-check` audits a recorded trace and exits non-zero when it
finds a violation:

```bash
slod policy-check --file .slod/runs/session.slod
```

- `--file <path>` is the trace to audit (required).
- `--allow-open` audits an open (not yet finished) trace, matching `verify`.

It reports two classes of violation:

1. a `permission.decision` deny/denied/block with no later `allow` resolving the
   same capability/command (an unresolved deny);
2. a `tool.call` that maps to a sensitive public capability (`git push` /
   `git.push` in the command or capability) with no prior or simultaneous
   `permission.decision` allow.

Run the simulated integration smoke, which now also exercises `hook install`
and `policy-check`:

```bash
sh scripts/hook-smoke.sh
```

That smoke proves the full local lifecycle: hook ingestion, finish, verify,
summary, inspect, render, ledger rebuild/list/show, hook install (merge +
idempotency), and policy-check (clean and violating traces).
