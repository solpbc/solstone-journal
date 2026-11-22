// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use solstone_core_cli_copy::MessageDetail;
use std::io;

pub fn io_message(error: &io::Error) -> Option<MessageDetail> {
    let cause = error.get_ref()?;
    if let Some(detail) = cause.downcast_ref::<MessageDetail>() {
        return Some(detail.clone());
    }
    if let Some(root) = cause.downcast_ref::<crate::JournalRootError>() {
        return Some(root_message(root));
    }
    if let Some(flat) = cause.downcast_ref::<crate::FlatDirectoryError>() {
        return Some(flat_message(flat));
    }
    if let Some(atomic) = cause.downcast_ref::<crate::AtomicWriteError>() {
        return Some(atomic_message(atomic));
    }
    #[cfg(windows)]
    if let Some(message) = windows::message(cause) {
        return Some(message);
    }
    None
}

fn source(error: &io::Error) -> MessageDetail {
    io_message(error).unwrap_or_else(|| error.to_string().into())
}

fn root_message(error: &crate::JournalRootError) -> MessageDetail {
    use crate::JournalRootError as Root;
    match error {
        Root::Invalid { root, reason, .. } => root_reason(reason).context_with(
            "journal.cli.transcribe.claim.root.invalid",
            [("path", &root.display())],
        ),
        Root::Unsupported { root, reason, .. } => root_reason(reason).context_with(
            "journal.cli.transcribe.claim.root.unsupported",
            [("path", &root.display())],
        ),
        Root::Io {
            operation,
            source: error,
            ..
        } => {
            let detail = source(error);
            let id = match *operation {
                "canonicalize journal root" => {
                    "journal.cli.describe.pipeline.root.canonicalize.journal.root"
                }
                "open filesystem root" => "journal.cli.describe.pipeline.root.open.filesystem.root",
                "open journal directory" => {
                    "journal.cli.describe.pipeline.root.open.journal.directory"
                }
                "open journal root" => "journal.cli.describe.pipeline.root.open.journal.root",
                "open journal root ancestor" => {
                    "journal.cli.describe.pipeline.root.open.journal.root.ancestor"
                }
                "open journal root namespace watch" => {
                    "journal.cli.describe.pipeline.root.open.journal.root.namespace.watch"
                }
                "open verified drive root" => {
                    "journal.cli.describe.pipeline.root.open.verified.drive.root"
                }
                "open verified journal root" => {
                    "journal.cli.describe.pipeline.root.open.verified.journal.root"
                }
                "query journal root ancestor attributes" => {
                    "journal.cli.describe.pipeline.root.query.journal.root.ancestor.attributes"
                }
                "query journal root attributes" => {
                    "journal.cli.describe.pipeline.root.query.journal.root.attributes"
                }
                "query journal root drive type" => {
                    "journal.cli.describe.pipeline.root.query.journal.root.drive.type"
                }
                "query journal root identity" => {
                    "journal.cli.describe.pipeline.root.query.journal.root.identity"
                }
                "query journal root namespace-watch attributes" => {
                    "journal.cli.describe.pipeline.root.query.journal.root.namespace.watch.attributes"
                }
                "query journal root namespace-watch identity" => {
                    "journal.cli.describe.pipeline.root.query.journal.root.namespace.watch.identity"
                }
                "query retained journal root attributes" => {
                    "journal.cli.describe.pipeline.root.query.retained.journal.root.attributes"
                }
                "query retained journal root identity" => {
                    "journal.cli.describe.pipeline.root.query.retained.journal.root.identity"
                }
                "query verified drive root attributes" => {
                    "journal.cli.describe.pipeline.root.query.verified.drive.root.attributes"
                }
                "query verified drive root identity" => {
                    "journal.cli.describe.pipeline.root.query.verified.drive.root.identity"
                }
                "query verified journal root attributes" => {
                    "journal.cli.describe.pipeline.root.query.verified.journal.root.attributes"
                }
                "query verified journal root identity" => {
                    "journal.cli.describe.pipeline.root.query.verified.journal.root.identity"
                }
                "read source file identity" => {
                    "journal.cli.describe.pipeline.root.read.source.file.identity"
                }
                "restat acquired journal root" => {
                    "journal.cli.describe.pipeline.root.restat.acquired.journal.root"
                }
                "stat acquired journal root" => {
                    "journal.cli.describe.pipeline.root.stat.acquired.journal.root"
                }
                "stat canonical root component" => {
                    "journal.cli.describe.pipeline.root.stat.canonical.root.component"
                }
                "stat opened canonical root component" => {
                    "journal.cli.describe.pipeline.root.stat.opened.canonical.root.component"
                }
                "stat opened filesystem root" => {
                    "journal.cli.describe.pipeline.root.stat.opened.filesystem.root"
                }
                _ => {
                    return detail.context_slot(
                        "journal.cli.transcribe.claim.root.io",
                        "reason",
                        [("operation", operation)],
                    );
                }
            };
            detail.context(id)
        }
        Root::Changed => MessageDetail::message("journal.cli.transcribe.claim.root.changed", []),
    }
}

