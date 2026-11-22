// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Source-English Markdown with presentation identities for the words supplied by its writer.

use std::path::Path;
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use solstone_core_journal_io::{
    AtomicWriteOptions, JournalRoot, atomic_replace, media::MediaDirectory,
};
const ENGLISH: &str = solstone_core_cli_copy::ENGLISH;
const MAX_RECEIPT: usize = 1 << 20;

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "message", rename_all = "snake_case")]
pub enum Message<T = String> {
    Topics,
    Quotes,
    Objects,
    Entities,
    Entity {
        kind: T,
        name: T,
        role: T,
        source: T,
        context: T,
    },
    PartialInput,
    PartialCounts {
        details: T,
    },
    PartialStory,
    TerminalNotice,
    TerminalClock {
        offset_ms: i64,
    },
    Speaker {
        number: serde_json::Number,
    },
    Object {
        class: T,
        frames: usize,
        photos: usize,
        most: usize,
    },
}

impl<T> Message<T> {
    fn map_text<U>(&self, mut f: impl FnMut(&T) -> Option<U>) -> Option<Message<U>> {
        Some(match self {
            Self::Topics => Message::Topics,
            Self::Quotes => Message::Quotes,
            Self::Objects => Message::Objects,
            Self::Entities => Message::Entities,
            Self::PartialInput => Message::PartialInput,
            Self::PartialStory => Message::PartialStory,
            Self::TerminalNotice => Message::TerminalNotice,
            Self::Speaker { number } => Message::Speaker {
                number: number.clone(),
            },
            Self::TerminalClock { offset_ms } => Message::TerminalClock {
                offset_ms: *offset_ms,
            },
            Self::PartialCounts { details } => Message::PartialCounts {
                details: f(details)?,
            },
            Self::Object {
                class,
                frames,
                photos,
                most,
            } => Message::Object {
                class: f(class)?,
                frames: *frames,
                photos: *photos,
                most: *most,
            },
            Self::Entity {
                kind,
                name,
                role,
                source,
                context,
            } => Message::Entity {
                kind: f(kind)?,
                name: f(name)?,
                role: f(role)?,
                source: f(source)?,
                context: f(context)?,
            },
        })
    }
}

impl Message {
    pub fn render(&self, locale: &str) -> String {
        self.render_at(locale, None)
    }

    // Render control characters in place of the writer's string slots. The English
    // template determines each range even when an owner value repeats another slot.
    fn bind_fields(&self, source: &str) -> Option<Message<std::ops::Range<usize>>> {
        let mut values = Vec::new();
        let tagged = self
            .map_text(|text| {
                values.push(text.clone());
                Some(char::from_u32(values.len() as u32)?.to_string())
            })?
            .render(ENGLISH);
        let mut ranges = vec![None; values.len()];
        let mut offset = 0;
        for c in tagged.chars() {
            let next = if let Some(id) = (c as usize).checked_sub(1).filter(|id| *id < values.len())
            {
                let value = &values[id];
                if !source.get(offset..)?.starts_with(value) {
                    return None;
                }
                let next = offset + value.len();
                ranges[id] = Some(offset..next);
                next
            } else {
                if !source.get(offset..)?.starts_with(c) {
                    return None;
                }
                offset + c.len_utf8()
            };
            offset = next;
        }
        if offset != source.len() {
            return None;
        }
        let mut ranges = ranges.into_iter();
        self.map_text(|_| ranges.next()?)
    }
    fn owner_entity(&self) -> Option<String> {
        let Self::Entity {
            kind,
            name,
            role,
            source,
            context,
        } = self
        else {
            return None;
        };
        if !matches!(kind.as_str(), "Person" | "Company" | "Project" | "Tool")
            || ![role, source]
                .iter()
                .all(|s| s.bytes().all(|b| b.is_ascii_lowercase() || b == b'_'))
        {
            return None;
        }
        Some(if context.is_empty() {
            name.clone()
        } else {
            format!("{name} — {context}")
        })
    }

