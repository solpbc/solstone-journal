// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use solstone_core_journal_archive::{
    ArchiveError, ArchivePublicationError, EncodeArchiveError, ExplicitTargetError,
};
use solstone_core_journal_io::cli_diagnostics::{io_message, root_io_message, root_reason};

fn io_detail(locale: &str, error: &std::io::Error) -> String {
    io_message(error).map_or_else(|| error.to_string(), |message| message.render(locale))
}

fn fixed_detail(locale: &str, value: &'static str) -> String {
    match value {
        "must use YYYY-MM-DDTHH:MM:SSZ" => solstone_core_cli_copy::render_for(
            locale,
            "journal.cli.journal.archive.metadata.reason.must.use.yyyy.mm.ddthh.mm.ssz",
            &[],
        ),
        "invalid second" => solstone_core_cli_copy::render_for(
            locale,
            "journal.cli.journal.archive.metadata.reason.invalid.second",
            &[],
        ),
        "invalid calendar date or time" => solstone_core_cli_copy::render_for(
            locale,
            "journal.cli.journal.archive.metadata.reason.invalid.calendar.date.or.time",
            &[],
        ),
        "invalid month" => solstone_core_cli_copy::render_for(
            locale,
            "journal.cli.journal.archive.metadata.reason.invalid.month",
            &[],
        ),
        "must not contain ASCII control characters" => solstone_core_cli_copy::render_for(
            locale,
            "journal.cli.journal.archive.metadata.reason.must.not.contain.ascii.control.characters",
            &[],
        ),
        "year must be in 1980..=2107" => solstone_core_cli_copy::render_for(
            locale,
            "journal.cli.journal.archive.metadata.reason.year.must.be.in.years",
            &[],
        ),
        "invalid day" => solstone_core_cli_copy::render_for(
            locale,
            "journal.cli.journal.archive.metadata.reason.invalid.day",
            &[],
        ),
        "second must be in 0..=59" => solstone_core_cli_copy::render_for(
            locale,
            "journal.cli.journal.archive.metadata.reason.second.must.be.in.seconds",
            &[],
        ),
        "invalid minute" => solstone_core_cli_copy::render_for(
            locale,
            "journal.cli.journal.archive.metadata.reason.invalid.minute",
            &[],
        ),
        "invalid hour" => solstone_core_cli_copy::render_for(
            locale,
            "journal.cli.journal.archive.metadata.reason.invalid.hour",
            &[],
        ),
        "must contain digits in numeric positions" => solstone_core_cli_copy::render_for(
            locale,
            "journal.cli.journal.archive.metadata.reason.must.contain.digits.in.numeric.positions",
            &[],
        ),
        "must not be empty" => solstone_core_cli_copy::render_for(
            locale,
            "journal.cli.journal.archive.metadata.reason.must.not.be.empty",
            &[],
        ),
        "UTF-8 member path exceeds ZIP u16 byte limit" => solstone_core_cli_copy::render_for(
            locale,
            "journal.cli.journal.archive.fact.utf8.member.path.exceeds.zip.u16.byte.limit",
            &[],
        ),
        "archive output contains a null byte" => solstone_core_cli_copy::render_for(
            locale,
            "journal.cli.journal.archive.fact.archive.output.contains.a.null.byte",
            &[],
        ),
        "archive output ends with a separator" => solstone_core_cli_copy::render_for(
            locale,
            "journal.cli.journal.archive.fact.archive.output.ends.with.a.separator",
            &[],
        ),
        "archive output has no file name" => solstone_core_cli_copy::render_for(
            locale,
            "journal.cli.journal.archive.fact.archive.output.has.no.file.name",
            &[],
        ),
        "archive output has no parent" => solstone_core_cli_copy::render_for(
            locale,
            "journal.cli.journal.archive.fact.archive.output.has.no.parent",
            &[],
        ),
        "archive output must be one file name" => solstone_core_cli_copy::render_for(
            locale,
            "journal.cli.journal.archive.fact.archive.output.must.be.one.file.name",
            &[],
        ),
        "archive output parent is missing or unsafe" => solstone_core_cli_copy::render_for(
            locale,
            "journal.cli.journal.archive.fact.archive.output.parent.is.missing.or.unsafe",
            &[],
        ),
        "archive output path is not valid UTF-8" => solstone_core_cli_copy::render_for(
            locale,
            "journal.cli.journal.archive.fact.archive.output.path.is.not.valid.utf8",
            &[],
        ),
        "archive output uses an unsupported prefix" => solstone_core_cli_copy::render_for(
            locale,
            "journal.cli.journal.archive.fact.archive.output.uses.an.unsupported.prefix",
            &[],
        ),
        "archive parent component is invalid" => solstone_core_cli_copy::render_for(
            locale,
            "journal.cli.journal.archive.fact.archive.parent.component.is.invalid",
            &[],
        ),
        "block device" => solstone_core_cli_copy::render_for(
            locale,
            "journal.cli.journal.archive.fact.block.device",
            &[],
        ),
        "character device" => solstone_core_cli_copy::render_for(
            locale,
            "journal.cli.journal.archive.fact.character.device",
            &[],
        ),
        "could not determine writer position" => solstone_core_cli_copy::render_for(
            locale,
            "journal.cli.journal.archive.fact.could.not.determine.writer.position",
            &[],
        ),
        "could not read writer flags" => solstone_core_cli_copy::render_for(
            locale,
            "journal.cli.journal.archive.fact.could.not.read.writer.flags",
            &[],
        ),
        "could not stat writer" => solstone_core_cli_copy::render_for(
            locale,
            "journal.cli.journal.archive.fact.could.not.stat.writer",
            &[],
        ),
        "directory" => solstone_core_cli_copy::render_for(
            locale,
            "journal.cli.journal.archive.fact.directory",
            &[],
        ),
        "fifo" => {
            solstone_core_cli_copy::render_for(locale, "journal.cli.journal.archive.fact.fifo", &[])
        }
        "filesystem root is not a directory" => solstone_core_cli_copy::render_for(
            locale,
            "journal.cli.journal.archive.fact.filesystem.root.is.not.a.directory",
            &[],
        ),
        "has no parent directory" => solstone_core_cli_copy::render_for(
            locale,
            "journal.cli.journal.archive.fact.has.no.parent.directory",
            &[],
        ),
        "must be a regular file" => solstone_core_cli_copy::render_for(
            locale,
            "journal.cli.journal.archive.fact.must.be.a.regular.file",
            &[],
        ),
        "must be empty" => solstone_core_cli_copy::render_for(
            locale,
            "journal.cli.journal.archive.fact.must.be.empty",
            &[],
        ),
        "must be opened for writing" => solstone_core_cli_copy::render_for(
            locale,
            "journal.cli.journal.archive.fact.must.be.opened.for.writing",
            &[],
        ),
        "must be positioned at the start" => solstone_core_cli_copy::render_for(
            locale,
            "journal.cli.journal.archive.fact.must.be.positioned.at.the.start",
            &[],
        ),
        "must be valid UTF-8" => solstone_core_cli_copy::render_for(
            locale,
            "journal.cli.journal.archive.fact.must.be.valid.utf8",
            &[],
        ),
        "must name one normal file leaf" => solstone_core_cli_copy::render_for(
            locale,
            "journal.cli.journal.archive.fact.must.name.one.normal.file.leaf",
            &[],
        ),
        "must not be opened in append mode" => solstone_core_cli_copy::render_for(
            locale,
            "journal.cli.journal.archive.fact.must.not.be.opened.in.append.mode",
            &[],
        ),
        "must resolve to an absolute path" => solstone_core_cli_copy::render_for(
            locale,
            "journal.cli.journal.archive.fact.must.resolve.to.an.absolute.path",
            &[],
        ),
        "other" => solstone_core_cli_copy::render_for(
            locale,
            "journal.cli.journal.archive.fact.other",
            &[],
        ),
        "regular file" => solstone_core_cli_copy::render_for(
            locale,
            "journal.cli.journal.archive.fact.regular.file",
            &[],
        ),
        "relative archive output requires an absolute cwd" => solstone_core_cli_copy::render_for(
            locale,
            "journal.cli.journal.archive.fact.relative.archive.output.requires.an.absolute.cwd",
            &[],
        ),
        "socket" => solstone_core_cli_copy::render_for(
            locale,
            "journal.cli.journal.archive.fact.socket",
            &[],
        ),
        "symlink" => solstone_core_cli_copy::render_for(
            locale,
            "journal.cli.journal.archive.fact.symlink",
            &[],
        ),
        other => other.to_owned(),
    }
}

