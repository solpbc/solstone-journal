// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! The process entry behind the binary.
//!
//! Invoked as `solstone`, `solstone.exe`, or `solstone-core-sol`, it enters the
//! solstone root after running any installer hooks. Invoked as private
//! `solstone-core-journal`, it enters the journal command family directly.
//! The names `journal` and `journal.exe` are not a command family: they fall
//! through the same way as any other unrecognized name.

use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

/// The command family an invocation enters.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Invocation {
    Solstone,
    CoreJournal,
}

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
    match invocation(argv0, current_exe) {
        Invocation::CoreJournal => run_journal(args),
        Invocation::Solstone => {
            run_hooks();
            run_solstone("solstone", args)
        }
    }
}

/// The `solstone journal` command family, entered by private `solstone-core-journal`
/// and by the `solstone journal` subcommand.
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
    if name == "solstone-core-journal" {
        Some(Invocation::CoreJournal)
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
/// through `solstone.exe`, the package's main program. Hooks run only for the
/// solstone invocation.
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
                    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
                    if let Err(error) = solstone_core_journal_cli::remove_commands_from_owner_path()
                    {
                        eprintln!("journal commands were not removed from PATH: {error}");
                        std::process::exit(1);
                    }
                    remove_service_before_uninstall(deadline);
                })
                .run();
        }
    }

    fn remove_service_before_uninstall(deadline: std::time::Instant) {
        use std::os::windows::process::CommandExt;

        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        let unix_millis = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .saturating_add(deadline.saturating_duration_since(std::time::Instant::now()))
            .as_millis()
            .min(u64::MAX as u128) as u64;
        let result = std::env::current_exe().and_then(|journal| {
            let core = journal.with_file_name("solstone-core.exe");
            std::process::Command::new(core)
                .args(["service", "__before-uninstall"])
                .env(
                    "SOLSTONE_BEFORE_UNINSTALL_DEADLINE_UNIX_MS",
                    unix_millis.to_string(),
                )
                .creation_flags(CREATE_NO_WINDOW)
                .output()
        });
        match result {
            Ok(output) if output.status.success() => {}
            Ok(output) => {
                eprintln!(
                    "solstone journal service cleanup failed during uninstall: {}",
                    String::from_utf8_lossy(&output.stderr)
                );
                std::process::exit(1);
            }
            Err(error) => {
                eprintln!(
                    "solstone journal service cleanup could not start during uninstall: {error}"
                );
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
            "solstone-core-journal",
            "/opt/solstone/bin/solstone-core-journal",
            "solstone-core-journal.exe",
            "SOLSTONE-CORE-JOURNAL.EXE",
        ] {
            assert_eq!(named(argv0), Invocation::CoreJournal, "{argv0}");
        }
        for argv0 in [
            "journal",
            "/usr/local/bin/journal",
            "C:\\Users\\Owner\\AppData\\Local\\Journal\\current\\bin\\journal.exe",
            "JOURNAL.EXE",
        ] {
            assert_eq!(
                invocation(Some(OsStr::new(argv0)), || None),
                Invocation::Solstone,
                "{argv0}"
            );
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
            Invocation::CoreJournal
        );
        assert_eq!(
            invocation(None, || Some("/opt/solstone/bin/journal".into())),
            Invocation::Solstone
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

        // 3. argv0 solstone-core-journal, args ["status"]
        hooks_run = 0;
        solstone_run = 0;
        journal_run = 0;
        let code = dispatch(
            Some(OsStr::new("solstone-core-journal")),
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
        assert_eq!(hooks_run, 0);
        assert_eq!(journal_run, 1);
        assert_eq!(journal_args, vec![OsString::from("status")]);
        assert_eq!(solstone_run, 0);

        // 4. argv0 journal.exe falls through to the solstone root.
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
            |_args| {
                journal_run += 1;
                ExitCode::SUCCESS
            },
        );
        assert_eq!(code, ExitCode::SUCCESS);
        assert_eq!(hooks_run, 1);
        assert_eq!(journal_run, 0);
        assert_eq!(solstone_run, 1);
    }

    #[test]
    fn public_journal_invocation_enters_the_solstone_root_without_a_notice() {
        for argv0 in [
            "journal",
            "journal.exe",
            "/usr/local/bin/journal",
            "C:\\install\\bin\\journal.exe",
            "JOURNAL.EXE",
        ] {
            let mut hooks_run = 0;
            let mut journal_run = 0;
            let mut solstone_run = 0;
            let code = dispatch(
                Some(OsStr::new(argv0)),
                vec![OsString::from("status")],
                || None,
                || hooks_run += 1,
                |_name, _args| {
                    solstone_run += 1;
                    ExitCode::SUCCESS
                },
                |_args| {
                    journal_run += 1;
                    ExitCode::SUCCESS
                },
            );
            assert_eq!(code, ExitCode::SUCCESS, "failed for {argv0}");
            assert_eq!(hooks_run, 1, "hooks did not run for {argv0}");
            assert_eq!(journal_run, 0, "journal ran for {argv0}");
            assert_eq!(solstone_run, 1, "solstone root did not run for {argv0}");
        }
    }

    #[test]
    fn core_journal_invocation_bypasses_hooks_and_invokes_journal_runner() {
        for argv0 in [
            "solstone-core-journal",
            "solstone-core-journal.exe",
            "/opt/solstone/bin/solstone-core-journal",
            "C:\\opt\\bin\\solstone-core-journal.exe",
        ] {
            let mut hooks_run = 0;
            let mut journal_run = 0;
            let mut solstone_run = 0;
            let code = dispatch(
                Some(OsStr::new(argv0)),
                vec![OsString::from("status")],
                || None,
                || hooks_run += 1,
                |_name, _args| {
                    solstone_run += 1;
                    ExitCode::SUCCESS
                },
                |_args| {
                    journal_run += 1;
                    ExitCode::SUCCESS
                },
            );
            assert_eq!(code, ExitCode::SUCCESS, "failed for {argv0}");
            assert_eq!(hooks_run, 0, "hooks ran for {argv0}");
            assert_eq!(journal_run, 1, "journal runner did not run for {argv0}");
            assert_eq!(solstone_run, 0, "solstone ran for {argv0}");
        }
    }

    #[test]
    fn solstone_invocation_runs_hooks_and_enters_solstone_root() {
        for argv0 in [
            "solstone",
            "solstone.exe",
            "solstone-core-sol",
            "/usr/bin/solstone",
            "C:\\bin\\solstone.exe",
        ] {
            let mut hooks_run = 0;
            let mut journal_run = 0;
            let mut solstone_run = 0;
            let code = dispatch(
                Some(OsStr::new(argv0)),
                vec![OsString::from("status")],
                || None,
                || hooks_run += 1,
                |_name, _args| {
                    solstone_run += 1;
                    ExitCode::SUCCESS
                },
                |_args| {
                    journal_run += 1;
                    ExitCode::SUCCESS
                },
            );
            assert_eq!(code, ExitCode::SUCCESS, "failed for {argv0}");
            assert_eq!(hooks_run, 1, "hooks did not run for {argv0}");
            assert_eq!(solstone_run, 1, "solstone root did not run for {argv0}");
            assert_eq!(journal_run, 0, "journal ran for {argv0}");
        }
    }
}
