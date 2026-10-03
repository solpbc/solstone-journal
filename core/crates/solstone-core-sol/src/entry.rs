// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! The process entry behind both public command names.
//!
//! `solstone` and `journal` are one program. Invoked as `journal`, it enters
//! the `solstone journal` command family as a whole, so `journal <args>` and
//! `solstone journal <args>` run the same code with the same arguments.

use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

/// The command family an invocation enters.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Invocation {
    Solstone,
    Journal,
}

/// Names that enter the journal family. On Linux and macOS the `journal`
/// launcher runs the installed `solstone-core-journal` binary.
const JOURNAL_NAMES: [&str; 2] = ["journal", "solstone-core-journal"];
const SOLSTONE_NAMES: [&str; 2] = ["solstone", "solstone-core-sol"];

/// Run the program named by this process's invocation.
#[must_use]
pub fn process_main() -> ExitCode {
    let mut args = std::env::args_os();
    let argv0 = args.next();
    let args = args.collect::<Vec<_>>();
    dispatch(
        argv0.as_deref(),
        args,
        || std::env::current_exe().ok(),
        || {
            #[cfg(windows)]
            windows_lifecycle::run_installer_hooks();
        },
        crate::run,
        run_journal,
    )
}

fn dispatch(
    argv0: Option<&OsStr>,
    args: Vec<OsString>,
    current_exe: impl FnOnce() -> Option<PathBuf>,
    mut run_hooks: impl FnMut(),
    mut run_solstone: impl FnMut(&str, Vec<OsString>) -> ExitCode,
    mut run_journal: impl FnMut(Vec<OsString>) -> ExitCode,
) -> ExitCode {
    run_hooks();
    match invocation(argv0, current_exe) {
        Invocation::Journal => run_journal(args),
        Invocation::Solstone => run_solstone("solstone", args),
    }
}

/// The `solstone journal` command family, which the `journal` alias enters.
pub(crate) fn run_journal(args: Vec<OsString>) -> ExitCode {
    install_logger();
    solstone_core_journal_cli::run(args)
}

/// The invoked name decides the family. A launcher can leave argv[0] as a
/// path the program does not recognize, so the executable's own name breaks
/// the tie, and anything else is the `solstone` root.
fn invocation(argv0: Option<&OsStr>, current_exe: impl FnOnce() -> Option<PathBuf>) -> Invocation {
    argv0
        .and_then(classify)
        .or_else(|| {
            current_exe()
                .as_deref()
                .map(Path::as_os_str)
                .and_then(classify)
        })
        .unwrap_or(Invocation::Solstone)
}

fn classify(path: &OsStr) -> Option<Invocation> {
    // Either separator: a Windows argv[0] is a backslash path.
    let name = path
        .to_str()?
        .rsplit(['/', '\\'])
        .next()?
        .to_ascii_lowercase();
    let name = name.strip_suffix(".exe").unwrap_or(&name);
    if JOURNAL_NAMES.contains(&name) {
        Some(Invocation::Journal)
    } else if SOLSTONE_NAMES.contains(&name) {
        Some(Invocation::Solstone)
    } else {
        None
    }
}

fn install_logger() {
    let _ = env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("warn"))
        .try_init();
}

/// Velopack runs the Windows journal's install, update and uninstall hooks
/// through `solstone.exe`, the package's main program; alias entry also runs them.
#[cfg(windows)]
mod windows_lifecycle {
    pub(super) fn run_installer_hooks() {
        if std::env::args_os().all(|arg| arg.to_str().is_some()) {
            // The SDK reads Unicode args during construction. Leave non-Unicode
            // arguments to the journal's existing OsString boundary.
            velopack::VelopackApp::build()
                .set_auto_apply_on_startup(false)
                .on_after_install_fast_callback(|_| {
                    solstone_core_journal_cli::add_commands_to_owner_path();
                    solstone_core_journal_cli::resume_service_after_update();
                })
                .on_after_update_fast_callback(|_| {
                    solstone_core_journal_cli::resume_service_after_update();
                })
                .on_before_uninstall_fast_callback(|_| {
                    solstone_core_journal_cli::remove_commands_from_owner_path();
                    remove_service_before_uninstall();
                })
                .run();
        }
    }

