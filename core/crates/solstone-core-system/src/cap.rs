// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::collections::BTreeMap;
use std::time::Duration;

use crate::partition::Partition;

/// Python's default wall-clock task budget when no partition cap is registered.
pub const DEFAULT_TASK_MAX_RUNTIME: Duration = Duration::from_secs(1_800);

/// Resolve a task partition's wall-clock budget.
pub trait CapResolver {
    fn cap_for(&self, partition: &Partition) -> Duration;
}

/// A caller-configured override map with Python-compatible fallback behavior.
#[derive(Debug, Clone)]
pub struct DefaultCapResolver {
    default: Duration,
    overrides: BTreeMap<Partition, Duration>,
}

impl DefaultCapResolver {
    pub fn new(default: Duration) -> Self {
        Self {
            default,
            overrides: BTreeMap::new(),
        }
    }

    pub fn set_override(&mut self, partition: Partition, cap: Duration) {
        self.overrides.insert(partition, cap);
    }
}

impl Default for DefaultCapResolver {
    fn default() -> Self {
        Self::new(DEFAULT_TASK_MAX_RUNTIME)
    }
}

impl CapResolver for DefaultCapResolver {
    fn cap_for(&self, partition: &Partition) -> Duration {
        self.overrides
            .get(partition)
            .copied()
            .filter(|cap| !cap.is_zero())
            .unwrap_or(self.default)
    }
}
