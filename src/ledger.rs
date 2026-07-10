use std::{
    fs,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};

use crate::{
    private_fs::write_private,
    trace::{EventKind, Trace},
};

pub const LEDGER_VERSION: u16 = 1;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LedgerEntry {
    pub version: u16,
    pub run_id: String,
    pub trace_path: String,
    pub status: String,
    pub event_count: usize,
    pub model_calls: usize,
    pub tool_calls: usize,
    pub tool_results: usize,
    pub permission_decisions: usize,
    pub errors: usize,
    pub started_ms: i128,
    pub finished_ms: Option<i128>,
    pub duration_ms: Option<i128>,
}

impl LedgerEntry {
    pub fn from_trace(path: &Path, trace: &Trace) -> Result<Self> {
        trace.verify_open()?;

        let summary = trace.summary();
        let started_ms = trace
            .events
            .first()
            .map(|event| event.ts_ms)
            .context("trace has no start event")?;
        let finished_ms = trace
            .events
            .iter()
            .find(|event| event.kind == EventKind::RunFinished)
            .map(|event| event.ts_ms);

        Ok(Self {
            version: LEDGER_VERSION,
            run_id: summary.run_id,
            trace_path: path.display().to_string(),
            status: summary.status,
            event_count: summary.event_count,
            model_calls: summary.model_calls,
            tool_calls: summary.tool_calls,
            tool_results: summary.tool_results,
            permission_decisions: summary.permission_decisions,
            errors: summary.errors,
            started_ms,
            finished_ms,
            duration_ms: summary.duration_ms,
        })
    }

    pub fn from_trace_file(path: &Path) -> Result<Self> {
        let trace = Trace::read(path)
            .with_context(|| format!("failed to read source trace {}", path.display()))?;
        Self::from_trace(path, &trace)
            .with_context(|| format!("invalid source trace {}", path.display()))
    }
}

pub fn rebuild(dir: &Path, out: &Path) -> Result<Vec<LedgerEntry>> {
    let mut trace_paths = trace_files(dir)?;
    trace_paths.sort();

    let mut entries = Vec::with_capacity(trace_paths.len());
    for path in trace_paths {
        if same_path(&path, out) {
            continue;
        }
        entries.push(LedgerEntry::from_trace_file(&path)?);
    }

    write(out, &entries)?;
    Ok(entries)
}

pub fn write(path: &Path, entries: &[LedgerEntry]) -> Result<()> {
    let content = render_jsonl(entries)?;
    write_private(path, content.as_bytes())
        .with_context(|| format!("failed to write {}", path.display()))
}

pub fn read(path: &Path) -> Result<Vec<LedgerEntry>> {
    let content = fs::read_to_string(path)
        .with_context(|| format!("failed to read ledger file {}", path.display()))?;
    let mut entries = Vec::new();

    for (line_index, line) in content.lines().enumerate() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let entry = serde_json::from_str::<LedgerEntry>(trimmed)
            .with_context(|| format!("invalid ledger entry at line {}", line_index + 1))?;
        if entry.version != LEDGER_VERSION {
            bail!(
                "unsupported ledger version {} at line {}",
                entry.version,
                line_index + 1
            );
        }
        entries.push(entry);
    }

    Ok(entries)
}

pub fn filter_by_status<'a>(
    entries: &'a [LedgerEntry],
    status: Option<&str>,
) -> Vec<&'a LedgerEntry> {
    entries
        .iter()
        .filter(|entry| status.is_none_or(|status| entry.status == status))
        .collect()
}

pub fn find_by_run_id<'a>(entries: &'a [LedgerEntry], run_id: &str) -> Option<&'a LedgerEntry> {
    entries.iter().find(|entry| entry.run_id == run_id)
}

pub fn render_list(entries: &[&LedgerEntry]) -> String {
    let mut output = format!(
        "{:<28} {:<10} {:>6} {:>6} {:>11} {}\n",
        "run_id", "status", "events", "errors", "duration_ms", "trace_path"
    );

    if entries.is_empty() {
        output.push_str("(no runs)\n");
        return output;
    }

    for entry in entries {
        output.push_str(&format!(
            "{:<28} {:<10} {:>6} {:>6} {:>11} {}\n",
            entry.run_id,
            entry.status,
            entry.event_count,
            entry.errors,
            entry
                .duration_ms
                .map(|duration| duration.to_string())
                .unwrap_or_else(|| "unknown".to_string()),
            entry.trace_path
        ));
    }

    output
}

pub fn render_entry(entry: &LedgerEntry) -> String {
    format!(
        "run_id: {}\nstatus: {}\ntrace_path: {}\nevents: {}\nmodel_calls: {}\ntool_calls: {}\ntool_results: {}\npermission_decisions: {}\nerrors: {}\nstarted_ms: {}\nfinished_ms: {}\nduration_ms: {}\n",
        entry.run_id,
        entry.status,
        entry.trace_path,
        entry.event_count,
        entry.model_calls,
        entry.tool_calls,
        entry.tool_results,
        entry.permission_decisions,
        entry.errors,
        entry.started_ms,
        entry
            .finished_ms
            .map(|timestamp| timestamp.to_string())
            .unwrap_or_else(|| "open".to_string()),
        entry
            .duration_ms
            .map(|duration| duration.to_string())
            .unwrap_or_else(|| "unknown".to_string())
    )
}

pub fn render_jsonl(entries: &[LedgerEntry]) -> Result<String> {
    let mut output = String::new();
    for entry in entries {
        output.push_str(&serde_json::to_string(entry)?);
        output.push('\n');
    }
    Ok(output)
}

