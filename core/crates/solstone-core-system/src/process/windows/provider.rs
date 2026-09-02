// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Retained ownership for long-lived providers shipped in a signed Windows
//! package.
//!
//! A provider is not a bounded request/response helper: it remains owned while
//! the service is ready. It nevertheless has the same non-negotiable launch
//! boundary: canonical package paths, a replacement environment, an atomic
//! kill-on-close Job, and optional limits configured before its first
//! instruction. The provider-specific runtime remains responsible for its
//! authenticated protocol and readiness proof before publishing a port.

use std::collections::BTreeMap;
use std::ffi::OsString;
use std::path::PathBuf;

use thiserror::Error;

use super::super::SpawnOptions;

/// Optional limits installed before an independent provider can execute.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct IndependentProviderResourceLimits {
    pub cpu_rate_per_10_000: u32,
    pub committed_memory_bytes: usize,
}

/// One package-rooted independent-provider launch.
///
/// `environment` is a complete replacement environment. It must include a
/// nonempty `SystemRoot`, and may not contain `PATH`; the owner writes an
/// explicit empty `PATH` entry so Windows cannot synthesize a parent search
/// path when the caller omits it.
#[cfg_attr(not(windows), allow(dead_code))]
pub struct IndependentProviderRequest {
    pub package_root: PathBuf,
    pub executable: PathBuf,
    pub current_directory: PathBuf,
    pub arguments: Vec<String>,
    pub environment: BTreeMap<OsString, OsString>,
    pub resource_limits: Option<IndependentProviderResourceLimits>,
    pub spawn_options: SpawnOptions,
}

/// A refusal before a provider can acquire a retained Job authority.
#[derive(Debug, Error, Eq, PartialEq)]
#[cfg_attr(not(windows), allow(dead_code))]
pub enum IndependentProviderError {
    #[error("independent provider environment must contain a nonempty SystemRoot")]
    MissingSystemRoot,
    #[error("independent provider environment may not contain PATH")]
    PathEnvironmentRefused,
    #[error("independent provider CPU rate must be within 1..=10000")]
    InvalidCpuRate,
    #[error("independent provider committed-memory ceiling must be nonzero")]
    InvalidCommittedMemoryLimit,
    #[error("independent provider package root could not be canonicalized")]
    PackageRootUnavailable,
    #[error("independent provider executable could not be canonicalized")]
    ExecutableUnavailable,
    #[error("independent provider executable is outside its package root")]
    ExecutableOutsidePackage,
    #[error("independent provider executable is not a regular file")]
    ExecutableNotFile,
    #[error("independent provider current directory could not be canonicalized")]
    CurrentDirectoryUnavailable,
    #[error("independent provider current directory is outside its package root")]
    CurrentDirectoryOutsidePackage,
    #[error("independent provider current directory is not a directory")]
    CurrentDirectoryNotDirectory,
    #[error(
        "independent provider path cannot be represented by the managed Windows command boundary"
    )]
    PathNotRepresentable,
    #[error("independent provider failed before a retained Job authority was available")]
    LaunchFailed,
}

fn validate_request_shape(
    request: &IndependentProviderRequest,
) -> Result<(), IndependentProviderError> {
    let mut has_system_root = false;
    for (key, value) in &request.environment {
        let key = key.to_string_lossy();
        if key.eq_ignore_ascii_case("path") {
            return Err(IndependentProviderError::PathEnvironmentRefused);
        }
        if key.eq_ignore_ascii_case("systemroot") && !value.is_empty() {
            has_system_root = true;
        }
    }
    if !has_system_root {
        return Err(IndependentProviderError::MissingSystemRoot);
    }
    if let Some(limits) = request.resource_limits {
        if !(1..=10_000).contains(&limits.cpu_rate_per_10_000) {
            return Err(IndependentProviderError::InvalidCpuRate);
        }
        if limits.committed_memory_bytes == 0 {
            return Err(IndependentProviderError::InvalidCommittedMemoryLimit);
        }
    }
    Ok(())
}

#[cfg(windows)]
struct CanonicalProviderRequest {
    executable: String,
    current_directory: PathBuf,
}

