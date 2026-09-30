<p align="center">
  <img src="assets/banner.svg?v=20260503-pipeline" alt="slod - inspectable traces for AI agent workflows" width="100%" />
</p>
<p align="center"><sub><em>The frame is the harness. The trace is what lets the next agent understand the run.</em></sub></p>

<p align="center">
  <a href="https://github.com/Arakiss/slod/actions/workflows/ci.yml"><img src="https://github.com/Arakiss/slod/actions/workflows/ci.yml/badge.svg" alt="CI"></a>
  <a href="https://github.com/Arakiss/slod/actions/workflows/audit.yml"><img src="https://github.com/Arakiss/slod/actions/workflows/audit.yml/badge.svg" alt="Audit"></a>
  <a href="https://scorecard.dev/viewer/?uri=github.com/Arakiss/slod"><img src="https://api.scorecard.dev/projects/github.com/Arakiss/slod/badge" alt="OpenSSF Scorecard"></a>
  <a href="https://github.com/Arakiss/slod/releases"><img src="https://img.shields.io/github/v/release/Arakiss/slod?sort=semver&color=blue" alt="Latest release"></a>
  <a href="LICENSE"><img src="https://img.shields.io/badge/license-MIT-yellow.svg" alt="License: MIT"></a>
  <a href="Cargo.toml"><img src="https://img.shields.io/badge/rust-1.94%2B-orange.svg" alt="Rust 1.94+"></a>
  <a href="examples/agent-run.slod"><img src="https://img.shields.io/badge/trace-local%20file-blue.svg" alt="Trace local file"></a>
  <a href="README.md"><img src="https://img.shields.io/badge/status-local%20MVP-blue.svg" alt="Status: local MVP"></a>
</p>

# slod

> _slod_ — Old Norse *slóð*: the trail something leaves as it moves. An agent
> run leaves a slóð; this tool makes it append-only, verifiable, and
> inspectable. Renamed from `traceframe` on 2026-07-07 (old GitHub URLs
> redirect); the name is host-neutral by design — Claude Code, Codex, Cursor,
> or any future harness leave the same kind of trail.

> _Agents do not need more autonomy before they have inspectable traces._

**A local-first Rust library and CLI for AI agent workflow traces.**

> **Development status: local MVP.** Slod is public, installable from source, and verified by CI, but the trace schema and CLI are still intentionally narrow. Expect breaking schema/CLI changes while the project is tested against real agent workflows. Use it first for local harness inspection, examples, and failure analysis.

> Direction lives in [ROADMAP.md](ROADMAP.md). Agents (and humans) working on
> this repo start at [AGENTS.md](AGENTS.md).

Slod records what an AI agent actually did: model calls, tool calls,
permission decisions, command results, errors, final state, and the order in
which those things happened. It is a small Rust crate and CLI for local harness
engineering, not a SaaS dashboard.

## Where it fits

A serious agent harness has multiple layers:

1. **Runtime and sandbox controls**: the agent harness, containers, OS
   confinement, and native approval modes.
2. **Policy decision gateways**: a policy layer that decides whether an agent
   may perform a capability.
3. **Trace capture**: the ordered run artifact showing what happened, which
   decisions were made, which tools ran, what failed, and how the run ended.
4. **Review and conversion**: humans or follow-up agents turn failed traces into
   policies, tests, evals, or workflow fixes.
5. **Export surfaces**: OpenTelemetry, dashboards, issue reports, PR comments,
   or HTML summaries.

Slod owns layer 3. It does not try to own the whole stack.

A policy layer answers:

```text
What is this agent allowed to do?
```

Slod answers:

```text
What did this agent actually do, and why did it fail?
```

## Why

Agent failures are often hard to review after the fact. A transcript is not a
trace. A shell log is not a trace. A permission decision alone does not explain
the full episode around it.

The fix is a durable artifact: an agent should leave evidence that another agent
or a human can inspect after the run — not a transcript, not a shell log, but an
ordered record of what happened. Slod is deliberately narrow: it records
the run, and nothing more.

Slod takes a narrow stance:

- **Local-first.** A trace is a file you can inspect, diff, archive, attach to
  an issue, or hand to another agent.
- **Append-only trace files.** Each event is one durable record. Partial writes
  are recoverable and agent-readable.
- **Harness-oriented.** Events are about runs, model calls, tool calls,
  permission decisions, errors, and final state.
- **No SaaS dependency.** Dashboards and OpenTelemetry can come later as export
  surfaces, not as the core contract.
- **Useful failure artifacts.** A failed run should become a policy, test, eval,
  or workflow improvement.

## Install

From this repository:

```bash
cargo install --path .
```

## Quick start

```bash
slod run --file run.slod --run-id run-demo -- cargo test
slod summary --file run.slod
slod inspect --file run.slod
slod render --file run.slod --html slod.html
```

For longer workflows, keep a trace open and append events as the harness runs:

