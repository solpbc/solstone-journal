// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::collections::{BTreeMap, BTreeSet};

use crate::vocabulary::{
    SEGMENT_FLOOR_TALENTS, SEGMENT_NO_PROCESSING_MODALITIES, SEGMENT_NONGATING_TALENTS,
    SEGMENT_SUPERSEDED_TALENTS, SENSED_TERMINAL_STATES,
};
use crate::{
    DataStateMap, SegmentBlocker, SegmentBlockerDimension, SegmentCompletion, SegmentIdentity,
    SegmentInput, SegmentProgress, ThoughtVerdict,
};

pub fn segment_fully_sensed(data_state: &DataStateMap) -> bool {
    data_state
        .0
        .values()
        .all(|state| SENSED_TERMINAL_STATES.contains(&state.as_str()))
}

pub fn segment_requires_processing(segment: &SegmentInput) -> bool {
    segment.data_state.0.is_empty()
        || segment
            .data_state
            .0
            .keys()
            .any(|modality| !SEGMENT_NO_PROCESSING_MODALITIES.contains(&modality.as_str()))
}

pub fn segment_fully_thought(progress: Option<&SegmentProgress>) -> ThoughtVerdict {
    let Some(progress) = progress.filter(|progress| progress.sensed) else {
        return ThoughtVerdict::NoSenseComplete;
    };
    if progress.density.as_deref() == Some("idle")
        || progress.change_class.as_deref() == Some("redundant")
    {
        return ThoughtVerdict::Complete;
    }
    for name in SEGMENT_FLOOR_TALENTS {
        if !progress.completed.contains(*name)
            && !progress.unconfigured.contains(*name)
            && !progress.capped_by_skip.contains(*name)
        {
            return ThoughtVerdict::Floor((*name).to_owned());
        }
    }
    for name in &progress.dispatched {
        if SEGMENT_NONGATING_TALENTS.contains(&name.as_str()) {
            continue;
        }
        if SEGMENT_SUPERSEDED_TALENTS
            .iter()
            .find(|(legacy, _)| *legacy == name)
            .is_some_and(|(_, replacement)| progress.completed.contains(*replacement))
        {
            continue;
        }
        if !progress.completed.contains(name) && !progress.capped_by_skip.contains(name) {
            return ThoughtVerdict::Dispatched(name.clone());
        }
    }
    ThoughtVerdict::Complete
}

pub fn lookup_segment_progress<'a>(
    progress: &'a BTreeMap<SegmentIdentity, SegmentProgress>,
    stream: &str,
    segment: &str,
) -> Option<&'a SegmentProgress> {
    let exact = SegmentIdentity {
        stream: Some(stream.to_owned()),
        segment: segment.to_owned(),
    };
    progress.get(&exact).or_else(|| {
        progress.get(&SegmentIdentity {
            stream: None,
            segment: segment.to_owned(),
        })
    })
}

pub fn classify_segment_completion(
    segments: &[SegmentInput],
    progress: &BTreeMap<SegmentIdentity, SegmentProgress>,
) -> SegmentCompletion {
    let mut completion = SegmentCompletion {
        total: segments.len(),
        ..SegmentCompletion::default()
    };
    let mut exhausted = BTreeSet::new();
    for segment in segments {
        if !segment_requires_processing(segment) {
            continue;
        }
        let segment_progress = lookup_segment_progress(progress, &segment.stream, &segment.key);
        if segment_progress.is_some_and(|value| !value.capped_by_skip.is_empty()) {
            completion.capped += 1;
        }
        if segment
            .data_state
            .0
            .values()
            .any(|state| state == "failed_final")
        {
            exhausted.insert(segment.key.clone());
        }
        if !segment_fully_sensed(&segment.data_state) {
            let detail = segment
                .data_state
                .0
                .iter()
                .filter(|(_, state)| !SENSED_TERMINAL_STATES.contains(&state.as_str()))
                .map(|(modality, state)| format!("{modality}={state}"))
                .collect::<Vec<_>>()
                .join(",");
            completion.blockers.push(SegmentBlocker {
                segment: segment.key.clone(),
                dimension: SegmentBlockerDimension::NotSensed,
                detail,
            });
            completion.not_sensed += 1;
            continue;
        }
        let verdict = segment_fully_thought(segment_progress);
        if verdict != ThoughtVerdict::Complete {
            completion.blockers.push(SegmentBlocker {
                segment: segment.key.clone(),
                dimension: SegmentBlockerDimension::NotThought,
                detail: verdict_detail(&verdict),
            });
            completion.not_thought += 1;
        }
    }
    completion.exhausted = exhausted.into_iter().collect();
    completion
}

pub fn blocked_segment_keys(
    segments: &[SegmentInput],
    progress: &BTreeMap<SegmentIdentity, SegmentProgress>,
) -> BTreeSet<SegmentIdentity> {
    segments
        .iter()
        .filter(|segment| segment_requires_processing(segment))
        .filter_map(|segment| {
            let blocked = !segment_fully_sensed(&segment.data_state)
                || segment_fully_thought(lookup_segment_progress(
                    progress,
                    &segment.stream,
                    &segment.key,
                )) != ThoughtVerdict::Complete;
            blocked.then_some(SegmentIdentity {
                stream: Some(segment.stream.clone()),
                segment: segment.key.clone(),
            })
        })
        .collect()
}

fn verdict_detail(verdict: &ThoughtVerdict) -> String {
    match verdict {
        ThoughtVerdict::Complete => String::new(),
        ThoughtVerdict::NoSenseComplete => "no_sense_complete".to_owned(),
        ThoughtVerdict::Floor(name) => format!("floor:{name}"),
        ThoughtVerdict::Dispatched(name) => format!("dispatched:{name}"),
    }
}
