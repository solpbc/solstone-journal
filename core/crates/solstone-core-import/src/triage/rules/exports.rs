// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use super::*;

const HEADER_LIMIT: usize = 96;

#[derive(Default)]
pub(super) struct Header {
    pub rows: usize,
    pub summary_words: usize,
    transcript: bool,
    facts: usize,
}

impl Header {
    pub(super) fn opening_paragraph_speaker<'a>(&self, lines: &[&'a str]) -> Option<&'a str> {
        if self.rows == 0 || self.transcript {
            return None;
        }

        fn inline_speaker(line: &str) -> Option<&str> {
            let (label, body) = line.split_once([':', '：'])?;
            let body = body.strip_prefix("**").unwrap_or(body);
            if body.starts_with(char::is_whitespace) {
                speaker_label(label)
            } else {
                None
            }
        }

        let mut lines = lines.iter().map(|line| line.trim());
        for _ in 0..self.rows {
            lines.find(|line| !line.is_empty())?;
        }
        let speaker = inline_speaker(lines.find(|line| !line.is_empty())?)?;
        // A paragraph before the dialogue can start with a colon label. Its
        // label needs a later turn to substantiate it as a speaker.
        if lines
            .by_ref()
            .take_while(|line| !line.is_empty())
            .any(|line| inline_speaker(line).is_some())
            || !lines.any(|line| !line.is_empty())
        {
            return None;
        }
        Some(speaker)
    }
}

pub(super) fn meeting_header(lines: &[&str]) -> Header {
    static DATE: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"^(?:[A-Z][a-z]{2,8} [0-9]{1,2}, [0-9]{4}|[0-9]{4}-[0-9]{2}-[0-9]{2}(?:[ T][0-9:]+Z?)?)$").unwrap()
    });
    let mut header = Header::default();
    let mut summary = false;
    let mut participants = false;
    let mut exporter_summary = false;
    for (index, line) in nonblank(lines).take(HEADER_LIMIT).enumerate() {
        let line = line.strip_prefix("> ").unwrap_or(line);
        let line = line.strip_prefix("- ").unwrap_or(line);
        let heading = markdown_title(line);
        let label = heading.unwrap_or(line).trim_matches(['*', ':', '=', ' ']);
        let field_line = line.trim_matches('*');
        let field = field_line
            .split_once(':')
            .map(|(name, _)| name.trim_matches('*'));
        let key = field.unwrap_or_default().to_ascii_lowercase();
        let section = label.to_lowercase();
        exporter_summary |= recording_link(line)
            || line.starts_with("**Date:** ")
                && line.contains(" | **Duration:** ")
                && line.contains(" | **Participants:** ")
            || line.starts_with("Source: https://notes.granola.ai/");
        let is_transcript = section == "transcript"
            || section == "transcripción de conversación"
            || index == 1 && section.ends_with(" - transcript");
        if is_transcript {
            header.transcript = true;
            summary = false;
        } else if header.transcript {
            if !meeting_notice(line) {
                break;
            }
        } else if matches!(
            section.as_str(),
            "overview" | "summary" | "action items" | "notes" | "my notes" | "ai summaries"
        ) && exporter_summary
        {
            summary = true;
        } else if summary {
            if !recording_link(line) {
                header.summary_words += line.split_whitespace().count();
            }
        } else if matches!(
            key.as_str(),
            "title"
                | "meeting"
                | "meeting title"
                | "date"
                | "date started"
                | "time"
                | "duration"
                | "attendees"
                | "participants"
                | "meeting participants"
                | "speakers"
                | "source"
                | "wiki source"
                | "media title"
                | "upload date"
                | "generated at"
                | "transcription model"
                | "recording id"
                | "share url"
                | "internal url"
        ) || DATE.is_match(line)
            || recording_link(line)
        {
            header.facts += 1;
            participants = matches!(
                key.as_str(),
                "attendees" | "participants" | "meeting participants" | "speakers"
            );
        } else if matches!(
            section.as_str(),
            "attendees" | "participants" | "meeting participants" | "speakers"
        ) {
            header.facts += 1;
            participants = true;
        } else if participants
            && line.split(',').count() >= 2
            && line.split(',').all(|part| speaker_label(part).is_some())
        {
            participants = false;
        } else if index == 0 && (heading.is_some() || short_title(line))
            || line.len() >= 3 && line.bytes().all(|b| b == b'-' || b == b'=')
        {
            // A single free-form title is allowed. Other rows need a known structure.
        } else {
            break;
        }
        header.rows += 1;
    }
    if summary || header.rows == HEADER_LIMIT || header.facts == 0 && !header.transcript {
        Header::default()
    } else {
        header
    }
}

