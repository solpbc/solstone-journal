// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use solstone_core_cli_copy::{display_for, render_for};
use solstone_core_generate::{CapturedStream, ChildStatus, ClientError};

fn status(locale: &str, status: &ChildStatus) -> String {
    match (status.exit_code, status.signal) {
        (Some(code), Some(signal)) => display_for(
            locale,
            "journal.cli.share.provider.child.exit.signal",
            [("code", &code), ("signal", &signal)],
        ),
        (Some(code), None) => display_for(
            locale,
            "journal.cli.share.provider.child.exit",
            [("code", &code)],
        ),
        (None, Some(signal)) => display_for(
            locale,
            "journal.cli.share.provider.child.signal",
            [("signal", &signal)],
        ),
        (None, None) => render_for(
            locale,
            "journal.cli.share.provider.child.status.unknown",
            &[],
        ),
    }
}

fn stream(locale: &str, stream: &CapturedStream) -> String {
    let bytes = stream.escaped();
    if stream.truncated {
        display_for(
            locale,
            "journal.cli.share.provider.child.truncated",
            [("bytes", &bytes)],
        )
    } else {
        bytes.to_string()
    }
}

/// The transport's status and diagnostics belong to the journal. Its captured
/// bytes, protocol reason, and supplied failure details belong to the child.
pub(super) fn describe(error: &ClientError, locale: &str) -> String {
    match error {
        ClientError::Resolve(text) | ClientError::Decode(text) => text.clone(),
        ClientError::Io {
            primary,
            cleanup: None,
        } => primary.clone(),
        ClientError::Io {
            primary,
            cleanup: Some(cleanup),
        } => display_for(
            locale,
            "journal.cli.share.provider.child.cleanup",
            [("primary", primary), ("cleanup", cleanup)],
        ),
        ClientError::Protocol(f) => display_for(
            locale,
            "journal.cli.share.provider.child.protocol",
            [
                ("reason", &f.error.reason),
                ("status", &status(locale, &f.status)),
                ("detail", &f.error.detail),
                ("early", &f.stdin_closed_early),
                ("out", &stream(locale, &f.stdout)),
                ("err", &stream(locale, &f.stderr)),
            ],
        ),
        ClientError::UnexpectedChild(f) => display_for(
            locale,
            "journal.cli.share.provider.child.unexpected",
            [
                ("status", &status(locale, &f.status)),
                ("early", &f.stdin_closed_early),
                ("out", &stream(locale, &f.stdout)),
                ("err", &stream(locale, &f.stderr)),
            ],
        ),
        ClientError::InvalidResponse(f) => display_for(
            locale,
            "journal.cli.share.provider.child.invalid",
            [
                ("status", &status(locale, &f.status)),
                ("detail", &f.detail),
                ("early", &f.stdin_closed_early),
                ("out", &stream(locale, &f.stdout)),
                ("err", &stream(locale, &f.stderr)),
            ],
        ),
        ClientError::ProcessIo(f) => {
            let status = f
                .status
                .as_ref()
                .map(|s| status(locale, s))
                .unwrap_or_else(|| {
                    render_for(locale, "journal.cli.share.provider.child.unavailable", &[])
                });
            let out = stream(locale, &f.stdout);
            let err = stream(locale, &f.stderr);
            let mut values = vec![
                ("primary", f.primary.clone()),
                ("status", status),
                ("early", f.stdin_closed_early.to_string()),
                ("out", out),
                ("err", err),
            ];
            let key = if f.cleanup.is_empty() {
                "journal.cli.share.provider.child.process"
            } else {
                values.push(("cleanup", f.cleanup.join("; ")));
                "journal.cli.share.provider.child.process.cleanup"
            };
            render_for(
                locale,
                key,
                &values
                    .iter()
                    .map(|(k, v)| (*k, solstone_core_cli_copy::Value::String(v)))
                    .collect::<Vec<_>>(),
            )
        }
    }
}
