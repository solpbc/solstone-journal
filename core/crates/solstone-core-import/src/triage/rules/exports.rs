// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use super::*;

const HEADER_LIMIT: usize = 96;

#[derive(Default)]
pub(super) struct Header {
    pub rows: usize,
    pub summary_words: usize,
    transcript: bool,
    transcription_model: bool,
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
    let mut in_disclaimer = false;
    let mut saw_blank = false;
    let mut index = 0usize;
    for raw in lines {
        let line = raw.trim();
        if line.is_empty() {
            saw_blank = true;
            in_disclaimer = false;
            continue;
        }
        if index >= HEADER_LIMIT {
            break;
        }
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
            in_disclaimer = false;
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
            in_disclaimer = false;
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
            if key == "transcription model" {
                header.transcription_model = true;
            }
            participants = matches!(
                key.as_str(),
                "attendees" | "participants" | "meeting participants" | "speakers"
            );
            in_disclaimer = false;
        } else if matches!(
            section.as_str(),
            "attendees" | "participants" | "meeting participants" | "speakers"
        ) {
            header.facts += 1;
            participants = true;
            in_disclaimer = false;
        } else if participants
            && line.split(',').count() >= 2
            && line.split(',').all(|part| speaker_label(part).is_some())
        {
            participants = false;
            in_disclaimer = false;
        } else if header.facts > 0 && machine_transcript_disclaimer(line) {
            in_disclaimer = true;
        } else if in_disclaimer
            && !saw_blank
            && line.split_whitespace().count() <= 12
            && !line.contains(':')
        {
            in_disclaimer = false;
        } else if index == 0 && (heading.is_some() || short_title(line))
            || line.len() >= 3 && line.bytes().all(|b| b == b'-' || b == b'=')
        {
            // A single free-form title is allowed. Other rows need a known structure.
            in_disclaimer = false;
        } else {
            break;
        }
        saw_blank = false;
        index += 1;
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
        || machine_transcript_disclaimer(line)
}

fn machine_transcript_disclaimer(line: &str) -> bool {
    let lower = line.trim_matches('*').to_ascii_lowercase();
    lower.starts_with("machine-generated transcript.")
        || lower.starts_with("automatically generated transcript.")
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
            r"(?P<prefix_clock>\[\d{1,2}:\d{2}(?::\d{2})?\]\s*|\d{1,3}:\d{2}(?::\d{2})?\s+(?:[-–—]\s+)?)?",
            r"(?P<first>[^*:：]{1,60}?)(?:\*\*)?",
            r"(?P<paren_clock>\s+\*?\(\s*\d{1,3}:\d{2}(?::\d{2})?\s*\)\*?)?[:：](?:\*\*)?(?:\s+|$)",
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
    let mut timed_turns = 0;
    let mut round_15_wall_clocks = 0usize;
    let mut sentence_turns = 0usize;
    let mut turn_has_sentence = false;
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
            let clock_text = matched
                .name("prefix_clock")
                .or_else(|| matched.name("paren_clock"))
                .map(|m| m.as_str());
            let timed = clock_text.is_some()
                || matched.name("stacked").is_some()
                || matched.name("clock_first").is_some();
            let round_15_wall = clock_text.is_some_and(|raw| schedule_clock_flags(raw).1);
            Some((speaker, body, timed, round_15_wall))
        });
        if let Some((speaker, body, timed, round_15_wall)) = found {
            if !has_body {
                return None;
            }
            if turns > 0 && turn_has_sentence {
                sentence_turns += 1;
            }
            let speaker = speaker.to_lowercase();
            if turns > 0 && opening_speaker.as_ref() == Some(&speaker) {
                opening_speaker = None;
            }
            changes += usize::from(last_speaker.as_ref().is_some_and(|last| last != &speaker));
            last_speaker = Some(speaker.clone());
            speakers.insert(speaker);
            turns += 1;
            timed_turns += usize::from(timed);
            round_15_wall_clocks += usize::from(round_15_wall);
            has_body = !body.is_empty();
            stacked = !has_body;
            turn_has_sentence = has_body && has_sentence(body);
            words += body.split_whitespace().count();
            if has_body && first_bodies.len() < 3 {
                first_bodies.insert(body);
            }
        } else if stacked && !line.starts_with(['#', '-', '>']) {
            has_body = true;
            turn_has_sentence |= has_sentence(line);
            words += line.split_whitespace().count();
            if first_bodies.len() < 3 {
                first_bodies.insert(line);
            }
        } else if turns > 0 {
            unmatched += 1;
            turn_has_sentence |= has_sentence(line);
        } else {
            return None;
        }
    }
    if turns > 0 && turn_has_sentence {
        sentence_turns += 1;
    }

    let single_speaker_transcription = header.transcription_model
        && speakers.len() == 1
        && timed_turns >= 3
        && timed_turns == turns;
    let is_export = turns >= 3
        && opening_speaker.is_none()
        && has_body
        && (speakers.len() >= 2 || single_speaker_transcription)
        && !speakers.iter().all(|speaker| {
            reserved_label(speaker)
                || matches!(
                    speaker.as_str(),
                    "q" | "a" | "question" | "answer" | "front" | "back"
                )
        })
        && first_bodies.len() >= 3
        && unmatched <= turns / 3
        && (header.transcript
            || fathom
            || single_speaker_transcription
            || changes >= 2
                && speakers.len() < turns
                && sentence_turns * 2 >= turns
                && sentence_turns >= 2
                && !(timed_turns == turns && round_15_wall_clocks == turns));
    is_export.then_some(if words > header.summary_words + action_words {
        Transcripts
    } else {
        Notes
    })
}

