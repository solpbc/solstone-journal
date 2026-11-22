// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Whether an activity was shared time with one linked person, and which of
//! its segments hold that interaction.

use std::collections::BTreeSet;
use std::path::Path;

use serde::Serialize;
use serde_json::Value;
use solstone_core_facets::ActivityRecord;

use crate::lines::utterance_party;
use crate::live::ActivityWindow;
use crate::messages::{Party, Thread, ThreadKey, conversation_threads};
use crate::names::PersonRef;
use crate::people::Owner;
use crate::source::{SegmentSource, Voice, load_activity_sources};
use crate::talents::{PRESENCE_TALENT, TalentCaller};

/// Distinct messages from the person that tie an unnamed thread to them.
const THREAD_SENDER_MESSAGES: usize = 3;
/// A focused read is a 1:1 thread open in at least this many frames...
const FOCUSED_READ_FRAMES: usize = 2;
/// ...that are at least half the activity's conversation frames.
const FOCUSED_READ_SHARE_PERCENT: usize = 50;
/// Sustained voice: at least this many lines...
const VOICE_LINES: usize = 10;
/// ...and at least this share of the activity's lines.
const VOICE_SHARE_PERCENT: usize = 10;
/// Lines either side of a spoken mention shown to the presence talent.
const MENTION_CONTEXT_LINES: usize = 3;
/// Presence questions asked per activity at most.
const MENTION_WINDOWS: usize = 8;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Basis {
    MessageThread,
    Voice,
    SpokenWith,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub enum Shape {
    #[serde(rename = "messaging_1to1")]
    Messaging1to1,
    #[serde(rename = "messaging_group")]
    MessagingGroup,
    #[serde(rename = "conversation")]
    Conversation,
}

/// Why an activity qualifies, and the segments in scope for its summary.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct ActivityMatch {
    pub basis: Basis,
    pub shape: Shape,
    pub segments: Vec<String>,
    pub threads: Vec<ThreadKey>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub partial_input: Option<Value>,
}

/// The parts of an activity record the matcher reads. The journal's title,
/// description and participation never decide a match.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ActivityRef {
    pub(crate) id: String,
    pub(crate) activity_type: String,
    pub(crate) segments: Vec<String>,
    pub(crate) stream: Option<String>,
}

impl ActivityRef {
    pub(crate) fn from_record(record: &ActivityRecord) -> Self {
        let text = |key: &str| {
            record
                .get(key)
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned()
        };
        Self {
            id: text("id"),
            activity_type: text("activity"),
            segments: record
                .get("segments")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(Value::as_str)
                .map(str::to_owned)
                .collect(),
            stream: record
                .get("stream")
                .and_then(Value::as_str)
                .filter(|stream| !stream.is_empty())
                .map(str::to_owned),
        }
    }
}

/// Read an activity's sources and decide whether it matches `person`.
/// Only the spoken-with check asks a model, and only when the person's name
/// is said.
pub fn match_activity(
    journal: &Path,
    facet: &str,
    day: &str,
    record: &ActivityRecord,
    person: &PersonRef,
    owner: &Owner,
    talents: &dyn TalentCaller,
) -> Option<ActivityMatch> {
    let activity = ActivityRef::from_record(record);
    log::debug!(
        "{}",
        solstone_core_cli_copy::display_for(
            talents.diagnostic_locale(),
            "journal.cli.share.provider.matching",
            [
                ("facet", &facet),
                ("day", &day),
                ("activity", &activity.id),
                ("entity", &person.entity_id)
            ]
        )
    );
    let sources = read_sources(journal, day, &activity, talents.diagnostic_locale());
    match_sources(&sources, person, owner, talents)
}

pub(crate) fn read_sources(
    journal: &Path,
    day: &str,
    activity: &ActivityRef,
    locale: &str,
) -> Vec<SegmentSource> {
    let voices = solstone_core_talent_runtime::voice_names_in(journal, locale);
    load_activity_sources(
        journal,
        day,
        &activity.segments,
        activity.stream.as_deref(),
        voices.as_ref(),
    )
}

