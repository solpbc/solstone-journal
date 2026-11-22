// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Scoped segment summaries: the model lists points citing line ids; code
//! decides who did each one.

use std::path::Path;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use solstone_core_facets::ActivityRecord;

use crate::lines::{SourceLine, conversation_lines, message_lines};
use crate::matching::{ActivityMatch, ActivityRef, Basis, Shape, match_sources, read_sources};
use crate::messages::{Party, conversation_threads};
use crate::names::PersonRef;
use crate::people::Owner;
use crate::source::SegmentSource;
use crate::talents::{ACTIVITY_TALENT, SEGMENT_TALENT, TalentCaller};

const ANOTHER_PARTICIPANT: &str = "another participant";

/// One point of a segment summary, with its code-written actor.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct SharedPoint {
    pub who: String,
    pub text: String,
    pub ids: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct SegmentSummary {
    pub segment: String,
    pub points: Vec<SharedPoint>,
    /// The points joined; `None` when the summary could not be made.
    pub summary: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub partial_input: Option<Value>,
}

/// What would be shared for one activity. It carries the activity's type,
/// never the journal's title or description of it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct SharePackage {
    pub sender: String,
    pub shared_with: String,
    pub day: String,
    pub facet: String,
    pub activity_id: String,
    pub activity_type: String,
    pub basis: Basis,
    pub shape: Shape,
    pub available: bool,
    pub activity_summary: Option<String>,
    pub segments: Vec<SegmentSummary>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub partial_input: Option<Value>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PointsReply {
    points: Vec<RawPoint>,
    #[serde(default)]
    partial_input: Option<Value>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawPoint {
    ids: Vec<String>,
    point: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ActivityReply {
    summary: String,
    #[serde(default)]
    partial_input: Option<Value>,
}

/// Match the activity and, when it matches, build its package. `None` when
/// the activity does not match the person.
pub fn build_package(
    journal: &Path,
    facet: &str,
    day: &str,
    record: &ActivityRecord,
    person: &PersonRef,
    owner: &Owner,
    talents: &dyn TalentCaller,
) -> Option<SharePackage> {
    let activity = ActivityRef::from_record(record);
    let sources = read_sources(journal, day, &activity, talents.diagnostic_locale());
    let matched = match_sources(&sources, person, owner, talents)?;
    let segments = summarize_segments(&sources, &matched, person, owner, talents);
    let available = segments.iter().all(|segment| segment.summary.is_some());
    let activity_summary = if available {
        summarize_activity(&segments, &activity.activity_type, person, owner, talents)
    } else {
        None
    };
    let partial_input = crate::talents::combine_partial_inputs(
        matched
            .partial_input
            .iter()
            .chain(segments.iter().filter_map(|s| s.partial_input.as_ref()))
            .chain(
                activity_summary
                    .as_ref()
                    .and_then(|(_, partial)| partial.as_ref()),
            ),
    );
    Some(SharePackage {
        partial_input,
        sender: owner.name.clone(),
        shared_with: person.name.clone(),
        day: day.to_owned(),
        facet: facet.to_owned(),
        activity_id: activity.id,
        activity_type: activity.activity_type,
        basis: matched.basis,
        shape: matched.shape,
        available: available && activity_summary.is_some(),
        activity_summary: activity_summary.map(|(summary, _)| summary),
        segments,
    })
}

pub(crate) fn summarize_segments(
    sources: &[SegmentSource],
    matched: &ActivityMatch,
    person: &PersonRef,
    owner: &Owner,
    talents: &dyn TalentCaller,
) -> Vec<SegmentSummary> {
    let threads = (matched.basis == Basis::MessageThread).then(|| conversation_threads(sources).0);
    let in_scope = sources
        .iter()
        .filter(|source| matched.segments.contains(&source.segment))
        .collect::<Vec<_>>();
    in_scope
        .iter()
        .enumerate()
        .map(|(index, source)| {
            let lines = match &threads {
                Some(threads) => {
                    message_lines(threads, &matched.threads, &source.segment, person, owner)
                }
                None => conversation_lines(source, person, owner),
            };
            let points = if lines.is_empty() {
                Some((Vec::new(), None))
            } else {
                let header = segment_header(matched, person, owner, index, in_scope.len());
                summarize_lines(&header, &lines, person, owner, talents)
            };
            SegmentSummary {
                segment: source.segment.clone(),
                summary: points.as_ref().map(|(points, _)| join_points(points)),
                partial_input: points.as_ref().and_then(|(_, partial)| partial.clone()),
                points: points.map(|(points, _)| points).unwrap_or_default(),
            }
        })
        .collect()
}

fn segment_header(
    matched: &ActivityMatch,
    person: &PersonRef,
    owner: &Owner,
    index: usize,
    count: usize,
) -> String {
    let source = match matched.shape {
        Shape::Conversation => "a spoken conversation",
        Shape::Messaging1to1 | Shape::MessagingGroup => "a message thread",
    };
    format!(
        "Owner: {}\nShared with: {}\nSource: {source}\nPart {} of {count}\n\nLines:",
        owner.name,
        person.name,
        index + 1
    )
}

/// Ask for this part's points and attribute them. `None` when the talent
/// call failed or returned something other than its schema.
fn summarize_lines(
    header: &str,
    lines: &[SourceLine],
    person: &PersonRef,
    owner: &Owner,
    talents: &dyn TalentCaller,
) -> Option<(Vec<SharedPoint>, Option<Value>)> {
    let mut input = header.to_owned();
    for line in lines {
        input.push('\n');
        input.push_str(&line.render());
    }
    let reply = talents.call(SEGMENT_TALENT, &input).and_then(|value| {
        serde_json::from_value::<PointsReply>(value).map_err(|error| error.to_string())
    });
    match reply {
        Ok(reply) => Some((
            attribute(reply.points, lines, person, owner),
            reply.partial_input.filter(Value::is_object),
        )),
        Err(error) => {
            log::warn!(
                "{}",
                solstone_core_cli_copy::display_for(
                    talents.diagnostic_locale(),
                    "journal.cli.share.provider.segment.unavailable",
                    [("detail", &error)]
                )
            );
            None
        }
    }
}

/// Drop ids not in the input and points left with none; write each point's
/// actor from the parties of the lines it cites.
fn attribute(
    points: Vec<RawPoint>,
    lines: &[SourceLine],
    person: &PersonRef,
    owner: &Owner,
) -> Vec<SharedPoint> {
    points
        .into_iter()
        .filter_map(|point| {
            let mut ids = Vec::new();
            for id in point.ids {
                if lines.iter().any(|line| line.id == id) && !ids.contains(&id) {
                    ids.push(id);
                }
            }
            let parties = lines
                .iter()
                .filter(|line| ids.contains(&line.id))
                .map(|line| &line.party)
                .collect::<Vec<_>>();
            let body = point.point.trim();
            if ids.is_empty() || body.is_empty() {
                return None;
            }
            let who = actor(&parties, person, owner);
            Some(SharedPoint {
                text: render_point(&who, body),
                who,
                ids,
            })
        })
        .collect()
}

/// Owner first, then the person, then anyone else.
fn actor(parties: &[&Party], person: &PersonRef, owner: &Owner) -> String {
    let mut names = Vec::new();
    if parties.iter().any(|party| matches!(party, Party::Owner)) {
        names.push(owner.name.clone());
    }
    if parties.iter().any(|party| matches!(party, Party::Person)) {
        names.push(person.name.clone());
    }
    if parties.iter().any(|party| matches!(party, Party::Other(_))) {
        names.push(ANOTHER_PARTICIPANT.to_owned());
    }
    let actor = match names.as_slice() {
        [] => ANOTHER_PARTICIPANT.to_owned(),
        [one] => one.clone(),
        [rest @ .., last] => format!("{} and {last}", rest.join(", ")),
    };
    capitalize(&actor)
}

/// "<Actor> <point>." with the point's own leading capital lowered.
fn render_point(actor: &str, point: &str) -> String {
    let mut characters = point.chars();
    let lowered = match (characters.next(), characters.clone().next()) {
        (Some(first), second) if !second.is_some_and(char::is_uppercase) => {
            first.to_lowercase().chain(characters).collect::<String>()
        }
        _ => point.to_owned(),
    };
    let mut rendered = format!("{actor} {lowered}");
    if !rendered.ends_with(['.', '!', '?']) {
        rendered.push('.');
    }
    rendered
}

fn capitalize(text: &str) -> String {
    let mut characters = text.chars();
    match characters.next() {
        Some(first) => first.to_uppercase().chain(characters).collect(),
        None => String::new(),
    }
}

fn join_points(points: &[SharedPoint]) -> String {
    points
        .iter()
        .map(|point| point.text.as_str())
        .collect::<Vec<_>>()
        .join(" ")
}

/// One paragraph from the rendered points only. Empty when nothing was
/// shared; `None` when the talent call failed.
fn summarize_activity(
    segments: &[SegmentSummary],
    activity_type: &str,
    person: &PersonRef,
    owner: &Owner,
    talents: &dyn TalentCaller,
) -> Option<(String, Option<Value>)> {
    let points = segments
        .iter()
        .flat_map(|segment| segment.points.iter())
        .map(|point| format!("- {}", point.text))
        .collect::<Vec<_>>();
    if points.is_empty() {
        return Some((String::new(), None));
    }
    let input = format!(
        "Owner: {}\nShared with: {}\nActivity type: {activity_type}\n\nPoints:\n{}",
        owner.name,
        person.name,
        points.join("\n")
    );
    match talents
        .call(ACTIVITY_TALENT, &input)
        .and_then(|value: Value| {
            serde_json::from_value::<ActivityReply>(value).map_err(|error| error.to_string())
        }) {
        Ok(reply) => Some((
            reply.summary.trim().to_owned(),
            reply.partial_input.filter(Value::is_object),
        )),
        Err(error) => {
            log::warn!(
                "{}",
                solstone_core_cli_copy::display_for(
                    talents.diagnostic_locale(),
                    "journal.cli.share.provider.activity.unavailable",
                    [("detail", &error)]
                )
            );
            None
        }
    }
}
