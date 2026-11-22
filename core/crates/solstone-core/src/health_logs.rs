// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::io::IsTerminal;
use std::process::ExitCode;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use solstone_core_cli::{HealthLogsArgs, HealthLogsValueCheck};
use solstone_core_operational_logs::{
    CollectError, HealthLogsQuery, ParsedCount, collect_health_logs, parse_health_log_count,
    render_collected, run_follow_on_empty,
};
use solstone_core_system::operational_log_parse::{HealthLogSinceError, parse_health_log_since};
use solstone_core_system_health::{
    GrepCompileError, compile_grep_pattern, sanitize_for_terminal, sanitize_os_bytes_for_terminal,
};

pub(super) fn run(args: HealthLogsArgs) -> ExitCode {
    let now = jiff::ZonedDateTime::now().datetime();
    let prepared = match prepare(args, now) {
        Ok(prepared) => prepared,
        Err(error) => return usage_value_error(&error),
    };
    let journal = match super::resolve_process_journal_path() {
        Ok(journal) => journal.path,
        Err(error) => return super::print_journal_error(error),
    };
    let is_tty = std::io::stdout().is_terminal();
    if prepared.follow {
        return run_follow_mode(&journal, is_tty);
    }
    let query = HealthLogsQuery {
        count: prepared.count,
        since: prepared.since,
        service: prepared.service,
        grep: prepared.grep,
    };
    match collect_health_logs(&journal, now, &query) {
        Ok(rows) => match render_collected(&mut std::io::stdout(), &rows, is_tty) {
            Ok(()) => ExitCode::SUCCESS,
            Err(error) => failure(SafeDiagnostic::dynamic(error.to_string())),
        },
        Err(error) => failure(collect_error_message(error)),
    }
}

pub(super) fn help(args: HealthLogsArgs) -> ExitCode {
    let now = jiff::ZonedDateTime::now().datetime();
    if let Err(error) = prepare(args, now) {
        return usage_value_error(&error);
    }
    print!(
        "{}",
        solstone_core_cli_copy::render_for(
            super::host_locale(),
            "journal.cli.host.health.logs.help",
            &[]
        )
    );
    ExitCode::SUCCESS
}

pub(super) fn usage(args: HealthLogsArgs) -> ExitCode {
    let now = jiff::ZonedDateTime::now().datetime();
    if let Err(error) = prepare(args, now) {
        return usage_value_error(&error);
    }
    eprint!(
        "{}",
        solstone_core_cli_copy::render_for(
            super::host_locale(),
            "journal.cli.host.health.logs.usage",
            &[]
        )
    );
    eprintln!(
        "{}",
        solstone_core_cli_copy::display_for(
            super::host_locale(),
            "journal.cli.host.arguments.invalid",
            [("command", &("solstone journal health logs"))]
        )
    );
    ExitCode::from(2)
}

struct PreparedHealthLogs {
    count: i64,
    follow: bool,
    since: Option<jiff::civil::DateTime>,
    service: Option<String>,
    grep: Option<solstone_core_system_health::GrepPattern>,
}

fn prepare(args: HealthLogsArgs, now: jiff::civil::DateTime) -> Result<PreparedHealthLogs, String> {
    let mut count = 5;
    let mut since = None;
    let mut grep = None;
    for check in args.value_checks {
        match check {
            HealthLogsValueCheck::Count(value) => {
                count = match parse_health_log_count(&value) {
                    Ok(ParsedCount::Value(value)) => value,
                    Ok(ParsedCount::SaturatedPositive) => i64::MAX,
                    Ok(ParsedCount::SaturatedNegative) => i64::MIN,
                    Err(error) => {
                        return Err(solstone_core_cli_copy::display_for(
                            crate::host_locale(),
                            "journal.cli.host.logs.count",
                            [("detail", &(format!("{error:?}")))],
                        ));
                    }
                };
            }
            HealthLogsValueCheck::Since(value) => {
                since = Some(
                    parse_health_log_since(&value, now).map_err(|error| match error {
                        HealthLogSinceError::RelativeOverflow { .. } => {
                            solstone_core_cli_copy::render_for(
                                crate::host_locale(),
                                "journal.cli.host.logs.since.overflow",
                                &[],
                            )
                        }
                        HealthLogSinceError::InvalidTime { input } => {
                            solstone_core_cli_copy::display_for(
                                crate::host_locale(),
                                "journal.cli.host.logs.since.invalid",
                                [("input", &(input))],
                            )
                        }
                    })?,
                );
            }
            HealthLogsValueCheck::Grep(value) => {
                grep = Some(compile_grep_pattern(&value).map_err(grep_error)?);
            }
        }
    }
    Ok(PreparedHealthLogs {
        count,
        follow: args.follow,
        since,
        service: args.service,
        grep,
    })
}

fn run_follow_mode(journal: &std::path::Path, is_tty: bool) -> ExitCode {
    let stopped = Arc::new(AtomicBool::new(false));
    install_stop_listener(stopped.clone());
    let result = run_follow_on_empty(
        journal,
        &|| stopped.load(Ordering::Relaxed),
        &mut std::io::stdout(),
        is_tty,
        &mut || {
            eprintln!(
                "{}",
                solstone_core_cli_copy::render_for(
                    crate::host_locale(),
                    "journal.cli.host.logs.follow.empty",
                    &[]
                )
            )
        },
    );
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => failure(follow_error_message(error)),
    }
}

