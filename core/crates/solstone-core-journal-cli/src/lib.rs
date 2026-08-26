// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::ffi::{OsStr, OsString};
use std::process::ExitCode;

use solstone_core_cli_boundary::JOURNAL_EXPORT_TOMBSTONE;

pub mod help;
mod host;
mod layout;
mod local_ops;
pub mod manifest;
mod notify;
mod notify_handler;
mod processes;
mod runner;

#[cfg(test)]
mod test_support;

pub use runner::{NativeExecutableError, sibling_native_in_dir};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum JournalCommand {
    Help,
    Version,
    Known {
        token: &'static str,
        rest: Vec<OsString>,
        verbose: bool,
    },
    Local {
        token: &'static str,
        rest: Vec<OsString>,
        verbose: bool,
    },
    RetiredExport,
    DottedModule,
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    Help(String),
    Version(String),
    Rejected,
    LocalSuccess {
        stdout: String,
        stderr: String,
    },
    LocalFailure {
        stdout: String,
        stderr: String,
        exit: u8,
    },
    ProcessLaunched,
    ProcessFailure {
        stderr: String,
        exit: u8,
    },
}

pub trait ProcessSpawner {
    fn spawn(&self, program: &OsStr, args: &[OsString]) -> std::io::Result<()>;
}

pub struct RealProcessSpawner;

impl ProcessSpawner for RealProcessSpawner {
    fn spawn(&self, program: &OsStr, args: &[OsString]) -> std::io::Result<()> {
        runner::exec_process(program, args)
    }
}

/// Run the same-device journal command surface as its own process identity.
#[must_use]
pub fn run(args: Vec<OsString>) -> ExitCode {
    match dispatch(evaluate_args(&args), &RealProcessSpawner) {
        Outcome::Help(text) | Outcome::Version(text) => {
            print!("{text}");
            ExitCode::SUCCESS
        }
        Outcome::LocalSuccess { stdout, stderr } => {
            print!("{stdout}");
            eprint!("{stderr}");
            ExitCode::SUCCESS
        }
        Outcome::LocalFailure {
            stdout,
            stderr,
            exit,
        } => {
            print!("{stdout}");
            eprint!("{stderr}");
            ExitCode::from(exit)
        }
        // On Unix a real process launch replaces this process and never returns.
        Outcome::ProcessLaunched => ExitCode::SUCCESS,
        Outcome::ProcessFailure { stderr, exit } => {
            eprint!("{stderr}");
            ExitCode::from(exit)
        }
        Outcome::Rejected => {
            eprint!("{JOURNAL_USAGE}");
            ExitCode::from(64)
        }
    }
}

#[must_use]
pub fn evaluate_args(args: &[OsString]) -> JournalCommand {
    let mut offset = 0;
    let mut verbose = false;
    if args
        .first()
        .is_some_and(|first| first == OsStr::new("-v") || first == OsStr::new("--verbose"))
    {
        verbose = true;
        offset = 1;
    }
    let Some(first) = args.get(offset) else {
        return JournalCommand::Help;
    };
    let rest = &args[offset + 1..];
    if first == OsStr::new("--help") || first == OsStr::new("-h") || first == OsStr::new("help") {
        return if rest.is_empty() {
            JournalCommand::Help
        } else {
            JournalCommand::Unknown
        };
    }
    if first == OsStr::new("--version") || first == OsStr::new("-V") {
        return if rest.is_empty() {
            JournalCommand::Version
        } else {
            JournalCommand::Unknown
        };
    }
    if first == OsStr::new("export") {
        return JournalCommand::RetiredExport;
    }
    let Some(value) = first.to_str() else {
        return JournalCommand::Unknown;
    };
    if let Some(token) = manifest::known_token(value) {
        if matches!(
            manifest::primitive_for(token),
            Some(
                manifest::Primitive::Path | manifest::Primitive::Status | manifest::Primitive::Root
            )
        ) && !rest.is_empty()
            && !(rest.len() == 1 && (rest[0] == "--help" || rest[0] == "-h"))
        {
            return JournalCommand::Unknown;
        }
        return JournalCommand::Known {
            token,
            rest: rest.to_vec(),
            verbose,
        };
    }
    if matches!(value, "archive" | "facet" | "news") {
        let Some(leaf) = rest.first().and_then(|leaf| leaf.to_str()) else {
            return JournalCommand::Unknown;
        };
        return manifest::local_for(value, leaf).map_or(JournalCommand::Unknown, |token| {
            JournalCommand::Local {
                token,
                rest: rest[1..].to_vec(),
                verbose,
            }
        });
    }
    if value.contains('.') {
        JournalCommand::DottedModule
    } else {
        JournalCommand::Unknown
    }
}

