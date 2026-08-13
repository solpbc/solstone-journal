// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::io::IsTerminal;
use std::process::ExitCode;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Instant;

use chrono::Local;
use solstone_core_cli::HealthLogsArgs;
use solstone_core_operational_logs::{
    CollectError, EnumerationError, HealthDirectoryState, HealthLogsQuery, OrdinaryTailError,
    ParsedCount, StdDayLogDirectoryOps, StdFollowFs, StdProbeOps, StdTailFileOpener,
    collect_health_logs, parse_health_log_count, probe_health_directory, render_collected,
    run_follow,
};
use solstone_core_system::operational_log_parse::parse_health_log_since;
use solstone_core_system_health::{
    GrepCompileError, compile_grep_pattern, sanitize_for_terminal, sanitize_os_bytes_for_terminal,
};

pub(super) fn run(args: HealthLogsArgs) -> ExitCode {
    let journal = match super::resolve_process_journal_path() {
        Ok(journal) => journal.path,
        Err(error) => return super::print_journal_error(error),
    };
    let now = Local::now().naive_local();
    let count = match parse_health_log_count(&args.count) {
        Ok(ParsedCount::Value(value)) => value,
        Ok(ParsedCount::SaturatedPositive) => i64::MAX,
        Ok(ParsedCount::SaturatedNegative) => i64::MIN,
        Err(error) => return usage_value_error(&format!("invalid count: {error:?}")),
    };
    let since = match args
        .since
        .as_deref()
        .map(|value| parse_health_log_since(value, now))
    {
        Some(Ok(value)) => Some(value),
        Some(Err(error)) => return usage_value_error(&error.to_string()),
        None => None,
    };
    let grep = match args.grep.as_deref().map(compile_grep_pattern) {
        Some(Ok(value)) => Some(value),
        Some(Err(error)) => return usage_value_error(&grep_error(error)),
        None => None,
    };
    let is_tty = std::io::stdout().is_terminal();
    if args.follow {
        return run_follow_mode(&journal, is_tty);
    }
    let query = HealthLogsQuery {
        count,
        since,
        service: args.service,
        grep,
    };
    match collect_health_logs(
        &journal,
        now,
        &query,
        &StdProbeOps,
        &StdTailFileOpener,
        &StdDayLogDirectoryOps,
    ) {
        Ok(rows) => match render_collected(&mut std::io::stdout(), &rows, is_tty) {
            Ok(()) => ExitCode::SUCCESS,
            Err(error) => failure(&error.to_string()),
        },
        Err(error) => failure(&collect_error_message(error)),
    }
}

fn run_follow_mode(journal: &std::path::Path, is_tty: bool) -> ExitCode {
    let health_dir = journal.join("health");
    match probe_health_directory(&health_dir, &StdProbeOps) {
        Ok(HealthDirectoryState::Absent | HealthDirectoryState::NotADirectory) => {
            eprintln!("No health directory found.");
            ExitCode::SUCCESS
        }
        Err(error) => failure(&path_error_message(&error.path, &error.source)),
        Ok(HealthDirectoryState::Directory) => {
            let stopped = Arc::new(AtomicBool::new(false));
            install_stop_listener(stopped.clone());
            let start = Instant::now();
            let result = run_follow(
                &StdFollowFs,
                &health_dir,
                &|| start.elapsed(),
                &|| stopped.load(Ordering::Relaxed),
                &mut std::io::stdout(),
                is_tty,
                &mut |warning| eprintln!("{}", sanitize_for_terminal(&warning)),
            );
            match result {
                Ok(()) => ExitCode::SUCCESS,
                Err(error) => failure(&follow_error_message(error)),
            }
        }
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
        "journal health logs: error: {}",
        sanitize_for_terminal(message)
    );
    ExitCode::from(2)
}

fn failure(message: &str) -> ExitCode {
    eprintln!("health logs: {}", sanitize_for_terminal(message));
    ExitCode::FAILURE
}

fn collect_error_message(error: CollectError) -> String {
    match error {
        CollectError::HealthDirectoryProbe(error) | CollectError::SupervisorProbe(error) => {
            path_error_message(&error.path, &error.source)
        }
        CollectError::Enumeration(EnumerationError::Enumerate { path, source }) => {
            path_error_message(&path, &source)
        }
        CollectError::InvalidUtf8(OrdinaryTailError::InvalidUtf8 { path, source }) => {
            path_error_message(&path, &source)
        }
    }
}

fn follow_error_message(error: solstone_core_operational_logs::FollowFatalError) -> String {
    let message = format!(
        "{} failed for {}",
        error.operation,
        terminal_path(&error.path)
    );
    error
        .source
        .as_ref()
        .map(|source| format!("{message}: {source}"))
        .unwrap_or(message)
}

fn path_error_message(path: &std::path::Path, source: &dyn std::fmt::Display) -> String {
    format!("{}: {source}", terminal_path(path))
}

fn terminal_path(path: &std::path::Path) -> String {
    sanitize_os_bytes_for_terminal(path.as_os_str().as_encoded_bytes())
}

fn grep_error(error: GrepCompileError) -> String {
    match error {
        GrepCompileError::UnsupportedFamily { family, offset } => {
            format!("unsupported regex feature '{family}' at byte offset {offset}")
        }
        GrepCompileError::InvalidPattern { offset } => {
            format!("invalid regex pattern at byte offset {offset}")
        }
        GrepCompileError::NativeCompileFailure => "regex pattern could not be compiled".to_owned(),
    }
}
