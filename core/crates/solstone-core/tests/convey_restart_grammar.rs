// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::ffi::OsString;
use std::process::Command as ProcessCommand;

use serde::Deserialize;
use solstone_core_cli::{
    CONVEY_HELP, CONVEY_USAGE, Command, RESTART_CONVEY_HELP, RESTART_CONVEY_USAGE, evaluate_args,
};

#[derive(Deserialize)]
struct Corpus {
    commands: Commands,
}

#[derive(Deserialize)]
struct Commands {
    convey: Grammar,
    restart_convey: Grammar,
}

#[derive(Deserialize)]
struct Grammar {
    cases: Vec<Case>,
}

#[derive(Deserialize)]
struct Case {
    label: String,
    argv: Vec<String>,
    phase: String,
    #[serde(default)]
    stdout: String,
    #[serde(default)]
    stderr: String,
    #[serde(default)]
    exit: Option<i32>,
    #[serde(default)]
    forwarded_argv: Vec<String>,
    #[serde(default)]
    parsed_timeout: Option<f64>,
    #[serde(default)]
    parsed_verbose: Option<bool>,
    #[serde(default)]
    parsed_debug: Option<bool>,
}

#[derive(Deserialize)]
struct Ledger {
    entries: Vec<LedgerEntry>,
}

#[derive(Deserialize)]
struct LedgerEntry {
    id: String,
}

fn corpus() -> Corpus {
    serde_json::from_str(include_str!(
        "../../../fixtures/convey_restart_reference_grammar.json"
    ))
    .expect("frozen grammar corpus parses")
}

fn ledger() -> Ledger {
    serde_json::from_str(include_str!(
        "../../../fixtures/convey_restart_divergence_ledger.json"
    ))
    .expect("closed divergence ledger parses")
}

fn binary(command: &str, argv: &[String]) -> std::process::Output {
    ProcessCommand::new(env!("CARGO_BIN_EXE_solstone-core"))
        .arg(command)
        .args(argv)
        .output()
        .expect("native command runs")
}

fn frozen_case<'a>(grammar: &'a Grammar, label: &str) -> &'a Case {
    grammar
        .cases
        .iter()
        .find(|case| case.label == label)
        .expect("frozen case exists")
}

fn usage_prefix(stderr: &str) -> &str {
    let end = stderr.find('\n').expect("usage has a newline") + 1;
    &stderr[..end]
}

#[test]
fn corpus_help_and_rejected_grammar_are_owner_faithful() {
    let corpus = corpus();
    for (command, grammar) in [
        ("convey", &corpus.commands.convey),
        ("restart-convey", &corpus.commands.restart_convey),
    ] {
        for case in grammar.cases.iter().filter(|case| case.phase == "parse") {
            if case.label.starts_with("preflight") || case.label.starts_with("template") {
                continue;
            }
            let output = binary(command, &case.argv);
            assert_eq!(output.status.code(), case.exit, "{command} {}", case.label);
            assert_eq!(
                String::from_utf8_lossy(&output.stdout),
                case.stdout,
                "{command} {} stdout",
                case.label
            );
            assert_eq!(
                String::from_utf8_lossy(&output.stderr),
                case.stderr,
                "{command} {} stderr",
                case.label
            );
        }
    }
}

#[test]
fn cli_help_and_usage_consts_are_pinned_to_the_frozen_corpus() {
    let corpus = corpus();
    let convey = &corpus.commands.convey;
    let restart = &corpus.commands.restart_convey;
    assert_eq!(CONVEY_HELP, frozen_case(convey, "help-long").stdout);
    assert_eq!(CONVEY_HELP, frozen_case(convey, "help-short").stdout);
    assert_eq!(
        RESTART_CONVEY_HELP,
        frozen_case(restart, "help-long").stdout
    );
    assert_eq!(
        RESTART_CONVEY_HELP,
        frozen_case(restart, "help-short").stdout
    );
    assert_eq!(
        CONVEY_USAGE,
        usage_prefix(&frozen_case(convey, "unknown-flag").stderr)
    );
    assert_eq!(
        RESTART_CONVEY_USAGE,
        usage_prefix(&frozen_case(restart, "unknown-flag").stderr)
    );
}

