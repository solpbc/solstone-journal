// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

#[cfg(unix)]
use std::fs;
use std::{
    env,
    ffi::OsString,
    path::{Path, PathBuf},
};

use crate::{
    checks::managed_wrapper::resolve_non_strict,
    context::CheckContext,
    vocabulary::{Check, Platform, RunnerResult, Status, make_result},
};

const MISSING_LOCAL_BIN_SOLSTONE_FIX: &str =
    "run journal setup to install the managed solstone wrapper";
const PATH_SOLSTONE_FIX: &str =
    "put ~/.local/bin earlier on PATH, or run journal setup to repoint the managed wrapper";

pub fn run(context: &CheckContext, check: Check) -> RunnerResult {
    // Python's shutil.which reads the process PATH; this check intentionally
    // does the same rather than adding a one-off field to CheckContext.
    run_with_path(context, check, env::var_os("PATH"))
}

fn run_with_path(context: &CheckContext, check: Check, path: Option<OsString>) -> RunnerResult {
    if context.platform == Platform::Windows {
        return Ok(make_result(
            check,
            Status::Skip,
            "not supported on windows",
            None::<String>,
        ));
    }
    let local = context.home_dir.join(".local/bin/solstone");
    let which = which_solstone(path.as_deref());
    if local.exists()
        && local.is_file()
        && let Some(which) = which.as_deref()
    {
        let local_resolved = resolve_non_strict(&local);
        let which_resolved = resolve_non_strict(which);
        if which != local && local.is_symlink() && local_resolved == which_resolved {
            return Ok(make_result(
                check,
                Status::Ok,
                format!(
                    "~/.local/bin/solstone symlinks to PATH solstone at {}",
                    which.display()
                ),
                None::<String>,
            ));
        }
        if which_resolved == local_resolved {
            return Ok(make_result(
                check,
                Status::Ok,
                format!("~/.local/bin/solstone is on PATH at {}", local.display()),
                None::<String>,
            ));
        }
    }

    let mut failures = Vec::new();
    let local_problem = if !local.exists() {
        failures.push(format!("{} is missing", local.display()));
        true
    } else if !local.is_file() {
        failures.push(format!("{} is not a file", local.display()));
        true
    } else {
        false
    };
    match which {
        None => failures.push("solstone is not on PATH".into()),
        Some(which) => failures.push(format!(
            "PATH solstone resolves to {}, expected {}",
            resolve_non_strict(&which).display(),
            resolve_non_strict(&local).display()
        )),
    }
    Ok(make_result(
        check,
        Status::Warn,
        failures.join("; "),
        Some(if local_problem {
            MISSING_LOCAL_BIN_SOLSTONE_FIX
        } else {
            PATH_SOLSTONE_FIX
        }),
    ))
}

fn which_solstone(path: Option<&std::ffi::OsStr>) -> Option<PathBuf> {
    env::split_paths(path?)
        .map(|directory| directory.join("solstone"))
        .find(|candidate| is_executable(candidate))
}

fn is_executable(path: &Path) -> bool {
    #[cfg(unix)]
    {
        fs::metadata(path).is_ok_and(|metadata| metadata.is_file())
            && nix::unistd::access(path, nix::unistd::AccessFlags::X_OK).is_ok()
    }
    #[cfg(not(unix))]
    {
        let _ = path;
        false
    }
}

#[cfg(all(test, unix))]
mod tests {
    use std::{
        fs,
        os::unix::fs::{PermissionsExt, symlink},
    };

    use super::*;
    use crate::{
        checks::test_support::{check, context},
        vocabulary::{Severity, Status},
    };

    fn executable(path: &Path) {
        fs::write(path, "#!/bin/sh\nexit 0\n").expect("write executable");
        fs::set_permissions(path, fs::Permissions::from_mode(0o755)).expect("make executable");
    }

    #[test]
    fn reports_path_symlinks_and_missing_local_aliases() {
        let staged = context();
        let bin = staged.home_dir.join("path-bin");
        fs::create_dir_all(&bin).expect("create PATH fixture");
        let target = bin.join("solstone");
        executable(&target);
        let local = staged.home_dir.join(".local/bin/solstone");
        fs::create_dir_all(local.parent().expect("local parent")).expect("create local parent");
        symlink(&target, &local).expect("link local solstone");
        let check = check("local_bin_solstone_reachable", Severity::Advisory);
        assert_eq!(
            run_with_path(&staged, check, Some(bin.clone().into()))
                .unwrap()
                .status,
            Status::Ok
        );

        fs::remove_file(&local).expect("remove local solstone");
        let result = run_with_path(&staged, check, Some(bin.into())).unwrap();
        assert_eq!(result.status, Status::Warn);
        assert!(result.detail.contains("is missing"));
        assert_eq!(result.fix.as_deref(), Some(MISSING_LOCAL_BIN_SOLSTONE_FIX));

        executable(&local);
        let result = run_with_path(&staged, check, None).unwrap();
        assert_eq!(result.status, Status::Warn);
        assert_eq!(result.fix.as_deref(), Some(PATH_SOLSTONE_FIX));
    }
}
