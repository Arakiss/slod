use std::{
    fs::{self, File},
    time::{Duration, SystemTime},
};

use predicates::prelude::*;
use tempfile::tempdir;

mod common;
use common::*;

/// Give a file a modification time in the past so a retention window can see
/// it as stale without waiting for one.
fn age_file(path: &std::path::Path, days: u64) {
    let when = SystemTime::now() - Duration::from_secs(days * 86_400);
    let file = File::options().write(true).open(path).unwrap();
    file.set_modified(when).unwrap();
}

fn seed_trace(dir: &std::path::Path, run_id: &str, days_old: u64) -> std::path::PathBuf {
    let trace = dir.join(format!("{run_id}.slod"));
    slod()
        .args(["init", "--file"])
        .arg(&trace)
        .args(["--run-id", run_id])
        .assert()
        .success();
    age_file(&trace, days_old);
    trace
}

#[test]
fn prune_is_a_dry_run_until_apply_is_passed() {
    let dir = tempdir().unwrap();
    let runs = dir.path().join("runs");
    fs::create_dir_all(&runs).unwrap();

    let old = seed_trace(&runs, "run-old", 45);
    let recent = seed_trace(&runs, "run-recent", 2);

    slod()
        .args(["prune", "--older-than", "30", "--dir"])
        .arg(&runs)
        .assert()
        .success()
        .stdout(predicate::str::contains("prune (dry run)"))
        .stdout(predicate::str::contains("traces      1"))
        .stdout(predicate::str::contains("kept        1"))
        .stdout(predicate::str::contains("re-run with --apply"))
        .stdout(predicate::str::contains("would remove"));

    assert!(old.exists(), "a dry run must not delete anything");
    assert!(recent.exists());
}

#[test]
fn prune_apply_removes_stale_traces_their_locks_and_orphan_locks() {
    let dir = tempdir().unwrap();
    let runs = dir.path().join("runs");
    fs::create_dir_all(&runs).unwrap();

    let old = seed_trace(&runs, "run-old", 45);
    let recent = seed_trace(&runs, "run-recent", 2);
    // `init` leaves the sidecar lock beside the trace it wrote.
    let old_lock = runs.join("run-old.slod.lock");
    assert!(old_lock.exists());

    // A lock whose trace is already gone is dead weight at any age.
    let orphan_lock = runs.join("run-vanished.slod.lock");
    fs::write(&orphan_lock, "").unwrap();

    slod()
        .args(["prune", "--older-than", "30", "--apply", "--dir"])
        .arg(&runs)
        .assert()
        .success()
        .stdout(predicate::str::contains("3 of 3 files"));

    assert!(!old.exists());
    assert!(!old_lock.exists());
    assert!(!orphan_lock.exists());
    assert!(recent.exists(), "a trace inside the window must survive");
}

#[test]
fn prune_never_touches_the_ledger_or_foreign_files() {
    let dir = tempdir().unwrap();
    let runs = dir.path().join("runs");
    fs::create_dir_all(&runs).unwrap();

    seed_trace(&runs, "run-old", 45);

    let ledger = runs.join("ledger.slod");
    fs::write(&ledger, "{}\n").unwrap();
    age_file(&ledger, 99);

    let foreign = runs.join("notes.txt");
    fs::write(&foreign, "keep me").unwrap();
    age_file(&foreign, 99);

    let nested = runs.join("nested");
    fs::create_dir_all(&nested).unwrap();
    let nested_trace = nested.join("run-deep.slod");
    fs::write(&nested_trace, "{}\n").unwrap();
    age_file(&nested_trace, 99);

    slod()
        .args(["prune", "--older-than", "30", "--apply", "--dir"])
        .arg(&runs)
        .assert()
        .success();

    assert!(ledger.exists(), "the ledger is a catalog, not a run");
    assert!(foreign.exists());
    assert!(nested_trace.exists(), "prune must not recurse");
}

#[test]
fn prune_reports_a_missing_directory_instead_of_creating_one() {
    let dir = tempdir().unwrap();
    let runs = dir.path().join("absent");

    slod()
        .args(["prune", "--older-than", "30", "--dir"])
        .arg(&runs)
        .assert()
        .failure()
        .stderr(predicate::str::contains("run directory does not exist"));

    assert!(!runs.exists());
}
