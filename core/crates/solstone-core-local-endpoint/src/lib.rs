// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! The desktop journal's per-account HTTP rendezvous. No journal data is read here.
#![cfg(any(target_os = "linux", target_os = "macos", windows))]

use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

#[cfg(any(target_os = "linux", target_os = "macos"))]
pub mod transport;
#[cfg(any(target_os = "linux", target_os = "macos"))]
mod unix;
#[cfg(any(target_os = "linux", target_os = "macos"))]
pub use unix::*;
#[cfg(windows)]
mod windows;
#[cfg(windows)]
pub use windows::*;

pub const SOCKET_ENV: &str = "SOLSTONE_JOURNAL_SOCKET";
pub const GENERATION_ENV: &str = "SOLSTONE_CONVEY_GENERATION";
pub const RUNTIME_ROUTE: &str = "/_solstone/local-runtime";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RuntimeRecord {
    pub generation: String,
    pub pid: u32,
    pub process_start_time: serde_json::Value,
    pub journal_root: PathBuf,
    pub tcp_port: u16,
    pub socket_path: PathBuf,
}

pub fn fresh_generation() -> String {
    format!(
        "{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos()
    )
}
