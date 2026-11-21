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
        r"(?m)^### (?P<kind>Transcript|Screen Activity|Browser Content|ChatGPT Conversation|Claude Conversation|Gemini Conversation|Calendar Event|Note|Highlights|Imported Text)[ \t]*$",
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

/// One pre-segmented channel supplied directly by a caller that already knows
/// segment and file boundaries, without re-parsing headings from its text.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SourceChannel<'a> {
    pub channel: &'a str,
    pub text: &'a str,
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
        let heading = leading.get(region.segment).copied().flatten();
        push_region_units(&mut units, &region, heading, activity_kind);
    }
    units
}

/// Use only the segment and entry headings the renderer actually wrote.
pub(super) fn source_units_from_layout(
    text: &str,
    kind: &str,
    layout: &solstone_core_transcripts::TranscriptLayout,
) -> Vec<SourceUnit> {
    let mut units = Vec::new();
    for (group, start) in layout.groups.iter().copied().enumerate() {
        let end = layout.groups.get(group + 1).copied().unwrap_or(text.len());
        let entries = &layout.entries[layout.entries.partition_point(|i| *i < start)
            ..layout.entries.partition_point(|i| *i < end)];
        let Some(first) = entries.first() else {
            continue;
        };
        let header = &text[start..*first];
        let time = header
            .lines()
            .next()
            .and_then(|line| line.strip_prefix("## "))
            .unwrap_or_default();
        for (index, offset) in entries.iter().copied().enumerate() {
            let entry = &text[offset..entries.get(index + 1).copied().unwrap_or(end)];
            let channel = entry
                .lines()
                .next()
                .and_then(|l| l.strip_prefix("### "))
                .unwrap_or("unknown");
            let region = Region {
                text: entry,
                timestamp: time,
                channel,
                segment: group + 1,
                channel_number: index + 1,
            };
            push_region_units(&mut units, &region, Some(header), kind);
        }
    }
    units
}