```bash
slod init --file workflow.slod --run-id run-demo
slod record --file workflow.slod --kind model.call --payload '{"provider":"openai","model":"gpt"}'
slod record --file workflow.slod --kind permission.decision --payload '{"capability":"fs.write:README.md","decision":"allow"}'
slod exec --file workflow.slod -- cargo test
slod finish --file workflow.slod --status success
slod verify --file workflow.slod
slod summary --file workflow.slod
slod inspect --file workflow.slod
slod render --file workflow.slod --html slod.html
```

`record` remains available for raw structured events. For day-to-day harness
use, `run`, `exec`, and `finish` avoid hand-writing the common `tool.call`,
`tool.result`, and `run.finished` payloads. `summary`, `inspect`, and `render`
also work on open traces so interrupted agent runs can still be reviewed.

## Import existing transcripts

Backfill traces from transcripts your harness already wrote (see
[docs/import.md](docs/import.md)):

```bash
slod import --format claude-code --input session.jsonl
slod import --format codex --input ~/.codex/sessions/YYYY/MM/DD/rollout-example.jsonl
slod ledger rebuild
```

Once runs accumulate under `.slod/runs/`, rebuild the local ledger. Omit
`--file` when you want `run` to use the default local run directory:

```bash
slod run --run-id run-demo -- cargo test
slod ledger rebuild
slod ledger list
slod ledger list --status failed
slod ledger show --run-id run-demo
slod ledger export --jsonl
```

The ledger is a derived catalog, not a database and not a second source of
truth. If it is stale or deleted, rebuild it from the trace files. Use
`ledger export --jsonl` when a layer-4 consumer needs a stable machine-readable
catalog instead of parsing the local ledger file directly.

For host hooks, ingest the JSON payload from stdin instead of wrapping each
command manually. Use `--dir` for per-session traces: slod derives the
run id from the payload's session, writes `<dir>/<run_id>.slod`, and
creates it on first use, so the wired command needs no `--run-id` or
`--init-if-missing`:

```bash
slod hook ingest \
  --source generic \
  --dir .slod/runs <<'JSON'
{"hook_event_name":"PreToolUse","tool_name":"Bash","tool_input":{"command":"cargo test"},"session_id":"host-session"}
JSON
```

`--source` is a free-form label the host chooses (default `generic`);
slod stores it verbatim and never names a specific harness.

To target one explicit trace file instead, pass `--file` (with
`--init-if-missing` for the first event). Pass exactly one of `--file` or
`--dir`.

To wire a host so it pipes hook payloads into `hook ingest`, use the idempotent
installer. It merges slod entries into a local hooks file (default
`.agent/hooks.json`), or prints a snippet to paste by hand when the host's
settings file is global or delicate:

```bash
slod hook install
slod hook install --print
```

The wired command is `slod hook ingest --source generic --dir .slod/runs`:
it derives the run id from the host session id and writes one
`<run-id>.slod` per session.

Wire the host's **session-ending** hook — not a per-turn one — to close the
trace, so `slod verify` passes on the captured session:

```bash
slod hook close --source generic --dir .slod/runs
```

`hook close` is idempotent, never creates a trace, and records a census of what
the session captured, including `tool.call` events that never received a
`tool.result`. Outcomes on a `tool.result` are three-state: `true`, `false`, or
`null` when the host reported nothing readable — slod never defaults a result to
success. See [`docs/hooks.md`](docs/hooks.md).

Keep the store bounded with a retention window; it is a dry run unless
`--apply` is passed:

```bash
slod prune --dir .slod/runs --older-than 30
```

To capture a real agent session end to end
(wire → run a real agent → verify → render), run
[`scripts/capture-session.sh`](scripts/capture-session.sh) (set `AGENT_CMD` to
launch your harness, or let it fall back to a `slod run` recording);
[`examples/agent-session.slod`](examples/agent-session.slod) is a
real (sanitized) capture of one such session.

Once a run is recorded, audit it against capability/permission policy:

```bash
slod policy-check --file .slod/runs/agent-run-demo.slod
```

`policy-check` fails when a permission deny is never resolved by a later allow,
or when a sensitive public capability (`git push` / `git.push`) ran without a
recorded permission allow.

Use `verify` + `policy-check` as a **gate** in CI or a pre-push hook so a public
action is blocked when the run behind it isn't clean. See
[`docs/ci-gate.md`](docs/ci-gate.md) and the example
[`.github/workflows/evidence-gate.yml`](.github/workflows/evidence-gate.yml).

See [`docs/hooks.md`](docs/hooks.md) for the agent-hook pattern, the installer,
and the policy-check rules.

## Example output

```text
run_id: run-agent-demo
status: failed
events: 8
model_calls: 1
tool_calls: 1
tool_results: 1
permission_decisions: 2
errors: 1
duration_ms: 110
```

See [`examples/agent-run.slod`](examples/agent-run.slod)
for a sample run with an allowed permission, a denied permission, a failed tool
result, and a final failed state.

