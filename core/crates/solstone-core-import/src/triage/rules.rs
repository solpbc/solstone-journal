// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use super::*;
use solstone_core_format::content::conversation::{ZOOM_CHAT_LINE, subtitle};

mod exports;
pub(crate) use exports::{BIDI_MARKS, whatsapp_notice, whatsapp_row};
mod screenshot_names;
use screenshot_names::{aosp_png_name, dated_name};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Stage {
    Container,
    Signature,
    StructuredExtension,
    Turns,
    Structure,
    Image,
    Name,
    Fallback,
}

pub(super) struct Evidence<'a> {
    pub bytes: &'a [u8],
    pub format: FileFormat,
    pub lines: &'a [&'a str],
    pub text: &'a str,
    pub name: &'a str,
    pub folder: &'a Path,
    pub image: Option<image::ImageMetadata>,
    pub meeting_kind: std::cell::OnceCell<Option<Kind>>,
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
// convention it recognizes. Verification requires a winning real-file verdict.
macro_rules! rule {
    (@verified) => { false };
    (@verified $verified:literal) => { $verified };
    ($priority:literal, $id:literal, $stage:ident, $kind:ident, $source:literal, $check:expr $(, $verified:literal)?) => {
        Rule {
            id: $id,
            stage: $stage,
            kind: $kind,
            priority: $priority,
            source: $source,
            verified_on_real_file: rule!(@verified $($verified)?),
            matcher: Matcher::Check($check),
        }
    };
    ($priority:literal, $id:literal, $source:literal, $shape:ident, $pattern:expr $(, $verified:literal)?) => {
        Rule {
            id: $id,
            stage: Turns,
            kind: Transcripts,
            priority: $priority,
            source: $source,
            verified_on_real_file: rule!(@verified $($verified)?),
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
            "file-format and container markers: PDF, office, EPUB, and OpenDocument signatures",
            |e| is_document(e.format) || solstone_core_doc_extract::is_supported_container(e.bytes)
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
            |e| script(e.lines),
            true
        ),
        rule!(
            24,
            "macwhisper_json",
            Signature,
            Transcripts,
            "MacWhisper JSON export: text and segments with speaker or UUID segment IDs",
            |e| crate::structured_exports::parse_macwhisper_json(e.text).is_some(),
            true
        ),
        rule!(
            25,
            "podcast_transcript_json",
            Signature,
            Transcripts,
            "Podcasting 2.0 JSON transcript: version and segments with startTime, endTime and body",
            |e| crate::structured_exports::parse_podcast_transcript_json(e.text).is_some(),
            true
        ),
        rule!(
            26,
            "telegram_json",
            Signature,
            Transcripts,
            "Telegram Desktop single-chat JSON export: messages with Telegram type, timestamp and text",
            |e| crate::structured_exports::parse_telegram_json(e.text).is_some(),
            true
        ),
        rule!(
            27,
            "telegram_html",
            Signature,
            Transcripts,
            "Telegram Desktop HTML chat export: chat_page history and message blocks",
            |e| crate::structured_exports::parse_telegram_html(e.text).is_some(),
            true
        ),
        rule!(
            28,
            "slack_json",
            Signature,
            Transcripts,
            "Slack channel export JSON: message array with Slack ts timestamps and user IDs",
            |e| crate::structured_exports::parse_slack_json(e.text).is_some(),
            true
        ),
        rule!(
            29,
            "fireflies_csv",
            Signature,
            Transcripts,
            "Fireflies CSV transcript export: Speaker, Start Time, End Time, Sentence",
            |e| crate::structured_exports::parse_fireflies_csv(e.text).is_some(),
            true
        ),
        rule!(
            30,
            "webvtt_cues",
            Signature,
            Transcripts,
            "W3C WebVTT: file header and cue blocks; Zoom, Teams, Whisper",
            |e| subtitle(e.lines, true),
            true
        ),
        rule!(
            31,
            "subrip_cues",
            Signature,
            Transcripts,
            "SubRip: numbered timing and payload blocks",
            |e| subtitle(e.lines, false),
            true
        ),
        rule!(
            32,
            "whisper_tsv",
            Signature,
            Transcripts,
            "openai/whisper utils.py: TSV column header and offsets",
            |e| crate::structured_exports::parse_whisper_tsv(e.text).is_some(),
            true
        ),
        rule!(
            33,
            "whisper_json",
            Signature,
            Transcripts,
            "openai/whisper utils.py: JSON output fields",
            |e| crate::structured_exports::parse_whisper_json(e.text).is_some(),
            true
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
            40,
            "structured_document_extension",
            StructuredExtension,
            Documents,
            "general structured document extensions (.csv, .tsv, .html, .htm, .eml) after transcript signatures",
            is_structured_document_extension
        ),
        rule!(
            42,
            "otter_export",
            Turns,
            Transcripts,
            "Otter transcript footer and stacked offset/body blocks",
            |e| exports::otter(e.lines),
            true
        ),
        rule!(
            43,
            "rev_numbered_speaker_clock",
            Turns,
            Transcripts,
            "Rev numbered speaker and parenthesized clock blocks",
            |e| exports::rev(e.lines),
            true
        ),
        rule!(
            44,
            "timed_transcript_narration",
            Turns,
            Transcripts,
            "Explicitly titled transcript with monotonic [HH:MM:SS] narration clocks",
            |e| exports::timed_transcript_narration(e.lines),
            true
        ),
        rule!(
            45,
            "meeting_summary_note",
            Turns,
            Notes,
            "A meeting export whose notes and action items outweigh its turns",
            |e| *e
                .meeting_kind
                .get_or_init(|| exports::meeting_kind(e.lines))
                == Some(Notes)
        ),
        rule!(
            46,
            "meeting_export_turns",
            Turns,
            Transcripts,
            "Meeting export metadata and bracketed, inline or stacked speaker turns",
            |e| *e
                .meeting_kind
                .get_or_init(|| exports::meeting_kind(e.lines))
                == Some(Transcripts),
            true
        ),
        rule!(
            47,
            "macwhisper_stacked",
            Turns,
            Transcripts,
            "MacWhisper Markdown stacked bold speaker, italic clock and body blocks",
            |e| exports::macwhisper_stacked(e.lines),
            true
        ),
        rule!(
            48,
            "dated_bold_speaker",
            Turns,
            Transcripts,
            "MacWhisper Markdown dated bullet and bold speaker turns",
            |e| exports::dated_bold_speaker(e.lines)
        ),
        rule!(
            50,
            "zoom_chat",
            "zoomchattr: saved chat line and block exports",
            ExportBlock,
            ZOOM_CHAT_LINE
        ),
        rule!(
            51,
            "whatsapp_bracketed",
            Turns,
            Transcripts,
            "WhatsApp iOS export timestamps, system notices and senders",
            |e| exports::whatsapp(e.lines, true),
            true
        ),
        rule!(
            52,
            "whatsapp_dash",
            Turns,
            Transcripts,
            "WhatsApp Android export timestamps, system notices and senders",
            |e| exports::whatsapp(e.lines, false),
            true
        ),
        rule!(
            53,
            "signal_dated",
            Turns,
            Transcripts,
            "signal-export models: bracketed ISO date and time, reactions and quotes",
            |e| exports::signal_dated(e.lines),
            true
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
            r"^(?:\|\s*)*>\s+(?P<speaker>[^\[\r\n]{1,60}?)\s+\[[A-Z0-9]+\]\s+@\s+[0-9]{2}/[0-9]{2}/[0-9]{4}\s+[0-9]{2}:[0-9]{2}:[0-9]{2}\s+(?:Z|[+-][0-9]{4}):$",
            true
        ),
        rule!(
            56,
            "bracketed_offsets",
            "whisper.cpp CLI: timestamp range per line",
            Offset,
            r"^\[[0-9]{2}:[0-9]{2}:[0-9]{2}\.[0-9]{3} --> [0-9]{2}:[0-9]{2}:[0-9]{2}\.[0-9]{3}\]\s+\S",
            true
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
            r"^[0-9]{4}-[0-9]{2}-[0-9]{2} [0-9]{2}:[0-9]{2}:[0-9]{2}(?:\.[0-9]+)?Z?\t[ @+%~&]?(?P<speaker>[^\t]+)\t(?P<body>\S.*)$"
        ),
        rule!(
            59,
            "bracketed_speaker",
            "Generic bracketed clock or HH:MM:SS range and speaker",
            Timed,
            r"^(?:\*\*)?\[(?:(?P<range_clock>[0-9]{1,2}:[0-5][0-9]:[0-5][0-9](?:\.[0-9]{1,3})?)\s*[-–—]\s*[0-9]{1,2}:[0-5][0-9]:[0-5][0-9](?:\.[0-9]{1,3})?|(?P<clock>[0-9]{1,2}:[0-9]{2}(?::[0-9]{2})?(?: ?[aApP][mM])?))\]\s*(?P<speaker>[^:：]{1,60})[:：](?:\*\*)?\s+(?P<body>\S.*)$",
            true
        ),
        rule!(
            60,
            "clock_dash_speaker",
            "Generic clock–speaker log",
            Timed,
            r"^(?P<clock>[0-9]{1,2}:[0-9]{2}(?::[0-9]{2})?(?: ?[aApP][mM])?)\s+[-–—]\s+(?P<speaker>[^:：]{1,60})[:：]\s+(?P<body>\S.*)$"
        ),
        rule!(
            61,
            "stacked_speaker_time",
            "Generic speaker/time block, including Otter and text rendered from documents",
            Block,
            r"^(?:\*\*)?(?P<speaker>[^*:]{1,60}?)(?:\*\*)?\s+(?:[-–—]\s+|\()?(?P<clock>\d{1,2}:\d{2}(?::\d{2})?)\)?:?$",
            true
        ),
        rule!(
            62,
            "clock_then_speaker",
            "Generic time-first stacked speaker blocks",
            Block,
            r"^(?P<clock>\d{1,2}:\d{2}(?::\d{2})?)\s+[-–—]\s+(?P<speaker>[^():]{1,60}?)(?:\s+\([^()]+\))?:?$"
        ),
        rule!(
            63,
            "speaker_parenthesized_clock",
            "Generic speaker label with parenthesized clock",
            Timed,
            r"^(?P<speaker>[^:]{1,60}?)\s+\*?\(\s*(?P<clock>\d{1,2}:\d{2}(?::\d{2})?)\s*\)\*?:(?:\s+(?P<body>\S.*)|\s*)$"
        ),
        rule!(
            64,
            "role_headers",
            "pionxzh/chatgpt-exporter Markdown and plain text; generic role headers",
            RoleBlock,
            r"(?i)^(?:#{1,6}\s+)?(?:\*\*)?(?P<speaker>you|user|human|me|them|assistant|ai|chatgpt|gpt|claude|gemini|copilot|bot|system|prompt|response|speaker [0-9]+)(?: said)?[:：]?(?:\*\*[:：]?)?\s*$",
            true
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
            r"^(?:[-*] )?\*\*(?P<speaker>[^*:：]{1,60})(?:[:：]\*\*|\*\*[:：])\s+(?P<body>\S.*)$"
        ),
        rule!(
            67,
            "bare_speaker",
            "Generic recurring Name: text labels",
            Bare,
            r"^(?P<speaker>[^:：]{1,60})[:：]\s+(?P<body>\S.*)$",
            true
        ),
        rule!(
            68,
            "dated_sender_blocks",
            Turns,
            Transcripts,
            "imessage-exporter text template: date, sender line, body",
            |e| imessage(e.lines),
            true
        ),
        rule!(
            69,
            "ai_chat_markdown_export",
            Turns,
            Transcripts,
            "Claude timestamped, ChatGPT role-heading, Gemini and AI Chat Exporter Markdown conventions",
            |e| exports::ai_markdown(e.lines, e.text),
            true
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
            |e| e.image.as_ref().is_some_and(|metadata| metadata.camera),
            true
        ),
        rule!(
            81,
            "screenshot_user_comment",
            Image,
            Screen,
            "Exact Exif UserComment, Exif ImageDescription or XMP UserComment written by Apple screenshots",
            |e| e
                .image
                .as_ref()
                .is_some_and(|metadata| metadata.screenshot_comment),
            true
        ),
        rule!(
            82,
            "android_screenshot_software",
            Image,
            Screen,
            "Android screenshot Software build identifier without camera Make",
            |e| e
                .image
                .as_ref()
                .is_some_and(|metadata| metadata.android_software),
            true
        ),
        rule!(
            83,
            "gnome_screenshot_software",
            Image,
            Screen,
            "GNOME screenshot PNG Software text",
            |e| e
                .image
                .as_ref()
                .is_some_and(|metadata| metadata.gnome_software),
            true
        ),
        rule!(
            84,
            "greenshot_screenshot_software",
            Image,
            Screen,
            "Exact Greenshot Software text in a PNG",
            |e| e
                .image
                .as_ref()
                .is_some_and(|metadata| metadata.greenshot_software),
            true
        ),
        rule!(
            85,
            "spectacle_screenshot_coordinates",
            Image,
            Screen,
            "KDE Spectacle PNG text keys logicalX and logicalY",
            |e| e
                .image
                .as_ref()
                .is_some_and(|metadata| metadata.spectacle_coordinates),
            true
        ),
        rule!(
            90,
            "aosp_screenshot_name",
            Name,
            Screen,
            "AOSP SystemUI ImageExporter.java PNG date and time name",
            aosp_png_name,
            true
        ),
        rule!(
            91,
            "aosp_screenshot_folder",
            Name,
            Screen,
            "AOSP Pictures/Screenshots relative path (supporting evidence only)",
            |e| aosp_png_name(e) && e.folder.ends_with("Pictures/Screenshots")
        ),
        rule!(
            92,
            "android_spectacle_screenshot_name",
            Name,
            Screen,
            "Android and KDE Spectacle compact PNG date and time name separated by an underscore",
            dated_name!(
                png,
                r"[Ss]creenshot[ _](?<date>[0-9]{8})[ _](?<time>[0-9]{6})",
                "%Y%m%d",
                "%H%M%S"
            ),
            true
        ),
        rule!(
            93,
            "android_screenshot_name",
            Name,
            Screen,
            "Android JPEG compact date and time name",
            dated_name!(
                jpeg,
                r"[Ss]creenshot[ _](?<date>[0-9]{8})-(?<time>[0-9]{6})",
                "%Y%m%d",
                "%H%M%S"
            ),
            true
        ),
        rule!(
            94,
            "android_app_screenshot_name",
            Name,
            Screen,
            "Android JPEG compact date and time with the foreground app name",
            dated_name!(
                jpeg,
                r"[Ss]creenshot[ _](?<date>[0-9]{8})[ _](?<time>[0-9]{6})[ _][^/\r\n]+",
                "%Y%m%d",
                "%H%M%S"
            ),
            true
        ),
        rule!(
            95,
            "xiaomi_screenshot_name",
            Name,
            Screen,
            "Xiaomi JPEG timestamp with milliseconds and a package name",
            dated_name!(
                jpeg,
                r"[Ss]creenshot[ _](?<date>[0-9]{4}-[0-9]{2}-[0-9]{2})-(?<time>[0-9]{2}-[0-9]{2}-[0-9]{2})-[0-9]{3}[ _][a-zA-Z0-9_]+(?:\.[a-zA-Z0-9_]+)+",
                "%Y-%m-%d",
                "%H-%M-%S"
            ),
            true
        ),
        rule!(
            96,
            "macos_screenshot_name",
            Name,
            Screen,
            "macOS PNG timestamp names in English, German, French, Russian and Simplified Chinese",
            screenshot_names::macos_name,
            true
        ),
        rule!(
            97,
            "snipping_tool_screenshot_name",
            Name,
            Screen,
            "Windows Snipping Tool dated PNG name with optional numeric copy suffix",
            dated_name!(
                png,
                r"(?<prefix>.+)[ _](?<date>[0-9]{4}-[0-9]{2}-[0-9]{2})[ _](?<time>[0-9]{6})(?:[ _]\([0-9]+\))?",
                "%Y-%m-%d",
                "%H%M%S"
            ),
            true
        ),
        rule!(
            98,
            "chromeos_screenshot_name",
            Name,
            Screen,
            "ChromeOS PNG date and dot-separated time name",
            dated_name!(
                png,
                r"[Ss]creenshot[ _](?<date>[0-9]{4}-[0-9]{2}-[0-9]{2})[ _](?<time>[0-9]{2}\.[0-9]{2}\.[0-9]{2})",
                "%Y-%m-%d",
                "%H.%M.%S"
            ),
            true
        ),
        rule!(
            99,
            "grim_screenshot_name",
            Name,
            Screen,
            "grim PNG date and time with the grim suffix",
            dated_name!(
                png,
                r"(?<date>[0-9]{8})[ _](?<time>[0-9]{2}h[0-9]{2}m[0-9]{2})s[ _]grim",
                "%Y%m%d",
                "%Hh%Mm%S"
            ),
            true
        ),
        rule!(
            100,
            "hyprshot_screenshot_name",
            Name,
            Screen,
            "hyprshot PNG date and time with the hyprshot suffix",
            dated_name!(
                png,
                r"(?<date>[0-9]{4}-[0-9]{2}-[0-9]{2})-(?<time>[0-9]{6})[ _]hyprshot",
                "%Y-%m-%d",
                "%H%M%S"
            ),
            true
        ),
        rule!(
            101,
            "scrot_screenshot_name",
            Name,
            Screen,
            "scrot PNG date, time and dimensions with the scrot suffix",
            dated_name!(
                png,
                r"(?<date>[0-9]{4}-[0-9]{2}-[0-9]{2})-(?<time>[0-9]{6})[ _][0-9]+x[0-9]+[ _]scrot",
                "%Y-%m-%d",
                "%H%M%S"
            ),
            true
        ),
        rule!(
            102,
            "cleanshot_screenshot_name",
            Name,
            Screen,
            "CleanShot PNG date and time for a 2x capture",
            dated_name!(
                png,
                r"[Cc]leanShot[ _](?<date>[0-9]{4}-[0-9]{2}-[0-9]{2})[ _]at[ _](?<time>[0-9]{2}\.[0-9]{2}\.[0-9]{2})@2x",
                "%Y-%m-%d",
                "%H.%M.%S"
            )
        ),
        rule!(
            103,
            "greenshot_screenshot_name",
            Name,
            Screen,
            "Greenshot PNG date and time with the observed Microsoft PowerToys ending",
            dated_name!(
                png,
                r"(?<date>[0-9]{4}-[0-9]{2}-[0-9]{2})[ _](?<time>[0-9]{2}[ _][0-9]{2}[ _][0-9]{2})-Microsoft[ _]PowerToys[ _]Greenshot",
                "%Y-%m-%d",
                "%H %M %S"
            )
        ),
        rule!(
            104,
            "gofullpage_screenshot_name",
            Name,
            Screen,
            "GoFullPage PNG site, date and time name",
            dated_name!(
                png,
                r"[Ss]creencapture-[A-Za-z0-9._-]+-(?<date>[0-9]{4}-[0-9]{2}-[0-9]{2})-(?<time>[0-9]{2}[ _][0-9]{2}[ _][0-9]{2})",
                "%Y-%m-%d",
                "%H %M %S"
            ),
            true
        ),
        rule!(
            110,
            "image_default",
            Fallback,
            Photos,
            "Manual image import default",
            |e| e.format.kind() == FormatKind::Image,
            true
        ),
        rule!(
            111,
            "note_default",
            Fallback,
            Notes,
            "Generic text import default",
            |e| e.format.kind() != FormatKind::Image,
            true
        ),
    ]
});

