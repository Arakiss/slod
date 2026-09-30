//! Retention: drop trace files older than a retention window.
//!
//! A capture store that only ever grows stops being local-first the day it
//! fills the disk. `prune` is the one operation that removes evidence, so it
//! is a dry run unless `--apply` is passed, it only ever touches regular files
//! named `*.slod` (and their `*.slod.lock` sidecars) directly inside the given
//! directory, and it never recurses.

use std::{
    fs,
    path::{Path, PathBuf},
    time::{Duration, SystemTime},
};

use anyhow::{Context, Result, bail};

use super::print_action;

/// Seconds in a day; the retention window is expressed in whole days because
/// that is the unit a retention policy is written in.
const SECONDS_PER_DAY: u64 = 86_400;

/// The ledger is a derived catalog, not a run, and lives under the same
/// extension. Never prune it even if it is stored beside the runs.
const LEDGER_FILE_NAME: &str = "ledger.slod";

#[derive(Debug, Default)]
struct Candidates {
    traces: Vec<PathBuf>,
    locks: Vec<PathBuf>,
    orphan_locks: Vec<PathBuf>,
    bytes: u64,
    kept: usize,
}

pub(crate) fn prune(dir: &Path, older_than_days: u64, apply: bool) -> Result<()> {
    if !dir.is_dir() {
        bail!("run directory does not exist: {}", dir.display());
    }
    let Some(cutoff) =
        SystemTime::now().checked_sub(Duration::from_secs(older_than_days * SECONDS_PER_DAY))
    else {
        bail!("retention window is too large: {older_than_days} days");
    };

    let candidates = collect(dir, cutoff)?;
    let removable =
        candidates.traces.len() + candidates.locks.len() + candidates.orphan_locks.len();

    if !apply {
        print_action(
            "prune (dry run)",
            &[
                ("dir", dir.display().to_string()),
                ("older_than", format!("{older_than_days}d")),
                ("traces", candidates.traces.len().to_string()),
                ("locks", candidates.locks.len().to_string()),
                ("orphan", candidates.orphan_locks.len().to_string()),
                ("frees", human_bytes(candidates.bytes)),
                ("kept", candidates.kept.to_string()),
                (
                    "note",
                    "nothing was removed; re-run with --apply to delete".to_string(),
                ),
            ],
        );
        for path in candidates.traces.iter().take(5) {
            println!("  would remove {}", path.display());
        }
        if candidates.traces.len() > 5 {
            println!("  … and {} more", candidates.traces.len() - 5);
        }
        return Ok(());
    }

    let mut removed = 0_usize;
    for path in candidates
        .traces
        .iter()
        .chain(&candidates.locks)
        .chain(&candidates.orphan_locks)
    {
        fs::remove_file(path).with_context(|| format!("failed to remove {}", path.display()))?;
        removed += 1;
    }

    print_action(
        "prune",
        &[
            ("dir", dir.display().to_string()),
            ("older_than", format!("{older_than_days}d")),
            ("removed", format!("{removed} of {removable} files")),
            ("freed", human_bytes(candidates.bytes)),
            ("kept", candidates.kept.to_string()),
        ],
    );
    Ok(())
}

/// Split the directory into what the retention window drops and what it keeps.
///
/// Age is taken from the file's modification time, which for an append-only
/// trace is the time of its last recorded event — the only timestamp that
/// answers "when did anything last happen in this run".
fn collect(dir: &Path, cutoff: SystemTime) -> Result<Candidates> {
    let mut candidates = Candidates::default();

    let entries = fs::read_dir(dir).with_context(|| format!("failed to read {}", dir.display()))?;
    for entry in entries {
        let entry =
            entry.with_context(|| format!("failed to read an entry in {}", dir.display()))?;
        let path = entry.path();
        let metadata = entry
            .metadata()
            .with_context(|| format!("failed to stat {}", path.display()))?;
        if !metadata.is_file() {
            continue;
        }
        let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
            continue;
        };
        if name == LEDGER_FILE_NAME {
            continue;
        }

        let stale = metadata
            .modified()
            .map(|modified| modified < cutoff)
            .unwrap_or(false);

        if name.ends_with(".slod.lock") {
            // A lock whose trace is gone is dead weight whatever its age says;
            // one whose trace survives is removed only with that trace, below.
            if !lock_trace_path(&path).exists() {
                candidates.orphan_locks.push(path);
            }
            continue;
        }
        if !name.ends_with(".slod") {
            continue;
        }

        if stale {
            candidates.bytes += metadata.len();
            let lock = trace_lock_path(&path);
            if lock.is_file() {
                candidates.locks.push(lock);
            }
            candidates.traces.push(path);
        } else {
            candidates.kept += 1;
        }
    }

    candidates.traces.sort();
    candidates.locks.sort();
    candidates.orphan_locks.sort();
    Ok(candidates)
}

fn trace_lock_path(path: &Path) -> PathBuf {
    let mut lock = path.as_os_str().to_os_string();
    lock.push(".lock");
    PathBuf::from(lock)
}

fn lock_trace_path(path: &Path) -> PathBuf {
    PathBuf::from(path.to_string_lossy().trim_end_matches(".lock").to_string())
}

fn human_bytes(bytes: u64) -> String {
    const UNITS: [&str; 4] = ["B", "KB", "MB", "GB"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit + 1 < UNITS.len() {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes} B")
    } else {
        format!("{value:.1} {}", UNITS[unit])
    }
}
