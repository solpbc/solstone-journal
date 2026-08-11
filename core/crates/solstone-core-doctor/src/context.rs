// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use crate::vocabulary::Platform;
use solstone_core_journal::{discover_home, read_config_journal, resolve_journal_path};
use std::env;
use std::path::PathBuf;
use std::time::Duration;
#[derive(Debug, Clone)]
pub struct CheckContext {
    pub home_dir: PathBuf,
    pub install_bin_dir: PathBuf,
    pub journal_path: PathBuf,
    pub callosum_socket_path: PathBuf,
    pub platform: Platform,
    pub port: u16,
    pub service_status_timeout: Duration,
    pub service_status_command_override: Option<(PathBuf, Vec<String>)>,
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
        let journal_path = resolve_journal_path(
            env::var_os("SOLSTONE_JOURNAL").as_deref(),
            config.as_deref(),
            solstone_core_journal::detect_checkout_root(
                &env::current_dir().map_err(|e| e.to_string())?,
            )
            .as_deref(),
            &home_dir,
        )
        .path;
        let platform = if cfg!(target_os = "macos") {
            Platform::Darwin
        } else {
            Platform::Linux
        };
        Ok(Self {
            callosum_socket_path: journal_path.join("health/callosum.sock"),
            home_dir,
            install_bin_dir,
            journal_path,
            platform,
            port,
            service_status_timeout: Duration::from_secs(10),
            service_status_command_override: None,
        })
    }
}
