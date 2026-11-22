// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::io::{self, IsTerminal, Write};
use std::path::Path;
use std::process::ExitCode;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use solstone_core_cli::ServiceLogsArgs;
use solstone_core_operational_logs::{
    CollectError, FollowFatalError, SourceTailSnapshot, collect_source_tail_snapshot,
    normalize_raw_stream, run_follow_from_snapshot,
};
use solstone_core_system_health::{sanitize_for_terminal, sanitize_os_bytes_for_terminal};

const SERVICE_SOURCE: &str = "service";
const TAIL_BYTE_LIMIT: usize = 40_000;
const TAIL_CODEPOINT_LIMIT: usize = 10_000;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum HostPlatform {
    Linux,
    Darwin,
    Windows,
    Unsupported(&'static str),
}

impl HostPlatform {
    fn current() -> Self {
        Self::from_os(std::env::consts::OS)
    }

    // The supervisor writes its `service` oplog on every journal platform,
    // Windows included, so the tail reads the same files everywhere.
    fn from_os(os: &'static str) -> Self {
        match os {
            "linux" => Self::Linux,
            "macos" => Self::Darwin,
            "windows" => Self::Windows,
            other => Self::Unsupported(other),
        }
    }
}

pub(super) fn run(args: ServiceLogsArgs) -> ExitCode {
    let mut stderr = io::stderr().lock();
    if let Some(exit) = unsupported_platform(HostPlatform::current(), &mut stderr) {
        return exit;
    }
    let journal = match super::resolve_process_journal_path() {
        Ok(journal) => journal.path,
        Err(error) => return super::print_journal_error(error),
    };
    let snapshot = match collect_source_tail_snapshot(
        &journal,
        jiff::ZonedDateTime::now().datetime(),
        SERVICE_SOURCE,
        TAIL_BYTE_LIMIT,
    ) {
        Ok(snapshot) => snapshot,
        Err(error) => return failure(&mut stderr, collect_error_message(error)),
    };
    let is_tty = io::stdout().is_terminal();
    let mut stdout = io::stdout().lock();
    if let Err(error) = render_snapshot(&snapshot, is_tty, &mut stdout) {
        return failure(&mut stderr, SafeDiagnostic::source("stdout", &error));
    }
    if !args.follow {
        return ExitCode::SUCCESS;
    }
    let stopped = Arc::new(AtomicBool::new(false));
    install_stop_listener(Arc::clone(&stopped));
    match run_follow_from_snapshot(
        snapshot,
        &journal,
        &|| stopped.load(Ordering::Relaxed),
        &mut stdout,
        is_tty,
    ) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => failure(&mut stderr, follow_error_message(error)),
    }
}

fn render_snapshot(
    snapshot: &SourceTailSnapshot,
    is_tty: bool,
    output: &mut dyn Write,
) -> io::Result<()> {
    if !snapshot.has_descriptors() {
        return writeln!(
            output,
            "{}",
            solstone_core_cli_copy::render_for(
                crate::host_locale(),
                "journal.cli.host.logs.service.empty",
                &[]
            )
        );
    }
    let normalized = normalize_raw_stream(snapshot.tail(), is_tty);
    let body = final_codepoints(&normalized, TAIL_CODEPOINT_LIMIT);
    let header = solstone_core_cli_copy::render_for(
        crate::host_locale(),
        "journal.cli.host.logs.service.heading",
        &[],
    );
    let mut staged = Vec::with_capacity(header.len() + body.len() + 2);
    staged.extend_from_slice(header.as_bytes());
    staged.push(b'\n');
    staged.extend_from_slice(body.as_bytes());
    staged.push(b'\n');
    output.write_all(&staged)
}

