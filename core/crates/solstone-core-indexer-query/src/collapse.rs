// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::collections::BTreeSet;

use solstone_core_format::segment::segment_key;

use crate::IndexAccessError;
use crate::execute::{QueryConnection, SqlPlan};

/// Suppress segment aggregates only when their matching child is also present.
pub(super) fn collapse_redundant_segments(
    connection: &mut QueryConnection,
    mut plan: SqlPlan,
) -> Result<SqlPlan, IndexAccessError> {
    let aggregate_paths = connection.distinct_paths(&plan, "agent='segment'")?;
    if aggregate_paths.is_empty() {
        return Ok(plan);
    }

    let child_paths = connection.distinct_paths(&plan, "agent!='segment'")?;
    let child_parents: BTreeSet<String> = child_paths
        .into_iter()
        .filter_map(|path| {
            let parts: Vec<&str> = path.split('/').collect();
            (parts.len() >= 4 && segment_key(parts[2]).is_some()).then(|| parts[..3].join("/"))
        })
        .collect();
    let redundant: Vec<String> = aggregate_paths
        .into_iter()
        .filter(|path| child_parents.contains(path))
        .collect();
    if redundant.is_empty() {
        return Ok(plan);
    }

    let placeholders = std::iter::repeat_n("?", redundant.len())
        .collect::<Vec<_>>()
        .join(", ");
    plan.where_clause
        .push_str(&format!(" AND path NOT IN ({placeholders})"));
    connection.note_collapse_bind_count(redundant.len());
    plan.params.extend(redundant);
    Ok(plan)
}
