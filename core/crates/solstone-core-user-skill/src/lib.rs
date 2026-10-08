// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UserSkillMode {
    Install,
    Refresh,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UserSkillSync {
    Installed,
    Replaced,
    Unchanged,
    Ineligible, // refresh only: not our skill; no warn
    Preserved,  // overlap or unknown evidence; caller warns
    Failed(String),
}

static COUNTER: AtomicU64 = AtomicU64::new(0);

#[cfg(feature = "test-hooks")]
type CopyFault = fn() -> io::Result<()>;

#[cfg(feature = "test-hooks")]
thread_local! {
    static COPY_FAULT: std::cell::RefCell<Option<CopyFault>> = const { std::cell::RefCell::new(None) };
}

#[cfg(feature = "test-hooks")]
pub fn set_user_skill_copy_fault(fault: Option<fn() -> io::Result<()>>) {
    COPY_FAULT.with(|cell| {
        *cell.borrow_mut() = fault;
    });
}

#[cfg(feature = "test-hooks")]
fn trigger_copy_fault() -> io::Result<()> {
    COPY_FAULT.with(|cell| {
        if let Some(fault) = *cell.borrow() {
            fault()
        } else {
            Ok(())
        }
    })
}

#[cfg(not(feature = "test-hooks"))]
fn trigger_copy_fault() -> io::Result<()> {
    Ok(())
}

pub struct UserSkillGuard {
    protected_journals: solstone_core_installation_identity::ProtectedJournals,
    platform: solstone_core_installation_identity::PlatformTag,
}

/// Locate the bundled solstone talent directory.
pub fn bundled_user_skill_dir() -> io::Result<PathBuf> {
    let exe = std::env::current_exe()?;
    let exe_dir = exe.parent().ok_or_else(|| {
        io::Error::new(io::ErrorKind::NotFound, "executable directory unavailable")
    })?;
    let root = solstone_core_journal::resolve_installation_root_from_executable_dir(exe_dir)
        .ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::NotFound,
                "installation root unavailable from executable directory",
            )
        })?;
    let skill_dir = root.join("solstone/talent/solstone");
    let skill_file = skill_dir.join("SKILL.md");
    if !skill_file.is_file() {
        return Err(io::Error::new(
            io::ErrorKind::NotFound,
            format!("bundled skill file not found at {}", skill_file.display()),
        ));
    }
    Ok(skill_dir)
}

/// Determine the selected journal path for user skill installation.
pub fn selected_journal_for_install(home: &Path) -> Result<PathBuf, String> {
    if !home.is_absolute() {
        return Err("home must be an absolute path".to_string());
    }
    let config_val = match solstone_core_journal::read_config_journal(home) {
        Ok(val) => val,
        Err(e) => return Err(format!("config journal decode error: {e:?}")),
    };
    let env_val = std::env::var_os("SOLSTONE_JOURNAL");
    let env_opt = env_val.as_deref().filter(|s| !s.is_empty());
    let resolved =
        solstone_core_journal::resolve_journal_path(env_opt, config_val.as_deref(), None, home);
    if !resolved.path.is_absolute() {
        return Err("resolved journal path is not absolute".to_string());
    }
    Ok(resolved.path)
}

