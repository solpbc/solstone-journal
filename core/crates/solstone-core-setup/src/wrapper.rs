// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Managed `solstone` and `journal` wrapper provisioning.

use std::fs::{self, File, OpenOptions};
use std::io;
use std::os::unix::fs::{PermissionsExt, symlink};
use std::path::{Path, PathBuf};

use chrono::Utc;
use nix::fcntl::{Flock, FlockArg};

use crate::args::canonicalize_or_normalize;

pub const WRAPPER_MARKER: &str = "# managed-version: 7";
pub const WRAPPER_VERSION: u8 = 7;
const BACKUP_ATTEMPTS: u8 = 100;

const WRAPPER_TEMPLATE: &str = r#"#!/bin/bash
# {binary} — managed by 'journal config'. Edits will be overwritten.
# managed-version: 7
: "${SOLSTONE_JOURNAL:={journal}}"
export SOLSTONE_JOURNAL
SOL_BIN='{sol_bin}'
# Warn when pyproject.toml or uv.lock is newer than .installed.
# Skipped silently if .installed is absent.
REPO_ROOT="${SOL_BIN%/.venv/bin/{binary}}"
if [ -f "$REPO_ROOT/.installed" ]; then
  if [ "$REPO_ROOT/pyproject.toml" -nt "$REPO_ROOT/.installed" ] \
     || [ "$REPO_ROOT/uv.lock" -nt "$REPO_ROOT/.installed" ]; then
    echo "{binary}: WARNING — venv is stale (pyproject.toml or uv.lock changed since last install). Run: cd $REPO_ROOT && make install" >&2
  fi
fi
if [ ! -x "$SOL_BIN" ]; then
    printf '{binary}: venv binary missing or not executable: %s\n' "$SOL_BIN" >&2
    exit 127
fi
exec "$SOL_BIN" "$@"
"#;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AliasState {
    Worktree,
    Absent,
    Owned,
    CrossRepo,
    Dangling,
    Foreign,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedWrapper {
    pub journal: String,
    pub sol_bin: PathBuf,
    pub version: u8,
}

#[derive(Debug)]
pub struct WrapperError(pub String);

impl std::fmt::Display for WrapperError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl std::error::Error for WrapperError {}

impl From<io::Error> for WrapperError {
    fn from(error: io::Error) -> Self {
        Self(error.to_string())
    }
}

#[derive(Debug, Clone)]
pub struct WrapperEnvironment {
    pub home_dir: PathBuf,
    pub curdir: PathBuf,
    pub executable_dir: PathBuf,
    /// Test-only override. `None` always means the literal production `/tmp`.
    pub backup_dir: Option<PathBuf>,
}