    fn render_at(&self, locale: &str, segment: Option<&str>) -> String {
        match self {
            Self::Topics => {
                solstone_core_cli_copy::render_for(locale, "journal.cli.think.markdown.topics", &[])
            }
            Self::Quotes => {
                solstone_core_cli_copy::render_for(locale, "journal.cli.think.markdown.quotes", &[])
            }
            Self::Objects => solstone_core_cli_copy::render_for(
                locale,
                "journal.cli.think.markdown.objects",
                &[],
            ),
            Self::Entities => solstone_core_cli_copy::render_for(
                locale,
                "journal.cli.think.markdown.entities",
                &[],
            ),
            Self::Entity {
                kind,
                name,
                role,
                source,
                context,
            } => solstone_core_cli_copy::display_for(
                locale,
                "journal.cli.think.markdown.entity",
                [
                    ("kind", &(kind)),
                    ("name", &(name)),
                    ("role", &(role)),
                    ("source", &(source)),
                    ("context", &(context)),
                ],
            ),
            Self::PartialCounts { details } => solstone_core_cli_copy::display_for(
                locale,
                "journal.cli.think.markdown.partial.counts",
                [("details", &(details))],
            ),
            Self::PartialInput => solstone_core_cli_copy::render_for(
                locale,
                "journal.cli.think.markdown.partial.input",
                &[],
            ),
            Self::PartialStory => solstone_core_cli_copy::render_for(
                locale,
                "journal.cli.think.markdown.partial.story",
                &[],
            ),
            Self::TerminalNotice => solstone_core_cli_copy::render_for(
                locale,
                "journal.cli.think.markdown.terminal.notice",
                &[],
            ),
            Self::TerminalClock { offset_ms } => {
                let clock = segment
                    .and_then(|s| crate::content::screen_text::screen_text_clock(s, *offset_ms))
                    .unwrap_or_else(|| crate::content::screen_text::screen_text_minute(*offset_ms));
                solstone_core_cli_copy::display_for(
                    locale,
                    "journal.cli.think.markdown.terminal.clock",
                    [("clock", &clock)],
                )
            }
            Self::Speaker { number } => solstone_core_cli_copy::display_for(
                locale,
                "journal.cli.think.markdown.speaker",
                [("number", &(number))],
            ),
            Self::Object {
                class,
                frames,
                photos,
                most,
            } => match (*frames, *photos) {
                (_, 0) => {
                    if *most > 1 {
                        solstone_core_cli_copy::render_for(
                            locale,
                            "journal.cli.think.markdown.object.frames.maximum",
                            &[
                                (
                                    "frames",
                                    solstone_core_cli_copy::Value::Cardinal((*frames) as i64),
                                ),
                                ("class", solstone_core_cli_copy::Value::String(class)),
                                (
                                    "most",
                                    solstone_core_cli_copy::Value::String(&(*most).to_string()),
                                ),
                            ],
                        )
                    } else {
                        solstone_core_cli_copy::render_for(
                            locale,
                            "journal.cli.think.markdown.object.frames",
                            &[
                                (
                                    "frames",
                                    solstone_core_cli_copy::Value::Cardinal((*frames) as i64),
                                ),
                                ("class", solstone_core_cli_copy::Value::String(class)),
                            ],
                        )
                    }
                }
                (0, _) => {
                    if *most > 1 {
                        solstone_core_cli_copy::render_for(
                            locale,
                            "journal.cli.think.markdown.object.photos.maximum",
                            &[
                                (
                                    "photos",
                                    solstone_core_cli_copy::Value::Cardinal((*photos) as i64),
                                ),
                                ("class", solstone_core_cli_copy::Value::String(class)),
                                (
                                    "most",
                                    solstone_core_cli_copy::Value::String(&(*most).to_string()),
                                ),
                            ],
                        )
                    } else {
                        solstone_core_cli_copy::render_for(
                            locale,
                            "journal.cli.think.markdown.object.photos",
                            &[
                                (
                                    "photos",
                                    solstone_core_cli_copy::Value::Cardinal((*photos) as i64),
                                ),
                                ("class", solstone_core_cli_copy::Value::String(class)),
                            ],
                        )
                    }
                }
                (_, _) => {
                    if *most > 1 {
                        solstone_core_cli_copy::render_for(
                            locale,
                            "journal.cli.think.markdown.object.both.maximum",
                            &[
                                (
                                    "frames",
                                    solstone_core_cli_copy::Value::Cardinal((*frames) as i64),
                                ),
                                (
                                    "photos",
                                    solstone_core_cli_copy::Value::Cardinal((*photos) as i64),
                                ),
                                ("class", solstone_core_cli_copy::Value::String(class)),
                                (
                                    "most",
                                    solstone_core_cli_copy::Value::String(&(*most).to_string()),
                                ),
                            ],
                        )
                    } else {
                        solstone_core_cli_copy::render_for(
                            locale,
                            "journal.cli.think.markdown.object.both",
                            &[
                                (
                                    "frames",
                                    solstone_core_cli_copy::Value::Cardinal((*frames) as i64),
                                ),
                                (
                                    "photos",
                                    solstone_core_cli_copy::Value::Cardinal((*photos) as i64),
                                ),
                                ("class", solstone_core_cli_copy::Value::String(class)),
                            ],
                        )
                    }
                }
            },
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
struct Span<T = String> {
    start: usize,
    end: usize,
    copy: Message<T>,
}

#[derive(Serialize, Deserialize)]
struct Receipt<T = String> {
    version: u8,
    source_sha256: String,
    spans: Vec<Span<T>>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Document {
    text: String,
    spans: Vec<Span>,
}

impl std::fmt::Display for Document {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.text)
    }
}

impl std::ops::Deref for Document {
    type Target = str;
    fn deref(&self) -> &str {
        &self.text
    }
}

impl Document {
    pub fn plain(text: String) -> Self {
        Self {
            text,
            spans: Vec::new(),
        }
    }
    pub fn push_str(&mut self, value: &str) {
        self.text.push_str(value)
    }
    pub fn push_screen_line(&mut self, line: &str) {
        crate::content::screen_text::append_screen_text_line(&mut self.text, line);
    }
    pub fn push_message(&mut self, copy: Message) {
        let start = self.text.len();
        self.text.push_str(&copy.render(ENGLISH));
        self.spans.push(Span {
            start,
            end: self.text.len(),
            copy,
        });
    }
    pub(crate) fn render_from(&self, start: usize, locale: &str) -> String {
        let mut out = String::with_capacity(self.text.len() - start);
        let mut end = start;
        for span in self.spans.iter().filter(|span| span.start >= start) {
            out.push_str(&self.text[end..span.start]);
            out.push_str(&span.copy.render(locale));
            end = span.end;
        }
        out.push_str(&self.text[end..]);
        out
    }
    pub fn into_text(self) -> String {
        self.text
    }
    pub fn truncate(&mut self, bytes: usize) {
        self.text.truncate(bytes);
        self.spans.retain(|span| span.end <= bytes);
    }

    /// A missing or torn companion leaves the English document directly readable.
    pub fn save_display_receipt(&self, path: &Path) -> std::io::Result<()> {
        let receipt_path = path.with_extension("md.display");
        if self.spans.is_empty() {
            return match std::fs::remove_file(receipt_path) {
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
                result => result,
            };
        }
        let spans = self
            .spans
            .iter()
            .map(|span| {
                let field = self.text.get(span.start..span.end)?;
                let copy = span.copy.bind_fields(field)?;
                Some(Span {
                    start: span.start,
                    end: span.end,
                    copy,
                })
            })
            .collect::<Option<Vec<_>>>()
            .ok_or_else(|| {
                std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    "could not bind display fields",
                )
            })?;
        let receipt = Receipt {
            version: 1,
            source_sha256: format!("{:x}", Sha256::digest(self.text.as_bytes())),
            spans,
        };
        let bytes = serde_json::to_vec(&receipt).map_err(std::io::Error::other)?;
        if bytes.len() > MAX_RECEIPT {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "display receipt exceeds its read limit",
            ));
        }
        atomic_replace(&receipt_path, &bytes, AtomicWriteOptions::default())
            .map_err(std::io::Error::other)
    }
}

