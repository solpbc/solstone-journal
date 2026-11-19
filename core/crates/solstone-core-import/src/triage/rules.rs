// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use super::*;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Stage {
    Container,
    Signature,
    Turns,
    Structure,
    Image,
    Name,
    Fallback,
}

pub(super) struct Evidence<'a> {
    pub format: FileFormat,
    pub lines: &'a [&'a str],
    pub text: &'a str,
    pub bytes: &'a [u8],
    pub name: &'a str,
    pub folder: &'a Path,
}

pub(super) struct Rule {
    pub id: &'static str,
    pub priority: u16,
    pub stage: Stage,
    pub kind: Kind,
    pub source: &'static str,
    pub verified_on_real_file: bool,
    matcher: Matcher,
}

enum Matcher {
    Check(fn(&Evidence<'_>) -> bool),
    Lines {
        pattern: &'static str,
        regex: OnceLock<Regex>,
        shape: Shape,
    },
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Shape {
    Bare,
    Timed,
    Block,
    ExportBlock,
    ExportTimed,
    RoleBlock,
    RoleLine,
    Offset,
}

use Kind::{Documents, Notes, Photos, Screen, Transcripts};
use Shape::*;
use Stage::*;
use std::sync::OnceLock;

impl Rule {
    pub fn matches(&self, evidence: &Evidence<'_>) -> bool {
        match &self.matcher {
            Matcher::Check(check) => check(evidence),
            Matcher::Lines {
                pattern,
                regex,
                shape,
            } => {
                let regex = regex.get_or_init(|| Regex::new(pattern).expect("valid triage rule"));
                line_shape(evidence.lines, regex, *shape)
            }
        }
    }
}

// Each literal is an independent recognition rule. Source identifies the
// convention it recognizes. All checks are qualified by synthetic files only.
macro_rules! rule {
    ($priority:literal, $id:literal, $stage:ident, $kind:ident, $source:literal, $check:expr) => {
        Rule {
            id: $id,
            stage: $stage,
            kind: $kind,
            priority: $priority,
            source: $source,
            verified_on_real_file: false,
            matcher: Matcher::Check($check),
        }
    };
    ($priority:literal, $id:literal, $source:literal, $shape:ident, $pattern:literal) => {
        Rule {
            id: $id,
            stage: Turns,
            kind: Transcripts,
            priority: $priority,
            source: $source,
            verified_on_real_file: false,
            matcher: Matcher::Lines {
                pattern: $pattern,
                regex: OnceLock::new(),
                shape: $shape,
            },
        }
    };
}

pub(super) static RULES: LazyLock<Vec<Rule>> = LazyLock::new(|| {
    vec![
        rule!(
            10,
            "document_format",
            Container,
            Documents,
            "file-format: PDF and office document/container signatures",
            |e| is_document(e.format)
        ),
        rule!(
            20,
            "email_headers",
            Signature,
            Documents,
            "RFC 5322 header fields and common saved reply framing",
            |e| email(e.lines)
        ),
        rule!(
            21,
            "script_scenes",
            Signature,
            Documents,
            "Fountain syntax: scene headings and character cues; ACT/SCENE for plays",
            |e| script(e.lines)
        ),
        rule!(
            30,
            "webvtt_cues",
            Signature,
            Transcripts,
            "W3C WebVTT: file header and cue blocks; Zoom, Teams, Whisper",
            |e| subtitle(e.lines, true)
        ),
        rule!(
            31,
            "subrip_cues",
            Signature,
            Transcripts,
            "SubRip: numbered timing and payload blocks",
            |e| subtitle(e.lines, false)
        ),
        rule!(
            32,
            "whisper_tsv",
            Signature,
            Transcripts,
            "openai/whisper utils.py: TSV column header and offsets",
            |e| whisper_tsv(e.lines)
        ),
        rule!(
            33,
            "whisper_json",
            Signature,
            Transcripts,
            "openai/whisper utils.py: JSON output fields",
            |e| whisper_json(e.text)
        ),
        rule!(
            34,
            "discord_export",
            Signature,
            Transcripts,
            "DiscordChatExporter PlainTextMessageWriter: preamble and footer",
            |e| discord(e.lines)
        ),
        rule!(
            35,
            "signal_sigtop_long",
            Signature,
            Transcripts,
            "sigtop fmt_text.go: conversation and message fields",
            |e| sigtop(e.lines)
        ),
        rule!(
            36,
            "childes_chat",
            Signature,
            Transcripts,
            "TalkBank CHAT manual: begin, participants and main tiers",
            |e| childes(e.lines)
        ),
        rule!(
            37,
            "sms_backup_xml",
            Signature,
            Transcripts,
            "SyncTech SMS Backup & Restore XML field documentation",
            |e| sms_backup(e.text)
        ),
        rule!(
            38,
            "gutenberg_header",
            Signature,
            Documents,
            "Project Gutenberg START OF ebook boilerplate",
            |e| e.lines.iter().take(30).any(|line| {
                let line = line.trim().to_ascii_uppercase();
                [
                    "*** START OF THE PROJECT GUTENBERG EBOOK ",
                    "*** START OF THIS PROJECT GUTENBERG EBOOK ",
                ]
                .iter()
                .any(|prefix| line.starts_with(prefix) && line.ends_with("***"))
            })
        ),
        rule!(
            39,
            "existing_timed_layout",
            Signature,
            Transcripts,
            "Journal import: relative headings and two inline timed layouts",
            |e| crate::text::has_existing_timed_layout(e.text, e.lines)
                && !only_reserved_inline_labels(e.lines)
        ),
        rule!(
            50,
            "zoom_chat",
            "zoomchattr: saved chat line and block exports",
            ExportBlock,
            r"^[0-9]{2}:[0-9]{2}:[0-9]{2} From (?P<speaker>[^:]{1,100}?)(?:\s+to\s+[^:]{1,100}?)?\s*[:：](?:\s|$)"
        ),
        rule!(
            51,
            "whatsapp_bracketed",
            "WhatsApp iOS export: date/time bracket then sender; whatsapp-chat-parser",
            ExportTimed,
            r"^\[(?:[0-9]{1,2}[-/.][0-9]{1,2}[-/.][0-9]{2,4}|[0-9]{4}[/.][0-9]{1,2}[/.][0-9]{1,2}),?\s+[0-9]{1,2}[:.][0-9]{2}(?:[:.][0-9]{2})?(?:\s+[aApP]\.?\s?[mM]\.?)?\]\s+(?P<speaker>[^:：]{1,60})[:：](?:\s|$)"
        ),
        rule!(
            52,
            "whatsapp_dash",
            "WhatsApp Android export: date/time and dash then sender; whatsapp-chat-parser",
            ExportTimed,
            r"^[0-9]{1,4}[-/.][0-9]{1,2}[-/.][0-9]{1,4},?\s+[0-9]{1,2}[:.][0-9]{2}(?:[:.][0-9]{2})?(?:\s+[aApP]\.?\s?[mM]\.?)?\s+-\s+(?P<speaker>[^:：]{1,60})[:：](?:\s|$)"
        ),
        rule!(
            53,
            "signal_dated",
            "signal-export models: bracketed ISO date and time",
            ExportTimed,
            r"^\[[0-9]{4}-[0-9]{2}-[0-9]{2},? [0-9]{2}:[0-9]{2}(?::[0-9]{2})?\]\s+(?P<speaker>[^:：]{1,60})[:：](?:\s|$)"
        ),
        rule!(
            54,
            "signal_sigtop_short",
            "sigtop fmt_text_short.go: ISO minute and sender",
            ExportTimed,
            r"^[0-9]{4}-[0-9]{2}-[0-9]{2} [0-9]{2}:[0-9]{2} (?P<speaker>[^:：]{1,60})[:：](?:\s|$)"
        ),
        rule!(
            55,
            "slack_text",
            "slackdump text export: > sender [id] @ date and nested reply prefixes",
            ExportBlock,
            r"^(?:\|\s*)*>\s+(?P<speaker>[^\[\r\n]{1,60}?)\s+\[[A-Z0-9]+\]\s+@\s+[0-9]{2}/[0-9]{2}/[0-9]{4}\s+[0-9]{2}:[0-9]{2}:[0-9]{2}\s+(?:Z|[+-][0-9]{4}):$"
        ),
        rule!(
            56,
            "bracketed_offsets",
            "whisper.cpp CLI: timestamp range per line",
            Offset,
            r"^\[[0-9]{2}:[0-9]{2}:[0-9]{2}\.[0-9]{3} --> [0-9]{2}:[0-9]{2}:[0-9]{2}\.[0-9]{3}\]\s+\S"
        ),
        rule!(
            57,
            "irc_angle_nick",
            "Generic IRC timestamp and angle-bracket nick; irssi default clock",
            Timed,
            r"^[0-9]{2}:[0-9]{2}(?::[0-9]{2})?\s+<[ @+%~&]?(?P<speaker>[^>]{1,40})>\s+(?P<body>\S.*)$"
        ),
        rule!(
            58,
            "dated_tab_nick",
            "Generic dated tab-separated nick and body; WeeChat default clock",
            Timed,
            r"^[0-9]{4}-[0-9]{2}-[0-9]{2} [0-9]{2}:[0-9]{2}:[0-9]{2}(?:\.[0-9]+)?Z?\t(?P<speaker>[^\t]+)\t(?P<body>\S.*)$"
        ),
        rule!(
            59,
            "bracketed_speaker",
            "Generic bracketed clock and speaker",
            Timed,
            r"^\[[0-9]{1,2}:[0-9]{2}(?::[0-9]{2})?(?: ?[aApP][mM])?\]\s*(?P<speaker>[^:：]{1,60})[:：]\s+(?P<body>\S.*)$"
        ),
        rule!(
            60,
            "clock_dash_speaker",
            "Generic clock–speaker log",
            Timed,
            r"^[0-9]{1,2}:[0-9]{2}(?::[0-9]{2})?(?: ?[aApP][mM])?\s+[-–—]\s+(?P<speaker>[^:：]{1,60})[:：]\s+(?P<body>\S.*)$"
        ),
        rule!(
            61,
            "stacked_speaker_time",
            "Generic speaker/time block, including Otter and text rendered from documents",
            Block,
            r"^(?:\*\*)?(?P<speaker>[^*():]{1,60}?)(?:\*\*)?\s+(?:[-–—]\s+|\()?\d{1,2}:\d{2}(?::\d{2})?\)?:?$"
        ),
        rule!(
            62,
            "clock_then_speaker",
            "Generic time-first stacked speaker blocks",
            Block,
            r"^\d{1,2}:\d{2}(?::\d{2})?\s+[-–—]\s+(?P<speaker>[^():]{1,60}?)(?:\s+\([^()]+\))?:?$"
        ),
        rule!(
            63,
            "speaker_parenthesized_clock",
            "Generic speaker label with parenthesized clock",
            Timed,
            r"^(?P<speaker>[^():]{1,60}?)\s+\(\d{1,2}:\d{2}(?::\d{2})?\):(?:\s+(?P<body>\S.*)|\s*)$"
        ),
        rule!(
            64,
            "role_headers",
            "pionxzh/chatgpt-exporter Markdown and plain text; generic role headers",
            RoleBlock,
            r"(?i)^(?:#{1,6}\s+)?(?:\*\*)?(?P<speaker>you|user|human|me|them|assistant|ai|chatgpt|gpt|claude|gemini|copilot|bot|system|prompt|response|speaker [0-9]+)(?: said)?[:：]?(?:\*\*[:：]?)?\s*$"
        ),
        rule!(
            65,
            "role_labels",
            "Generic AI, interview, and microphone/system roles",
            RoleLine,
            r"(?i)^(?:\*\*)?\[?(?P<speaker>you|user|human|me|them|assistant|ai|chatgpt|gpt|claude|gemini|copilot|bot|system|prompt|response|q|a|question|answer|interviewer|interviewee|respondent|speaker [0-9]+|spk [0-9]+|participant [0-9]+|unknown speaker)\]?(?:[:：]\*\*|\*\*[:：]|[:：])\s+\S"
        ),
        rule!(
            66,
            "bold_speaker",
            "Generic Markdown bold speaker labels",
            Bare,
            r"^\*\*(?P<speaker>[^*:：]{1,60})(?:[:：]\*\*|\*\*[:：])\s+(?P<body>\S.*)$"
        ),
        rule!(
            67,
            "bare_speaker",
            "Generic recurring Name: text labels",
            Bare,
            r"^(?P<speaker>[^:：]{1,60})[:：]\s+(?P<body>\S.*)$"
        ),
        rule!(
            68,
            "dated_sender_blocks",
            Turns,
            Transcripts,
            "imessage-exporter text template: date, sender line, body",
            |e| imessage(e.lines)
        ),
        rule!(
            70,
            "chapter_markers",
            Structure,
            Documents,
            "Three numbered chapter, book or part headings",
            |e| chapter_count(e.lines) >= 3
        ),
        rule!(
            71,
            "table_of_contents",
            Structure,
            Documents,
            "Contents with at least three entries repeated as headings",
            |e| table_of_contents(e.lines)
        ),
        rule!(
            72,
            "structured_prose",
            Structure,
            Documents,
            "Text import document threshold: 1,500 words, prose, two heading levels",
            |e| structured_prose(e.lines)
        ),
        rule!(
            73,
            "long_prose",
            Structure,
            Documents,
            "Text import document threshold: 5,000 words with prose predominating",
            |e| prose_words(e.lines).0 >= 5_000 && mostly_prose(e.lines)
        ),
        rule!(
            80,
            "camera_exif",
            Image,
            Photos,
            "Exif Make, Model and exposure fields",
            |e| e.format.kind() == FormatKind::Image && camera_metadata(e.bytes)
        ),
        rule!(
            90,
            "aosp_screenshot_name",
            Name,
            Screen,
            "AOSP SystemUI ImageExporter.java file naming convention",
            |e| e.format == FileFormat::PortableNetworkGraphics && aosp_name(e.name)
        ),
        rule!(
            91,
            "aosp_screenshot_folder",
            Name,
            Screen,
            "AOSP SystemUI ImageExporter.java Pictures/Screenshots relative path (supporting evidence only)",
            |e| e.format == FileFormat::PortableNetworkGraphics
                && aosp_name(e.name)
                && e.folder.ends_with("Pictures/Screenshots")
        ),
        rule!(
            100,
            "image_default",
            Fallback,
            Photos,
            "Manual image import default",
            |e| e.format.kind() == FormatKind::Image
        ),
        rule!(
            101,
            "note_default",
            Fallback,
            Notes,
            "Generic text import default",
            |e| e.format.kind() != FormatKind::Image
        ),
    ]
});

fn is_document(format: FileFormat) -> bool {
    matches!(format.kind(), FormatKind::Document | FormatKind::Spreadsheet | FormatKind::Presentation | FormatKind::Ebook)
        // These are text and must pass through text rules if they are routed here.
        && !matches!(format, FileFormat::RichTextFormat)
}

fn nonblank<'a>(lines: &'a [&'a str]) -> impl Iterator<Item = &'a str> {
    lines
        .iter()
        .map(|line| line.trim())
        .filter(|line| !line.is_empty())
}

fn line_shape(lines: &[&str], regex: &Regex, shape: Shape) -> bool {
    let mut speakers = Vec::new();
    let mut unmatched = 0;
    let mut matched = 0;
    let mut continuations = 0;
    let mut in_turn = false;
    let mut block_has_body = true;
    let generic = matches!(shape, Bare | Timed | Block);
    let mut has_title = false;
    // Only the first five distinct pairs matter. Clocks and Markdown
    // wrappers are already outside the captured body.
    let mut utterances = HashSet::new();
    let mut pending_body = None;
    for line in nonblank(lines) {
        let captured = regex.captures(line);
        let speaker = captured
            .as_ref()
            .and_then(|c| c.name("speaker"))
            .map(|m| m.as_str().trim());
        let evidence = (shape == Offset && captured.is_some()) || speaker.is_some_and(name_label);
        if evidence {
            if matches!(shape, Block | ExportBlock | RoleBlock) && !block_has_body {
                return false;
            }
            matched += 1;
            if let Some(speaker) = speaker {
                speakers.push(speaker.to_lowercase());
            }
            in_turn = true;
            block_has_body = !matches!(shape, Block | RoleBlock);
            if generic
                && utterances.len() < 5
                && let Some(speaker) = speaker
            {
                if let Some(body) = captured.as_ref().and_then(|c| c.name("body")) {
                    utterances.insert((speaker, body.as_str()));
                    pending_body = None;
                } else {
                    pending_body = Some(speaker);
                }
            }
        } else if !in_turn {
            if is_export_notice(line) {
                continue;
            }
            let existing_heading = matches!(shape, Block | RoleBlock) && line.starts_with("# ");
            let short_heading =
                matches!(shape, Bare | Timed | Block | RoleLine | RoleBlock) && short_title(line);
            if !has_title && (existing_heading || short_heading) {
                has_title = true;
                continue;
            }
            // Generic and exporter-specific turn rules do not promote a pasted
            // exchange following a prose preamble to a transcript.
            return false;
        } else if !is_export_notice(line) {
            if matches!(shape, Bare | Timed | Block | RoleLine) && markdown_title(line).is_some() {
                return false;
            }
            unmatched += 1;
            if let Some(speaker) = pending_body.take() {
                utterances.insert((speaker, line));
            }
            if matches!(shape, Block | ExportBlock | RoleBlock) && !line.starts_with('#') {
                block_has_body = true;
                continuations += 1;
            }
        }
    }
    let distinct: HashSet<_> = speakers.iter().collect();
    if generic
        && (distinct.iter().all(|speaker| reserved_label(speaker))
            || repeats_in_order(&speakers, distinct.len())
            || utterances.len() < matched.min(5))
    {
        return false;
    }
    let changes = speakers
        .windows(2)
        .filter(|pair| pair[0] != pair[1])
        .count();
    let interleaves = changes >= 2;
    match shape {
        Offset => matched >= 3 && unmatched <= matched / 4,
        ExportBlock => distinct.len() >= 2 && matched >= 2 && continuations <= matched * 4,
        ExportTimed => distinct.len() >= 2 && matched >= 2 && unmatched <= matched,
        Block => {
            distinct.len() >= 2
                && matched >= 3
                && interleaves
                && block_has_body
                && continuations <= matched * 4
        }
        Timed => {
            matched >= 3
                && ((distinct.len() >= 2 && interleaves) || matched >= 5 && distinct.len() == 1)
                && unmatched <= matched
        }
        RoleBlock => {
            distinct.len() >= 2
                && matched >= 2
                && block_has_body
                && changes >= 1
                && continuations >= matched
        }
        RoleLine => distinct.len() >= 2 && matched >= 3 && interleaves && unmatched <= matched / 3,
        Bare => {
            let mut counts = HashMap::new();
            for speaker in &speakers {
                *counts.entry(speaker).or_insert(0) += 1;
            }
            let recurring = speakers
                .iter()
                .filter(|speaker| counts[*speaker] >= 2)
                .count();
            matched >= 6
                && (2..=12).contains(&distinct.len())
                && changes * 3 >= matched - 1
                && recurring * 5 >= matched * 4
                && unmatched <= matched / 3
        }
    }
}

fn markdown_title(line: &str) -> Option<&str> {
    let (marker, title) = line.split_once(' ')?;
    ((1..=6).contains(&marker.len()) && marker.bytes().all(|c| c == b'#')).then_some(title.trim())
}

fn short_title(line: &str) -> bool {
    if let Some(title) = markdown_title(line) {
        return (1..=10).contains(&title.split_whitespace().count());
    }
    if jiff::civil::Date::strptime("%Y-%m-%d", line).is_ok() {
        return true;
    }
    let mut words = line.split_whitespace();
    words.next().is_some_and(|first| {
        first.starts_with(char::is_uppercase)
            && !matches!(
                first.to_ascii_lowercase().as_str(),
                "i" | "we" | "this" | "these" | "here"
            )
    }) && words.count() < 10
        && !line.contains(':')
        && !line.ends_with(['.', '!', '?'])
}

fn reserved_label(label: &str) -> bool {
    matches!(
        label.trim_end_matches('.').to_ascii_lowercase().as_str(),
        "mon"
            | "tue"
            | "tues"
            | "wed"
            | "thu"
            | "thur"
            | "thurs"
            | "fri"
            | "sat"
            | "sun"
            | "monday"
            | "tuesday"
            | "wednesday"
            | "thursday"
            | "friday"
            | "saturday"
            | "sunday"
            | "jan"
            | "feb"
            | "mar"
            | "apr"
            | "jun"
            | "jul"
            | "aug"
            | "sep"
            | "sept"
            | "oct"
            | "nov"
            | "dec"
            | "january"
            | "february"
            | "march"
            | "april"
            | "may"
            | "june"
            | "july"
            | "august"
            | "september"
            | "october"
            | "november"
            | "december"
            | "breakfast"
            | "lunch"
            | "dinner"
            | "supper"
            | "snack"
            | "brunch"
            | "host"
            | "port"
            | "mode"
            | "start"
            | "lap"
            | "stop"
            | "end"
    )
}

fn repeats_in_order(speakers: &[String], distinct: usize) -> bool {
    distinct >= 3
        && speakers.len() / 2 >= distinct
        && speakers[distinct..] == speakers[..speakers.len() - distinct]
}

// The legacy parser's full clocks register at the signature stage. A log
// consisting entirely of calendar or field labels has no speaker evidence.
fn only_reserved_inline_labels(lines: &[&str]) -> bool {
    static INLINE_LABEL: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"^(?:\[[0-9]{1,3}:[0-5][0-9]:[0-5][0-9]\]\s*([^:]+):|([^():]+?)\s*\([0-9]{1,3}:[0-5][0-9]:[0-5][0-9]\))").unwrap()
    });
    let mut rows = nonblank(lines).peekable();
    if rows.peek().is_some_and(|line| short_title(line)) {
        rows.next();
    }
    let has_reserved_label = |line| {
        INLINE_LABEL.captures(line).is_some_and(|captures| {
            captures
                .get(1)
                .or_else(|| captures.get(2))
                .is_some_and(|label| reserved_label(label.as_str().trim()))
        })
    };
    rows.next().is_some_and(has_reserved_label)
        && rows.next().is_some_and(has_reserved_label)
        && rows.all(has_reserved_label)
}

fn is_export_notice(line: &str) -> bool {
    let irc_event = line.split_once(' ').is_some_and(|(clock, body)| {
        clock.len() == 5 && clock.as_bytes()[2] == b':' && body.trim_start().starts_with("-!- ")
    });
    let tab_event = line.split_once('\t').is_some_and(|(stamp, body)| {
        stamp.len() >= 19
            && stamp.as_bytes()[10] == b' '
            && body
                .split_once('\t')
                .is_some_and(|(nick, _)| matches!(nick.trim(), "-->" | "<--" | "--" | "*"))
    });
    irc_event || tab_event || line.starts_with("--- Log opened ") || line.starts_with("--- Log closed ")
        || line.starts_with("--- Day changed ")
        || line.ends_with("Messages and calls are end-to-end encrypted.")
        || line.ends_with("Messages and calls are end-to-end encrypted. No one outside of this chat, not even WhatsApp, can read or listen to them.")
        || line.starts_with("Transcribed by https://otter.ai")
        || line.starts_with("|") && line.trim_matches(|c: char| c == '|' || c.is_whitespace()).is_empty()
}

pub(super) fn subtitle(lines: &[&str], vtt: bool) -> bool {
    static VTT_TIME: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"^(?:[0-9]{2,}:)?[0-5][0-9]:[0-5][0-9]\.[0-9]{3}[ \t]+-->[ \t]+(?:[0-9]{2,}:)?[0-5][0-9]:[0-5][0-9]\.[0-9]{3}(?:[ \t]+.*)?$").unwrap()
    });
    static SRT_TIME: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"^[0-9]{2,}:[0-5][0-9]:[0-5][0-9][,.][0-9]{3}[ \t]+-->[ \t]+[0-9]{2,}:[0-5][0-9]:[0-5][0-9][,.][0-9]{3}(?:[ \t]+.*)?$").unwrap()
    });
    let lines = if vtt {
        let Some((header, rest)) = lines.split_first() else {
            return false;
        };
        if !(*header == "WEBVTT" || header.starts_with("WEBVTT ") || header.starts_with("WEBVTT\t"))
        {
            return false;
        }
        // The header and any header text end at the first blank line.
        let Some(split) = rest.iter().position(|line| line.trim().is_empty()) else {
            return false;
        };
        &rest[split + 1..]
    } else {
        lines
    };
    let mut cues = 0;
    for block in lines
        .split(|line| line.trim().is_empty())
        .filter(|block| !block.is_empty())
    {
        if vtt && (matches!(block[0], "STYLE" | "REGION" | "NOTE") || block[0].starts_with("NOTE "))
        {
            continue;
        }
        let timing = if vtt { &*VTT_TIME } else { &*SRT_TIME };
        let index = usize::from(!vtt || !timing.is_match(block[0]));
        if !vtt && (block[0].is_empty() || !block[0].bytes().all(|byte| byte.is_ascii_digit())) {
            return false;
        }
        if block.len() <= index + 1 || !timing.is_match(block[index]) {
            return false;
        }
        cues += 1;
    }
    cues >= if vtt { 1 } else { 2 }
}