/// Evidence in order; the first that holds wins.
pub(crate) fn match_sources(
    sources: &[SegmentSource],
    person: &PersonRef,
    owner: &Owner,
    talents: &dyn TalentCaller,
) -> Option<ActivityMatch> {
    let segments = sources
        .iter()
        .map(|source| source.segment.clone())
        .collect::<Vec<_>>();
    let window = ActivityWindow::from_segments(&segments);
    message_thread(sources, window.as_ref(), person, owner)
        .or_else(|| sustained_voice(sources, person))
        .or_else(|| spoken_with(sources, person, owner, talents))
}

fn message_thread(
    sources: &[SegmentSource],
    window: Option<&ActivityWindow>,
    person: &PersonRef,
    owner: &Owner,
) -> Option<ActivityMatch> {
    let (threads, conversation_frames) = conversation_threads(sources);
    let qualifying = threads
        .iter()
        .filter(|thread| thread_qualifies(thread, conversation_frames, window, person, owner))
        .collect::<Vec<_>>();
    if qualifying.is_empty() {
        return None;
    }
    let shape = if qualifying
        .iter()
        .all(|thread| is_one_to_one(thread, person, owner))
    {
        Shape::Messaging1to1
    } else {
        Shape::MessagingGroup
    };
    let in_scope = qualifying
        .iter()
        .flat_map(|thread| thread.segments.iter())
        .collect::<BTreeSet<_>>();
    Some(ActivityMatch {
        partial_input: None,
        basis: Basis::MessageThread,
        shape,
        segments: sources
            .iter()
            .map(|source| &source.segment)
            .filter(|segment| in_scope.contains(segment))
            .cloned()
            .collect(),
        threads: qualifying.iter().map(|thread| thread.key.clone()).collect(),
    })
}

/// A thread is the person's when its title names them or at least two of its
/// distinct messages come from them. It qualifies on a live exchange (both
/// sides wrote inside the activity window) or on a focused read of their
/// 1:1 thread after they wrote during the activity, answered or not.
fn thread_qualifies(
    thread: &Thread,
    conversation_frames: usize,
    window: Option<&ActivityWindow>,
    person: &PersonRef,
    owner: &Owner,
) -> bool {
    let titled = thread.title_names(person);
    let from_person = thread
        .messages
        .iter()
        .filter(|message| person.is_sender(message.sender()))
        .count();
    if !titled && from_person < THREAD_SENDER_MESSAGES {
        return false;
    }
    let (mut live_owner, mut live_other, mut live_person) = (false, false, false);
    for message in thread.messages.iter().filter(|m| m.is_live(window)) {
        match message.party(person, owner, false) {
            Party::Owner => live_owner = true,
            Party::Person => {
                live_other = true;
                live_person = true;
            }
            Party::Other(_) => live_other = true,
        }
    }
    let live_exchange = live_owner && live_other;
    // Read but not answered: the person wrote during the activity and their
    // 1:1 thread held the owner's attention. A thread merely left open, with
    // nothing new from them, is not an exchange.
    let focused_read = titled
        && live_person
        && thread.frames >= FOCUSED_READ_FRAMES
        && thread.frames * 100 >= conversation_frames * FOCUSED_READ_SHARE_PERCENT;
    live_exchange || focused_read
}

/// A 1:1: the title names the person and every message not from the owner
/// shows the person as its sender.
fn is_one_to_one(thread: &Thread, person: &PersonRef, owner: &Owner) -> bool {
    thread.title_names(person)
        && thread.messages.iter().all(|message| {
            matches!(
                message.party(person, owner, false),
                Party::Owner | Party::Person
            )
        })
}

