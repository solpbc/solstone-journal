// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::ffi::{OsStr, OsString};

pub mod help;
pub mod manifest;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum JournalCommand {
    Help,
    Version,
    Known {
        token: &'static str,
        rest: Vec<OsString>,
    },
    DottedModule,
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    Help(String),
    Version(String),
    Unavailable { token: &'static str },
    Rejected,
}

pub trait ProcessSpawner {
    fn spawn(&self, program: &str, args: &[String]) -> std::io::Result<()>;
}

pub struct RealProcessSpawner;

impl ProcessSpawner for RealProcessSpawner {
    fn spawn(&self, program: &str, args: &[String]) -> std::io::Result<()> {
        unimplemented!(
            "solstone-core-journal-cli M1 never spawns a process; this seam is reserved for a future wave: {program} {args:?}"
        )
    }
}

#[must_use]
pub fn evaluate_args(args: &[OsString]) -> JournalCommand {
    let Some(first) = args.first() else {
        return JournalCommand::Help;
    };
    if first == OsStr::new("--help") || first == OsStr::new("-h") {
        return JournalCommand::Help;
    }
    if first == OsStr::new("--version") {
        return JournalCommand::Version;
    }
    let Some(value) = first.to_str() else {
        return JournalCommand::Unknown;
    };
    if value.contains('.') {
        return JournalCommand::DottedModule;
    }
    match manifest::known_token(value) {
        Some(token) => JournalCommand::Known {
            token,
            rest: args[1..].to_vec(),
        },
        None => JournalCommand::Unknown,
    }
}

#[must_use]
pub fn dispatch(command: JournalCommand, _spawner: &dyn ProcessSpawner) -> Outcome {
    match command {
        JournalCommand::Help => Outcome::Help(help::render_help()),
        JournalCommand::Version => Outcome::Version(help::version_line()),
        JournalCommand::Known { token, .. } => Outcome::Unavailable { token },
        JournalCommand::DottedModule | JournalCommand::Unknown => Outcome::Rejected,
    }
}

pub use help::{JOURNAL_USAGE, unavailable_message};

#[cfg(test)]
mod tests {
    use super::*;
    use crate::manifest::{JOURNAL_COMMAND_COUNT, ROOT_COMMANDS};
    use std::collections::BTreeSet;

    const CLI_BOUNDARY_JSON: &str =
        include_str!("../../../fixtures/native-sol/cli-boundary-v1.json");
    const EXPECTED_HELP: &str = "journal - native journal command root (solstone)\n\nUsage: journal <command> [args...]\n\nRoot commands:\n  --path\n  path\n  status\n  root\n  doctor\n  check\n  contract\n  notify\n\nService commands:\n  backfill-processing-records\n  backup\n  brain\n  config\n  convey\n  cortex\n  depict\n  describe\n  down\n  engage\n  export\n  facet-candidates\n  grab\n  health\n  heartbeat\n  identity\n  importer\n  indexer\n  install-models\n  install-provider\n  journal-stats\n  maint\n  maintenance\n  navigate\n  observer\n  reprocess\n  restart-convey\n  schedule\n  segment\n  sense\n  service\n  settings\n  setup\n  spl\n  start\n  streams\n  supervisor\n  talent\n  think\n  top\n  transcribe\n  transfer\n  up\n  warm\n\nOptions:\n  -h, --help    Show this help\n  --version     Show version\n";

    fn args(values: &[&str]) -> Vec<OsString> {
        values.iter().map(OsString::from).collect()
    }

    fn all_tokens() -> impl Iterator<Item = &'static str> {
        ROOT_COMMANDS
            .iter()
            .copied()
            .chain(solstone_core_sol::JOURNAL_HOST_COMMANDS.iter().copied())
    }

    #[test]
    fn evaluate_args_recognizes_every_manifest_token_and_rest() {
        for token in all_tokens() {
            assert_eq!(
                evaluate_args(&args(&[token, "opaque", "--flag"])),
                JournalCommand::Known {
                    token,
                    rest: args(&["opaque", "--flag"]),
                },
                "{token}"
            );
        }
    }

    #[test]
    fn evaluate_args_classifies_special_and_unrecognized_inputs() {
        assert_eq!(evaluate_args(&[]), JournalCommand::Help);
        assert_eq!(evaluate_args(&args(&["--help"])), JournalCommand::Help);
        assert_eq!(evaluate_args(&args(&["-h"])), JournalCommand::Help);
        assert_eq!(
            evaluate_args(&args(&["--version"])),
            JournalCommand::Version
        );
        assert_eq!(
            evaluate_args(&args(&["solstone.think.supervisor"])),
            JournalCommand::DottedModule
        );
        assert_eq!(evaluate_args(&args(&["a.b"])), JournalCommand::DottedModule);
        assert_eq!(evaluate_args(&args(&["bogus"])), JournalCommand::Unknown);
    }

