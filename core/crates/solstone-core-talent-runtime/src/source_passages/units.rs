// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Bounded source units from an activity's rendered transcript.
//!
//! Each segment's capture channels become their own units, normalized for
//! reading, with the activity type, segment heading and channel name carried at
//! the top of the unit's text. A channel longer than the unit ceiling is split
//! into overlapping chunks, so no source character falls between two units.

use std::sync::LazyLock;

use regex::Regex;

use super::json_values;

/// The most characters one unit may hold, its heading lines included.
pub const UNIT_MAX_CHARS: usize = 16_000;
/// Characters each chunk repeats from the end of the one before it.
pub const UNIT_OVERLAP_CHARS: usize = 256;

static SEGMENT: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?m)^## (?P<time>\d{4}-\d{2}-\d{2} \d{2}:\d{2}:\d{2} - \d{2}:\d{2}:\d{2})[ \t]*$")
        .expect("segment heading pattern")
});
static CHANNEL: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?m)^### (?P<kind>Transcript|Imported Transcript|Imported Conversation|Imported Messages|Screen Activity|Browser Content|ChatGPT Conversation|Claude Conversation|Gemini Conversation|Calendar Event|Note|Document|Screenshot|Photo|Highlights|Body Records|Imported Text)[ \t]*$",
    )
    .expect("channel heading pattern")
});

/// What kind of capture a unit's text came from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SourceKind {
    Audio,
    Screen,
    Browser,
    Import,
    /// Text before the first segment heading.
    Context,
    /// A segment with no recognized channel heading.
    Unknown,
}

impl SourceKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Audio => "audio",
            Self::Screen => "screen",
            Self::Browser => "browser",
            Self::Import => "import",
            Self::Context => "context",
            Self::Unknown => "unknown",
        }
    }

    fn of_channel(channel: &str) -> Self {
        match channel {
            "Transcript" => Self::Audio,
            "Screen Activity" => Self::Screen,
            "Browser Content" => Self::Browser,
            "context" => Self::Context,
            "unknown" => Self::Unknown,
            _ => Self::Import,
        }
    }
}

/// One normalized source unit: the exact text passages are copied from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SourceUnit {
    pub text: String,
    pub kind: SourceKind,
    /// The segment heading's time range, when the unit sits in a segment.
    pub timestamp: Option<String>,
    /// The transcript page marked this segment untimed. A clock inside the text
    /// is then the native projection's synthetic start, not a captured time.
    /// `source_units` leaves this false; the caller that read the page sets it.
    pub untimed: bool,
    /// 1-based segment ordinal in the transcript; 0 before the first segment.
    pub segment: usize,
    /// 1-based channel ordinal in its segment; 0 when it has none.
    pub channel: usize,
    /// 1-based chunk ordinal in its channel.
    pub chunk: usize,
}

struct Region<'a> {
    text: &'a str,
    timestamp: &'a str,
    channel: &'a str,
    segment: usize,
    channel_number: usize,
}