fn recording_link(line: &str) -> bool {
    let line = line.strip_prefix("- ").unwrap_or(line);
    line.starts_with("[View Recording](https://fathom.video/")
        || line.starts_with("VIEW RECORDING - ") && line.contains(": https://fathom.video/")
}

fn meeting_notice(line: &str) -> bool {
    let line = markdown_title(line).unwrap_or(line).trim_matches('*');
    let body = line.split_once(' ').map_or(line, |(_, body)| body);
    body.starts_with("editable transcript was computer generated and might contain errors.")
        || line.starts_with("View in Fireflies: https://app.fireflies.ai/")
}

pub(super) fn interview_title(line: &str) -> bool {
    let line = markdown_title(line).unwrap_or(line);
    let title = line.to_ascii_lowercase();
    title == "interview" || title.starts_with("interview: ") || title.starts_with("interview with ")
}

pub(super) fn meeting_kind(lines: &[&str]) -> Option<Kind> {
    static TURN: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(concat!(
            r"^(?:[-*] )?(?:\*\*)?(?:",
            r"(?:\[\d{1,2}:\d{2}(?::\d{2})?\]\s*|\d{1,3}:\d{2}(?::\d{2})?\s+(?:[-–—]\s+)?)?",
            r"(?P<first>[^*:：]{1,60}?)(?:\*\*)?",
            r"(?:\s+\*?\(\s*\d{1,3}:\d{2}(?::\d{2})?\s*\)\*?)?[:：](?:\*\*)?(?:\s+|$)",
            r"|(?P<stacked>[^*:：]{1,60}?)(?:\*\*)?\s+[-–—]\s+\d{1,3}:\d{2}(?::\d{2})?$",
            r"|(?:\d{1,3}:\d{2}(?::\d{2})?\s+[-–—]\s+)(?P<clock_first>[^*:：]{1,60}?)(?:\*\*)?$",
            r")(?P<body>.*)$"
        ))
        .unwrap()
    });
    static OFFSET: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"^(?:\d{1,3}:)?[0-5]?\d:[0-5]\d$").unwrap());
    let header = meeting_header(lines);
    let fathom = nonblank(lines).take(header.rows).any(recording_link);
    if !header.transcript && header.facts < 2 && !fathom {
        return None;
    }

    let mut opening_speaker = header
        .opening_paragraph_speaker(lines)
        .map(str::to_lowercase);
    let mut speakers = HashSet::new();
    let mut turns = 0;
    let mut words = 0;
    let mut first_bodies = HashSet::new();
    let mut unmatched = 0;
    let mut has_body = true;
    let mut stacked = false;
    let mut action_words = 0;
    let mut changes = 0;
    let mut last_speaker = None;
    for line in nonblank(lines).skip(header.rows) {
        if meeting_notice(line) || OFFSET.is_match(line) {
            continue;
        }
        if fathom {
            if let Some(action) = line.strip_prefix("ACTION ITEM: ")
                && turns == 0
            {
                action_words += action.split_whitespace().count();
                continue;
            }
            if line.starts_with("- WATCH: https://fathom.video/")
                || (line.starts_with("BOOKMARK: ") || line.starts_with("HIGHLIGHT: "))
                    && line.contains(" - WATCH: https://fathom.video/")
            {
                continue;
            }
        }
        let found = TURN.captures(line).and_then(|matched| {
            let label = matched
                .name("first")
                .or_else(|| matched.name("stacked"))
                .or_else(|| matched.name("clock_first"))?;
            let speaker = speaker_label(label.as_str())?;
            let body = matched.name("body").map_or("", |body| body.as_str());
            Some((speaker, body))
        });
        if let Some((speaker, body)) = found {
            if !has_body {
                return None;
            }
            let speaker = speaker.to_lowercase();
            if turns > 0 && opening_speaker.as_ref() == Some(&speaker) {
                opening_speaker = None;
            }
            changes += usize::from(last_speaker.as_ref().is_some_and(|last| last != &speaker));
            last_speaker = Some(speaker.clone());
            speakers.insert(speaker);
            turns += 1;
            has_body = !body.is_empty();
            stacked = !has_body;
            words += body.split_whitespace().count();
            if has_body && first_bodies.len() < 3 {
                first_bodies.insert(body);
            }
        } else if stacked && !line.starts_with(['#', '-', '>']) {
            has_body = true;
            words += line.split_whitespace().count();
            if first_bodies.len() < 3 {
                first_bodies.insert(line);
            }
        } else if turns > 0 {
            unmatched += 1;
        } else {
            return None;
        }
    }

    let is_export = turns >= 3
        && opening_speaker.is_none()
        && has_body
        && speakers.len() >= 2
        && !speakers.iter().all(|speaker| {
            reserved_label(speaker)
                || matches!(
                    speaker.as_str(),
                    "q" | "a" | "question" | "answer" | "front" | "back"
                )
        })
        && first_bodies.len() >= 3
        && unmatched <= turns / 3
        && (header.transcript || fathom || changes >= 2 && speakers.len() < turns);
    is_export.then_some(if words > header.summary_words + action_words {
        Transcripts
    } else {
        Notes
    })
}

