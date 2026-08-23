// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use solstone_core_indexer_store::db::prune_by_paths;
use solstone_core_journal_io::remove_contained_tree;
use solstone_core_segment::touch_stream_health_marker;

use super::super::history::load_history;
use super::super::paths::history_path;
use super::attribution::observer_prefix_for_stream;
use super::chain::{repair_stream_chain, repair_stream_registry_state};
use super::history::{append_pruned_once, torn_history_refusal};
use super::marker::StreamMarker;
use super::types::{PruneCandidate, PruneGroup, PruneResult, Refusal};

/// Delete every safe candidate in `groups`. The `pruned` history record is
/// appended BEFORE the directory is removed: if removal then fails, the
/// group stops loudly (this candidate and every later one in it are left
/// untouched) rather than losing the audit trail for a deletion that already
/// happened. A second, later run dedupes the existing record and converges.
pub fn execute_plan(
    journal: &Path,
    result: &mut PruneResult,
    groups: Vec<PruneGroup>,
    now_ms: i64,
) {
    let days: Vec<String> = groups.iter().map(|group| group.day.clone()).collect();
    let stream = groups.first().map(|group| group.stream.as_str());
    if let Err(refusal) = super::identity_preflight(journal, &days, stream) {
        result.refusals.push(refusal);
        return;
    }
    let mut affected_streams: BTreeSet<String> = BTreeSet::new();
    let mut affected_days: BTreeSet<String> = BTreeSet::new();
    let mut deleted_markers_by_stream: BTreeMap<String, BTreeMap<(String, String), StreamMarker>> =
        BTreeMap::new();

    for group in groups {
        let prefix = match observer_prefix_for_stream(journal, &group.stream) {
            Ok(prefix) => prefix,
            Err(refusal) => {
                result.refusals.push(refusal);
                continue;
            }
        };
        let deleted_markers = deleted_markers_by_stream
            .entry(group.stream.clone())
            .or_default();
        let mut days: BTreeSet<String> = group
            .candidates
            .iter()
            .map(|candidate| candidate.analysis.day.clone())
            .collect();
        days.insert(group.canonical.day.clone());
        let mut torn = false;
        for day in &days {
            if load_history(&history_path(journal, &prefix, day))
                .stopped
                .is_some()
            {
                result
                    .refusals
                    .push(torn_history_refusal(day, &group.stream));
                torn = true;
            }
        }
        if torn {
            continue;
        }
        for candidate in group.candidates {
            let analysis = candidate.analysis;
            let Some(marker) = analysis.marker.clone() else {
                result.refusals.push(Refusal::new(
                    analysis.label(),
                    "chain-identity",
                    Some("stream.json".to_owned()),
                    "restore a readable stream.json marker before pruning",
                ));
                break;
            };
            if let Err(refusal) = append_pruned_once(
                journal,
                &prefix,
                &analysis.day,
                &analysis.stream,
                &analysis.segment,
                &group.canonical.segment,
                now_ms,
            ) {
                result.refusals.push(refusal);
                break;
            }
            if analysis.path.strip_prefix(journal).is_err() {
                result.refusals.push(Refusal::new(
                    analysis.label(),
                    "delete",
                    Some(analysis.segment.clone()),
                    "segment path is not inside the journal; fix the filesystem error and rerun prune",
                ));
                break;
            }
            if let Err(error) = remove_contained_tree(journal, &analysis.path) {
                result.refusals.push(Refusal::new(
                    analysis.label(),
                    "delete",
                    Some(analysis.segment.clone()),
                    format!(
                        "delete failed after the pruned history record was written: {error}; fix the filesystem error and rerun prune"
                    ),
                ));
                break;
            }
            deleted_markers.insert((analysis.day.clone(), analysis.segment.clone()), marker);
            affected_streams.insert(analysis.stream.clone());
            affected_days.insert(analysis.day.clone());
            let index_rel = format!("{}/{}/{}", analysis.day, analysis.stream, analysis.segment);
            match prune_by_paths(journal, &[index_rel.as_str()]) {
                Ok(_) => {}
                Err(error) => {
                    result.index_errors.push(format!("{index_rel}: {error}"));
                }
            }
            result.deleted.push(PruneCandidate {
                analysis,
                last_physical_copy: candidate.last_physical_copy,
            });
        }
    }

    let empty_markers = BTreeMap::new();
    for stream in &affected_streams {
        let deleted_markers = deleted_markers_by_stream
            .get(stream)
            .unwrap_or(&empty_markers);
        let (refusals, repaired) = repair_stream_chain(journal, stream, deleted_markers, false);
        let had_refusals = !refusals.is_empty();
        result.refusals.extend(refusals);
        result.chain_repaired += repaired;
        if had_refusals {
            continue;
        }
        repair_stream_registry_state(journal, stream);
    }
    for day in &affected_days {
        let _ = touch_stream_health_marker(journal, day);
    }
}