// These are typed root-refusal facts from journal-io, never input or helper text.
fn root_reason(reason: &'static str) -> MessageDetail {
    match reason {
        "journal root does not exist" => {
            MessageDetail::message("journal.cli.transcribe.claim.root.absent", [])
        }
        "journal root is not a directory" => {
            MessageDetail::message("journal.cli.transcribe.claim.root.directory", [])
        }
        "journal root must be absolute" => {
            MessageDetail::message("journal.cli.transcribe.claim.root.absolute", [])
        }
        "canonical journal root has no final component" => {
            MessageDetail::message("journal.cli.transcribe.claim.root.component", [])
        }
        "canonical journal root has a non-UTF-8 ancestor" => {
            MessageDetail::message("journal.cli.transcribe.claim.root.ancestor", [])
        }
        "canonical journal root is not absolute" => {
            MessageDetail::message("journal.cli.transcribe.claim.root.canonical.absolute", [])
        }
        "Cloud Files sync-root status could not be verified" => MessageDetail::message(
            "journal.cli.describe.pipeline.root.cloud.files.sync.root.status.could.not.be.verified",
            [],
        ),
        "filesystem is not NTFS or ReFS" => MessageDetail::message(
            "journal.cli.describe.pipeline.root.filesystem.is.not.ntfs.or.refs",
            [],
        ),
        "filesystem type could not be verified" => MessageDetail::message(
            "journal.cli.describe.pipeline.root.filesystem.type.could.not.be.verified",
            [],
        ),
        "journal root contains a reparse point" => MessageDetail::message(
            "journal.cli.describe.pipeline.root.journal.root.contains.a.reparse.point",
            [],
        ),
        "journal root contains an interior NUL" => MessageDetail::message(
            "journal.cli.describe.pipeline.root.journal.root.contains.an.interior.nul",
            [],
        ),
        "journal root has an invalid portable name" => MessageDetail::message(
            "journal.cli.describe.pipeline.root.journal.root.has.an.invalid.portable.name",
            [],
        ),
        "journal root is a Cloud Files sync root" => MessageDetail::message(
            "journal.cli.describe.pipeline.root.journal.root.is.a.cloud.files.sync.root",
            [],
        ),
        "journal root is a reparse point" => MessageDetail::message(
            "journal.cli.describe.pipeline.root.journal.root.is.a.reparse.point",
            [],
        ),
        "journal root is on a RAM disk" => MessageDetail::message(
            "journal.cli.describe.pipeline.root.journal.root.is.on.a.ram.disk",
            [],
        ),
        "journal root is on a non-local volume" => MessageDetail::message(
            "journal.cli.describe.pipeline.root.journal.root.is.on.a.non.local.volume",
            [],
        ),
        "journal root is on removable or optical media" => MessageDetail::message(
            "journal.cli.describe.pipeline.root.journal.root.is.on.removable.or.optical.media",
            [],
        ),
        "journal root must be lexically canonical" => MessageDetail::message(
            "journal.cli.describe.pipeline.root.journal.root.must.be.lexically.canonical",
            [],
        ),
        "journal root must use a fully qualified drive path" => MessageDetail::message(
            "journal.cli.describe.pipeline.root.journal.root.must.use.a.fully.qualified.drive.path",
            [],
        ),
        "journal root volume type is unknown" => MessageDetail::message(
            "journal.cli.describe.pipeline.root.journal.root.volume.type.is.unknown",
            [],
        ),
        technical => technical.into(),
    }
}