fn source_operation(
    locale: &str,
    operation: &'static str,
    member: Option<&str>,
    error: &std::io::Error,
) -> String {
    let detail = io_detail(locale, error);
    match (operation, member) {
        #[cfg(windows)]
        ("arm recursive namespace witness", Some(member)) => solstone_core_cli_copy::display_for(
            locale,
            "journal.cli.journal.archive.source.windows.arm.recursive.namespace.witness.member",
            [("member", &(member)), ("detail", &(detail))],
        ),
        #[cfg(windows)]
        ("arm recursive namespace witness", None) => solstone_core_cli_copy::display_for(
            locale,
            "journal.cli.journal.archive.source.windows.arm.recursive.namespace.witness",
            [("detail", &(detail))],
        ),
        #[cfg(windows)]
        ("borrow admitted journal root", Some(member)) => solstone_core_cli_copy::display_for(
            locale,
            "journal.cli.journal.archive.source.windows.borrow.admitted.journal.root.member",
            [("member", &(member)), ("detail", &(detail))],
        ),
        #[cfg(windows)]
        ("borrow admitted journal root", None) => solstone_core_cli_copy::display_for(
            locale,
            "journal.cli.journal.archive.source.windows.borrow.admitted.journal.root",
            [("detail", &(detail))],
        ),
        #[cfg(windows)]
        ("borrow admitted journal-root namespace watch", Some(member)) => {
            solstone_core_cli_copy::display_for(
                locale,
                "journal.cli.journal.archive.source.windows.borrow.admitted.journal.root.namespace.watch.member",
                [("member", &(member)), ("detail", &(detail))],
            )
        }
        #[cfg(windows)]
        ("borrow admitted journal-root namespace watch", None) => {
            solstone_core_cli_copy::display_for(
                locale,
                "journal.cli.journal.archive.source.windows.borrow.admitted.journal.root.namespace.watch",
                [("detail", &(detail))],
            )
        }
        #[cfg(windows)]
        ("cancel recursive namespace witness", Some(member)) => {
            solstone_core_cli_copy::display_for(
                locale,
                "journal.cli.journal.archive.source.windows.cancel.recursive.namespace.witness.member",
                [("member", &(member)), ("detail", &(detail))],
            )
        }
        #[cfg(windows)]
        ("cancel recursive namespace witness", None) => solstone_core_cli_copy::display_for(
            locale,
            "journal.cli.journal.archive.source.windows.cancel.recursive.namespace.witness",
            [("detail", &(detail))],
        ),
        #[cfg(windows)]
        ("check recursive namespace witness", Some(member)) => solstone_core_cli_copy::display_for(
            locale,
            "journal.cli.journal.archive.source.windows.check.recursive.namespace.witness.member",
            [("member", &(member)), ("detail", &(detail))],
        ),
        #[cfg(windows)]
        ("check recursive namespace witness", None) => solstone_core_cli_copy::display_for(
            locale,
            "journal.cli.journal.archive.source.windows.check.recursive.namespace.witness",
            [("detail", &(detail))],
        ),
        #[cfg(windows)]
        ("create recursive namespace witness event", Some(member)) => {
            solstone_core_cli_copy::display_for(
                locale,
                "journal.cli.journal.archive.source.windows.create.recursive.namespace.witness.event.member",
                [("member", &(member)), ("detail", &(detail))],
            )
        }
        #[cfg(windows)]
        ("create recursive namespace witness event", None) => solstone_core_cli_copy::display_for(
            locale,
            "journal.cli.journal.archive.source.windows.create.recursive.namespace.witness.event",
            [("detail", &(detail))],
        ),
        #[cfg(windows)]
        ("drain recursive namespace witness", Some(member)) => solstone_core_cli_copy::display_for(
            locale,
            "journal.cli.journal.archive.source.windows.drain.recursive.namespace.witness.member",
            [("member", &(member)), ("detail", &(detail))],
        ),
        #[cfg(windows)]
        ("drain recursive namespace witness", None) => solstone_core_cli_copy::display_for(
            locale,
            "journal.cli.journal.archive.source.windows.drain.recursive.namespace.witness",
            [("detail", &(detail))],
        ),
        #[cfg(windows)]
        ("list retained directory", Some(member)) => solstone_core_cli_copy::display_for(
            locale,
            "journal.cli.journal.archive.source.windows.list.retained.directory.member",
            [("member", &(member)), ("detail", &(detail))],
        ),
        #[cfg(windows)]
        ("list retained directory", None) => solstone_core_cli_copy::display_for(
            locale,
            "journal.cli.journal.archive.source.windows.list.retained.directory",
            [("detail", &(detail))],
        ),
        #[cfg(windows)]
        ("open retained relative entry", Some(member)) => solstone_core_cli_copy::display_for(
            locale,
            "journal.cli.journal.archive.source.windows.open.retained.relative.entry.member",
            [("member", &(member)), ("detail", &(detail))],
        ),
        #[cfg(windows)]
        ("open retained relative entry", None) => solstone_core_cli_copy::display_for(
            locale,
            "journal.cli.journal.archive.source.windows.open.retained.relative.entry",
            [("detail", &(detail))],
        ),
        #[cfg(windows)]
        ("parse retained directory listing", Some(member)) => solstone_core_cli_copy::display_for(
            locale,
            "journal.cli.journal.archive.source.windows.parse.retained.directory.listing.member",
            [("member", &(member)), ("detail", &(detail))],
        ),
        #[cfg(windows)]
        ("parse retained directory listing", None) => solstone_core_cli_copy::display_for(
            locale,
            "journal.cli.journal.archive.source.windows.parse.retained.directory.listing",
            [("detail", &(detail))],
        ),
        #[cfg(windows)]
        ("query retained entry attributes", Some(member)) => solstone_core_cli_copy::display_for(
            locale,
            "journal.cli.journal.archive.source.windows.query.retained.entry.attributes.member",
            [("member", &(member)), ("detail", &(detail))],
        ),
        #[cfg(windows)]
        ("query retained entry attributes", None) => solstone_core_cli_copy::display_for(
            locale,
            "journal.cli.journal.archive.source.windows.query.retained.entry.attributes",
            [("detail", &(detail))],
        ),
        #[cfg(windows)]
        ("query retained entry identity", Some(member)) => solstone_core_cli_copy::display_for(
            locale,
            "journal.cli.journal.archive.source.windows.query.retained.entry.identity.member",
            [("member", &(member)), ("detail", &(detail))],
        ),
        #[cfg(windows)]
        ("query retained entry identity", None) => solstone_core_cli_copy::display_for(
            locale,
            "journal.cli.journal.archive.source.windows.query.retained.entry.identity",
            [("detail", &(detail))],
        ),
        #[cfg(windows)]
        ("query retained entry metadata", Some(member)) => solstone_core_cli_copy::display_for(
            locale,
            "journal.cli.journal.archive.source.windows.query.retained.entry.metadata.member",
            [("member", &(member)), ("detail", &(detail))],
        ),
        #[cfg(windows)]
        ("query retained entry metadata", None) => solstone_core_cli_copy::display_for(
            locale,
            "journal.cli.journal.archive.source.windows.query.retained.entry.metadata",
            [("detail", &(detail))],
        ),
        #[cfg(windows)]
        ("query retained entry type", Some(member)) => solstone_core_cli_copy::display_for(
            locale,
            "journal.cli.journal.archive.source.windows.query.retained.entry.type.member",
            [("member", &(member)), ("detail", &(detail))],
        ),
        #[cfg(windows)]
        ("query retained entry type", None) => solstone_core_cli_copy::display_for(
            locale,
            "journal.cli.journal.archive.source.windows.query.retained.entry.type",
            [("detail", &(detail))],
        ),
        #[cfg(windows)]
        ("read observed file", Some(member)) => solstone_core_cli_copy::display_for(
            locale,
            "journal.cli.journal.archive.source.windows.read.observed.file.member",
            [("member", &(member)), ("detail", &(detail))],
        ),
        #[cfg(windows)]
        ("read observed file", None) => solstone_core_cli_copy::display_for(
            locale,
            "journal.cli.journal.archive.source.windows.read.observed.file",
            [("detail", &(detail))],
        ),
        #[cfg(windows)]
        ("reverify retained relative directory identity", Some(member)) => {
            solstone_core_cli_copy::display_for(
                locale,
                "journal.cli.journal.archive.source.windows.reverify.retained.relative.directory.identity.member",
                [("member", &(member)), ("detail", &(detail))],
            )
        }
        #[cfg(windows)]
        ("reverify retained relative directory identity", None) => {
            solstone_core_cli_copy::display_for(
                locale,
                "journal.cli.journal.archive.source.windows.reverify.retained.relative.directory.identity",
                [("detail", &(detail))],
            )
        }
        #[cfg(windows)]
        ("size checked-read buffer", Some(member)) => solstone_core_cli_copy::display_for(
            locale,
            "journal.cli.journal.archive.source.windows.size.checked.read.buffer.member",
            [("member", &(member)), ("detail", &(detail))],
        ),
        #[cfg(windows)]
        ("size checked-read buffer", None) => solstone_core_cli_copy::display_for(
            locale,
            "journal.cli.journal.archive.source.windows.size.checked.read.buffer",
            [("detail", &(detail))],
        ),
        #[cfg(windows)]
        ("trace retained relative directory listing open", Some(member)) => {
            solstone_core_cli_copy::display_for(
                locale,
                "journal.cli.journal.archive.source.windows.trace.retained.relative.directory.listing.open.member",
                [("member", &(member)), ("detail", &(detail))],
            )
        }
        #[cfg(windows)]
        ("trace retained relative directory listing open", None) => {
            solstone_core_cli_copy::display_for(
                locale,
                "journal.cli.journal.archive.source.windows.trace.retained.relative.directory.listing.open",
                [("detail", &(detail))],
            )
        }
        #[cfg(windows)]
        ("wait for recursive namespace witness", Some(member)) => {
            solstone_core_cli_copy::display_for(
                locale,
                "journal.cli.journal.archive.source.windows.wait.for.recursive.namespace.witness.member",
                [("member", &(member)), ("detail", &(detail))],
            )
        }
        #[cfg(windows)]
        ("wait for recursive namespace witness", None) => solstone_core_cli_copy::display_for(
            locale,
            "journal.cli.journal.archive.source.windows.wait.for.recursive.namespace.witness",
            [("detail", &(detail))],
        ),
        ("list journal directory", Some(member)) => solstone_core_cli_copy::display_for(
            locale,
            "journal.cli.journal.archive.source.io.list.journal.directory.member",
            [("member", &(member)), ("detail", &(detail))],
        ),
        ("list journal directory", None) => solstone_core_cli_copy::display_for(
            locale,
            "journal.cli.journal.archive.source.io.list.journal.directory",
            [("detail", &(detail))],
        ),
        ("open journal directory", Some(member)) => solstone_core_cli_copy::display_for(
            locale,
            "journal.cli.journal.archive.source.io.open.journal.directory.member",
            [("member", &(member)), ("detail", &(detail))],
        ),
        ("stat opened inventoried file", Some(member)) => solstone_core_cli_copy::display_for(
            locale,
            "journal.cli.journal.archive.source.io.stat.opened.inventoried.file.member",
            [("member", &(member)), ("detail", &(detail))],
        ),
        ("stat opened inventoried file", None) => solstone_core_cli_copy::display_for(
            locale,
            "journal.cli.journal.archive.source.io.stat.opened.inventoried.file",
            [("detail", &(detail))],
        ),
        ("open journal file", Some(member)) => solstone_core_cli_copy::display_for(
            locale,
            "journal.cli.journal.archive.source.io.open.journal.file.member",
            [("member", &(member)), ("detail", &(detail))],
        ),
        ("open journal file", None) => solstone_core_cli_copy::display_for(
            locale,
            "journal.cli.journal.archive.source.io.open.journal.file",
            [("detail", &(detail))],
        ),
        ("stat journal entry", Some(member)) => solstone_core_cli_copy::display_for(
            locale,
            "journal.cli.journal.archive.source.io.stat.journal.entry.member",
            [("member", &(member)), ("detail", &(detail))],
        ),
        ("stat journal entry", None) => solstone_core_cli_copy::display_for(
            locale,
            "journal.cli.journal.archive.source.io.stat.journal.entry",
            [("detail", &(detail))],
        ),
        ("stat opened journal file", Some(member)) => solstone_core_cli_copy::display_for(
            locale,
            "journal.cli.journal.archive.source.io.stat.opened.journal.file.member",
            [("member", &(member)), ("detail", &(detail))],
        ),
        ("stat opened journal file", None) => solstone_core_cli_copy::display_for(
            locale,
            "journal.cli.journal.archive.source.io.stat.opened.journal.file",
            [("detail", &(detail))],
        ),
        ("read regular-file size", Some(member)) => solstone_core_cli_copy::display_for(
            locale,
            "journal.cli.journal.archive.source.io.read.regular.file.size.member",
            [("member", &(member)), ("detail", &(detail))],
        ),
        ("read regular-file size", None) => solstone_core_cli_copy::display_for(
            locale,
            "journal.cli.journal.archive.source.io.read.regular.file.size",
            [("detail", &(detail))],
        ),
        ("stat inventoried directory", Some(member)) => solstone_core_cli_copy::display_for(
            locale,
            "journal.cli.journal.archive.source.io.stat.inventoried.directory.member",
            [("member", &(member)), ("detail", &(detail))],
        ),
        ("stat inventoried directory", None) => solstone_core_cli_copy::display_for(
            locale,
            "journal.cli.journal.archive.source.io.stat.inventoried.directory",
            [("detail", &(detail))],
        ),
        ("open journal directory for listing", Some(member)) => {
            solstone_core_cli_copy::display_for(
                locale,
                "journal.cli.journal.archive.source.io.open.journal.directory.for.listing.member",
                [("member", &(member)), ("detail", &(detail))],
            )
        }
        ("open journal directory for listing", None) => solstone_core_cli_copy::display_for(
            locale,
            "journal.cli.journal.archive.source.io.open.journal.directory.for.listing",
            [("detail", &(detail))],
        ),
        ("stat inventoried file", Some(member)) => solstone_core_cli_copy::display_for(
            locale,
            "journal.cli.journal.archive.source.io.stat.inventoried.file.member",
            [("member", &(member)), ("detail", &(detail))],
        ),
        ("stat inventoried file", None) => solstone_core_cli_copy::display_for(
            locale,
            "journal.cli.journal.archive.source.io.stat.inventoried.file",
            [("detail", &(detail))],
        ),
        ("stat opened inventoried directory", Some(member)) => solstone_core_cli_copy::display_for(
            locale,
            "journal.cli.journal.archive.source.io.stat.opened.inventoried.directory.member",
            [("member", &(member)), ("detail", &(detail))],
        ),
        ("stat opened inventoried directory", None) => solstone_core_cli_copy::display_for(
            locale,
            "journal.cli.journal.archive.source.io.stat.opened.inventoried.directory",
            [("detail", &(detail))],
        ),
        ("read source file identity", Some(member)) => solstone_core_cli_copy::display_for(
            locale,
            "journal.cli.journal.archive.source.io.read.source.file.identity.member",
            [("member", &(member)), ("detail", &(detail))],
        ),
        ("open retained journal root", Some(member)) => solstone_core_cli_copy::display_for(
            locale,
            "journal.cli.journal.archive.source.io.open.retained.journal.root.member",
            [("member", &(member)), ("detail", &(detail))],
        ),
        ("open retained journal root", None) => solstone_core_cli_copy::display_for(
            locale,
            "journal.cli.journal.archive.source.io.open.retained.journal.root",
            [("detail", &(detail))],
        ),
        ("stat opened journal directory", Some(member)) => solstone_core_cli_copy::display_for(
            locale,
            "journal.cli.journal.archive.source.io.stat.opened.journal.directory.member",
            [("member", &(member)), ("detail", &(detail))],
        ),
        ("stat opened journal directory", None) => solstone_core_cli_copy::display_for(
            locale,
            "journal.cli.journal.archive.source.io.stat.opened.journal.directory",
            [("detail", &(detail))],
        ),
        ("read inventoried file", Some(member)) => solstone_core_cli_copy::display_for(
            locale,
            "journal.cli.journal.archive.source.io.read.inventoried.file.member",
            [("member", &(member)), ("detail", &(detail))],
        ),
        ("read inventoried file", None) => solstone_core_cli_copy::display_for(
            locale,
            "journal.cli.journal.archive.source.io.read.inventoried.file",
            [("detail", &(detail))],
        ),
        (operation, None) => root_io_message(operation, error).render(locale),
        (operation, Some(member)) => solstone_core_cli_copy::display_for(
            locale,
            "journal.cli.journal.archive.source.io.member",
            [
                ("operation", &(operation)),
                ("member", &(member)),
                ("detail", &(detail)),
            ],
        ),
    }
}

