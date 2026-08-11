// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::ffi::OsStr;
use std::fs;
use std::path::{Path, PathBuf};
use std::{env, io};

use solstone_core_journal::{
    ConfigError, HomeError, ResolvedJournal, Source, detect_checkout_root, discover_home,
    installed_site_packages_from_executable_dir, read_config_journal,
    resolve_installation_root_from_executable_dir, resolve_journal_path,
};

#[derive(Debug)]
pub(crate) enum JournalResolutionError {
    Config,
    Home,
}

impl std::fmt::Display for JournalResolutionError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Config => formatter.write_str("config decode failed"),
            Self::Home => formatter.write_str("home unavailable"),
        }
    }
}

impl std::error::Error for JournalResolutionError {}

#[derive(Debug)]
pub(crate) enum ProjectRootError {
    CurrentExe(io::Error),
    Unclassified(PathBuf),
}

impl std::fmt::Display for ProjectRootError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::CurrentExe(error) => write!(
                formatter,
                "native journal project root resolution failed: could not inspect current executable: {error}"
            ),
            Self::Unclassified(executable) => write!(
                formatter,
                "native journal project root resolution failed: could not locate source checkout or installed solstone package from {}",
                executable.display()
            ),
        }
    }
}

impl std::error::Error for ProjectRootError {}

pub(crate) fn resolve_current_journal() -> Result<ResolvedJournal, JournalResolutionError> {
    let env_journal = env::var_os("SOLSTONE_JOURNAL");
    if let Some(path) = env_journal
        .as_deref()
        .filter(|value| *value != OsStr::new(""))
    {
        return Ok(ResolvedJournal {
            path: PathBuf::from(path),
            source: Source::Env,
        });
    }

    let home =
        discover_current_home().map_err(|HomeError::Unavailable| JournalResolutionError::Home)?;
    let config_journal =
        read_config_journal(&home).map_err(|ConfigError::Decode| JournalResolutionError::Config)?;
    let checkout_root = detect_current_checkout_root();
    Ok(resolve_journal_path(
        env_journal.as_deref(),
        config_journal.as_deref(),
        checkout_root.as_deref(),
        &home,
    ))
}

pub(crate) fn resolve_project_root() -> Result<PathBuf, ProjectRootError> {
    let executable = env::current_exe().map_err(ProjectRootError::CurrentExe)?;
    resolve_project_root_from_executable(&executable)
}

pub(crate) fn resolve_project_root_from_executable(
    executable: &Path,
) -> Result<PathBuf, ProjectRootError> {
    let Some(executable_dir) = executable.parent() else {
        return Err(ProjectRootError::Unclassified(executable.to_path_buf()));
    };
    resolve_installation_root_from_executable_dir(executable_dir)
        .ok_or_else(|| ProjectRootError::Unclassified(executable.to_path_buf()))
}

pub(crate) fn inspect_journal_days(journal: &Path) -> Result<Option<usize>, io::Error> {
    match fs::metadata(journal) {
        Ok(metadata) => {
            if !metadata.is_dir() {
                return Ok(None);
            }
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error),
    }

    let chronicle = journal.join("chronicle");
    match fs::metadata(&chronicle) {
        Ok(metadata) => {
            if !metadata.is_dir() {
                return Ok(Some(0));
            }
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Some(0)),
        Err(error) => return Err(error),
    }

    let mut days = 0;
    for entry in fs::read_dir(chronicle)? {
        let entry = entry?;
        if !entry.file_name().to_str().is_some_and(is_day_dir_name) {
            continue;
        }
        match entry.metadata() {
            Ok(metadata) if metadata.is_dir() => days += 1,
            Ok(_) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }
    }
    Ok(Some(days))
}

fn discover_current_home() -> Result<PathBuf, HomeError> {
    let home_env = env::var_os("HOME");
    if let Some(home) = home_env.as_deref() {
        return discover_home(Some(home), None);
    }
    let fallback = env::home_dir();
    discover_home(None, fallback.as_deref())
}

fn detect_current_checkout_root() -> Option<PathBuf> {
    let executable = env::current_exe().ok()?;
    let executable_dir = executable.parent()?;
    if installed_site_packages_from_executable_dir(executable_dir).is_some() {
        return None;
    }
    executable_dir.ancestors().find_map(detect_checkout_root)
}

fn is_day_dir_name(value: &str) -> bool {
    value.len() == 8 && value.bytes().all(|byte| byte.is_ascii_digit())
}
