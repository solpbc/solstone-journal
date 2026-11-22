// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use super::{FileFormat, IndexChunk, ProducedChunks};
use crate::calendar::{CalendarNote, OwnerAddresses};

/// Naming stays with the location domain, which already depends on this crate.
pub trait PlaceFormatter {
    fn render(&self, text: &str) -> ProducedChunks;
}

pub enum FormatterView<'a> {
    Stored,
    LiteralMarkdown,
    BodyLine,
    CalendarNote {
        note: &'a CalendarNote,
        owner: &'a OwnerAddresses,
    },
    LocalizedCalendarNote {
        note: &'a CalendarNote,
        owner: &'a OwnerAddresses,
        locale: &'a str,
    },
    Places(&'a dyn PlaceFormatter),
}

pub fn literal(text: String) -> ProducedChunks {
    ProducedChunks {
        chunks: vec![IndexChunk {
            content: text,
            occurrence_time_ms: None,
            source: None,
            group_offset_ms: None,
        }],
        agent_override: None,
        header: None,
        error: None,
        warnings: Vec::new(),
    }
}

/// All text formatting enters here. A view retains a reader's existing
/// projection; choosing a format never changes index or permission admission.
pub fn format_file(
    format: FileFormat,
    rel: &str,
    text: &str,
    view: FormatterView<'_>,
) -> ProducedChunks {
    use super::Family;
    match (format, view) {
        (FileFormat::Content(Family::Markdown), FormatterView::LiteralMarkdown) => {
            literal(text.to_owned())
        }
        (FileFormat::Content(Family::BodyRecords), FormatterView::BodyLine) => {
            literal(super::body_records::render_line(text))
        }
        (FileFormat::Content(Family::Calendar), FormatterView::CalendarNote { note, owner }) => {
            literal(crate::calendar::render_calendar_note(note, owner))
        }
        (
            FileFormat::Content(Family::Calendar),
            FormatterView::LocalizedCalendarNote {
                note,
                owner,
                locale,
            },
        ) => literal(crate::calendar::calendar_note_for_locale(
            note, owner, locale,
        )),
        (FileFormat::Place, FormatterView::Places(formatter)) => formatter.render(text),
        (FileFormat::Content(family), FormatterView::Stored) => {
            super::produce_content_chunks(family, rel, text)
        }
        (FileFormat::Raw(family), FormatterView::Stored) => {
            let records = super::parse_jsonl_objects(text);
            super::produce_raw_percept_chunks_by_shape(family, Some(rel), &records)
        }
        (FileFormat::PhotoDescription, FormatterView::Stored) => {
            let mut produced = literal(String::new());
            produced.chunks = super::parse_jsonl_objects(text)
                .into_iter()
                .filter_map(|record| {
                    let text = record.get("text")?.as_str()?;
                    let content = match record
                        .get("detections")
                        .and_then(crate::objects::objects_line)
                    {
                        Some(line) => format!("{text}\n\n{line}"),
                        None => text.to_owned(),
                    };
                    Some(IndexChunk {
                        content,
                        occurrence_time_ms: None,
                        source: Some(record),
                        group_offset_ms: None,
                    })
                })
                .collect();
            produced
        }
        _ => {
            let mut produced = literal(String::new());
            produced.chunks.clear();
            produced.error = Some("file format requires its reader's rendering context".to_owned());
            produced
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::content::Family;

    #[test]
    fn literal_and_body_views_preserve_their_existing_line_contracts() {
        let markdown = "  <script>raw</script>\r\n\n# heading\n";
        assert_eq!(
            format_file(
                FileFormat::Content(Family::Markdown),
                "note_transcript.md",
                markdown,
                FormatterView::LiteralMarkdown
            )
            .chunks[0]
                .content,
            markdown
        );
        for line in ["damaged body row", "{}", ""] {
            assert_eq!(
                format_file(
                    FileFormat::Content(Family::BodyRecords),
                    "body.jsonl",
                    line,
                    FormatterView::BodyLine
                )
                .chunks[0]
                    .content,
                super::super::body_records::render_line(line)
            );
        }
    }

    #[test]
    fn photo_description_is_its_own_formatter_without_speech_labels() {
        let text = "{\"kind\":\"image\",\"raw\":\"photo.jpg\"}\n{\"start\":\"00:00:00\",\"text\":\"A red bicycle.\"}\n";
        let produced = format_file(
            FileFormat::PhotoDescription,
            "20260107/camera/120000_1/photo.jsonl",
            text,
            FormatterView::Stored,
        );
        assert_eq!(produced.chunks.len(), 1);
        assert_eq!(produced.chunks[0].content, "A red bicycle.");
        assert_eq!(
            produced.chunks[0].source.as_ref().unwrap()["text"],
            "A red bicycle."
        );
        assert!(
            format_file(
                FileFormat::Place,
                "location.jsonl",
                text,
                FormatterView::Stored
            )
            .error
            .is_some()
        );
    }
}
