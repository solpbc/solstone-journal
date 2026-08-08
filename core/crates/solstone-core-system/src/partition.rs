// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::path::Path;

/// Canonical queue and log partition for a command.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Partition(String);

impl Partition {
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl AsRef<str> for Partition {
    fn as_ref(&self) -> &str {
        self.as_str()
    }
}

/// Mirror the Python supervisor's ordered command partition resolver.
pub fn partition_for(cmd: &[String]) -> Partition {
    let Some(first) = cmd.first() else {
        return Partition::new("unknown");
    };

    if matches!(first.as_str(), "sol" | "journal") && cmd.len() > 1 {
        let mut name = cmd[1].clone();
        if name == "think" {
            // Order is a contract: the first matching mode wins.
            for (flag, mode) in [
                ("--activity", "activity"),
                ("--flush", "flush"),
                ("--segments", "segment"),
                ("--weekly", "weekly"),
                ("--cadence", "cadence"),
                ("--segment", "segment"),
            ] {
                if cmd.iter().any(|arg| arg == flag) {
                    name = mode.to_owned();
                    break;
                }
            }
            if name == "think" {
                name = "daily".to_owned();
            }
        } else if name == "maintenance" && cmd.len() >= 4 && cmd[2] == "run" {
            name = format!("maintenance:{}", cmd[3]);
        }
        return Partition::new(name);
    }

    Partition::new(
        Path::new(first)
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or(first)
            .to_owned(),
    )
}