/// Build a journal mutation guard ensuring no write touches any protected journal.
pub fn guard_user_skill_mutation(
    home: &Path,
    selected_journal: &Path,
) -> Result<UserSkillGuard, String> {
    if !home.is_absolute() {
        return Err("home must be an absolute path".to_string());
    }
    if !selected_journal.is_absolute() {
        return Err("selected journal must be an absolute path".to_string());
    }
    let owner = solstone_core_installation_identity::OwnerBase::at_home(
        home.to_path_buf(),
        solstone_core_installation_identity::PlatformTag::current(),
    )
    .map_err(|e| format!("cannot form owner base at {}: {e}", home.display()))?;

    let exe = std::env::current_exe().map_err(|e| format!("cannot determine current exe: {e}"))?;
    let exe_dir = exe
        .parent()
        .ok_or_else(|| "current exe has no parent directory".to_string())?;
    let id_root = solstone_core_journal::resolve_identity_root_from_executable_dir(exe_dir)
        .unwrap_or_else(|| exe_dir.to_path_buf());
    let root_token = solstone_core_installation_identity::root_token_from_path(&id_root)
        .map_err(|e| format!("cannot derive root token from {}: {e}", id_root.display()))?;

    let census = solstone_core_installation_identity::read_installation_journal_census(
        &owner,
        &root_token,
        false,
    )
    .map_err(|e| format!("census read failed: {e}"))?;

    if !census.registry_known {
        return Err("installation registry is unknown".to_string());
    }

    let mut protected_journals =
        solstone_core_installation_identity::ProtectedJournals::new(owner.platform());
    protected_journals.insert(selected_journal.to_path_buf());
    for record in &census.records {
        protected_journals.insert(record.journal_token.to_path_buf());
    }

    Ok(UserSkillGuard {
        protected_journals,
        platform: owner.platform(),
    })
}

impl UserSkillGuard {
    /// Verify whether a destination path is allowed to be mutated.
    pub fn allows(&self, path: &Path) -> Result<(), String> {
        if !path.is_absolute() {
            return Err("path must be absolute".to_string());
        }
        if self.protected_journals.overlaps(path) {
            return Err(format!(
                "path {} overlaps protected journal",
                path.display()
            ));
        }

        // Walk up with symlink_metadata until an existing component (a symlink counts)
        let mut cursor = path.to_path_buf();
        let mut suffix_components = Vec::new();
        while !cursor.as_os_str().is_empty() {
            match fs::symlink_metadata(&cursor) {
                Ok(_) => break,
                Err(e) if e.kind() == io::ErrorKind::NotFound => {
                    if let Some(file_name) = cursor.file_name() {
                        suffix_components.push(file_name.to_os_string());
                        cursor.pop();
                    } else {
                        break;
                    }
                }
                Err(e) => {
                    return Err(format!(
                        "inspecting ancestor {} failed: {e}",
                        cursor.display()
                    ));
                }
            }
        }
        if cursor.as_os_str().is_empty() {
            return Err(format!("no existing ancestor found for {}", path.display()));
        }
        let canonical_ancestor = fs::canonicalize(&cursor)
            .map_err(|e| format!("canonicalizing ancestor {} failed: {e}", cursor.display()))?;
        let mut resolved_path = canonical_ancestor;
        for comp in suffix_components.into_iter().rev() {
            resolved_path.push(comp);
        }

        if self.protected_journals.overlaps(&resolved_path) {
            return Err(format!(
                "resolved ancestor path {} overlaps protected journal",
                resolved_path.display()
            ));
        }
        Ok(())
    }

