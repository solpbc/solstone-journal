// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Display provenance for text supplied by a Pulse or Story writer.

use serde::{Deserialize, Serialize};
use solstone_core_window_catalog::{strings, text};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Copy<T = String> {
    PulseTitle,
    PulseBody,
    RecentEntities { error: T },
    MissingDay,
    MissingSegmentId,
    MissingSegment { segment: T },
    MissingSegmentSummary { segment: T },
    InvalidSense { segment: T },
    UnreadableSense { segment: T, error: T },
    SenseLimit { segment: T },
    MissingActivityId { activity: T },
    MissingActivity,
    UnreadableActivities { facet: T, error: T },
    ActivityNotFound { facet: T, activity: T },
    CurrentAwareness { error: T },
    ImportAwareness { error: T },
    OwnerMessage { date: T, quote: T },
    AgentMessage { date: T, quote: T },
    OwnerSpeech { quote: T },
    NamedSpeech { name: T, quote: T },
    UnknownSpeech { quote: T },
    NumberedSpeech { speaker: T, quote: T },
    ScreenNotes,
    Recorded { date: T },
    RecordedApp { date: T, app: T },
    NoDate,
    NoDateSender { sender: T },
}

impl<T> Copy<T> {
    pub(crate) fn is_story(&self) -> bool {
        matches!(
            self,
            Self::OwnerMessage { .. }
                | Self::AgentMessage { .. }
                | Self::OwnerSpeech { .. }
                | Self::NamedSpeech { .. }
                | Self::UnknownSpeech { .. }
                | Self::NumberedSpeech { .. }
                | Self::ScreenNotes
                | Self::Recorded { .. }
                | Self::RecordedApp { .. }
                | Self::NoDate
                | Self::NoDateSender { .. }
        )
    }
    pub(crate) fn is_gap(&self) -> bool {
        !self.is_story() && !matches!(self, Self::PulseTitle | Self::PulseBody)
    }
    pub(crate) fn map_text<U>(&self, mut f: impl FnMut(&T) -> Option<U>) -> Option<Copy<U>> {
        Some(match self {
            Self::PulseTitle => Copy::PulseTitle,
            Self::PulseBody => Copy::PulseBody,
            Self::RecentEntities { error } => Copy::RecentEntities { error: f(error)? },
            Self::MissingDay => Copy::MissingDay,
            Self::MissingSegmentId => Copy::MissingSegmentId,
            Self::MissingSegment { segment } => Copy::MissingSegment {
                segment: f(segment)?,
            },
            Self::MissingSegmentSummary { segment } => Copy::MissingSegmentSummary {
                segment: f(segment)?,
            },
            Self::InvalidSense { segment } => Copy::InvalidSense {
                segment: f(segment)?,
            },
            Self::UnreadableSense { segment, error } => Copy::UnreadableSense {
                segment: f(segment)?,
                error: f(error)?,
            },
            Self::SenseLimit { segment } => Copy::SenseLimit {
                segment: f(segment)?,
            },
            Self::MissingActivityId { activity } => Copy::MissingActivityId {
                activity: f(activity)?,
            },
            Self::MissingActivity => Copy::MissingActivity,
            Self::UnreadableActivities { facet, error } => Copy::UnreadableActivities {
                facet: f(facet)?,
                error: f(error)?,
            },
            Self::ActivityNotFound { facet, activity } => Copy::ActivityNotFound {
                facet: f(facet)?,
                activity: f(activity)?,
            },
            Self::CurrentAwareness { error } => Copy::CurrentAwareness { error: f(error)? },
            Self::ImportAwareness { error } => Copy::ImportAwareness { error: f(error)? },
            Self::OwnerMessage { date, quote } => Copy::OwnerMessage {
                date: f(date)?,
                quote: f(quote)?,
            },
            Self::AgentMessage { date, quote } => Copy::AgentMessage {
                date: f(date)?,
                quote: f(quote)?,
            },
            Self::OwnerSpeech { quote } => Copy::OwnerSpeech { quote: f(quote)? },
            Self::NamedSpeech { name, quote } => Copy::NamedSpeech {
                name: f(name)?,
                quote: f(quote)?,
            },
            Self::UnknownSpeech { quote } => Copy::UnknownSpeech { quote: f(quote)? },
            Self::NumberedSpeech { speaker, quote } => Copy::NumberedSpeech {
                speaker: f(speaker)?,
                quote: f(quote)?,
            },
            Self::ScreenNotes => Copy::ScreenNotes,
            Self::Recorded { date } => Copy::Recorded { date: f(date)? },
            Self::RecordedApp { date, app } => Copy::RecordedApp {
                date: f(date)?,
                app: f(app)?,
            },
            Self::NoDate => Copy::NoDate,
            Self::NoDateSender { sender } => Copy::NoDateSender { sender: f(sender)? },
        })
    }
}