impl WrapperEnvironment {
    #[must_use]
    pub fn backup_dir(&self) -> PathBuf {
        self.backup_dir
            .clone()
            .unwrap_or_else(|| PathBuf::from("/tmp"))
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WrapperPaths {
    pub solstone: PathBuf,
    pub journal: PathBuf,
}

#[must_use]
pub fn wrapper_paths(home_dir: &Path) -> WrapperPaths {
    let bin_dir = home_dir.join(".local/bin");
    WrapperPaths {
        solstone: bin_dir.join("solstone"),
        journal: bin_dir.join("journal"),
    }
}

#[must_use]
pub fn render_wrapper(binary: &str, journal: &Path, sol_bin: &Path) -> String {
    WRAPPER_TEMPLATE
        .replace("{binary}", binary)
        .replace("{journal}", &journal.to_string_lossy())
        .replace(
            "{sol_bin}",
            &sol_bin.to_string_lossy().replace('\'', "'\\''"),
        )
}

#[must_use]
pub fn parse_wrapper(content: &str) -> Option<ParsedWrapper> {
    let marker = content
        .lines()
        .find(|line| line.starts_with("# managed-version: "))?;
    let version = marker
        .strip_prefix("# managed-version: ")?
        .parse::<u8>()
        .ok()?;
    if !(1..=WRAPPER_VERSION).contains(&version) {
        return None;
    }
    let journal = content
        .lines()
        .find_map(|line| {
            line.strip_prefix(": \"${SOLSTONE_JOURNAL:=")
                .and_then(|value| value.strip_suffix("}\""))
        })?
        .to_owned();
    let sol_bin = content
        .lines()
        .find_map(|line| {
            line.strip_prefix("SOL_BIN='")
                .and_then(|value| value.strip_suffix('\''))
        })?
        .replace("'\\''", "'");
    Some(ParsedWrapper {
        journal,
        sol_bin: PathBuf::from(sol_bin),
        version,
    })
}

fn resolved(path: &Path) -> PathBuf {
    canonicalize_or_normalize(path)
}

fn expected_targets(environment: &WrapperEnvironment, binary: &str) -> [PathBuf; 2] {
    [
        environment.curdir.join(".venv/bin").join(binary),
        environment.executable_dir.join(binary),
    ]
}

pub fn check_alias(
    environment: &WrapperEnvironment,
    binary: &str,
) -> Result<(AliasState, Option<PathBuf>), WrapperError> {
    if environment.curdir.join(".git").is_file() {
        return Ok((AliasState::Worktree, None));
    }
    let aliases = wrapper_paths(&environment.home_dir);
    let alias = if binary == "solstone" {
        aliases.solstone
    } else {
        aliases.journal
    };
    if !alias.exists() && !alias.is_symlink() {
        return Ok((AliasState::Absent, None));
    }
    let targets = expected_targets(environment, binary)
        .iter()
        .map(|path| resolved(path))
        .collect::<Vec<_>>();
    if alias.is_symlink() {
        let target = fs::read_link(&alias)?;
        let target = if target.is_absolute() {
            target
        } else {
            alias.parent().unwrap_or(Path::new(".")).join(target)
        };
        let target_resolved = resolved(&target);
        if targets.contains(&target_resolved) {
            return Ok((AliasState::Owned, Some(target_resolved)));
        }
        if !target.exists() {
            return Ok((AliasState::Dangling, Some(target)));
        }
        return Ok((AliasState::CrossRepo, Some(target_resolved)));
    }
    let Ok(content) = fs::read_to_string(&alias) else {
        return Ok((AliasState::Foreign, None));
    };
    let Some(wrapper) = parse_wrapper(&content) else {
        return Ok((AliasState::Foreign, None));
    };
    let target = resolved(&wrapper.sol_bin);
    Ok((
        if targets.contains(&target) {
            AliasState::Owned
        } else {
            AliasState::Foreign
        },
        Some(target),
    ))
}

fn candidate_taken(path: &Path) -> bool {
    path.exists() || path.is_symlink()
}

pub fn legacy_backup_path(
    binary: &str,
    directory: &Path,
    timestamp: &str,
) -> Result<PathBuf, WrapperError> {
    for index in 0..BACKUP_ATTEMPTS {
        let suffix = if index == 0 {
            String::new()
        } else {
            format!("-{index}")
        };
        let candidate = directory.join(format!("{binary}.old-symlink-{timestamp}{suffix}"));
        if !candidate_taken(&candidate) {
            return Ok(candidate);
        }
    }
    Err(WrapperError(format!(
        "could not allocate backup path for {binary} after {BACKUP_ATTEMPTS} attempts"
    )))
}

fn backup_alias_to_tmp(
    alias: &Path,
    binary: &str,
    environment: &WrapperEnvironment,
) -> Option<PathBuf> {
    let timestamp = Utc::now().format("%Y%m%d%H%M%S").to_string();
    let directory = environment.backup_dir();
    let outcome = (|| -> Result<PathBuf, WrapperError> {
        fs::create_dir_all(&directory)?;
        let backup = legacy_backup_path(binary, &directory, &timestamp)?;
        if alias.is_symlink() {
            symlink(fs::read_link(alias)?, &backup)?;
        } else {
            fs::copy(alias, &backup)?;
            let mode = fs::metadata(alias)?.permissions().mode();
            fs::set_permissions(&backup, fs::Permissions::from_mode(mode))?;
        }
        Ok(backup)
    })();
    match outcome {
        Ok(path) => Some(path),
        Err(error) => {
            eprintln!(
                "journal setup: could not back up {}: {error}",
                alias.display()
            );
            None
        }
    }
}

#[derive(Debug, Clone)]
enum PathSnapshot {
    Absent,
    Symlink(PathBuf),
    File { content: Vec<u8>, mode: u32 },
}

fn snapshot_path(path: &Path) -> Result<PathSnapshot, WrapperError> {
    if !path.exists() && !path.is_symlink() {
        return Ok(PathSnapshot::Absent);
    }
    if path.is_symlink() {
        return Ok(PathSnapshot::Symlink(fs::read_link(path)?));
    }
    Ok(PathSnapshot::File {
        content: fs::read(path)?,
        mode: fs::metadata(path)?.permissions().mode(),
    })
}

fn unlink_any(path: &Path) -> io::Result<()> {
    if path.is_symlink() || path.is_file() {
        fs::remove_file(path)
    } else if path.exists() {
        fs::remove_dir_all(path)
    } else {
        Ok(())
    }
}

fn restore_path(path: &Path, snapshot: &PathSnapshot) -> Result<(), WrapperError> {
    unlink_any(path)?;
    match snapshot {
        PathSnapshot::Absent => Ok(()),
        PathSnapshot::Symlink(target) => {
            symlink(target, path)?;
            Ok(())
        }
        PathSnapshot::File { content, mode } => {
            fs::write(path, content)?;
            fs::set_permissions(path, fs::Permissions::from_mode(*mode))?;
            Ok(())
        }
    }
}

fn write_wrappers_atomically_with<F>(
    contents: &[(PathBuf, String)],
    mut replace: F,
) -> Result<(), WrapperError>
where
    F: FnMut(&Path, &Path) -> io::Result<()>,
{
    let snapshots = contents
        .iter()
        .map(|(path, _)| snapshot_path(path).map(|snapshot| (path.clone(), snapshot)))
        .collect::<Result<Vec<_>, _>>()?;
    let mut staged = Vec::new();
    let attempt = (|| -> Result<(), WrapperError> {
        for (index, (path, content)) in contents.iter().enumerate() {
            fs::create_dir_all(path.parent().unwrap_or(Path::new(".")))?;
            let staged_path = path.parent().unwrap_or(Path::new(".")).join(format!(
                ".{}.tmp-{}-{index}",
                path.file_name()
                    .and_then(|name| name.to_str())
                    .unwrap_or("wrapper"),
                std::process::id()
            ));
            fs::write(&staged_path, content)?;
            fs::set_permissions(&staged_path, fs::Permissions::from_mode(0o755))?;
            staged.push(staged_path);
        }
        for ((path, _), staged_path) in contents.iter().zip(&staged) {
            replace(staged_path, path)?;
        }
        Ok(())
    })();
    if attempt.is_err() {
        for (path, snapshot) in &snapshots {
            let _ = restore_path(path, snapshot);
        }
    }
    for staged_path in staged {
        let _ = unlink_any(&staged_path);
    }
    attempt
}

fn write_wrappers_atomically(contents: &[(PathBuf, String)]) -> Result<(), WrapperError> {
    write_wrappers_atomically_with(contents, |from, to| fs::rename(from, to))
}

fn wrapper_lock(home_dir: &Path) -> Result<Flock<File>, WrapperError> {
    let directory = home_dir.join(".local/bin");
    fs::create_dir_all(&directory)?;
    let file = OpenOptions::new()
        .create(true)
        .read(true)
        .write(true)
        .truncate(false)
        .open(directory.join(".sol.lock"))?;
    Flock::lock(file, FlockArg::LockExclusive).map_err(|(_, error)| WrapperError(error.to_string()))
}

pub fn validate_journal_path_for_wrapper(journal: &Path) -> Result<(), WrapperError> {
    let journal = journal.to_string_lossy();
    for character in ['$', '`', '"', '\\', '\n'] {
        if journal.contains(character) {
            return Err(WrapperError(format!(
                "journal path contains shell-active character {character:?}: {journal:?}"
            )));
        }
    }
    Ok(())
}

pub fn provision_wrappers(
    environment: &WrapperEnvironment,
    journal: &Path,
) -> Result<WrapperPaths, WrapperError> {
    validate_journal_path_for_wrapper(journal)?;
    let paths = wrapper_paths(&environment.home_dir);
    let _lock = wrapper_lock(&environment.home_dir)?;
    let states = [
        ("solstone", check_alias(environment, "solstone")?),
        ("journal", check_alias(environment, "journal")?),
    ];
    if states
        .iter()
        .any(|(_, (state, _))| *state == AliasState::Worktree)
    {
        return Ok(paths);
    }
    for (binary, (state, _)) in &states {
        if matches!(
            state,
            AliasState::CrossRepo | AliasState::Dangling | AliasState::Foreign
        ) {
            let alias = if *binary == "solstone" {
                &paths.solstone
            } else {
                &paths.journal
            };
            let _ = backup_alias_to_tmp(alias, binary, environment);
        }
    }
    write_wrappers_atomically(&[
        (
            paths.solstone.clone(),
            render_wrapper(
                "solstone",
                journal,
                &environment.executable_dir.join("solstone"),
            ),
        ),
        (
            paths.journal.clone(),
            render_wrapper(
                "journal",
                journal,
                &environment.executable_dir.join("journal"),
            ),
        ),
    ])?;
    Ok(paths)
}

/// Remove only aliases this runtime owns.  Refusal state is returned intact for callers.
pub fn uninstall_wrappers(
    environment: &WrapperEnvironment,
) -> Result<(), (AliasState, Option<PathBuf>)> {
    let states = [
        check_alias(environment, "solstone").map_err(|_| (AliasState::Foreign, None))?,
        check_alias(environment, "journal").map_err(|_| (AliasState::Foreign, None))?,
    ];
    if let Some(blocked) = states
        .iter()
        .find(|(state, _)| !matches!(state, AliasState::Absent | AliasState::Owned))
    {
        return Err(blocked.clone());
    }
    let _lock = wrapper_lock(&environment.home_dir).map_err(|_| (AliasState::Foreign, None))?;
    let states = [
        check_alias(environment, "solstone").map_err(|_| (AliasState::Foreign, None))?,
        check_alias(environment, "journal").map_err(|_| (AliasState::Foreign, None))?,
    ];
    if let Some(blocked) = states
        .iter()
        .find(|(state, _)| !matches!(state, AliasState::Absent | AliasState::Owned))
    {
        return Err(blocked.clone());
    }
    let paths = wrapper_paths(&environment.home_dir);
    for path in [&paths.solstone, &paths.journal] {
        if path.exists() || path.is_symlink() {
            fs::remove_file(path).map_err(|_| (AliasState::Foreign, None))?;
        }
    }
    Ok(())
}

/// Whether an alias is the app-owned child launcher used by the macOS app.
#[must_use]
pub fn is_live_app_owned_child_launcher(path: &Path) -> bool {
    let Ok(content) = fs::read_to_string(path) else {
        return false;
    };
    let Some(target) = content
        .strip_prefix("#!/bin/sh\n# managed-version: app-owned-child\nexec '")
        .and_then(|value| value.strip_suffix("' \"$@\"\n"))
        .map(|value| PathBuf::from(value.replace("'\\''", "'")))
    else {
        return false;
    };
    fs::metadata(target)
        .is_ok_and(|metadata| metadata.is_file() && metadata.permissions().mode() & 0o111 != 0)
}

pub fn ensure_user_bin_on_path(home_dir: &Path) -> String {
    let bin = home_dir.join(".local/bin");
    let targets = [
        (
            home_dir.join(".bashrc"),
            "export PATH=\"$HOME/.local/bin:$PATH\"\n",
        ),
        (
            home_dir.join(".zshrc"),
            "export PATH=\"$HOME/.local/bin:$PATH\"\n",
        ),
        (
            home_dir.join(".config/fish/config.fish"),
            "fish_add_path $HOME/.local/bin\n",
        ),
        (
            home_dir.join(".profile"),
            "export PATH=\"$HOME/.local/bin:$PATH\"\n",
        ),
    ];
    let mut existing = targets
        .iter()
        .filter(|(path, _)| path.exists())
        .collect::<Vec<_>>();
    if existing.is_empty() {
        existing.push(&targets[3]);
    }
    let changed_existing = existing.iter().any(|(path, _)| path.exists());
    let mut changed = false;
    let result = (|| -> io::Result<()> {
        fs::create_dir_all(&bin)?;
        for (path, line) in existing {
            if let Some(parent) = path.parent() {
                fs::create_dir_all(parent)?;
            }
            let content = if path.exists() {
                fs::read_to_string(path)?
            } else {
                String::new()
            };
            let has_path = content.lines().any(|line| {
                line.contains(".local/bin")
                    && (line.contains("PATH") || line.contains("fish_add_path"))
            });
            if !has_path {
                let separator = if content.is_empty() || content.ends_with('\n') {
                    ""
                } else {
                    "\n"
                };
                fs::write(path, format!("{content}{separator}{line}"))?;
                changed = true;
            }
        }
        Ok(())
    })();
    if result.is_err() {
        return "path: could not auto-add ~/.local/bin to PATH — add this line to your shell rc manually: export PATH=\"$HOME/.local/bin:$PATH\"".into();
    }
    if !changed {
        return "path: ~/.local/bin already on PATH".into();
    }
    if changed_existing {
        "path: added ~/.local/bin to shell PATH — restart your shell or run 'exec $SHELL -l' to pick it up".into()
    } else {
        "path: added ~/.local/bin to shell PATH".into()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn root(name: &str) -> PathBuf {
        let path = PathBuf::from("/var/tmp")
            .join(format!("solstone-wrapper-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&path);
        fs::create_dir_all(&path).unwrap();
        path
    }
    fn environment(root: &Path) -> WrapperEnvironment {
        WrapperEnvironment {
            home_dir: root.join("home"),
            curdir: root.join("repo"),
            executable_dir: root.join("runtime"),
            backup_dir: Some(root.join("backups")),
        }
    }
    #[test]
    fn parses_rendered_wrapper_and_unescapes_sol_bin() {
        let rendered = render_wrapper(
            "solstone",
            Path::new("/journal"),
            Path::new("/a'quoted/.venv/bin/solstone"),
        );
        let parsed = parse_wrapper(&rendered).unwrap();
        assert_eq!(parsed.version, 7);
        assert_eq!(parsed.journal, "/journal");
        assert_eq!(
            parsed.sol_bin,
            PathBuf::from("/a'quoted/.venv/bin/solstone")
        );
    }
    #[test]
    fn backup_default_is_literal_tmp_and_collision_names_increment() {
        let env = WrapperEnvironment {
            home_dir: PathBuf::from("/ignored"),
            curdir: PathBuf::new(),
            executable_dir: PathBuf::new(),
            backup_dir: None,
        };
        assert_eq!(env.backup_dir(), PathBuf::from("/tmp"));
        let path = root("backup-names");
        fs::write(path.join("solstone.old-symlink-20260101000000"), "x").unwrap();
        assert_eq!(
            legacy_backup_path("solstone", &path, "20260101000000")
                .unwrap()
                .file_name()
                .unwrap(),
            "solstone.old-symlink-20260101000000-1"
        );
        assert_eq!(BACKUP_ATTEMPTS, 100);
    }
    #[test]
    fn backup_path_gives_up_after_the_hundredth_taken_name() {
        let path = root("backup-limit");
        for index in 0..BACKUP_ATTEMPTS {
            let suffix = if index == 0 {
                String::new()
            } else {
                format!("-{index}")
            };
            fs::write(
                path.join(format!("solstone.old-symlink-20260101000000{suffix}")),
                "x",
            )
            .unwrap();
        }
        assert!(legacy_backup_path("solstone", &path, "20260101000000").is_err());
    }
    #[test]
    fn shell_active_journal_path_is_refused_before_write() {
        let root = root("shell-char");
        let env = environment(&root);
        fs::create_dir_all(&env.curdir).unwrap();
        let error = provision_wrappers(&env, Path::new("/bad$journal")).unwrap_err();
        assert!(error.to_string().contains('$'));
        assert!(!wrapper_paths(&env.home_dir).solstone.exists());
    }
    #[test]
    fn dangling_alias_backup_preserves_the_link_target_then_is_overwritten() {
        let root = root("dangling");
        let env = environment(&root);
        fs::create_dir_all(&env.curdir).unwrap();
        fs::create_dir_all(env.home_dir.join(".local/bin")).unwrap();
        symlink("/missing-target", wrapper_paths(&env.home_dir).solstone).unwrap();
        provision_wrappers(&env, Path::new("/journal")).unwrap();
        let backup = fs::read_dir(env.backup_dir())
            .unwrap()
            .flatten()
            .find(|entry| {
                entry
                    .file_name()
                    .to_string_lossy()
                    .starts_with("solstone.old-symlink-")
            })
            .unwrap()
            .path();
        assert!(backup.is_symlink());
        assert_eq!(
            fs::read_link(backup).unwrap(),
            PathBuf::from("/missing-target")
        );
        assert!(
            fs::read_to_string(wrapper_paths(&env.home_dir).solstone)
                .unwrap()
                .contains(WRAPPER_MARKER)
        );
    }
    #[test]
    fn nonexistent_local_venv_symlink_alias_is_owned() {
        let root = root("nonexistent-symlink-owner");
        let environment = environment(&root);
        fs::create_dir_all(&environment.curdir).unwrap();
        fs::create_dir_all(environment.home_dir.join(".local/bin")).unwrap();
        let expected = environment.curdir.join(".venv/bin/solstone");
        assert!(!expected.exists());
        symlink(&expected, wrapper_paths(&environment.home_dir).solstone).unwrap();
        assert_eq!(
            check_alias(&environment, "solstone").unwrap().0,
            AliasState::Owned
        );
    }
    #[test]
    fn nonexistent_local_venv_regular_wrapper_alias_is_owned() {
        let root = root("nonexistent-wrapper-owner");
        let environment = environment(&root);
        fs::create_dir_all(&environment.curdir).unwrap();
        fs::create_dir_all(environment.home_dir.join(".local/bin")).unwrap();
        let expected = environment.curdir.join(".venv/bin/solstone");
        assert!(!expected.exists());
        fs::write(
            wrapper_paths(&environment.home_dir).solstone,
            render_wrapper("solstone", Path::new("/journal"), &expected),
        )
        .unwrap();
        assert_eq!(
            check_alias(&environment, "solstone").unwrap().0,
            AliasState::Owned
        );
    }
    #[test]
    fn foreign_alias_is_backed_up_and_absent_or_owned_aliases_are_not() {
        let root = root("states");
        let env = environment(&root);
        fs::create_dir_all(&env.curdir).unwrap();
        fs::create_dir_all(&env.executable_dir).unwrap();
        fs::create_dir_all(env.home_dir.join(".local/bin")).unwrap();
        fs::write(env.executable_dir.join("solstone"), "runtime").unwrap();
        fs::write(env.executable_dir.join("journal"), "runtime").unwrap();
        fs::write(wrapper_paths(&env.home_dir).solstone, "foreign wrapper").unwrap();
        fs::write(
            wrapper_paths(&env.home_dir).journal,
            render_wrapper(
                "journal",
                Path::new("/old"),
                &env.executable_dir.join("journal"),
            ),
        )
        .unwrap();
        provision_wrappers(&env, Path::new("/journal")).unwrap();
        let backups = fs::read_dir(env.backup_dir())
            .unwrap()
            .flatten()
            .map(|entry| entry.file_name().to_string_lossy().into_owned())
            .collect::<Vec<_>>();
        assert_eq!(backups.len(), 1);
        assert!(backups[0].starts_with("solstone.old-symlink-"));
        assert!(
            fs::read_to_string(wrapper_paths(&env.home_dir).solstone)
                .unwrap()
                .contains(WRAPPER_MARKER)
        );
        assert!(
            fs::read_to_string(wrapper_paths(&env.home_dir).journal)
                .unwrap()
                .contains(WRAPPER_MARKER)
        );
        let absent_home = root.join("absent-home");
        let absent = WrapperEnvironment {
            home_dir: absent_home,
            ..env.clone()
        };
        provision_wrappers(&absent, Path::new("/journal")).unwrap();
        assert_eq!(
            fs::read_dir(absent.backup_dir()).unwrap().flatten().count(),
            1
        );
    }
    #[test]
    fn backup_failures_are_swallowed_and_wrappers_still_replace_aliases() {
        let root = root("backup-failure");
        let mut env = environment(&root);
        fs::create_dir_all(&env.curdir).unwrap();
        fs::create_dir_all(env.home_dir.join(".local/bin")).unwrap();
        fs::write(wrapper_paths(&env.home_dir).solstone, "foreign").unwrap();
        let blocked = root.join("blocked-backups");
        fs::write(&blocked, "not a directory").unwrap();
        env.backup_dir = Some(blocked);
        provision_wrappers(&env, Path::new("/journal")).unwrap();
        assert!(
            fs::read_to_string(wrapper_paths(&env.home_dir).solstone)
                .unwrap()
                .contains(WRAPPER_MARKER)
        );
    }
    #[test]
    fn worktree_leaves_aliases_unwritten() {
        let root = root("worktree");
        let env = environment(&root);
        fs::create_dir_all(&env.curdir).unwrap();
        fs::write(env.curdir.join(".git"), "gitdir: x").unwrap();
        let paths = provision_wrappers(&env, Path::new("/journal")).unwrap();
        assert!(!paths.solstone.exists() && !paths.journal.exists());
    }
    #[test]
    fn provision_and_uninstall_leave_a_foreign_file_at_local_bin_sol() {
        let root = root("leave-sol-path");
        let env = environment(&root);
        fs::create_dir_all(&env.curdir).unwrap();
        fs::create_dir_all(&env.executable_dir).unwrap();
        fs::create_dir_all(env.home_dir.join(".local/bin")).unwrap();
        fs::write(env.executable_dir.join("solstone"), "runtime").unwrap();
        fs::write(env.executable_dir.join("journal"), "runtime").unwrap();
        let leftover = env.home_dir.join(".local/bin/sol");
        fs::write(&leftover, "macos-app-owned-or-foreign").unwrap();
        provision_wrappers(&env, Path::new("/journal")).unwrap();
        assert_eq!(
            fs::read_to_string(&leftover).unwrap(),
            "macos-app-owned-or-foreign"
        );
        assert!(wrapper_paths(&env.home_dir).solstone.exists());
        uninstall_wrappers(&env).unwrap();
        assert_eq!(
            fs::read_to_string(&leftover).unwrap(),
            "macos-app-owned-or-foreign"
        );
        assert!(!wrapper_paths(&env.home_dir).solstone.exists());
    }
    #[test]
    fn atomic_failure_restores_every_snapshot_kind() {
        let root = root("rollback");
        let first = root.join("first");
        let second = root.join("second");
        fs::write(&first, "old").unwrap();
        fs::set_permissions(&first, fs::Permissions::from_mode(0o640)).unwrap();
        symlink("/old-target", &second).unwrap();
        let contents = vec![
            (first.clone(), "new1".into()),
            (second.clone(), "new2".into()),
        ];
        let mut calls = 0;
        assert!(
            write_wrappers_atomically_with(&contents, |from, to| {
                calls += 1;
                if calls == 2 {
                    Err(io::Error::other("fail"))
                } else {
                    fs::rename(from, to)
                }
            })
            .is_err()
        );
        assert_eq!(fs::read_to_string(&first).unwrap(), "old");
        assert_eq!(
            fs::metadata(&first).unwrap().permissions().mode() & 0o777,
            0o640
        );
        assert!(second.is_symlink());
        assert_eq!(fs::read_link(second).unwrap(), PathBuf::from("/old-target"));
        let absent = root.join("absent");
        let file = root.join("file");
        fs::write(&file, "old-file").unwrap();
        fs::set_permissions(&file, fs::Permissions::from_mode(0o600)).unwrap();
        let mut calls = 0;
        assert!(
            write_wrappers_atomically_with(
                &[
                    (absent.clone(), "new-absent".into()),
                    (file.clone(), "new-file".into()),
                ],
                |from, to| {
                    calls += 1;
                    if calls == 2 {
                        Err(io::Error::other("fail"))
                    } else {
                        fs::rename(from, to)
                    }
                }
            )
            .is_err()
        );
        assert!(!absent.exists() && !absent.is_symlink());
        assert_eq!(fs::read_to_string(&file).unwrap(), "old-file");
        assert_eq!(
            fs::metadata(&file).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }
    #[test]
    fn path_provisioning_is_idempotent() {
        let root = root("path");
        let home = root.join("home");
        fs::create_dir_all(&home).unwrap();
        fs::write(home.join(".bashrc"), "# rc\n").unwrap();
        assert!(ensure_user_bin_on_path(&home).contains("added"));
        let first = fs::read(home.join(".bashrc")).unwrap();
        assert_eq!(
            ensure_user_bin_on_path(&home),
            "path: ~/.local/bin already on PATH"
        );
        assert_eq!(fs::read(home.join(".bashrc")).unwrap(), first);
    }
}
