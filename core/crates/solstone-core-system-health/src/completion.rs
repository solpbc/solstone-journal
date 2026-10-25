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
    if progress.dispatched.contains("facet_routing")
        && !progress.completed.contains("facet_routing")
    {
        return ThoughtVerdict::Dispatched("facet_routing".to_owned());
    }
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

/// Every step a segment's thinking still waits on, where
/// `segment_fully_thought` names only the first. Empty exactly when that
/// verdict is complete.
pub fn segment_pending_talents(progress: Option<&SegmentProgress>) -> BTreeSet<String> {
    let Some(progress) = progress.filter(|progress| progress.sensed) else {
        return BTreeSet::from(["sense".to_owned()]);
    };
    let mut pending = BTreeSet::new();
    if progress.dispatched.contains("facet_routing")
        && !progress.completed.contains("facet_routing")
    {
        pending.insert("facet_routing".to_owned());
    }
    if progress.density.as_deref() == Some("idle")
        || progress.change_class.as_deref() == Some("redundant")
    {
        return pending;
    }
    for name in SEGMENT_FLOOR_TALENTS {
        if !progress.completed.contains(*name)
            && !progress.unconfigured.contains(*name)
            && !progress.capped_by_skip.contains(*name)
        {
            pending.insert((*name).to_owned());
        }
    }
    for name in &progress.dispatched {
        if SEGMENT_NONGATING_TALENTS.contains(&name.as_str())
            || SEGMENT_SUPERSEDED_TALENTS
                .iter()
                .find(|(legacy, _)| *legacy == name)
                .is_some_and(|(_, replacement)| progress.completed.contains(*replacement))
        {
            continue;
        }
        if !progress.completed.contains(name) && !progress.capped_by_skip.contains(name) {
            pending.insert(name.clone());
        }
    }
    pending
}

/// Whether a segment's thinking already covers the input it holds now.
///
/// True only when the segment is fully thought and none of its input is newer
/// than the instant its latest Sense began reading it.  A modality that landed,
/// or was analyzed again, after that instant makes this false, so the segment
/// is thought again rather than kept on partial input.
pub fn segment_thinking_is_current(
    progress: Option<&SegmentProgress>,
    newest_input_ms: Option<i64>,
) -> bool {
    let Some(read_from) = progress.and_then(|progress| progress.sense_input_ms) else {
        return false;
    };
    segment_fully_thought(progress) == ThoughtVerdict::Complete
        && newest_input_ms.is_none_or(|newest| newest < read_from)
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
        if let Some(blocker) = segment_blocker(segment, segment_progress) {
            match blocker.dimension {
                SegmentBlockerDimension::NotSensed => completion.not_sensed += 1,
                SegmentBlockerDimension::NotThought => completion.not_thought += 1,
            }
            completion.blockers.push(blocker);
        }
    }
    completion.exhausted = exhausted.into_iter().collect();
    completion
}

pub fn blocked_segment_keys(
    segments: &[SegmentInput],
    progress: &BTreeMap<SegmentIdentity, SegmentProgress>,
) -> BTreeSet<SegmentIdentity> {
    blocked_segment_details(segments, progress)
        .into_keys()
        .collect()
}

/// Each segment still blocking its day, with why: its unsensed modalities as
/// `modality=state`, or the thinking it still waits on.
pub fn blocked_segment_details(
    segments: &[SegmentInput],
    progress: &BTreeMap<SegmentIdentity, SegmentProgress>,
) -> BTreeMap<SegmentIdentity, String> {
    segments
        .iter()
        .filter(|segment| segment_requires_processing(segment))
        .filter_map(|segment| {
            let segment_progress = lookup_segment_progress(progress, &segment.stream, &segment.key);
            let blocker = segment_blocker(segment, segment_progress)?;
            Some((
                SegmentIdentity {
                    stream: Some(segment.stream.clone()),
                    segment: segment.key.clone(),
                },
                blocker.detail,
            ))
        })
        .collect()
}

fn segment_blocker(
    segment: &SegmentInput,
    progress: Option<&SegmentProgress>,
) -> Option<SegmentBlocker> {
    if !segment_fully_sensed(&segment.data_state) {
        let detail = segment
            .data_state
            .0
            .iter()
            .filter(|(_, state)| !SENSED_TERMINAL_STATES.contains(&state.as_str()))
            .map(|(modality, state)| format!("{modality}={state}"))
            .collect::<Vec<_>>()
            .join(",");
        return Some(SegmentBlocker {
            segment: segment.key.clone(),
            dimension: SegmentBlockerDimension::NotSensed,
            detail,
        });
    }
    let verdict = segment_fully_thought(progress);
    (verdict != ThoughtVerdict::Complete).then(|| SegmentBlocker {
        segment: segment.key.clone(),
        dimension: SegmentBlockerDimension::NotThought,
        detail: verdict_detail(&verdict),
    })
}