fn sustained_voice(sources: &[SegmentSource], person: &PersonRef) -> Option<ActivityMatch> {
    let total = sources
        .iter()
        .map(|source| source.utterances.len())
        .sum::<usize>();
    let mut lines = 0;
    let mut evidenced = BTreeSet::new();
    for (index, source) in sources.iter().enumerate() {
        for utterance in &source.utterances {
            if matches!(&utterance.voice, Voice::Entity { id, .. } if *id == person.entity_id) {
                lines += 1;
                evidenced.insert(index);
            }
        }
    }
    (lines >= VOICE_LINES && lines * 100 >= total * VOICE_SHARE_PERCENT).then(|| ActivityMatch {
        partial_input: None,
        basis: Basis::Voice,
        shape: Shape::Conversation,
        segments: with_neighbours(sources, &evidenced),
        threads: Vec::new(),
    })
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Presence {
    With,
    About,
    Unclear,
}

fn spoken_with(
    sources: &[SegmentSource],
    person: &PersonRef,
    owner: &Owner,
    talents: &dyn TalentCaller,
) -> Option<ActivityMatch> {
    let lines = sources
        .iter()
        .enumerate()
        .flat_map(|(index, source)| {
            source
                .utterances
                .iter()
                .map(move |utterance| (index, utterance))
        })
        .collect::<Vec<_>>();
    let mentions = lines
        .iter()
        .enumerate()
        .filter(|(_, (_, utterance))| person.mentioned_in(&utterance.text))
        .map(|(position, _)| position)
        .take(MENTION_WINDOWS);
    for position in mentions {
        let first = position.saturating_sub(MENTION_CONTEXT_LINES);
        let last = (position + MENTION_CONTEXT_LINES).min(lines.len() - 1);
        let window = (first..=last)
            .map(|line| {
                let (_, label) = utterance_party(lines[line].1, person, owner);
                let marker = if line == position { ">> " } else { "" };
                format!("{marker}{label}: {}", lines[line].1.text)
            })
            .collect::<Vec<_>>();
        let (presence, partial_input) = ask_presence(talents, person, owner, &window);
        if presence == Presence::With {
            let evidenced = BTreeSet::from([lines[position].0]);
            return Some(ActivityMatch {
                partial_input,
                basis: Basis::SpokenWith,
                shape: Shape::Conversation,
                segments: with_neighbours(sources, &evidenced),
                threads: Vec::new(),
            });
        }
    }
    None
}

fn ask_presence(
    talents: &dyn TalentCaller,
    person: &PersonRef,
    owner: &Owner,
    window: &[String],
) -> (Presence, Option<Value>) {
    let mut text = format!("Owner: {}\nName in question: {}", owner.name, person.name);
    if !person.aliases.is_empty() {
        text.push_str(&format!(" (also: {})", person.aliases.join(", ")));
    }
    text.push_str("\n\nLines (the marked line contains the name):\n");
    text.push_str(&window.join("\n"));
    match talents.call(PRESENCE_TALENT, &text) {
        Ok(value) => {
            let partial = value
                .get("partial_input")
                .filter(|v| v.is_object())
                .cloned();
            let presence = match value.get("role").and_then(Value::as_str) {
                Some("with") => Presence::With,
                Some("about") => Presence::About,
                _ => Presence::Unclear,
            };
            (presence, partial)
        }
        Err(error) => {
            log::warn!(
                "{}",
                solstone_core_cli_copy::display_for(
                    talents.diagnostic_locale(),
                    "journal.cli.share.provider.presence.unavailable",
                    [("detail", &error)]
                )
            );
            (Presence::Unclear, None)
        }
    }
}

/// The evidenced segments and each one's immediate neighbours, in activity
/// order.
pub(crate) fn with_neighbours(
    sources: &[SegmentSource],
    evidenced: &BTreeSet<usize>,
) -> Vec<String> {
    let mut indices = BTreeSet::new();
    for index in evidenced {
        indices.insert(index.saturating_sub(1));
        indices.insert(*index);
        if index + 1 < sources.len() {
            indices.insert(index + 1);
        }
    }
    indices
        .into_iter()
        .map(|index| sources[index].segment.clone())
        .collect()
}