    /// Synchronize the user skill into the destination path.
    pub fn sync(&self, source: &Path, target: &Path, mode: UserSkillMode) -> UserSkillSync {
        if self.allows(target).is_err() {
            return UserSkillSync::Preserved;
        }
        match solstone_core_skill_state::user_skill_copy_matches(source, source) {
            Ok(true) => {}
            Ok(false) => {
                return UserSkillSync::Failed(
                    "bundled skill contains linked or special entries".to_string(),
                );
            }
            Err(e) => return UserSkillSync::Failed(e.to_string()),
        }

        let target_parent = match target.parent() {
            Some(p) => p,
            None => return UserSkillSync::Failed("target has no parent directory".to_string()),
        };

        let count = COUNTER.fetch_add(1, Ordering::Relaxed);
        let staging_name = format!(".solstone_staging_{}_{}.tmp", std::process::id(), count);
        let staging_dir = target_parent.join(staging_name);

        if self.allows(&staging_dir).is_err() {
            return UserSkillSync::Preserved;
        }

        let target_meta = fs::symlink_metadata(target);
        let target_exists = target_meta.is_ok();

        if mode == UserSkillMode::Refresh {
            match target_meta {
                Err(e) if e.kind() == io::ErrorKind::NotFound => {
                    return UserSkillSync::Ineligible;
                }
                Err(e) => return UserSkillSync::Failed(e.to_string()),
                Ok(meta) => {
                    let file_type = meta.file_type();
                    if file_type.is_symlink() {
                        let link_target = match fs::read_link(target) {
                            Ok(t) if t.is_absolute() => t,
                            Ok(t) => target_parent.join(t),
                            Err(e) => return UserSkillSync::Failed(e.to_string()),
                        };
                        let bundled = match bundled_user_skill_dir() {
                            Ok(b) => b,
                            Err(e) => return UserSkillSync::Failed(e.to_string()),
                        };
                        let is_windows = self.platform
                            == solstone_core_installation_identity::PlatformTag::Windows;
                        if !solstone_core_installation_identity::same_protected_place(
                            &link_target,
                            &bundled,
                            is_windows,
                        ) {
                            return UserSkillSync::Ineligible;
                        }
                    } else if file_type.is_dir() {
                        let skill_md = target.join("SKILL.md");
                        match fs::symlink_metadata(&skill_md) {
                            Ok(sm_meta) => {
                                if sm_meta.file_type().is_symlink()
                                    || !sm_meta.file_type().is_file()
                                {
                                    return UserSkillSync::Ineligible;
                                }
                            }
                            Err(e) if e.kind() == io::ErrorKind::NotFound => {
                                return UserSkillSync::Ineligible;
                            }
                            Err(e) => return UserSkillSync::Failed(e.to_string()),
                        }
                        match fs::read(&skill_md) {
                            Ok(bytes) => match check_skill_md_name(&bytes) {
                                SkillMdCheck::MatchesSolstone => {}
                                SkillMdCheck::Ineligible => return UserSkillSync::Ineligible,
                                SkillMdCheck::InvalidUtf8(err) => {
                                    return UserSkillSync::Failed(format!(
                                        "SKILL.md is not valid UTF-8: {err}"
                                    ));
                                }
                            },
                            Err(e) => return UserSkillSync::Failed(e.to_string()),
                        }
                        if let Err(e) =
                            solstone_core_skill_state::validate_user_skill_copy_entries(target)
                        {
                            return UserSkillSync::Failed(e.to_string());
                        }
                        match solstone_core_skill_state::user_skill_copy_matches(source, target) {
                            Ok(true) => return UserSkillSync::Unchanged,
                            Ok(false) => {}
                            Err(e) => return UserSkillSync::Failed(e.to_string()),
                        }
                    } else {
                        return UserSkillSync::Ineligible;
                    }
                }
            }
        } else {
            // Mode is Install
            let is_regular_dir = match &target_meta {
                Ok(meta) => meta.file_type().is_dir() && !meta.file_type().is_symlink(),
                Err(_) => false,
            };
            if is_regular_dir {
                if let Err(e) = solstone_core_skill_state::validate_user_skill_copy_entries(target)
                {
                    return UserSkillSync::Failed(e.to_string());
                }
                match solstone_core_skill_state::user_skill_copy_matches(source, target) {
                    Ok(true) => return UserSkillSync::Unchanged,
                    Ok(false) => {}
                    Err(e) => return UserSkillSync::Failed(e.to_string()),
                }
            }
            if !target_parent.exists() {
                match fs::create_dir(target_parent) {
                    Ok(()) => {}
                    Err(e) => return UserSkillSync::Failed(e.to_string()),
                }
            }
        }

        if let Err(e) = fs::create_dir(&staging_dir) {
            return UserSkillSync::Failed(e.to_string());
        }

        if let Err(e) = copy_tree_to_staging(source, &staging_dir) {
            let _ = fs::remove_dir_all(&staging_dir);
            return UserSkillSync::Failed(e.to_string());
        }

        let aside_name = format!(
            ".solstone_aside_{}_{}.tmp",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed)
        );
        let aside_dir = target_parent.join(aside_name);

