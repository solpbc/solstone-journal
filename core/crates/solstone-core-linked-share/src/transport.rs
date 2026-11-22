// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use solstone_core_cli_copy::{display_for, render_for};
use solstone_core_generate::TransportDiagnostic as D;

/// Present journal-authored diagnostics from the installed one-shot process.
pub fn transport_diagnostic(locale: &str, diagnostic: D) -> String {
    match diagnostic {
        D::Object => render_for(locale, "journal.cli.share.transport.object", &[]),
        D::StringField { field } => display_for(
            locale,
            "journal.cli.share.transport.field.string",
            [("field", &field)],
        ),
        D::OptionalString { field } => display_for(
            locale,
            "journal.cli.share.transport.field.optional",
            [("field", &field)],
        ),
        D::StringItems { field } => display_for(
            locale,
            "journal.cli.share.transport.field.strings",
            [("field", &field)],
        ),
        D::OptionalArray { field } => display_for(
            locale,
            "journal.cli.share.transport.field.array",
            [("field", &field)],
        ),
        D::Schema => render_for(locale, "journal.cli.share.transport.schema", &[]),
        D::Usage => render_for(locale, "journal.cli.share.transport.usage", &[]),
        D::Boolean { field } => display_for(
            locale,
            "journal.cli.share.transport.field.boolean",
            [("field", &field)],
        ),
        D::Outcome => render_for(locale, "journal.cli.share.transport.outcome", &[]),
        D::Truncated { limit } => display_for(
            locale,
            "journal.cli.share.transport.stdout.limit",
            [("limit", &limit)],
        ),
        D::WireUnavailable { pipe } => display_for(
            locale,
            "journal.cli.share.transport.pipe.missing",
            [("pipe", &pipe)],
        ),
        D::StartWorker { pipe, detail } => display_for(
            locale,
            "journal.cli.share.transport.worker.start",
            [("pipe", &pipe), ("detail", &detail)],
        ),
        D::Terminate { detail } => display_for(
            locale,
            "journal.cli.share.transport.child.terminate",
            [("detail", &detail)],
        ),
        D::Inspect { detail } => display_for(
            locale,
            "journal.cli.share.transport.child.inspect",
            [("detail", &detail)],
        ),
        D::Reap { detail } => display_for(
            locale,
            "journal.cli.share.transport.child.reap",
            [("detail", &detail)],
        ),
        D::RetryReap { detail } => display_for(
            locale,
            "journal.cli.share.transport.child.retry",
            [("detail", &detail)],
        ),
        D::Completion { detail } => display_for(
            locale,
            "journal.cli.share.transport.worker.channel",
            [("detail", &detail)],
        ),
        D::JoinPanic { pipe } => display_for(
            locale,
            "journal.cli.share.transport.worker.join",
            [("pipe", &pipe)],
        ),
        D::Writer { detail } => display_for(
            locale,
            "journal.cli.share.transport.writer",
            [("detail", &detail)],
        ),
        D::Collector { pipe, detail } => display_for(
            locale,
            "journal.cli.share.transport.collector",
            [("pipe", &pipe), ("detail", &detail)],
        ),
        D::WriterPanic => render_for(locale, "journal.cli.share.transport.writer.panic", &[]),
        D::CollectorPanic { pipe } => display_for(
            locale,
            "journal.cli.share.transport.collector.panic",
            [("pipe", &pipe)],
        ),
        D::EmptyContents => render_for(locale, "journal.cli.share.transport.request.empty", &[]),
        D::OverflowRequiresText => {
            render_for(locale, "journal.cli.share.transport.request.overflow", &[])
        }
    }
}
