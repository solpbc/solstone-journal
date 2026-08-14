// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! One-time migration of legacy segment document/image extraction JSONL into
//! durable `document_transcript.md` transcripts.
//!
//! Delete-safety is the whole point of this module: a legacy JSONL is removed
//! only once a readable, non-empty markdown transcript is proven to exist. A
//! duplicate JSONL sitting beside a transcript that already carries a body is
//! removed because the transcript is the durable readable form. A transcript
//! that is header-only carries no body, so the JSONL beside it is the only copy
//! of the extracted text and the deletion is refused and reported instead.
//!
//! An orphan document JSONL is converted first and deleted second: the rendered
//! markdown is written, re-read, and checked non-empty before the source bytes
//! go away. Anything unparseable, or parseable but textless, is preserved and
//! reported — this migration never trades a readable artifact for a guess.
//!
//! Re-drain consequence: historical PDF days fingerprint differently once,
//! because raw PDF originals now contribute size markers. This migration must
//! not rebaseline stored catchup fingerprints; catchup owns that transition.

use std::error::Error;
use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};

use serde_json::{Map, Value};
use solstone_core_journal_io::{AtomicWriteOptions, remove_file, write_text};

/// The one legacy extraction suffix this migration recognizes as a raw PDF.
const PDF_EXTENSION: &str = ".pdf";

/// The transcript file this migration writes, and the suffix it looks for.
const DOCUMENT_TRANSCRIPT_NAME: &str = "document_transcript.md";
const TRANSCRIPT_SUFFIX: &str = "_transcript.md";

/// A document-extraction migration step that could not be completed.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PdfExtractionMigrationError(String);

impl PdfExtractionMigrationError {
    /// Describe a document-extraction migration failure.
    pub fn new(message: impl Into<String>) -> Self {
        Self(message.into())
    }
}

impl fmt::Display for PdfExtractionMigrationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl Error for PdfExtractionMigrationError {}

/// What one migration run scanned, removed, converted, and refused to touch.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct PdfExtractionMigrationReport {
    /// Legacy document/image JSONL files considered.
    pub scanned: u64,
    /// Duplicate JSONL files removed beside a transcript that has a body.
    pub deleted_duplicates: u64,
    /// Orphan document JSONL files converted into a transcript and removed.
    pub converted_documents: u64,
    /// JSONL files whose lines could not be parsed. Left on disk.
    pub skipped_unparseable: u64,
    /// Document JSONL files that parsed but carried no text. Left on disk.
    pub skipped_no_text: u64,
    /// Duplicate JSONL files beside a header-only transcript. Left on disk.
    pub skipped_empty_transcript: u64,
    /// Journal-relative paths of every file this run declined to remove.
    pub left_in_place: Vec<String>,
}

impl PdfExtractionMigrationReport {
    /// Render the operator-facing report of a completed run.
    pub fn report_lines(&self) -> Vec<String> {
        let mut lines = vec![
            format!("Scanned {} JSONL file(s)", self.scanned),
            format!(
                "Deleted {} duplicate extraction JSONL file(s)",
                self.deleted_duplicates
            ),
            format!(
                "Converted {} document JSONL file(s)",
                self.converted_documents
            ),
            format!(
                "Skipped {} unparseable JSONL file(s)",
                self.skipped_unparseable
            ),
            format!(
                "Skipped {} document JSONL file(s) with no text",
                self.skipped_no_text
            ),
            format!(
                "Skipped {} duplicate JSONL file(s) beside empty transcripts",
                self.skipped_empty_transcript
            ),
        ];
        lines.extend(
            self.left_in_place
                .iter()
                .map(|path| format!("Left in place: {path}")),
        );
        lines
    }
}

/// Migrate every legacy document/image extraction JSONL below `journal`.
///
/// Walks `chronicle/<day>/<stream>/<segment>/*.jsonl` in the same order the
/// retired Python migration did, so the reported paths line up run for run.
pub fn migrate_pdf_extractions(
    journal: &Path,
) -> Result<PdfExtractionMigrationReport, PdfExtractionMigrationError> {
    let mut report = PdfExtractionMigrationReport::default();
    for output in legacy_outputs(journal) {
        migrate_one(journal, &output, &mut report)?;
    }
    Ok(report)
}

