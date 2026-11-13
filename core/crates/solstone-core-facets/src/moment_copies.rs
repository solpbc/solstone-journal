// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! One moment, recorded once in each facet it belongs to.
//!
//! The activity state machine runs per facet, so a segment that belongs to two
//! facets starts an activity record in each. When both records cover the same
//! stream and the same segments they are copies of one moment, each with its
//! facet's own title and people. A reader that lists across facets shows the
//! moment once: the copy from the facet it belongs to most, with the other
//! facets named beside it.
//!
//! Records that share a segment but are cut differently are separate readings
//! of the time, and stay separate.

use std::collections::BTreeMap;

use serde_json::{Map, Value};

/// What copies of one moment share: the stream and the whole segment list.
/// `None` for a record with no stream or no segments, which is never a copy.
pub fn moment_key(record: &Map<String, Value>) -> Option<String> {
    let stream = record.get("stream").and_then(Value::as_str)?.trim();
    if stream.is_empty() {
        return None;
    }
    let segments = record.get("segments").and_then(Value::as_array)?;
    if segments.is_empty() {
        return None;
    }
    let mut key = String::from(stream);
    for segment in segments {
        key.push('\u{1f}');
        key.push_str(segment.as_str()?);
    }
    Some(key)
}

/// How much of the moment belongs to the record's facet, as the state machine
/// measured it. A record without a level counts as none.
fn level(record: &Map<String, Value>) -> f64 {
    record
        .get("level_avg")
        .and_then(Value::as_f64)
        .filter(|level| level.is_finite())
        .unwrap_or(0.0)
}

/// One moment as a cross-facet reader shows it.
#[derive(Debug)]
pub struct Moment<T> {
    /// The copy from the facet the moment belongs to most.
    pub primary: T,
    /// The other facets' copies, in the order they were given.
    pub copies: Vec<T>,
}

/// Fold the copies of each moment on a day into one.
///
/// The primary is the copy with the highest level; of equals, the first given,
/// so a caller that wants a stable choice gives its items in a stable order.
/// The result keeps the order the primaries were given in.
pub fn merge_moment_copies<T>(
    items: Vec<T>,
    day: impl Fn(&T) -> &str,
    record: impl Fn(&T) -> &Map<String, Value>,
) -> Vec<Moment<T>> {
    let mut places: BTreeMap<(String, String), usize> = BTreeMap::new();
    let mut groups: Vec<Vec<T>> = Vec::new();
    for item in items {
        let place = moment_key(record(&item)).map(|key| (day(&item).to_owned(), key));
        match place.as_ref().and_then(|place| places.get(place)) {
            Some(index) => groups[*index].push(item),
            None => {
                if let Some(place) = place {
                    places.insert(place, groups.len());
                }
                groups.push(vec![item]);
            }
        }
    }
    groups
        .into_iter()
        .map(|mut group| {
            let mut best = 0;
            for (index, item) in group.iter().enumerate() {
                if level(record(item)) > level(record(&group[best])) {
                    best = index;
                }
            }
            let primary = group.remove(best);
            Moment {
                primary,
                copies: group,
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn record(value: Value) -> Map<String, Value> {
        value.as_object().cloned().unwrap()
    }

    fn item(facet: &str, day: &str, value: Value) -> (String, String, Map<String, Value>) {
        (facet.to_owned(), day.to_owned(), record(value))
    }

    fn merged(items: Vec<(String, String, Map<String, Value>)>) -> Vec<(String, Vec<String>)> {
        merge_moment_copies(items, |item| item.1.as_str(), |item| &item.2)
            .into_iter()
            .map(|moment| {
                (
                    moment.primary.0,
                    moment.copies.into_iter().map(|copy| copy.0).collect(),
                )
            })
            .collect()
    }

    #[test]
    fn copies_of_one_moment_fold_into_the_facet_it_belongs_to_most() {
        let call = |level: f64| json!({"stream": "device_audio", "segments": ["150000_600"], "level_avg": level});
        let result = merged(vec![
            item("work", "20261109", call(0.4)),
            item("side", "20261109", call(0.9)),
        ]);
        assert_eq!(result, vec![("side".to_owned(), vec!["work".to_owned()])]);
    }

    #[test]
    fn equal_levels_keep_the_first_given() {
        let call = json!({"stream": "device_audio", "segments": ["150000_600"], "level_avg": 0.5});
        let result = merged(vec![
            item("work", "20261109", call.clone()),
            item("side", "20261109", call),
        ]);
        assert_eq!(result, vec![("work".to_owned(), vec!["side".to_owned()])]);
    }

    #[test]
    fn different_cuts_days_and_streams_stay_separate() {
        let result = merged(vec![
            item(
                "work",
                "20261109",
                json!({"stream": "device_screen", "segments": ["150000_300", "150500_300"]}),
            ),
            // The same first segment, cut shorter.
            item(
                "side",
                "20261109",
                json!({"stream": "device_screen", "segments": ["150000_300"]}),
            ),
            // The same segments on another day.
            item(
                "side",
                "20261110",
                json!({"stream": "device_screen", "segments": ["150000_300", "150500_300"]}),
            ),
            // The same segment names on another stream.
            item(
                "side",
                "20261109",
                json!({"stream": "device_audio", "segments": ["150000_300", "150500_300"]}),
            ),
        ]);
        assert_eq!(result.len(), 4);
        assert!(result.iter().all(|(_, copies)| copies.is_empty()));
    }

    #[test]
    fn a_record_with_no_stream_or_segments_is_never_a_copy() {
        let result = merged(vec![
            item("work", "20261109", json!({"segments": ["150000_300"]})),
            item("side", "20261109", json!({"segments": ["150000_300"]})),
            item(
                "work",
                "20261109",
                json!({"stream": "device_audio", "segments": []}),
            ),
            item(
                "side",
                "20261109",
                json!({"stream": "device_audio", "segments": []}),
            ),
        ]);
        assert_eq!(result.len(), 4);
    }
}