        self.publish_staged_skill(&staging_dir, target, &aside_dir, target_exists)
    }

    fn publish_staged_skill(
        &self,
        staging_dir: &Path,
        target: &Path,
        aside_dir: &Path,
        target_exists: bool,
    ) -> UserSkillSync {
        if self.allows(target).is_err() || self.allows(staging_dir).is_err() {
            return UserSkillSync::Preserved;
        }
        if target_exists {
            // Reserve a guarded container exclusively. Renaming onto a predictable
            // sibling directory could otherwise replace somebody else's empty tree.
            if self.allows(aside_dir).is_err() {
                let _ = fs::remove_dir_all(staging_dir);
                return UserSkillSync::Preserved;
            }
            if let Err(e) = fs::create_dir(aside_dir) {
                let _ = fs::remove_dir_all(staging_dir);
                return UserSkillSync::Failed(e.to_string());
            }
            let previous = aside_dir.join("previous");
            if let Err(e) = fs::rename(target, &previous) {
                let _ = fs::remove_dir(aside_dir);
                let _ = fs::remove_dir_all(staging_dir);
                return UserSkillSync::Failed(e.to_string());
            }
            if let Err(e) = fs::rename(staging_dir, target) {
                let rollback = fs::rename(&previous, target);
                let _ = fs::remove_dir_all(staging_dir);
                return match rollback {
                    Ok(()) => {
                        let _ = fs::remove_dir(aside_dir);
                        UserSkillSync::Failed(e.to_string())
                    }
                    Err(rollback_error) => UserSkillSync::Failed(format!(
                        "publishing skill failed: {e}; restoring failed: {rollback_error}; backup path: {}",
                        previous.display()
                    )),
                };
            }
            // Publication committed the complete new tree. Cleanup can partially
            // remove the backup, so it must never roll that partial tree back over it.
            if let Err(e) = fs::remove_dir_all(aside_dir) {
                log::warn!(
                    "refresh user skills: installed {}; old skill cleanup at {} failed: {e}",
                    target.display(),
                    aside_dir.display()
                );
            }
            UserSkillSync::Replaced
        } else {
            if let Err(e) = fs::rename(staging_dir, target) {
                let _ = fs::remove_dir_all(staging_dir);
                return UserSkillSync::Failed(e.to_string());
            }
            UserSkillSync::Installed
        }
    }
}

/// Inspect and refresh installed user skills across .claude, .codex, and .gemini.
pub fn refresh_installed_user_skills(home: &Path, selected_journal: &Path) {
    let bundled = match bundled_user_skill_dir() {
        Ok(b) => b,
        Err(e) => {
            log::warn!("refresh user skills: bundled skill directory unavailable: {e}");
            return;
        }
    };
    let guard = match guard_user_skill_mutation(home, selected_journal) {
        Ok(g) => g,
        Err(e) => {
            log::warn!("refresh user skills: mutation guard unavailable: {e}");
            return;
        }
    };
    const AGENTS: [&str; 3] = [".claude", ".codex", ".gemini"];
    for agent_dir in AGENTS {
        let parent = home.join(agent_dir);
        match fs::symlink_metadata(&parent) {
            Ok(_) => {
                if !parent.is_dir() {
                    continue;
                }
            }
            Err(_) => continue,
        }
        let target = parent.join("skills").join("solstone");
        match guard.sync(&bundled, &target, UserSkillMode::Refresh) {
            UserSkillSync::Failed(e) => {
                log::warn!(
                    "refresh user skills: failed to sync {}: {e}",
                    target.display()
                );
            }
            UserSkillSync::Preserved => {
                log::warn!(
                    "refresh user skills: preserved {} due to journal overlap or unknown state",
                    target.display()
                );
            }
            UserSkillSync::Ineligible
            | UserSkillSync::Unchanged
            | UserSkillSync::Installed
            | UserSkillSync::Replaced => {}
        }
    }
}

