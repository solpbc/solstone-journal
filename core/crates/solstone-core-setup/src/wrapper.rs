// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Managed `solstone` and `journal` wrapper provisioning.

use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::os::unix::fs::{PermissionsExt, symlink};
use std::path::{Path, PathBuf};

use chrono::Utc;
use nix::fcntl::{Flock, FlockArg};
use solstone_core_installation_identity::{GuardFields, InstallationBinding, wrapper_guard_lines};
use solstone_core_journal::resolve_identity_root_from_executable_dir;

use crate::args::canonicalize_or_normalize;
use crate::legacy_launcher;

pub const WRAPPER_MARKER: &str = "# managed-version: 8";
pub const WRAPPER_VERSION: u8 = 8;
const BACKUP_ATTEMPTS: u8 = 100;

const WRAPPER_TEMPLATE: &str = r#"#!/bin/bash
# {binary} — managed by 'journal config'. Edits will be overwritten.
# managed-version: 8
{guard}: "${SOLSTONE_JOURNAL:={journal}}"
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
    /// Test-only override. Production keeps recovery copies under the owner home.
    pub backup_dir: Option<PathBuf>,
    /// Setup identity admission positively identified exact V1 launchers.
    pub legacy_replacement: bool,
}

impl WrapperEnvironment {
    #[must_use]
    pub fn backup_dir(&self) -> PathBuf {
        self.backup_dir
            .clone()
            .unwrap_or_else(|| self.home_dir.join(".local/share/solstone/setup-backups"))
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
pub fn render_wrapper(binary: &str, journal: &Path, sol_bin: &Path, guard: &GuardFields) -> String {
    WRAPPER_TEMPLATE
        .replace("{binary}", binary)
        .replace("{guard}", &wrapper_guard_lines(guard))
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

/// Whether `target` is `<prefix>/versions/<some-build>/bin/<binary>` for the
/// same versioned-prefix installation `environment.executable_dir` resolves
/// to.
///
/// `expected_targets` alone only recognises the *current* run's executable
/// directory, which is exactly wrong across a version swap: after
/// `install.sh` flips `current` onto a newly installed build, the wrapper's
/// own `SOL_BIN=` still names the *previous* build's `versions/<old>/bin`
/// (that is the version this wrapper was written for). Without this check
/// that legitimate, sol-pbc-owned sibling reads as `AliasState::Foreign` and
/// `provision_wrappers` refuses to touch it, permanently stranding the
/// wrapper (and the service it fronts) on the old build. Any single path
/// component between `versions/` and `bin/<binary>` qualifies -- it is
/// content-addressed by `install.sh` and never chosen by the owner.
fn is_own_versioned_target(environment: &WrapperEnvironment, binary: &str, target: &Path) -> bool {
    let Some(identity_root) =
        resolve_identity_root_from_executable_dir(&environment.executable_dir)
    else {
        return false;
    };
    // `target` is already `resolved()` by the caller; match that so a
    // non-canonical `identity_root` (the literal-`current` resolver arm
    // returns one unchanged) cannot produce a false negative.
    let versions_dir = resolved(&identity_root).join("versions");
    let Ok(relative) = target.strip_prefix(&versions_dir) else {
        return false;
    };
    let mut components = relative.iter();
    components.next().is_some()
        && components.next().is_some_and(|part| part == "bin")
        && components.next().is_some_and(|part| part == binary)
        && components.next().is_none()
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
        if targets.contains(&target) || is_own_versioned_target(environment, binary, &target) {
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

fn prepare_backup_directory(environment: &WrapperEnvironment) -> Result<PathBuf, WrapperError> {
    let directory = environment.backup_dir();
    match fs::symlink_metadata(&directory) {
        Ok(metadata) if !metadata.file_type().is_dir() => {
            return Err(WrapperError(format!(
                "backup path is not a real directory: {}",
                directory.display()
            )));
        }
        Ok(_) => {}
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            fs::create_dir_all(&directory)?;
        }
        Err(error) => return Err(error.into()),
    }
    if environment.backup_dir.is_none() {
        let resolved_home = fs::canonicalize(&environment.home_dir)?;
        let resolved_directory = fs::canonicalize(&directory)?;
        if !resolved_directory.starts_with(&resolved_home) {
            return Err(WrapperError(format!(
                "backup path resolves outside the owner home: {}",
                directory.display()
            )));
        }
    }
    Ok(directory)
}

fn backup_snapshot(
    snapshot: &PathSnapshot,
    binary: &str,
    directory: &Path,
) -> Result<Option<PathBuf>, WrapperError> {
    if matches!(snapshot, PathSnapshot::Absent) {
        return Ok(None);
    }
    let timestamp = Utc::now().format("%Y%m%d%H%M%S").to_string();
    let backup = legacy_backup_path(binary, directory, &timestamp)?;
    match snapshot {
        PathSnapshot::Absent => unreachable!(),
        PathSnapshot::Symlink(target) => symlink(target, &backup)?,
        PathSnapshot::File { content, mode } => {
            let mut file = OpenOptions::new()
                .create_new(true)
                .write(true)
                .open(&backup)?;
            file.write_all(content)?;
            file.set_permissions(fs::Permissions::from_mode(*mode))?;
            file.sync_all()?;
        }
    }
    File::open(directory)?.sync_all()?;
    Ok(Some(backup))
}

#[derive(Debug, Clone, Eq, PartialEq)]
enum PathSnapshot {
    Absent,
    Symlink(PathBuf),
    File { content: Vec<u8>, mode: u32 },
}

#[derive(Debug)]
struct LegacyExpectation {
    home: PathBuf,
    path: PathBuf,
    command: String,
    launcher: legacy_launcher::LegacyLauncher,
}

fn snapshot_path(path: &Path) -> Result<PathSnapshot, WrapperError> {
    if !path.exists() && !path.is_symlink() {
        return Ok(PathSnapshot::Absent);
    }
    if path.is_symlink() {
        return Ok(PathSnapshot::Symlink(fs::read_link(path)?));
    }
    if !path.is_file() {
        return Err(WrapperError(format!(
            "wrapper path is not a regular file or symlink: {}",
            path.display()
        )));
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
        Err(io::Error::other(format!(
            "refusing to remove non-file wrapper path {}",
            path.display()
        )))
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
            File::open(path.parent().unwrap_or(Path::new(".")))?.sync_all()?;
            Ok(())
        }
        PathSnapshot::File { content, mode } => {
            let mut file = OpenOptions::new().create_new(true).write(true).open(path)?;
            file.write_all(content)?;
            file.set_permissions(fs::Permissions::from_mode(*mode))?;
            file.sync_all()?;
            File::open(path.parent().unwrap_or(Path::new(".")))?.sync_all()?;
            Ok(())
        }
    }
}

pub fn write_wrappers_atomically_with<F>(
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
    write_wrappers_and_remove_with(contents, &[], &snapshots, &[], &mut replace)
}

fn write_wrappers_and_remove_with<F>(
    contents: &[(PathBuf, String)],
    removals: &[PathBuf],
    snapshots: &[(PathBuf, PathSnapshot)],
    legacy_expectations: &[LegacyExpectation],
    replace: &mut F,
) -> Result<(), WrapperError>
where
    F: FnMut(&Path, &Path) -> io::Result<()>,
{
    let mut staged = Vec::new();
    let mut mutation_started = false;
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
            let mut file = OpenOptions::new()
                .create_new(true)
                .write(true)
                .open(&staged_path)?;
            file.write_all(content.as_bytes())?;
            file.set_permissions(fs::Permissions::from_mode(0o755))?;
            file.sync_all()?;
            staged.push(staged_path);
        }
        for (path, expected) in snapshots {
            if snapshot_path(path)? != *expected {
                return Err(WrapperError(format!(
                    "wrapper changed before replacement: {}",
                    path.display()
                )));
            }
        }
        for expected in legacy_expectations {
            let current =
                legacy_launcher::classify(&expected.home, &expected.path, &expected.command)
                    .map_err(WrapperError)?;
            if current.as_ref() != Some(&expected.launcher) {
                return Err(WrapperError(format!(
                    "legacy wrapper changed before replacement: {}",
                    expected.path.display()
                )));
            }
        }
        for ((path, _), staged_path) in contents.iter().zip(&staged) {
            mutation_started = true;
            replace(staged_path, path)?;
        }
        for path in removals {
            mutation_started = true;
            unlink_any(path)?;
        }
        if let Some(directory) = contents.first().and_then(|(path, _)| path.parent()) {
            File::open(directory)?.sync_all()?;
        }
        Ok(())
    })();
    if attempt.is_err() && mutation_started {
        let mut restore_errors = Vec::new();
        for (path, snapshot) in snapshots {
            if let Err(error) = restore_path(path, snapshot) {
                restore_errors.push(format!("{}: {error}", path.display()));
            }
        }
        if !restore_errors.is_empty() {
            for staged_path in staged {
                let _ = unlink_any(&staged_path);
            }
            return Err(WrapperError(format!(
                "wrapper replacement failed and restoration was incomplete: {}",
                restore_errors.join("; ")
            )));
        }
    }
    for staged_path in staged {
        let _ = unlink_any(&staged_path);
    }
    attempt
}