pub(crate) fn whatsapp_notice(body: &str) -> bool {
    [
        "Messages and calls are end-to-end encrypted.",
        "Berichten en oproepen worden end-to-end versleuteld.",
        "الرسائل والمكالمات مشفرة تمامًا بين الطرفين",
        "This business uses a secure service from Meta to manage this chat.",
    ]
    .iter()
    .any(|notice| body.starts_with(notice))
}

pub(crate) fn whatsapp_row(line: &str, bracketed: bool) -> Option<&str> {
    static BRACKETED: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(concat!(r"^\[(?:\p{Nd}{1,2}[-/.]\p{Nd}{1,2}[-/.]\p{Nd}{2,4}|\p{Nd}{4}[/.]\p{Nd}{1,2}[/.]\p{Nd}{1,2})(?: BE)?[,،]?\s+",
            r"\p{Nd}{1,2}[:.]\p{Nd}{2}(?:[:.]\p{Nd}{2})?(?:\s+(?:[aApP]\.?\s?[mM]\.?|[صم]))?\]\s+(.+)$")).unwrap()
    });
    static DASH: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(concat!(r"^\p{Nd}{1,4}[-/.]\p{Nd}{1,2}[-/.]\p{Nd}{1,4}[,،]?\s+(?:klo\s+)?",
            r"\p{Nd}{1,2}[:.]\p{Nd}{2}(?:[:.]\p{Nd}{2})?(?:\s+[aApP]\.?\s?[mM]\.?)?(?:\s+-\s+|:\s+)(.+)$")).unwrap()
    });
    let stamp = if bracketed { &*BRACKETED } else { &*DASH };
    stamp.captures(line)?.get(1).map(|m| m.as_str())
}