impl Receipt {
    fn valid(&self, source: &str) -> bool {
        if self.version != 1
            || self.source_sha256 != format!("{:x}", Sha256::digest(source.as_bytes()))
        {
            return false;
        }
        let mut end = 0;
        for span in self.spans.iter() {
            if span.start < end
                || span.start >= span.end
                || matches!(&span.copy, Message::Object { frames, photos, .. }
                    if i32::try_from(*frames).is_err() || i32::try_from(*photos).is_err())
                || source.get(span.start..span.end) != Some(span.copy.render(ENGLISH).as_str())
            {
                return false;
            }
            end = span.end;
        }
        true
    }

    fn render(&self, source: &str, locale: &str, segment: Option<&str>) -> Option<String> {
        if !self.valid(source) {
            return None;
        }
        let mut end = 0;
        let mut output = String::with_capacity(source.len());
        for span in self.spans.iter() {
            output.push_str(source.get(end..span.start)?);
            output.push_str(&span.copy.render_at(locale, segment));
            end = span.end;
        }
        output.push_str(source.get(end..)?);
        Some(output)
    }
}

pub fn project_file(
    root: &Path,
    path: &Path,
    displayed: &str,
    locale: &str,
    segment: Option<&str>,
) -> Option<String> {
    if locale == ENGLISH {
        return None;
    }
    let retained = directory(root, path)?;
    let (source, receipt) =
        read_document(&retained, path, displayed.len().checked_add(65536)?, None)?;
    let source = source.as_str();
    if displayed != source.trim() {
        return None;
    }
    Some(receipt.render(source, locale, segment)?.trim().to_owned())
}