pub fn write_wrappers_atomically(contents: &[(PathBuf, String)]) -> Result<(), WrapperError> {
    write_wrappers_atomically_with(contents, |from, to| fs::rename(from, to))
}

pub fn wrapper_lock(home_dir: &Path) -> Result<Flock<File>, WrapperError> {
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
    binding: &InstallationBinding,
) -> Result<WrapperPaths, WrapperError> {
    validate_journal_path_for_wrapper(journal)?;
    let paths = wrapper_paths(&environment.home_dir);
    let guard = GuardFields::from_binding(binding);
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

    let main_paths = [
        ("solstone", paths.solstone.clone()),
        ("journal", paths.journal.clone()),
    ];
    let legacy_mains = main_paths
        .iter()
        .map(|(command, path)| {
            if environment.legacy_replacement {
                legacy_launcher::classify(&environment.home_dir, path, command)
            } else {
                Ok(None)
            }
            .map(|launcher| ((*command).to_owned(), launcher))
        })
        .collect::<Result<Vec<_>, _>>()
        .map_err(WrapperError)?;
    let mut legacy_expectations = main_paths
        .iter()
        .zip(&legacy_mains)
        .filter_map(|((command, path), (_, launcher))| {
            launcher.clone().map(|launcher| LegacyExpectation {
                home: environment.home_dir.clone(),
                path: path.clone(),
                command: (*command).to_owned(),
                launcher,
            })
        })
        .collect::<Vec<_>>();
    for ((command, (state, _)), (_, legacy)) in states.iter().zip(&legacy_mains) {
        if matches!(
            state,
            AliasState::CrossRepo | AliasState::Dangling | AliasState::Foreign
        ) && legacy.is_none()
        {
            return Err(WrapperError(format!(
                "refusing to replace unrecognized {command} wrapper"
            )));
        }
    }

    let mut removals = Vec::new();
    for (command, companion, main_indices) in [
        (
            "sol",
            environment.home_dir.join(".local/bin/sol"),
            [Some(0_usize), Some(1_usize)],
        ),
        (
            "mlx-vlm-server",
            environment.home_dir.join(".local/bin/mlx-vlm-server"),
            [Some(1_usize), None],
        ),
    ] {
        if let Some(companion_launcher) =
            legacy_launcher::classify(&environment.home_dir, &companion, command)
                .map_err(WrapperError)?
            && main_indices.into_iter().flatten().any(|index| {
                legacy_mains[index]
                    .1
                    .as_ref()
                    .is_some_and(|main| main.same_installation(&companion_launcher))
            })
        {
            removals.push(companion.clone());
            legacy_expectations.push(LegacyExpectation {
                home: environment.home_dir.clone(),
                path: companion,
                command: command.to_owned(),
                launcher: companion_launcher,
            });
        }
    }

    let mut snapshot_paths = main_paths
        .iter()
        .map(|(_, path)| path.clone())
        .collect::<Vec<_>>();
    snapshot_paths.extend(removals.iter().cloned());
    let snapshots = snapshot_paths
        .iter()
        .map(|path| snapshot_path(path).map(|snapshot| (path.clone(), snapshot)))
        .collect::<Result<Vec<_>, _>>()?;
    let backup_directory = prepare_backup_directory(environment)?;
    for (path, snapshot) in &snapshots {
        let name = path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("wrapper");
        backup_snapshot(snapshot, name, &backup_directory)?;
    }

    let contents = [
        (
            paths.solstone.clone(),
            render_wrapper(
                "solstone",
                journal,
                &environment.executable_dir.join("solstone"),
                &guard,
            ),
        ),
        (
            paths.journal.clone(),
            render_wrapper(
                "journal",
                journal,
                &environment.executable_dir.join("journal"),
                &guard,
            ),
        ),
    ];
    write_wrappers_and_remove_with(
        &contents,
        &removals,
        &snapshots,
        &legacy_expectations,
        &mut |from, to| fs::rename(from, to),
    )?;
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
    use std::ops::Deref;
    use std::sync::{Arc, Barrier, mpsc};
    use std::thread;
    use std::time::Duration;

    fn binding() -> InstallationBinding {
        InstallationBinding {
            namespace: solstone_core_installation_identity::NamespaceName::parse(
                "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef",
            )
            .expect("namespace"),
            id: solstone_core_installation_identity::InstallationId::parse(
                "00112233445566778899aabbccddeeff",
            )
            .expect("id"),
            generation: solstone_core_installation_identity::Generation::new(1)
                .expect("generation"),
            platform: solstone_core_installation_identity::PlatformTag::Linux,
            root_token: solstone_core_installation_identity::RootToken::from_raw_absolute(
                b"/install/wrapper".to_vec(),
            )
            .expect("root"),
            journal_token: solstone_core_installation_identity::JournalToken::from_raw_absolute(
                b"/journal".to_vec(),
            )
            .expect("journal"),
        }
    }

    fn guard() -> GuardFields {
        GuardFields::from_binding(&binding())
    }

    struct TestRoot(PathBuf);

    impl TestRoot {
        fn new(name: &str) -> Self {
            let path = PathBuf::from("/var/tmp")
                .join(format!("solstone-wrapper-{name}-{}", std::process::id()));
            let _ = fs::remove_dir_all(&path);
            fs::create_dir_all(&path).unwrap();
            Self(path)
        }
    }

    impl Deref for TestRoot {
        type Target = Path;

        fn deref(&self) -> &Self::Target {
            &self.0
        }
    }

    impl Drop for TestRoot {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn root(name: &str) -> TestRoot {
        TestRoot::new(name)
    }
    fn environment(root: &Path) -> WrapperEnvironment {
        WrapperEnvironment {
            home_dir: root.join("home"),
            curdir: root.join("repo"),
            executable_dir: root.join("runtime"),
            backup_dir: Some(root.join("backups")),
            legacy_replacement: false,
        }
    }

    fn legacy_python_launcher(command: &str) -> String {
        let function = if command == "journal" {
            "journal_main"
        } else {
            "main"
        };
        format!(
            "#!/usr/bin/python3\n# -*- coding: utf-8 -*-\nimport sys\nfrom solstone.think.sol_cli import {function}\nif __name__ == '__main__':\n    if sys.argv[0].endswith('-script.pyw'):\n        sys.argv[0] = sys.argv[0][:-11]\n    elif sys.argv[0].endswith('.exe'):\n        sys.argv[0] = sys.argv[0][:-4]\n    sys.exit({function}())\n"
        )
    }

    fn write_executable(path: &Path, content: &str) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, content).unwrap();
        fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
    }
    #[test]
    fn parses_rendered_wrapper_and_unescapes_sol_bin() {
        let rendered = render_wrapper(
            "solstone",
            Path::new("/journal"),
            Path::new("/a'quoted/.venv/bin/solstone"),
            &guard(),
        );
        let parsed = parse_wrapper(&rendered).unwrap();
        assert_eq!(parsed.version, 8);
        assert_eq!(parsed.journal, "/journal");
        assert_eq!(
            parsed.sol_bin,
            PathBuf::from("/a'quoted/.venv/bin/solstone")
        );
        assert_eq!(
            solstone_core_installation_identity::parse_wrapper_guard(&rendered)
                .expect("parse wrapper guard"),
            Some(guard())
        );
    }

    #[test]
    fn wrapper_base_parser_ignores_corrupted_guard_but_guard_parser_refuses_it() {
        let rendered = render_wrapper(
            "solstone",
            Path::new("/journal"),
            Path::new("/runtime/solstone"),
            &guard(),
        );
        let corrupted = rendered.replace(
            "# solstone-installation-id: 00112233445566778899aabbccddeeff",
            "# solstone-installation-id: invalid",
        );
        assert!(parse_wrapper(&corrupted).is_some());
        assert!(solstone_core_installation_identity::parse_wrapper_guard(&corrupted).is_err());
    }

    #[test]
    fn v7_wrapper_has_no_identity_guard() {
        let fields = guard();
        let rendered = render_wrapper(
            "solstone",
            Path::new("/journal"),
            Path::new("/runtime/solstone"),
            &fields,
        );
        let legacy = rendered
            .replace(WRAPPER_MARKER, "# managed-version: 7")
            .replace(&wrapper_guard_lines(&fields), "");
        assert_eq!(parse_wrapper(&legacy).expect("parse v7 wrapper").version, 7);
        assert_eq!(
            solstone_core_installation_identity::parse_wrapper_guard(&legacy),
            Ok(None)
        );
    }
    #[test]
    fn backup_default_is_durable_under_home_and_collision_names_increment() {
        let env = WrapperEnvironment {
            home_dir: PathBuf::from("/ignored"),
            curdir: PathBuf::new(),
            executable_dir: PathBuf::new(),
            backup_dir: None,
            legacy_replacement: false,
        };
        assert_eq!(
            env.backup_dir(),
            PathBuf::from("/ignored/.local/share/solstone/setup-backups")
        );
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
        let error = provision_wrappers(&env, Path::new("/bad$journal"), &binding()).unwrap_err();
        assert!(error.to_string().contains('$'));
        assert!(!wrapper_paths(&env.home_dir).solstone.exists());
    }
    #[test]
    fn dangling_alias_is_refused_and_left_untouched() {
        let root = root("dangling");
        let env = environment(&root);
        fs::create_dir_all(&env.curdir).unwrap();
        fs::create_dir_all(env.home_dir.join(".local/bin")).unwrap();
        symlink("/missing-target", wrapper_paths(&env.home_dir).solstone).unwrap();
        assert!(provision_wrappers(&env, Path::new("/journal"), &binding()).is_err());
        assert_eq!(
            fs::read_link(wrapper_paths(&env.home_dir).solstone).unwrap(),
            PathBuf::from("/missing-target")
        );
        assert!(!env.backup_dir().exists());
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
            render_wrapper("solstone", Path::new("/journal"), &expected, &guard()),
        )
        .unwrap();
        assert_eq!(
            check_alias(&environment, "solstone").unwrap().0,
            AliasState::Owned
        );
    }

    /// After `install.sh` flips `current` onto a newly installed sibling
    /// build, an existing wrapper's `SOL_BIN=` still names the *previous*
    /// build's `versions/<old>/bin` -- that is the version it was written
    /// for, and `expected_targets` alone only ever recognises this run's own
    /// `executable_dir`. Without recognising the sibling shape,
    /// `provision_wrappers` reads that as `AliasState::Foreign` and refuses
    /// to touch it, permanently stranding the wrapper on the old build.
    #[test]
    fn sibling_version_directory_target_is_recognized_as_owned() {
        let root = root("sibling-version-owned");
        let prefix = root.join("prefix");
        let old_bin = prefix.join("versions/1.0.0-aaaaaaaa/bin");
        let new_bin = prefix.join("versions/2.0.0-bbbbbbbb/bin");
        fs::create_dir_all(&old_bin).unwrap();
        fs::create_dir_all(&new_bin).unwrap();
        // `current` already reflects the flip install.sh just performed: the
        // running process's own executable_dir is the NEW build.
        symlink("versions/2.0.0-bbbbbbbb", prefix.join("current")).unwrap();

        let home = root.join("home");
        let curdir = root.join("repo");
        fs::create_dir_all(home.join(".local/bin")).unwrap();
        fs::create_dir_all(&curdir).unwrap();
        let environment = WrapperEnvironment {
            home_dir: home.clone(),
            curdir,
            executable_dir: new_bin,
            backup_dir: Some(root.join("backups")),
            legacy_replacement: false,
        };

        fs::write(
            wrapper_paths(&home).journal,
            render_wrapper(
                "journal",
                Path::new("/journal"),
                &old_bin.join("journal"),
                &guard(),
            ),
        )
        .unwrap();

        assert_eq!(
            check_alias(&environment, "journal").unwrap().0,
            AliasState::Owned,
            "a sibling version directory under the same installation prefix must not read as foreign"
        );
    }
    #[test]
    fn owned_aliases_are_backed_up_and_absent_aliases_are_not() {
        let root = root("states");
        let env = environment(&root);
        fs::create_dir_all(&env.curdir).unwrap();
        fs::create_dir_all(&env.executable_dir).unwrap();
        fs::create_dir_all(env.home_dir.join(".local/bin")).unwrap();
        fs::write(env.executable_dir.join("solstone"), "runtime").unwrap();
        fs::write(env.executable_dir.join("journal"), "runtime").unwrap();
        fs::write(
            wrapper_paths(&env.home_dir).solstone,
            render_wrapper(
                "solstone",
                Path::new("/old"),
                &env.executable_dir.join("solstone"),
                &guard(),
            ),
        )
        .unwrap();
        fs::write(
            wrapper_paths(&env.home_dir).journal,
            render_wrapper(
                "journal",
                Path::new("/old"),
                &env.executable_dir.join("journal"),
                &guard(),
            ),
        )
        .unwrap();
        provision_wrappers(&env, Path::new("/journal"), &binding()).unwrap();
        let backups = fs::read_dir(env.backup_dir())
            .unwrap()
            .flatten()
            .map(|entry| entry.file_name().to_string_lossy().into_owned())
            .collect::<Vec<_>>();
        assert_eq!(backups.len(), 2);
        assert!(
            backups
                .iter()
                .any(|name| name.starts_with("solstone.old-symlink-"))
        );
        assert!(
            backups
                .iter()
                .any(|name| name.starts_with("journal.old-symlink-"))
        );
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
        provision_wrappers(&absent, Path::new("/journal"), &binding()).unwrap();
        assert_eq!(
            fs::read_dir(absent.backup_dir()).unwrap().flatten().count(),
            2
        );
    }
    #[test]
    fn backup_failure_aborts_before_wrapper_replacement() {
        let root = root("backup-failure");
        let mut env = environment(&root);
        fs::create_dir_all(&env.curdir).unwrap();
        fs::create_dir_all(env.home_dir.join(".local/bin")).unwrap();
        let old = render_wrapper(
            "solstone",
            Path::new("/old"),
            &env.executable_dir.join("solstone"),
            &guard(),
        );
        fs::write(wrapper_paths(&env.home_dir).solstone, &old).unwrap();
        let blocked = root.join("blocked-backups");
        fs::write(&blocked, "not a directory").unwrap();
        env.backup_dir = Some(blocked);
        assert!(provision_wrappers(&env, Path::new("/journal"), &binding()).is_err());
        assert_eq!(
            fs::read_to_string(wrapper_paths(&env.home_dir).solstone).unwrap(),
            old
        );
        assert!(!wrapper_paths(&env.home_dir).journal.exists());
    }
    #[test]
    fn worktree_leaves_aliases_unwritten() {
        let root = root("worktree");
        let env = environment(&root);
        fs::create_dir_all(&env.curdir).unwrap();
        fs::write(env.curdir.join(".git"), "gitdir: x").unwrap();
        let paths = provision_wrappers(&env, Path::new("/journal"), &binding()).unwrap();
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
        provision_wrappers(&env, Path::new("/journal"), &binding()).unwrap();
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
    fn exact_v1_solstone_and_sol_are_backed_up_and_replaced_together() {
        let root = root("v1-sol-companion");
        let mut env = environment(&root);
        env.legacy_replacement = true;
        fs::create_dir_all(&env.curdir).unwrap();
        let legacy_bin = env.home_dir.join(".local/share/uv/tools/solstone/bin");
        let public_bin = env.home_dir.join(".local/bin");
        fs::create_dir_all(&public_bin).unwrap();
        for command in ["solstone", "sol"] {
            let launcher = legacy_bin.join(command);
            write_executable(&launcher, &legacy_python_launcher(command));
            symlink(&launcher, public_bin.join(command)).unwrap();
        }

        provision_wrappers(&env, Path::new("/journal"), &binding()).unwrap();

        assert!(
            fs::read_to_string(public_bin.join("solstone"))
                .unwrap()
                .contains(WRAPPER_MARKER)
        );
        assert!(
            fs::read_to_string(public_bin.join("journal"))
                .unwrap()
                .contains(WRAPPER_MARKER)
        );
        assert!(!public_bin.join("sol").exists());
        let backups = fs::read_dir(env.backup_dir())
            .unwrap()
            .flatten()
            .map(|entry| entry.file_name().to_string_lossy().into_owned())
            .collect::<Vec<_>>();
        assert!(
            backups
                .iter()
                .any(|name| name.starts_with("solstone.old-symlink-"))
        );
        assert!(
            backups
                .iter()
                .any(|name| name.starts_with("sol.old-symlink-"))
        );
    }

    #[test]
    fn legacy_referent_change_is_refused_without_rewriting_the_public_link() {
        let root = root("legacy-referent-race");
        let home = root.join("home");
        let legacy = home.join("v1/bin/solstone");
        let public = home.join(".local/bin/solstone");
        write_executable(&legacy, &legacy_python_launcher("solstone"));
        fs::create_dir_all(public.parent().unwrap()).unwrap();
        symlink(&legacy, &public).unwrap();
        let launcher = legacy_launcher::classify(&home, &public, "solstone")
            .unwrap()
            .unwrap();
        let snapshot = snapshot_path(&public).unwrap();
        fs::write(&legacy, "#!/bin/sh\nexit 9\n").unwrap();
        let error = write_wrappers_and_remove_with(
            &[(public.clone(), "new wrapper".into())],
            &[],
            &[(public.clone(), snapshot)],
            &[LegacyExpectation {
                home,
                path: public.clone(),
                command: "solstone".into(),
                launcher,
            }],
            &mut |from, to| fs::rename(from, to),
        )
        .unwrap_err();
        assert!(error.to_string().contains("changed before replacement"));
        assert!(public.is_symlink());
        assert_eq!(fs::read_link(&public).unwrap(), legacy);
        assert_eq!(fs::read_to_string(public).unwrap(), "#!/bin/sh\nexit 9\n");
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
    fn wrapper_lock_blocks_a_second_descriptor_until_the_first_write_lease_releases() {
        let root = root("lock-contention");
        let home = root.join("home");
        let paths = wrapper_paths(&home);
        fs::create_dir_all(paths.solstone.parent().unwrap()).unwrap();
        fs::write(&paths.solstone, "old-solstone").unwrap();
        fs::write(&paths.journal, "old-journal").unwrap();

        let first_lock = wrapper_lock(&home).expect("first distinct file descriptor lock");
        let rendezvous = Arc::new(Barrier::new(2));
        let (attempt_tx, attempt_rx) = mpsc::channel();
        let (done_tx, done_rx) = mpsc::channel();
        let worker_home = home.clone();
        let worker_paths = paths.clone();
        let worker_rendezvous = Arc::clone(&rendezvous);
        let worker = thread::spawn(move || {
            // The barrier is the rendezvous immediately before the separate
            // descriptor attempts its OS-level exclusive flock.
            worker_rendezvous.wait();
            attempt_tx.send(()).expect("tell owner about lock attempt");
            let _second_lock = wrapper_lock(&worker_home).expect("second descriptor lock");
            write_wrappers_atomically(&[
                (worker_paths.solstone, "new-solstone".into()),
                (worker_paths.journal, "new-journal".into()),
            ])
            .expect("write after lock release");
            done_tx.send(()).expect("tell owner about completed write");
        });

        rendezvous.wait();
        attempt_rx
            .recv_timeout(Duration::from_secs(1))
            .expect("worker reached its lock attempt");
        assert!(matches!(done_rx.try_recv(), Err(mpsc::TryRecvError::Empty)));
        assert_eq!(fs::read_to_string(&paths.solstone).unwrap(), "old-solstone");
        assert_eq!(fs::read_to_string(&paths.journal).unwrap(), "old-journal");

        drop(first_lock);
        done_rx
            .recv_timeout(Duration::from_secs(1))
            .expect("second writer completed after release");
        worker.join().expect("worker thread");
        assert_eq!(fs::read_to_string(&paths.solstone).unwrap(), "new-solstone");
        assert_eq!(fs::read_to_string(&paths.journal).unwrap(), "new-journal");
        assert!(
            fs::read_dir(paths.solstone.parent().unwrap())
                .unwrap()
                .flatten()
                .all(|entry| !entry.file_name().to_string_lossy().contains(".tmp-"))
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
