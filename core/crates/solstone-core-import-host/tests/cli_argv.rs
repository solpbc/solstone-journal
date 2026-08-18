// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::path::Path;

use solstone_core_import::cli_render::CliRun;
use solstone_core_import_host::cli_argv::{CliOutcome, run_cli_with};
use solstone_core_segment::SUPERVISOR_MESSAGE;

#[test]
fn argv_parses_before_supervisor_preflight_and_preserves_exit_contract() {
    let unknown = run(&["--nonsense"], |_| None, || false);
    assert_eq!(unknown.exit_code, 2);
    assert!(
        unknown
            .stderr
            .contains("unrecognized arguments: --nonsense")
    );
    assert!(unknown.stderr.contains("usage: journal importer"));

    let spawned = run(
        &["file"],
        |name| (name == "SOL_SUPERVISOR_SPAWNED").then(|| "1".to_owned()),
        || false,
    );
    assert_eq!(spawned.exit_code, 75);
    assert!(spawned.stderr.is_empty());

    let unavailable = run(&["file"], |_| None, || false);
    assert_eq!(unavailable.exit_code, 1);
    assert_eq!(unavailable.stderr, format!("{SUPERVISOR_MESSAGE}\n"));
}

#[test]
fn positional_timestamp_reaches_the_generic_dispatch() {
    let result = run(
        &["file", "20260311_120000"],
        |name| (name == "SOL_SKIP_SUPERVISOR_CHECK").then(|| "1".to_owned()),
        || false,
    );

    assert!(!result.stderr.contains("usage: journal importer"));
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

        assert!(!result.stderr.contains("usage: journal importer"));
        assert!(!result.stderr.contains("media"));
    }
}

#[test]
fn auto_does_not_swallow_a_path_positional() {
    let result = run(
        &["--auto", "/tmp/solstone-cycle2-does-not-exist.md"],
        |name| (name == "SOL_SKIP_SUPERVISOR_CHECK").then(|| "1".to_owned()),
        || false,
    );
    assert_eq!(result.exit_code, 1);
    assert!(
        result
            .stderr
            .contains("import source is missing: /tmp/solstone-cycle2-does-not-exist.md"),
        "stderr={}",
        result.stderr
    );
    assert!(
        !result
            .stderr
            .contains("the following arguments are required: media")
    );
}

#[test]
fn unknown_attached_option_is_rejected() {
    let result = run(&["--nonsense=x", "file"], |_| None, || false);

    assert_eq!(result.exit_code, 2);
    assert!(result.stderr.contains("usage: journal importer"));
}

fn run<E, C>(args: &[&str], lookup_env: E, connectivity: C) -> CliRun
where
    E: Fn(&str) -> Option<String>,
    C: FnOnce() -> bool,
{
    match run_cli_with(
        &args
            .iter()
            .map(|value| (*value).to_owned())
            .collect::<Vec<_>>(),
        Path::new("."),
        lookup_env,
        connectivity,
    ) {
        CliOutcome::Rendered(run) => run,
        CliOutcome::Registry(_) => panic!("test invocation must not reach a registry body"),
    }
}