pub fn project_sense_file(
    root: &Path,
    path: &Path,
    displayed: &str,
    locale: &str,
) -> Option<String> {
    if locale == ENGLISH {
        return None;
    }
    let retained = directory(root, path)?;
    let (source, receipt) =
        read_document(&retained, path, displayed.len().checked_add(65536)?, None)?;
    let source = source.as_str();
    if source.trim() != displayed || !receipt.valid(source) {
        return None;
    }
    let (heading, rows) = receipt.spans.split_first()?;
    if !matches!(heading.copy, Message::Entities)
        || source.get(..heading.start)? != "# "
        || !rows.iter().any(|row| row.copy.owner_entity().is_some())
    {
        return None;
    }
    let mut result = String::new();
    let mut end = heading.end;
    for span in rows {
        result.push_str(source.get(end..span.start)?);
        result.push_str(
            &span
                .copy
                .owner_entity()
                .unwrap_or_else(|| span.copy.render(locale)),
        );
        end = span.end;
    }
    result.push_str(source.get(end..)?);
    Some(result.trim().to_string())
}

fn directory(root: &Path, path: &Path) -> Option<Arc<MediaDirectory>> {
    let directory = path.parent()?.strip_prefix(root).ok()?;
    MediaDirectory::open(Arc::new(JournalRoot::open(root).ok()?), directory).ok()
}

impl Receipt<std::ops::Range<usize>> {
    fn resolve(self, source: &str) -> Option<Receipt> {
        if self.version != 1
            || self.source_sha256 != format!("{:x}", Sha256::digest(source.as_bytes()))
        {
            return None;
        }
        let mut previous = 0;
        let spans = self
            .spans
            .into_iter()
            .map(|span| {
                if span.start < previous || span.end <= span.start {
                    return None;
                }
                previous = span.end;
                let body = source.get(span.start..span.end)?;
                let mut remaining = body.len();
                let copy = span.copy.map_text(|range| {
                    let text = body.get(range.clone())?;
                    remaining = remaining.checked_sub(text.len())?;
                    Some(text.to_owned())
                })?;
                Some(Span {
                    start: span.start,
                    end: span.end,
                    copy,
                })
            })
            .collect::<Option<Vec<_>>>()?;
        Some(Receipt {
            version: self.version,
            source_sha256: self.source_sha256,
            spans,
        })
    }
}

fn read_receipt(
    retained: &MediaDirectory,
    path: &Path,
) -> Option<(Receipt<std::ops::Range<usize>>, String)> {
    let bytes = receipt_bytes(retained, path)?;
    let receipt = serde_json::from_slice(&bytes).ok()?;
    Some((receipt, format!("{:x}", Sha256::digest(&bytes))))
}

fn receipt_bytes(retained: &MediaDirectory, path: &Path) -> Option<Vec<u8>> {
    let sibling = path.with_extension("md.display");
    Some(
        retained
            .read_file_bounded(sibling.file_name()?, MAX_RECEIPT)
            .ok()??
            .bytes,
    )
}