enum SkillMdCheck {
    MatchesSolstone,
    Ineligible,
    InvalidUtf8(String),
}

fn check_skill_md_name(bytes: &[u8]) -> SkillMdCheck {
    let text = match std::str::from_utf8(bytes) {
        Ok(text) => text,
        Err(e) => return SkillMdCheck::InvalidUtf8(e.to_string()),
    };
    let mut lines = text.lines();
    if lines.next() != Some("---") {
        return SkillMdCheck::Ineligible;
    }
    #[derive(serde::Deserialize)]
    struct SkillHeader {
        name: String,
    }
    let mut frontmatter = String::new();
    for line in lines {
        if line == "---" {
            return match serde_yaml_ng::from_str::<SkillHeader>(&frontmatter) {
                Ok(header) if header.name == "solstone" => SkillMdCheck::MatchesSolstone,
                _ => SkillMdCheck::Ineligible,
            };
        }
        frontmatter.push_str(line);
        frontmatter.push('\n');
    }
    SkillMdCheck::Ineligible
}

fn copy_tree_to_staging(src: &Path, dst: &Path) -> io::Result<()> {
    let mut wrote_first_file = false;
    copy_tree_inner(src, dst, &mut wrote_first_file)?;
    if !wrote_first_file {
        trigger_copy_fault()?;
    }
    Ok(())
}

fn copy_tree_inner(src: &Path, dst: &Path, wrote_first_file: &mut bool) -> io::Result<()> {
    for entry in fs::read_dir(src)? {
        let entry = entry?;
        let src_path = entry.path();
        let meta = fs::symlink_metadata(&src_path)?;
        let file_type = meta.file_type();
        let dst_path = dst.join(entry.file_name());

        if file_type.is_symlink() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "symlinks are not permitted in user skill trees",
            ));
        } else if file_type.is_dir() {
            fs::create_dir_all(&dst_path)?;
            copy_tree_inner(&src_path, &dst_path, wrote_first_file)?;
        } else if file_type.is_file() {
            copy_file_0600(&src_path, &dst_path)?;
            if !*wrote_first_file {
                *wrote_first_file = true;
                trigger_copy_fault()?;
            }
        } else {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "special files are not permitted in user skill trees",
            ));
        }
    }
    Ok(())
}

fn copy_file_0600(src: &Path, dst: &Path) -> io::Result<()> {
    if let Some(parent) = dst.parent() {
        fs::create_dir_all(parent)?;
    }
    let temp = temp_path(dst);
    let result = (|| {
        let mut input = File::open(src)?;
        let mut output = create_temp_file_0600(&temp)?;
        let mut buffer = Vec::new();
        input.read_to_end(&mut buffer)?;
        output.write_all(&buffer)?;
        output.sync_all()?;
        drop(output);
        set_mode_0600(&temp)?;
        fs::rename(&temp, dst)
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temp);
    }
    result
}

fn temp_path(dst: &Path) -> PathBuf {
    let count = COUNTER.fetch_add(1, Ordering::Relaxed);
    let name = format!(".tmp_{}_{}.tmp", std::process::id(), count);
    dst.parent().unwrap_or_else(|| Path::new(".")).join(name)
}

#[cfg(unix)]
fn create_temp_file_0600(path: &Path) -> io::Result<File> {
    use std::os::unix::fs::OpenOptionsExt;
    OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)
}

#[cfg(not(unix))]
fn create_temp_file_0600(path: &Path) -> io::Result<File> {
    OpenOptions::new().write(true).create_new(true).open(path)
}

#[cfg(unix)]
fn set_mode_0600(path: &Path) -> io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(0o600))
}

#[cfg(not(unix))]
fn set_mode_0600(_path: &Path) -> io::Result<()> {
    Ok(())
}