impl Copy {
    pub(crate) fn line(&self, locale: &str, compact_owner: bool) -> String {
        match self {
            Self::OwnerSpeech { quote } if compact_owner => strings(
                locale,
                "journal.window.talent.story.line.owner",
                [("quote", quote)],
            ),
            Self::OwnerMessage { date, quote } if compact_owner => strings(
                locale,
                "journal.window.talent.story.line.message.owner",
                [("date", date), ("quote", quote)],
            ),
            _ => self.render(locale),
        }
    }
    pub fn render(&self, locale: &str) -> String {
        match self {
            Self::PulseTitle => text(locale, "journal.window.talent.pulse.title.unavailable"),
            Self::PulseBody => text(locale, "journal.window.talent.pulse.body.unavailable"),
            Self::RecentEntities { error } => strings(
                locale,
                "journal.window.talent.pulse.gap.recent",
                [("error", error)],
            ),
            Self::MissingDay => text(locale, "journal.window.talent.pulse.gap.day"),
            Self::MissingSegmentId => text(locale, "journal.window.talent.pulse.gap.segment.id"),
            Self::MissingSegment { segment } => strings(
                locale,
                "journal.window.talent.pulse.gap.segment.missing",
                [("segment", segment)],
            ),
            Self::MissingSegmentSummary { segment } => strings(
                locale,
                "journal.window.talent.pulse.gap.segment.summary",
                [("segment", segment)],
            ),
            Self::InvalidSense { segment } => strings(
                locale,
                "journal.window.talent.pulse.gap.sense.invalid",
                [("segment", segment)],
            ),
            Self::UnreadableSense { segment, error } => strings(
                locale,
                "journal.window.talent.pulse.gap.sense.read",
                [("segment", segment), ("error", error)],
            ),
            Self::SenseLimit { segment } => strings(
                locale,
                "journal.window.talent.pulse.gap.sense.limit",
                [("segment", segment)],
            ),
            Self::MissingActivityId { activity } => strings(
                locale,
                "journal.window.talent.pulse.gap.activity.id",
                [("activity", activity)],
            ),
            Self::MissingActivity => text(locale, "journal.window.talent.pulse.gap.activity.empty"),
            Self::UnreadableActivities { facet, error } => strings(
                locale,
                "journal.window.talent.pulse.gap.activities.read",
                [("facet", facet), ("error", error)],
            ),
            Self::ActivityNotFound { facet, activity } => strings(
                locale,
                "journal.window.talent.pulse.gap.activity.missing",
                [("facet", facet), ("activity", activity)],
            ),
            Self::CurrentAwareness { error } => strings(
                locale,
                "journal.window.talent.pulse.gap.awareness.current",
                [("error", error)],
            ),
            Self::ImportAwareness { error } => strings(
                locale,
                "journal.window.talent.pulse.gap.awareness.import",
                [("error", error)],
            ),
            Self::OwnerMessage { date, quote } => strings(
                locale,
                "journal.window.talent.story.message.owner",
                [("date", date), ("quote", quote)],
            ),
            Self::AgentMessage { date, quote } => strings(
                locale,
                "journal.window.talent.story.message.agent",
                [("date", date), ("quote", quote)],
            ),
            Self::OwnerSpeech { quote } => strings(
                locale,
                "journal.window.talent.story.speech.owner",
                [("quote", quote)],
            ),
            Self::NamedSpeech { name, quote } => strings(
                locale,
                "journal.window.talent.story.speech.named",
                [("name", name), ("quote", quote)],
            ),
            Self::UnknownSpeech { quote } => strings(
                locale,
                "journal.window.talent.story.speech.unknown",
                [("quote", quote)],
            ),
            Self::NumberedSpeech { speaker, quote } => strings(
                locale,
                "journal.window.talent.story.speech.numbered",
                [("speaker", speaker), ("quote", quote)],
            ),
            Self::ScreenNotes => text(locale, "journal.window.talent.story.notes.heading"),
            Self::Recorded { date } => strings(
                locale,
                "journal.window.talent.story.notes.recorded",
                [("date", date)],
            ),
            Self::RecordedApp { date, app } => strings(
                locale,
                "journal.window.talent.story.notes.recorded.app",
                [("date", date), ("app", app)],
            ),
            Self::NoDate => text(locale, "journal.window.talent.story.notes.date.missing"),
            Self::NoDateSender { sender } => strings(
                locale,
                "journal.window.talent.story.notes.date.missing.sender",
                [("sender", sender)],
            ),
        }
    }
}