fn whisper_tsv(lines: &[&str]) -> bool {
    let mut lines = nonblank(lines);
    if lines.next() != Some("start\tend\ttext") {
        return false;
    }
    let mut rows = 0;
    for row in lines {
        let mut parts = row.splitn(3, '\t');
        let endpoints = (
            parts.next().and_then(|n| n.parse::<u64>().ok()),
            parts.next().and_then(|n| n.parse::<u64>().ok()),
        );
        if !matches!(endpoints, (Some(start), Some(end)) if end >= start)
            || parts.next().is_none_or(str::is_empty)
        {
            return false;
        }
        rows += 1;
    }
    rows > 0
}

fn whisper_json(text: &str) -> bool {
    let trimmed = text.trim_start();
    if !trimmed.starts_with('{') {
        return false;
    }
    let Ok(json) = serde_json::from_str::<serde_json::Value>(trimmed) else {
        return false;
    };
    let Some(segments) = json["segments"].as_array() else {
        return false;
    };
    json["text"].is_string()
        && json["language"].is_string()
        && !segments.is_empty()
        && segments.iter().all(|segment| {
            segment["start"].is_number()
                && segment["end"].is_number()
                && segment["text"].is_string()
        })
}

fn discord(lines: &[&str]) -> bool {
    let lines: Vec<_> = nonblank(lines).collect();
    let border = |line: &str| line.len() == 62 && line.bytes().all(|byte| byte == b'=');
    lines.len() >= 9
        && border(lines[0])
        && lines[1].starts_with("Guild: ")
        && lines[2].starts_with("Channel: ")
        && border(lines[lines.len() - 1])
        && border(lines[lines.len() - 3])
        && lines[lines.len() - 2].starts_with("Exported ")
        && lines[lines.len() - 2].ends_with(" message(s)")
        && lines[3..lines.len() - 3]
            .iter()
            .any(|line| line.starts_with('[') && line.contains("] "))
}