#[cfg(test)]
mod header_tests {
    use super::{SkillMdCheck, check_skill_md_name};

    #[test]
    fn only_an_unambiguous_top_level_solstone_name_qualifies() {
        for header in [
            "name: solstone",
            "name: 'solstone' # our skill",
            "name: solstone\ndescription: |\n  name: another-skill",
        ] {
            let text = format!("---\n{header}\n---\n# skill\n");
            assert!(matches!(
                check_skill_md_name(text.as_bytes()),
                SkillMdCheck::MatchesSolstone
            ));
        }
        for header in [
            "metadata:\n  name: solstone",
            "name: another-skill",
            "name: solstone\n'name': another-skill",
            "name: solstone\ndescription: [",
            "- name: solstone",
        ] {
            let text = format!("---\n{header}\n---\n# skill\n");
            assert!(
                matches!(
                    check_skill_md_name(text.as_bytes()),
                    SkillMdCheck::Ineligible
                ),
                "must preserve header: {header}"
            );
        }
    }
}

#[cfg(all(test, feature = "full-tests"))]
mod tests {
    use super::*;

    fn replacement_fixture() -> (tempfile::TempDir, PathBuf, PathBuf, PathBuf, PathBuf) {
        let temp = tempfile::tempdir().expect("fixture");
        let home = temp.path().join("home");
        let target = home.join(".claude/skills/solstone");
        let source = temp.path().join("source");
        let staging = home.join(".claude/skills/staged");
        for dir in [&target, &source, &staging] {
            fs::create_dir_all(dir).unwrap();
        }
        fs::write(target.join("SKILL.md"), b"---\nname: solstone\n---\nold").unwrap();
        fs::write(source.join("SKILL.md"), b"---\nname: solstone\n---\nnew").unwrap();
        copy_tree_to_staging(&source, &staging).unwrap();
        (temp, home, target, source, staging)
    }

    #[test]
    fn publication_preserves_an_absent_protected_backup_location() {
        let (_temp, home, target, _source, staging) = replacement_fixture();
        let backup = target.parent().unwrap().join("future-journal");
        let guard = guard_user_skill_mutation(&home, &backup).unwrap();
        let before = fs::read(target.join("SKILL.md")).unwrap();
        assert_eq!(
            guard.publish_staged_skill(&staging, &target, &backup, true),
            UserSkillSync::Preserved
        );
        assert!(!backup.exists());
        assert_eq!(fs::read(target.join("SKILL.md")).unwrap(), before);
    }

    #[test]
    fn publication_does_not_replace_a_preexisting_empty_backup_directory() {
        let (temp, home, target, _source, staging) = replacement_fixture();
        let backup = target.parent().unwrap().join("existing-directory");
        fs::create_dir(&backup).unwrap();
        let guard = guard_user_skill_mutation(&home, &temp.path().join("journal")).unwrap();
        let before = fs::read(target.join("SKILL.md")).unwrap();
        assert!(matches!(
            guard.publish_staged_skill(&staging, &target, &backup, true),
            UserSkillSync::Failed(_)
        ));
        assert!(backup.is_dir());
        assert_eq!(fs::read_dir(&backup).unwrap().count(), 0);
        assert_eq!(fs::read(target.join("SKILL.md")).unwrap(), before);
    }

    #[test]
    fn publication_failure_restores_the_complete_previous_tree() {
        let (temp, home, target, _source, staging) = replacement_fixture();
        fs::create_dir(target.join("empty")).unwrap();
        fs::write(target.join("additional-file"), b"old additional bytes").unwrap();
        fs::remove_dir_all(&staging).unwrap();
        let backup = target.parent().unwrap().join("backup");
        let guard = guard_user_skill_mutation(&home, &temp.path().join("journal")).unwrap();
        let before = fs::read(target.join("SKILL.md")).unwrap();
        assert!(matches!(
            guard.publish_staged_skill(&staging, &target, &backup, true),
            UserSkillSync::Failed(_)
        ));
        assert_eq!(fs::read(target.join("SKILL.md")).unwrap(), before);
        assert!(target.join("empty").is_dir());
        assert_eq!(
            fs::read(target.join("additional-file")).unwrap(),
            b"old additional bytes"
        );
        assert!(!backup.exists());
    }

