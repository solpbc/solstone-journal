// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use solstone_core_cli_copy as copy;

use solstone_core_journal::{
    LAYOUT_BUNDLE_ANCHOR, LAYOUT_LAYOUT_ANCHOR, LAYOUT_TEMPLATE_ANCHOR,
    resolve_installation_root_from_executable_dir,
};
use std::{fs, path::Path};

use crate::vocabulary::ExecutionError;
use crate::{
    context::CheckContext,
    vocabulary::{Check, RunnerResult, Status, make_result},
};

// Covers filesystem allocation and transient installation bookkeeping beyond
// the measured replacement tree.
const INSTALL_TREE_HEADROOM_BYTES: u64 = 1024 * 1024 * 1024;
const NOT_LAYOUT_INSTALL_TREE_DETAIL: &str = "journal.cli.doctor.disk.space.layout.unsupported";
const PACKAGE_INSTALL_DETAIL: &str = "journal.cli.doctor.disk.space.package.install";
/// Where a distribution package (apt, dnf or zypper, from the repository or a
/// standalone file) puts `solstone`. Its prefix is the system's `/usr`, not a
/// tree this install owns, so there is no install of ours to measure there.
const PACKAGE_INSTALL_BIN_DIR: &str = "/usr/bin";

pub fn run(context: &CheckContext, check: Check) -> RunnerResult {
    // The space this check asks for is room to write the next version beside
    // the running one, which only a self-updating tree install does.
    if context.install_bin_dir == Path::new(PACKAGE_INSTALL_BIN_DIR) {
        return Ok(make_result(
            check,
            Status::Skip,
            copy::text(PACKAGE_INSTALL_DETAIL),
            None::<String>,
        ));
    }
    let Some(_resolved_root) =
        resolve_installation_root_from_executable_dir(&context.install_bin_dir)
    else {
        return Ok(make_result(
            check,
            Status::Skip,
            copy::display(
                "journal.cli.doctor.disk.space.root.unresolved",
                [("path", &context.install_bin_dir.display())],
            ),
            None::<String>,
        ));
    };
    let prefix = context
        .install_bin_dir
        .parent()
        .expect("installation bin directory has a prefix");
    let share = prefix.join("share");
    if ![
        share.join(LAYOUT_BUNDLE_ANCHOR),
        share.join(LAYOUT_LAYOUT_ANCHOR),
        share.join(LAYOUT_TEMPLATE_ANCHOR),
    ]
    .iter()
    .all(|path| fs::metadata(path).is_ok_and(|metadata| metadata.is_file()))
    {
        return Ok(make_result(
            check,
            Status::Skip,
            copy::text(NOT_LAYOUT_INSTALL_TREE_DETAIL),
            None::<String>,
        ));
    }
    let (tree_bytes, slack_bytes) = required_free_space_bytes(prefix)?;
    let free_bytes = match context.free_space_bytes_override {
        Some(bytes) => bytes,
        None => available_bytes(prefix)?,
    };
    let required_bytes = tree_bytes + slack_bytes;
    let free_gib = bytes_to_gib(free_bytes);
    let required_gib = bytes_to_gib(required_bytes);
    if free_bytes < required_bytes {
        return Ok(make_result(
            check,
            Status::Warn,
            copy::display(
                "journal.cli.doctor.disk.space.space.low",
                [
                    ("free_gib", &format!("{:.1}", free_gib)),
                    ("path", &prefix.display()),
                    ("required_gib", &format!("{:.1}", required_gib)),
                ],
            ),
            None::<String>,
        ));
    }
    Ok(make_result(
        check,
        Status::Ok,
        copy::display(
            "journal.cli.doctor.disk.space.space.available",
            [
                ("free_gib", &format!("{:.1}", free_gib)),
                ("path", &prefix.display()),
                ("required_gib", &format!("{:.1}", required_gib)),
            ],
        ),
        None::<String>,
    ))
}

/// Bytes this account may still write on the filesystem holding `prefix`.
#[cfg(unix)]
fn available_bytes(prefix: &Path) -> Result<u64, ExecutionError> {
    let stats = nix::sys::statvfs::statvfs(prefix).map_err(|error| {
        ExecutionError::localized(
            "StatvfsError",
            copy::Diagnostic::new(
                "journal.cli.doctor.disk.space.space.inspect",
                [("path", &prefix.display()), ("error", &error)],
            ),
        )
    })?;
    Ok(stats.blocks_available() as u64 * stats.fragment_size() as u64)
}

/// Bytes this account may still write on the volume holding `prefix` -- the
/// caller's quota-aware figure, the same one the journal's own writers check.
#[cfg(windows)]
fn available_bytes(prefix: &Path) -> Result<u64, ExecutionError> {
    solstone_core_journal_io::windows_available_disk_bytes(prefix).map_err(|error| {
        ExecutionError::localized(
            "DiskFreeSpaceError",
            copy::Diagnostic::new(
                "journal.cli.doctor.disk.space.space.inspect",
                [("path", &prefix.display()), ("error", &error)],
            ),
        )
    })
}

#[cfg(not(any(unix, windows)))]
fn available_bytes(prefix: &Path) -> Result<u64, ExecutionError> {
    Err(ExecutionError::localized(
        "UnsupportedPlatform",
        copy::Diagnostic::new(
            "journal.cli.doctor.disk.space.space.unreadable",
            [("path", &prefix.display())],
        ),
    ))
}