#[cfg(windows)]
fn canonicalize_request(
    request: &IndependentProviderRequest,
) -> Result<CanonicalProviderRequest, IndependentProviderError> {
    validate_request_shape(request)?;
    let package_root = std::fs::canonicalize(&request.package_root)
        .map_err(|_| IndependentProviderError::PackageRootUnavailable)?;
    let executable = std::fs::canonicalize(&request.executable)
        .map_err(|_| IndependentProviderError::ExecutableUnavailable)?;
    if !executable.starts_with(&package_root) {
        return Err(IndependentProviderError::ExecutableOutsidePackage);
    }
    if !executable.is_file() {
        return Err(IndependentProviderError::ExecutableNotFile);
    }
    let current_directory = std::fs::canonicalize(&request.current_directory)
        .map_err(|_| IndependentProviderError::CurrentDirectoryUnavailable)?;
    if !current_directory.starts_with(&package_root) {
        return Err(IndependentProviderError::CurrentDirectoryOutsidePackage);
    }
    if !current_directory.is_dir() {
        return Err(IndependentProviderError::CurrentDirectoryNotDirectory);
    }
    let executable = executable
        .into_os_string()
        .into_string()
        .map_err(|_| IndependentProviderError::PathNotRepresentable)?;
    Ok(CanonicalProviderRequest {
        executable,
        current_directory,
    })
}

/// Acquire retained authority over one package-owned Windows provider.
///
/// The returned authority owns the full process tree and its managed log
/// drains. A caller must retain it until its authenticated readiness proof has
/// either completed or been stopped, and must use `terminate` for all stop,
/// restart, update, and logoff paths.
#[cfg(windows)]
pub fn launch_independent_provider(
    request: IndependentProviderRequest,
) -> Result<super::managed::LaunchAuthority, IndependentProviderError> {
    use super::super::Disposition;
    use super::managed::{ManagedProcess, launch_managed};

    let canonical = canonicalize_request(&request)?;
    let mut command = Vec::with_capacity(request.arguments.len() + 1);
    command.push(canonical.executable);
    command.extend(request.arguments);
    let mut options = request.spawn_options;
    options.environment = request.environment;
    // Windows supplies a process PATH if the custom block omits it. Force the
    // empty value only after refusing any caller-selected PATH.
    options
        .environment
        .insert(OsString::from("PATH"), OsString::new());
    let limits = request
        .resource_limits
        .map(|limits| (limits.cpu_rate_per_10_000, limits.committed_memory_bytes));
    launch_managed(Disposition::IndependentLongLived, move || {
        ManagedProcess::spawn_package_owned(command, options, &canonical.current_directory, limits)
    })
    .map_err(|_| IndependentProviderError::LaunchFailed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    fn request() -> IndependentProviderRequest {
        IndependentProviderRequest {
            package_root: PathBuf::from("package"),
            executable: PathBuf::from("package/provider.exe"),
            current_directory: PathBuf::from("package"),
            arguments: Vec::new(),
            environment: BTreeMap::from([(
                OsString::from("SystemRoot"),
                OsString::from("C:\\Windows"),
            )]),
            resource_limits: None,
            spawn_options: SpawnOptions {
                journal_root: PathBuf::from("journal"),
                reference: "provider-test".to_owned(),
                day: None,
                sink: None::<Arc<dyn crate::process::ProcessEventSink>>,
                environment: BTreeMap::new(),
            },
        }
    }

    #[test]
    fn request_shape_requires_system_root_and_refuses_path() {
        let mut candidate = request();
        candidate.environment.clear();
        assert_eq!(
            validate_request_shape(&candidate),
            Err(IndependentProviderError::MissingSystemRoot)
        );

        let mut candidate = request();
        candidate
            .environment
            .insert(OsString::from("Path"), OsString::from("C:\\poison"));
        assert_eq!(
            validate_request_shape(&candidate),
            Err(IndependentProviderError::PathEnvironmentRefused)
        );
    }

    #[test]
    fn request_shape_requires_nonzero_prelaunch_resource_limits() {
        let mut candidate = request();
        candidate.resource_limits = Some(IndependentProviderResourceLimits {
            cpu_rate_per_10_000: 0,
            committed_memory_bytes: 1,
        });
        assert_eq!(
            validate_request_shape(&candidate),
            Err(IndependentProviderError::InvalidCpuRate)
        );

        let mut candidate = request();
        candidate.resource_limits = Some(IndependentProviderResourceLimits {
            cpu_rate_per_10_000: 1,
            committed_memory_bytes: 0,
        });
        assert_eq!(
            validate_request_shape(&candidate),
            Err(IndependentProviderError::InvalidCommittedMemoryLimit)
        );
    }
}
