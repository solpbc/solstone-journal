// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Who said an activity's lines, in the owner's words.
//!
//! The owner names one speaker for a set of the activity's transcript lines.
//! The answers go to the owner line-label store, never to the segment's
//! speaker labels, corrections, voiceprints or owner-voice state, and the
//! activity records an `owner:lines` edit with each line's answer token and
//! the owner answer it replaced. Removing that edit puts back what it
//! replaced, line by line, unless a later answer has taken the line since.
//! The activity's edit history is the record of both; they write nothing
//! else beyond the answers, the activity and the owner's ask.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use axum::{
    body::Bytes,
    extract::{Path as RoutePath, RawQuery, State},
    http::StatusCode,
    response::Response,
};
use chrono::Utc;
use serde_json::{Map, Value, json};
use solstone_core_facets::{
    ActivityRecord, ActivityRecordStoreError, EDIT_LINES, EDIT_SPEAKER, OWNER_LINES_ACTOR,
    edit_removed,
};
use solstone_core_speaker_id::owner_line_labels::{
    OwnerLineEntry, owner_line_entry_from_value, owner_line_entry_value,
};
use solstone_core_speaker_resolve::owner_line_labels::{
    LineRevert, LineSpeaker, OwnerLine, OwnerLineLabelError, current_owner_line_entries,
    revert_owner_line_labels_then, set_owner_line_labels_then,
};
use solstone_core_system::owner_ask::{AskedActivity, activities_over};

use super::{
    ACTOR_HEADER, ACTOR_OWNER, not_found, object_body, query_value, store_error, think_again_over,
    with_ask,
};
use crate::{Clock, http};

/// One line as a request or an edit names it.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct LineKey {
    stream: String,
    segment: String,
    source: String,
    sentence_id: i64,
}

impl LineKey {
    fn from_value(value: &Value) -> Option<Self> {
        let object = value.as_object()?;
        let text = |key: &str| {
            object
                .get(key)
                .and_then(Value::as_str)
                .filter(|value| !value.is_empty())
                .map(str::to_owned)
        };
        let key = Self {
            stream: text("stream").filter(|stream| valid_stream(stream))?,
            segment: text("segment").filter(|segment| valid_segment(segment))?,
            source: text("source")?,
            sentence_id: object.get("sentence_id")?.as_i64()?,
        };
        Some(key)
    }

    fn owner_line(&self, root: &Path, day: &str) -> OwnerLine {
        OwnerLine {
            segment_dir: segment_dir(root, day, &self.stream, &self.segment),
            source: self.source.clone(),
            sentence_id: self.sentence_id,
        }
    }
}

/// A segment's directory: `_default` stream segments sit directly under the day.
pub(super) fn segment_dir(root: &Path, day: &str, stream: &str, segment: &str) -> PathBuf {
    let day_dir = root.join("chronicle").join(day);
    if stream == solstone_core_journal_io::DEFAULT_STREAM {
        day_dir.join(segment)
    } else {
        day_dir.join(stream).join(segment)
    }
}

fn valid_stream(stream: &str) -> bool {
    !matches!(stream, "." | "..")
        && stream
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
}

fn valid_segment(segment: &str) -> bool {
    let Some((time, length)) = segment.split_once('_') else {
        return false;
    };
    time.len() == 6
        && time.bytes().all(|byte| byte.is_ascii_digit())
        && !length.is_empty()
        && length.bytes().all(|byte| byte.is_ascii_digit())
}

fn refused(reason_code: &str, detail: impl Into<String>, status: StatusCode) -> Response {
    http::error(
        reason_code,
        "who said those lines couldn't be saved.",
        detail.into(),
        status,
    )
}

/// The reply for a refusal from the owner line-label store.
pub(super) fn store_refused(error: &OwnerLineLabelError) -> Response {
    let status = match error {
        OwnerLineLabelError::NoLines => StatusCode::BAD_REQUEST,
        OwnerLineLabelError::Lock(_) | OwnerLineLabelError::MergeRecoveryPending => {
            StatusCode::SERVICE_UNAVAILABLE
        }
        OwnerLineLabelError::Write(_) => StatusCode::INTERNAL_SERVER_ERROR,
        _ => StatusCode::CONFLICT,
    };
    refused(error.reason_code(), error.to_string(), status)
}