pub(crate) fn required_free_space_bytes(prefix: &Path) -> Result<(u64, u64), ExecutionError> {
    let tree_bytes = ["bin", "lib", "share"]
        .into_iter()
        .try_fold(0_u64, |total, name| {
            let bytes = tree_bytes(&prefix.join(name))?;
            total.checked_add(bytes).ok_or_else(|| {
                ExecutionError::localized(
                    "InstallTreeWalkError",
                    copy::Diagnostic::new(
                        "journal.cli.doctor.disk.space.size.overflow",
                        [("path", &prefix.display())],
                    ),
                )
            })
        })?;
    Ok((tree_bytes, INSTALL_TREE_HEADROOM_BYTES))
}

fn tree_bytes(path: &Path) -> Result<u64, ExecutionError> {
    let metadata = fs::symlink_metadata(path).map_err(|error| {
        ExecutionError::localized(
            "InstallTreeWalkError",
            copy::Diagnostic::new(
                "journal.cli.doctor.disk.space.size.unreadable",
                [("path", &path.display()), ("error", &error)],
            ),
        )
    })?;
    if metadata.file_type().is_symlink() {
        return Ok(0);
    }
    if metadata.is_file() {
        return Ok(metadata.len());
    }
    if !metadata.is_dir() {
        return Ok(0);
    }
    fs::read_dir(path)
        .map_err(|error| {
            ExecutionError::localized(
                "InstallTreeWalkError",
                copy::Diagnostic::new(
                    "journal.cli.doctor.disk.space.size.unreadable",
                    [("path", &path.display()), ("error", &error)],
                ),
            )
        })?
        .try_fold(0_u64, |total, entry| {
            let entry = entry.map_err(|error| {
                ExecutionError::localized(
                    "InstallTreeWalkError",
                    copy::Diagnostic::new(
                        "journal.cli.doctor.disk.space.size.unreadable",
                        [("path", &path.display()), ("error", &error)],
                    ),
                )
            })?;
            let bytes = tree_bytes(&entry.path())?;
            total.checked_add(bytes).ok_or_else(|| {
                ExecutionError::localized(
                    "InstallTreeWalkError",
                    copy::Diagnostic::new(
                        "journal.cli.doctor.disk.space.size.overflow",
                        [("path", &path.display())],
                    ),
                )
            })
        })
}

fn bytes_to_gib(bytes: u64) -> f64 {
    bytes as f64 / 1024_f64.powi(3)
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use crate::{
        checks::test_support::{check, context, layout_install_root},
        vocabulary::{Severity, Status},
    };
    use std::fs;

    #[test]
    fn measures_all_distribution_directories_and_uses_the_free_space_override() {
        let mut staged = context();
        let prefix = layout_install_root(&staged);
        fs::create_dir_all(prefix.join("bin")).unwrap();
        fs::create_dir_all(prefix.join("lib")).unwrap();
        fs::write(prefix.join("bin/sol"), [1_u8; 3]).unwrap();
        fs::write(prefix.join("lib/runtime"), [2_u8; 11]).unwrap();
        fs::write(prefix.join("share/license"), [3_u8; 5]).unwrap();
        let (tree_bytes, slack_bytes) = required_free_space_bytes(&prefix).unwrap();
        assert_eq!(tree_bytes, 19);
        let check = check("disk_space", Severity::Advisory);
        staged.context.free_space_bytes_override = Some(tree_bytes + slack_bytes - 1);
        assert_eq!(run(&staged, check).unwrap().status, Status::Warn);
        staged.context.free_space_bytes_override = Some(tree_bytes + slack_bytes);
        assert_eq!(run(&staged, check).unwrap().status, Status::Ok);
    }

    #[test]
    fn skips_a_package_install_without_walking_the_system_prefix() {
        let mut staged = context();
        staged.context.install_bin_dir = std::path::PathBuf::from(PACKAGE_INSTALL_BIN_DIR);
        let check = check("disk_space", Severity::Advisory);
        let result = run(&staged, check).unwrap();
        assert_eq!(result.status, Status::Skip);
        assert_eq!(
            result.detail,
            "a package install is replaced in place when it updates, so no second copy is needed"
        );
        assert!(result.execution_error.is_none());
    }

    #[test]
    fn skips_a_checkout_shaped_context_without_measuring_its_tree() {
        let staged = context();
        let checkout = staged
            .install_bin_dir
            .parent()
            .expect("staged install bin has a prefix");
        fs::create_dir_all(checkout.join(".git")).unwrap();
        fs::write(checkout.join("pyproject.toml"), "").unwrap();
        // A checkout is recognised by its payload root, not by a `solstone`
        // directory. Staging the three layout anchors is what makes this a
        // checkout the resolver resolves -- and the root it returns is still
        // not a layout install tree, which is the skip under test.
        let payload = checkout.join(solstone_core_journal::CHECKOUT_PAYLOAD_ROOT);
        for anchor in [
            solstone_core_journal::LAYOUT_BUNDLE_ANCHOR,
            solstone_core_journal::LAYOUT_LAYOUT_ANCHOR,
            solstone_core_journal::LAYOUT_TEMPLATE_ANCHOR,
        ] {
            let path = payload.join(anchor);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(&path, anchor).unwrap();
        }
        let check = check("disk_space", Severity::Advisory);
        let result = run(&staged, check).unwrap();
        assert_eq!(result.status, Status::Skip);
        assert_eq!(
            result.detail,
            "resolved installation root is not a layout install tree"
        );
    }
}
