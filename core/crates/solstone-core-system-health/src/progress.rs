// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::collections::{BTreeMap, BTreeSet};

use crate::event::HealthEvent;
use crate::read::read_day_records;
use crate::{FoldRead, HealthError, HealthLogSource, SegmentIdentity, SegmentProgress};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SegmentTerminal {
    Complete,
    Fail,
    Capped,
}

pub fn read_segment_progress<S: HealthLogSource>(
    source: &S,
    day: &str,
) -> Result<FoldRead<BTreeMap<SegmentIdentity, SegmentProgress>>, HealthError> {
    let scanned = read_day_records(source, day)?;
    let mut latest_sense: BTreeMap<SegmentIdentity, (i64, Option<String>)> = BTreeMap::new();
    let mut latest_change: BTreeMap<SegmentIdentity, (i64, Option<String>)> = BTreeMap::new();
    let mut dispatched: BTreeMap<SegmentIdentity, BTreeSet<String>> = BTreeMap::new();
    let mut terminals: BTreeMap<SegmentIdentity, BTreeMap<String, (i64, SegmentTerminal)>> =
        BTreeMap::new();
    let mut unconfigured: BTreeMap<SegmentIdentity, BTreeSet<String>> = BTreeMap::new();
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
            HealthEvent::TalentDispatch(_) => {
                if let Some(name) = payload.name.clone() {
                    dispatched.entry(key).or_default().insert(name);
                }
            }
            HealthEvent::TalentComplete(_) => update_terminal(
                &mut terminals,
                key,
                payload.name.clone(),
                record.ts,
                SegmentTerminal::Complete,
            ),
            HealthEvent::TalentFail(_) => update_terminal(
                &mut terminals,
                key,
                payload.name.clone(),
                record.ts,
                SegmentTerminal::Fail,
            ),
            HealthEvent::TalentSkip(_) if payload.reason.as_deref() == Some("capped") => {
                update_terminal(
                    &mut terminals,
                    key,
                    payload.name.clone(),
                    record.ts,
                    SegmentTerminal::Capped,
                )
            }
            HealthEvent::TalentSkip(_) if payload.reason.as_deref() == Some("no_config") => {
                if let Some(name) = payload.name.clone() {
                    unconfigured.entry(key).or_default().insert(name);
                }
            }
            _ => {}
        }
    }
    let keys = latest_sense
        .keys()
        .chain(latest_change.keys())
        .chain(dispatched.keys())
        .chain(terminals.keys())
        .chain(unconfigured.keys())
        .cloned()
        .collect::<BTreeSet<_>>();
    let value = keys
        .into_iter()
        .map(|key| {
            let terminal = terminals.get(&key);
            let completed = terminal
                .into_iter()
                .flat_map(|entries| entries.iter())
                .filter_map(|(name, (_, state))| {
                    (*state == SegmentTerminal::Complete).then_some(name.clone())
                })
                .collect();
            let capped_by_skip = terminal
                .into_iter()
                .flat_map(|entries| entries.iter())
                .filter_map(|(name, (_, state))| {
                    (*state == SegmentTerminal::Capped).then_some(name.clone())
                })
                .collect();
            let progress = SegmentProgress {
                sensed: latest_sense.contains_key(&key),
                density: latest_sense
                    .get(&key)
                    .and_then(|(_, density)| density.clone()),
                change_class: latest_change
                    .get(&key)
                    .and_then(|(_, change)| change.clone()),
                dispatched: dispatched.remove(&key).unwrap_or_default(),
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

fn update_terminal(
    values: &mut BTreeMap<SegmentIdentity, BTreeMap<String, (i64, SegmentTerminal)>>,
    key: SegmentIdentity,
    name: Option<String>,
    ts: i64,
    state: SegmentTerminal,
) {
    let Some(name) = name else { return };
    let terminals = values.entry(key).or_default();
    if terminals.get(&name).is_none_or(|(latest, _)| ts >= *latest) {
        terminals.insert(name, (ts, state));
    }
}
