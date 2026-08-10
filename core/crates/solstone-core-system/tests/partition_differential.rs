// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Proves `solstone_core_system::partition::partition_for` agrees with the
//! Python original it replaces (`solstone.think.runner._command_partition`).
//!
//! This differential exists because its absence already cost a real defect.
//! The partition resolver is the task queue's identity function: it decides
//! what serializes against what, which cap applies, and what a refusal
//! collides with. The Rust port widened Python's *exact* `cmd[0] in ("sol",
//! "journal")` head match into a *basename* match, so `/opt/tools/journal
//! backup` resolved to a service command in one language and to a plain
//! basename in the other. That shipped, and survived, because every other
//! seam in this conversion has an oracle and this one did not.
//!
//! The rows below are not a sample. They are the branch structure of
//! `_command_partition` read off the source: the `think` flag ladder in its
//! declared order, the `maintenance` sub-partition and its arity guard, both
//! accepted heads, the path-form fallback, and the empty-argv case.

use std::io::Write;
use std::path::PathBuf;
use std::process::{Command, Stdio};

use serde_json::{Value, json};
use solstone_core_system::partition::partition_for;

fn repository_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(3)
        .expect("repository root")
        .to_path_buf()
}

fn python() -> PathBuf {
    let venv = repository_root().join(".venv/bin/python3");
    if venv.is_file() {
        venv
    } else {
        PathBuf::from("python3")
    }
}

/// Every case is named, so a failure says which branch diverged rather than
/// which array index did.
fn decision_table() -> Vec<(&'static str, Vec<String>)> {
    let row = |name: &'static str, argv: &[&str]| {
        (
            name,
            argv.iter().map(|part| (*part).to_owned()).collect::<Vec<_>>(),
        )
    };
    vec![
        // the two accepted heads, exact
        row("journal_head", &["journal", "indexer", "--rescan"]),
        row("sol_head", &["sol", "heartbeat"]),
        // the `think` flag ladder, in its declared first-hit order
        row("think_bare_is_daily", &["journal", "think"]),
        row("think_activity", &["journal", "think", "--activity", "a"]),
        row("think_flush", &["journal", "think", "--flush"]),
        row("think_segments", &["journal", "think", "--segments"]),
        row("think_weekly", &["journal", "think", "--weekly"]),
        row("think_cadence", &["journal", "think", "--cadence"]),
        row("think_segment", &["journal", "think", "--segment", "x"]),
        // first hit wins: a production argv carries BOTH, and --flush precedes
        // --segment in the ladder. A set-membership port routes this elsewhere.
        row(
            "think_flush_and_segment_first_hit_wins",
            &["journal", "think", "--segment", "x", "--flush"],
        ),
        // maintenance sub-partitions only at the right arity and shape
        row(
            "maintenance_run_subpartition",
            &["journal", "maintenance", "run", "backup:run"],
        ),
        row("maintenance_short_argv", &["journal", "maintenance", "run"]),
        row(
            "maintenance_not_run_verb",
            &["journal", "maintenance", "status", "backup:run"],
        ),
        // path-form fallback: basename of argv[0], NOT a service command
        row("path_form_journal_is_basename", &["/opt/tools/journal", "backup"]),
        row("path_form_sol_is_basename", &["/usr/local/bin/sol", "backup"]),
        row("unrelated_binary", &["/usr/bin/rsync", "-av"]),
        // degenerate shapes
        row("head_only_no_subcommand", &["journal"]),
        row("empty_argv", &[]),
    ]
}

/// Runs the real Python `_command_partition` over `rows` and returns its
/// answers in order.
///
/// ⚠ This imports the package rather than loading `runner.py` by path: unlike
/// the STT oracle's `resource.py`, `runner.py` does not execute standalone
/// (its module-level state raises during a bare `exec_module`). The import is
/// the honest way to reach the real function, and reaching the *real* function
/// is the entire point of a differential.
fn python_partitions(rows: &Value) -> Vec<String> {
    let script = concat!(
        "import json, sys\n",
        "sys.path.insert(0, os.environ['SOLSTONE_REPO_ROOT'])\n",
        "from solstone.think.runner import _command_partition\n",
        "rows = json.load(sys.stdin)\n",
        "json.dump([_command_partition(argv) for argv in rows], sys.stdout)\n",
    );
    let script = format!("import os\n{script}");
    let mut child = Command::new(python())
        .args(["-c", &script])
        .env("SOLSTONE_REPO_ROOT", repository_root())
        .current_dir(repository_root())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("start Python oracle");
    child
        .stdin
        .take()
        .expect("Python stdin")
        .write_all(rows.to_string().as_bytes())
        .expect("write decision table to Python stdin");
    let output = child.wait_with_output().expect("Python oracle exit");
    assert!(
        output.status.success(),
        "Python oracle failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).expect("Python oracle JSON output")
}

#[test]
fn rust_and_python_agree_on_every_partition_branch() {
    let table = decision_table();
    let rows: Value = json!(
        table
            .iter()
            .map(|(_name, argv)| json!(argv))
            .collect::<Vec<_>>()
    );
    let python = python_partitions(&rows);
    assert_eq!(
        python.len(),
        table.len(),
        "oracle returned {} answers for {} rows",
        python.len(),
        table.len()
    );

    let mut divergences = Vec::new();
    for ((name, argv), expected) in table.iter().zip(python.iter()) {
        let actual = partition_for(argv);
        if actual.as_str() != expected {
            divergences.push(format!(
                "  {name}: argv={argv:?}\n    python={expected:?}\n    rust  ={:?}",
                actual.as_str()
            ));
        }
    }
    assert!(
        divergences.is_empty(),
        "partition resolver diverges from its Python oracle:\n{}",
        divergences.join("\n")
    );
}