fn unsupported_platform(platform: HostPlatform, stderr: &mut impl Write) -> Option<ExitCode> {
    let HostPlatform::Unsupported(platform) = platform else {
        return None;
    };
    let _ = writeln!(
        stderr,
        "{}",
        solstone_core_cli_copy::display_for(
            crate::host_locale(),
            "journal.cli.host.logs.service.unsupported",
            [("platform", &(sanitize_for_terminal(platform)))]
        )
    );
    Some(ExitCode::FAILURE)
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

fn final_codepoints(value: &str, count: usize) -> &str {
    value
        .char_indices()
        .rev()
        .nth(count.saturating_sub(1))
        .map_or(value, |(offset, _)| &value[offset..])
}

fn failure(stderr: &mut impl Write, message: SafeDiagnostic) -> ExitCode {
    let _ = writeln!(
        stderr,
        "{}",
        solstone_core_cli_copy::display_for(
            crate::host_locale(),
            "journal.cli.host.logs.service.error",
            [("detail", &(message.0))]
        )
    );
    ExitCode::FAILURE
}

fn collect_error_message(error: CollectError) -> SafeDiagnostic {
    SafeDiagnostic::dynamic(error.to_string())
}

pub(super) fn known_follow_error(error: &FollowFatalError, path: &str) -> Option<String> {
    use solstone_core_operational_logs::FollowDiagnostic;
    let id = match error.diagnostic()? {
        FollowDiagnostic::RootUnavailable => "journal.cli.host.logs.follow.root",
        FollowDiagnostic::DescriptorChanged => "journal.cli.host.logs.follow.descriptor",
    };
    Some(solstone_core_cli_copy::display_for(
        crate::host_locale(),
        id,
        [("path", &path)],
    ))
}

fn follow_error_message(error: FollowFatalError) -> SafeDiagnostic {
    let path = terminal_path(&error.path);
    if let Some(message) = known_follow_error(&error, &path) {
        return SafeDiagnostic::dynamic(message);
    }
    match error.source {
        Some(source) => SafeDiagnostic::dynamic(solstone_core_cli_copy::display_for(
            crate::host_locale(),
            "journal.cli.host.logs.follow.source",
            [
                ("operation", &(error.operation)),
                ("path", &(path)),
                ("source", &(source)),
            ],
        )),
        None => SafeDiagnostic::dynamic(solstone_core_cli_copy::display_for(
            crate::host_locale(),
            "journal.cli.host.logs.follow.failed",
            [("operation", &(error.operation)), ("path", &(path))],
        )),
    }
}

struct SafeDiagnostic(String);

impl SafeDiagnostic {
    fn dynamic(value: impl AsRef<str>) -> Self {
        Self(sanitize_for_terminal(value.as_ref()))
    }

    fn source(operation: &str, source: &dyn std::fmt::Display) -> Self {
        Self::dynamic(solstone_core_cli_copy::display_for(
            crate::host_locale(),
            "journal.cli.host.logs.service.operation",
            [("operation", &(operation)), ("source", &(source))],
        ))
    }
}

fn terminal_path(path: &Path) -> String {
    sanitize_os_bytes_for_terminal(path.as_os_str().as_encoded_bytes())
}

#[cfg(test)]
mod tests {
    use solstone_core_journal_io::{
        JournalRoot,
        operational_log::{OplogFormat, create_oplog_at},
    };

    use super::*;

    #[test]
    fn snapshot_rendering_preserves_the_existing_terminal_policy() {
        let journal = tempfile::tempdir().unwrap();
        let opened = jiff::civil::date(2026, 8, 7)
            .at(12, 0, 0, 0)
            .to_zoned_datetime(jiff::tz::TimeZone::fixed(jiff::tz::Offset::UTC))
            .unwrap();
        let mut writer = create_oplog_at(
            JournalRoot::open(journal.path()).unwrap(),
            SERVICE_SOURCE,
            "supervisor",
            OplogFormat::Log,
            &opened,
        )
        .unwrap();
        use std::io::Write as _;
        writer.write_all(b"first\r\nsecond\rlast\xff").unwrap();
        let snapshot = collect_source_tail_snapshot(
            journal.path(),
            opened.datetime(),
            SERVICE_SOURCE,
            TAIL_BYTE_LIMIT,
        )
        .unwrap();
        let mut rendered = Vec::new();
        render_snapshot(&snapshot, false, &mut rendered).unwrap();
        assert_eq!(
            rendered,
            "=== service logs ===\nfirst\nsecond\nlast�\n".as_bytes()
        );
    }

    #[test]
    fn every_journal_platform_reads_the_service_tail() {
        for os in ["linux", "macos", "windows"] {
            let mut stderr = Vec::new();
            assert!(
                unsupported_platform(HostPlatform::from_os(os), &mut stderr).is_none(),
                "{os} must read the service tail"
            );
            assert!(stderr.is_empty());
        }
        let mut stderr = Vec::new();
        assert!(unsupported_platform(HostPlatform::from_os("freebsd"), &mut stderr).is_some());
        assert_eq!(stderr, b"Error: unsupported platform 'freebsd'\n");
    }

    #[test]
    fn no_oplog_leaves_have_a_stable_one_shot_message() {
        let journal = tempfile::tempdir().unwrap();
        let snapshot = collect_source_tail_snapshot(
            journal.path(),
            jiff::ZonedDateTime::now().datetime(),
            SERVICE_SOURCE,
            TAIL_BYTE_LIMIT,
        )
        .unwrap();
        let mut rendered = Vec::new();
        render_snapshot(&snapshot, false, &mut rendered).unwrap();
        assert_eq!(rendered, b"=== service logs === (no oplog leaves)\n");
    }
}