/// One candidate legacy output, carried with the rel its removal needs.
struct LegacyOutput {
    path: PathBuf,
    directory: PathBuf,
    name: String,
    rel: String,
}

fn migrate_one(
    journal: &Path,
    output: &LegacyOutput,
    report: &mut PdfExtractionMigrationReport,
) -> Result<(), PdfExtractionMigrationError> {
    let Some(header) = read_header(&output.path) else {
        // A headerless file is only this migration's business when a raw PDF of
        // the same stem sits beside it; anything else is another producer's.
        if same_stem_pdf_exists(&output.directory, &output.name) {
            report.scanned += 1;
            report.skipped_unparseable += 1;
            report.left_in_place.push(output.rel.clone());
        }
        return Ok(());
    };

    let kind = header.get("kind").and_then(Value::as_str);
    if !matches!(kind, Some("document" | "image")) {
        return Ok(());
    }
    report.scanned += 1;

    if transcript_exists(&output.directory) {
        if !transcript_with_body_exists(&output.directory) {
            // The transcript beside this file is header-only, so the JSONL is
            // still the only readable copy of the extracted text.
            report.skipped_empty_transcript += 1;
            report.left_in_place.push(output.rel.clone());
            return Ok(());
        }
        remove_legacy_output(journal, &output.rel)?;
        report.deleted_duplicates += 1;
        return Ok(());
    }

    // An orphan image extraction has no durable markdown form to convert into.
    if kind != Some("document") {
        return Ok(());
    }

    let Some(text) = read_document_text(&output.path) else {
        report.skipped_unparseable += 1;
        report.left_in_place.push(output.rel.clone());
        return Ok(());
    };
    if text.is_empty() {
        report.skipped_no_text += 1;
        report.left_in_place.push(output.rel.clone());
        return Ok(());
    }

    let title = match header.get("raw").and_then(Value::as_str) {
        Some(raw) => path_stem(raw).to_owned(),
        None => file_stem(&output.name).to_owned(),
    };
    let transcript = output.directory.join(DOCUMENT_TRANSCRIPT_NAME);
    write_verified(
        &transcript,
        &render_document_markdown(&title, &text, &header),
    )?;
    remove_legacy_output(journal, &output.rel)?;
    report.converted_documents += 1;
    Ok(())
}

/// List `chronicle/*/*/*/*.jsonl` in journal-path order.
///
/// Each level is listed by name, which reproduces the component-wise ordering
/// of the retired `sorted(chronicle.glob(...))`. Hidden names are included, as
/// `pathlib` includes them. An unreadable directory contributes nothing.
fn legacy_outputs(journal: &Path) -> Vec<LegacyOutput> {
    let chronicle = journal.join("chronicle");
    let mut outputs = Vec::new();
    for (day, day_path) in sorted_children(&chronicle) {
        if !day_path.is_dir() {
            continue;
        }
        for (stream, stream_path) in sorted_children(&day_path) {
            if !stream_path.is_dir() {
                continue;
            }
            for (segment, segment_path) in sorted_children(&stream_path) {
                if !segment_path.is_dir() {
                    continue;
                }
                for (name, path) in sorted_children(&segment_path) {
                    if !name.ends_with(".jsonl") {
                        continue;
                    }
                    outputs.push(LegacyOutput {
                        path,
                        directory: segment_path.clone(),
                        rel: format!("chronicle/{day}/{stream}/{segment}/{name}"),
                        name,
                    });
                }
            }
        }
    }
    outputs
}

/// List a directory's entries by name, ignoring an unreadable directory.
///
/// A name that is not valid UTF-8 cannot be expressed as a journal rel, so it
/// is skipped rather than migrated: this door never removes a file it could not
/// name back to the removal primitive.
fn sorted_children(directory: &Path) -> Vec<(String, PathBuf)> {
    let Ok(entries) = fs::read_dir(directory) else {
        return Vec::new();
    };
    let mut children: Vec<(String, PathBuf)> = entries
        .flatten()
        .filter_map(|entry| {
            entry
                .file_name()
                .to_str()
                .map(|name| (name.to_owned(), entry.path()))
        })
        .collect();
    children.sort_by(|left, right| left.0.cmp(&right.0));
    children
}

