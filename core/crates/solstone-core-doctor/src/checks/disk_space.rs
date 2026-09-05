// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

#[cfg(unix)]
use nix::sys::statvfs::statvfs;
#[cfg(unix)]
use solstone_core_journal::{
    LAYOUT_BUNDLE_ANCHOR, LAYOUT_LAYOUT_ANCHOR, LAYOUT_TEMPLATE_ANCHOR,
    resolve_installation_root_from_executable_dir,
};
#[cfg(unix)]
use std::{fs, path::Path};

#[cfg(unix)]
use crate::vocabulary::ExecutionError;
use crate::{
    context::CheckContext,
    vocabulary::{Check, RunnerResult, Status, make_result},
};

// Covers filesystem allocation and transient installation bookkeeping beyond
// the measured replacement tree.
#[cfg(unix)]
const INSTALL_TREE_HEADROOM_BYTES: u64 = 1024 * 1024 * 1024;
#[cfg(unix)]
const NOT_LAYOUT_INSTALL_TREE_DETAIL: &str =
    "resolved installation root is not a layout install tree";

pub fn run(context: &CheckContext, check: Check) -> RunnerResult {
    #[cfg(unix)]
    {
        run_unix(context, check)
    }
    #[cfg(not(unix))]
    {
        let _ = context;
        Ok(make_result(
            check,
            Status::Skip,
            "not supported on windows",
            None::<String>,
        ))
    }
}

#[cfg(unix)]
fn run_unix(context: &CheckContext, check: Check) -> RunnerResult {
    let Some(_resolved_root) =
        resolve_installation_root_from_executable_dir(&context.install_bin_dir)
    else {
        return Ok(make_result(
            check,
            Status::Skip,
            format!(
                "could not resolve installation root from install bin directory: {}",
                context.install_bin_dir.display()
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
            NOT_LAYOUT_INSTALL_TREE_DETAIL,
            None::<String>,
        ));
    }
    let (tree_bytes, slack_bytes) = required_free_space_bytes(prefix)?;
    let stats = statvfs(prefix).map_err(|error| ExecutionError {
        kind: "StatvfsError".into(),
        message: format!(
            "could not inspect free space at {}: {error}",
            prefix.display()
        ),
    })?;
    let free_bytes = context
        .free_space_bytes_override
        .unwrap_or_else(|| stats.blocks_available() as u64 * stats.fragment_size() as u64);
    let required_bytes = tree_bytes + slack_bytes;
    let free_gib = bytes_to_gib(free_bytes);
    let required_gib = bytes_to_gib(required_bytes);
    if free_bytes < required_bytes {
        return Ok(make_result(
            check,
            Status::Warn,
            format!(
                "only {free_gib:.1} GiB free on the filesystem holding {}; {required_gib:.1} GiB is a second copy of this install plus headroom",
                prefix.display()
            ),
            None::<String>,
        ));
    }
    Ok(make_result(
        check,
        Status::Ok,
        format!(
            "{free_gib:.1} GiB free on the filesystem holding {} ({required_gib:.1} GiB is a second copy of this install plus headroom)",
            prefix.display()
        ),
        None::<String>,
    ))
}

#[cfg(unix)]
pub(crate) fn required_free_space_bytes(prefix: &Path) -> Result<(u64, u64), ExecutionError> {
    let tree_bytes = ["bin", "lib", "share"]
        .into_iter()
        .try_fold(0_u64, |total, name| {
            let bytes = tree_bytes(&prefix.join(name))?;
            total.checked_add(bytes).ok_or_else(|| ExecutionError {
                kind: "InstallTreeWalkError".into(),
                message: format!("installation tree size overflow at {}", prefix.display()),
            })
        })?;
    Ok((tree_bytes, INSTALL_TREE_HEADROOM_BYTES))
}

#[cfg(unix)]
fn tree_bytes(path: &Path) -> Result<u64, ExecutionError> {
    let metadata = fs::symlink_metadata(path).map_err(|error| ExecutionError {
        kind: "InstallTreeWalkError".into(),
        message: format!(
            "could not measure installation tree at {}: {error}",
            path.display()
        ),
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
        .map_err(|error| ExecutionError {
            kind: "InstallTreeWalkError".into(),
            message: format!(
                "could not measure installation tree at {}: {error}",
                path.display()
            ),
        })?
        .try_fold(0_u64, |total, entry| {
            let entry = entry.map_err(|error| ExecutionError {
                kind: "InstallTreeWalkError".into(),
                message: format!(
                    "could not measure installation tree at {}: {error}",
                    path.display()
                ),
            })?;
            let bytes = tree_bytes(&entry.path())?;
            total.checked_add(bytes).ok_or_else(|| ExecutionError {
                kind: "InstallTreeWalkError".into(),
                message: format!("installation tree size overflow at {}", path.display()),
            })
        })
}

#[cfg(unix)]
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
        assert_eq!(result.detail, NOT_LAYOUT_INSTALL_TREE_DETAIL);
    }
}
