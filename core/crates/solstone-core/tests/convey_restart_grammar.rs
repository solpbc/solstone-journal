// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::ffi::OsString;
use std::process::Command as ProcessCommand;

use serde::Deserialize;
use solstone_core_cli::{Command, evaluate_args};

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
fn corpus_accepted_convey_forms_preserve_the_effective_port() {
    for case in corpus().commands.convey.cases {
        if case.phase != "parse-accept" {
            continue;
        }
        let Ok(expected) = case.forwarded_argv[2].parse::<u16>() else {
            // AF-008 is the closed, owner-facing range-validation divergence.
            continue;
        };
        if expected == 0 {
            // AF-008 is the closed, owner-facing range-validation divergence.
            continue;
        }
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
    let output = binary("convey", &["--nonsense".to_owned()]);
    assert_eq!(output.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&output.stderr).starts_with("usage: journal convey"));
}

#[test]
fn ac5_restart_convey_is_a_reachable_native_verb() {
    let output = binary("restart-convey", &["--nonsense".to_owned()]);
    assert_eq!(output.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&output.stderr).starts_with("usage: journal restart-convey"));
}