pub(super) fn whatsapp(lines: &[&str], bracketed: bool) -> bool {
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
        let Some(row) = whatsapp_row(&line, bracketed) else {
            if messages == 0 {
                return false;
            }
            body_lines += 1;
            continue;
        };
        let (label, body) = row.split_once([':', '：']).unwrap_or(("", row));
        let body = body.trim();
        if whatsapp_notice(body) {
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

pub(crate) const BIDI_MARKS: [char; 11] = [
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

pub(super) fn signal_dated(lines: &[&str]) -> bool {
    static TURN: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(
            r"^\[[0-9]{4}-[0-9]{2}-[0-9]{2},? [0-9]{2}:[0-9]{2}(?::[0-9]{2})?\]\s+(?P<speaker>[^:：]{1,60})[:：](?:\s|$)",
        )
        .unwrap()
    });
    let mut speakers = HashSet::new();
    let mut matched = 0usize;
    let mut unmatched = 0usize;
    let mut annotations = 0usize;
    for line in nonblank(lines) {
        if let Some(speaker) = TURN
            .captures(line)
            .and_then(|c| c.name("speaker"))
            .and_then(|m| speaker_label(m.as_str()))
        {
            speakers.insert(speaker.to_lowercase());
            matched += 1;
        } else if matched == 0 {
            if is_export_notice(line) {
                continue;
            }
            return false;
        } else if is_signal_annotation(line) {
            annotations += 1;
        } else if !is_export_notice(line) {
            unmatched += 1;
        }
    }
    speakers.len() >= 2
        && matched >= 2
        && unmatched <= matched
        && annotations <= matched * 3
        && !speakers.iter().all(|speaker| reserved_label(speaker))
}

fn is_signal_annotation(line: &str) -> bool {
    // signal-export renders reactions as `(- Name: emoji -)` (optionally
    // followed by a Markdown attachment link) and quoted replies with `> `.
    let reaction = line.starts_with("(- ") && line.contains(" -)");
    let quote = line.starts_with("> ");
    let attachment = line.starts_with("![") && line.contains("](") && line.ends_with(')');
    reaction || quote || attachment
}

fn bold_speaker_line(line: &str) -> Option<&str> {
    let inner = line.strip_prefix("**")?.strip_suffix("**")?.trim();
    if inner.is_empty() || inner.contains(['*', ':', '：']) {
        return None;
    }
    speaker_label(inner)
}

fn italic_clock_line(line: &str) -> Option<u64> {
    let inner = line.strip_prefix('*')?.strip_suffix('*')?.trim();
    if inner.starts_with('*') || inner.ends_with('*') {
        return None;
    }
    parse_clock_seconds(inner)
}

pub(super) fn macwhisper_stacked(lines: &[&str]) -> bool {
    let header = meeting_header(lines);
    let rows: Vec<_> = nonblank(lines).skip(header.rows).collect();
    if rows.len() < 6 {
        return false;
    }
    let mut idx = 0usize;
    if header.rows == 0 && (markdown_title(rows[0]).is_some() || short_title(rows[0])) {
        let plain = markdown_title(rows[0]).is_none();
        idx = 1;
        if plain
            && rows.get(idx).is_some_and(|line| {
                line.len() >= 3
                    && (line.bytes().all(|b| b == b'=') || line.bytes().all(|b| b == b'-'))
            })
        {
            idx += 1;
        }
    }

    let mut speakers = Vec::new();
    let mut clocks = Vec::new();
    let mut utterances = HashSet::new();
    let mut pending_speaker = None;
    let mut continuations = 0usize;
    let mut turn_words = 0usize;
    let mut sentence_turns = 0usize;
    let mut turn_has_sentence = false;
    let mut has_body = true;
    while idx < rows.len() {
        if let Some(speaker) = bold_speaker_line(rows[idx])
            && let Some(clock) = rows.get(idx + 1).and_then(|next| italic_clock_line(next))
        {
            if !has_body {
                return false;
            }
            if !speakers.is_empty() && turn_has_sentence {
                sentence_turns += 1;
            }
            speakers.push(speaker.to_lowercase());
            pending_speaker = Some(speaker);
            clocks.push(clock);
            has_body = false;
            turn_has_sentence = false;
            idx += 2;
        } else if speakers.is_empty() {
            return false;
        } else {
            let line = rows[idx];
            if markdown_title(line).is_some() {
                return false;
            }
            if let Some(speaker) = pending_speaker.take()
                && utterances.len() < 5
            {
                utterances.insert((speaker, line));
            }
            has_body = true;
            turn_has_sentence |= has_sentence(line);
            continuations += 1;
            turn_words += line.split_whitespace().count();
            idx += 1;
        }
    }
    if !speakers.is_empty() && turn_has_sentence {
        sentence_turns += 1;
    }
    let turns = speakers.len();
    if turns < 2
        || !has_body
        || continuations > turns * 4
        || (header.summary_words != 0 && turn_words <= header.summary_words)
        || !is_recording_offset_clocks(&clocks)
        || sentence_turns * 2 < turns
        || sentence_turns < 2
    {
        return false;
    }
    let distinct: HashSet<_> = speakers.iter().collect();
    if distinct.iter().all(|speaker| reserved_label(speaker))
        || repeats_in_order(&speakers, distinct.len())
        || utterances.len() < turns.min(5)
    {
        return false;
    }
    let changes = speakers
        .windows(2)
        .filter(|pair| pair[0] != pair[1])
        .count();
    if turns >= 3 {
        distinct.len() >= 2 && distinct.len() < turns && changes >= 2
    } else {
        // Two-turn exports (e.g., short clips) need an explicit 00:00 audio origin
        // in addition to the recording-offset and spoken-sentence checks.
        distinct.len() == 2 && clocks[0] == 0 && clocks[1] > 0
    }
}

pub(super) fn dated_bold_speaker(lines: &[&str]) -> bool {
    static TURN: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(
            r"^[-*]\s+(?P<date>[0-9]{4}-[0-9]{2}-[0-9]{2})\s+(?P<time>[0-9]{2}:[0-9]{2}:[0-9]{2})\s+\*\*(?P<speaker>[^*:：]{1,60})(?:[:：]\*\*|\*\*[:：])\s+(?P<body>\S.*)$",
        )
        .unwrap()
    });
    const MAX_TURN_GAP_SECS: i64 = 30 * 60;
    const MAX_RECORDING_SPAN_SECS: i64 = 6 * 60 * 60;

    let header = meeting_header(lines);
    let mut has_title = header.rows > 0;
    let mut allow_underline = false;
    let mut in_turn = false;
    let mut speakers = Vec::new();
    let mut timestamps = Vec::new();
    let mut round_schedule_clocks = 0usize;
    let mut utterances = HashSet::new();
    let mut sentence_turns = 0usize;
    let mut turn_has_sentence = false;
    let mut matched = 0usize;
    let mut unmatched = 0usize;
    let mut turn_words = 0usize;

    for line in nonblank(lines).skip(header.rows) {
        let captured = TURN.captures(line).and_then(|c| {
            let speaker = speaker_label(c.name("speaker")?.as_str())?;
            let date = jiff::civil::Date::strptime("%Y-%m-%d", c.name("date")?.as_str()).ok()?;
            let time = jiff::civil::Time::strptime("%H:%M:%S", c.name("time")?.as_str()).ok()?;
            let dt = date.to_datetime(time);
            let epoch = jiff::tz::Offset::UTC.to_timestamp(dt).ok()?.as_second();
            let round_clock = dt.second() == 0 && dt.minute() % 5 == 0;
            let body = c.name("body")?.as_str();
            Some((speaker, epoch, round_clock, body))
        });
        if let Some((speaker, epoch, round_clock, body)) = captured {
            if in_turn && turn_has_sentence {
                sentence_turns += 1;
            }
            matched += 1;
            if header.summary_words != 0 {
                turn_words += body.split_whitespace().count();
            }
            speakers.push(speaker.to_lowercase());
            timestamps.push(epoch);
            round_schedule_clocks += usize::from(round_clock);
            if utterances.len() < 5 {
                utterances.insert((speaker, body));
            }
            turn_has_sentence = has_sentence(body);
            in_turn = true;
        } else if !in_turn {
            if is_export_notice(line) {
                allow_underline = false;
                continue;
            }
            if allow_underline
                && line.len() >= 3
                && (line.bytes().all(|b| b == b'=') || line.bytes().all(|b| b == b'-'))
            {
                allow_underline = false;
                continue;
            }
            if !has_title && short_title(line) {
                has_title = true;
                allow_underline = markdown_title(line).is_none();
                continue;
            }
            return false;
        } else if !is_export_notice(line) {
            if markdown_title(line).is_some() {
                return false;
            }
            unmatched += 1;
            turn_has_sentence |= has_sentence(line);
        }
    }
    if in_turn && turn_has_sentence {
        sentence_turns += 1;
    }
    if matched < 3
        || unmatched > matched
        || (header.summary_words != 0 && turn_words <= header.summary_words)
        || round_schedule_clocks == matched
        || sentence_turns * 2 < matched
        || sentence_turns < 2
    {
        return false;
    }
    let continuous_recording = timestamps
        .windows(2)
        .all(|pair| pair[1] >= pair[0] && pair[1] - pair[0] <= MAX_TURN_GAP_SECS)
        && timestamps
            .last()
            .zip(timestamps.first())
            .is_some_and(|(last, first)| last > first && last - first <= MAX_RECORDING_SPAN_SECS);
    if !continuous_recording {
        return false;
    }
    let distinct: HashSet<_> = speakers.iter().collect();
    if distinct.len() < 2
        || distinct.len() >= matched
        || distinct.iter().all(|speaker| reserved_label(speaker))
        || repeats_in_order(&speakers, distinct.len())
        || utterances.len() < matched.min(5)
    {
        return false;
    }
    let changes = speakers
        .windows(2)
        .filter(|pair| pair[0] != pair[1])
        .count();
    changes >= 2
}