fn sigtop(lines: &[&str]) -> bool {
    let lines: Vec<_> = nonblank(lines).collect();
    lines
        .first()
        .is_some_and(|line| line.starts_with("Conversation: "))
        && lines
            .windows(3)
            .filter(|row| {
                row[0].starts_with("From: ")
                    && row[1].starts_with("Type: ")
                    && row[2].starts_with("Sent: ")
            })
            .count()
            >= 2
}

fn childes(lines: &[&str]) -> bool {
    let lines: Vec<_> = nonblank(lines).collect();
    lines
        .first()
        .is_some_and(|line| matches!(*line, "@Begin" | "@UTF8"))
        && lines.contains(&"@Begin")
        && lines.iter().any(|line| line.starts_with("@Participants:"))
        && lines
            .iter()
            .filter(|line| line.starts_with('*') && line.contains(":\t"))
            .count()
            >= 2
}

fn sms_backup(text: &str) -> bool {
    let mut text = text.trim_start();
    if text.starts_with("<?xml")
        && let Some((_, body)) = text.split_once("?>")
    {
        text = body.trim_start();
    }
    (text.starts_with("<smses ") || text.starts_with("<smses>"))
        && text.trim_end().ends_with("</smses>")
        && (text.contains("<sms ") || text.contains("<mms "))
}

