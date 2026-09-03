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
/// Test seam for the VAD helper launch probe (binary path and deadline).
#[derive(Debug, Clone)]
pub struct VadRuntimeProbeSeam {
    pub binary: PathBuf,
    pub timeout: Duration,
}
/// Test seam that returns a VAD helper path and probe deadline.
pub type VadRuntimeProbeResolver = fn() -> VadRuntimeProbeSeam;

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
    /// The repository root, when the doctor is running from a source checkout.
    /// This is what the developer journal is resolved against.
    pub checkout_root: Option<PathBuf>,
    /// The shipped payload's root inside that checkout — a different directory
    /// from `checkout_root`, because the payload does not live in the package
    /// tree. Anything joining `solstone/...` wants this one.
    pub payload_root: Option<PathBuf>,
    pub port: u16,
    pub service_status_timeout: Duration,
    pub service_status_command_override: Option<(PathBuf, Vec<String>)>,
    pub parakeet_server_probe_override: Option<ParakeetServerProbe>,
    pub speakers_analyze_resolvers: Option<(SpeakersBinaryResolver, SpeakersModelResolver)>,
    /// Test seam for the VAD helper binary and probe deadline.
    pub vad_runtime_probe: Option<VadRuntimeProbeResolver>,
    /// Test seam for available bytes on the installation filesystem.
    pub free_space_bytes_override: Option<u64>,
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
        let (checkout_root, payload_root) =
            match checkout_ancestor(&executable).or_else(|| checkout_ancestor(&cwd)) {
                Some((repository, payload)) => (Some(repository), Some(payload)),
                None => (None, None),
            };
        let journal_path = resolve_journal_path(
            env::var_os("SOLSTONE_JOURNAL").as_deref(),
            config.as_deref(),
            checkout_root.as_deref(),
            &home_dir,
        )
        .path;
        let platform = if cfg!(target_os = "macos") {
            Platform::Darwin
        } else if cfg!(windows) {
            Platform::Windows
        } else {
            Platform::Linux
        };
        let hostname = solstone_core_system::lifecycle::sanitize_hostname(
            &std::env::var("HOSTNAME").unwrap_or_else(|_| "unknown".to_owned()),
        );
        Ok(Self {
            callosum_socket_path: journal_path.join("health/callosum.sock"),
            home_dir,
            install_bin_dir,
            journal_path,
            platform,
            now: Utc::now(),
            host_arch: env::consts::ARCH.to_owned(),
            hostname,
            checkout_root,
            payload_root,
            port,
            service_status_timeout: Duration::from_secs(10),
            service_status_command_override: None,
            parakeet_server_probe_override: None,
            speakers_analyze_resolvers: None,
            vad_runtime_probe: None,
            free_space_bytes_override: None,
        })
    }
}

/// The repository root and its payload root, together.
///
/// These were one value until the payload left the package tree. Keeping them
/// together here is deliberate: the router-skill sources are found under the
/// payload root, and the developer journal under the repository root, and
/// resolving one from the other after the fact is how the two get confused.
fn checkout_ancestor(path: &Path) -> Option<(PathBuf, PathBuf)> {
    let start = if path.is_dir() { path } else { path.parent()? };
    start.ancestors().find_map(|candidate| {
        let repository = solstone_core_journal::detect_checkout_root(candidate)?;
        let payload = solstone_core_journal::payload_root_in_checkout(&repository)?;
        (payload.join("solstone/talent/solstone/SKILL.md").is_file()
            && payload.join("solstone/talent/journal/SKILL.md").is_file())
        .then_some((repository, payload))
    })
}