use crate::stored_markdown::{Document, MAX_RECEIPT};
use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};
use solstone_core_journal_io::{JournalRoot, read_relative_file_bounded};
use std::path::{Path, PathBuf};

fn hash(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

fn display_path(source: &Path) -> PathBuf {
    source.with_extension("jsonl.display")
}

pub fn read(root: &Path, source: &Path) -> Option<Value> {
    let journal = JournalRoot::open(root).ok()?;
    let path = display_path(source);
    let observation =
        read_relative_file_bounded(&journal, path.strip_prefix(root).ok()?, MAX_RECEIPT).ok()??;
    serde_json::from_slice(&observation.bytes).ok()
}

/// Change one row under the caller's source lock, shared by every writer of these receipts.
/// Retire the previous receipt before replacing canonical bytes. A later failure
/// to save new metadata leaves the canonical record readable in English.
pub fn publish(
    root: &Path,
    source: &Path,
    key: &str,
    receipt: Option<Value>,
) -> std::io::Result<()> {
    use solstone_core_journal_io::{AtomicWriteOptions, atomic_replace};
    let path = display_path(source);
    let mut rows: Map<String, Value> = match std::fs::symlink_metadata(&path) {
        Ok(metadata) if metadata.is_file() && metadata.len() <= MAX_RECEIPT as u64 => {
            let journal = JournalRoot::open(root).map_err(std::io::Error::other)?;
            let relative = path.strip_prefix(root).map_err(std::io::Error::other)?;
            read_relative_file_bounded(&journal, relative, MAX_RECEIPT)
                .map_err(std::io::Error::other)?
                .and_then(|file| serde_json::from_slice(&file.bytes).ok())
                .unwrap_or_default()
        }
        Ok(_) if receipt.is_none() => return Ok(()), // Readers already reject this file.
        Ok(_) => {
            return Err(std::io::Error::other(
                "invalid talent display metadata file",
            ));
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Map::new(),
        Err(error) => return Err(error),
    };
    if let Some(receipt) = receipt {
        rows.insert(key.to_owned(), receipt);
    } else if rows.remove(key).is_none() {
        return Ok(());
    }
    let bytes = serde_json::to_vec(&rows)?;
    if bytes.len() > MAX_RECEIPT {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "talent display metadata is too large",
        ));
    }
    atomic_replace(path, &bytes, AtomicWriteOptions { mode: Some(0o600) })
        .map_err(std::io::Error::other)
}

pub fn pulse_receipt(record: &Map<String, Value>, fields: &Value) -> Value {
    json!({"version":1, "digest":hash(&serde_json::to_vec(record).expect("JSON record")), "fields":fields})
}

pub fn project_pulse(record: &mut Map<String, Value>, receipt: &Value, locale: &str) {
    if locale == "en-US"
        || receipt["version"] != 1
        || receipt["digest"] != hash(&serde_json::to_vec(record).expect("JSON record"))
    {
        return;
    }
    let fields = &receipt["fields"];
    if fields["fallback"] == true {
        for (field, copy) in [
            ("title", Copy::PulseTitle),
            ("one_sentence", Copy::PulseBody),
            ("full_details", Copy::PulseBody),
        ] {
            if let Some(Value::String(value)) = record.get_mut(field)
                && *value == copy.render("en-US")
            {
                *value = copy.render(locale)
            }
        }
    }
    if let Some(gaps) = record
        .get_mut("window")
        .and_then(|w| w.get_mut("gaps"))
        .and_then(Value::as_array_mut)
        && let Some(receipts) = fields["gaps"].as_array()
        && gaps.len() == receipts.len()
    {
        for (gap, receipt) in gaps.iter_mut().zip(receipts) {
            if let Some(shown) = gap
                .as_str()
                .and_then(|text| Document::project_talent(text, receipt, locale, Copy::is_gap))
            {
                *gap = shown.into()
            }
        }
    }
}

