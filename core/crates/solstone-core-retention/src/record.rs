// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! What the door removed, added to the journal's deletion record.
//!
//! The door's verbs return an [`Outcome`] that names, per target, exactly what
//! was confirmed gone. An owner removal that goes through the door records from
//! that outcome after the run, so the record never names something the door did
//! not remove. A target the door kept, or only partly removed, is not recorded.

use solstone_core_removals::{Removal, Unit};

use crate::receipt::{Outcome, TargetOutcome};

/// Record every segment the run removed whole.
pub fn record_removed_segments(removal: &Removal, outcome: &Outcome) {
    for target in outcome
        .targets
        .iter()
        .filter(|target| removed_whole(target))
    {
        removal.record(Unit::Segment {
            day: target.target.day.clone(),
            stream: target.target.stream.clone(),
            segment: target.target.dir.clone(),
        });
    }
}

/// Record every original file the run released, one unit per file.
pub fn record_released_originals(removal: &Removal, outcome: &Outcome) {
    for target in &outcome.targets {
        for path in &target.removed {
            let file = path
                .as_str()
                .rsplit('/')
                .next()
                .unwrap_or_default()
                .to_owned();
            removal.record(Unit::Original {
                day: target.target.day.clone(),
                stream: target.target.stream.clone(),
                segment: target.target.dir.clone(),
                file,
            });
        }
    }
}

fn removed_whole(target: &TargetOutcome) -> bool {
    target.not_removed.is_empty() && !target.removed.is_empty()
}