fn owner_only() -> Response {
    http::error(
        "activity_owner_only",
        "only you can say who said these lines.",
        "saying who said a line needs the owner".to_owned(),
        StatusCode::FORBIDDEN,
    )
}

/// The activity's stream and segment keys.
fn activity_segments(record: &ActivityRecord) -> Option<(String, BTreeSet<String>)> {
    let stream = record.get("stream").and_then(Value::as_str)?.to_owned();
    let segments = record
        .get("segments")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .map(str::to_owned)
        .collect();
    Some((stream, segments))
}

/// Ask the journal to think again every activity, in any facet, over the
/// lines' segments, and the day's briefing. `this` is always named.
fn ask_over(root: &Path, day: &str, this: AskedActivity, lines: &[LineKey]) -> Map<String, Value> {
    let pairs = lines
        .iter()
        .map(|line| (line.stream.clone(), line.segment.clone()))
        .collect::<BTreeSet<_>>();
    let mut activities = activities_over(root, day, &pairs).unwrap_or_else(|error| {
        log::warn!("owner lines: other activities over the lines not found: {error}");
        Vec::new()
    });
    if !activities.contains(&this) {
        activities.push(this);
    }
    think_again_over(root, day, &activities)
}

pub(super) async fn set_lines(
    State((root, clock)): State<(PathBuf, Clock)>,
    RoutePath((day, span_id)): RoutePath<(String, String)>,
    RawQuery(query): RawQuery,
    headers: axum::http::HeaderMap,
    body: Bytes,
) -> Response {
    if headers.get(ACTOR_HEADER).and_then(|v| v.to_str().ok()) != Some(ACTOR_OWNER) {
        return owner_only();
    }
    let facet = query_value(query.as_deref(), "facet").unwrap_or_default();
    set_lines_checked(&root, &clock, &day, &span_id, &facet, &body, || Ok(()))
}

/// Save who said the lines `body` names, then the activity's edit recording
/// it. `edit_check` runs under the activity day's lock just before the edit
/// is written; when the edit can't be written the answers are put back.
pub(super) fn set_lines_checked(
    root: &Path,
    clock: &Clock,
    day: &str,
    span_id: &str,
    facet: &str,
    body: &[u8],
    edit_check: impl FnOnce() -> Result<(), String>,
) -> Response {
    if facet.is_empty() || !crate::segments::is_day(day) {
        return not_found(span_id.to_owned());
    }
    let body = object_body(body);
    let lines = match body.get("lines").map(Value::as_array) {
        Some(Some(lines)) if lines.is_empty() => {
            return store_refused(&OwnerLineLabelError::NoLines);
        }
        Some(Some(lines)) => lines,
        _ => {
            return refused(
                "owner_line_invalid",
                "lines must be a list of lines",
                StatusCode::BAD_REQUEST,
            );
        }
    };
    let Some(lines) = lines
        .iter()
        .map(LineKey::from_value)
        .collect::<Option<BTreeSet<_>>>()
    else {
        return refused(
            "owner_line_invalid",
            "each line names its stream, segment, source and sentence_id",
            StatusCode::BAD_REQUEST,
        );
    };
    let lines = lines.into_iter().collect::<Vec<_>>();
    let Some(speaker) = body.get("speaker").and_then(LineSpeaker::from_tagged) else {
        return refused(
            "owner_line_speaker_invalid",
            "speaker is {\"kind\":\"owner\"}, {\"kind\":\"person\",\"id\":...} or {\"kind\":\"unknown\"}",
            StatusCode::BAD_REQUEST,
        );
    };
    let record = match solstone_core_facets::get_activity_record(root, facet, day, span_id) {
        Ok(Some(record)) => record,
        Ok(None) | Err(ActivityRecordStoreError::MissingDayFile { .. }) => {
            return not_found(span_id.to_owned());
        }
        Err(error) => return store_error(error),
    };
    let held = activity_segments(&record);
    if let Some(line) = lines.iter().find(|line| {
        !held.as_ref().is_some_and(|(stream, segments)| {
            *stream == line.stream && segments.contains(&line.segment)
        })
    }) {
        return refused(
            "owner_line_outside_activity",
            format!(
                "{}/{} isn't a segment of this activity",
                line.stream, line.segment
            ),
            StatusCode::BAD_REQUEST,
        );
    }
    let owner_lines = lines
        .iter()
        .map(|line| line.owner_line(root, day))
        .collect::<Vec<_>>();
    let by_dir = lines
        .iter()
        .zip(&owner_lines)
        .map(|(key, line)| {
            (
                line.segment_dir.clone(),
                (key.stream.clone(), key.segment.clone()),
            )
        })
        .collect::<BTreeMap<_, _>>();
    let timestamp = clock
        .now()
        .with_timezone(&Utc)
        .format("%Y-%m-%dT%H:%M:%SZ")
        .to_string();
    let tagged = speaker.tagged();
    let saved = set_owner_line_labels_then(root, &owner_lines, &speaker, |set| {
        let recorded = set
            .iter()
            .map(|set| {
                let (stream, segment) = by_dir[&set.line.segment_dir].clone();
                json!({
                    "stream": stream,
                    "segment": segment,
                    "source": set.line.source,
                    "sentence_id": set.line.sentence_id,
                    "set_at": set.set_at,
                    "prior": set.prior.as_ref().map(owner_line_entry_value),
                })
            })
            .collect();
        match solstone_core_facets::append_activity_lines_edit(
            root,
            facet,
            day,
            span_id,
            recorded,
            tagged.clone(),
            &timestamp,
            edit_check,
        ) {
            Ok(Some(record)) => Ok(record),
            Ok(None) | Err(ActivityRecordStoreError::MissingDayFile { .. }) => {
                Err(Box::new(not_found(span_id.to_owned())))
            }
            Err(error) => Err(Box::new(store_error(error))),
        }
    });
    let record = match saved {
        Ok(Ok(record)) => record,
        Ok(Err(response)) => return *response,
        Err(error) => return store_refused(&error),
    };
    let this = AskedActivity {
        facet: facet.to_owned(),
        activity: span_id.to_owned(),
    };
    let ask = ask_over(root, day, this, &lines);
    with_ask(record, ask)
}