pub(super) fn whatsapp(lines: &[&str], bracketed: bool) -> bool {
    static BRACKETED: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(concat!(r"^\[(?:\p{Nd}{1,2}[-/.]\p{Nd}{1,2}[-/.]\p{Nd}{2,4}|\p{Nd}{4}[/.]\p{Nd}{1,2}[/.]\p{Nd}{1,2})(?: BE)?[,،]?\s+",
            r"\p{Nd}{1,2}[:.]\p{Nd}{2}(?:[:.]\p{Nd}{2})?(?:\s+(?:[aApP]\.?\s?[mM]\.?|[صم]))?\]\s+(.+)$")).unwrap()
    });
    static DASH: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(concat!(r"^\p{Nd}{1,4}[-/.]\p{Nd}{1,2}[-/.]\p{Nd}{1,4}[,،]?\s+(?:klo\s+)?",
            r"\p{Nd}{1,2}[:.]\p{Nd}{2}(?:[:.]\p{Nd}{2})?(?:\s+[aApP]\.?\s?[mM]\.?)?(?:\s+-\s+|:\s+)(.+)$")).unwrap()
    });
    let stamp = if bracketed { &*BRACKETED } else { &*DASH };
    let mut speakers = HashSet::new();
    let mut messages = 0;
    let mut body_lines = 0;
    let mut marked = false;
    for line in nonblank(lines) {
        let line = if line.contains(BIDI_MARKS) {
            Cow::Owned(line.replace(BIDI_MARKS, ""))
        } else {
            Cow::Borrowed(line)
        };
        let Some(row) = stamp.captures(&line).and_then(|m| m.get(1)) else {
            if messages == 0 {
                return false;
            }
            body_lines += 1;
            continue;
        };
        let row = row.as_str();
        let (label, body) = row.split_once([':', '：']).unwrap_or(("", row));
        let body = body.trim();
        if [
            "Messages and calls are end-to-end encrypted.",
            "Berichten en oproepen worden end-to-end versleuteld.",
            "الرسائل والمكالمات مشفرة تمامًا بين الطرفين",
            "This business uses a secure service from Meta to manage this chat.",
        ]
        .iter()
        .any(|notice| body.starts_with(notice))
        {
            marked = true;
        } else if let Some(speaker) = speaker_label(label) {
            speakers.insert(speaker.to_lowercase());
            messages += 1;
        } else if [
            " created group ",
            " added ",
            " changed the group ",
            " joined using ",
            "You were added",
            "You created group ",
        ]
        .iter()
        .any(|notice| row.contains(notice))
        {
            // Exporter events are not turns or continuation prose.
        } else if messages == 0 {
            return false;
        } else {
            body_lines += 1;
        }
    }
    messages >= 2
        && if marked {
            body_lines <= messages * 4
        } else {
            speakers.len() >= 2 && body_lines <= messages
        }
}

const BIDI_MARKS: [char; 11] = [
    '\u{200e}', '\u{200f}', '\u{202a}', '\u{202b}', '\u{202c}', '\u{202d}', '\u{202e}', '\u{2066}',
    '\u{2067}', '\u{2068}', '\u{2069}',
];

pub(super) fn otter(lines: &[&str]) -> bool {
    static CLOCK: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"^(?:(?P<speaker>[^:]{1,60}?)\s{2,})?\d{1,3}:[0-5]\d(?::[0-5]\d)?$").unwrap()
    });
    let lines: Vec<_> = nonblank(lines).collect();
    let Some((&"Transcribed by https://otter.ai", body)) = lines.split_last() else {
        return false;
    };
    let rows = body.iter().copied().skip(meeting_header(body).rows);
    stacked_blocks(rows, &CLOCK)
}