fn is_document(format: FileFormat) -> bool {
    matches!(format.kind(), FormatKind::Document | FormatKind::Spreadsheet | FormatKind::Presentation | FormatKind::Ebook)
        // These are text and must pass through text rules if they are routed here.
        && !matches!(format, FileFormat::RichTextFormat)
}

fn is_structured_document_extension(e: &Evidence<'_>) -> bool {
    if matches!(
        e.format.kind(),
        FormatKind::Image | FormatKind::Audio | FormatKind::Video
    ) {
        return false;
    }
    Path::new(e.name)
        .extension()
        .and_then(|ext| ext.to_str())
        .is_some_and(|ext| {
            matches!(
                ext.to_ascii_lowercase().as_str(),
                "csv" | "tsv" | "html" | "htm" | "eml"
            )
        })
}

fn nonblank<'a>(lines: &'a [&'a str]) -> impl Iterator<Item = &'a str> {
    lines
        .iter()
        .map(|line| line.trim())
        .filter(|line| !line.is_empty())
}

fn line_shape(lines: &[&str], regex: &Regex, shape: Shape) -> bool {
    let mut speakers = Vec::new();
    let mut clocks = Vec::new();
    let mut range_turns = 0usize;
    let mut single_clock_turns = 0usize;
    let mut round_5_clocks = 0usize;
    let mut round_15_wall_clocks = 0usize;
    let mut sentence_turns = 0usize;
    let mut turn_has_sentence = false;
    let mut unmatched = 0;
    let mut matched = 0;
    let mut continuations = 0;
    let mut in_turn = false;
    let mut block_has_body = true;
    let generic = matches!(shape, Bare | Timed | Block);
    let header = if matches!(shape, Bare | Timed | Block | RoleLine | RoleBlock) {
        exports::meeting_header(lines)
    } else {
        exports::Header::default()
    };
    let mut has_title = header.rows > 0;
    let mut allow_underline = false;
    let mut turn_words = 0;
    let interview = nonblank(lines).next().is_some_and(exports::interview_title);
    // Only the first five distinct pairs matter. Clocks and Markdown
    // wrappers are already outside the captured body.
    let mut utterances = HashSet::new();
    let mut pending_body = None;
    let mut all_language_labels = true;
    let mut numeric_bodies = 0usize;
    for line in nonblank(lines).skip(header.rows) {
        let captured = regex.captures(line);
        let block_clock = if shape == Block {
            captured
                .as_ref()
                .and_then(|c| c.name("clock"))
                .and_then(|m| parse_clock_seconds(m.as_str()))
        } else {
            None
        };
        let speaker = captured
            .as_ref()
            .and_then(|c| c.name("speaker"))
            .and_then(|m| speaker_label(m.as_str()));
        let evidence = (shape == Offset && captured.is_some())
            || (speaker.is_some() && (shape != Block || block_clock.is_some()));
        if evidence {
            if matches!(shape, Block | ExportBlock | RoleBlock) && !block_has_body {
                return false;
            }
            if in_turn && turn_has_sentence {
                sentence_turns += 1;
            }
            matched += 1;
            if let Some(secs) = block_clock {
                clocks.push(secs);
            } else if shape == Timed {
                if let Some(range) = captured.as_ref().and_then(|c| c.name("range_clock")) {
                    let base = range.as_str().split('.').next().unwrap_or_default();
                    if let Some(secs) = parse_clock_seconds(base) {
                        clocks.push(secs);
                        range_turns += 1;
                    }
                } else if let Some(clock) = captured.as_ref().and_then(|c| c.name("clock")) {
                    single_clock_turns += 1;
                    let (round_5, round_15_wall) = schedule_clock_flags(clock.as_str());
                    round_5_clocks += usize::from(round_5);
                    round_15_wall_clocks += usize::from(round_15_wall);
                }
            }
            turn_has_sentence = false;
            if let Some(body) = captured.as_ref().and_then(|c| c.name("body")) {
                if header.summary_words != 0 {
                    turn_words += body.as_str().split_whitespace().count();
                }
                if shape == Bare && is_numeric_value(body.as_str()) {
                    numeric_bodies += 1;
                }
                turn_has_sentence = has_sentence(body.as_str());
            }
            if let Some(speaker) = speaker {
                if shape == Bare && all_language_labels {
                    all_language_labels = is_language_label(speaker);
                }
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
            let existing_heading = matches!(shape, Block | RoleBlock) && line.starts_with("# ");
            let short_heading =
                matches!(shape, Bare | Timed | Block | RoleLine | RoleBlock) && short_title(line);
            if !has_title && (existing_heading || short_heading) {
                has_title = true;
                allow_underline = short_heading && markdown_title(line).is_none();
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
            if header.summary_words != 0 && matches!(shape, Block | RoleBlock) {
                turn_words += line.split_whitespace().count();
            }
            if let Some(speaker) = pending_body.take() {
                utterances.insert((speaker, line));
            }
            if matches!(shape, Bare | Timed | Block) && !line.starts_with('#') {
                turn_has_sentence |= has_sentence(line);
            }
            if matches!(shape, Block | ExportBlock | RoleBlock) && !line.starts_with('#') {
                block_has_body = true;
                continuations += 1;
            }
        }
    }
    if in_turn && turn_has_sentence {
        sentence_turns += 1;
    }
    let distinct: HashSet<_> = speakers.iter().collect();
    if matches!(shape, Bare | RoleLine)
        && let Some(speaker) = header
            .opening_paragraph_speaker(lines)
            .map(str::to_lowercase)
        && !speakers
            .iter()
            .skip(1)
            .any(|candidate| candidate == &speaker)
    {
        return false;
    }
    if (header.summary_words != 0 && turn_words <= header.summary_words)
        || (matches!(shape, Bare | RoleLine)
            && !interview
            && distinct.iter().all(|speaker| {
                matches!(
                    speaker.as_str(),
                    "front" | "back" | "q" | "a" | "question" | "answer"
                )
            }))
    {
        return false;
    }
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
        ExportTimed => {
            distinct.len() >= 2
                && matched >= 2
                && unmatched <= matched
                && !distinct.iter().all(|speaker| reserved_label(speaker))
        }
        Block => {
            distinct.len() >= 2
                && matched >= 3
                && interleaves
                && block_has_body
                && continuations <= matched * 4
                && is_recording_offset_clocks(&clocks)
                && sentence_turns * 2 >= matched
                && sentence_turns >= 2
        }
        Timed => {
            let range_ok = range_turns == 0
                || (range_turns == matched
                    && is_recording_offset_clocks(&clocks)
                    && sentence_turns * 2 >= matched
                    && sentence_turns >= 2);
            let schedule_like = single_clock_turns > 0
                && single_clock_turns == matched
                && (round_15_wall_clocks == matched
                    || round_5_clocks == matched && sentence_turns == 0);
            matched >= 3
                && distinct.len() >= 2
                && interleaves
                && unmatched <= matched
                && range_ok
                && !schedule_like
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
            if (!interview && all_language_labels)
                || numeric_bodies * 5 >= matched * 4
                || sentence_turns < 2
                || sentence_turns * 3 < matched
            {
                return false;
            }
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

pub(super) fn parse_clock_seconds(text: &str) -> Option<u64> {
    let parse_part = |part: &str, min_len: usize, max_len: usize| -> Option<u64> {
        ((min_len..=max_len).contains(&part.len()) && part.bytes().all(|b| b.is_ascii_digit()))
            .then(|| part.parse::<u64>().ok())
            .flatten()
    };
    let mut parts = text.split(':');
    let first = parse_part(parts.next()?, 1, 3)?;
    let second = parse_part(parts.next()?, 2, 2)?;
    match (parts.next(), parts.next()) {
        (None, None) if second < 60 => first.checked_mul(60)?.checked_add(second),
        (Some(third), None) => {
            let third = parse_part(third, 2, 2)?;
            (second < 60 && third < 60)
                .then(|| {
                    first
                        .checked_mul(3600)?
                        .checked_add(second.checked_mul(60)?)?
                        .checked_add(third)
                })
                .flatten()
        }
        _ => None,
    }
}

pub(super) fn is_recording_offset_clocks(clocks: &[u64]) -> bool {
    clocks.len() >= 2
        && clocks[0] < 60
        && clocks.windows(2).all(|pair| pair[1] >= pair[0])
        && clocks.last() > clocks.first()
        && clocks.iter().any(|&secs| secs % 5 != 0)
}

pub(super) fn schedule_clock_flags(text: &str) -> (bool, bool) {
    let stripped = text
        .trim()
        .trim_matches(|c: char| !c.is_ascii_digit() && c != ':' && c != '.')
        .split('.')
        .next()
        .unwrap_or_default();
    let parts: Vec<_> = stripped.split(':').collect();
    match parts.as_slice() {
        [first, second] => match (first.parse::<u64>(), second.parse::<u64>()) {
            (Ok(h), Ok(m)) if m < 60 => (m % 5 == 0, h >= 1 && m % 15 == 0),
            _ => (false, false),
        },
        [first, second, third] => {
            match (
                first.parse::<u64>(),
                second.parse::<u64>(),
                third.parse::<u64>(),
            ) {
                (Ok(h), Ok(m), Ok(s)) if m < 60 && s < 60 => {
                    (s == 0 && m % 5 == 0, h >= 1 && s == 0 && m % 15 == 0)
                }
                _ => (false, false),
            }
        }
        _ => (false, false),
    }
}

pub(super) fn has_sentence(line: &str) -> bool {
    let trimmed = line
        .trim()
        .trim_end_matches(['"', '\'', '”', '’', ')', ']']);
    if let Some(last) = trimmed.chars().last()
        && matches!(last, '.' | '?' | '!' | '…' | '。' | '？' | '！')
        && trimmed
            .chars()
            .rev()
            .nth(1)
            .is_some_and(char::is_alphanumeric)
        && trimmed.chars().any(char::is_alphabetic)
    {
        return true;
    }
    let words: Vec<_> = trimmed.split_whitespace().collect();
    words.windows(2).any(|pair| {
        let word = pair[0].trim_end_matches(['"', '\'', '”', '’', ')', ']']);
        let Some(punct) = word.chars().last() else {
            return false;
        };
        let stem = &word[..word.len() - punct.len_utf8()];
        let next_starts_upper = pair[1]
            .trim_start_matches(['"', '\'', '“', '‘', '('])
            .chars()
            .next()
            .is_some_and(char::is_uppercase);
        match punct {
            '?' | '!' | '…' | '？' | '！' => stem.chars().any(char::is_alphabetic),
            '.' | '。' => {
                stem.chars().count() >= 3
                    && stem.chars().all(char::is_alphabetic)
                    && next_starts_upper
            }
            _ => false,
        }
    })
}

fn is_numeric_value(body: &str) -> bool {
    let body = body.trim();
    if body.is_empty() {
        return false;
    }
    let mut parts = body.split_whitespace();
    let Some(first) = parts.next() else {
        return false;
    };
    let rest: Vec<_> = parts.collect();
    if rest.len() > 1 || !rest.first().is_none_or(|unit| is_measurement_unit(unit)) {
        return false;
    }
    let core = first
        .trim_start_matches(['$', '€', '£', '¥', '+', '-', '~', '±'])
        .trim_end_matches(['%', '°']);
    let core = if rest.is_empty() {
        let idx = core.find(|c: char| c.is_alphabetic()).unwrap_or(core.len());
        let (num, suffix) = core.split_at(idx);
        if !suffix.is_empty() && !is_measurement_unit(suffix) {
            return false;
        }
        num.trim_end_matches(['%', '°'])
    } else {
        core
    };
    core.bytes().any(|b| b.is_ascii_digit())
        && core
            .bytes()
            .all(|b| b.is_ascii_digit() || matches!(b, b'.' | b',' | b'/' | b':' | b'-' | b'+'))
}

fn is_measurement_unit(unit: &str) -> bool {
    matches!(
        unit.trim_end_matches('.').to_ascii_lowercase().as_str(),
        "%" | "usd"
            | "eur"
            | "gbp"
            | "jpy"
            | "cad"
            | "aud"
            | "chf"
            | "btc"
            | "eth"
            | "kg"
            | "g"
            | "mg"
            | "lb"
            | "lbs"
            | "oz"
            | "km"
            | "m"
            | "cm"
            | "mm"
            | "mi"
            | "ft"
            | "in"
            | "l"
            | "ml"
            | "ms"
            | "s"
            | "sec"
            | "min"
            | "hr"
            | "hrs"
            | "kb"
            | "mb"
            | "gb"
            | "tb"
            | "hz"
            | "khz"
            | "mhz"
            | "ghz"
            | "c"
            | "f"
            | "°c"
            | "°f"
            | "bpm"
            | "kcal"
            | "cal"
            | "pts"
            | "px"
    )
}

fn is_language_label(label: &str) -> bool {
    let label = label.trim();
    let lower = label.to_ascii_lowercase();
    if matches!(
        lower.as_str(),
        "english"
            | "french"
            | "spanish"
            | "german"
            | "italian"
            | "portuguese"
            | "russian"
            | "japanese"
            | "chinese"
            | "korean"
            | "arabic"
            | "dutch"
            | "swedish"
            | "polish"
            | "turkish"
            | "hindi"
            | "ukrainian"
            | "vietnamese"
            | "hebrew"
            | "greek"
            | "latin"
            | "finnish"
            | "danish"
            | "norwegian"
            | "czech"
            | "original"
            | "translation"
            | "translated"
            | "source"
            | "target"
    ) {
        return true;
    }
    // Two-letter ISO 639-1 language codes (EN/FR, en/fr) or locale tags (en-US/fr-FR).
    // Bare two-letter codes require uniform case so Title-Case names (Vi, An) stay speakers.
    let (code, region) = match label.split_once(['-', '_']) {
        Some((c, r)) => (c, Some(r)),
        None => (label, None),
    };
    let valid_case = if let Some(r) = region {
        (r.len() == 2 || r.len() == 4) && r.bytes().all(|b| b.is_ascii_alphabetic())
    } else {
        code.bytes().all(|b| b.is_ascii_uppercase()) || code.bytes().all(|b| b.is_ascii_lowercase())
    };
    valid_case
        && matches!(
            code.to_ascii_lowercase().as_str(),
            "en" | "fr"
                | "de"
                | "es"
                | "it"
                | "pt"
                | "ru"
                | "ja"
                | "zh"
                | "ko"
                | "ar"
                | "nl"
                | "sv"
                | "pl"
                | "tr"
                | "hi"
                | "uk"
                | "vi"
                | "he"
                | "el"
                | "la"
                | "fi"
                | "da"
                | "no"
                | "cs"
                | "ro"
                | "hu"
                | "th"
                | "id"
                | "ca"
        )
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
        && colon_title_ok(line)
        && !line.ends_with(['.', '!', '?'])
}

fn colon_title_ok(line: &str) -> bool {
    let Some((prefix, suffix)) = line.split_once(':') else {
        return true;
    };
    if !suffix.starts_with(' ') || suffix.contains(':') || suffix.trim().is_empty() {
        return false;
    }
    let prefix = prefix.trim();
    let mut words = prefix.split_whitespace();
    let Some(first) = words.next() else {
        return false;
    };
    let count = 1 + words.count();
    let lower_first = first.to_ascii_lowercase();
    let lower_prefix = prefix.to_ascii_lowercase();
    (count >= 2
        || matches!(
            lower_first.as_str(),
            "interview" | "episode" | "podcast" | "recording" | "transcript" | "call" | "session"
        ))
        && !matches!(
            lower_first.as_str(),
            "note"
                | "notes"
                | "my"
                | "our"
                | "reminder"
                | "memo"
                | "summary"
                | "action"
                | "actions"
                | "todo"
                | "context"
                | "background"
                | "excerpt"
                | "quote"
                | "clip"
                | "draft"
                | "pasted"
                | "copied"
                | "saved"
                | "regarding"
                | "re"
                | "fwd"
                | "warning"
                | "tip"
                | "update"
                | "agenda"
                | "attendees"
                | "participants"
                | "decisions"
                | "status"
                | "owner"
                | "date"
                | "from"
                | "to"
                | "cc"
                | "sent"
                | "subject"
        )
        && !prefix.split_whitespace().any(|word| {
            matches!(
                word.trim_matches(|c: char| !c.is_alphanumeric())
                    .to_ascii_lowercase()
                    .as_str(),
                "note"
                    | "notes"
                    | "summary"
                    | "pasted"
                    | "excerpt"
                    | "plan"
                    | "plans"
                    | "schedule"
                    | "itinerary"
                    | "agenda"
                    | "roster"
                    | "timetable"
                    | "shifts"
                    | "changelog"
                    | "log"
                    | "logs"
                    | "checklist"
                    | "tasks"
                    | "todo"
                    | "todos"
            )
        })
        && !reserved_label(&lower_prefix)
        && !matches!(
            lower_prefix.as_str(),
            "action items" | "key takeaways" | "follow up" | "follow-up" | "next steps"
        )
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
            | "added"
            | "changed"
            | "deprecated"
            | "removed"
            | "fixed"
            | "security"
            | "updated"
            | "improved"
            | "refactored"
            | "reverted"
            | "resolved"
            | "bugfix"
            | "hotfix"
            | "breaking"
            | "chore"
            | "opened"
            | "closed"
            | "merged"
            | "deployed"
            | "released"
            | "started"
            | "stopped"
            | "passed"
            | "failed"
            | "pass"
            | "fail"
            | "blocked"
            | "pending"
            | "done"
            | "completed"
            | "skipped"
            | "retried"
            | "alert"
            | "notice"
            | "critical"
            | "fatal"
            | "success"
            | "home"
            | "away"
            | "before"
            | "after"
            | "input"
            | "output"
            | "expected"
            | "actual"
            | "buy"
            | "sell"
            | "debit"
            | "credit"
            | "income"
            | "expense"
            | "win"
            | "loss"
            | "problem"
            | "solution"
            | "cause"
            | "fix"
            | "issue"
            | "resolution"
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
        clock.len() == 5
            && clock.as_bytes()[2] == b':'
            && ["-!- ", "* "]
                .iter()
                .any(|prefix| body.trim_start().starts_with(prefix))
    });
    let tab_event = line.split_once('\t').is_some_and(|(stamp, body)| {
        stamp.len() >= 19
            && stamp.as_bytes()[10] == b' '
            && body.split_once('\t').is_some_and(|(nick, _)| {
                matches!(
                    nick.trim(),
                    "-->" | "<--" | "--" | "*" | "◥" | "◣" | "→" | "←"
                )
            })
    });
    irc_event
        || tab_event
        || line.starts_with("--- Log opened ")
        || line.starts_with("--- Log closed ")
        || line.starts_with("--- Day changed ")
        || line.starts_with("Transcribed by https://otter.ai")
        || line.starts_with("|")
            && line
                .trim_matches(|c: char| c == '|' || c.is_whitespace())
                .is_empty()
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
    static EVENT: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(concat!(
            r"^[A-Z][a-z]{2} [0-9]{1,2}, [0-9]{4}\s+[0-9]{1,2}:[0-9]{2}:[0-9]{2} [AP]M\s+",
            r".+\s+(?:named the conversation\s+\S|removed the name from the conversation|changed the group photo|removed the group photo|left the conversation|(?:added|removed)\s+.+\s+(?:to|from) the conversation)"
        ))
        .unwrap()
    });
    let lines: Vec<_> = nonblank(lines).collect();
    // Allow leading imessage-exporter conversation events, but require the first
    // non-event line to be a timestamped message header (no prose preamble).
    if !lines
        .iter()
        .find(|line| !EVENT.is_match(line))
        .is_some_and(|line| HEADER.is_match(line))
    {
        return false;
    }
    lines
        .windows(3)
        .filter(|block| {
            HEADER.is_match(block[0])
                && name_label(block[1])
                && !HEADER.is_match(block[2])
                && !EVENT.is_match(block[2])
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