fn install_stop_listener(stopped: Arc<AtomicBool>) {
    let _ = std::thread::spawn(move || {
        let Ok(runtime) = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
        else {
            return;
        };
        runtime.block_on(async {
            #[cfg(unix)]
            {
                use tokio::signal::unix::{SignalKind, signal};
                if let Ok(mut term) = signal(SignalKind::terminate()) {
                    tokio::select! { _ = tokio::signal::ctrl_c() => {}, _ = term.recv() => {} }
                } else {
                    let _ = tokio::signal::ctrl_c().await;
                }
            }
            #[cfg(not(unix))]
            {
                let _ = tokio::signal::ctrl_c().await;
            }
        });
        stopped.store(true, Ordering::Relaxed);
    });
}

fn usage_value_error(message: &str) -> ExitCode {
    eprintln!(
        "{}",
        solstone_core_cli_copy::display_for(
            crate::host_locale(),
            "journal.cli.host.logs.health.usage",
            [("detail", &(sanitize_for_terminal(message)))]
        )
    );
    ExitCode::from(2)
}

fn failure(message: SafeDiagnostic) -> ExitCode {
    eprintln!(
        "{}",
        solstone_core_cli_copy::display_for(
            crate::host_locale(),
            "journal.cli.host.logs.health.error",
            [("detail", &(message.0))]
        )
    );
    ExitCode::FAILURE
}

struct SafeDiagnostic(String);

impl SafeDiagnostic {
    fn dynamic(value: impl AsRef<str>) -> Self {
        Self(sanitize_for_terminal(value.as_ref()))
    }
}

fn collect_error_message(error: CollectError) -> SafeDiagnostic {
    match error {
        CollectError::Root | CollectError::CatalogIo | CollectError::CatalogUtf8 => {
            SafeDiagnostic::dynamic(error.to_string())
        }
        CollectError::Catalog(error) => SafeDiagnostic::dynamic(error.to_string()),
    }
}

fn follow_error_message(error: solstone_core_operational_logs::FollowFatalError) -> SafeDiagnostic {
    let path = terminal_path(&error.path);
    if let Some(message) = crate::service_logs::known_follow_error(&error, &path) {
        return SafeDiagnostic(message);
    }
    match error.source {
        Some(source) => SafeDiagnostic(solstone_core_cli_copy::display_for(
            crate::host_locale(),
            "journal.cli.host.logs.follow.source",
            [
                ("operation", &(error.operation)),
                ("path", &(path)),
                ("source", &(sanitize_for_terminal(&source.to_string()))),
            ],
        )),
        None => SafeDiagnostic(solstone_core_cli_copy::display_for(
            crate::host_locale(),
            "journal.cli.host.logs.follow.failed",
            [("operation", &(error.operation)), ("path", &(path))],
        )),
    }
}

fn terminal_path(path: &std::path::Path) -> String {
    sanitize_os_bytes_for_terminal(path.as_os_str().as_encoded_bytes())
}

fn grep_error(error: GrepCompileError) -> String {
    match error {
        GrepCompileError::UnsupportedFamily { family, offset } => {
            solstone_core_cli_copy::display_for(
                crate::host_locale(),
                "journal.cli.host.logs.grep.unsupported",
                [("family", &(family)), ("offset", &(offset))],
            )
        }
        GrepCompileError::InvalidPattern { offset } => solstone_core_cli_copy::display_for(
            crate::host_locale(),
            "journal.cli.host.logs.grep.invalid",
            [("offset", &(offset))],
        ),
        GrepCompileError::NativeCompileFailure => solstone_core_cli_copy::render_for(
            crate::host_locale(),
            "journal.cli.host.logs.grep.compile",
            &[],
        ),
    }
}

#[cfg(all(test, unix))]
mod tests {
    use std::os::unix::ffi::OsStringExt;
    use std::path::PathBuf;

    use super::*;

    #[test]
    fn safe_diagnostics_escape_dynamic_text_once() {
        assert_eq!(
            SafeDiagnostic::dynamic("ordinary source").0,
            "ordinary source"
        );
        assert_eq!(
            SafeDiagnostic::dynamic("source\\\n\x1b\u{202e}").0,
            "source\\\\\\n\\x1b\\u{202e}"
        );
    }

    #[test]
    fn follow_fatal_keeps_safe_path_and_source_from_double_escaping() {
        let error = solstone_core_operational_logs::FollowFatalError {
            path: PathBuf::from(std::ffi::OsString::from_vec(b"bad\\\xff".to_vec())),
            operation: "read",
            source: Some(std::io::Error::other("source\\\n\x1b\u{202e}")),
        };
        assert_eq!(
            follow_error_message(error).0,
            "read failed for bad\\\\\\xff: source\\\\\\n\\x1b\\u{202e}"
        );
    }
}