fn name_reason(reason: crate::NameAdmissionReason) -> MessageDetail {
    use crate::NameAdmissionReason as Name;
    match reason {
        Name::Empty => {
            MessageDetail::message("journal.cli.describe.pipeline.windows.name.empty", [])
        }
        Name::DotComponent => MessageDetail::message(
            "journal.cli.describe.pipeline.windows.name.dot.component",
            [],
        ),
        Name::RootOrPrefix => MessageDetail::message(
            "journal.cli.describe.pipeline.windows.name.root.or.prefix",
            [],
        ),
        Name::Separator => {
            MessageDetail::message("journal.cli.describe.pipeline.windows.name.separator", [])
        }
        Name::Control => {
            MessageDetail::message("journal.cli.describe.pipeline.windows.name.control", [])
        }
        Name::AlternateDataStream => MessageDetail::message(
            "journal.cli.describe.pipeline.windows.name.alternate.data.stream",
            [],
        ),
        Name::ForbiddenCharacter => MessageDetail::message(
            "journal.cli.describe.pipeline.windows.name.forbidden.character",
            [],
        ),
        Name::TooLong => {
            MessageDetail::message("journal.cli.describe.pipeline.windows.name.too.long", [])
        }
        Name::ReservedDevice => MessageDetail::message(
            "journal.cli.describe.pipeline.windows.name.reserved.device",
            [],
        ),
        Name::TrailingDotOrSpace => MessageDetail::message(
            "journal.cli.describe.pipeline.windows.name.trailing.dot.or.space",
            [],
        ),
        Name::NotUtf8 => MessageDetail::message("journal.cli.describe.pipeline.flat.utf8", []),
        // Neither portable-component parser performs stream or claim grammar admission.
        other @ (Name::StreamGrammar | Name::ClaimGrammar) => other.to_string().into(),
    }
}

pub fn flat_message(error: &crate::FlatDirectoryError) -> MessageDetail {
    use crate::FlatDirectoryError as Dir;
    match error {
        Dir::InvalidRelativePath { path, reason } => {
            let detail = match *reason {
                "name must be one normal component" => {
                    MessageDetail::message("journal.cli.describe.pipeline.flat.normal", [])
                }
                "path must be a nonempty sequence of normal portable components" => {
                    MessageDetail::message("journal.cli.describe.pipeline.flat.relative", [])
                }
                other => other.into(),
            };
            detail.context_slot(
                "journal.cli.doctor.sync.path.invalid",
                "reason",
                [("path", &path.display())],
            )
        }
        Dir::InvalidName { name, reason } => name_reason(*reason).context_slot(
            "journal.cli.doctor.sync.entry.invalid",
            "reason",
            [("entry", &format!("{name:?}"))],
        ),
        Dir::NotDirectory { path } => MessageDetail::message(
            "journal.cli.doctor.sync.directory.required",
            [("path", &path.display())],
        ),
        Dir::SymlinkRefused { path } => MessageDetail::message(
            "journal.cli.doctor.sync.symlink",
            [("path", &path.display())],
        ),
        Dir::NotRegular { path } => MessageDetail::message(
            "journal.cli.doctor.sync.file.required",
            [("path", &path.display())],
        ),
        Dir::SizeLimitExceeded {
            path,
            kind,
            size,
            limit,
        } => MessageDetail::message(
            "journal.cli.doctor.sync.size",
            [
                ("path", &path.display()),
                ("kind", &format!("{kind:?}")),
                ("size", &size),
                ("limit", &limit),
            ],
        ),
        Dir::IdentityChanged { path } => MessageDetail::message(
            "journal.cli.doctor.sync.identity.changed",
            [("path", &path.display())],
        ),
        Dir::EnumerationChanged { path } => MessageDetail::message(
            "journal.cli.doctor.sync.entry.vanished",
            [("path", &path.display())],
        ),
        Dir::Io {
            operation,
            path,
            source: error,
        } => {
            let detail = source(error);
            let (id, slot) = match *operation {
                "open flat-directory descendant" => {
                    ("journal.cli.brain.check.open.directory", "reason")
                }
                "create flat-directory child" => (
                    "journal.cli.describe.pipeline.flat.create.flat.directory.child",
                    "detail",
                ),
                "stat flat-directory descendant" => (
                    "journal.cli.describe.pipeline.flat.stat.flat.directory.descendant",
                    "detail",
                ),
                "stat opened flat-directory descendant" => (
                    "journal.cli.describe.pipeline.flat.stat.opened.flat.directory.descendant",
                    "detail",
                ),
                "revalidate flat directory" => (
                    "journal.cli.describe.pipeline.flat.revalidate.flat.directory",
                    "detail",
                ),
                "read flat-directory identity" => (
                    "journal.cli.describe.pipeline.flat.read.flat.directory.identity",
                    "detail",
                ),
                _ => {
                    return detail.context_slot(
                        "journal.cli.doctor.sync.io",
                        "reason",
                        [("operation", operation), ("path", &path.display())],
                    );
                }
            };
            detail.context_slot(id, slot, [("path", &path.display())])
        }
    }
}

