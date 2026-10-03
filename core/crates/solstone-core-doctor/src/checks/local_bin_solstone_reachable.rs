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
    "run solstone journal setup to install the managed solstone wrapper";
const PATH_SOLSTONE_FIX: &str = "put ~/.local/bin earlier on PATH, or run solstone journal setup to repoint the managed wrapper";

pub fn run(context: &CheckContext, check: Check) -> RunnerResult {
    // Python's shutil.which reads the process PATH; this check intentionally
    // does the same rather than adding a one-off field to CheckContext.
    run_with_path(context, check, env::var_os("PATH"))
}

fn run_with_path(context: &CheckContext, check: Check, path: Option<OsString>) -> RunnerResult {
    if context.platform == Platform::Windows {
        return windows(
            context,
            check,
            path.as_deref(),
            env::var_os("PATHEXT").as_deref(),
        );
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

const WINDOWS_NOT_ON_PATH_FIX: &str = "open a new terminal window and try again; if solstone is still not found, reinstall the journal";
const WINDOWS_MISSING_FIX: &str = "reinstall the journal to restore solstone.exe";

/// Windows has no managed wrapper: the installer puts the package's own `bin`
/// on the owner's `Path`, so the same question -- does `solstone` on PATH
/// reach this install? -- asks whether the first `solstone` the shell would
/// run is this install's `solstone.exe`.
fn windows(
    context: &CheckContext,
    check: Check,
    path: Option<&std::ffi::OsStr>,
    pathext: Option<&std::ffi::OsStr>,
) -> RunnerResult {
    let expected = context.install_bin_dir.join("solstone.exe");
    if !expected.is_file() {
        return Ok(make_result(
            check,
            Status::Warn,
            format!("{} is missing", expected.display()),
            Some(WINDOWS_MISSING_FIX),
        ));
    }
    let Some(found) = which_windows(path, pathext) else {
        return Ok(make_result(
            check,
            Status::Warn,
            "solstone is not on PATH",
            Some(WINDOWS_NOT_ON_PATH_FIX),
        ));
    };
    let expected_resolved = resolve_non_strict(&expected);
    let found_resolved = resolve_non_strict(&found);
    if found_resolved == expected_resolved {
        return Ok(make_result(
            check,
            Status::Ok,
            format!("solstone is on PATH at {}", expected.display()),
            None::<String>,
        ));
    }
    Ok(make_result(
        check,
        Status::Warn,
        format!(
            "PATH solstone resolves to {}, expected {}",
            found_resolved.display(),
            expected_resolved.display()
        ),
        Some(format!(
            "put {} ahead of {} on your PATH",
            context.install_bin_dir.display(),
            found.parent().unwrap_or(&found).display()
        )),
    ))
}

#[cfg(test)]
pub(crate) fn windows_for_test(
    context: &CheckContext,
    check: Check,
    path: &std::ffi::OsStr,
    pathext: &std::ffi::OsStr,
) -> crate::vocabulary::CheckResult {
    windows(context, check, Some(path), Some(pathext)).expect("the Windows arm never errors")
}

/// The first `solstone` the Windows shell would run: each PATH directory in
/// order, trying each PATHEXT extension in order within it.
fn which_windows(
    path: Option<&std::ffi::OsStr>,
    pathext: Option<&std::ffi::OsStr>,
) -> Option<PathBuf> {
    let extensions = pathext
        .and_then(std::ffi::OsStr::to_str)
        .filter(|value| !value.trim().is_empty())
        .unwrap_or(".COM;.EXE;.BAT;.CMD")
        .split(';')
        .map(str::trim)
        .filter(|extension| !extension.is_empty())
        // Windows names are case-insensitive, so the lowercase spelling finds
        // the same file there and a test fixture finds it everywhere.
        .map(str::to_ascii_lowercase)
        .collect::<Vec<_>>();
    env::split_paths(path?).find_map(|directory| {
        extensions
            .iter()
            .map(|extension| directory.join(format!("solstone{extension}")))
            .find(|candidate| candidate.is_file())
    })
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
