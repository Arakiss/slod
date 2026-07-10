# Storage model

Slod records agent runs. The storage decision matters because the trace is
the evidence artifact: it must be readable by humans, parseable by agents, easy
to attach to issues, and robust when a process crashes halfway through a run.

## Decision

The source of truth is one append-only Slod trace file per run. The
current file encoding is line-delimited JSON because it is simple to append,
inspect, diff, and recover from.

Recommended local layout:

```text
.slod/
  runs/
    run-2026-05-03T09-00-00Z.slod
    run-2026-05-03T09-00-00Z.slod.lock
  ledger.slod
  reports/
    run-2026-05-03T09-00-00Z.html
```

Most commands accept an explicit `--file` path. That keeps v0.1 simple and lets
agents decide where a trace belongs. For one-command dogfooding, `slod run`
can also create a default trace under `.slod/runs/` when `--file` is not
provided.

## Local confidentiality

On Unix, Slod creates trace data directories with mode `0700` and trace,
coordination-lock, imported-trace, and ledger files with mode `0600`. These
modes are requested at creation time, so a permissive process umask cannot
briefly expose new trace data before a later permission change. Slod does not
change the modes of pre-existing paths solely because it opens them.

Host hook payloads pass through deterministic recursive redaction before they
become trace events. Keys such as `secret`, `token`, `password`, `cookie`,
`authorization`, and `api-key` are matched case-insensitively across common
snake-case, kebab-case, and camel-case forms. Sensitive HTTP header values are
also replaced in both header maps and name/value header lists. Other fields and
array order are preserved.

Redaction is structural. Slod does not attempt to parse arbitrary shell command
strings or provider-specific free-form text for embedded credentials. Hosts
should pass credentials in named fields or headers and avoid embedding them in
commands. Parse failures report the JSON error without echoing the submitted
payload.

## Concurrent access and lock sidecars

Each trace has a sibling coordination file named `<trace>.lock`. Slod may leave
this empty sidecar in place between commands; it is not part of the trace and
does not change the JSONL event format. The ledger only scans files whose
extension is exactly `.slod`, so lock sidecars are never indexed as runs.

Slod uses operating-system file locks on the sidecar:

- Writers acquire an exclusive lock before deciding whether a trace must be
  initialized. The same lock covers the complete trace read, open-run check,
  next sequence calculation, and append of one serialized JSON event plus its
  newline. A hook payload that maps to multiple events keeps one exclusive lock
  for the full ingest operation.
- Readers acquire a shared lock for the complete file read used by `verify`,
  `summary`, `inspect`, rendering, policy checks, and ledger rebuilds. Multiple
  readers may proceed together, but they wait for an active writer and cannot
  observe its partial append.
- Locks are released when their file handles are dropped, including after an
  error or process exit. The sidecar itself may persist and can be recreated; a
  copied trace does not need its old lock file.

These locks are advisory. Programs that write `.slod` files directly must use
the same sidecar convention to coordinate with Slod. Read-only tools such as
`cat` do not take the lock and may still observe an append in progress.

## Why line-delimited JSON first

Line-delimited JSON matches the first-principles harness requirement:

- **Append-only.** Each event is one line. The writer does not need to rewrite a
  whole document.
- **Crash tolerant.** If a process dies mid-write, earlier complete lines can
  still be read and verified.
- **Agent-readable.** Agent harnesses, shell scripts, and CI can inspect it
  without a database client.
- **Human-readable.** `cat`, `tail`, `rg`, `jq`, and PR attachments work.
- **Versionable.** Sample traces can live in Git, and changes can be reviewed.
- **Portable.** A trace can be copied into an issue, artifact store, or support
  bundle.
- **Exportable.** OpenTelemetry, SQLite, Parquet, HTML, or dashboards can be
  generated from it later.

This is an implementation choice, not the product identity. Slod's public
contract is the event model and the local evidence artifact. The encoding can
gain exports or derived indexes without turning v0.1 into a platform.

## Why not a database as source of truth in v0.1