pub(super) fn source(locale: &str, error: &ArchiveError) -> String {
    use ArchiveError as E;
    match error {
        E::InvalidJournal { root, reason } => root_reason(reason)
            .context_with(
                "journal.cli.transcribe.claim.root.invalid",
                [("path", &root.display())],
            )
            .render(locale),
        #[cfg(windows)]
        E::UnsupportedJournal {
            root,
            reason: "wait for recursive namespace witness",
        } => solstone_core_cli_copy::display_for(
            locale,
            "journal.cli.journal.archive.source.windows.unsupported.wait.for.recursive.namespace.witness",
            [("path", &(root.display()))],
        ),
        #[cfg(windows)]
        E::UnsupportedJournal {
            root,
            reason: "arm recursive namespace witness",
        } => solstone_core_cli_copy::display_for(
            locale,
            "journal.cli.journal.archive.source.windows.unsupported.arm.recursive.namespace.witness",
            [("path", &(root.display()))],
        ),
        #[cfg(windows)]
        E::UnsupportedJournal {
            root,
            reason: "cancel recursive namespace witness",
        } => solstone_core_cli_copy::display_for(
            locale,
            "journal.cli.journal.archive.source.windows.unsupported.cancel.recursive.namespace.witness",
            [("path", &(root.display()))],
        ),
        #[cfg(windows)]
        E::UnsupportedJournal {
            root,
            reason: "check recursive namespace witness",
        } => solstone_core_cli_copy::display_for(
            locale,
            "journal.cli.journal.archive.source.windows.unsupported.check.recursive.namespace.witness",
            [("path", &(root.display()))],
        ),
        #[cfg(windows)]
        E::UnsupportedJournal {
            root,
            reason: "drain recursive namespace witness",
        } => solstone_core_cli_copy::display_for(
            locale,
            "journal.cli.journal.archive.source.windows.unsupported.drain.recursive.namespace.witness",
            [("path", &(root.display()))],
        ),
        E::UnsupportedJournal {
            root,
            reason: "archive source exceeds its supported inventory budget",
        } => solstone_core_cli_copy::display_for(
            locale,
            "journal.cli.journal.archive.source.budget",
            [("path", &(root.display()))],
        ),
        E::UnsupportedJournal { root, reason } => root_reason(reason)
            .context_with(
                "journal.cli.transcribe.claim.root.unsupported",
                [("path", &root.display())],
            )
            .render(locale),
        E::UnsafeJournalEntry { member, kind } => solstone_core_cli_copy::display_for(
            locale,
            "journal.cli.journal.archive.source.unsafe",
            [
                ("member", &(member.as_str())),
                ("kind", &(format!("{kind:?}"))),
            ],
        ),
        E::SourceIo {
            operation,
            member,
            source,
        } => source_operation(
            locale,
            operation,
            member.as_ref().map(|name| name.as_str()),
            source,
        ),
        E::SourceChanged {
            member: Some(member),
        } => solstone_core_cli_copy::display_for(
            locale,
            "journal.cli.journal.archive.source.changed",
            [("member", &(member.as_str()))],
        ),
        E::SourceChanged { member: None } => solstone_core_cli_copy::render_for(
            locale,
            "journal.cli.transcribe.claim.root.changed",
            &[],
        ),
    }
}

