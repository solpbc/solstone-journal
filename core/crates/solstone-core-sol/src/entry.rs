// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! The process entry behind the binary.
//!
//! Invoked as `solstone`, `solstone.exe`, or `solstone-core-sol`, it enters the
//! solstone root after running any installer hooks. Invoked as private
//! `solstone-core-journal`, it enters the journal command family directly.
//! Invoked as public `journal` or `journal.exe`, it prints a notice to stderr
//! pointing to `solstone journal` and exits nonzero without running hooks or
//! accessing the journal.

use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

/// The command family an invocation enters.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Invocation {
    Solstone,
    CoreJournal,
    PublicNotice,
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
        Invocation::PublicNotice => {
            eprint!("{}", render_notice(&args));
            ExitCode::from(1)
        }
        Invocation::CoreJournal => run_journal(args),
        Invocation::Solstone => {
            run_hooks();
            run_solstone("solstone", args)
        }
    }
}

/// The `solstone journal` command family, entered by private `solstone-core-journal`
/// and by the `solstone journal` subcommand. Public `journal` does not enter it.
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
    if name == "journal" {
        Some(Invocation::PublicNotice)
    } else if name == "solstone-core-journal" {
        Some(Invocation::CoreJournal)
    } else if SOLSTONE_NAMES.contains(&name) {
        Some(Invocation::Solstone)
    } else {
        None
    }
}

pub fn quote_posix_arg(arg: &str) -> String {
    let mut quoted = String::with_capacity(arg.len() + 2);
    quoted.push('\'');
    for ch in arg.chars() {
        if ch == '\'' {
            quoted.push_str(r"'\''");
        } else {
            quoted.push(ch);
        }
    }
    quoted.push('\'');
    quoted
}

pub fn quote_powershell_arg(arg: &str) -> String {
    let mut quoted = String::with_capacity(arg.len() + 2);
    quoted.push('\'');
    for ch in arg.chars() {
        if ch == '\'' {
            quoted.push_str("''");
        } else {
            quoted.push(ch);
        }
    }
    quoted.push('\'');
    quoted
}

pub fn render_notice(args: &[OsString]) -> String {
    let quoter: fn(&str) -> String = if cfg!(windows) {
        quote_powershell_arg
    } else {
        quote_posix_arg
    };
    render_notice_with(args, quoter)
}

