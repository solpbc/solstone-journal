// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::collections::BTreeMap;
use std::path::Path;

use solstone_core_segment::list_segments;

use super::attribution::observer_prefix_for_stream;
use super::identity::{
    analyze_segment, first_identity_difference, identity_key, is_last_physical_copy,
};
use super::types::{PruneCandidate, PruneGroup, PruneResult, Refusal, SegmentAnalysis};

/// Every same-start `(day, stream, HHMMSS start)` set with more than one
/// segment -- the ingest planner's collision-ladder candidate pool. Restricting
/// grouping to one start prevents two silent captures at different times with
/// identical bytes from ever being treated as duplicates.
pub fn same_start_sets(
    journal: &Path,
    days: &[String],
    stream: Option<&str>,
) -> Vec<Vec<SegmentAnalysis>> {
    let mut sets: BTreeMap<(String, String, String), Vec<SegmentAnalysis>> = BTreeMap::new();
    for day in days {
        let Ok(segments) = list_segments(journal, day) else {
            continue;
        };
        for segment in segments {
            if let Some(filter) = stream
                && !segment.stream().matches(filter)
            {
                continue;
            }
            let Some(identity) = segment.record_identity() else {
                continue;
            };
            let Some(start) = identity.key.split_once('_').map(|(start, _)| start) else {
                continue;
            };
            let analysis =
                analyze_segment(journal, day, identity.stream, identity.name, segment.path());
            sets.entry((day.clone(), identity.stream.to_owned(), start.to_owned()))
                .or_default()
                .push(analysis);
        }
    }
    sets.into_values().filter(|items| items.len() > 1).collect()
}

/// Byte-identical duplicate clusters (>1 member sharing a content identity
/// key) and, separately, the singleton "held" segments that do not match any
/// duplicate cluster's identity -- distinct recordings that must be left in
/// place.
fn duplicate_groups(
    analyses: &[SegmentAnalysis],
) -> (Vec<Vec<SegmentAnalysis>>, Vec<SegmentAnalysis>) {
    let mut by_key: BTreeMap<Vec<(String, String, u64)>, Vec<SegmentAnalysis>> = BTreeMap::new();
    for analysis in analyses {
        if analysis.identity_issue.is_some() {
            continue;
        }
        let key = identity_key(analysis.identity.as_ref().expect("no identity issue"));
        by_key.entry(key).or_default().push(analysis.clone());
    }
    let duplicate_groups: Vec<Vec<SegmentAnalysis>> = by_key
        .values()
        .filter(|items| items.len() > 1)
        .cloned()
        .collect();
    let duplicate_keys: std::collections::BTreeSet<_> = duplicate_groups
        .iter()
        .map(|items| identity_key(items[0].identity.as_ref().expect("no identity issue")))
        .collect();
    let singleton_mismatches: Vec<SegmentAnalysis> = by_key
        .into_iter()
        .filter(|(key, _)| !duplicate_keys.contains(key) && !duplicate_groups.is_empty())
        .map(|(_, items)| items[0].clone())
        .collect();
    (duplicate_groups, singleton_mismatches)
}

/// The comparison canonical used only to phrase singleton-mismatch refusal
/// messages: the largest duplicate cluster, tie-broken by earliest segment.
fn mismatch_comparison_canonical(duplicate_groups: &[Vec<SegmentAnalysis>]) -> SegmentAnalysis {
    let group = duplicate_groups
        .iter()
        .min_by_key(|items| {
            let earliest = items
                .iter()
                .map(|item| item.segment.clone())
                .min()
                .expect("nonempty group");
            (std::cmp::Reverse(items.len()), earliest)
        })
        .expect("nonempty duplicate_groups");
    group
        .iter()
        .min_by(|a, b| a.segment.cmp(&b.segment))
        .expect("nonempty group")
        .clone()
}

/// Plan (never write) a same-start prune: group duplicates, pick the earliest
/// held canonical within each duplicate cluster, and record every refusal.
pub fn plan(journal: &Path, days: &[String], stream: Option<&str>) -> PruneResult {
    let mut result = PruneResult::new(false);
    if let Err(refusal) = super::identity_preflight(journal, days, stream) {
        result.refusals.push(refusal);
        return result;
    }
    for analyses in same_start_sets(journal, days, stream) {
        let (duplicate_groups, singleton_mismatches) = duplicate_groups(&analyses);
        let identity_errors: Vec<&SegmentAnalysis> = analyses
            .iter()
            .filter(|analysis| analysis.identity_issue.is_some())
            .collect();
        if duplicate_groups.is_empty() {
            if !identity_errors.is_empty()
                && analyses
                    .iter()
                    .any(|analysis| analysis.identity_issue.is_none())
            {
                for analysis in identity_errors {
                    result
                        .refusals
                        .push(analysis.identity_issue.clone().expect("has issue"));
                }
            }
            continue;
        }
        if !identity_errors.is_empty() {
            for analysis in identity_errors {
                result
                    .refusals
                    .push(analysis.identity_issue.clone().expect("has issue"));
            }
            continue;
        }
        let mismatch_canonical = mismatch_comparison_canonical(&duplicate_groups);
        for mismatch in &singleton_mismatches {
            let diff = first_identity_difference(
                mismatch_canonical.identity.as_ref().expect("held"),
                mismatch.identity.as_ref().expect("held"),
            );
            result.refusals.push(Refusal::new(
                mismatch.label(),
                "content-identity",
                diff,
                format!(
                    "compared to canonical {}; leave it in place; only byte-identical same-start duplicates are pruned",
                    mismatch_canonical.segment
                ),
            ));
        }
        for group_analyses in duplicate_groups {
            let canonical = group_analyses
                .iter()
                .min_by(|a, b| a.segment.cmp(&b.segment))
                .expect("nonempty group")
                .clone();
            if let Some(marker_error) = &canonical.marker_error {
                result.refusals.push(Refusal::new(
                    canonical.label(),
                    "chain-identity",
                    Some("stream.json".to_owned()),
                    marker_error.clone(),
                ));
                continue;
            }
            if let Err(refusal) = observer_prefix_for_stream(journal, &canonical.stream) {
                result.refusals.push(refusal);
                continue;
            }
            let mut sorted_group = group_analyses.clone();
            sorted_group.sort_by(|a, b| a.segment.cmp(&b.segment));
            let mut safe_candidates = Vec::new();
            for analysis in sorted_group {
                if analysis.segment == canonical.segment {
                    continue;
                }
                if let Some(marker_error) = &analysis.marker_error {
                    result.refusals.push(Refusal::new(
                        analysis.label(),
                        "chain-identity",
                        Some("stream.json".to_owned()),
                        marker_error.clone(),
                    ));
                    continue;
                }
                if let Some(unknown) = analysis.unknown_files.first() {
                    result.refusals.push(Refusal::new(
                        analysis.label(),
                        "derived-output",
                        Some(unknown.clone()),
                        "remove the file or add a valid ingest manifest proving it is content",
                    ));
                    continue;
                }
                let last_physical_copy = is_last_physical_copy(
                    canonical.identity.as_ref().expect("held"),
                    &analysis.path,
                );
                safe_candidates.push(PruneCandidate {
                    analysis,
                    last_physical_copy,
                });
            }
            if !safe_candidates.is_empty() {
                result.groups.push(PruneGroup {
                    day: canonical.day.clone(),
                    stream: canonical.stream.clone(),
                    start: canonical.segment.split('_').next().unwrap_or("").to_owned(),
                    canonical,
                    candidates: safe_candidates,
                });
            }
        }
    }
    result
}
