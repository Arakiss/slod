use std::path::PathBuf;

use anyhow::Result;
use clap::{Parser, Subcommand};

mod commands;

#[derive(Debug, Parser)]
#[command(author, version, about)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Create a new trace file with a run.started event.
    Init {
        #[arg(long)]
        file: PathBuf,
        #[arg(long)]
        run_id: String,
        #[arg(long, default_value_t = false)]
        force: bool,
    },
    /// Append one structured event to a trace.
    Record {
        #[arg(long)]
        file: PathBuf,
        #[arg(long)]
        kind: String,
        #[arg(long)]
        payload: String,
    },
    /// Close a trace with a run.finished event.
    Finish {
        #[arg(long)]
        file: PathBuf,
        #[arg(long, default_value = "success")]
        status: String,
        #[arg(long)]
        summary: Option<String>,
    },
    /// Run a command and append tool.call/tool.result events.
    Exec {
        #[arg(long)]
        file: PathBuf,
        #[arg(required = true, trailing_var_arg = true, allow_hyphen_values = true)]
        command: Vec<String>,
    },
    /// Create a trace, run one command, and close the trace automatically.
    Run {
        #[arg(long)]
        file: Option<PathBuf>,
        #[arg(long)]
        run_id: Option<String>,
        #[arg(long, default_value_t = false)]
        force: bool,
        #[arg(required = true, trailing_var_arg = true, allow_hyphen_values = true)]
        command: Vec<String>,
    },
    /// Validate schema and ordering invariants.
    Verify {
        #[arg(long)]
        file: PathBuf,
        #[arg(long, default_value_t = false)]
        allow_open: bool,
    },
    /// Print ordered events for terminal inspection.
    Inspect {
        #[arg(long)]
        file: PathBuf,
    },
    /// Print a compact run summary.
    Summary {
        #[arg(long)]
        file: PathBuf,
    },
    /// Render a standalone HTML report.
    Render {
        #[arg(long)]
        file: PathBuf,
        #[arg(long)]
        html: PathBuf,
    },
    /// Import a harness-native session transcript as a closed trace.
    ///
    /// Backfills traces from transcripts a harness already wrote on disk,
    /// preserving the transcript's own timestamps. With `--dir` (default
    /// `.slod/runs`), the target is `<dir>/<run_id>.slod` and the
    /// run id is derived from the transcript session when `--run-id` is
    /// omitted.
    Import {
        /// Transcript format. Supported: claude-code, codex.
        #[arg(long)]
        format: String,
        /// Transcript file (newline-delimited JSON).
        #[arg(long)]
        input: PathBuf,
        /// Explicit target trace file (mutually exclusive with --dir).
        #[arg(long)]
        file: Option<PathBuf>,
        /// Target run directory (default .slod/runs).
        #[arg(long)]
        dir: Option<PathBuf>,
        /// Free-form source label recorded on imported events (default: the format).
        #[arg(long)]
        source: Option<String>,
        #[arg(long)]
        run_id: Option<String>,
        /// Overwrite an existing target trace.
        #[arg(long, default_value_t = false)]
        force: bool,
    },
    /// Ingest host hook payloads from agent harnesses.
    Hook {
        #[command(subcommand)]
        command: HookCommand,
    },
    /// Audit a trace against capability/permission policy rules.
    ///
    /// Reports two classes of violation:
    ///   1. a permission.decision deny/denied/block with no later allow that
    ///      resolves the same capability/command;
    ///   2. a sensitive public capability (git push / git.push) executed via a
    ///      tool.call with no prior or simultaneous permission.decision allow.
    PolicyCheck {
        #[arg(long)]
        file: PathBuf,
        #[arg(long, default_value_t = false)]
        allow_open: bool,
    },
    /// Rebuild and inspect the local run ledger.
    Ledger {
        #[command(subcommand)]
        command: LedgerCommand,
    },
    /// Drop traces older than a retention window (dry run unless --apply).
    ///
    /// Reports what it would remove and exits without touching anything. Pass
    /// `--apply` to actually delete. Only regular `*.slod` files and their
    /// `*.slod.lock` sidecars directly inside `--dir` are ever considered.
    Prune {
        #[arg(long, default_value = ".slod/runs")]
        dir: PathBuf,
        /// Retention window in days; traces last modified before it are dropped.
        #[arg(long, value_name = "DAYS")]
        older_than: u64,
        /// Perform the deletion instead of reporting it.
        #[arg(long, default_value_t = false)]
        apply: bool,
    },
}

