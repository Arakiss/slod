//! Command handlers for the slod CLI.
//!
//! `main.rs` owns the clap definition and a thin dispatch; each submodule owns
//! the behavior for one area of the CLI. Output formatting and a couple of
//! cross-cutting helpers live here so every handler renders actions the same
//! way.

use anyhow::{Context, Result};
use serde_json::Value;

pub mod hook;
pub mod import;
pub mod ledger;
pub mod lifecycle;
pub mod policy;
pub mod report;
pub mod verify;

/// Maximum number of characters captured from a child command's stdout/stderr
/// when recording a `tool.result` preview.
pub(crate) const OUTPUT_PREVIEW_CHARS: usize = 4_000;

pub(crate) fn parse_payload(payload: &str) -> Result<Value> {
    serde_json::from_str(payload).context("invalid JSON payload")
}

pub(crate) fn print_action(action: &str, fields: &[(&str, String)]) {
    println!("{}", format_action(action, fields));
}

pub(crate) fn eprint_action(action: &str, fields: &[(&str, String)]) {
    eprintln!("{}", format_action(action, fields));
}

fn format_action(action: &str, fields: &[(&str, String)]) -> String {
    let mut output = format!("slod {action}\n");
    for (label, value) in fields {
        output.push_str(&format!("  {label:<11} {value}\n"));
    }
    output
}