    #[cfg(unix)]
    #[test]
    fn cleanup_failure_keeps_the_complete_published_skill() {
        use std::os::unix::fs::PermissionsExt;
        let (temp, home, target, source, staging) = replacement_fixture();
        let locked = target.join("locked");
        fs::create_dir(&locked).unwrap();
        fs::write(locked.join("old-file"), b"old bytes").unwrap();
        fs::set_permissions(&locked, fs::Permissions::from_mode(0o555)).unwrap();
        let backup = target.parent().unwrap().join("backup");
        struct Restore(Vec<PathBuf>);
        impl Drop for Restore {
            fn drop(&mut self) {
                for dir in &self.0 {
                    let _ = fs::set_permissions(dir, fs::Permissions::from_mode(0o755));
                }
            }
        }
        let _restore = Restore(vec![locked, backup.join("previous/locked")]);
        let guard = guard_user_skill_mutation(&home, &temp.path().join("journal")).unwrap();
        assert_eq!(
            guard.publish_staged_skill(&staging, &target, &backup, true),
            UserSkillSync::Replaced
        );
        assert!(solstone_core_skill_state::user_skill_copy_matches(&source, &target).unwrap());
        assert!(backup.is_dir(), "the real cleanup failure was exercised");
        assert_eq!(
            guard.sync(&source, &target, UserSkillMode::Refresh),
            UserSkillSync::Unchanged
        );
    }

    #[test]
    fn ambiguous_frontmatter_is_preserved_at_the_mutation_seam() {
        let (temp, home, target, source, _staging) = replacement_fixture();
        let guard = guard_user_skill_mutation(&home, &temp.path().join("journal")).unwrap();
        for header in [
            "metadata:\n  name: solstone",
            "name: solstone\n'name': another-skill",
            "name: solstone\ndescription: [",
        ] {
            let bytes = format!("---\n{header}\n---\nold body").into_bytes();
            fs::write(target.join("SKILL.md"), &bytes).unwrap();
            assert_eq!(
                guard.sync(&source, &target, UserSkillMode::Refresh),
                UserSkillSync::Ineligible
            );
            assert_eq!(fs::read(target.join("SKILL.md")).unwrap(), bytes);
        }
    }

    #[cfg(unix)]
    #[test]
    fn unreadable_extra_entries_are_preserved_despite_a_different_entry_set() {
        use std::os::unix::fs::PermissionsExt;
        let (temp, home, target, source, _staging) = replacement_fixture();
        let extra = target.join("owner-extra");
        fs::write(&extra, b"keep these bytes").unwrap();
        struct Restore(PathBuf);
        impl Drop for Restore {
            fn drop(&mut self) {
                let _ = fs::set_permissions(&self.0, fs::Permissions::from_mode(0o600));
            }
        }
        let restore = Restore(extra.clone());
        fs::set_permissions(&extra, fs::Permissions::from_mode(0o000)).unwrap();
        let guard = guard_user_skill_mutation(&home, &temp.path().join("journal")).unwrap();
        assert!(matches!(
            guard.sync(&source, &target, UserSkillMode::Refresh),
            UserSkillSync::Failed(_)
        ));
        drop(restore);
        assert_eq!(fs::read(&extra).unwrap(), b"keep these bytes");
        assert_eq!(
            fs::read(target.join("SKILL.md")).unwrap(),
            b"---\nname: solstone\n---\nold"
        );
    }