fn imessage(lines: &[&str]) -> bool {
    static HEADER: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"^[A-Z][a-z]{2} [0-9]{1,2}, [0-9]{4}\s+[0-9]{1,2}:[0-9]{2}:[0-9]{2} [AP]M(?: \(.+\))?$").unwrap()
    });
    let lines: Vec<_> = nonblank(lines).collect();
    if !lines.first().is_some_and(|line| HEADER.is_match(line)) {
        return false;
    }
    lines
        .windows(3)
        .filter(|block| {
            HEADER.is_match(block[0]) && name_label(block[1]) && !HEADER.is_match(block[2])
        })
        .count()
        >= 2
}

fn email(lines: &[&str]) -> bool {
    fn field(line: &str) -> Option<&str> {
        let (field, value) = line.trim().split_once(':')?;
        (!value.trim().is_empty()
            && matches!(field, "From" | "To" | "Cc" | "Sent" | "Date" | "Subject"))
        .then_some(field)
    }
    lines.windows(3).any(|block| {
        let fields: HashSet<_> = block.iter().filter_map(|line| field(line)).collect();
        fields.len() == 3
            && fields.contains("From")
            && (fields.contains("To") || fields.contains("Subject"))
    }) || lines.windows(2).any(|pair| {
        matches!(
            (field(pair[0]), field(pair[1])),
            (Some("From"), Some("To" | "Subject"))
        )
    }) && lines.iter().any(|line| {
        line.trim_start().starts_with('>') || line.trim() == "-----Original Message-----"
    })
}