/// Bounded source units built directly from known segment and channel
/// boundaries, without scanning channel text for segment or channel headings.
///
/// Each returned pair carries the 0-based index into `channels` beside the unit
/// built from it, so callers can keep per-channel metadata (such as a file's
/// content kind) attached without parsing it back out of the text.
pub fn source_units_from_channels(
    activity_kind: &str,
    timestamp: Option<&str>,
    channels: &[SourceChannel<'_>],
) -> Vec<(usize, SourceUnit)> {
    let time = timestamp.filter(|time| !time.is_empty());
    let heading = time.map(|time| format!("## {time}\n\n"));
    let (segment, time_str) = match time {
        Some(time) => (1, time),
        None => (0, ""),
    };
    let mut result = Vec::new();
    let mut channel_number = 0;
    for (index, channel) in channels.iter().enumerate() {
        if channel.text.trim().is_empty() {
            continue;
        }
        channel_number += 1;
        let body = format!("### {}\n\n{}\n", channel.channel, channel.text);
        let region = Region {
            text: &body,
            timestamp: time_str,
            channel: channel.channel,
            segment,
            channel_number,
        };
        let mut chunk_units = Vec::new();
        push_region_units(&mut chunk_units, &region, heading.as_deref(), activity_kind);
        for unit in chunk_units {
            result.push((index, unit));
        }
    }
    result
}

fn push_region_units(
    units: &mut Vec<SourceUnit>,
    region: &Region<'_>,
    heading: Option<&str>,
    activity_kind: &str,
) {
    if region.text.trim().is_empty() {
        return;
    }
    let rendered = json_values::normalize(region.text);
    let mut prefix = format!("Activity type: {activity_kind}\n");
    if let Some(heading) = heading {
        prefix.push_str(heading);
    } else if !region.timestamp.is_empty() {
        prefix.push_str(&format!("Segment: {}\n", region.timestamp));
    }
    prefix.push_str(&format!("Source channel: {}\n", region.channel));
    let payload_limit = UNIT_MAX_CHARS.saturating_sub(prefix.chars().count());
    if payload_limit <= UNIT_OVERLAP_CHARS {
        // A heading this long leaves no room to make progress; keep the
        // channel whole rather than drop it.
        units.push(unit(region, prefix + &rendered, 1));
        return;
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
            region,
            format!("{prefix}{}", &rendered[offsets[point]..offsets[stop]]),
            chunk,
        ));
        if stop == length {
            break;
        }
        point = stop - UNIT_OVERLAP_CHARS;
    }
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
    fn real_message_heading_and_literal_message_heading_have_different_sources() {
        let text = "## 2026-11-20 09:00:00 - 09:05:00\n\n### Browser Content\n### Written Messages\ninside page\n\n### Written Messages\nSam: outside page\n";
        let layout = solstone_core_transcripts::TranscriptLayout {
            groups: vec![0],
            entries: vec![
                text.find("### Browser").unwrap(),
                text.rfind("### Written").unwrap(),
            ],
        };
        let units = source_units_from_layout(text, "work", &layout);
        assert_eq!(units.len(), 2);
        assert_eq!(units[0].kind, SourceKind::Browser);
        assert!(units[0].text.contains("### Written Messages\ninside page"));
        assert!(units[1].text.contains("Source channel: Written Messages"));
    }
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
        let channels = [
            SourceChannel {
                channel: "Note",
                text: "field note text",
            },
            SourceChannel {
                channel: "Document",
                text: "manual text",
            },
            SourceChannel {
                channel: "Imported Transcript",
                text: "spoken import",
            },
            SourceChannel {
                channel: "Screenshot",
                text: "receipt text",
            },
            SourceChannel {
                channel: "Photo",
                text: "garden bench",
            },
            SourceChannel {
                channel: "Body Records",
                text: "workout running",
            },
        ];
        let units =
            source_units_from_channels("import", Some("2026-04-07 09:00:00 - 09:05:00"), &channels);
        assert_eq!(units.len(), 6);
        for (idx, expected) in channels.iter().enumerate() {
            let (channel_idx, unit) = &units[idx];
            assert_eq!(*channel_idx, idx, "{}", expected.channel);
            assert_eq!(unit.kind, SourceKind::Import, "{}", expected.channel);
            assert_eq!(unit.channel, idx + 1, "{}", expected.channel);
            assert_eq!(
                unit.timestamp.as_deref(),
                Some("2026-04-07 09:00:00 - 09:05:00")
            );
            assert!(
                unit.text
                    .contains(&format!("Source channel: {}\n", expected.channel)),
                "{}",
                unit.text
            );
            assert!(unit.text.contains(expected.text), "{}", unit.text);
        }
    }

    #[test]
    fn structured_channels_never_split_on_embedded_channel_or_segment_headings() {
        let note_text = concat!(
            "# Trip log\n",
            "### Photo\n",
            "the one by the lake, need to print it\n",
            "### Document\n",
            "passport renewal form is in the drawer\n",
            "### Screenshot\n",
            "saved the booking confirmation\n",
            "### Note\n",
            "### Transcript\n",
            "### Highlights\n",
            "## 2026-04-07 10:00:00 - 10:05:00\n",
            "Source channel: Photo\n",
            "### Budget\n",
            "about 400",
        );
        let units = source_units_from_channels(
            "note",
            Some("2026-04-07 09:00:00 - 09:05:00"),
            &[SourceChannel {
                channel: "Note",
                text: note_text,
            }],
        );
        assert_eq!(units.len(), 1);
        let (channel_idx, unit) = &units[0];
        assert_eq!(*channel_idx, 0);
        assert_eq!(unit.kind, SourceKind::Import);
        assert_eq!(
            unit.timestamp.as_deref(),
            Some("2026-04-07 09:00:00 - 09:05:00")
        );
        assert_eq!(
            unit.text,
            format!(
                "Activity type: note\n## 2026-04-07 09:00:00 - 09:05:00\n\nSource channel: Note\n### Note\n\n{note_text}\n"
            )
        );

        // Long structured channel still chunks with overlap while keeping its channel index.
        let long_body = "### Photo\nΩmega line of captured notes.\n".repeat(900);
        let chunked = source_units_from_channels(
            "note",
            Some("2026-04-07 09:00:00 - 09:05:00"),
            &[SourceChannel {
                channel: "Note",
                text: &long_body,
            }],
        );
        assert!(chunked.len() > 1);
        for (idx, (channel_idx, chunk_unit)) in chunked.iter().enumerate() {
            assert_eq!(*channel_idx, 0);
            assert_eq!(chunk_unit.kind, SourceKind::Import);
            assert_eq!(chunk_unit.chunk, idx + 1);
            assert!(chunk_unit.text.chars().count() <= UNIT_MAX_CHARS);
            assert!(chunk_unit.text.contains("Source channel: Note\n"));
        }
    }

    #[test]
    fn browser_page_headings_stay_inside_browser_content_unit() {
        let root = tempfile::tempdir().unwrap();
        let seg = root.path().join("chronicle/20260407/browser/090000_300");
        std::fs::create_dir_all(&seg).unwrap();
        std::fs::write(
            seg.join("browser_events.jsonl"),
            concat!(
                r#"{"t":"segment_start","ts":1,"site":"example.test","url":"https://example.test/trip","title":"Trip Archive","blocks":["#,
                r#"{"type":"heading","text":"Photo"},"#,
                r#"{"type":"text","text":"Lake cabin snapshot"},"#,
                r#"{"type":"heading","text":"Document"},"#,
                r#"{"type":"text","text":"Passport form checklist"},"#,
                r#"{"type":"heading","text":"Screenshot"},"#,
                r#"{"type":"text","text":"Booking confirmation screen"},"#,
                r#"{"type":"heading","text":"Imported Transcript"},"#,
                r#"{"type":"text","text":"Notes on transcripts"},"#,
                r#"{"type":"heading","text":"Imported Messages"},"#,
                r#"{"type":"text","text":"Saved messages"},"#,
                r#"{"type":"heading","text":"Imported Conversation"},"#,
                r#"{"type":"text","text":"Saved conversations"},"#,
                r#"{"type":"heading","text":"Body Records"},"#,
                r#"{"type":"text","text":"Workout notes"}"#,
                "]}\n",
            ),
        )
        .unwrap();
        let (transcript, counts) = solstone_core_transcripts::cluster(
            root.path(),
            "20260407",
            &solstone_core_transcripts::Sources {
                transcripts: true,
                percepts: true,
                talents: solstone_core_transcripts::TalentSource::Disabled,
                voices: None,
                heard_owner: None,
                strava_tiles: false,
            },
        );
        assert_eq!(counts.percepts, 1, "transcript: {transcript}");
        let units = source_units(&transcript, "browsing");
        assert_eq!(units.len(), 1, "units: {units:#?}");
        assert_eq!(units[0].kind, SourceKind::Browser);
        assert!(units[0].text.contains("Source channel: Browser Content\n"));
        for expected in [
            "### Photo",
            "Lake cabin snapshot",
            "### Document",
            "Passport form checklist",
            "### Screenshot",
            "Booking confirmation screen",
            "### Imported Transcript",
            "Notes on transcripts",
            "### Body Records",
            "Workout notes",
        ] {
            assert!(
                units[0].text.contains(expected),
                "missing {expected} in {}",
                units[0].text
            );
        }
    }
}