    #[cfg(unix)]
    #[test]
    fn evaluate_args_classifies_non_utf8_first_argument_as_unknown() {
        use std::os::unix::ffi::OsStringExt;

        assert_eq!(
            evaluate_args(&[OsString::from_vec(vec![0xff])]),
            JournalCommand::Unknown
        );
    }

    #[test]
    fn manifest_has_fifty_two_commands() {
        assert_eq!(JOURNAL_COMMAND_COUNT, 52);
        assert_eq!(all_tokens().count(), JOURNAL_COMMAND_COUNT);
    }

    struct PanicSpawner;

    impl ProcessSpawner for PanicSpawner {
        fn spawn(&self, _program: &str, _args: &[String]) -> std::io::Result<()> {
            panic!("M1 dispatch must not spawn")
        }
    }

    #[test]
    fn dispatch_never_invokes_the_spawner() {
        let spawner = PanicSpawner;
        for token in all_tokens() {
            assert_eq!(
                dispatch(
                    JournalCommand::Known {
                        token,
                        rest: args(&["opaque"]),
                    },
                    &spawner,
                ),
                Outcome::Unavailable { token },
                "{token}"
            );
        }
        assert!(matches!(
            dispatch(JournalCommand::Help, &spawner),
            Outcome::Help(_)
        ));
        assert!(matches!(
            dispatch(JournalCommand::Version, &spawner),
            Outcome::Version(_)
        ));
        assert_eq!(
            dispatch(JournalCommand::DottedModule, &spawner),
            Outcome::Rejected
        );
        assert_eq!(
            dispatch(JournalCommand::Unknown, &spawner),
            Outcome::Rejected
        );
    }

    #[test]
    fn render_help_matches_the_manifest_projection() {
        let output = help::render_help();
        assert_eq!(output, EXPECTED_HELP);
        for command in ROOT_COMMANDS
            .iter()
            .chain(solstone_core_sol::JOURNAL_HOST_COMMANDS.iter())
        {
            assert!(output.contains(&format!("  {command}\n")), "{command}");
        }
    }

    #[test]
    fn version_line_matches_the_native_product_format() {
        assert_eq!(
            help::version_line(),
            format!("journal (solstone) {}\n", env!("CARGO_PKG_VERSION"))
        );
    }

    #[test]
    fn unavailable_message_names_the_token_and_category() {
        let message = unavailable_message("think");
        assert!(message.contains("think"));
        assert!(message.contains("journal_command_unavailable"));
    }

    fn fixture_tokens(value: &serde_json::Value, field: &str) -> BTreeSet<String> {
        value["identities"]["journal"][field]
            .as_array()
            .expect("fixture field must be an array")
            .iter()
            .map(|item| {
                item.as_str()
                    .expect("fixture command must be a string")
                    .to_owned()
            })
            .collect()
    }

    fn manifest_matches_fixture(
        value: &serde_json::Value,
        roots: &[&str],
        service: &[&str],
    ) -> bool {
        fixture_tokens(value, "root_commands")
            == roots.iter().map(|item| (*item).to_owned()).collect()
            && fixture_tokens(value, "service_commands")
                == service.iter().map(|item| (*item).to_owned()).collect()
    }

    #[test]
    fn manifest_matches_the_journal_fixture() {
        let value: serde_json::Value =
            serde_json::from_str(CLI_BOUNDARY_JSON).expect("parse CLI boundary fixture");
        assert!(manifest_matches_fixture(
            &value,
            ROOT_COMMANDS,
            solstone_core_sol::JOURNAL_HOST_COMMANDS,
        ));
    }

    #[test]
    fn fixture_comparison_rejects_a_corrupted_manifest() {
        let value: serde_json::Value =
            serde_json::from_str(CLI_BOUNDARY_JSON).expect("parse CLI boundary fixture");
        let mut corrupted_roots = ROOT_COMMANDS.to_vec();
        corrupted_roots.remove(0);
        corrupted_roots.push("invented-command");

        assert!(!manifest_matches_fixture(
            &value,
            &corrupted_roots,
            solstone_core_sol::JOURNAL_HOST_COMMANDS,
        ));
    }
}