pub(super) fn target(locale: &str, error: &ExplicitTargetError) -> String {
    use ExplicitTargetError as E;
    match error {
        E::InvalidTarget { path, reason } => solstone_core_cli_copy::display_for(
            locale,
            "journal.cli.journal.archive.target.invalid",
            [
                ("path", &(path.display())),
                ("reason", &(fixed_detail(locale, reason))),
            ],
        ),
        E::Collision { path } => solstone_core_cli_copy::display_for(
            locale,
            "journal.cli.journal.archive.target.collision",
            [("path", &(path.display()))],
        ),
        #[cfg(unix)]
        E::TargetChanged { path } => solstone_core_cli_copy::display_for(
            locale,
            "journal.cli.journal.archive.target.changed",
            [("path", &(path.display()))],
        ),
        #[cfg(windows)]
        E::TargetChanged { path } => solstone_core_cli_copy::display_for(
            locale,
            "journal.cli.journal.archive.target.windows.changed",
            [("path", &(path.display()))],
        ),
        #[cfg(windows)]
        E::TargetIo { path, source } => solstone_core_cli_copy::display_for(
            locale,
            "journal.cli.journal.archive.target.windows.io",
            [
                ("path", &(path.display())),
                ("detail", &(io_detail(locale, source))),
            ],
        ),
        #[cfg(windows)]
        E::UnsafeTarget { path, reason } => solstone_core_cli_copy::display_for(
            locale,
            "journal.cli.journal.archive.target.windows.unsafe",
            [
                ("path", &(path.display())),
                ("reason", &(fixed_detail(locale, reason))),
            ],
        ),
        #[cfg(unix)]
        E::UnsafeTarget { path, kind } => solstone_core_cli_copy::display_for(
            locale,
            "journal.cli.journal.archive.target.unsafe",
            [
                ("kind", &(fixed_detail(locale, kind))),
                ("path", &(path.display())),
            ],
        ),
        #[cfg(unix)]
        E::TargetIo {
            operation: "open archive output filesystem root",
            path,
            source,
        } => solstone_core_cli_copy::display_for(
            locale,
            "journal.cli.journal.archive.target.io.open.archive.output.filesystem.root",
            [
                ("path", &(path.display())),
                ("detail", &(io_detail(locale, source))),
            ],
        ),
        #[cfg(unix)]
        E::TargetIo {
            operation: "open archive output parent",
            path,
            source,
        } => solstone_core_cli_copy::display_for(
            locale,
            "journal.cli.journal.archive.target.io.open.archive.output.parent",
            [
                ("path", &(path.display())),
                ("detail", &(io_detail(locale, source))),
            ],
        ),
        #[cfg(unix)]
        E::TargetIo {
            operation: "retain admitted archive output parent",
            path,
            source,
        } => solstone_core_cli_copy::display_for(
            locale,
            "journal.cli.journal.archive.target.io.retain.admitted.archive.output.parent",
            [
                ("path", &(path.display())),
                ("detail", &(io_detail(locale, source))),
            ],
        ),
        #[cfg(unix)]
        E::TargetIo {
            operation: "revalidate admitted archive output parent",
            path,
            source,
        } => solstone_core_cli_copy::display_for(
            locale,
            "journal.cli.journal.archive.target.io.revalidate.admitted.archive.output.parent",
            [
                ("path", &(path.display())),
                ("detail", &(io_detail(locale, source))),
            ],
        ),
        #[cfg(unix)]
        E::TargetIo {
            operation: "stat admitted archive output parent",
            path,
            source,
        } => solstone_core_cli_copy::display_for(
            locale,
            "journal.cli.journal.archive.target.io.stat.admitted.archive.output.parent",
            [
                ("path", &(path.display())),
                ("detail", &(io_detail(locale, source))),
            ],
        ),
        #[cfg(unix)]
        E::TargetIo {
            operation: "stat archive output filesystem root",
            path,
            source,
        } => solstone_core_cli_copy::display_for(
            locale,
            "journal.cli.journal.archive.target.io.stat.archive.output.filesystem.root",
            [
                ("path", &(path.display())),
                ("detail", &(io_detail(locale, source))),
            ],
        ),
        #[cfg(unix)]
        E::TargetIo {
            operation: "stat archive output final name",
            path,
            source,
        } => solstone_core_cli_copy::display_for(
            locale,
            "journal.cli.journal.archive.target.io.stat.archive.output.final.name",
            [
                ("path", &(path.display())),
                ("detail", &(io_detail(locale, source))),
            ],
        ),
        #[cfg(unix)]
        E::TargetIo {
            operation: "stat archive output parent",
            path,
            source,
        } => solstone_core_cli_copy::display_for(
            locale,
            "journal.cli.journal.archive.target.io.stat.archive.output.parent",
            [
                ("path", &(path.display())),
                ("detail", &(io_detail(locale, source))),
            ],
        ),
        #[cfg(unix)]
        E::TargetIo {
            operation: "stat opened archive output parent",
            path,
            source,
        } => solstone_core_cli_copy::display_for(
            locale,
            "journal.cli.journal.archive.target.io.stat.opened.archive.output.parent",
            [
                ("path", &(path.display())),
                ("detail", &(io_detail(locale, source))),
            ],
        ),
        #[cfg(unix)]
        E::TargetIo {
            operation: "stat retained archive output parent",
            path,
            source,
        } => solstone_core_cli_copy::display_for(
            locale,
            "journal.cli.journal.archive.target.io.stat.retained.archive.output.parent",
            [
                ("path", &(path.display())),
                ("detail", &(io_detail(locale, source))),
            ],
        ),
        #[cfg(unix)]
        E::TargetIo {
            operation,
            path,
            source,
        } => solstone_core_cli_copy::display_for(
            locale,
            "journal.cli.journal.archive.target.io",
            [
                ("operation", &(operation)),
                ("path", &(path.display())),
                ("detail", &(io_detail(locale, source))),
            ],
        ),
    }
}

