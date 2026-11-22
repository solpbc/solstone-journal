// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use crate::SegmentIdentityError;

pub fn path_detail(locale: &str, error: &crate::PathError) -> String {
    use crate::{PathError, paths::RelativePathProblem};
    match error {
        PathError::InvalidRelativePath { rel, .. } => match RelativePathProblem::of(rel) {
            Some(RelativePathProblem::Empty) => {
                solstone_core_cli_copy::render_for(locale, "journal.cli.talent.path.empty", &[])
            }
            Some(RelativePathProblem::Absolute) => {
                solstone_core_cli_copy::render_for(locale, "journal.cli.talent.path.absolute", &[])
            }
            Some(RelativePathProblem::Backslash) => {
                solstone_core_cli_copy::render_for(locale, "journal.cli.talent.path.backslash", &[])
            }
            Some(RelativePathProblem::Component) => {
                solstone_core_cli_copy::render_for(locale, "journal.cli.talent.path.component", &[])
            }
            Some(RelativePathProblem::Unreadable) => solstone_core_cli_copy::render_for(
                locale,
                "journal.cli.talent.path.unreadable",
                &[],
            ),
            None => error.to_string(),
        },
        PathError::Escape(error) => solstone_core_cli_copy::display_for(
            locale,
            "journal.cli.talent.path.escape",
            [
                ("relative", &(format!("{:?}", error.rel))),
                ("path", &(error.path.display())),
            ],
        ),
        PathError::Io { .. } => error.to_string(),
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
    use crate::ReadError;
    match error {
        ReadError::Malformed(error) => match error.line {
            Some(line) => solstone_core_cli_copy::display_for(
                locale,
                "journal.cli.talent.facet.malformed.line",
                [("path", &(error.path.display())), ("line", &(line))],
            ),
            None => solstone_core_cli_copy::display_for(
                locale,
                "journal.cli.talent.facet.malformed",
                [("path", &(error.path.display()))],
            ),
        },
        ReadError::Io { .. } => error.to_string(),
    }
}

pub fn lock_detail(locale: &str, error: &crate::LockError) -> String {
    match error {
        crate::LockError::Io { .. } => error.to_string(),
        crate::LockError::Timeout(detail) => solstone_core_cli_copy::display_for(
            locale,
            "journal.cli.backup.config.lock.timeout",
            [
                ("path", &(detail.path.display())),
                ("seconds", &(detail.timeout.as_secs_f64())),
            ],
        ),
    }
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
