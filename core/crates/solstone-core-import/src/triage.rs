// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Deterministic classification for generic text and manual image imports.

use std::borrow::Cow;
use std::collections::{HashMap, HashSet};
use std::io::Cursor;
use std::path::Path;
use std::sync::LazyLock;

use file_format::{FileFormat, Kind as FormatKind};
use regex::Regex;
use serde::Serialize;
use solstone_core_format::content::kinds::KINDS;
pub use solstone_core_format::content::kinds::Kind;

mod image;
mod rules;
use rules::{Evidence, RULES, Rule, Stage};

pub const RULESET_VERSION: u32 = 7;
pub use solstone_core_format::content::IMPORT_KIND_FILENAME as METADATA_FILENAME;

/// Rule provenance is static for the version written with each verdict.
pub fn rule_provenance(id: &str) -> Option<(&'static str, bool)> {
    RULES
        .iter()
        .find(|rule| rule.id == id)
        .map(|rule| (rule.source, rule.verified_on_real_file))
}

pub fn write_metadata(
    path: &Path,
    metadata: &impl Serialize,
) -> Result<(), solstone_core_journal_io::AtomicWriteError> {
    let bytes = serde_json::to_vec(metadata).expect("a triage verdict is serializable");
    solstone_core_journal_io::atomic_replace(
        path,
        &bytes,
        solstone_core_journal_io::AtomicWriteOptions { mode: Some(0o600) },
    )
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Verdict {
    #[serde(serialize_with = "serialize_kind")]
    pub kind: Kind,
    pub reasons: Vec<&'static str>,
    pub ruleset_version: u32,
}

fn serialize_kind<S: serde::Serializer>(kind: &Kind, s: S) -> Result<S::Ok, S::Error> {
    let word = KINDS
        .iter()
        .find(|spec| spec.kind == *kind)
        .expect("registered content kind")
        .stored_word;
    s.serialize_str(word)
}

/// Classify without I/O, without a model, and without consulting the system locale.
/// A declared kind uses the format registry's canonical stored word.
pub fn classify(
    bytes: &[u8],
    name: &str,
    folder: &Path,
    declared_kind: Option<&str>,
    owner_choice: Option<Kind>,
) -> Verdict {
    classify_with_rules(
        bytes,
        name,
        folder,
        declared_kind,
        owner_choice,
        RULES.iter(),
    )
}

fn classify_with_rules<'a>(
    bytes: &[u8],
    name: &str,
    folder: &Path,
    declared_kind: Option<&str>,
    owner_choice: Option<Kind>,
    rules: impl Iterator<Item = &'a Rule> + Clone,
) -> Verdict {
    let declared =
        declared_kind.and_then(|word| KINDS.iter().find(|spec| spec.stored_word == word));
    let finish = |kind, reasons| Verdict {
        kind,
        reasons,
        ruleset_version: RULESET_VERSION,
    };
    if let Some(kind) = owner_choice {
        return finish(kind, vec!["owner_choice"]);
    }
    if let Some(spec) = declared {
        return finish(spec.kind, vec!["declared_kind"]);
    }

    // The ZIP/CFB readers refine their magic signature using the container directory.
    // No members are extracted and nothing is opened from a path or the network.
    let format =
        FileFormat::from_reader(Cursor::new(bytes)).unwrap_or(FileFormat::ArbitraryBinaryData);
    let base = Evidence {
        format,
        lines: &[],
        text: "",
        name,
        folder,
        image: (format.kind() == FormatKind::Image).then(|| image::ImageMetadata::read(bytes)),
        meeting_kind: std::cell::OnceCell::new(),
    };
    let decide = |stage, evidence: &Evidence<'_>| {
        let mut hits: Vec<_> = rules
            .clone()
            .filter(|rule| rule.stage == stage && rule.matches(evidence))
            .collect();
        hits.sort_unstable_by_key(|rule| rule.priority);
        hits.first()
            .map(|winner| finish(winner.kind, hits.iter().map(|rule| rule.id).collect()))
    };
    if let Some(verdict) = decide(Stage::Container, &base) {
        return verdict;
    }
    let decoded = if format.kind() == FormatKind::Image {
        None
    } else {
        decode_text(bytes)
    };
    let normalized = decoded.as_deref().map(analysis_text);
    let lines = normalized.as_deref().map(scoring_lines).unwrap_or_default();
    let evidence = Evidence {
        lines: &lines,
        text: decoded.as_deref().unwrap_or_default(),
        ..base
    };
    for stage in [
        Stage::Signature,
        Stage::Turns,
        Stage::Structure,
        Stage::Image,
        Stage::Name,
        Stage::Fallback,
    ] {
        if let Some(verdict) = decide(stage, &evidence) {
            return verdict;
        }
    }
    unreachable!("the fallback stage always matches")
}

