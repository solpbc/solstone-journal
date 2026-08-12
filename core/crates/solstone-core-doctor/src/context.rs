// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use crate::vocabulary::Platform;
use chrono::{DateTime, Utc};
use solstone_core_journal::{discover_home, read_config_journal, resolve_journal_path};
use std::env;
use std::path::{Path, PathBuf};
use std::time::Duration;

/// Test seam for the bounded durable Parakeet server state probe.
pub type ParakeetServerProbe = fn(&Path, Duration) -> Result<(), String>;
/// Test seam for the native speakers-analyze installation resolvers.
pub type SpeakersBinaryResolver = fn() -> Result<PathBuf, String>;
/// Test seam for native speakers-analyze model resolution.
pub type SpeakersModelResolver =
    fn(&str) -> Result<PathBuf, solstone_core_transcribe::TranscribeError>;

#[derive(Debug, Clone)]
pub struct CheckContext {
    pub home_dir: PathBuf,
    pub install_bin_dir: PathBuf,
    pub journal_path: PathBuf,
    pub callosum_socket_path: PathBuf,
    pub platform: Platform,
    pub now: DateTime<Utc>,
    pub host_arch: String,
    pub hostname: String,
    pub machine_id: Option<String>,
    pub checkout_root: Option<PathBuf>,
    pub python_env_root: Option<PathBuf>,
    pub port: u16,
    pub service_status_timeout: Duration,
    pub service_status_command_override: Option<(PathBuf, Vec<String>)>,
    pub parakeet_server_probe_override: Option<ParakeetServerProbe>,
    pub speakers_analyze_resolvers: Option<(SpeakersBinaryResolver, SpeakersModelResolver)>,
}
impl CheckContext {
    pub fn production(port: u16) -> Result<Self, String> {
        let executable = env::current_exe().map_err(|error| error.to_string())?;
        let install_bin_dir = executable
            .parent()
            .ok_or_else(|| {
                format!(
                    "could not determine executable directory: {}",
                    executable.display()
                )
            })?
            .to_path_buf();
        let home_dir =
            discover_home(env::var_os("HOME").as_deref(), None).map_err(|e| format!("{e:?}"))?;
        let config = read_config_journal(&home_dir).map_err(|e| format!("{e:?}"))?;
        let executable = env::current_exe().map_err(|error| error.to_string())?;
        let cwd = env::current_dir().map_err(|error| error.to_string())?;
        let checkout_root = checkout_ancestor(&executable).or_else(|| checkout_ancestor(&cwd));
        let journal_path = resolve_journal_path(
            env::var_os("SOLSTONE_JOURNAL").as_deref(),
            config.as_deref(),
            checkout_root.as_deref(),
            &home_dir,
        )
        .path;
        let platform = if cfg!(target_os = "macos") {
            Platform::Darwin
        } else {
            Platform::Linux
        };
        let hostname = solstone_core_system::lifecycle::sanitize_hostname(
            &std::env::var("HOSTNAME").unwrap_or_else(|_| "unknown".to_owned()),
        );
        let machine_id = solstone_core_system::lifecycle::machine_id();
        Ok(Self {
            callosum_socket_path: journal_path.join("health/callosum.sock"),
            home_dir,
            install_bin_dir,
            journal_path,
            platform,
            now: Utc::now(),
            host_arch: env::consts::ARCH.to_owned(),
            hostname,
            machine_id: (!machine_id.is_empty()).then_some(machine_id),
            python_env_root: resolve_python_env(checkout_root.as_deref(), &executable),
            checkout_root,
            port,
            service_status_timeout: Duration::from_secs(10),
            service_status_command_override: None,
            parakeet_server_probe_override: None,
            speakers_analyze_resolvers: None,
        })
    }
}

fn checkout_ancestor(path: &Path) -> Option<PathBuf> {
    let start = if path.is_dir() { path } else { path.parent()? };
    start.ancestors().find_map(|candidate| {
        solstone_core_journal::detect_checkout_root(candidate).filter(|root| {
            root.join("solstone/talent/sol/SKILL.md").is_file()
                && root.join("solstone/talent/journal/SKILL.md").is_file()
        })
    })
}

fn resolve_python_env(checkout_root: Option<&Path>, executable: &Path) -> Option<PathBuf> {
    if let Some(root) = checkout_root {
        let environment = root.join(".venv");
        if environment.is_dir() {
            return Some(environment);
        }
    }
    let prefix = executable.parent()?.parent()?;
    python_site_packages(prefix).then(|| prefix.to_path_buf())
}

fn python_site_packages(prefix: &Path) -> bool {
    let Ok(lib) = std::fs::read_dir(prefix.join("lib")) else {
        return false;
    };
    lib.filter_map(Result::ok).any(|entry| {
        entry.file_name().to_string_lossy().starts_with("python")
            && (entry.path().join("site-packages").is_dir()
                || entry.path().join("dist-packages").is_dir())
    })
}