pub fn atomic_message(error: &crate::AtomicWriteError) -> MessageDetail {
    match error {
        crate::AtomicWriteError::Io {
            path,
            source: error,
        } => source(error).context_with(
            "journal.cli.describe.pipeline.publish.path",
            [("path", &path.display())],
        ),
        #[cfg(windows)]
        crate::AtomicWriteError::PublicationUncertain {
            path,
            operation,
            source: error,
        } => {
            let detail = source(error);
            let id = match *operation {
                "reconcile install after failed move" => {
                    "journal.cli.describe.pipeline.install.reconcile.install.after.failed.move"
                }
                "revalidate install paths after move" => {
                    "journal.cli.describe.pipeline.install.revalidate.install.paths.after.move"
                }
                "observe installed destination after move" => {
                    "journal.cli.describe.pipeline.install.observe.installed.destination.after.move"
                }
                _ => {
                    return detail.context_with(
                        "journal.cli.describe.pipeline.publish.uncertain",
                        [("path", &path.display()), ("operation", operation)],
                    );
                }
            };
            detail.context_with(id, [("path", &path.display())])
        }
    }
}

#[cfg(windows)]
mod windows {
    use super::*;
    use crate::windows_publication_path::{
        PublicationPathError as PathError, PublicationPathParseError as Parse,
    };
    pub(super) fn message(
        cause: &(dyn std::error::Error + Send + Sync + 'static),
    ) -> Option<MessageDetail> {
        if let Some(error) = cause.downcast_ref::<PathError>() {
            return Some(path_message(error));
        }
        if let Some(error) = cause.downcast_ref::<Parse>() {
            return Some(parse_message(error));
        }
        crate::errors::cleanup_message(cause)
            .or_else(|| crate::atomic::namespace_change_message(cause))
    }
    fn parse_message(error: &Parse) -> MessageDetail {
        match error {
            Parse::Empty => {
                MessageDetail::message("journal.cli.describe.pipeline.windows.path.empty", [])
            }
            Parse::RootOnly => {
                MessageDetail::message("journal.cli.describe.pipeline.windows.path.root.only", [])
            }
            Parse::DriveRelative => MessageDetail::message(
                "journal.cli.describe.pipeline.windows.path.drive.relative",
                [],
            ),
            Parse::RootedWithoutDrive => MessageDetail::message(
                "journal.cli.describe.pipeline.windows.path.rooted.without.drive",
                [],
            ),
            Parse::UncOrDevicePrefix => MessageDetail::message(
                "journal.cli.describe.pipeline.windows.path.unc.or.device.prefix",
                [],
            ),
            Parse::DotComponent => MessageDetail::message(
                "journal.cli.describe.pipeline.windows.path.dot.component",
                [],
            ),
            Parse::InteriorNul => MessageDetail::message(
                "journal.cli.describe.pipeline.windows.path.interior.nul",
                [],
            ),
            Parse::InvalidComponent(reason) => {
                name_reason(*reason).context("journal.cli.describe.pipeline.windows.path.component")
            }
        }
    }
    fn path_message(error: &PathError) -> MessageDetail {
        match error {
            PathError::Parse(e) => parse_message(e),
            PathError::UnstableNamespace { source: error } => {
                source(error).context("journal.cli.describe.pipeline.windows.path.unstable")
            }
            PathError::NotDirectory { component } => MessageDetail::message(
                "journal.cli.describe.pipeline.windows.path.not.directory",
                [("component", &(component))],
            ),
            PathError::ReparsePoint { component } => MessageDetail::message(
                "journal.cli.describe.pipeline.windows.path.reparse.point",
                [("component", &(component))],
            ),
            PathError::AlreadyExists { component } => MessageDetail::message(
                "journal.cli.describe.pipeline.windows.path.already.exists",
                [("component", &(component))],
            ),
            PathError::IdentityMismatch { component } => MessageDetail::message(
                "journal.cli.describe.pipeline.windows.path.identity.mismatch",
                [("component", &(component))],
            ),
            PathError::IdentityChanged { level } => MessageDetail::message(
                "journal.cli.describe.pipeline.windows.path.identity.changed",
                [("level", &(level))],
            ),
            PathError::Missing { component } => MessageDetail::message(
                "journal.cli.describe.pipeline.windows.path.missing",
                [("component", &(component))],
            ),
            PathError::NotRegularFile { component } => MessageDetail::message(
                "journal.cli.describe.pipeline.windows.path.not.regular.file",
                [("component", &(component))],
            ),
            PathError::PathTooLong => MessageDetail::message(
                "journal.cli.describe.pipeline.windows.path.path.too.long",
                [],
            ),
            PathError::MoveSpellingDiverged => MessageDetail::message(
                "journal.cli.describe.pipeline.windows.path.move.spelling.diverged",
                [],
            ),
            PathError::Io {
                operation,
                source: error,
            } => {
                let detail = source(error);
                let id = match *operation {
                    "classify install source" => {
                        "journal.cli.describe.pipeline.windows.path.classify.install.source"
                    }
                    "create publication-path ancestor" => {
                        "journal.cli.describe.pipeline.windows.path.create.publication.path.ancestor"
                    }
                    "open install source" => {
                        "journal.cli.describe.pipeline.windows.path.open.install.source"
                    }
                    "open publication-path ancestor" => {
                        "journal.cli.describe.pipeline.windows.path.open.publication.path.ancestor"
                    }
                    "open publication-path anchor" => {
                        "journal.cli.describe.pipeline.windows.path.open.publication.path.anchor"
                    }
                    "open publication-path move spelling" => {
                        "journal.cli.describe.pipeline.windows.path.open.publication.path.move.spelling"
                    }
                    "query install path attributes" => {
                        "journal.cli.describe.pipeline.windows.path.query.install.path.attributes"
                    }
                    "query install source identity" => {
                        "journal.cli.describe.pipeline.windows.path.query.install.source.identity"
                    }
                    "query publication-path attributes" => {
                        "journal.cli.describe.pipeline.windows.path.query.publication.path.attributes"
                    }
                    "query publication-path identity" => {
                        "journal.cli.describe.pipeline.windows.path.query.publication.path.identity"
                    }
                    "reopen publication-path ancestor" => {
                        "journal.cli.describe.pipeline.windows.path.reopen.publication.path.ancestor"
                    }
                    "resolve publication-path current directory" => {
                        "journal.cli.describe.pipeline.windows.path.resolve.publication.path.current.directory"
                    }
                    "revalidate install source after classification" => {
                        "journal.cli.describe.pipeline.windows.path.revalidate.install.source.after.classification"
                    }
                    _ => {
                        return detail.context_slot(
                            "journal.cli.transcribe.claim.root.io",
                            "reason",
                            [("operation", operation)],
                        );
                    }
                };
                detail.context(id)
            }
        }
    }
}