fn trace_files(dir: &Path) -> Result<Vec<PathBuf>> {
    let mut files = Vec::new();
    for entry in
        fs::read_dir(dir).with_context(|| format!("failed to read trace dir {}", dir.display()))?
    {
        let entry = entry.with_context(|| format!("failed to read entry in {}", dir.display()))?;
        let path = entry.path();
        if path.is_file() && path.extension().and_then(|ext| ext.to_str()) == Some("slod") {
            files.push(path);
        }
    }
    Ok(files)
}

fn same_path(left: &Path, right: &Path) -> bool {
    if left == right {
        return true;
    }

    match (left.canonicalize(), right.canonicalize()) {
        (Ok(left), Ok(right)) => left == right,
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use std::fs;

    use serde_json::json;
    use tempfile::tempdir;

    use crate::trace::{Event, SLOD_VERSION};

    use super::*;

    fn trace(status: Option<&str>) -> Trace {
        let mut events = vec![
            Event {
                version: SLOD_VERSION,
                run_id: "run-ledger".into(),
                event_id: "e0".into(),
                kind: EventKind::RunStarted,
                ts_ms: 100,
                seq: 0,
                payload: json!({"status":"started"}),
            },
            Event {
                version: SLOD_VERSION,
                run_id: "run-ledger".into(),
                event_id: "e1".into(),
                kind: EventKind::ToolCall,
                ts_ms: 110,
                seq: 1,
                payload: json!({"tool":"shell"}),
            },
            Event {
                version: SLOD_VERSION,
                run_id: "run-ledger".into(),
                event_id: "e2".into(),
                kind: EventKind::PermissionDecision,
                ts_ms: 120,
                seq: 2,
                payload: json!({"decision":"allow"}),
            },
        ];

        if let Some(status) = status {
            events.push(Event {
                version: SLOD_VERSION,
                run_id: "run-ledger".into(),
                event_id: "e3".into(),
                kind: EventKind::RunFinished,
                ts_ms: 140,
                seq: 3,
                payload: json!({"status":status}),
            });
        }

        Trace { events }
    }

    #[test]
    fn entry_summarizes_closed_trace() {
        let entry =
            LedgerEntry::from_trace(Path::new("runs/run.slod"), &trace(Some("success"))).unwrap();

        assert_eq!(entry.run_id, "run-ledger");
        assert_eq!(entry.status, "success");
        assert_eq!(entry.event_count, 4);
        assert_eq!(entry.tool_calls, 1);
        assert_eq!(entry.permission_decisions, 1);
        assert_eq!(entry.started_ms, 100);
        assert_eq!(entry.finished_ms, Some(140));
        assert_eq!(entry.duration_ms, Some(40));
    }

    #[test]
    fn entry_summarizes_open_trace() {
        let entry = LedgerEntry::from_trace(Path::new("runs/open.slod"), &trace(None)).unwrap();

        assert_eq!(entry.status, "open");
        assert_eq!(entry.finished_ms, None);
    }

    #[test]
    fn rebuild_reads_trace_files_in_deterministic_order() {
        let dir = tempdir().unwrap();
        let runs_dir = dir.path().join("runs");
        fs::create_dir(&runs_dir).unwrap();
        let out = dir.path().join("ledger.slod");

        Trace::init(&runs_dir.join("b.slod"), "run-b", false).unwrap();
        Trace::append(
            &runs_dir.join("b.slod"),
            EventKind::RunFinished,
            json!({"status":"failed"}),
        )
        .unwrap();
        Trace::init(&runs_dir.join("a.slod"), "run-a", false).unwrap();
        Trace::append(
            &runs_dir.join("a.slod"),
            EventKind::RunFinished,
            json!({"status":"success"}),
        )
        .unwrap();

        let entries = rebuild(&runs_dir, &out).unwrap();

        assert_eq!(entries[0].run_id, "run-a");
        assert_eq!(entries[1].run_id, "run-b");
        assert_eq!(read(&out).unwrap(), entries);
    }

    #[test]
    fn filter_and_find_entries() {
        let entries = vec![
            LedgerEntry {
                version: LEDGER_VERSION,
                run_id: "run-a".into(),
                trace_path: "a.slod".into(),
                status: "success".into(),
                event_count: 1,
                model_calls: 0,
                tool_calls: 0,
                tool_results: 0,
                permission_decisions: 0,
                errors: 0,
                started_ms: 1,
                finished_ms: Some(2),
                duration_ms: Some(1),
            },
            LedgerEntry {
                version: LEDGER_VERSION,
                run_id: "run-b".into(),
                trace_path: "b.slod".into(),
                status: "failed".into(),
                event_count: 1,
                model_calls: 0,
                tool_calls: 0,
                tool_results: 0,
                permission_decisions: 0,
                errors: 1,
                started_ms: 1,
                finished_ms: Some(2),
                duration_ms: Some(1),
            },
        ];

        let failed = filter_by_status(&entries, Some("failed"));

        assert_eq!(failed.len(), 1);
        assert_eq!(failed[0].run_id, "run-b");
        assert_eq!(find_by_run_id(&entries, "run-a").unwrap().status, "success");
        assert!(find_by_run_id(&entries, "missing").is_none());
    }

    #[test]
    fn read_rejects_malformed_ledger_line() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("ledger.slod");
        fs::write(&path, "{bad}\n").unwrap();

        let error = read(&path).unwrap_err();

        assert!(error.to_string().contains("invalid ledger entry at line 1"));
    }
}