/// Every source unit of a rendered transcript, in transcript order.
pub fn source_units(transcript: &str, activity_kind: &str) -> Vec<SourceUnit> {
    let segments = SEGMENT.captures_iter(transcript).collect::<Vec<_>>();
    let mut regions = Vec::new();
    let mut leading = vec![None; segments.len() + 1];
    let first = segments
        .first()
        .map_or(transcript.len(), |segment| segment.get(0).unwrap().start());
    if first > 0 {
        push_leading_channels(&mut regions, &transcript[..first]);
    }
    for (index, segment) in segments.iter().enumerate() {
        let heading = segment.get(0).unwrap();
        let time = segment.name("time").unwrap().as_str();
        let end = segments
            .get(index + 1)
            .map_or(transcript.len(), |next| next.get(0).unwrap().start());
        let body = &transcript[heading.end()..end];
        let channels = CHANNEL.captures_iter(body).collect::<Vec<_>>();
        if channels.is_empty() {
            regions.push(Region {
                text: &transcript[heading.start()..end],
                timestamp: time,
                channel: "unknown",
                segment: index + 1,
                channel_number: 0,
            });
            continue;
        }
        let channel_start =
            |position: usize| heading.end() + channels[position].get(0).unwrap().start();
        // The segment heading and anything before its first channel head every unit in it.
        leading[index + 1] = Some(&transcript[heading.start()..channel_start(0)]);
        for (number, channel) in channels.iter().enumerate() {
            let stop = if number + 1 < channels.len() {
                channel_start(number + 1)
            } else {
                end
            };
            regions.push(Region {
                text: &transcript[channel_start(number)..stop],
                timestamp: time,
                channel: channel.name("kind").unwrap().as_str(),
                segment: index + 1,
                channel_number: number + 1,
            });
        }
    }
    let mut units = Vec::new();
    for region in regions {
        if region.text.trim().is_empty() {
            continue;
        }
        let rendered = json_values::normalize(region.text);
        let mut prefix = format!("Activity type: {activity_kind}\n");
        if let Some(Some(heading)) = leading.get(region.segment) {
            prefix.push_str(heading);
        } else if !region.timestamp.is_empty() {
            prefix.push_str(&format!("Segment: {}\n", region.timestamp));
        }
        prefix.push_str(&format!("Source channel: {}\n", region.channel));
        let payload_limit = UNIT_MAX_CHARS.saturating_sub(prefix.chars().count());
        if payload_limit <= UNIT_OVERLAP_CHARS {
            // A heading this long leaves no room to make progress; keep the
            // channel whole rather than drop it.
            units.push(unit(&region, prefix + &rendered, 1));
            continue;
        }
        let offsets = rendered
            .char_indices()
            .map(|(offset, _)| offset)
            .chain([rendered.len()])
            .collect::<Vec<_>>();
        let length = offsets.len() - 1;
        let mut point = 0;
        let mut chunk = 0;
        while point < length {
            let stop = length.min(point + payload_limit);
            chunk += 1;
            units.push(unit(
                &region,
                format!("{prefix}{}", &rendered[offsets[point]..offsets[stop]]),
                chunk,
            ));
            if stop == length {
                break;
            }
            point = stop - UNIT_OVERLAP_CHARS;
        }
    }
    units
}

/// Text before the first clock heading. A channel heading here keeps its
/// family and has no timestamp, so an untimed import is not speech and not a
/// clock range.
fn push_leading_channels<'a>(regions: &mut Vec<Region<'a>>, text: &'a str) {
    let channels = CHANNEL.captures_iter(text).collect::<Vec<_>>();
    if channels.is_empty() {
        regions.push(Region {
            text,
            timestamp: "",
            channel: "context",
            segment: 0,
            channel_number: 0,
        });
        return;
    }
    let first_start = channels[0].get(0).unwrap().start();
    if first_start > 0 {
        regions.push(Region {
            text: &text[..first_start],
            timestamp: "",
            channel: "context",
            segment: 0,
            channel_number: 0,
        });
    }
    for (number, channel) in channels.iter().enumerate() {
        let start = channel.get(0).unwrap().start();
        let stop = channels
            .get(number + 1)
            .map_or(text.len(), |next| next.get(0).unwrap().start());
        regions.push(Region {
            text: &text[start..stop],
            timestamp: "",
            channel: channel.name("kind").unwrap().as_str(),
            segment: 0,
            channel_number: number + 1,
        });
    }
}