#[derive(Debug, Subcommand)]
enum HookCommand {
    /// Read one hook JSON payload from stdin and append mapped events.
    ///
    /// Exactly one of `--file` or `--dir` must be given. With `--dir`, the
    /// trace file is `<dir>/<run_id>.slod`, the run id is derived from
    /// the payload's session when `--run-id` is omitted, and the per-session
    /// trace is created on first use without `--init-if-missing`.
    Ingest {
        /// Trace file to append to (mutually exclusive with --dir).
        #[arg(long)]
        file: Option<PathBuf>,
        /// Per-session run directory; the trace is <dir>/<run_id>.slod.
        #[arg(long)]
        dir: Option<PathBuf>,
        #[arg(long, default_value = "generic")]
        source: String,
        #[arg(long)]
        run_id: Option<String>,
        #[arg(long, default_value_t = false)]
        init_if_missing: bool,
    },
    /// Close the trace of a finished host session with a `run.finished` event.
    ///
    /// Reads the session-ending hook payload from stdin and derives the run id
    /// the same way `ingest` does. Idempotent: an already closed trace is left
    /// alone, and a session that never produced a trace is not an error.
    ///
    /// Wire this on the host's *session*-ending hook (`SessionEnd`), never on a
    /// per-turn one (`Stop`): closing a trace forbids further appends, so a
    /// per-turn hook would end capture at the first turn.
    Close {
        /// Trace file to close (mutually exclusive with --dir).
        #[arg(long)]
        file: Option<PathBuf>,
        /// Per-session run directory; the trace is <dir>/<run_id>.slod.
        #[arg(long)]
        dir: Option<PathBuf>,
        #[arg(long, default_value = "generic")]
        source: String,
        #[arg(long)]
        run_id: Option<String>,
        /// Status recorded on the run.finished event.
        #[arg(long, default_value = "closed")]
        status: String,
    },
    /// Wire an agent host so it pipes hook payloads into `slod hook ingest`.
    ///
    /// Idempotent. Merges slod `PreToolUse`/`PostToolUse` entries into the
    /// local hooks file given by `--file` (default `.agent/hooks.json`), never a
    /// global one. Use `--print` to emit the snippet for manual paste instead of
    /// writing it, e.g. when the host's settings file is global or delicate.
    Install {
        /// Local hooks file to merge the wiring into (relative to cwd by default).
        #[arg(long, default_value = ".agent/hooks.json")]
        file: PathBuf,
        /// Free-form source label recorded on ingested events.
        #[arg(long, default_value = "generic")]
        source: String,
        /// Print the planned wiring without writing anything.
        #[arg(long, default_value_t = false)]
        print: bool,
        /// Optional run id to pin in the ingest commands.
        #[arg(long)]
        run_id: Option<String>,
    },
}

#[derive(Debug, Subcommand)]
enum LedgerCommand {
    /// Rebuild a ledger from trace files.
    Rebuild {
        #[arg(long, default_value = ".slod/runs")]
        dir: PathBuf,
        #[arg(long, default_value = ".slod/ledger.slod")]
        out: PathBuf,
    },
    /// List runs from a ledger.
    List {
        #[arg(long, default_value = ".slod/ledger.slod")]
        file: PathBuf,
        #[arg(long)]
        status: Option<String>,
    },
    /// Show one run from a ledger.
    Show {
        #[arg(long, default_value = ".slod/ledger.slod")]
        file: PathBuf,
        #[arg(long)]
        run_id: String,
    },
    /// Export ledger entries for layer-4 consumers.
    Export {
        #[arg(long, default_value = ".slod/ledger.slod")]
        file: PathBuf,
        /// Emit stable newline-delimited JSON.
        #[arg(long, default_value_t = false)]
        jsonl: bool,
    },
}

fn main() -> Result<()> {
    let cli = Cli::parse();

    match cli.command {
        Command::Init {
            file,
            run_id,
            force,
        } => commands::lifecycle::init(&file, &run_id, force)?,
        Command::Record {
            file,
            kind,
            payload,
        } => commands::lifecycle::record(&file, &kind, &payload)?,
        Command::Finish {
            file,
            status,
            summary,
        } => commands::lifecycle::finish(&file, &status, summary.as_deref())?,
        Command::Exec { file, command } => commands::lifecycle::exec(&file, &command)?,
        Command::Run {
            file,
            run_id,
            force,
            command,
        } => commands::lifecycle::run(file, run_id, force, &command)?,
        Command::Verify { file, allow_open } => commands::verify::verify(&file, allow_open)?,
        Command::Inspect { file } => commands::report::inspect(&file)?,
        Command::Summary { file } => commands::report::summary(&file)?,
        Command::Render { file, html } => commands::report::render(&file, &html)?,
        Command::Import {
            format,
            input,
            file,
            dir,
            source,
            run_id,
            force,
        } => commands::import::import(
            &format,
            &input,
            file.as_deref(),
            dir.as_deref(),
            source.as_deref(),
            run_id.as_deref(),
            force,
        )?,
        Command::Hook { command } => match command {
            HookCommand::Ingest {
                file,
                dir,
                source,
                run_id,
                init_if_missing,
            } => commands::hook::ingest(
                file.as_deref(),
                dir.as_deref(),
                &source,
                run_id.as_deref(),
                init_if_missing,
            )?,
            HookCommand::Close {
                file,
                dir,
                source,
                run_id,
                status,
            } => commands::hook::close(
                file.as_deref(),
                dir.as_deref(),
                &source,
                run_id.as_deref(),
                &status,
            )?,
            HookCommand::Install {
                file,
                source,
                print,
                run_id,
            } => commands::hook::install(&file, &source, print, run_id.as_deref())?,
        },
        Command::PolicyCheck { file, allow_open } => {
            commands::policy::policy_check(&file, allow_open)?
        }
        Command::Ledger { command } => match command {
            LedgerCommand::Rebuild { dir, out } => commands::ledger::rebuild(&dir, &out)?,
            LedgerCommand::List { file, status } => {
                commands::ledger::list(&file, status.as_deref())?
            }
            LedgerCommand::Show { file, run_id } => commands::ledger::show(&file, &run_id)?,
            LedgerCommand::Export { file, jsonl } => commands::ledger::export(&file, jsonl)?,
        },
        Command::Prune {
            dir,
            older_than,
            apply,
        } => commands::prune::prune(&dir, older_than, apply)?,
    }

    Ok(())
}
