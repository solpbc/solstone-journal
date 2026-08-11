// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::path::Path;

use serde_json::Value;
use solstone_core_import::cli_argv::run_cli_with;
use solstone_core_segment::SUPERVISOR_MESSAGE;

const GRAMMAR: &str = include_str!("../../../fixtures/import_reference_grammar.json");

#[test]
fn argv_parses_before_supervisor_preflight_and_preserves_exit_contract() {
    let fixture: Value = serde_json::from_str(GRAMMAR).unwrap();
    let note = fixture["unknown_option_exit_codes"]["note"]
        .as_str()
        .unwrap();
    assert!(note.contains("expected exit 2 is a MATCH"));

    let unknown = run(&["--nonsense"], |_| None, || false);
    assert_eq!(
        unknown.exit_code,
        fixture["unknown_option_exit_codes"]["supervisor_gate_passed"]["exit"]
            .as_i64()
            .unwrap() as i32
    );
    assert!(unknown.stderr.contains("usage: journal importer"));

    let spawned = run(
        &["file"],
        |name| (name == "SOL_SUPERVISOR_SPAWNED").then(|| "1".to_owned()),
        || false,
    );
    assert_eq!(
        spawned.exit_code,
        fixture["unknown_option_exit_codes"]["supervisor_spawned"]["exit"]
            .as_i64()
            .unwrap() as i32
    );
    assert!(spawned.stderr.is_empty());

    let unavailable = run(&["file"], |_| None, || false);
    assert_eq!(
        unavailable.exit_code,
        fixture["unknown_option_exit_codes"]["solstone_down"]["exit"]
            .as_i64()
            .unwrap() as i32
    );
    assert_eq!(unavailable.stderr, format!("{SUPERVISOR_MESSAGE}\n"));
}

#[test]
fn positional_timestamp_reaches_the_unimplemented_handler() {
    let result = run(
        &["file", "20260311_120000"],
        |name| (name == "SOL_SKIP_SUPERVISOR_CHECK").then(|| "1".to_owned()),
        || false,
    );

    assert!(result.stderr.contains("import: unimplemented: cli_argv"));
}

#[test]
fn value_options_accept_attached_and_separated_values() {
    for arguments in [
        &["--timestamp=20260311_120000", "file"][..],
        &["--timestamp", "20260311_120000", "file"][..],
    ] {
        let result = run(
            arguments,
            |name| (name == "SOL_SKIP_SUPERVISOR_CHECK").then(|| "1".to_owned()),
            || false,
        );

        assert!(result.stderr.contains("import: unimplemented: cli_argv"));
        assert!(!result.stderr.contains("media"));
    }
}

#[test]
fn unknown_attached_option_is_rejected() {
    let result = run(&["--nonsense=x", "file"], |_| None, || false);

    assert_eq!(result.exit_code, 2);
    assert!(result.stderr.contains("usage: journal importer"));
}

fn run<E, C>(
    args: &[&str],
    lookup_env: E,
    connectivity: C,
) -> solstone_core_import::cli_argv::CliRun
where
    E: Fn(&str) -> Option<String>,
    C: FnOnce() -> bool,
{
    run_cli_with(
        &args
            .iter()
            .map(|value| (*value).to_owned())
            .collect::<Vec<_>>(),
        Path::new("."),
        lookup_env,
        connectivity,
    )
}