pub fn render_notice_with(args: &[OsString], quote: impl Fn(&str) -> String) -> String {
    if args.is_empty() {
        "the 'journal' command has moved. run:\n  solstone journal\n".to_string()
    } else {
        let mut out = String::from("the 'journal' command has moved. run:\n  solstone journal");
        for arg in args {
            out.push(' ');
            out.push_str(&quote(&arg.to_string_lossy()));
        }
        out.push('\n');
        out
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
            "journal",
            "/usr/local/bin/journal",
            "C:\\Users\\Owner\\AppData\\Local\\Journal\\current\\bin\\journal.exe",
            "JOURNAL.EXE",
        ] {
            assert_eq!(named(argv0), Invocation::PublicNotice, "{argv0}");
        }
        for argv0 in [
            "solstone-core-journal",
            "/opt/solstone/bin/solstone-core-journal",
            "solstone-core-journal.exe",
            "SOLSTONE-CORE-JOURNAL.EXE",
        ] {
            assert_eq!(named(argv0), Invocation::CoreJournal, "{argv0}");
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
            Invocation::PublicNotice
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

        // 4. argv0 journal.exe, args ["status"]
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
        assert_ne!(code, ExitCode::SUCCESS);
        assert_eq!(hooks_run, 0);
        assert_eq!(journal_run, 0);
        assert_eq!(solstone_run, 0);
    }

    fn parse_posix_notice(rendered: &str) -> Vec<String> {
        let prefix = "the 'journal' command has moved. run:\n  solstone journal";
        assert!(rendered.starts_with(prefix));
        assert!(rendered.ends_with('\n'));
        let tail = &rendered[prefix.len()..rendered.len() - 1];
        if tail.is_empty() {
            return Vec::new();
        }
        assert!(tail.starts_with(' '));
        let mut args = Vec::new();
        let mut chars = tail[1..].chars().peekable();
        while chars.peek().is_some() {
            assert_eq!(chars.next(), Some('\''));
            let mut cur = String::new();
            loop {
                match chars.next() {
                    Some('\'') => {
                        if chars.peek() == Some(&'\\') {
                            chars.next();
                            assert_eq!(chars.next(), Some('\''));
                            assert_eq!(chars.next(), Some('\''));
                            cur.push('\'');
                        } else {
                            break;
                        }
                    }
                    Some(c) => cur.push(c),
                    None => panic!("unclosed quote in {rendered}"),
                }
            }
            args.push(cur);
            if chars.peek() == Some(&' ') {
                chars.next();
            }
        }
        args
    }

    fn parse_powershell_notice(rendered: &str) -> Vec<String> {
        let prefix = "the 'journal' command has moved. run:\n  solstone journal";
        assert!(rendered.starts_with(prefix));
        assert!(rendered.ends_with('\n'));
        let tail = &rendered[prefix.len()..rendered.len() - 1];
        if tail.is_empty() {
            return Vec::new();
        }
        assert!(tail.starts_with(' '));
        let mut args = Vec::new();
        let mut chars = tail[1..].chars().peekable();
        while chars.peek().is_some() {
            assert_eq!(chars.next(), Some('\''));
            let mut cur = String::new();
            loop {
                match chars.next() {
                    Some('\'') => {
                        if chars.peek() == Some(&'\'') {
                            chars.next();
                            cur.push('\'');
                        } else {
                            break;
                        }
                    }
                    Some(c) => cur.push(c),
                    None => panic!("unclosed quote in {rendered}"),
                }
            }
            args.push(cur);
            if chars.peek() == Some(&' ') {
                chars.next();
            }
        }
        args
    }

    #[test]
    fn posix_and_powershell_quoting_round_trips_special_characters() {
        let cases: &[&[&str]] = &[
            &[],
            &[""],
            &["foo bar", "  leading", "trailing  "],
            &["it's", "say \"hello\"", "'nested'"],
            &["$", "`", ";", "&", "|", "<", ">", "*", "?", "(", ")"],
            &["first\nsecond\nthird"],
            &["-v", "--flag=value"],
            &["--help", "-V"],
            &[
                "", "a b", "'", "$foo", "`cmd`", ";", "&", "|", "<", ">", "*", "?", "(", ")", "\n",
                "-v", "--help", "-V",
            ],
        ];

        for case in cases {
            let os_args: Vec<OsString> = case.iter().map(OsString::from).collect();
            let posix_rendered = render_notice_with(&os_args, quote_posix_arg);
            let recovered_posix = parse_posix_notice(&posix_rendered);
            assert_eq!(
                &recovered_posix, case,
                "POSIX round trip failed for {case:?}"
            );

            let ps_rendered = render_notice_with(&os_args, quote_powershell_arg);
            let recovered_ps = parse_powershell_notice(&ps_rendered);
            assert_eq!(
                &recovered_ps, case,
                "PowerShell round trip failed for {case:?}"
            );
        }
    }

    #[test]
    fn public_journal_invocation_emits_notice_and_exits_without_hooks_or_journal_runner() {
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
            assert_ne!(code, ExitCode::SUCCESS, "failed for {argv0}");
            assert_eq!(hooks_run, 0, "hooks ran for {argv0}");
            assert_eq!(journal_run, 0, "journal ran for {argv0}");
            assert_eq!(solstone_run, 0, "solstone ran for {argv0}");
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

    #[cfg(all(test, feature = "full-tests"))]
    #[cfg(unix)]
    #[test]
    fn public_launcher_script_emits_notice_without_journal_directory() {
        let repo_root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .parent()
            .unwrap()
            .parent()
            .unwrap();
        let launcher = repo_root.join("scripts/root-launchers/journal");
        let temp = tempfile::tempdir().expect("tempdir");
        let cwd = temp.path();

        let expected_args = [
            "think",
            "--weekly",
            "",
            "special 'arg'",
            "first\nsecond",
            "$`;&|<>()*?\"",
        ];
        let output = std::process::Command::new("sh")
            .arg(&launcher)
            .args(expected_args)
            .current_dir(cwd)
            .env_remove("SOLSTONE_JOURNAL")
            .output()
            .expect("run launcher script");

        assert!(!output.status.success());
        assert_eq!(output.status.code(), Some(1));
        assert!(output.stdout.is_empty());

        let stderr = String::from_utf8(output.stderr).expect("utf-8 stderr");
        let argv_str = stderr
            .strip_prefix("the 'journal' command has moved. run:\n  solstone journal ")
            .and_then(|tail| tail.strip_suffix('\n'))
            .expect("notice command arguments");

        let sh_parse = std::process::Command::new("sh")
            .arg("-c")
            .arg(r#"eval "set -- $1"; for a in "$@"; do printf '%s\0' "$a"; done"#)
            .arg("_")
            .arg(argv_str)
            .output()
            .expect("parse argv via sh");
        assert!(sh_parse.status.success());
        let recovered: Vec<String> = sh_parse
            .stdout
            .strip_suffix(&[0])
            .expect("NUL-terminated shell arguments")
            .split(|&b| b == 0)
            .map(|chunk| String::from_utf8_lossy(chunk).to_string())
            .collect();

        assert_eq!(recovered, expected_args);
        assert!(!cwd.join("journal").exists());
    }
}
