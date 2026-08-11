// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::{
    fs,
    path::{Path, PathBuf},
    time::UNIX_EPOCH,
};

use nix::unistd::{AccessFlags, access};
use solstone_core_journal::resolve_installation_root_from_executable_dir;

use crate::{
    checks::managed_wrapper::{parse_sol_bin, resolve_non_strict},
    context::CheckContext,
    vocabulary::{Check, RunnerResult, Status, make_result},
};

const REPAIR: &str =
    "another sol/journal CLI is installed at ~/.local/bin/{binary}; run `journal setup` to repair";

#[derive(Clone, Copy, PartialEq, Eq)]
enum AliasState {
    Worktree,
    Absent,
    Owned,
    CrossRepo,
    Dangling,
    Foreign,
}

pub fn run(context: &CheckContext, check: Check, binary: &str) -> RunnerResult {
    let alias = context.home_dir.join(".local/bin").join(binary);
    if !alias.exists()
        && !alias.is_symlink()
        && let Some(backup) = latest_legacy_backup(binary)
    {
        return Ok(make_result(
            check,
            Status::Warn,
            partial_migration_detail(binary, &backup),
            Some("restore the backup or re-run from a fresh shell"),
        ));
    }
    let Some(root) = resolve_installation_root_from_executable_dir(&context.install_bin_dir) else {
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
    let (state, other) = classify_alias(context, &root, &alias, binary);
    match state {
        AliasState::Worktree => Ok(make_result(
            check,
            Status::Skip,
            "git worktree; run doctor from the primary clone",
            None::<String>,
        )),
        AliasState::Absent | AliasState::Owned => Ok(make_result(
            check,
            Status::Ok,
            format!("{binary} alias absent or owned by this repo"),
            None::<String>,
        )),
        AliasState::Foreign if is_app_owned_child_launcher(context, &alias, binary) => {
            Ok(make_result(
                check,
                Status::Ok,
                format!("~/.local/bin/{binary} is an app-owned child launcher for this runtime"),
                None::<String>,
            ))
        }
        _ => {
            if let Some(target) = other.as_deref()
                && let Some(tag) = recognized_legacy_target(&context.home_dir, target)
            {
                return Ok(make_result(
                    check,
                    Status::Warn,
                    format!(
                        "~/.local/bin/{binary} is a legacy {tag} install ({})",
                        target.display()
                    ),
                    Some(repair(binary)),
                ));
            }
            let detail = match state {
                AliasState::CrossRepo => format!(
                    "~/.local/bin/{binary} points at another repo ({})",
                    other
                        .as_deref()
                        .expect("cross-repo aliases carry a target")
                        .display()
                ),
                AliasState::Dangling => format!(
                    "~/.local/bin/{binary} is dangling ({})",
                    other
                        .as_deref()
                        .expect("dangling aliases carry a target")
                        .display()
                ),
                AliasState::Foreign => {
                    format!("~/.local/bin/{binary} exists but is not a symlink")
                }
                AliasState::Worktree | AliasState::Absent | AliasState::Owned => {
                    unreachable!("handled alias state")
                }
            };
            Ok(make_result(
                check,
                Status::Warn,
                detail,
                Some(repair(binary)),
            ))
        }
    }
}

fn classify_alias(
    context: &CheckContext,
    root: &Path,
    alias: &Path,
    binary: &str,
) -> (AliasState, Option<PathBuf>) {
    if root.join(".git").is_file() {
        return (AliasState::Worktree, None);
    }
    if !alias.exists() && !alias.is_symlink() {
        return (AliasState::Absent, None);
    }
    let checkout_target = resolve_non_strict(&root.join(".venv/bin").join(binary));
    let packaged_target = resolve_non_strict(&context.install_bin_dir.join(binary));
    if alias.is_symlink() {
        let Ok(raw_target) = fs::read_link(alias) else {
            return (AliasState::Foreign, None);
        };
        let target = if raw_target.is_absolute() {
            raw_target
        } else {
            alias
                .parent()
                .unwrap_or_else(|| Path::new("."))
                .join(raw_target)
        };
        let target = resolve_non_strict(&target);
        if !target.exists() {
            return (AliasState::Dangling, Some(target));
        }
        if target == checkout_target || target == packaged_target {
            return (AliasState::Owned, Some(target));
        }
        return (AliasState::CrossRepo, Some(target));
    }
    let Ok(content) = fs::read_to_string(alias) else {
        return (AliasState::Foreign, None);
    };
    let Some(sol_bin) = parse_sol_bin(&content) else {
        return (AliasState::Foreign, None);
    };
    let target = resolve_non_strict(&sol_bin);
    if target == checkout_target || target == packaged_target {
        (AliasState::Owned, Some(target))
    } else {
        (AliasState::Foreign, None)
    }
}

fn is_app_owned_child_launcher(context: &CheckContext, alias: &Path, binary: &str) -> bool {
    if alias.is_symlink() || !alias.is_file() {
        return false;
    }
    let expected = context.install_bin_dir.join(binary);
    let template = format!(
        "#!/bin/sh\n# managed-version: app-owned-child\nexec '{}' \"$@\"\n",
        expected.display().to_string().replace('\'', "'\\''")
    );
    fs::read_to_string(alias).is_ok_and(|content| content == template)
        && expected.exists()
        && access(&expected, AccessFlags::X_OK).is_ok()
}

fn recognized_legacy_target(home: &Path, target: &Path) -> Option<&'static str> {
    let target = resolve_non_strict(target);
    [
        (home.join(".local/share/uv/tools/solstone"), "uv-tool"),
        (home.join(".local/share/pipx/venvs/solstone"), "pipx-xdg"),
        (home.join(".local/pipx/venvs/solstone"), "pipx-legacy"),
    ]
    .into_iter()
    .find_map(|(prefix, tag)| {
        target
            .strip_prefix(resolve_non_strict(&prefix))
            .is_ok()
            .then_some(tag)
    })
}