fn transcript_heading(line: &str) -> bool {
    let title = if let Some(heading) = markdown_title(line) {
        heading
    } else if short_title(line) {
        line
    } else {
        return false;
    };
    let words: Vec<String> = title
        .split(|c: char| !c.is_alphanumeric())
        .filter(|w| !w.is_empty())
        .map(|w| w.to_ascii_lowercase())
        .collect();
    if !words.iter().any(|w| w == "transcript") {
        return false;
    }
    if words.iter().any(|w| {
        matches!(
            w.as_str(),
            "note"
                | "notes"
                | "summary"
                | "summaries"
                | "chapter"
                | "chapters"
                | "outline"
                | "timestamp"
                | "timestamps"
                | "edit"
                | "edits"
                | "review"
                | "highlights"
                | "takeaways"
                | "index"
                | "todo"
                | "todos"
                | "agenda"
                | "minutes"
                | "guide"
                | "about"
                | "my"
                | "our"
                | "schedule"
                | "plan"
                | "itinerary"
                | "roster"
                | "changelog"
                | "log"
        )
    }) {
        return false;
    }
    // Accept titles where "transcript" labels the document (e.g., "… — Transcript",
    // "… — Transcript (2026-11-12)", "Episode 4 Transcript", "Transcript: …").
    let pos = words.iter().position(|w| w == "transcript").unwrap();
    let tail = &words[pos + 1..];
    pos == 0 || tail.is_empty() || tail.iter().all(|w| w.bytes().all(|b| b.is_ascii_digit()))
}