fn script(lines: &[&str]) -> bool {
    static ACT_OR_SCENE: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(
            r"(?i)^(?:ACT|SCENE)\s+(?:[IVXLCDM]+|[0-9]+|ONE|TWO|THREE)(?:\.|\s*[:–—-]\s*\S.*)?$",
        )
        .unwrap()
    });
    static SLUG_LINE: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"(?i)^\.?(?:INT\.?/EXT\.?|I/E\.?|INT\.|EXT\.|EST\.)\s+(\S.*)$").unwrap()
    });
    let scene_heading = |line: &str| {
        ACT_OR_SCENE.is_match(line)
            || SLUG_LINE.captures(line).is_some_and(|capture| {
                let setting = &capture[1];
                setting.contains(" - ") || !setting.chars().any(char::is_lowercase)
            })
    };
    let character_cue = |line: &str| {
        let cue = line.trim_end_matches(['.', '^']);
        (2..=40).contains(&cue.chars().count())
            && !matches!(cue, "TODO" | "NOTES" | "IDEAS" | "BUDGET" | "SUMMARY")
            && cue.chars().any(char::is_uppercase)
            && cue
                .chars()
                .all(|c| c.is_uppercase() || c.is_ascii_digit() || " ._'-’".contains(c))
    };
    let Some(start) = lines.iter().position(|line| scene_heading(line.trim())) else {
        return false;
    };
    let scene = &lines[start..];
    scene
        .windows(3)
        .enumerate()
        .filter(|(index, lines)| {
            let (before, cue, next) = (lines[0].trim(), lines[1].trim(), lines[2].trim());
            let dialogue = if next.starts_with('(') && next.ends_with(')') {
                scene.get(index + 3).map_or("", |line| line.trim())
            } else {
                next
            };
            (before.is_empty() || scene_heading(before))
                && character_cue(cue)
                && !scene_heading(cue)
                && !dialogue.is_empty()
                && !character_cue(dialogue)
                && !scene_heading(dialogue)
        })
        .take(2)
        .count()
        == 2
}

