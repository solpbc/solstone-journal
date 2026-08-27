// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::ffi::OsString;
use std::process::Command as ProcessCommand;

use serde::Deserialize;
use solstone_core_cli::{CONVEY_HELP, CONVEY_USAGE, Command, evaluate_args};

#[derive(Deserialize)]
struct Corpus {
    commands: Commands,
}

#[derive(Deserialize)]
struct Commands {
    convey: Grammar,
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
}

fn corpus() -> Corpus {
    serde_json::from_str(include_str!(
        "../../../fixtures/convey_restart_reference_grammar.json"
    ))
    .expect("frozen grammar corpus parses")
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
    for case in corpus
        .commands
        .convey
        .cases
        .iter()
        .filter(|case| case.phase == "parse")
    {
        let output = binary("convey", &case.argv);
        assert_eq!(output.status.code(), case.exit, "convey {}", case.label);
        assert_eq!(
            String::from_utf8_lossy(&output.stdout),
            case.stdout,
            "convey {} stdout",
            case.label
        );
        assert_eq!(
            String::from_utf8_lossy(&output.stderr),
            case.stderr,
            "convey {} stderr",
            case.label
        );
    }
}

#[test]
fn cli_help_and_usage_consts_are_pinned_to_the_frozen_corpus() {
    let corpus = corpus();
    let convey = &corpus.commands.convey;
    assert_eq!(CONVEY_HELP, frozen_case(convey, "help-long").stdout);
    assert_eq!(CONVEY_HELP, frozen_case(convey, "help-short").stdout);
    assert_eq!(
        CONVEY_USAGE,
        usage_prefix(&frozen_case(convey, "unknown-flag").stderr)
    );
}

#[test]
fn corpus_accepted_convey_forms_preserve_the_effective_port() {
    for case in corpus().commands.convey.cases {
        if case.phase != "parse-accept" {
            continue;
        }
        if matches!(case.label.as_str(), "port-negative" | "port-zero") {
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
fn ac5_convey_invalid_argument_stays_verb_owned() {
    let corpus = corpus();
    let expected = frozen_case(&corpus.commands.convey, "unknown-flag");
    let output = binary("convey", &expected.argv);
    assert_eq!(output.status.code(), expected.exit);
    assert_eq!(String::from_utf8_lossy(&output.stdout), expected.stdout);
    assert_eq!(String::from_utf8_lossy(&output.stderr), expected.stderr);
}