fn unit(region: &Region, text: String, chunk: usize) -> SourceUnit {
    SourceUnit {
        text,
        kind: SourceKind::of_channel(region.channel),
        timestamp: (!region.timestamp.is_empty()).then(|| region.timestamp.to_owned()),
        untimed: false,
        segment: region.segment,
        channel: region.channel_number,
        chunk,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_channel_is_a_unit_headed_by_its_activity_segment_and_channel() {
        let transcript = concat!(
            "## 2026-01-01 09:00:00 - 09:05:00\n\n",
            "### Transcript\n[09:00:01] Ari: Could we ship v14 after review?\n\n",
            "### Screen Activity\nEditor shows retry.rs\n\n",
            "## 2026-01-01 09:05:00 - 09:10:00\n\n",
            "### Transcript\n[09:05:02] Bea: Tests passed.\n",
        );
        let units = source_units(transcript, "meeting");
        assert_eq!(units.len(), 3);
        assert_eq!(
            units[0].text,
            concat!(
                "Activity type: meeting\n",
                "## 2026-01-01 09:00:00 - 09:05:00\n\n",
                "Source channel: Transcript\n",
                "### Transcript\n[09:00:01] Ari: Could we ship v14 after review?\n\n",
            )
        );
        assert_eq!(units[0].kind, SourceKind::Audio);
        assert_eq!(units[1].kind, SourceKind::Screen);
        assert_eq!(
            units[2].timestamp.as_deref(),
            Some("2026-01-01 09:05:00 - 09:10:00")
        );
        assert_eq!(
            (units[2].segment, units[2].channel, units[2].chunk),
            (2, 1, 1)
        );
    }

    #[test]
    fn a_long_channel_splits_into_overlapping_chunks_that_cover_every_character() {
        let body = "Ωmega line of captured words.\n".repeat(1200);
        let transcript = format!("## 2026-01-01 09:00:00 - 09:05:00\n\n### Transcript\n{body}");
        let units = source_units(&transcript, "meeting");
        assert!(units.len() > 1);
        let prefix_end = |unit: &SourceUnit| unit.text.find("### Transcript").unwrap_or(0);
        let mut covered = String::new();
        for (index, unit) in units.iter().enumerate() {
            assert!(unit.text.chars().count() <= UNIT_MAX_CHARS);
            assert_eq!(unit.chunk, index + 1);
            let payload = if index == 0 {
                &unit.text[prefix_end(unit)..]
            } else {
                let start = unit.text.find("Source channel: Transcript\n").unwrap()
                    + "Source channel: Transcript\n".len();
                let payload = &unit.text[start..];
                let overlap = payload
                    .char_indices()
                    .nth(UNIT_OVERLAP_CHARS)
                    .map_or(payload.len(), |(offset, _)| offset);
                assert!(covered.ends_with(&payload[..overlap]));
                &payload[overlap..]
            };
            covered.push_str(payload);
        }
        assert_eq!(covered, format!("### Transcript\n{body}"));
    }

    #[test]
    fn whitespace_regions_make_no_unit_and_text_before_a_segment_is_context() {
        let units = source_units(
            "Opening note\n## 2026-01-01 09:00:00 - 09:05:00\n\n",
            "coding",
        );
        assert_eq!(units.len(), 2);
        assert_eq!(units[0].kind, SourceKind::Context);
        assert_eq!(
            units[0].text,
            "Activity type: coding\nSource channel: context\nOpening note\n"
        );
        assert_eq!(units[1].kind, SourceKind::Unknown);
    }

    #[test]
    fn an_untimed_import_channel_is_not_audio_and_has_no_timestamp() {
        let units = source_units(
            "### Imported Text\n\nnotes from the import\n\n### ChatGPT Conversation\n\nHello\n",
            "import",
        );
        assert_eq!(units.len(), 2);
        assert_eq!(units[0].kind, SourceKind::Import);
        assert_eq!(units[1].kind, SourceKind::Import);
        assert!(units[0].timestamp.is_none());
        assert!(units[1].timestamp.is_none());
        assert!(!units[0].untimed);
        assert!(!units[1].untimed);
        assert!(
            units[1]
                .text
                .contains("Source channel: ChatGPT Conversation")
        );
    }

    #[test]
    fn a_segment_with_two_imported_kinds_splits_into_two_import_units() {
        let transcript = concat!(
            "## 2026-04-07 09:00:00 - 09:05:00\n\n",
            "### Note\n\nfield note text\n\n",
            "### Document\n\nmanual text\n\n",
            "### Imported Transcript\n\nspoken import\n\n",
            "### Screenshot\n\nreceipt text\n\n",
            "### Photo\n\ngarden bench\n\n",
            "### Body Records\n\nworkout running\n",
        );
        let units = source_units(transcript, "import");
        assert_eq!(units.len(), 6);
        for (idx, expected) in [
            "Note",
            "Document",
            "Imported Transcript",
            "Screenshot",
            "Photo",
            "Body Records",
        ]
        .into_iter()
        .enumerate()
        {
            assert_eq!(units[idx].kind, SourceKind::Import, "{expected}");
            assert_eq!(units[idx].channel, idx + 1, "{expected}");
            assert!(
                units[idx]
                    .text
                    .contains(&format!("Source channel: {expected}\n")),
                "{}",
                units[idx].text
            );
        }
    }
}