A database is attractive when there are many runs, multiple writers, search
queries, dashboards, retention rules, and aggregate analytics. Slod is not
there yet.

Starting with a database would add costs too early:

- harder to inspect with basic tools;
- harder to attach to a GitHub issue or PR;
- schema migrations before the event model is proven;
- binary state that is worse for diffs and review;
- more failure modes around locks, corruption, and client versions;
- risk of designing for dashboards before designing for agent use.

The v0.1 question is not:

```text
Can we query thousands of traces?
```

The v0.1 question is:

```text
Can a real agent run leave behind a trace that explains what happened?
```

## When SQLite should enter

SQLite is the likely first database, but as a derived local index.

Use SQLite when Slod needs:

- fast search across many trace files;
- run catalog metadata;
- query by tool, event kind, status, duration, capability, or error;
- joins between traces, eval outcomes, and policy decisions;
- local dashboard support without a server.

Important rule:

```text
SQLite should be rebuildable from Slod trace files.
```

That keeps the event log as evidence and the database as acceleration.

Possible future layout:

```text
.slod/
  runs/
    *.slod
  ledger.slod
  index/
    slod.sqlite
```

Possible future commands:

```bash
slod index rebuild --dir .slod/runs --ledger .slod/ledger.slod --db .slod/index/slod.sqlite
slod index query --status failed --tool shell
```

## Do we need a ledger?

Yes, and Slod now includes it as a derived local catalog.

A ledger is useful when there are many traces and agents need a fast catalog of
runs without opening every trace file. It should answer:

- which runs exist;
- where each trace file lives;
- which agent/harness created the run;
- when the run started and finished;
- whether it passed, failed, or was cancelled;
- how many events/errors/permission decisions it contains;
- which repository, branch, commit, or task it relates to;
- whether derived reports or indexes exist.

But the ledger should not replace the trace. The trace is the evidence. The
ledger is a catalog.

Recommended rule:

```text
ledger.slod must be rebuildable from runs/*.slod
```

This avoids having two competing truths. If the ledger is corrupt or deleted,
Slod can rebuild it. If a trace is deleted, the evidence is gone.

Current commands:

```bash
slod ledger rebuild --dir .slod/runs --out .slod/ledger.slod
slod ledger list --file .slod/ledger.slod --status failed
slod ledger show --file .slod/ledger.slod --run-id run-agent-demo
slod ledger export --file .slod/ledger.slod --jsonl
```

The ledger comes before SQLite because the current bottleneck is
discoverability for agents. SQLite should come after the ledger if the
bottleneck becomes query speed, joins, dashboards, or aggregate analysis.

## Tradeoffs

### Append-only trace file source of truth

Pros:

- simplest reliable writer;
- excellent for local agents and CI;
- easy to read, diff, copy, and attach;
- works without daemon or service;
- good fit for append-only event streams.

Cons:

- slow search across many runs;
- no indexes;
- local multi-writer coordination relies on advisory sidecar locks;
- validation happens at read/verify time;
- large traces may need compression later.

### SQLite primary store

Pros:

- indexes and queries;
- transactions;
- good local dashboard foundation;
- better for thousands of runs.

Cons:

- less transparent for humans and agents;
- migrations required;
- harder to attach/review directly;
- risks turning Slod into an app before the trace contract is proven.

### SQLite derived index

Pros:

- keeps the raw trace file as evidence;
- adds speed when needed;
- rebuildable;
- supports dashboards without sacrificing portability.

Cons:

- duplicate storage;
- index consistency must be checked;
- extra command surface.

### OpenTelemetry export

Pros:

- plugs into existing observability systems;
- useful for teams with collectors and dashboards;
- good long-term export target.

Cons:

- too heavy as v0.1 source of truth;
- agent harness events do not map perfectly to generic spans;
- can hide local review behind infrastructure.

## Current rule

For now:

```text
Write trace files. Verify trace files. Render from trace files. Derive indexes
and exports from trace files.
```

Use the ledger for local run discovery. Do not add a database until real trace
usage proves that search/indexing is the actual bottleneck.