pub(super) fn rev(lines: &[&str]) -> bool {
    static CLOCK: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"^Speaker [0-9]+ \(\s*\d{1,3}:[0-5]\d(?::[0-5]\d)?\s*\):$").unwrap()
    });
    stacked_blocks(nonblank(lines), &CLOCK)
}

fn stacked_blocks<'a>(lines: impl Iterator<Item = &'a str>, clock: &Regex) -> bool {
    let mut turns = 0;
    let mut has_body = false;
    for line in lines {
        if let Some(found) = clock.captures(line)
            && found
                .name("speaker")
                .is_none_or(|label| speaker_label(label.as_str()).is_some())
        {
            if turns > 0 && !has_body {
                return false;
            }
            turns += 1;
            has_body = false;
        } else if turns == 0 {
            return false;
        } else {
            has_body = true;
        }
    }
    turns >= 2 && has_body
}

pub(super) fn ai_markdown(lines: &[&str], text: &str) -> bool {
    static CLAUDE: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"^# (Human|Claude) — (?:[0-9]{4}-[0-9]{2}-[0-9]{2} [0-9]{2}:[0-9]{2}:[0-9]{2}|[A-Z][a-z]{2} [0-9]{1,2}, [0-9]{4}, [0-9]{1,2}:[0-9]{2} [AP]M)$").unwrap()
    });
    static CHATGPT: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"^(?:#### )?(You|ChatGPT)(?: said)?:$").unwrap());
    static GEMINI: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"^#{2,3} (User|Gemini)$").unwrap());
    static CHAT_ANCHOR: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"^#{2,3} chat-[0-9]+$").unwrap());
    let rows: Vec<_> = nonblank(lines).collect();
    if rows.is_empty() {
        return false;
    }
    let metadata = text
        .lines()
        .next()
        .filter(|line| *line == "---")
        .map(|_| text.lines().skip(1).take_while(|line| *line != "---"));
    let has_exporter = metadata.is_some_and(|metadata| {
        ["author: ", "count: ", "exporter: "]
            .iter()
            .all(|key| metadata.clone().any(|line| line.starts_with(key)))
    });
    if has_exporter
        && rows
            .split(|line| CHAT_ANCHOR.is_match(line))
            .skip(1)
            .filter(|turn| {
                turn.first().is_some_and(|line| line.starts_with("> "))
                    && turn[1..].iter().any(|line| {
                        !line.starts_with(['>', '#']) && line.chars().any(char::is_alphabetic)
                    })
            })
            .take(2)
            .count()
            == 2
    {
        return true;
    }
    let gemini = rows.get(1).is_some_and(|line| {
        line.starts_with("> Source: [Google Gemini](https://gemini.google.com/")
    });
    let mut rows = rows.as_slice();
    if gemini {
        rows = &rows[1..];
        while let Some((line, rest)) = rows.split_first() {
            if !line.starts_with("> ")
                && !line.starts_with("<!-- gemini-export: ")
                && !line.starts_with("## turn-")
                && *line != "## Conversation outline"
                && !line.split_once(". [Turn ").is_some_and(|(index, entry)| {
                    index.parse::<usize>().is_ok() && entry.contains("](#turn-")
                })
            {
                break;
            }
            rows = rest;
        }
        return role_export(rows, &GEMINI);
    }
    let chatgpt_browser = rows[0].starts_with("[Skip to content](https://chatgpt.com/")
        || rows[0] == "Skip to content";
    if chatgpt_browser {
        rows = &rows[1..];
        if !rows
            .first()
            .is_some_and(|line| matches!(*line, "Chat history" | "## Chat history"))
        {
            return false;
        }
        rows = &rows[1..];
    } else if markdown_title(rows[0]).is_some()
        && !CLAUDE.is_match(rows[0])
        && !CHATGPT.is_match(rows[0])
    {
        rows = &rows[1..];
    }
    role_export(rows, &CLAUDE)
        || (chatgpt_browser
            || rows
                .windows(2)
                .any(|pair| pair.iter().all(|line| CHATGPT.is_match(line))))
            && role_export(rows, &CHATGPT)
}

fn role_export(lines: &[&str], matcher: &Regex) -> bool {
    let mut role = None;
    let mut roles_with_body = HashSet::new();
    for &line in lines {
        if let Some(found) = matcher.captures(line).and_then(|m| m.get(1)) {
            role = Some(found.as_str());
        } else if let Some(role) = role {
            if !line.starts_with("<!--") && !line.starts_with("## turn-") {
                roles_with_body.insert(role);
            }
        } else {
            return false;
        }
    }
    roles_with_body.len() == 2
}