pub(super) fn timed_transcript_narration(lines: &[&str]) -> bool {
    static FULL_CLOCK: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"\[([0-9]{2}):([0-5][0-9]):([0-5][0-9])\]").unwrap());
    static LABELLED_START: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"^\[[0-9]{2}:[0-5][0-9]:[0-5][0-9]\]\s+[^:：]{1,60}[:：]\s+\S").unwrap()
    });
    let rows: Vec<_> = nonblank(lines).collect();
    let Some((&first, rest)) = rows.split_first() else {
        return false;
    };
    if !transcript_heading(first) {
        return false;
    }
    let mut body_start = None;
    for (offset, &line) in rest.iter().take(8).enumerate() {
        if FULL_CLOCK.find(line).is_some_and(|m| m.start() == 0) {
            body_start = Some(offset);
            break;
        }
        let allowed_meta = line.starts_with("http://")
            || line.starts_with("https://")
            || (line.len() >= 3 && line.bytes().all(|b| b == b'-' || b == b'='))
            || line.split_once(':').is_some_and(|(k, _)| {
                matches!(
                    k.trim().trim_matches('*').to_ascii_lowercase().as_str(),
                    "date"
                        | "source"
                        | "url"
                        | "listen"
                        | "duration"
                        | "episode"
                        | "title"
                        | "show"
                        | "published"
                )
            });
        if !allowed_meta {
            return false;
        }
    }
    let Some(start) = body_start else {
        return false;
    };
    let body = &rest[start..];
    if body
        .first()
        .is_some_and(|line| LABELLED_START.is_match(line))
    {
        return false;
    }

    let mut clocks = Vec::new();
    let mut inline_clocks = 0usize;
    let mut total_words = 0usize;
    for &line in body {
        if markdown_title(line).is_some()
            || ["- ", "* ", "+ "].iter().any(|p| line.starts_with(p))
            || line
                .split_once(". ")
                .is_some_and(|(n, _)| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()))
        {
            return false;
        }
        for caps in FULL_CLOCK.captures_iter(line) {
            let full = caps.get(0).unwrap();
            if !line[..full.start()].trim().is_empty() {
                inline_clocks += 1;
            }
            let h = caps[1].parse::<u64>().unwrap();
            let m = caps[2].parse::<u64>().unwrap();
            let s = caps[3].parse::<u64>().unwrap();
            clocks.push(h * 3600 + m * 60 + s);
        }
        let cleaned = FULL_CLOCK.replace_all(line, " ");
        total_words += cleaned.split_whitespace().count();
    }
    clocks.len() >= 3
        && clocks.windows(2).all(|pair| pair[1] > pair[0])
        && total_words >= 60
        && total_words >= clocks.len() * 18
        && (inline_clocks >= 1 || total_words >= clocks.len() * 25)
}
