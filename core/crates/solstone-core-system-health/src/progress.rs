// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::collections::{BTreeMap, BTreeSet};

use crate::event::HealthEvent;
use crate::read::read_day_records;
use crate::{FoldRead, HealthError, HealthLogSource, SegmentIdentity, SegmentProgress};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ProgressKind {
    Dispatch,
    Complete,
    Fail,
    Capped,
    NotRecommended,
}

#[derive(Debug, Clone)]
struct ProgressRecord {
    ts: i64,
    sequence: usize,
    kind: ProgressKind,
    use_id: Option<String>,
}

pub fn read_segment_progress<S: HealthLogSource>(
    source: &S,
    day: &str,
) -> Result<FoldRead<BTreeMap<SegmentIdentity, SegmentProgress>>, HealthError> {
    let scanned = read_day_records(source, day)?;
    let mut latest_sense: BTreeMap<SegmentIdentity, (i64, Option<String>)> = BTreeMap::new();
    let mut latest_change: BTreeMap<SegmentIdentity, (i64, Option<String>)> = BTreeMap::new();
    let mut records: BTreeMap<SegmentIdentity, BTreeMap<String, Vec<ProgressRecord>>> =
        BTreeMap::new();
    let mut unconfigured: BTreeMap<SegmentIdentity, BTreeSet<String>> = BTreeMap::new();
    let mut sequence = 0;
    for record in scanned.value {
        let Some(payload) = record.event.payload() else {
            continue;
        };
        if payload.mode.as_deref() != Some("segment") {
            continue;
        }
        let Some(segment) = payload.segment.clone().filter(|value| !value.is_empty()) else {
            continue;
        };
        let key = SegmentIdentity {
            stream: payload.stream.clone(),
            segment,
        };
        match &record.event {
            HealthEvent::SenseComplete(_) => {
                update_latest(&mut latest_sense, key, record.ts, payload.density.clone())
            }
            HealthEvent::SenseChangeDetect(_) => update_latest(
                &mut latest_change,
                key,
                record.ts,
                payload.change_class.clone(),
            ),
            HealthEvent::TalentDispatch(_) => push_record(
                &mut records,
                &mut sequence,
                key,
                payload.name.clone(),
                record.ts,
                ProgressKind::Dispatch,
                payload.use_id.clone(),
            ),
            HealthEvent::TalentComplete(_) => push_record(
                &mut records,
                &mut sequence,
                key,
                payload.name.clone(),
                record.ts,
                ProgressKind::Complete,
                payload.use_id.clone(),
            ),
            HealthEvent::TalentFail(_) => push_record(
                &mut records,
                &mut sequence,
                key,
                payload.name.clone(),
                record.ts,
                ProgressKind::Fail,
                payload.use_id.clone(),
            ),
            HealthEvent::TalentSkip(_) if payload.reason.as_deref() == Some("capped") => {
                push_record(
                    &mut records,
                    &mut sequence,
                    key,
                    payload.name.clone(),
                    record.ts,
                    ProgressKind::Capped,
                    payload.use_id.clone(),
                )
            }
            HealthEvent::TalentSkip(_) if payload.reason.as_deref() == Some("no_config") => {
                if let Some(name) = payload.name.clone() {
                    unconfigured.entry(key).or_default().insert(name);
                }
            }
            HealthEvent::TalentSkip(_)
                if payload.reason.as_deref() == Some("not_recommended")
                    && matches!(
                        payload.name.as_deref(),
                        Some("screen" | "speaker_attribution")
                    ) =>
            {
                push_record(
                    &mut records,
                    &mut sequence,
                    key,
                    payload.name.clone(),
                    record.ts,
                    ProgressKind::NotRecommended,
                    None,
                )
            }
            _ => {}
        }
    }
    let keys = latest_sense
        .keys()
        .chain(latest_change.keys())
        .chain(records.keys())
        .chain(unconfigured.keys())
        .cloned()
        .collect::<BTreeSet<_>>();
    let value = keys
        .into_iter()
        .map(|key| {
            let mut dispatched = BTreeSet::new();
            let mut completed = BTreeSet::new();
            let mut capped_by_skip = BTreeSet::new();
            if let Some(by_name) = records.remove(&key) {
                for (name, mut entries) in by_name {
                    entries.sort_by_key(|item| (item.ts, item.sequence));
                    // A fresh selection can withdraw optional work. A later
                    // dispatch restores the obligation; late worker outcomes
                    // cannot restore work that is no longer recommended.
                    if entries
                        .iter()
                        .rfind(|item| {
                            matches!(
                                item.kind,
                                ProgressKind::Dispatch | ProgressKind::NotRecommended
                            )
                        })
                        .is_some_and(|item| item.kind == ProgressKind::Dispatch)
                    {
                        dispatched.insert(name.clone());
                    }
                    if qualifying_terminal(&entries)
                        .is_some_and(|item| item.kind == ProgressKind::Capped)
                    {
                        capped_by_skip.insert(name.clone());
                    }
                    if name_is_completed(&entries) {
                        completed.insert(name);
                    }
                }
            }
            let progress = SegmentProgress {
                sensed: latest_sense.contains_key(&key),
                density: latest_sense
                    .get(&key)
                    .and_then(|(_, density)| density.clone()),
                change_class: latest_change
                    .get(&key)
                    .and_then(|(_, change)| change.clone()),
                dispatched,
                completed,
                unconfigured: unconfigured.remove(&key).unwrap_or_default(),
                capped_by_skip,
            };
            (key, progress)
        })
        .collect();
    Ok(FoldRead {
        value,
        malformed_line_count: scanned.malformed_line_count,
    })
}