    #[test]
    fn matcher_equal_bytes_extra_dir_inner_symlink_fifo() {
        let temp = std::env::temp_dir().join(format!(
            "user-skill-matcher-{}-{}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = fs::remove_dir_all(&temp);
        fs::create_dir_all(&temp).unwrap();

        let left = temp.join("left");
        let right = temp.join("right");
        fs::create_dir_all(&left).unwrap();
        fs::create_dir_all(&right).unwrap();
        fs::write(left.join("SKILL.md"), b"content").unwrap();
        fs::write(right.join("SKILL.md"), b"content").unwrap();

        // 1. equal bytes
        assert!(solstone_core_skill_state::user_skill_copy_matches(&left, &right).unwrap());

        // 2. extra empty directory
        fs::create_dir(left.join("empty_dir")).unwrap();
        assert!(!solstone_core_skill_state::user_skill_copy_matches(&left, &right).unwrap());
        fs::remove_dir(left.join("empty_dir")).unwrap();

        // 3. inner symlink
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink("target", left.join("link")).unwrap();
            assert!(!solstone_core_skill_state::user_skill_copy_matches(&left, &right).unwrap());
            fs::remove_file(left.join("link")).unwrap();
        }

        // 4. fifo via mkfifo
        #[cfg(unix)]
        {
            let fifo_path = left.join("named_pipe");
            let status = std::process::Command::new("mkfifo")
                .arg(&fifo_path)
                .status()
                .expect("mkfifo");
            assert!(status.success());
            assert!(!solstone_core_skill_state::user_skill_copy_matches(&left, &right).unwrap());
            let _ = fs::remove_file(&fifo_path);
        }

        let _ = fs::remove_dir_all(&temp);
    }

    #[test]
    fn synthetic_source_install_then_refresh_preserves_inodes_and_empty_dirs() {
        #[cfg(unix)]
        use std::os::unix::fs::MetadataExt;

        let temp = std::env::temp_dir().join(format!(
            "user-skill-synthetic-{}-{}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = fs::remove_dir_all(&temp);
        fs::create_dir_all(&temp).unwrap();

        let home = temp.join("home");
        let journal = temp.join("journal");
        fs::create_dir_all(&home).unwrap();
        fs::create_dir_all(&journal).unwrap();

        let source = temp.join("source");
        fs::create_dir_all(source.join("empty_sub")).unwrap();
        let skill_md_content = b"---\nname: solstone\n---\n# Skill\n";
        fs::write(source.join("SKILL.md"), skill_md_content).unwrap();

        let guard = guard_user_skill_mutation(&home, &journal).expect("guard");
        let claude_root = home.join(".claude");
        fs::create_dir_all(&claude_root).unwrap();
        let target = claude_root.join("skills/solstone");

        // First: Install mode
        let sync_install = guard.sync(&source, &target, UserSkillMode::Install);
        assert_eq!(sync_install, UserSkillSync::Installed);
        assert!(target.join("empty_sub").is_dir());
        assert!(solstone_core_skill_state::user_skill_copy_matches(&source, &target).unwrap());

        #[cfg(unix)]
        {
            let target_dir_meta = fs::metadata(&target).unwrap();
            let file_meta = fs::metadata(target.join("SKILL.md")).unwrap();

            let target_dir_ino = target_dir_meta.ino();
            let file_ino = file_meta.ino();
            let file_mtime = file_meta.mtime();
            let file_mtime_nsec = file_meta.mtime_nsec();

            // Second: Refresh mode
            let sync_refresh = guard.sync(&source, &target, UserSkillMode::Refresh);
            assert_eq!(sync_refresh, UserSkillSync::Unchanged);

            let target_dir_meta_after = fs::metadata(&target).unwrap();
            let file_meta_after = fs::metadata(target.join("SKILL.md")).unwrap();

            assert_eq!(target_dir_meta_after.ino(), target_dir_ino);
            assert_eq!(file_meta_after.ino(), file_ino);
            assert_eq!(file_meta_after.mtime(), file_mtime);
            assert_eq!(file_meta_after.mtime_nsec(), file_mtime_nsec);
        }

        let _ = fs::remove_dir_all(&temp);
    }
}
