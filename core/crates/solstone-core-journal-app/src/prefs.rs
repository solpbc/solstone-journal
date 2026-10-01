// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! What the app remembers between runs: its update settings and the mark it
//! last drew as its icon. It lives beside the journal's own per-user state,
//! outside the install root, so an update or reinstall leaves it alone.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum CheckInterval {
    Day,
    #[default]
    Week,
    Month,
}

impl CheckInterval {
    pub fn seconds(self) -> u64 {
        match self {
            Self::Day => 86_400,
            Self::Week => 604_800,
            Self::Month => 2_592_000,
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(default)]
pub struct Prefs {
    pub auto_check: bool,
    pub auto_download: bool,
    pub interval: CheckInterval,
    pub last_checked_at: Option<u64>,
    /// The mark the app's icon was last drawn from, so a launch can put the
    /// icon back without drawing it again.
    pub icon_mark: Option<Value>,
}

impl Default for Prefs {
    fn default() -> Self {
        Self {
            auto_check: true,
            auto_download: false,
            interval: CheckInterval::Week,
            last_checked_at: None,
            icon_mark: None,
        }
    }
}

/// `%LOCALAPPDATA%\solstone-journal`, the journal's per-user state directory.
pub fn state_dir() -> PathBuf {
    std::env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir)
        .join("solstone-journal")
}

fn prefs_path() -> PathBuf {
    state_dir().join("journal-app.json")
}

/// The icon file drawn from the journal's mark.
pub fn mark_icon_path() -> PathBuf {
    state_dir().join("journal-mark.ico")
}

/// Where the window's web view keeps its own data.
pub fn webview_dir() -> PathBuf {
    state_dir().join("journal-app-webview")
}

pub fn load() -> Prefs {
    std::fs::read(prefs_path())
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or_default()
}

pub fn save(prefs: &Prefs) {
    let path = prefs_path();
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if let Ok(bytes) = serde_json::to_vec_pretty(prefs) {
        let temporary = path.with_extension("json.tmp");
        if std::fs::write(&temporary, bytes).is_ok() {
            let _ = std::fs::rename(&temporary, &path);
        }
    }
}