fn read_document(
    retained: &MediaDirectory,
    path: &Path,
    limit: usize,
    expected_stamp: Option<&str>,
) -> Option<(String, Receipt)> {
    let (receipt, stamp) = read_receipt(retained, path)?;
    if expected_stamp.is_some_and(|expected| expected != stamp) {
        return None;
    }
    let bytes = retained
        .read_file_bounded(path.file_name()?, limit)
        .ok()??;
    // Equal English bytes can have different provenance (e.g. a literal speaker name).
    // A writer invalidates the old receipt before publishing its replacement.
    if format!("{:x}", Sha256::digest(receipt_bytes(retained, path)?)) != stamp {
        return None;
    }
    let source = String::from_utf8(bytes.bytes).ok()?;
    let receipt = receipt.resolve(&source)?;
    Some((source, receipt))
}

pub fn is_display_document(path: &Path) -> bool {
    path.parent()
        .and_then(Path::file_name)
        .is_some_and(|name| name == "talents")
        && matches!(
            path.file_name().and_then(|name| name.to_str()),
            Some(
                "activity.md"
                    | "topics.md"
                    | "objects.md"
                    | "sense.md"
                    | "sense.json"
                    | "screen_text.md"
            )
        )
}

/// Bind the derived index to the writer's exact presentation metadata and English source.
pub fn index_stamp(root: &Path, path: &Path, text: &str) -> Option<String> {
    if !is_display_document(path) {
        return None;
    }
    if path.file_name().is_some_and(|name| name == "sense.json") {
        return Some(format!("{:x}", Sha256::digest(text.as_bytes())));
    }
    let (receipt, stamp) = read_receipt(directory(root, path)?.as_ref(), path)?;
    (receipt.version == 1
        && receipt.source_sha256 == format!("{:x}", Sha256::digest(text.as_bytes())))
    .then_some(stamp)
}

/// The English chunk and its display are paired so an index hit can verify its exact text.
pub struct IndexedDisplay(Vec<(String, Option<String>)>);

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum DisplayUse {
    Record,
    SearchExcerpt,
}

impl IndexedDisplay {
    pub fn get(&self, idx: i64, english: &str) -> Option<&str> {
        let (original, display) = self.0.get(usize::try_from(idx).ok()?)?;
        (original == english)
            .then_some(display.as_deref())
            .flatten()
    }