/// One `owner:lines` edit's lines with each line's token and prior answer.
struct LinesEdit {
    removed: bool,
    lines: BTreeMap<LineKey, (String, Option<Value>)>,
}

fn lines_edits(record: &ActivityRecord) -> Vec<Option<LinesEdit>> {
    record
        .get("edits")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_object)
        .map(|edit| {
            (edit.get("actor").and_then(Value::as_str) == Some(OWNER_LINES_ACTOR)).then(|| {
                LinesEdit {
                    removed: edit_removed(edit),
                    lines: edit
                        .get(EDIT_LINES)
                        .and_then(Value::as_array)
                        .into_iter()
                        .flatten()
                        .filter_map(|line| {
                            let key = LineKey::from_value(line)?;
                            let set_at = line.get("set_at")?.as_str()?.to_owned();
                            let prior = line.get("prior").filter(|prior| !prior.is_null()).cloned();
                            Some((key, (set_at, prior)))
                        })
                        .collect(),
                }
            })
        })
        .collect()
}

/// Remove the `owner:lines` edit at `position` and put back its lines. `None`
/// when the activity holds no such edit there that is still in place, so the
/// ordinary removal answers instead.
#[allow(clippy::too_many_arguments)] // The fields that name the one edit removed.
pub(super) fn remove_lines_edit(
    root: &Path,
    facet: &str,
    day: &str,
    span_id: &str,
    position: usize,
    edit_timestamp: &str,
    edit_actor: &str,
    removed_at: &str,
) -> Option<Response> {
    if edit_actor != OWNER_LINES_ACTOR {
        return None;
    }
    // A lines edit is never removed without putting its lines back.
    let record = match solstone_core_facets::get_activity_record(root, facet, day, span_id) {
        Ok(Some(record)) => record,
        Ok(None) | Err(ActivityRecordStoreError::MissingDayFile { .. }) => {
            return Some(not_found(span_id.to_owned()));
        }
        Err(error) => return Some(store_error(error)),
    };
    let edit = record
        .get("edits")
        .and_then(Value::as_array)?
        .iter()
        .filter_map(Value::as_object)
        .nth(position)?;
    if edit.get("timestamp").and_then(Value::as_str) != Some(edit_timestamp)
        || edit_removed(edit)
        || !edit.get(EDIT_LINES).is_some_and(Value::is_array)
    {
        return None;
    }
    let mut edits = lines_edits(&record);
    if let Some(Some(edit)) = edits.get_mut(position) {
        edit.removed = true;
    }
    let Some(Some(target)) = edits.get(position) else {
        return None;
    };
    let mut reverts = Vec::new();
    for key in target.lines.keys() {
        let covering = edits
            .iter()
            .enumerate()
            .filter_map(|(index, edit)| {
                let edit = edit.as_ref()?;
                edit.lines.get(key).map(|line| (index, edit.removed, line))
            })
            .collect::<Vec<_>>();
        // A later edit that still stands keeps the line.
        if covering
            .iter()
            .any(|(index, removed, _)| *index > position && !removed)
        {
            continue;
        }
        let tokens = covering
            .iter()
            .filter(|(_, removed, _)| *removed)
            .map(|(_, _, (set_at, _))| set_at.clone())
            .collect::<BTreeSet<_>>();
        // The line goes back to what the oldest removed edit in the newest
        // run of removed edits found there.
        let run_start = covering
            .iter()
            .rposition(|(_, removed, _)| !removed)
            .map_or(0, |kept| kept + 1);
        let restore = match &covering[run_start].2.1 {
            Some(prior) => match owner_line_entry_from_value(prior) {
                Ok(entry) => Some(entry),
                Err(detail) => {
                    return Some(refused(
                        "owner_line_edit_unreadable",
                        format!("the answer this edit replaced can't be read: {detail}"),
                        StatusCode::CONFLICT,
                    ));
                }
            },
            None => None,
        };
        reverts.push(LineRevert {
            line: key.owner_line(root, day),
            tokens,
            restore,
        });
    }
    let removed = revert_owner_line_labels_then(root, &reverts, |_| {
        match solstone_core_facets::remove_activity_edit(
            root,
            facet,
            day,
            span_id,
            position,
            edit_timestamp,
            edit_actor,
            removed_at,
        ) {
            Ok(Some(record)) => Ok(record),
            Ok(None) | Err(ActivityRecordStoreError::MissingDayFile { .. }) => {
                Err(Box::new(not_found(span_id.to_owned())))
            }
            Err(ActivityRecordStoreError::EditRefused(refusal)) => {
                Err(Box::new(super::edit_refused(refusal)))
            }
            Err(error) => Err(Box::new(store_error(error))),
        }
    });
    let record = match removed {
        Ok(Ok(record)) => record,
        Ok(Err(response)) => return Some(*response),
        Err(error) => return Some(store_refused(&error)),
    };
    let lines = target.lines.keys().cloned().collect::<Vec<_>>();
    let this = AskedActivity {
        facet: facet.to_owned(),
        activity: span_id.to_owned(),
    };
    let ask = ask_over(root, day, this, &lines);
    Some(with_ask(record, ask))
}