/// Parse the first line of a JSONL output as its header object.
///
/// Any unreadable file — absent, a directory, or not UTF-8 — reads as headerless
/// rather than as an error, which is the conservative direction: a headerless
/// file is never removed.
fn read_header(path: &Path) -> Option<Map<String, Value>> {
    let text = fs::read_to_string(path).ok()?;
    let first_line = text.lines().next()?;
    let value = serde_json::from_str::<Value>(first_line).ok()?;
    match value {
        Value::Object(header) => Some(header),
        _ => None,
    }
}

/// Whether a raw PDF with the same stem sits beside `name` in `directory`.
fn same_stem_pdf_exists(directory: &Path, name: &str) -> bool {
    let stem = file_stem(name);
    sorted_children(directory)
        .into_iter()
        .any(|(sibling, path)| {
            file_suffix(&sibling).eq_ignore_ascii_case(PDF_EXTENSION)
                && file_stem(&sibling) == stem
                && path.is_file()
        })
}

/// Whether any `*_transcript.md` entry exists beside the legacy output.
fn transcript_exists(directory: &Path) -> bool {
    sorted_children(directory)
        .iter()
        .any(|(name, _)| name.ends_with(TRANSCRIPT_SUFFIX))
}

/// Whether any readable `*_transcript.md` file carries content past its header.
fn transcript_with_body_exists(directory: &Path) -> bool {
    sorted_children(directory)
        .into_iter()
        .filter(|(name, path)| name.ends_with(TRANSCRIPT_SUFFIX) && path.is_file())
        .any(|(_, path)| markdown_has_body(&path))
}

/// Whether a transcript carries a body, not just a title and metadata.
///
/// A front-matter style `---` rule ends the header, so anything after the first
/// one is the body. Without a rule, a line that is neither blank, nor a
/// heading, nor a `**Label:** value` metadata line is the body.
fn markdown_has_body(path: &Path) -> bool {
    let Ok(text) = fs::read_to_string(path) else {
        return false;
    };
    let lines: Vec<&str> = text.lines().collect();
    for (index, line) in lines.iter().enumerate() {
        if line.trim() == "---" {
            return lines[index + 1..]
                .iter()
                .any(|rest| !rest.trim().is_empty());
        }
    }
    for line in &lines {
        let stripped = line.trim();
        if stripped.is_empty() || stripped.starts_with('#') {
            continue;
        }
        if stripped.starts_with("**") && stripped.contains(":**") {
            continue;
        }
        return true;
    }
    false
}

/// Join every non-empty `text` field of a document extraction.
///
/// `None` means a line could not be parsed, and the file must be preserved.
/// `Some("")` means the file parsed and holds no text at all.
fn read_document_text(path: &Path) -> Option<String> {
    let text = fs::read_to_string(path).ok()?;
    let mut parts: Vec<String> = Vec::new();
    for line in text.lines().skip(1) {
        if line.trim().is_empty() {
            continue;
        }
        let entry = serde_json::from_str::<Value>(line).ok()?;
        let Some(object) = entry.as_object() else {
            continue;
        };
        if let Some(field) = object.get("text").and_then(Value::as_str) {
            let trimmed = field.trim();
            if !trimmed.is_empty() {
                parts.push(trimmed.to_owned());
            }
        }
    }
    Some(parts.join("\n\n").trim().to_owned())
}

fn render_document_markdown(title: &str, text: &str, header: &Map<String, Value>) -> String {
    let mut lines = vec![
        format!("# {title}"),
        String::new(),
        "**Type:** Document".to_owned(),
    ];
    if let Some(pages) = header.get("page_count").filter(|value| !value.is_null()) {
        lines.push(format!("**Pages:** {}", scalar_display(pages)));
    }
    if let Some(method) = header
        .get("extraction_method")
        .filter(|value| is_truthy(value))
    {
        lines.push(format!("**Extraction method:** {}", scalar_display(method)));
    }
    lines.extend([
        String::new(),
        "---".to_owned(),
        String::new(),
        text.trim().to_owned(),
    ]);
    let mut rendered = lines.join("\n");
    rendered.truncate(rendered.trim_end().len());
    rendered.push('\n');
    rendered
}