fn latest_legacy_backup(binary: &str) -> Option<PathBuf> {
    let prefix = format!("{binary}.old-symlink-");
    fs::read_dir("/tmp")
        .ok()?
        .flatten()
        .filter(|entry| entry.file_name().to_string_lossy().starts_with(&prefix))
        .max_by_key(|entry| {
            entry
                .path()
                .symlink_metadata()
                .ok()
                .and_then(|metadata| metadata.modified().ok())
                .and_then(|modified| modified.duration_since(UNIX_EPOCH).ok())
                .unwrap_or_default()
        })
        .map(|entry| entry.path())
}

fn partial_migration_detail(binary: &str, backup: &Path) -> String {
    format!(
        "partial migration detected; backup at {} — restore with `mv {} ~/.local/bin/{binary}` or re-run from a fresh shell",
        backup.display(),
        backup.display(),
    )
}

fn repair(binary: &str) -> String {
    REPAIR.replace("{binary}", binary)
}

#[cfg(test)]
mod tests {
    use std::{
        fs,
        os::unix::fs::{PermissionsExt, symlink},
    };

    use super::*;
    use crate::{
        checks::test_support::{check, context},
        registry::{self, Battery},
        vocabulary::{Severity, Status},
    };

    struct Backup(PathBuf);

    impl Drop for Backup {
        fn drop(&mut self) {
            let _ = fs::remove_file(&self.0);
        }
    }

    fn executable(path: &Path) {
        fs::write(path, "#!/bin/sh\nexit 0\n").expect("write executable");
        fs::set_permissions(path, fs::Permissions::from_mode(0o755)).expect("make executable");
    }

    fn checkout(staged: &mut crate::checks::test_support::StagedContext) {
        let root = staged
            .install_bin_dir
            .parent()
            .and_then(Path::parent)
            .expect("staged root")
            .to_path_buf();
        fs::create_dir_all(root.join("solstone")).expect("create checkout package");
        fs::create_dir(root.join(".git")).expect("create primary checkout marker");
        fs::write(root.join("pyproject.toml"), "").expect("write pyproject marker");
        let install_bin = root.join(".venv/bin");
        fs::create_dir_all(&install_bin).expect("create checkout bin");
        staged.context.install_bin_dir = install_bin;
    }

    #[test]
    fn bindings_classify_owned_and_foreign_aliases_independently() {
        let mut staged = context();
        checkout(&mut staged);
        let sol = staged.install_bin_dir.join("sol");
        let journal = staged.install_bin_dir.join("journal");
        executable(&sol);
        executable(&journal);
        let aliases = staged.home_dir.join(".local/bin");
        fs::create_dir_all(&aliases).expect("create aliases");
        symlink(&sol, aliases.join("sol")).expect("link owned sol");
        fs::write(aliases.join("journal"), "foreign launcher").expect("write foreign journal");

        let readiness = registry::lookup(Battery::JournalReadiness, "stale_alias_symlink")
            .expect("readiness alias entry");
        let journal_entry =
            registry::lookup(Battery::Journal, "stale_alias_symlink").expect("journal alias entry");
        assert_eq!((readiness.runner)(&staged).unwrap().status, Status::Ok);
        assert_eq!(
            (journal_entry.runner)(&staged).unwrap().status,
            Status::Warn
        );
        fs::remove_file(aliases.join("sol")).expect("remove sol-only fixture");
        assert_eq!(
            (journal_entry.runner)(&staged).unwrap().status,
            Status::Warn,
            "journal must not inherit sol's owned fixture"
        );
    }

    #[test]
    fn foreign_managed_wrapper_under_a_legacy_prefix_is_not_a_legacy_alias() {
        let mut staged = context();
        checkout(&mut staged);
        let legacy_target = staged
            .home_dir
            .join(".local/share/uv/tools/solstone/bin/journal");
        fs::create_dir_all(legacy_target.parent().expect("legacy target parent"))
            .expect("create legacy target parent");
        executable(&legacy_target);
        let alias = staged.home_dir.join(".local/bin/journal");
        fs::create_dir_all(alias.parent().expect("alias parent")).expect("create alias parent");
        fs::write(
            &alias,
            format!(
                "# managed-version: 7\nSOL_BIN='{}'\n",
                legacy_target.display()
            ),
        )
        .expect("write foreign managed wrapper");

        let result = run(
            &staged,
            check("stale_alias_symlink", Severity::Blocker),
            "journal",
        )
        .unwrap();
        assert_eq!(result.status, Status::Warn);
        assert_eq!(
            result.detail,
            "~/.local/bin/journal exists but is not a symlink"
        );
        assert!(!result.detail.contains("legacy uv-tool install"));
    }

    #[test]
    fn reports_real_tmp_partial_migration_backups_and_cleans_them_up() {
        let staged = context();
        let path = PathBuf::from("/tmp").join(format!(
            "sol.old-symlink-doctor-test-{}",
            std::process::id()
        ));
        fs::write(&path, "backup").expect("write temporary migration backup");
        let backup = Backup(path.clone());
        let result = run(
            &staged,
            crate::vocabulary::Check {
                name: "stale_alias_symlink",
                severity: crate::vocabulary::Severity::Blocker,
                platforms: &[crate::vocabulary::Platform::Linux],
            },
            "sol",
        )
        .unwrap();
        assert_eq!(result.status, Status::Warn);
        assert!(result.detail.contains(&path.display().to_string()));
        drop(backup);
        assert!(!path.exists());
    }
}