## Library API

Rust harnesses can write traces directly with `TraceRecorder`:

```rust
use slod::trace::TraceRecorder;

let recorder = TraceRecorder::start(
    ".slod/runs/my-agent-run.slod",
    "my-agent-run",
    true,
)?;

recorder.model_call("openai", "gpt-5.5")?;
recorder.permission_decision("fs.write:README.md", "allow")?;
recorder.tool_call("shell", "cargo test", ["cargo", "test"])?;
recorder.tool_result("shell", "cargo test", true, Some(0), Some(320))?;
recorder.finish("success", Some("harness completed"))?;
```

See [`docs/harness-integration.md`](docs/harness-integration.md) and
[`examples/harness-recorder.rs`](examples/harness-recorder.rs).

## Event model

v0.1 supports one run per trace file. The public contract is the event model;
the current local file encoding is line-delimited JSON for simple append,
inspection, and recovery.

Required event fields:

- `version`
- `run_id`
- `event_id`
- `kind`
- `ts_ms`
- `seq`
- `payload`

Supported event kinds:

- `run.started`
- `model.call`
- `tool.call`
- `tool.result`
- `permission.decision`
- `error`
- `run.finished`

## CLI experience

Slod's CLI is designed for both humans and agents:

- commands print stable, aligned summaries;
- `run` creates, records, and closes a command trace in one step;
- wrapped command stdout/stderr is preserved;
- `exec` returns the wrapped command's exit code;
- `hook ingest` lets any agent host append tool, result, permission, and
  error events from stdin;
- `hook install` idempotently wires a local hooks file (or prints a snippet to
  paste by hand) so a host pipes payloads into `hook ingest`;
- `policy-check` audits a trace for unresolved permission denies and sensitive
  capabilities (`git push`) that ran without a recorded allow;
- command traces include argv, exit code, duration, byte counts, and bounded
  stdout/stderr previews;
- open traces can still be summarized, inspected, and rendered;
- `ledger rebuild/list/show` gives agents a stable catalog once many local runs
  exist;
- the raw trace file remains the source of truth when an agent needs to inspect
  or pass the run evidence to another step.

## Quality gate

Every public change should pass the same gate that CI runs:

```bash
just ci
```

That expands to:

```bash
cargo fmt --all --check
cargo clippy --locked --all-targets -- -D warnings
cargo test --locked
cargo llvm-cov --workspace --all-targets --locked --fail-under-lines 80
cargo deny check advisories bans licenses sources
sh scripts/check-local-agent-files.sh
sh scripts/check-release-readiness.sh
sh scripts/host-smoke.sh
sh scripts/hook-smoke.sh
sh scripts/codex-omx-hook-smoke.sh
sh scripts/evidence-gate-smoke.sh
```

CI also runs the tests and smokes on Linux and macOS, performs a locked
release build, runs a scheduled RustSec audit, and publishes an OpenSSF
Scorecard signal. The 80% line-coverage threshold is intentionally modest for
v0.1, but it is a floor, not a target. New command behavior should come with
focused tests before it is treated as part of the tool.

`scripts/host-smoke.sh` is the deeper dogfood path. It creates real success,
failed, manual, and open traces in a temporary workspace, renders HTML, rebuilds
the ledger, filters by status, and verifies the Rust harness example.
`scripts/hook-smoke.sh` separately simulates the host-hook path used by generic
agent workflows.

## Storage model

Slod stores traces as local append-only files. The trace file is the
source of truth. The current implementation uses line-delimited JSON because it
is simple to append, inspect, diff, and recover from. A database may be added
later as a derived local index, but not as the primary record of what the agent
did.

The run ledger is the first derived storage layer. It catalogs local trace files
for discovery, filtering, and handoff, but it is intentionally rebuildable from
`.slod/runs/*.slod`.

See [`docs/storage.md`](docs/storage.md) for the storage decision record and
tradeoffs.

## Agent Skill

This repo ships an agent-facing skill at
[`skills/slod`](skills/slod). Install or copy it into your agent
harness's skill directory when agents should know the correct Slod
operating contract, commands, and release gate.

## Product boundaries

Slod deliberately starts with one contract: capture a local, ordered,
inspectable record of an agent run. Runtime control, permission policy,
dashboards, OpenTelemetry export, eval suites, and prompt management can connect
around that trace contract, but they should not define v0.1.

## Versioning and changelog

Slod is pre-1.0. While the project is in local-MVP/alpha territory,
breaking changes to the event schema, CLI flags, or output contracts may happen
without a major version bump. The short-term goal is not feature breadth; it is
to prove that the local trace contract is useful inside real agent workflows.

## Design principles

- Agents are primary operators; humans are reviewers and operators.
- A trace must help explain a real run, not decorate a dashboard.
- A failed run should become a test, policy, eval, or workflow improvement.
- Local trace evidence comes before SaaS.
- Export surfaces come after the core trace contract is useful.