fn encode(locale: &str, error: &EncodeArchiveError) -> String {
    use EncodeArchiveError as E;
    match error {
        E::InvalidWriter { reason } => solstone_core_cli_copy::display_for(
            locale,
            "journal.cli.journal.archive.encode.writer",
            [("reason", &(fixed_detail(locale, reason)))],
        ),
        E::InvalidMetadata {
            field,
            value,
            reason,
        } => solstone_core_cli_copy::display_for(
            locale,
            "journal.cli.journal.archive.encode.metadata",
            [
                ("field", &(field)),
                ("value", &(format!("{value:?}"))),
                ("reason", &(fixed_detail(locale, reason))),
            ],
        ),
        E::Source(error) => solstone_core_cli_copy::display_for(
            locale,
            "journal.cli.journal.archive.encode.source",
            [("detail", &(source(locale, error)))],
        ),
        E::ArchiveWrite {
            member: Some(member),
            source,
            ..
        } => solstone_core_cli_copy::display_for(
            locale,
            "journal.cli.journal.archive.encode.member",
            [
                ("member", &(member.as_str())),
                ("detail", &(io_detail(locale, source))),
            ],
        ),
        E::ArchiveWrite {
            member: None,
            source,
            ..
        } => solstone_core_cli_copy::display_for(
            locale,
            "journal.cli.journal.archive.encode.write",
            [("detail", &(io_detail(locale, source)))],
        ),
        E::ArchiveFinish {
            phase: solstone_core_journal_archive::EncodingPhase::Body,
            source,
            ..
        } => solstone_core_cli_copy::display_for(
            locale,
            "journal.cli.journal.archive.encode.finish.body",
            [("detail", &(io_detail(locale, source)))],
        ),
        E::ArchiveFinish {
            phase: solstone_core_journal_archive::EncodingPhase::Finalize,
            source,
            ..
        } => solstone_core_cli_copy::display_for(
            locale,
            "journal.cli.journal.archive.encode.finish.finalize",
            [("detail", &(io_detail(locale, source)))],
        ),
        E::Cancelled | E::Aborted(_) => error.to_string(),
    }
}