fn push_record(
    records: &mut BTreeMap<SegmentIdentity, BTreeMap<String, Vec<ProgressRecord>>>,
    sequence: &mut usize,
    key: SegmentIdentity,
    name: Option<String>,
    ts: i64,
    kind: ProgressKind,
    use_id: Option<String>,
) {
    let Some(name) = name else { return };
    *sequence += 1;
    records
        .entry(key)
        .or_default()
        .entry(name)
        .or_default()
        .push(ProgressRecord {
            ts,
            sequence: *sequence,
            kind,
            use_id,
        });
}

fn update_latest(
    values: &mut BTreeMap<SegmentIdentity, (i64, Option<String>)>,
    key: SegmentIdentity,
    ts: i64,
    value: Option<String>,
) {
    if values.get(&key).is_none_or(|(latest, _)| ts >= *latest) {
        values.insert(key, (ts, value));
    }
}

fn latest_terminal(entries: &[ProgressRecord]) -> Option<&ProgressRecord> {
    entries
        .iter()
        .rfind(|item| item.kind != ProgressKind::Dispatch)
}

fn qualifying_terminal(entries: &[ProgressRecord]) -> Option<&ProgressRecord> {
    let last_dispatch = entries
        .iter()
        .rfind(|item| item.kind == ProgressKind::Dispatch);
    let Some(dispatch) = last_dispatch else {
        return latest_terminal(entries);
    };
    // Legacy dispatch/terminal rows on disk predate use_id correlation and
    // cannot be rewritten, so a missing use_id on either side falls back to
    // record order.
    entries.iter().rfind(|item| {
        item.kind != ProgressKind::Dispatch
            && (item.ts, item.sequence) > (dispatch.ts, dispatch.sequence)
            && use_id_matches(&dispatch.use_id, &item.use_id)
    })
}

fn name_is_completed(entries: &[ProgressRecord]) -> bool {
    qualifying_terminal(entries).is_some_and(|item| item.kind == ProgressKind::Complete)
}

fn use_id_matches(dispatch: &Option<String>, terminal: &Option<String>) -> bool {
    match (dispatch, terminal) {
        (Some(left), Some(right)) => left == right,
        _ => true,
    }
}