fn chapter_count(lines: &[&str]) -> usize {
    static CHAPTER: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"(?i)^(?:chapter|book|part)\s+(?:[0-9]+|[ivxlcdm]+|one|two|three|four|five|six|seven|eight|nine|ten)\b").unwrap()
    });
    nonblank(lines)
        .filter(|line| CHAPTER.is_match(line.trim_start_matches('#').trim_start()))
        .count()
}

fn table_of_contents(lines: &[&str]) -> bool {
    let Some(start) = lines.iter().position(|line| {
        matches!(
            line.trim()
                .trim_start_matches('#')
                .trim()
                .to_ascii_lowercase()
                .as_str(),
            "contents" | "table of contents"
        )
    }) else {
        return false;
    };
    let after = &lines[start + 1..];
    let entries: HashSet<_> = nonblank(after)
        .take(32)
        .take_while(|line| line.len() < 100 && !line.starts_with('#'))
        .map(|line| line.trim_start_matches(['-', '*']).trim())
        .collect();
    let mut seen = HashSet::new();
    for line in nonblank(after) {
        if line.starts_with('#') {
            let heading = line.trim_start_matches('#').trim();
            if entries.contains(heading) {
                seen.insert(heading);
            }
        }
    }
    seen.len() >= 3
}

fn prose_words(lines: &[&str]) -> (usize, usize) {
    let mut prose = 0;
    let mut total = 0;
    for line in nonblank(lines) {
        let words = line.split_whitespace().count();
        total += words;
        let list = ["#", "- ", "* ", "+ ", "> "]
            .iter()
            .any(|prefix| line.starts_with(prefix))
            || line
                .split_once(". ")
                .is_some_and(|(n, _)| n.bytes().all(|byte| byte.is_ascii_digit()));
        if !list && (words >= 8 || line.ends_with(['.', '?', '!']) && words >= 4) {
            prose += words;
        }
    }
    (total, prose)
}

fn mostly_prose(lines: &[&str]) -> bool {
    let (words, prose) = prose_words(lines);
    words != 0 && prose * 10 >= words * 7
}

fn structured_prose(lines: &[&str]) -> bool {
    let (words, prose) = prose_words(lines);
    if words < 1_500 || prose * 10 < words * 7 {
        return false;
    }
    let levels: HashSet<_> = nonblank(lines)
        .filter_map(|line| line.split_once(' ').map(|(first, _)| first))
        .filter(|first| first.len() <= 6 && first.bytes().all(|byte| byte == b'#'))
        .map(str::len)
        .collect();
    levels.len() >= 2
}