pub(super) fn publication(locale: &str, error: &ArchivePublicationError) -> String {
    use ArchivePublicationError as E;
    match error {
        E::Encode(error) => encode(locale, error),
        E::Target(error) => target(locale, error),
        E::CreateTemp(error) => solstone_core_cli_copy::display_for(
            locale,
            "journal.cli.journal.archive.publish.createtemp",
            [("detail", &(io_detail(locale, error)))],
        ),
        E::SyncTemp(error) => solstone_core_cli_copy::display_for(
            locale,
            "journal.cli.journal.archive.publish.synctemp",
            [("detail", &(io_detail(locale, error)))],
        ),
        E::Cleanup(error) => solstone_core_cli_copy::display_for(
            locale,
            "journal.cli.journal.archive.publish.cleanup",
            [("detail", &(io_detail(locale, error)))],
        ),
        #[cfg(unix)]
        E::SyncDirectory(error) => solstone_core_cli_copy::display_for(
            locale,
            "journal.cli.journal.archive.publish.syncdirectory",
            [("detail", &(io_detail(locale, error)))],
        ),
        #[cfg(unix)]
        E::Publish(error) => solstone_core_cli_copy::display_for(
            locale,
            "journal.cli.journal.archive.publish.publish",
            [("detail", &(io_detail(locale, error)))],
        ),
        #[cfg(windows)]
        E::Publish(error) => solstone_core_cli_copy::display_for(
            locale,
            "journal.cli.journal.archive.publish.publish",
            [(
                "detail",
                &(solstone_core_journal_io::cli_diagnostics::atomic_message(error).render(locale)),
            )],
        ),
    }
}