#[must_use]
pub fn dispatch(command: JournalCommand, spawner: &dyn ProcessSpawner) -> Outcome {
    match command {
        JournalCommand::Help => Outcome::Help(help::render_help()),
        JournalCommand::Version => Outcome::Version(help::version_line()),
        JournalCommand::Known {
            token,
            rest,
            verbose,
        } => match manifest::primitive_for(token) {
            Some(manifest::Primitive::Path) => {
                if host::is_help_only(&rest) {
                    Outcome::LocalSuccess {
                        stdout: host::PATH_HELP.to_owned(),
                        stderr: String::new(),
                    }
                } else {
                    host::path()
                }
            }
            Some(manifest::Primitive::Status) => {
                if host::is_help_only(&rest) {
                    Outcome::LocalSuccess {
                        stdout: host::STATUS_HELP.to_owned(),
                        stderr: String::new(),
                    }
                } else {
                    host::status()
                }
            }
            Some(manifest::Primitive::Root) => {
                if host::is_help_only(&rest) {
                    Outcome::LocalSuccess {
                        stdout: host::ROOT_HELP.to_owned(),
                        stderr: String::new(),
                    }
                } else {
                    host::root()
                }
            }
            Some(manifest::Primitive::Notify) => notify::notify(&rest),
            Some(manifest::Primitive::Indexer) => local_ops::dispatch("indexer", &rest),
            None => dispatch_process(token, &rest, verbose, spawner),
        },
        JournalCommand::Local { token, rest, .. } => local_ops::dispatch(token, &rest),
        JournalCommand::RetiredExport => Outcome::LocalFailure {
            stdout: String::new(),
            stderr: format!("{JOURNAL_EXPORT_TOMBSTONE}\n"),
            exit: 64,
        },
        JournalCommand::DottedModule | JournalCommand::Unknown => Outcome::Rejected,
    }
}

pub use help::JOURNAL_USAGE;

fn dispatch_process(
    token: &'static str,
    owner_argv: &[OsString],
    _verbose: bool,
    spawner: &dyn ProcessSpawner,
) -> Outcome {
    let Some(native) = processes::native_process_spec_for(token) else {
        return Outcome::ProcessFailure {
            stderr: format!("native journal process launch failed: no native body for {token}\n"),
            exit: 70,
        };
    };
    let executable = match runner::sibling_native_for_current_executable(native.binary) {
        Ok(executable) => executable,
        Err(error) => {
            let exit = match error {
                runner::NativeExecutableError::Missing { .. }
                | runner::NativeExecutableError::NonExecutable { .. } => 70,
                runner::NativeExecutableError::CurrentExe(_) => 70,
            };
            return Outcome::ProcessFailure {
                stderr: format!("native journal process launch failed: {error}\n"),
                exit,
            };
        }
    };
    dispatch_native_process(native, &executable, owner_argv, spawner)
}

