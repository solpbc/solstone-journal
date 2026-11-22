// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use crate::SegmentIdentityError;

pub fn path_detail(locale: &str, error: &crate::PathError) -> String {
    path_message(error).render(locale)
}

pub fn path_message(error: &crate::PathError) -> solstone_core_cli_copy::MessageDetail {
    use crate::{PathError, paths::RelativePathProblem};
    use solstone_core_cli_copy::MessageDetail;
    match error {
        PathError::InvalidRelativePath { rel, .. } => match RelativePathProblem::of(rel) {
            Some(RelativePathProblem::Empty) => {
                MessageDetail::message("journal.cli.talent.path.empty", [])
            }
            Some(RelativePathProblem::Absolute) => {
                MessageDetail::message("journal.cli.talent.path.absolute", [])
            }
            Some(RelativePathProblem::Backslash) => {
                MessageDetail::message("journal.cli.talent.path.backslash", [])
            }
            Some(RelativePathProblem::Component) => {
                MessageDetail::message("journal.cli.talent.path.component", [])
            }
            Some(RelativePathProblem::Unreadable) => {
                MessageDetail::message("journal.cli.talent.path.unreadable", [])
            }
            None => error.to_string().into(),
        },
        PathError::Escape(error) => MessageDetail::message(
            "journal.cli.talent.path.escape",
            [
                ("relative", &(format!("{:?}", error.rel))),
                ("path", &(error.path.display())),
            ],
        ),
        PathError::Io { .. } => error.to_string().into(),
    }
}

pub fn segment_identity(locale: &str, error: &SegmentIdentityError) -> String {
    match error {
        SegmentIdentityError::NotUtf8 { path } => solstone_core_cli_copy::display_for(
            locale,
            "journal.cli.backfill.identity.encoding",
            [("path", &path.display())],
        ),
        SegmentIdentityError::AmbiguousNamedDefault { path } => {
            solstone_core_cli_copy::display_for(
                locale,
                "journal.cli.backfill.identity.ambiguous",
                [("path", &path.display())],
            )
        }
        SegmentIdentityError::DuplicateKey { stream, key } => solstone_core_cli_copy::display_for(
            locale,
            "journal.cli.backfill.identity.duplicate",
            [
                ("stream", &(format!("{stream:?}"))),
                ("key", &(format!("{key:?}"))),
            ],
        ),
    }
}

pub fn read_detail(locale: &str, error: &crate::ReadError) -> String {
    read_message(error).render(locale)
}

pub fn read_message(error: &crate::ReadError) -> solstone_core_cli_copy::MessageDetail {
    use crate::ReadError;
    use solstone_core_cli_copy::MessageDetail;
    match error {
        ReadError::Malformed(error) => match error.line {
            Some(line) => MessageDetail::message(
                "journal.cli.talent.facet.malformed.line",
                [("path", &(error.path.display())), ("line", &(line))],
            ),
            None => MessageDetail::message(
                "journal.cli.talent.facet.malformed",
                [("path", &(error.path.display()))],
            ),
        },
        ReadError::Io { path, source } => io_message(source)
            .unwrap_or_else(|| source.to_string().into())
            .context_with(
                "journal.cli.describe.pipeline.publish.path",
                [("path", &path.display())],
            ),
    }
}

pub fn lock_message(error: &crate::LockError) -> solstone_core_cli_copy::MessageDetail {
    use solstone_core_cli_copy::MessageDetail;
    match error {
        crate::LockError::Io { .. } => error.to_string().into(),
        crate::LockError::Timeout(detail) => MessageDetail::message(
            "journal.cli.backup.config.lock.timeout",
            [
                ("path", &detail.path.display()),
                ("seconds", &detail.timeout.as_secs_f64()),
            ],
        ),
    }
}

pub fn lock_detail(locale: &str, error: &crate::LockError) -> String {
    lock_message(error).render(locale)
}

pub fn health_marker_detail(locale: &str, error: &crate::HealthMarkerError) -> String {
    use crate::HealthMarkerError as E;
    match error {
        E::Io { path, source } => solstone_core_cli_copy::display_for(
            locale,
            "journal.cli.think.health.marker.io",
            [("path", &(path.display())), ("error", &(source))],
        ),
        E::Lock(detail) => solstone_core_cli_copy::display_for(
            locale,
            "journal.cli.think.health.marker.lock",
            [("error", &(lock_detail(locale, detail)))],
        ),
        E::Atomic(detail) => solstone_core_cli_copy::display_for(
            locale,
            "journal.cli.think.health.marker.atomic",
            [("error", &(detail))],
        ),
        E::Malformed { path } => solstone_core_cli_copy::display_for(
            locale,
            "journal.cli.think.health.marker.malformed",
            [("path", &(path.display()))],
        ),
        E::Fingerprint { message } => solstone_core_cli_copy::display_for(
            locale,
            "journal.cli.think.health.marker.fingerprint",
            [("detail", &(message))],
        ),
    }
}

mod publication;
pub use publication::{atomic_message, flat_message, io_message, root_io_message, root_reason};