/// Decode BOM-marked UTF-16 and UTF-8. Use Windows-1252 only for structurally
/// recognized SubRip in the loader, so arbitrary binary bytes cannot become text.
pub(crate) fn decode_text(bytes: &[u8]) -> Option<Cow<'_, str>> {
    if let Some((little, raw)) = bytes
        .strip_prefix(&[0xff, 0xfe])
        .map(|raw| (true, raw))
        .or_else(|| bytes.strip_prefix(&[0xfe, 0xff]).map(|raw| (false, raw)))
    {
        if !raw.len().is_multiple_of(2) {
            return None;
        }
        let words: Vec<_> = raw
            .chunks_exact(2)
            .map(|pair| {
                let pair = [pair[0], pair[1]];
                if little {
                    u16::from_le_bytes(pair)
                } else {
                    u16::from_be_bytes(pair)
                }
            })
            .collect();
        return String::from_utf16(&words)
            .ok()
            .filter(|s| !s.contains('\0'))
            .map(Cow::Owned);
    }
    let utf8 = bytes.strip_prefix(b"\xef\xbb\xbf").unwrap_or(bytes);
    if let Ok(text) = std::str::from_utf8(utf8) {
        return (!text.contains('\0')).then_some(Cow::Borrowed(text));
    }
    if !bytes.contains(&0) {
        const WINDOWS_1252: [char; 32] = [
            '€', '\u{81}', '‚', 'ƒ', '„', '…', '†', '‡', 'ˆ', '‰', 'Š', '‹', 'Œ', '\u{8d}', 'Ž',
            '\u{8f}', '\u{90}', '‘', '’', '“', '”', '•', '–', '—', '˜', '™', 'š', '›', 'œ',
            '\u{9d}', 'ž', 'Ÿ',
        ];
        let legacy: String = bytes
            .iter()
            .map(|&byte| match byte {
                0x80..=0x9f => WINDOWS_1252[usize::from(byte - 0x80)],
                _ => char::from(byte),
            })
            .collect();
        if rules::subtitle(&scoring_lines(&analysis_text(&legacy)), false) {
            return Some(Cow::Owned(legacy));
        }
    }
    None
}

fn analysis_text(text: &str) -> Cow<'_, str> {
    if !text.contains(['\r', '\u{a0}', '\u{202f}']) {
        return Cow::Borrowed(text);
    }
    Cow::Owned(
        text.replace("\r\n", "\n")
            .replace('\r', "\n")
            .replace(['\u{a0}', '\u{202f}'], " "),
    )
}

/// Ignore front matter and code only during scoring. The written text is untouched.
fn scoring_lines(text: &str) -> Vec<&str> {
    let mut lines: Vec<_> = text.lines().collect();
    if lines.first() == Some(&"---")
        && let Some(end) = lines
            .iter()
            .skip(1)
            .position(|line| matches!(line.trim(), "---" | "..."))
    {
        lines.drain(..end + 2);
    }
    let mut fence: Option<&str> = None;
    lines
        .into_iter()
        .map(|line| {
            let line = line.trim_start_matches([
                '\u{200e}', '\u{200f}', '\u{202a}', '\u{202b}', '\u{202c}', '\u{202d}', '\u{202e}',
                '\u{2066}', '\u{2067}', '\u{2068}', '\u{2069}',
            ]);
            let trimmed = line.trim_start();
            if let Some(marker) = fence {
                if trimmed.starts_with(marker)
                    && trimmed
                        .chars()
                        .all(|c| c == marker.as_bytes()[0] as char || c.is_whitespace())
                {
                    fence = None;
                }
                return "";
            }
            if let Some(marker) = ["```", "~~~"]
                .into_iter()
                .find(|marker| trimmed.starts_with(marker))
            {
                fence = Some(marker);
                return "";
            }
            line
        })
        .collect()
}

fn speaker_label(label: &str) -> Option<&str> {
    let label = label.trim().trim_matches('*').trim();
    let label = match label.rsplit_once(" (") {
        Some((name, annotation)) if annotation.ends_with(')') => {
            let annotation = annotation.trim_end_matches(')').trim();
            // Dictionary headings use grammatical labels in the same position
            // as the pronouns or affiliation in a speaker's display name.
            if matches!(
                annotation.to_ascii_lowercase().as_str(),
                "noun"
                    | "verb"
                    | "adjective"
                    | "adj"
                    | "adverb"
                    | "adv"
                    | "pronoun"
                    | "preposition"
                    | "conjunction"
                    | "interjection"
                    | "determiner"
                    | "article"
            ) {
                return None;
            }
            name
        }
        _ => label,
    };
    static EMOJI: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"^(?:\p{Extended_Pictographic}|\p{Regional_Indicator})(?:\p{Extended_Pictographic}|\p{Regional_Indicator}|[\x{fe0f}\x{200d}\x{1f3fb}-\x{1f3ff}])*$").unwrap()
    });
    // The Slack export includes the literal <external> namespace in the label.
    let external_id = label.strip_prefix("<external>:").is_some_and(|id| {
        !id.is_empty()
            && id
                .bytes()
                .all(|b| b.is_ascii_uppercase() || b.is_ascii_digit())
    });
    (name_label(label) || label.chars().count() <= 20 && EMOJI.is_match(label) || external_id)
        .then_some(label)
}

fn name_label(label: &str) -> bool {
    let label = label.trim();
    !label.is_empty()
        && label.chars().count() <= 40
        && label.split_whitespace().count() <= 5
        && label
            .chars()
            .any(|c| c.is_alphabetic() || c.is_ascii_digit())
        && label
            .chars()
            .all(|c| c.is_alphanumeric() || " ._'’+-@".contains(c))
        && !matches!(
            label.to_ascii_lowercase().as_str(),
            "note"
                | "todo"
                | "warning"
                | "tip"
                | "update"
                | "summary"
                | "status"
                | "owner"
                | "date"
                | "title"
                | "subject"
                | "re"
                | "prep"
                | "cook"
                | "serves"
                | "yield"
                | "ingredients"
                | "step"
                | "pros"
                | "cons"
                | "example"
                | "source"
                | "tags"
                | "author"
                | "from"
                | "to"
                | "sent"
                | "cc"
                | "http"
                | "https"
                | "mailto"
                | "info"
                | "error"
                | "warn"
                | "debug"
                | "trace"
                | "agenda"
                | "attendees"
                | "decisions"
                | "action items"
                | "priority"
        )
}

#[cfg(test)]
mod tests;