fn verdict_detail(verdict: &ThoughtVerdict) -> String {
    match verdict {
        ThoughtVerdict::Complete => String::new(),
        ThoughtVerdict::NoSenseComplete => "no_sense_complete".to_owned(),
        ThoughtVerdict::Floor(name) => format!("floor:{name}"),
        ThoughtVerdict::Dispatched(name) => format!("dispatched:{name}"),
    }
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::segment_thinking_is_current;
    use crate::{FilesystemHealthLogSource, lookup_segment_progress, read_segment_progress};

    const DAY: &str = "20261002";

    fn progress_after(lines: &[&str]) -> Option<crate::SegmentProgress> {
        let root = tempfile::tempdir().unwrap();
        let dir = root.path().join("chronicle").join(DAY).join("health");
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("run.jsonl"), lines.join("\n") + "\n").unwrap();
        let progress =
            read_segment_progress(&FilesystemHealthLogSource::new(root.path()), DAY).unwrap();
        lookup_segment_progress(&progress.value, "watch", "193336_300").cloned()
    }

    fn event(event: &str, ts: i64, extra: &str) -> String {
        format!(
            r#"{{"event":"{event}","ts":{ts},"mode":"segment","day":"{DAY}","stream":"watch","segment":"193336_300"{extra}}}"#
        )
    }

    fn thought_at(sense_dispatch: i64) -> Vec<String> {
        vec![
            event(
                "talent.dispatch",
                sense_dispatch,
                r#","name":"sense","use_id":"s""#,
            ),
            event(
                "sense.complete",
                sense_dispatch + 50,
                r#","density":"active""#,
            ),
            event(
                "talent.complete",
                sense_dispatch + 50,
                r#","name":"sense","use_id":"s","state":"finish""#,
            ),
            event(
                "talent.dispatch",
                sense_dispatch + 60,
                r#","name":"documents","use_id":"d""#,
            ),
            event(
                "talent.complete",
                sense_dispatch + 90,
                r#","name":"documents","use_id":"d","state":"finish""#,
            ),
        ]
    }

    fn refs(lines: &[String]) -> Vec<&str> {
        lines.iter().map(String::as_str).collect()
    }

    #[test]
    fn pending_talents_name_every_step_the_verdict_waits_on() {
        use super::{segment_fully_thought, segment_pending_talents};
        use crate::ThoughtVerdict;
        assert_eq!(segment_pending_talents(None), ["sense".to_owned()].into());
        let thought = thought_at(100);
        let mut screen_and_speakers = thought.clone();
        for (name, ts) in [("screen", 200), ("speaker_attribution", 210)] {
            screen_and_speakers.push(event(
                "talent.dispatch",
                ts,
                &format!(r#","name":"{name}","use_id":"{name}""#),
            ));
            screen_and_speakers.push(event(
                "talent.fail",
                ts + 1,
                &format!(r#","name":"{name}","use_id":"{name}","reason_code":"no_output""#),
            ));
        }
        for (lines, pending) in [
            (thought.clone(), vec![]),
            (screen_and_speakers, vec!["screen", "speaker_attribution"]),
        ] {
            let progress = progress_after(&refs(&lines));
            let names = segment_pending_talents(progress.as_ref());
            assert_eq!(
                names.iter().map(String::as_str).collect::<Vec<_>>(),
                pending
            );
            assert_eq!(
                names.is_empty(),
                segment_fully_thought(progress.as_ref()) == ThoughtVerdict::Complete
            );
        }
    }

    #[test]
    fn thinking_is_current_until_input_newer_than_the_sense_read_arrives() {
        let progress = progress_after(&refs(&thought_at(1_000)));
        assert_eq!(progress.as_ref().unwrap().sense_input_ms, Some(1_000));
        assert!(segment_thinking_is_current(progress.as_ref(), Some(999)));
        assert!(segment_thinking_is_current(progress.as_ref(), None));
        // Input written while Sense was reading, or after, was not part of it.
        assert!(!segment_thinking_is_current(progress.as_ref(), Some(1_000)));
        assert!(!segment_thinking_is_current(progress.as_ref(), Some(1_500)));
    }

    #[test]
    fn an_outstanding_or_unfinished_run_is_never_current() {
        let mut lines = thought_at(1_000);
        lines.push(event(
            "talent.dispatch",
            2_000,
            r#","name":"sense","use_id":"again""#,
        ));
        let progress = progress_after(&refs(&lines));
        assert_eq!(progress.as_ref().unwrap().sense_input_ms, None);
        assert!(!segment_thinking_is_current(progress.as_ref(), Some(1)));

        let mut lines = thought_at(1_000);
        lines.pop();
        let progress = progress_after(&refs(&lines));
        assert!(!segment_thinking_is_current(progress.as_ref(), Some(1)));
        assert!(!segment_thinking_is_current(None, Some(1)));
    }

    #[test]
    fn sense_without_a_model_call_reads_its_input_when_it_records() {
        let line = event("sense.complete", 500, r#","density":"idle""#);
        let progress = progress_after(&[&line]);
        assert_eq!(progress.as_ref().unwrap().sense_input_ms, Some(500));
        assert!(segment_thinking_is_current(progress.as_ref(), Some(499)));
        assert!(!segment_thinking_is_current(progress.as_ref(), Some(501)));
    }

    #[test]
    fn a_later_pass_without_a_model_call_reads_its_input_when_it_records() {
        let mut lines = thought_at(1_000);
        lines.push(event("sense.complete", 5_000, r#","density":"idle""#));
        let progress = progress_after(&refs(&lines));
        assert_eq!(progress.as_ref().unwrap().sense_input_ms, Some(5_000));
        assert!(segment_thinking_is_current(progress.as_ref(), Some(3_000)));
        assert!(!segment_thinking_is_current(progress.as_ref(), Some(5_000)));
    }
}