/// What the day view says of an `owner:lines` edit: how many lines and who.
pub(super) fn history_fields(edit: &Map<String, Value>) -> Option<(usize, Value)> {
    (edit.get("actor").and_then(Value::as_str) == Some(OWNER_LINES_ACTOR)).then(|| {
        (
            edit.get(EDIT_LINES)
                .and_then(Value::as_array)
                .map_or(0, Vec::len),
            edit.get(EDIT_SPEAKER).cloned().unwrap_or(Value::Null),
        )
    })
}

/// Segments' current owner answers, read once each while building a view.
#[derive(Default)]
pub(super) struct Answers(BTreeMap<PathBuf, Option<Vec<OwnerLineEntry>>>);

impl Answers {
    /// Whether some `owner:lines` edit still in place on `record` has a line
    /// whose answer still carries that edit's token.
    pub(super) fn hold(&mut self, root: &Path, day: &str, record: &ActivityRecord) -> bool {
        lines_edits(record)
            .into_iter()
            .flatten()
            .filter(|edit| !edit.removed)
            .any(|edit| {
                edit.lines.iter().any(|(key, (set_at, _))| {
                    let dir = segment_dir(root, day, &key.stream, &key.segment);
                    self.0
                        .entry(dir)
                        .or_insert_with_key(|dir| current_owner_line_entries(dir).ok())
                        .as_ref()
                        .is_some_and(|entries| {
                            entries.iter().any(|entry| {
                                entry.key() == (key.source.as_str(), key.sentence_id)
                                    && entry.set_at == *set_at
                            })
                        })
                })
            })
    }
}

#[cfg(test)]
#[path = "lines_tests.rs"]
mod tests;