#[test]
fn restart_preflight_cases_are_closed_by_af003() {
    let corpus = corpus();
    let restart = &corpus.commands.restart_convey;
    assert!(
        ["preflight-stack-down", "preflight-supervisor-spawned"]
            .into_iter()
            .all(|label| frozen_case(restart, label).phase == "preflight")
    );
    assert!(
        ledger()
            .entries
            .iter()
            .any(|entry| entry.id == "AF-003-restart-preflight-replaced")
    );
}

#[test]
fn corpus_accepted_convey_forms_preserve_the_effective_port() {
    for case in corpus().commands.convey.cases {
        if case.phase != "parse-accept" {
            continue;
        }
        if matches!(case.label.as_str(), "port-negative" | "port-zero") {
            // AF-008 is the closed, owner-facing range-validation divergence.
            let output = binary("convey", &case.argv);
            assert_eq!(
                output.status.code(),
                frozen_case(&corpus().commands.convey, "port-non-integer").exit,
                "{} exit",
                case.label
            );
            assert!(
                String::from_utf8_lossy(&output.stderr).starts_with(CONVEY_USAGE),
                "{} retains owner usage",
                case.label
            );
            continue;
        }
        let expected = case.forwarded_argv[2]
            .parse::<u16>()
            .expect("non-divergent forwarded port parses");
        let arguments = std::iter::once(OsString::from("convey"))
            .chain(case.argv.into_iter().map(OsString::from))
            .collect::<Vec<_>>();
        match evaluate_args(&arguments).expect("native convey parses") {
            Command::Convey(options) => assert_eq!(options.port, expected, "{}", case.label),
            command => panic!("{} parsed to {command:?}", case.label),
        }
    }
}

#[test]
fn restart_timeout_accepts_python_non_finite_float_forms() {
    for (value, expected_nan) in [("inf", false), ("nan", true)] {
        let arguments = [
            OsString::from("restart-convey"),
            OsString::from("--timeout"),
            OsString::from(value),
        ];
        match evaluate_args(&arguments).expect("Python-compatible float parses") {
            Command::RestartConvey(options) if expected_nan => assert!(options.timeout.is_nan()),
            Command::RestartConvey(options) => assert!(options.timeout.is_infinite()),
            command => panic!("{value} parsed to {command:?}"),
        }
    }
}

#[test]
fn corpus_accepted_restart_forms_preserve_parser_values() {
    for case in corpus().commands.restart_convey.cases {
        if case.phase != "parse-accept" {
            continue;
        }
        let arguments = std::iter::once(OsString::from("restart-convey"))
            .chain(case.argv.into_iter().map(OsString::from))
            .collect::<Vec<_>>();
        match evaluate_args(&arguments).expect("native restart parses") {
            Command::RestartConvey(options) => {
                assert_eq!(
                    Some(options.timeout),
                    case.parsed_timeout,
                    "{} timeout",
                    case.label
                );
                assert_eq!(
                    Some(options.verbose),
                    case.parsed_verbose,
                    "{} verbose",
                    case.label
                );
                assert_eq!(
                    Some(options.debug),
                    case.parsed_debug,
                    "{} debug",
                    case.label
                );
            }
            command => panic!("{} parsed to {command:?}", case.label),
        }
    }
}

#[test]
fn ac5_convey_invalid_argument_stays_verb_owned() {
    let corpus = corpus();
    let expected = frozen_case(&corpus.commands.convey, "unknown-flag");
    let output = binary("convey", &expected.argv);
    assert_eq!(output.status.code(), expected.exit);
    assert_eq!(String::from_utf8_lossy(&output.stdout), expected.stdout);
    assert_eq!(String::from_utf8_lossy(&output.stderr), expected.stderr);
}

#[test]
fn ac5_restart_convey_is_a_reachable_native_verb() {
    let corpus = corpus();
    let expected = frozen_case(&corpus.commands.restart_convey, "unknown-flag");
    let output = binary("restart-convey", &expected.argv);
    assert_eq!(output.status.code(), expected.exit);
    assert_eq!(String::from_utf8_lossy(&output.stdout), expected.stdout);
    assert_eq!(String::from_utf8_lossy(&output.stderr), expected.stderr);
}