    pub fn load(
        root: &Path,
        rel: &str,
        stamp: &str,
        locale: &str,
        usage: DisplayUse,
    ) -> Option<Self> {
        if locale == ENGLISH {
            return None;
        }
        let rel = Path::new(rel);
        if !is_display_document(rel)
            || rel
                .components()
                .any(|part| !matches!(part, std::path::Component::Normal(_)))
        {
            return None;
        }
        let path = root
            .join("chronicle")
            .join(rel.strip_prefix("chronicle").unwrap_or(rel));
        let retained = directory(root, &path)?;
        if path.file_name().is_some_and(|name| name == "sense.json") {
            let bytes = retained
                .read_file_bounded(path.file_name()?, 32 * 1024 * 1024)
                .ok()??;
            if format!("{:x}", Sha256::digest(&bytes.bytes)) != stamp {
                return None;
            }
            let (english, full, owner) = crate::content::sense_presentations(&bytes.bytes, locale)?;
            return Some(Self(vec![(
                english.trim().to_owned(),
                Some(if usage == DisplayUse::SearchExcerpt {
                    owner.unwrap_or(full)
                } else {
                    full
                }),
            )]));
        }
        let (source, receipt) = read_document(&retained, &path, 32 * 1024 * 1024, Some(stamp))?;
        let source = source.as_str();
        if !receipt.valid(source) {
            return None;
        }
        let rel = rel.to_str()?;
        if crate::content::screen_text::is_screen_text_path(rel) {
            let segment = crate::content::screen_text::screen_text_segment(rel);
            let mut start = 0;
            let mut spans = receipt.spans.iter().peekable();
            let mut result = Vec::new();
            for chunk in crate::content::screen_text::produce_screen_text_chunks(source).chunks {
                let end = start + chunk.content.len();
                let mut body = chunk.content.clone();
                let mut inside = Vec::new();
                while let Some(span) = spans.next_if(|span| span.start < end) {
                    if span.start < start || span.end > end {
                        continue;
                    }
                    if source.get(span.start..span.end)? != span.copy.render(ENGLISH) {
                        return None;
                    }
                    inside.push(span);
                }
                for span in inside.iter().rev() {
                    body.replace_range(
                        span.start - start..span.end - start,
                        &span.copy.render_at(locale, segment),
                    );
                }
                result.push((
                    chunk.content.trim().into(),
                    (!inside.is_empty()).then(|| body.trim().to_owned()),
                ));
                start = end;
            }
            return Some(Self(result));
        }

        // Carry the location of a writer's field through the same Markdown parser that
        // created the index. A one-character mark preserves the chunker's character budget.
        const MARK: u32 = 0xf0000;
        if source
            .chars()
            .any(|c| (MARK..=0xffffd).contains(&(c as u32)))
        {
            return None;
        }
        let mut tagged = String::with_capacity(source.len());
        let mut fields = Vec::new();
        let mut end = 0;
        for (i, span) in receipt.spans.iter().enumerate() {
            let field = source.get(span.start..span.end)?;
            if field != span.copy.render(ENGLISH) {
                return None;
            }
            let (offset, letter) = field.char_indices().find(|(_, c)| c.is_alphanumeric())?;
            let mark = MARK.checked_add(u32::try_from(i).ok()?)?;
            if mark > 0xffffd {
                return None;
            }
            tagged.push_str(source.get(end..span.start + offset)?);
            tagged.push(char::from_u32(mark)?);
            end = span.start + offset + letter.len_utf8();
            fields.push((offset, letter, field, &span.copy));
        }
        tagged.push_str(source.get(end..)?);
        let original = crate::chunker::format_markdown(source).chunks;
        let marked = crate::chunker::format_markdown(&tagged).chunks;
        if original.len() != marked.len() {
            return None;
        }
        let result = original
            .into_iter()
            .zip(marked)
            .map(|(original, marked)| {
                let mut restored = String::new();
                let mut substitutions = Vec::new();
                for ch in marked.markdown.chars() {
                    if let Some((offset, first, field, copy)) = (ch as u32)
                        .checked_sub(MARK)
                        .and_then(|id| fields.get(id as usize))
                    {
                        if let Some(start) = restored.len().checked_sub(*offset) {
                            substitutions.push((start, *field, *copy));
                        }
                        restored.push(*first);
                    } else {
                        restored.push(ch)
                    }
                }
                let english = original.markdown.trim().to_owned();
                if restored != original.markdown || substitutions.is_empty() {
                    return (english, None);
                }
                let names = usage == DisplayUse::SearchExcerpt
                    && path.file_name().is_some_and(|n| n == "sense.md")
                    && substitutions
                        .iter()
                        .any(|(_, _, m)| m.owner_entity().is_some());
                let mut next = restored.len();
                for (start, field, copy) in substitutions.into_iter().rev() {
                    let mut display = if names { copy.owner_entity() } else { None }
                        .unwrap_or_else(|| copy.render(locale));
                    let mut indexed_field = std::borrow::Cow::Borrowed(field);
                    if matches!(copy, Message::Entity { .. })
                        && !restored
                            .get(start..next)
                            .is_some_and(|part| part.starts_with(field))
                    {
                        // List items in the index have already passed through Markdown.
                        // Use the same parser on both versions of this writer-identified row.
                        let Some((normalized, normalized_display)) =
                            normalized_item(field).zip(normalized_item(&display))
                        else {
                            return (english, None);
                        };
                        indexed_field = std::borrow::Cow::Owned(normalized);
                        display = normalized_display;
                    }
                    let end = start + indexed_field.len();
                    if end > next || restored.get(start..end) != Some(indexed_field.as_ref()) {
                        return (english, None);
                    }
                    if names
                        && matches!(copy, Message::Entities)
                        && restored.get(..start) == Some("# ")
                    {
                        restored.replace_range(..end, "");
                    } else {
                        restored.replace_range(start..end, &display);
                    }
                    next = start;
                }
                (english, Some(restored.trim().to_owned()))
            })
            .collect();
        Some(Self(result))
    }
}

fn normalized_item(text: &str) -> Option<String> {
    let mut chunks = crate::chunker::format_markdown(&format!("- {text}")).chunks;
    (chunks.len() == 1).then(|| chunks.pop().unwrap().markdown)
}

pub fn project_sense_record(
    root: &Path,
    path: &Path,
    displayed: &str,
    locale: &str,
) -> Option<(String, Option<String>)> {
    if locale == ENGLISH {
        return None;
    }
    let retained = directory(root, path)?;
    let bytes = retained
        .read_file_bounded(path.file_name()?, 32 * 1024 * 1024)
        .ok()??;
    let (english, full, owner) = crate::content::sense_presentations(&bytes.bytes, locale)?;
    (english.trim() == displayed).then_some((full, owner))
}