    fn remove_service_before_uninstall() {
        use std::os::windows::process::CommandExt;

        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        let result = std::env::current_exe().and_then(|journal| {
            let core = journal.with_file_name("solstone-core.exe");
            std::process::Command::new(core)
                .args(["service", "__before-uninstall"])
                .creation_flags(CREATE_NO_WINDOW)
                .output()
        });
        match result {
            Ok(output) if output.status.success() => {}
            Ok(output) => {
                eprintln!(
                    "journal service cleanup failed during uninstall: {}",
                    String::from_utf8_lossy(&output.stderr)
                );
                std::process::exit(1);
            }
            Err(error) => {
                eprintln!("journal service cleanup could not start during uninstall: {error}");
                std::process::exit(1);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn named(argv0: &str) -> Invocation {
        invocation(Some(OsStr::new(argv0)), || {
            panic!("a recognized argv[0] decides without the executable path")
        })
    }

    #[test]
    fn the_journal_names_enter_the_journal_family() {
        for argv0 in [
            "journal",
            "/usr/local/bin/journal",
            "/opt/solstone/bin/solstone-core-journal",
            "C:\\Users\\Owner\\AppData\\Local\\Journal\\current\\bin\\journal.exe",
            "JOURNAL.EXE",
        ] {
            assert_eq!(named(argv0), Invocation::Journal, "{argv0}");
        }
        for argv0 in [
            "solstone",
            "/opt/solstone/bin/solstone-core-sol",
            "C:\\journal\\bin\\solstone.exe",
        ] {
            assert_eq!(named(argv0), Invocation::Solstone, "{argv0}");
        }
    }

    #[test]
    fn an_unrecognized_argv0_falls_back_to_the_executable_then_the_solstone_root() {
        assert_eq!(
            invocation(Some(OsStr::new("-zsh")), || Some(
                "/opt/solstone/bin/solstone-core-journal".into()
            )),
            Invocation::Journal
        );
        assert_eq!(
            invocation(None, || Some("/opt/solstone/bin/journal".into())),
            Invocation::Journal
        );
        assert_eq!(
            invocation(Some(OsStr::new("renamed")), || Some("/tmp/renamed".into())),
            Invocation::Solstone
        );
        assert_eq!(invocation(None, || None), Invocation::Solstone);
    }

    #[test]
    fn logger_install_is_idempotent_and_defaults_to_warn() {
        install_logger();
        install_logger();
        if std::env::var("RUST_LOG").is_err() {
            assert!(log::max_level() >= log::LevelFilter::Warn);
        }
    }

    #[test]
    fn dispatch_routes_invocations_and_runs_hooks() {
        let mut hooks_run = 0;
        let mut solstone_run = 0;
        let mut journal_run = 0;
        let mut solstone_args = Vec::new();
        let mut journal_args = Vec::new();

        // 1. argv0 solstone.exe, args ["status"]
        let code = dispatch(
            Some(OsStr::new("solstone.exe")),
            vec![OsString::from("status")],
            || None,
            || hooks_run += 1,
            |_name, args| {
                solstone_run += 1;
                solstone_args = args;
                ExitCode::SUCCESS
            },
            |_args| {
                journal_run += 1;
                ExitCode::SUCCESS
            },
        );
        assert_eq!(code, ExitCode::SUCCESS);
        assert_eq!(hooks_run, 1);
        assert_eq!(solstone_run, 1);
        assert_eq!(solstone_args, vec![OsString::from("status")]);
        assert_eq!(journal_run, 0);

        // 2. argv0 solstone.exe, args ["journal", "status"]
        hooks_run = 0;
        solstone_run = 0;
        journal_run = 0;
        solstone_args.clear();
        let code = dispatch(
            Some(OsStr::new("solstone.exe")),
            vec![OsString::from("journal"), OsString::from("status")],
            || None,
            || hooks_run += 1,
            |_name, args| {
                solstone_run += 1;
                solstone_args = args;
                ExitCode::SUCCESS
            },
            |_args| {
                journal_run += 1;
                ExitCode::SUCCESS
            },
        );
        assert_eq!(code, ExitCode::SUCCESS);
        assert_eq!(hooks_run, 1);
        assert_eq!(solstone_run, 1);
        assert_eq!(
            solstone_args,
            vec![OsString::from("journal"), OsString::from("status")]
        );
        assert_eq!(journal_run, 0);

        // 3. argv0 journal.exe, args ["status"]
        hooks_run = 0;
        solstone_run = 0;
        journal_run = 0;
        let code = dispatch(
            Some(OsStr::new("journal.exe")),
            vec![OsString::from("status")],
            || None,
            || hooks_run += 1,
            |_name, _args| {
                solstone_run += 1;
                ExitCode::SUCCESS
            },
            |args| {
                journal_run += 1;
                journal_args = args;
                ExitCode::SUCCESS
            },
        );
        assert_eq!(code, ExitCode::SUCCESS);
        assert_eq!(hooks_run, 1);
        assert_eq!(journal_run, 1);
        assert_eq!(journal_args, vec![OsString::from("status")]);
        assert_eq!(solstone_run, 0);
    }
}