pub fn activity_path(root: &Path, facet: &str, day: &str) -> PathBuf {
    root.join("facets")
        .join(facet)
        .join("activities")
        .join(format!("{day}.jsonl"))
}

fn field_digest(record: &Map<String, Value>, field: &str) -> String {
    let edit = record
        .get("edits")
        .and_then(Value::as_array)
        .and_then(|edits| {
            edits.iter().enumerate().rev().find(|(_, edit)| {
                edit.get(crate::content::EDIT_REMOVED).is_none()
                    && edit
                        .get("fields")
                        .and_then(Value::as_array)
                        .is_some_and(|fields| fields.iter().any(|name| name == field))
            })
        });
    hash(&serde_json::to_vec(&(record.get("id"), record.get(field), edit)).expect("JSON activity"))
}

pub fn story_receipt(
    record: &Map<String, Value>,
    body: &Value,
    line_end: Option<usize>,
) -> Option<Value> {
    use crate::content::{activity_field_held, edit_names, edit_removed};
    let edit = record.get("edits")?.as_array()?.last()?.as_object()?;
    let written = |key| edit_names(edit, key) && !activity_field_held(record, key);
    if edit.get("actor")? != "story" || edit_removed(edit) || !written("story") {
        return None;
    }
    let mut lines = Map::new();
    for key in ["title", "description"] {
        if written(key) {
            lines.insert(key.into(), field_digest(record, key).into());
        }
    }
    Some(
        json!({"version":1,"digest":field_digest(record,"story"),"body":body,"lines":lines,"line_end":line_end}),
    )
}

pub fn project_story(record: &mut Map<String, Value>, receipts: &Value, locale: &str) {
    if locale == "en-US" {
        return;
    }
    let Some(receipt) = record
        .get("id")
        .and_then(Value::as_str)
        .and_then(|id| receipts.get(id))
    else {
        return;
    };
    if receipt["version"] != 1 || receipt["digest"] != field_digest(record, "story") {
        return;
    }
    let Some(body) = record.get_mut("story").and_then(|s| s.get_mut("body")) else {
        return;
    };
    let Some(shown) = body
        .as_str()
        .and_then(|s| Document::project_talent(s, &receipt["body"], locale, Copy::is_story))
    else {
        return;
    };
    // The opening line is derived only when the writer marked that exact field.
    // Screen notes come after the model body; a title from that body stays verbatim.
    let Some(source) = body.as_str() else { return };
    let english = source.to_owned();
    *body = shown.into();
    for key in ["title", "description"] {
        if receipt["lines"][key] == field_digest(record, key)
            && let Some(Value::String(value)) = record.get_mut(key)
            && let Some(end) = receipt["line_end"]
                .as_u64()
                .and_then(|n| usize::try_from(n).ok())
            && let Some(line) =
                Document::project_talent_prefix(&english, end, &receipt["body"], locale, value)
        {
            *value = line;
        }
    }
}

fn family(rel: &str) -> Option<crate::content::Family> {
    use crate::content::{Family, resolve_spec};
    let relative = rel.strip_prefix("chronicle/").unwrap_or(rel);
    let family = resolve_spec(relative)?.family;
    match family {
        Family::DayAccumulator if relative.ends_with("/talents/pulse.jsonl") => Some(family),
        Family::Activity | Family::MorningBriefing => Some(family),
        _ => None,
    }
}

pub fn indexed_path(rel: &str) -> bool {
    family(rel).is_some()
}

fn combined_stamp(bytes: &[u8], receipts: &Value) -> String {
    let mut digest = Sha256::new();
    digest.update(Sha256::digest(bytes));
    digest.update(serde_json::to_vec(receipts).expect("JSON receipt"));
    format!("{:x}", digest.finalize())
}

