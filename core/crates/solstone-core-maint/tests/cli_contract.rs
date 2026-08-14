// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use solstone_core_maint::run_cli;
use tempfile::tempdir;

fn args(values: &[&str]) -> Vec<String> {
    values.iter().map(|value| (*value).to_owned()).collect()
}

#[test]
fn parser_contract_covers_help_aliases_and_usage_anchor() {
    let journal = tempdir().expect("journal");
    let help = run_cli(&args(&["--help"]), journal.path());
    assert_eq!(help.exit_code, 0);
    assert!(
        help.stdout
            .contains("Task to show details for (or to re-run with --force)")
    );
    let list = run_cli(&args(&["-l", "-v", "-d"]), journal.path());
    assert_eq!(list.exit_code, 0);
    assert!(list.stdout.starts_with("Pending (27):"));
    let unknown = run_cli(&args(&["--nonsense"]), journal.path());
    assert_eq!(unknown.exit_code, 2);
    assert!(unknown.stderr.starts_with("usage: journal maint"));
}
