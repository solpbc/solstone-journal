// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::time::Duration;

use crate::partition::{Partition, partition_for};

/// Caps that apply before schedule-configured overrides.
pub fn baseline_cap_contributions() -> Vec<(Partition, Duration)> {
    [
        (
            vec!["solstone", "journal", "think"],
            Duration::from_secs(21_600),
        ),
        (
            vec!["solstone", "journal", "think", "--segment"],
            Duration::from_secs(4_500),
        ),
        (
            vec!["solstone", "journal", "indexer"],
            Duration::from_secs(7_200),
        ),
        (
            vec!["solstone", "journal", "importer"],
            Duration::from_secs(3_600),
        ),
        (
            vec!["solstone", "journal", "maintenance", "run", "backup:run"],
            Duration::from_secs(49 * 3_600),
        ),
    ]
    .into_iter()
    .map(|(cmd, duration)| {
        let cmd = cmd.into_iter().map(str::to_owned).collect::<Vec<_>>();
        (partition_for(&cmd), duration)
    })
    .collect()
}