pub fn index_stamp(root: &Path, source: &Path, rel: &str, bytes: &[u8]) -> Option<String> {
    let family = family(rel)?;
    if family == crate::content::Family::MorningBriefing {
        return Some(hash(bytes));
    }
    Some(combined_stamp(bytes, &read(root, source)?))
}

pub struct IndexedTalent(Vec<(String, String)>);
impl IndexedTalent {
    pub fn get(&self, idx: i64, english: &str) -> Option<&str> {
        let (source, shown) = self.0.get(usize::try_from(idx).ok()?)?;
        (source == english && source != shown).then_some(shown)
    }
    pub fn load(root: &Path, rel: &str, stamp: &str, locale: &str) -> Option<Self> {
        use crate::content::{Family, parse_jsonl_objects, produce_chunks_by_shape};
        let family = family(rel)?;
        let relative = Path::new(rel);
        if relative
            .components()
            .any(|p| !matches!(p, std::path::Component::Normal(_)))
        {
            return None;
        }
        let path = if family == Family::Activity || relative.starts_with("chronicle") {
            relative.to_owned()
        } else {
            Path::new("chronicle").join(relative)
        };
        let journal = JournalRoot::open(root).ok()?;
        let bytes = read_relative_file_bounded(&journal, &path, 32 * 1024 * 1024)
            .ok()??
            .bytes;
        if family == Family::MorningBriefing {
            return (hash(&bytes) == stamp)
                .then(|| crate::content::morning_briefing_presentations(&bytes, locale))
                .flatten()
                .map(|pair| Self(vec![pair]));
        }
        let receipts = read(root, &root.join(path))?;
        if combined_stamp(&bytes, &receipts) != stamp {
            return None;
        }
        let mut rows = parse_jsonl_objects(std::str::from_utf8(&bytes).ok()?);
        let chunks = |rows: &[_]| produce_chunks_by_shape(family, Some(rel), rows).chunks;
        let original = chunks(&rows);
        for (idx, row) in rows.iter_mut().enumerate() {
            if family == Family::Activity {
                project_story(row, &receipts, locale);
            } else if let Some(receipt) = receipts.get(idx.to_string()) {
                project_pulse(row, receipt, locale);
            }
        }
        let shown = chunks(&rows);
        if original.len() != shown.len() {
            return None;
        }
        Some(Self(
            original
                .into_iter()
                .zip(shown)
                .map(|(a, b)| (a.content.trim().to_owned(), b.content.trim().to_owned()))
                .collect(),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn story_receipt_follows_the_saved_edit_and_owner_holds() {
        let saved = json!({"id":"a","story":{"body":"same"},"title":"same","description":"same",
            "edits":[{"actor":"owner:fixture","fields":["title"]},
                     {"actor":"story","fields":["story","title","description"]}]});
        let record = saved.as_object().unwrap();
        let receipt = story_receipt(record, &Value::Null, Some(4)).unwrap();
        assert!(receipt["lines"].get("title").is_none());
        assert!(receipt["lines"].get("description").is_some());
        let mut partial = saved;
        partial["edits"][1]["fields"] = json!(["description"]);
        assert!(story_receipt(partial.as_object().unwrap(), &Value::Null, Some(4)).is_none());
    }

    #[test]
    fn a_story_line_keeps_its_normalized_model_opening_before_a_marked_screen_note() {
        let opening = "You said screen notes (unverified)";
        let saved = "Said screen notes (unverified)";
        let mut document = Document::plain(format!("{opening}\n\n"));
        document.push_talent(Copy::ScreenNotes);
        let receipt = document.talent_receipt().unwrap();
        let title = format!("{saved}\n\n{}", Copy::ScreenNotes.render("en-US"));
        for tag in ["en-US", "en-XA"] {
            let locale = solstone_core_window_catalog::packaged_locale(Some(tag));
            let expected = format!("{saved}\n\n{}", Copy::ScreenNotes.render(locale));
            assert_eq!(
                Document::project_talent_prefix(
                    &document,
                    document.len(),
                    &receipt,
                    locale,
                    &title
                ),
                Some(expected)
            );
        }
    }
}