/// Render a header value the way the retired Python renderer interpolated it.
fn scalar_display(value: &Value) -> String {
    match value {
        Value::String(text) => text.clone(),
        Value::Bool(flag) => if *flag { "True" } else { "False" }.to_owned(),
        Value::Null => "None".to_owned(),
        other => other.to_string(),
    }
}

/// Python truthiness for a header value, used for the optional metadata lines.
fn is_truthy(value: &Value) -> bool {
    match value {
        Value::Null => false,
        Value::Bool(flag) => *flag,
        Value::Number(number) => number.as_f64().is_none_or(|value| value != 0.0),
        Value::String(text) => !text.is_empty(),
        Value::Array(items) => !items.is_empty(),
        Value::Object(fields) => !fields.is_empty(),
    }
}

/// Write the transcript, then prove it landed readable and non-empty.
///
/// This is the gate the removal below depends on. It must fail loudly rather
/// than let a caller delete the only readable copy of the extracted text.
fn write_verified(path: &Path, contents: &str) -> Result<(), PdfExtractionMigrationError> {
    write_text(path, contents, AtomicWriteOptions::default()).map_err(|error| {
        PdfExtractionMigrationError::new(format!("could not write {}: {error}", path.display()))
    })?;
    if !path.is_file() {
        return Err(PdfExtractionMigrationError::new(format!(
            "failed to write {}",
            path.display()
        )));
    }
    let written = fs::read_to_string(path).map_err(|error| {
        PdfExtractionMigrationError::new(format!("could not read {}: {error}", path.display()))
    })?;
    if written.trim().is_empty() {
        return Err(PdfExtractionMigrationError::new(format!(
            "wrote empty transcript {}",
            path.display()
        )));
    }
    Ok(())
}

fn remove_legacy_output(journal: &Path, rel: &str) -> Result<(), PdfExtractionMigrationError> {
    remove_file(journal, rel).map(|_| ()).map_err(|error| {
        PdfExtractionMigrationError::new(format!("could not remove {rel}: {error}"))
    })
}

/// Split a file name into Python's `PurePath` stem and suffix.
///
/// The dot must be neither the first nor the last character, so `.hidden` is
/// all stem and `trailing.` has no suffix.
fn split_name(name: &str) -> Option<(&str, &str)> {
    let index = name.rfind('.')?;
    if index > 0 && index < name.len() - 1 {
        Some((&name[..index], &name[index..]))
    } else {
        None
    }
}

fn file_stem(name: &str) -> &str {
    split_name(name).map_or(name, |(stem, _)| stem)
}

fn file_suffix(name: &str) -> &str {
    split_name(name).map_or("", |(_, suffix)| suffix)
}

/// The stem of the final component of a recorded raw path.
fn path_stem(value: &str) -> &str {
    let name = value
        .rsplit('/')
        .find(|part| !part.is_empty() && *part != ".")
        .unwrap_or_default();
    file_stem(name)
}