#[cfg(test)]
mod tests {
    use super::*;
    const REL: &str = "20260202/default/080000_60/talents/topics.md";
    fn file(root: &Path, rel: &str, doc: &Document) -> std::path::PathBuf {
        let path = root.join("chronicle").join(rel);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, doc.as_bytes()).unwrap();
        doc.save_display_receipt(&path).unwrap();
        path
    }

    #[test]
    fn indexed_and_file_readers_keep_literal_twins_and_require_matching_source_and_receipt() {
        let root = tempfile::tempdir().unwrap();
        let mut doc = Document::plain("## ".into());
        doc.push_message(Message::Topics);
        doc.push_str("\n- Topics\n## ");
        doc.push_message(Message::Quotes);
        doc.push_str("\n- ");
        doc.push_message(Message::Speaker { number: 2.into() });
        doc.push_str(": Speaker 2 said Topics.\n- Speaker 2: literal label.\n");
        let path = file(root.path(), REL, &doc);
        let stamp = index_stamp(root.path(), &path, &doc).unwrap();
        let private = "en-XA";
        let speaker = Message::Speaker { number: 2.into() }.render(private);
        let topics = Message::Topics.render(private);
        let projected = project_file(root.path(), &path, doc.trim(), private, None).unwrap();
        assert!(projected.contains(&format!("## {topics}\n- Topics")));
        assert!(projected.contains(&format!("- {speaker}: Speaker 2 said Topics.")));
        assert!(projected.contains("- Speaker 2: literal label."));
        let index =
            IndexedDisplay::load(root.path(), REL, &stamp, private, DisplayUse::Record).unwrap();
        let chunks = crate::chunker::format_markdown(&doc).chunks;
        assert_eq!(chunks.len(), 3);
        for (i, chunk) in chunks.iter().enumerate() {
            let shown = index.get(i as i64, &chunk.markdown).unwrap();
            if i == 0 {
                assert_eq!(shown, format!("## {topics}\n\nTopics"));
            }
            if i == 1 {
                assert!(shown.contains(&format!("{speaker}: Speaker 2 said Topics.")));
            }
            if i == 2 {
                assert!(shown.contains("Speaker 2: literal label."));
            }
            assert!(index.get(i as i64, "prior version").is_none());
        }
        let sibling = path.with_extension("md.display");
        let receipt = std::fs::read(&sibling).unwrap();
        let mut broken: serde_json::Value = serde_json::from_slice(&receipt).unwrap();
        broken["spans"][1]["start"] = 0.into();
        std::fs::write(&sibling, serde_json::to_vec(&broken).unwrap()).unwrap();
        assert!(project_file(root.path(), &path, doc.trim(), private, None).is_none());
        assert!(
            IndexedDisplay::load(root.path(), REL, &stamp, private, DisplayUse::Record).is_none()
        );
        std::fs::write(&sibling, receipt).unwrap();
        std::fs::write(&path, "different").unwrap();
        assert!(
            IndexedDisplay::load(root.path(), REL, &stamp, private, DisplayUse::Record).is_none()
        );
        assert!(project_file(root.path(), &path, "different", private, None).is_none());
        std::fs::remove_file(&sibling).unwrap();
        assert!(index_stamp(root.path(), &path, "different").is_none());
    }

    #[test]
    fn indexed_entities_match_the_markdown_chunker_with_empty_and_formatted_contexts() {
        let root = tempfile::tempdir().unwrap();
        let rel = "20260202/default/080000_60/talents/sense.md";
        let mut doc = Document::plain("# ".into());
        doc.push_message(Message::Entities);
        doc.push_str("\n");
        let mut rows = Vec::new();
        for (name, context) in [
            ("Ada (role=literal)", ""),
            ("**Notes**", "Talked about `Rust` and **Tools**"),
        ] {
            doc.push_str("\n- ");
            let row = Message::Entity {
                kind: "Person".into(),
                name: name.into(),
                role: "mentioned".into(),
                source: "transcript".into(),
                context: context.into(),
            };
            doc.push_message(row.clone());
            rows.push(row);
        }
        let path = file(root.path(), rel, &doc);
        let stamp = index_stamp(root.path(), &path, &doc).unwrap();
        let chunks = crate::chunker::format_markdown(&doc).chunks;
        assert_eq!(chunks.len(), 2);
        for usage in [DisplayUse::SearchExcerpt, DisplayUse::Record] {
            let file = IndexedDisplay::load(root.path(), rel, &stamp, "en-XA", usage).unwrap();
            for (i, (row, chunk)) in rows.iter().zip(&chunks).enumerate() {
                let shown = file.get(i as i64, &chunk.markdown).unwrap();
                if usage == DisplayUse::SearchExcerpt {
                    assert_eq!(
                        shown,
                        ["Ada (role=literal)", "Notes — Talked about Rust and Tools"][i]
                    );
                } else {
                    let expected = crate::chunker::format_markdown(&format!(
                        "# {}\n\n- {}",
                        Message::Entities.render("en-XA"),
                        row.render("en-XA")
                    ));
                    assert_eq!(shown, expected.chunks[0].markdown);
                }
            }
        }
    }

    #[test]
    fn english_keeps_its_existing_sense_excerpt_and_private_uses_the_writer_fields() {
        let root = tempfile::tempdir().unwrap();
        let mut doc = Document::plain("# ".into());
        doc.push_message(Message::Entities);
        doc.push_str("\n\n- ");
        doc.push_message(Message::Entity {
            kind: "Person".into(),
            name: "Ada (role=reader, source=notes) — Sam".into(),
            role: "mentioned".into(),
            source: "transcript".into(),
            context: "at lunch".into(),
        });
        let path = file(
            root.path(),
            "20260202/default/080000_60/talents/sense.md",
            &doc,
        );
        let english = project_sense_file(root.path(), &path, &doc, ENGLISH)
            .or_else(|| crate::content::owner_sense_text(&doc))
            .unwrap();
        assert_eq!(
            english,
            "- Ada — Sam (role=mentioned, source=transcript) — at lunch"
        );
        assert_eq!(
            project_sense_file(root.path(), &path, &doc, "en-XA").unwrap(),
            "- Ada (role=reader, source=notes) — Sam — at lunch"
        );
    }

    #[test]
    fn a_receipt_with_an_out_of_range_object_count_falls_back_to_english() {
        let root = tempfile::tempdir().unwrap();
        let mut doc = Document::default();
        doc.push_message(Message::Object {
            class: "person".into(),
            frames: 2,
            photos: 1,
            most: 1,
        });
        let path = file(root.path(), REL, &doc);
        let sibling = path.with_extension("md.display");
        let mut receipt: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&sibling).unwrap()).unwrap();
        for key in ["frames", "photos"] {
            let original = receipt["spans"][0]["copy"][key].clone();
            receipt["spans"][0]["copy"][key] = (u64::from(i32::MAX as u32) + 1).into();
            std::fs::write(&sibling, serde_json::to_vec(&receipt).unwrap()).unwrap();
            assert!(project_file(root.path(), &path, &doc, "en-XA", None).is_none());
            receipt["spans"][0]["copy"][key] = original;
        }
    }

    #[test]
    fn terminal_chunks_keep_escaped_lines_and_convert_only_the_writer_clock() {
        let root = tempfile::tempdir().unwrap();
        let rel = "20260202/default/080000_60/talents/screen_text.md";
        let mut doc = Document::default();
        doc.push_message(Message::TerminalNotice);
        doc.push_str("\n\n## ");
        let clock = Message::TerminalClock { offset_ms: 12000 };
        doc.push_message(clock.clone());
        doc.push_str("\n");
        doc.push_screen_line("## 00:12 · terminal");
        let path = file(root.path(), rel, &doc);
        let stamp = index_stamp(root.path(), &path, &doc).unwrap();
        let chunks = crate::content::screen_text::produce_screen_text_chunks(&doc).chunks;
        let index =
            IndexedDisplay::load(root.path(), rel, &stamp, "en-XA", DisplayUse::Record).unwrap();
        assert_eq!(
            index.get(0, chunks[0].content.trim()),
            Some(Message::TerminalNotice.render("en-XA").as_str())
        );
        let shown = index.get(1, chunks[1].content.trim()).unwrap();
        assert!(shown.starts_with(&format!(
            "## {}",
            clock.render_at("en-XA", Some("080000_60"))
        )));
        assert!(shown.ends_with(r"- \#\# 00\:12 · terminal"));
    }
}