fn dispatch_native_process(
    spec: &processes::NativeProcessSpec,
    executable: &std::path::Path,
    owner_argv: &[OsString],
    spawner: &dyn ProcessSpawner,
) -> Outcome {
    let args = runner::native_process_args(spec, owner_argv);
    match spawner.spawn(executable.as_os_str(), &args) {
        Ok(()) => Outcome::ProcessLaunched,
        Err(error) => Outcome::ProcessFailure {
            stderr: format!("native journal process launch failed: {error}\n"),
            exit: 70,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::manifest::{
        JOURNAL_COMMAND_COUNT, JOURNAL_HOST_COMMAND_COUNT, all_leaf_paths, process_command_tokens,
    };
    use crate::processes::{NATIVE_PROCESS_SPECS, PROCESS_SPECS, native_process_spec_for};
    use std::cell::RefCell;
    use std::collections::BTreeSet;
    use std::path::Path;

    fn args(values: &[&str]) -> Vec<OsString> {
        values.iter().map(OsString::from).collect()
    }

    struct PanicSpawner;

    impl ProcessSpawner for PanicSpawner {
        fn spawn(&self, _program: &OsStr, _args: &[OsString]) -> std::io::Result<()> {
            panic!("non-process dispatch must not spawn")
        }
    }

    #[derive(Default)]
    struct RecordingSpawner {
        calls: RefCell<Vec<(OsString, Vec<OsString>)>>,
    }

    impl ProcessSpawner for RecordingSpawner {
        fn spawn(&self, program: &OsStr, args: &[OsString]) -> std::io::Result<()> {
            self.calls
                .borrow_mut()
                .push((program.to_os_string(), args.to_vec()));
            Ok(())
        }
    }

    #[test]
    fn evaluate_args_recognizes_processes_and_preserves_owner_argv() {
        for token in process_command_tokens() {
            assert_eq!(
                evaluate_args(&args(&[token, "opaque", "--help", "a.b"])),
                JournalCommand::Known {
                    token,
                    rest: args(&["opaque", "--help", "a.b"]),
                    verbose: false,
                },
                "{token}"
            );
        }
    }

    #[test]
    fn evaluate_args_classifies_root_grammar() {
        assert_eq!(evaluate_args(&[]), JournalCommand::Help);
        assert_eq!(evaluate_args(&args(&["-v"])), JournalCommand::Help);
        assert_eq!(evaluate_args(&args(&["--help"])), JournalCommand::Help);
        assert_eq!(evaluate_args(&args(&["help"])), JournalCommand::Help);
        assert_eq!(
            evaluate_args(&args(&["--version"])),
            JournalCommand::Version
        );
        assert_eq!(evaluate_args(&args(&["-V"])), JournalCommand::Version);
        assert_eq!(
            evaluate_args(&args(&["help", "extra"])),
            JournalCommand::Unknown
        );
        assert_eq!(
            evaluate_args(&args(&["--version", "extra"])),
            JournalCommand::Unknown
        );
        assert_eq!(
            evaluate_args(&args(&["--verbose", "think", "-v"])),
            JournalCommand::Known {
                token: "think",
                rest: args(&["-v"]),
                verbose: true,
            }
        );
        assert_eq!(
            evaluate_args(&args(&["archive", "export", "--help"])),
            JournalCommand::Local {
                token: "archive export",
                rest: args(&["--help"]),
                verbose: false,
            }
        );
        assert_eq!(
            evaluate_args(&args(&["export", "--help"])),
            JournalCommand::RetiredExport
        );
        assert_eq!(
            evaluate_args(&args(&["archive", "unknown"])),
            JournalCommand::Unknown
        );
        assert_eq!(
            evaluate_args(&args(&["status", "extra"])),
            JournalCommand::Unknown
        );
        assert_eq!(
            evaluate_args(&args(&["status", "--help"])),
            JournalCommand::Known {
                token: "status",
                rest: args(&["--help"]),
                verbose: false,
            }
        );
        assert_eq!(
            evaluate_args(&args(&["solstone.think.supervisor"])),
            JournalCommand::DottedModule
        );
    }

    #[cfg(unix)]
    #[test]
    fn evaluate_args_preserves_non_utf8_owner_argv() {
        use std::os::unix::ffi::OsStringExt;

        assert_eq!(
            evaluate_args(&[OsString::from("think"), OsString::from_vec(vec![0xff])]),
            JournalCommand::Known {
                token: "think",
                rest: vec![OsString::from_vec(vec![0xff])],
                verbose: false,
            }
        );
        assert_eq!(
            evaluate_args(&[OsString::from_vec(vec![0xff])]),
            JournalCommand::Unknown
        );
    }

    #[cfg(unix)]
    #[test]
    fn notify_rejects_non_utf8_owner_argv_without_spawning() {
        use std::os::unix::ffi::OsStringExt;

        let outcome = dispatch(
            evaluate_args(&[OsString::from("notify"), OsString::from_vec(vec![0xff])]),
            &PanicSpawner,
        );
        assert!(matches!(
            outcome,
            Outcome::LocalFailure {
                stdout,
                stderr,
                exit: 75,
            } if stdout.is_empty() && stderr.contains("not valid UTF-8")
        ));
    }

    #[test]
    fn manifest_has_fifty_four_unique_leaf_paths() {
        let paths = all_leaf_paths();
        let unique = paths
            .iter()
            .map(|path| path.join("\u{0}"))
            .collect::<BTreeSet<_>>();
        assert_eq!(JOURNAL_COMMAND_COUNT, 54);
        assert_eq!(paths.len(), JOURNAL_COMMAND_COUNT);
        assert_eq!(unique.len(), JOURNAL_COMMAND_COUNT);
        assert_eq!(JOURNAL_HOST_COMMAND_COUNT, 40);
    }

    #[test]
    fn native_process_specs_bypass_python_with_exact_argv() {
        let spawner = RecordingSpawner::default();
        let owner_argv = args(&["opaque", "--help", "has space"]);
        for spec in NATIVE_PROCESS_SPECS {
            assert_eq!(
                dispatch_native_process(
                    spec,
                    Path::new(&format!("/native/{}", spec.binary)),
                    &owner_argv,
                    &spawner,
                ),
                Outcome::ProcessLaunched,
                "{}",
                spec.token
            );
        }
        let calls = spawner.calls.borrow();
        assert_eq!(calls.len(), NATIVE_PROCESS_SPECS.len());
        for (spec, (program, argv)) in NATIVE_PROCESS_SPECS.iter().zip(calls.iter()) {
            assert_eq!(program, OsStr::new(&format!("/native/{}", spec.binary)));
            let expected = [
                spec.preset_argv.iter().map(OsString::from).collect(),
                owner_argv.clone(),
            ]
            .concat();
            assert_eq!(*argv, expected, "{}", spec.token);
        }
    }

    #[test]
    fn describe_native_preset_argv_composes_dispatcher_argv() {
        let spec = native_process_spec_for("describe").expect("describe native spec");
        let owner_argv = args(&["screen.webm", "-j", "2", "-d", "-v"]);
        assert_eq!(
            crate::runner::native_process_args(spec, &owner_argv),
            args(&["--describe", "screen.webm", "-j", "2", "-d", "-v"])
        );
    }

    #[test]
    fn native_process_specs_are_unique_explicit_census_cutovers() {
        let tokens = NATIVE_PROCESS_SPECS
            .iter()
            .map(|spec| spec.token)
            .collect::<BTreeSet<_>>();
        assert_eq!(tokens.len(), NATIVE_PROCESS_SPECS.len());
        for spec in NATIVE_PROCESS_SPECS {
            assert!(
                PROCESS_SPECS
                    .iter()
                    .any(|historical| historical.token == spec.token),
                "native token {} must retain its historical census row",
                spec.token
            );
            assert_eq!(native_process_spec_for(spec.token), Some(spec));
        }
    }

    #[test]
    fn native_process_specs_pin_five_storage_ops_rows_to_core_argv() {
        for (token, expected_argv) in [
            ("streams", "streams"),
            ("segment", "segment"),
            ("journal-stats", "journal-stats"),
            ("reprocess", "reprocess"),
            ("backfill-processing-records", "backfill-processing-records"),
        ] {
            let spec = native_process_spec_for(token).expect("storage operation must be native");
            assert_eq!(spec.binary, "solstone-core", "{token}");
            assert_eq!(spec.preset_argv, &[expected_argv], "{token}");
        }
    }

    #[test]
    fn non_process_categories_never_invoke_the_spawner() {
        let spawner = PanicSpawner;
        for token in ["--path", "path", "status", "root"] {
            assert!(matches!(
                dispatch(evaluate_args(&args(&[token])), &spawner),
                Outcome::LocalSuccess { .. } | Outcome::LocalFailure { .. }
            ));
        }
        assert!(matches!(
            dispatch(evaluate_args(&args(&["notify", "opaque"])), &spawner),
            Outcome::LocalSuccess { .. } | Outcome::LocalFailure { .. }
        ));
        for command in [
            JournalCommand::Help,
            JournalCommand::Version,
            JournalCommand::Unknown,
        ] {
            assert!(matches!(
                dispatch(command, &spawner),
                Outcome::Help(_) | Outcome::Version(_) | Outcome::Rejected
            ));
        }
        assert_eq!(
            dispatch(
                JournalCommand::Local {
                    token: "archive export",
                    rest: args(&["--help"]),
                    verbose: false,
                },
                &spawner,
            ),
            Outcome::LocalSuccess {
                stdout: "Usage: journal archive export [--out PATH] [--quiet] [--day YYYYMMDD | --from YYYYMMDD [--to YYYYMMDD] | --to YYYYMMDD]\n".to_owned(),
                stderr: String::new(),
            }
        );
    }

    #[test]
    fn retired_export_never_spawns_and_names_its_replacement() {
        for argv in [["export"].as_slice(), ["export", "--help"].as_slice()] {
            let outcome = dispatch(evaluate_args(&args(argv)), &PanicSpawner);
            assert!(matches!(
                outcome,
                Outcome::LocalFailure {
                    stdout,
                    stderr,
                    exit: 64,
                } if stdout.is_empty() && stderr.contains("journal transfer send --to")
            ));
        }
    }

    #[test]
    fn render_help_projects_the_manifest() {
        let output = help::render_help();
        for path in all_leaf_paths() {
            assert!(
                output.contains(&format!("  {}\n", path.join(" "))),
                "{path:?}"
            );
        }
        assert!(!output.contains("solstone.think."));
    }

    #[test]
    fn version_line_matches_the_native_product_format() {
        assert_eq!(
            help::version_line(),
            format!("journal (solstone) {}\n", env!("CARGO_PKG_VERSION"))
        );
    }
}