#[cfg(test)]
#[allow(clippy::disallowed_methods, clippy::disallowed_types)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::test_support::TempDir;

    fn segment_dir(root: &Path, day: &str, stream: &str, segment: &str) -> PathBuf {
        let path = root.join("chronicle").join(day).join(stream).join(segment);
        fs::create_dir_all(&path).expect("segment directory");
        path
    }

    fn write_jsonl(path: &Path, header: &Value, entries: &[Value]) {
        let mut lines = vec![serde_json::to_string(header).expect("header serializes")];
        lines.extend(
            entries
                .iter()
                .map(|entry| serde_json::to_string(entry).expect("entry serializes")),
        );
        fs::write(path, format!("{}\n", lines.join("\n"))).expect("jsonl written");
    }

    #[test]
    fn duplicate_document_and_image_jsonl_are_deleted_beside_a_transcript_with_a_body() {
        let temporary = TempDir::new();
        let segment = segment_dir(temporary.path(), "20250101", "import.document", "120000_0");
        fs::write(segment.join("document_transcript.md"), "already migrated")
            .expect("transcript written");
        let document = segment.join("document.jsonl");
        let image = segment.join("image.jsonl");
        write_jsonl(
            &document,
            &json!({"raw": "original.pdf", "kind": "document"}),
            &[json!({"start": "00:00:00", "text": "duplicate document"})],
        );
        write_jsonl(
            &image,
            &json!({"raw": "image.png", "kind": "image"}),
            &[json!({"start": "00:00:00", "text": "duplicate image"})],
        );

        let report = migrate_pdf_extractions(temporary.path()).expect("migration runs");

        assert_eq!(
            report,
            PdfExtractionMigrationReport {
                scanned: 2,
                deleted_duplicates: 2,
                ..PdfExtractionMigrationReport::default()
            }
        );
        assert!(!document.exists());
        assert!(!image.exists());
        assert_eq!(
            fs::read_to_string(segment.join("document_transcript.md")).expect("transcript read"),
            "already migrated",
            "the durable transcript is left exactly as it was"
        );
    }

    #[test]
    fn an_orphan_document_jsonl_in_any_stream_converts_once_and_then_has_nothing_left_to_do() {
        let temporary = TempDir::new();
        let segment = segment_dir(temporary.path(), "20250101", "manual.docs", "120000_0");
        let jsonl = segment.join("document.jsonl");
        write_jsonl(
            &jsonl,
            &json!({
                "raw": "original.pdf",
                "kind": "document",
                "page_count": 3,
                "extraction_method": "pypdf",
            }),
            &[
                json!({"start": "00:00:00", "text": "First extracted paragraph."}),
                json!({"start": "00:00:01", "text": "Second extracted paragraph."}),
            ],
        );

        let report = migrate_pdf_extractions(temporary.path()).expect("migration runs");

        assert_eq!(
            report,
            PdfExtractionMigrationReport {
                scanned: 1,
                converted_documents: 1,
                ..PdfExtractionMigrationReport::default()
            }
        );
        assert!(!jsonl.exists());
        let markdown =
            fs::read_to_string(segment.join("document_transcript.md")).expect("transcript read");
        assert_eq!(
            markdown,
            concat!(
                "# original\n",
                "\n",
                "**Type:** Document\n",
                "**Pages:** 3\n",
                "**Extraction method:** pypdf\n",
                "\n",
                "---\n",
                "\n",
                "First extracted paragraph.\n",
                "\n",
                "Second extracted paragraph.\n",
            )
        );

        let rerun = migrate_pdf_extractions(temporary.path()).expect("migration reruns");
        assert_eq!(rerun, PdfExtractionMigrationReport::default());
        assert_eq!(
            fs::read_to_string(segment.join("document_transcript.md")).expect("transcript read"),
            markdown,
            "a second run rewrites nothing"
        );
    }

    /// An image extraction has no markdown form, so an orphan one is counted
    /// and then left exactly where it is — converting it would invent text.
    #[test]
    fn an_orphan_image_extraction_is_scanned_and_then_left_untouched() {
        let temporary = TempDir::new();
        let segment = segment_dir(temporary.path(), "20250101", "import.image", "120000_0");
        let image = segment.join("image.jsonl");
        write_jsonl(
            &image,
            &json!({"raw": "photo.png", "kind": "image"}),
            &[json!({"start": "00:00:00", "text": "a caption"})],
        );

        let report = migrate_pdf_extractions(temporary.path()).expect("migration runs");

        assert_eq!(
            report,
            PdfExtractionMigrationReport {
                scanned: 1,
                ..PdfExtractionMigrationReport::default()
            }
        );
        assert!(image.is_file());
        assert!(!segment.join(DOCUMENT_TRANSCRIPT_NAME).exists());
    }

    /// Absent optional header fields drop their lines rather than rendering a
    /// placeholder, and the title comes from the recorded raw name's stem.
    #[test]
    fn a_document_without_optional_header_metadata_renders_only_the_type_line() {
        let temporary = TempDir::new();
        let segment = segment_dir(temporary.path(), "20250101", "manual.docs", "120000_0");
        write_jsonl(
            &segment.join("document.jsonl"),
            &json!({"raw": "Quarterly Notes.PDF", "kind": "document", "page_count": null}),
            &[json!({"start": "00:00:00", "text": "  Body text.  "})],
        );

        let report = migrate_pdf_extractions(temporary.path()).expect("migration runs");

        assert_eq!(report.converted_documents, 1);
        assert_eq!(
            fs::read_to_string(segment.join(DOCUMENT_TRANSCRIPT_NAME)).expect("transcript read"),
            "# Quarterly Notes\n\n**Type:** Document\n\n---\n\nBody text.\n"
        );
    }

    #[test]
    fn unparseable_and_textless_outputs_stay_in_place_and_are_reported() {
        let temporary = TempDir::new();
        let segment = segment_dir(temporary.path(), "20250101", "import.document", "120000_0");
        let malformed = segment.join("malformed.jsonl");
        fs::write(&malformed, "{not json\n").expect("malformed written");
        fs::write(segment.join("malformed.PDF"), b"%PDF-1.4 synthetic").expect("raw pdf written");
        let no_text = segment.join("no_text.jsonl");
        write_jsonl(
            &no_text,
            &json!({"raw": "empty.pdf", "kind": "document"}),
            &[json!({"start": "00:00:00", "text": ""})],
        );
        let screen = segment.join("screen.jsonl");
        fs::write(&screen, "{}\n{\"text\": \"screen output\"}\n").expect("screen written");
        let empty = segment.join("empty.jsonl");
        fs::write(&empty, "").expect("empty written");

        let report = migrate_pdf_extractions(temporary.path()).expect("migration runs");

        assert_eq!(
            report,
            PdfExtractionMigrationReport {
                scanned: 2,
                skipped_unparseable: 1,
                skipped_no_text: 1,
                left_in_place: vec![
                    "chronicle/20250101/import.document/120000_0/malformed.jsonl".to_owned(),
                    "chronicle/20250101/import.document/120000_0/no_text.jsonl".to_owned(),
                ],
                ..PdfExtractionMigrationReport::default()
            }
        );
        for preserved in [&malformed, &no_text, &screen, &empty] {
            assert!(preserved.is_file(), "{} was removed", preserved.display());
        }
        assert!(!segment.join(DOCUMENT_TRANSCRIPT_NAME).exists());
        assert_eq!(
            report.report_lines().last().expect("a reported path"),
            "Left in place: chronicle/20250101/import.document/120000_0/no_text.jsonl"
        );
    }

    #[test]
    fn a_header_only_transcript_refuses_the_deletion_and_reports_the_jsonl() {
        let temporary = TempDir::new();
        let segment = segment_dir(temporary.path(), "20250101", "import.document", "120000_0");
        let jsonl = segment.join("document.jsonl");
        write_jsonl(
            &jsonl,
            &json!({"raw": "original.pdf", "kind": "document"}),
            &[json!({"start": "00:00:00", "text": "Only durable extracted text."})],
        );
        fs::write(
            segment.join("document_transcript.md"),
            "# original\n\n**Type:** Document\n**Pages:** 1\n\n---\n",
        )
        .expect("transcript written");

        let report = migrate_pdf_extractions(temporary.path()).expect("migration runs");

        assert_eq!(
            report,
            PdfExtractionMigrationReport {
                scanned: 1,
                skipped_empty_transcript: 1,
                left_in_place: vec![
                    "chronicle/20250101/import.document/120000_0/document.jsonl".to_owned(),
                ],
                ..PdfExtractionMigrationReport::default()
            }
        );
        assert!(
            fs::read_to_string(&jsonl)
                .expect("jsonl read")
                .contains("Only durable extracted text."),
            "the only readable copy of the text survives"
        );
    }
}
