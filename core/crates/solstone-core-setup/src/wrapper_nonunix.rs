// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Non-Unix boundary for managed shell wrappers.
//!
//! A Windows package exposes its executable entry points directly. It must not
//! create, replace, remove, or adopt the POSIX shell wrappers used by Unix
//! installs. This module nevertheless preserves the shared wrapper API so the
//! read-only setup identity flow compiles on every supported platform.

use std::io;
use std::path::{Path, PathBuf};

use solstone_core_installation_identity::{
    GuardFields, InstallationBinding, parse_wrapper_guard, wrapper_guard_lines,
};

pub const WRAPPER_MARKER: &str = "# managed-version: 8";
pub const WRAPPER_VERSION: u8 = 8;

const UNSUPPORTED: &str = "POSIX wrapper provisioning is unsupported on this platform";

/// Command names the Unix wrapper implementation supports.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WrapperCommand {
    Sol,
    Solstone,
    Journal,
}

impl WrapperCommand {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Sol => "sol",
            Self::Solstone => "solstone",
            Self::Journal => "journal",
        }
    }
}

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
    pub guard: Option<GuardFields>,
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
    pub backup_dir: Option<PathBuf>,
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

pub fn render_wrapper(
    command: WrapperCommand,
    journal: &Path,
    sol_bin: &Path,
    guard: &GuardFields,
) -> Result<String, WrapperError> {
    if command == WrapperCommand::Sol {
        return Err(WrapperError(
            "the current wrapper format does not publish the retired sol command".into(),
        ));
    }
    if guard.journal_token.to_path_buf() != journal {
        return Err(WrapperError(
            "wrapper journal does not match its installation identity guard".into(),
        ));
    }
    Ok(format!(
        "#!/bin/bash\n# {} — managed by 'journal config'. Edits will be overwritten.\n{WRAPPER_MARKER}\n{}: \"${{SOLSTONE_JOURNAL:={}}}\"\nexport SOLSTONE_JOURNAL\nSOL_BIN='{}'\nexec \"$SOL_BIN\" \"$@\"\n",
        command.as_str(),
        wrapper_guard_lines(guard),
        journal.to_string_lossy(),
        sol_bin.to_string_lossy().replace('\'', "'\\''"),
    ))
}

#[must_use]
pub fn parse_wrapper(command: WrapperCommand, content: &str) -> Option<ParsedWrapper> {
    let marker = content
        .lines()
        .find(|line| line.starts_with("# managed-version: "))?;
    let version = marker
        .strip_prefix("# managed-version: ")?
        .parse::<u8>()
        .ok()?;
    if !matches!(version, 7 | 8) || (version == 8 && command == WrapperCommand::Sol) {
        return None;
    }
    let journal = content
        .lines()
        .find_map(|line| {
            line.strip_prefix(": \"${SOLSTONE_JOURNAL:={")
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
    let guard = if version == 8 {
        parse_wrapper_guard(content).ok()?
    } else {
        None
    };
    if guard
        .as_ref()
        .is_some_and(|guard| guard.journal_token.as_bytes() != journal.as_bytes())
    {
        return None;
    }
    Some(ParsedWrapper {
        journal,
        sol_bin: PathBuf::from(sol_bin),
        version,
        guard,
    })
}

pub fn check_alias(
    environment: &WrapperEnvironment,
    binary: &str,
) -> Result<(AliasState, Option<PathBuf>), WrapperError> {
    if environment.curdir.join(".git").is_file() {
        return Ok((AliasState::Worktree, None));
    }
    let paths = wrapper_paths(&environment.home_dir);
    let path = if binary == "solstone" {
        paths.solstone
    } else {
        paths.journal
    };
    if !path.exists() && !path.is_symlink() {
        Ok((AliasState::Absent, None))
    } else {
        Ok((AliasState::Foreign, None))
    }
}

pub fn legacy_backup_path(
    _binary: &str,
    _directory: &Path,
    _timestamp: &str,
) -> Result<PathBuf, WrapperError> {
    Err(WrapperError(UNSUPPORTED.into()))
}

pub fn write_wrappers_atomically_with<F>(
    _contents: &[(PathBuf, String)],
    _replace: F,
) -> Result<(), WrapperError>
where
    F: FnMut(&Path, &Path) -> io::Result<()>,
{
    Err(WrapperError(UNSUPPORTED.into()))
}

pub fn write_wrappers_atomically(_contents: &[(PathBuf, String)]) -> Result<(), WrapperError> {
    Err(WrapperError(UNSUPPORTED.into()))
}

pub struct WrapperLock;

pub fn wrapper_lock(_home_dir: &Path) -> Result<WrapperLock, WrapperError> {
    Err(WrapperError(UNSUPPORTED.into()))
}

pub fn validate_journal_path_for_wrapper(_journal: &Path) -> Result<(), WrapperError> {
    Ok(())
}

/// Windows packages do not create POSIX wrappers. The setup preflight keeps
/// this no-op so the shared lifecycle flow can reach its Windows skip step.
pub fn validate_wrapper_pair(_journal: &Path, _executable_dir: &Path) -> Result<(), WrapperError> {
    Ok(())
}

pub fn provision_wrappers(
    _environment: &WrapperEnvironment,
    _journal: &Path,
    _binding: &InstallationBinding,
) -> Result<WrapperPaths, WrapperError> {
    Err(WrapperError(UNSUPPORTED.into()))
}

pub fn uninstall_wrappers(
    environment: &WrapperEnvironment,
) -> Result<(), (AliasState, Option<PathBuf>)> {
    let paths = wrapper_paths(&environment.home_dir);
    if [&paths.solstone, &paths.journal]
        .iter()
        .any(|path| path.exists() || path.is_symlink())
    {
        Err((AliasState::Foreign, None))
    } else {
        Ok(())
    }
}

#[must_use]
pub fn is_live_app_owned_child_launcher(_path: &Path) -> bool {
    false
}

pub fn ensure_user_bin_on_path(_home_dir: &Path) -> String {
    "Windows packages expose solstone and journal directly; no POSIX PATH wrapper was installed."
        .into()
}
