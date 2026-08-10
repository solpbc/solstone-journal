// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use solstone_core_indexer_store::db::prune_by_paths;
use solstone_core_journal_io::remove_dir_all;

use super::attribution::observer_prefix_for_stream;
use super::chain::{repair_stream_chain, repair_stream_registry_state};
use super::history::append_pruned_once;
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
            append_pruned_once(
                journal,
                &prefix,
                &analysis.day,
                &analysis.stream,
                &analysis.segment,
                &group.canonical.segment,
                now_ms,
            );
            let Ok(rel) = analysis.path.strip_prefix(journal) else {
                result.refusals.push(Refusal::new(
                    analysis.label(),
                    "delete",
                    Some(analysis.segment.clone()),
                    "segment path is not inside the journal; fix the filesystem error and rerun prune",
                ));
                break;
            };
            if let Err(error) = remove_dir_all(journal, &rel.to_string_lossy()) {
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
        touch_stream_health_marker(journal, day);
    }
}

fn touch_stream_health_marker(journal: &Path, day: &str) {
    let path = journal
        .join("chronicle")
        .join(day)
        .join("health")
        .join("stream.updated");
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let _ = std::fs::write(&path, b"");
}
