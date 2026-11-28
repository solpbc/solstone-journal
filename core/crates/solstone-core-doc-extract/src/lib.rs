// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Pure-Rust bounded document text extraction shared across desktop and
//! standalone mobile hosts.
//!
//! Supported formats in this crate:
//! - Modern Microsoft Office OOXML: `.docx`, `.docm`, `.xlsx`, `.xlsm`,
//!   `.xlsb`, `.pptx`, `.pptm`, `.ppsx`
//! - Legacy binary Microsoft Office: `.doc`, `.xls`, `.ppt`
//! - E-books and OpenDocument: `.epub`, `.odt`, `.ods`, `.odp`
//! - Rich Text Format: `.rtf`
//! - General structured text documents: `.eml`, `.csv`, `.tsv`, `.html`, `.htm`
//!
//! Extraction produces structured Markdown, plain text, optional document
//! title metadata, and truncation/diagnostic warnings without executing
//! macros, inflating embedded images, or invoking external processes.

mod epub;
mod html_render;
mod odf;
mod office;
mod rtf;
mod sniff;
mod structured;

#[cfg(test)]
mod tests;

use std::fmt;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::Path;

use crate::sniff::SniffedContainer;

/// Document extensions that never collide with specialized structured export
/// importers (Strava, Fireflies, Whisper, Telegram). Safe for auto-detection.
pub const UNAMBIGUOUS_DOCUMENT_EXTENSIONS: &[&str] = &[
    "docx", "docm", "xlsx", "xlsm", "xlsb", "pptx", "pptm", "ppsx", "doc", "xls", "ppt", "epub",
    "odt", "ods", "odp", "rtf", "eml",
];

/// All lowercase file extensions (without leading dots) accepted by this crate.
pub const SUPPORTED_EXTENSIONS: &[&str] = &[
    "docx", "docm", "xlsx", "xlsm", "xlsb", "pptx", "pptm", "ppsx", "doc", "xls", "ppt", "epub",
    "odt", "ods", "odp", "rtf", "eml", "csv", "tsv", "html", "htm",
];

/// Whether `ext` (with or without a leading dot, case-insensitive) is a
/// document extension supported by this crate.
#[must_use]
pub fn is_supported_extension(ext: &str) -> bool {
    DocumentFormat::from_extension(ext).is_some()
}

/// Whether `ext` (with or without a leading dot, case-insensitive) is an
/// unambiguous document extension that never collides with specialized
/// structured export formats (`.csv`, `.tsv`, `.html`, `.htm`).
#[must_use]
pub fn is_unambiguous_document_extension(ext: &str) -> bool {
    DocumentFormat::from_extension(ext).is_some_and(DocumentFormat::is_unambiguous)
}

/// Fast, size-independent container-marker check for triage classification.
///
/// Inspects ZIP central-directory entry names (plus bounded `mimetype` /
/// `META-INF/manifest.xml` inspection for EPUB and OpenDocument) or MS-CFB
/// top-level directory names without decompressing document payload streams or
/// enforcing output/part-size limits, so oversized or bomb document containers
/// still classify as documents rather than falling through to plain-text notes.
#[must_use]
pub fn is_supported_container(bytes: &[u8]) -> bool {
    sniff::is_supported_container(bytes)
}

/// Document formats supported by this extractor.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum DocumentFormat {
    /// Office Open XML Word document (`.docx`, `.docm`).
    Docx,
    /// Office Open XML Excel workbook (`.xlsx`, `.xlsm`).
    Xlsx,
    /// Office Open XML Binary Excel workbook (`.xlsb`).
    Xlsb,
    /// Office Open XML PowerPoint presentation (`.pptx`, `.pptm`, `.ppsx`).
    Pptx,
    /// Legacy binary Word document (`.doc`).
    Doc,
    /// Legacy binary Excel workbook (`.xls`).
    Xls,
    /// Legacy binary PowerPoint presentation (`.ppt`).
    Ppt,
    /// EPUB 2 or 3 e-book archive (`.epub`).
    Epub,
    /// OpenDocument Text (`.odt`).
    Odt,
    /// OpenDocument Spreadsheet (`.ods`).
    Ods,
    /// OpenDocument Presentation (`.odp`).
    Odp,
    /// Rich Text Format document (`.rtf`).
    Rtf,
    /// Saved RFC 5322 email message (`.eml`).
    Eml,
    /// Comma-separated tabular document (`.csv`).
    Csv,
    /// Tab-separated tabular document (`.tsv`).
    Tsv,
    /// HTML document (`.html`, `.htm`).
    Html,
}

impl DocumentFormat {
    /// Canonical lowercase file extension without a leading dot.
    #[must_use]
    pub const fn extension(self) -> &'static str {
        match self {
            Self::Docx => "docx",
            Self::Xlsx => "xlsx",
            Self::Xlsb => "xlsb",
            Self::Pptx => "pptx",
            Self::Doc => "doc",
            Self::Xls => "xls",
            Self::Ppt => "ppt",
            Self::Epub => "epub",
            Self::Odt => "odt",
            Self::Ods => "ods",
            Self::Odp => "odp",
            Self::Rtf => "rtf",
            Self::Eml => "eml",
            Self::Csv => "csv",
            Self::Tsv => "tsv",
            Self::Html => "html",
        }
    }

    /// Whether this format's extensions never collide with specialized
    /// structured exports (Strava CSV, Fireflies CSV, Whisper TSV, Telegram
    /// HTML).
    #[must_use]
    pub const fn is_unambiguous(self) -> bool {
        !matches!(self, Self::Csv | Self::Tsv | Self::Html)
    }

    /// Whether this format is a non-archive structured text format.
    #[must_use]
    pub(crate) const fn is_structured_text(self) -> bool {
        matches!(self, Self::Csv | Self::Tsv | Self::Html | Self::Eml)
    }

    /// Map a file extension (with or without a leading dot, case-insensitive)
    /// to a supported document format.
    #[must_use]
    pub fn from_extension(ext: &str) -> Option<Self> {
        let trimmed = ext.trim().trim_start_matches('.');
        match trimmed.to_ascii_lowercase().as_str() {
            "docx" | "docm" => Some(Self::Docx),
            "xlsx" | "xlsm" => Some(Self::Xlsx),
            "xlsb" => Some(Self::Xlsb),
            "pptx" | "pptm" | "ppsx" => Some(Self::Pptx),
            "doc" => Some(Self::Doc),
            "xls" => Some(Self::Xls),
            "ppt" => Some(Self::Ppt),
            "epub" => Some(Self::Epub),
            "odt" => Some(Self::Odt),
            "ods" => Some(Self::Ods),
            "odp" => Some(Self::Odp),
            "rtf" => Some(Self::Rtf),
            "eml" => Some(Self::Eml),
            "csv" => Some(Self::Csv),
            "tsv" => Some(Self::Tsv),
            "html" | "htm" => Some(Self::Html),
            _ => None,
        }
    }

    /// Map a filename or path to a supported document format using its
    /// extension.
    #[must_use]
    pub fn from_filename(filename: &str) -> Option<Self> {
        let ext = Path::new(filename).extension()?.to_str()?;
        Self::from_extension(ext)
    }

    /// All file extensions accepted by this crate (lowercase, without dots).
    #[must_use]
    pub const fn supported_extensions() -> &'static [&'static str] {
        SUPPORTED_EXTENSIONS
    }
}

impl fmt::Display for DocumentFormat {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.extension())
    }
}

/// Resource bounds enforced for a single document extraction.
///
/// Every field on this struct is actively enforced against the input bytes,
/// container archive, or rendered output.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExtractLimits {
    /// Maximum encoded input file size in bytes.
    pub max_input_bytes: u64,
    /// Maximum archive or compound-file entries inspected/allowed.
    pub max_package_entries: usize,
    /// Maximum cumulative decompressed bytes read from one container.
    pub max_uncompressed_bytes: u64,
    /// Maximum decompressed bytes read from any single part.
    pub max_part_bytes: u64,
    /// Maximum decompressed-to-compressed ratio allowed for large parts.
    pub max_inflate_ratio: u64,
    /// Maximum internal working memory budget for sparse structures/expansion.
    pub max_memory_bytes: u64,
    /// Anti-bomb ceiling on internal text fan-out (e.g., shared-string
    /// multiplication or ODF repeat expansion) in Unicode characters. Must be
    /// strictly greater than `max_output_chars` so legitimate long documents
    /// truncate cleanly while expansion bombs fail with
    /// [`ExtractError::LimitExceeded`].
    pub max_expanded_text_chars: usize,
    /// Maximum Unicode characters (`char` scalar values) retained in each of
    /// `markdown` and `plain_text`. Exceeding this truncates cleanly and sets
    /// `ExtractedDocument::truncated = true`.
    pub max_output_chars: usize,
}

impl ExtractLimits {
    /// Default bounds for desktop hosts (Linux, macOS, Windows).
    #[must_use]
    pub const fn desktop() -> Self {
        Self {
            max_input_bytes: 32 * 1024 * 1024,
            max_package_entries: 10_000,
            max_uncompressed_bytes: 64 * 1024 * 1024,
            max_part_bytes: 32 * 1024 * 1024,
            max_inflate_ratio: 100,
            max_memory_bytes: 64 * 1024 * 1024,
            max_expanded_text_chars: 4_000_000,
            max_output_chars: 512_000,
        }
    }

    /// Tighter bounds for standalone mobile hosts (iOS, Android).
    #[must_use]
    pub const fn mobile() -> Self {
        Self {
            max_input_bytes: 16 * 1024 * 1024,
            max_package_entries: 4_000,
            max_uncompressed_bytes: 24 * 1024 * 1024,
            max_part_bytes: 12 * 1024 * 1024,
            max_inflate_ratio: 100,
            max_memory_bytes: 24 * 1024 * 1024,
            max_expanded_text_chars: 1_500_000,
            max_output_chars: 256_000,
        }
    }

    /// Default bounds for the compilation target (`mobile()` on iOS/Android,
    /// `desktop()` elsewhere).
    #[must_use]
    pub const fn for_current_platform() -> Self {
        #[cfg(any(target_os = "ios", target_os = "android"))]
        {
            Self::mobile()
        }
        #[cfg(not(any(target_os = "ios", target_os = "android")))]
        {
            Self::desktop()
        }
    }
}

impl Default for ExtractLimits {
    fn default() -> Self {
        Self::desktop()
    }
}

/// Extracted text and metadata from a readable document.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExtractedDocument {
    /// Detected format that produced this text.
    pub format: DocumentFormat,
    /// Optional document title from container metadata (trimmed, non-empty).
    pub title: Option<String>,
    /// Structured Markdown rendering (headings, lists, tables, slide breaks,
    /// speaker notes), bounded to `limits.max_output_chars` Unicode chars.
    pub markdown: String,
    /// Plain text rendering, bounded to `limits.max_output_chars` Unicode chars.
    pub plain_text: String,
    /// `true` when extracted content was truncated by output, row, or column
    /// limits.
    pub truncated: bool,
    /// Non-fatal diagnostics recorded during extraction (sanitized of internal
    /// dependency paths).
    pub warnings: Vec<String>,
}

/// Outcome of a bounded document extraction attempt.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ExtractOutcome {
    /// Text was extracted successfully.
    Extracted(ExtractedDocument),
    /// The file is a valid document of a supported format, but contains no
    /// extractable text (for example, a blank sheet or diagram-only slide).
    Empty {
        /// Detected format of the text-free document.
        format: DocumentFormat,
    },
    /// The file is encrypted or password-protected and cannot be read.
    Encrypted,
    /// The file is not a format supported by this extractor.
    Unsupported,
}

/// Failure modes for document extraction.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum ExtractError {
    /// The input is empty, truncated, or structurally invalid.
    #[error("invalid document: {0}")]
    Invalid(String),
    /// The input exceeded one of the configured [`ExtractLimits`].
    #[error("document exceeded extraction limit: {0}")]
    LimitExceeded(String),
    /// An unexpected internal failure (such as a caught panic) occurred.
    #[error("internal extraction error: {0}")]
    Internal(String),
}

/// Raw extraction payload returned by an internal format backend before common
/// sanitization, emptiness detection, and output truncation.
pub(crate) struct RawExtraction {
    pub(crate) format: DocumentFormat,
    pub(crate) title: Option<String>,
    pub(crate) markdown: String,
    pub(crate) plain_text: String,
    pub(crate) truncated: bool,
    pub(crate) warnings: Vec<String>,
}

/// Internal backend outcome before common post-processing.
pub(crate) enum BackendOutcome {
    Extracted(RawExtraction),
    Encrypted,
}

/// Internal backend seam isolating callers from any specific parser crate.
pub(crate) trait FormatBackend {
    fn extract(
        &self,
        bytes: &[u8],
        format: DocumentFormat,
        limits: &ExtractLimits,
    ) -> Result<BackendOutcome, ExtractError>;
}

struct DefaultBackend;

impl FormatBackend for DefaultBackend {
    fn extract(
        &self,
        bytes: &[u8],
        format: DocumentFormat,
        limits: &ExtractLimits,
    ) -> Result<BackendOutcome, ExtractError> {
        match format {
            DocumentFormat::Docx
            | DocumentFormat::Xlsx
            | DocumentFormat::Xlsb
            | DocumentFormat::Pptx
            | DocumentFormat::Doc
            | DocumentFormat::Xls
            | DocumentFormat::Ppt => office::OfficeBackend.extract(bytes, format, limits),
            DocumentFormat::Epub => epub::extract_epub(bytes, limits),
            DocumentFormat::Odt | DocumentFormat::Ods | DocumentFormat::Odp => {
                odf::extract_odf(bytes, format, limits)
            }
            DocumentFormat::Rtf => rtf::extract_rtf(bytes, limits),
            DocumentFormat::Csv
            | DocumentFormat::Tsv
            | DocumentFormat::Html
            | DocumentFormat::Eml => structured::extract_structured(bytes, format, limits),
        }
    }
}

/// Extract text and structure from in-memory document bytes under strict
/// resource bounds.
///
/// `filename_hint` may be a filename, relative path, or `None`. Container
/// magic bytes and internal structure take precedence over the hint when
/// determining format or encryption, so misnamed files and unhinted buffers
/// route accurately without treating non-document archives as encrypted.
pub fn extract_bytes(
    bytes: &[u8],
    filename_hint: Option<&str>,
    limits: &ExtractLimits,
) -> Result<ExtractOutcome, ExtractError> {
    extract_with_backend(bytes, filename_hint, limits, &DefaultBackend)
}

pub(crate) fn extract_with_backend(
    bytes: &[u8],
    filename_hint: Option<&str>,
    limits: &ExtractLimits,
    backend: &(dyn FormatBackend + Sync),
) -> Result<ExtractOutcome, ExtractError> {
    if bytes.is_empty() {
        return Err(ExtractError::Invalid("document input is empty".to_owned()));
    }
    if (bytes.len() as u64) > limits.max_input_bytes {
        return Err(ExtractError::LimitExceeded(format!(
            "input size {} bytes exceeds limit of {} bytes",
            bytes.len(),
            limits.max_input_bytes
        )));
    }

    let hinted = filename_hint.and_then(DocumentFormat::from_filename);

    let container = sniff::sniff_container(bytes, limits)?;

    let format = match container {
        SniffedContainer::EncryptedOoxml => return Ok(ExtractOutcome::Encrypted),
        SniffedContainer::Ooxml(fmt)
        | SniffedContainer::LegacyCfb(fmt)
        | SniffedContainer::Odf(fmt) => fmt,
        SniffedContainer::Epub => DocumentFormat::Epub,
        SniffedContainer::Rtf => DocumentFormat::Rtf,
        SniffedContainer::NonDocumentZip => {
            return match hinted {
                Some(_) => Err(ExtractError::Invalid(
                    "ZIP archive does not contain a supported document entry".to_owned(),
                )),
                None => Ok(ExtractOutcome::Unsupported),
            };
        }
        SniffedContainer::NonOfficeCfb => {
            return match hinted {
                Some(_) => Err(ExtractError::Invalid(
                    "compound file does not contain a supported Office stream".to_owned(),
                )),
                None => Ok(ExtractOutcome::Unsupported),
            };
        }
        SniffedContainer::Unknown => match hinted {
            Some(fmt) if fmt.is_structured_text() => fmt,
            Some(_) => {
                return Err(ExtractError::Invalid(
                    "file header does not match a supported document container".to_owned(),
                ));
            }
            None if structured::sniff_unhinted_html(bytes) => DocumentFormat::Html,
            None => return Ok(ExtractOutcome::Unsupported),
        },
    };

    let guarded = catch_unwind(AssertUnwindSafe(|| backend.extract(bytes, format, limits)));
    let backend_outcome = match guarded {
        Ok(result) => result?,
        Err(payload) => {
            let reason = panic_message(payload.as_ref());
            return Err(ExtractError::Internal(format!(
                "extractor panicked: {reason}"
            )));
        }
    };

    match backend_outcome {
        BackendOutcome::Encrypted => Ok(ExtractOutcome::Encrypted),
        BackendOutcome::Extracted(raw) => Ok(finalize_extraction(raw, limits)),
    }
}

fn finalize_extraction(raw: RawExtraction, limits: &ExtractLimits) -> ExtractOutcome {
    let title = raw
        .title
        .map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty());

    let mut warnings: Vec<String> = raw
        .warnings
        .into_iter()
        .filter_map(|warning| sanitize_warning(&warning))
        .collect();

    let (markdown_clean, md_notices) = strip_upstream_notices(&raw.markdown);
    let (plain_clean, plain_notices) = strip_upstream_notices(&raw.plain_text);
    for notice in md_notices.into_iter().chain(plain_notices) {
        if let Some(clean) = sanitize_warning(&notice)
            && !warnings.contains(&clean)
        {
            warnings.push(clean);
        }
    }

    let markdown_trimmed = markdown_clean.trim();
    let plain_trimmed = plain_clean.trim();
    if markdown_trimmed.is_empty() && plain_trimmed.is_empty() {
        return ExtractOutcome::Empty { format: raw.format };
    }

    let (markdown, md_truncated) = truncate_chars(markdown_trimmed, limits.max_output_chars);
    let (plain_text, plain_truncated) = truncate_chars(plain_trimmed, limits.max_output_chars);
    let char_truncated = md_truncated || plain_truncated;
    if char_truncated {
        let msg = format!(
            "extracted text truncated to {} characters",
            limits.max_output_chars
        );
        if !warnings.contains(&msg) {
            warnings.push(msg);
        }
    }
    let truncated = char_truncated || raw.truncated;

    ExtractOutcome::Extracted(ExtractedDocument {
        format: raw.format,
        title,
        markdown,
        plain_text,
        truncated,
        warnings,
    })
}

/// Truncate `text` to at most `max_chars` Unicode scalar values without
/// splitting a multi-byte UTF-8 sequence.
fn truncate_chars(text: &str, max_chars: usize) -> (String, bool) {
    let mut chars = text.char_indices();
    if let Some((byte_idx, _)) = chars.nth(max_chars) {
        (text[..byte_idx].trim_end().to_owned(), true)
    } else {
        (text.to_owned(), false)
    }
}

/// Strip in-band diagnostic bracket lines emitted by upstream parsers (such as
/// lines referencing `office_oxide::`) so they never appear as document text or
/// fake non-empty output.
pub(crate) fn strip_upstream_notices(text: &str) -> (String, Vec<String>) {
    let mut kept = Vec::new();
    let mut notices = Vec::new();
    for line in text.lines() {
        let trimmed = line.trim();
        let is_upstream_notice = trimmed.contains("office_oxide::")
            || (trimmed.starts_with('[')
                && trimmed.ends_with(']')
                && (trimmed.contains("output truncated:")
                    || trimmed.contains("nesting depth")
                    || trimmed.contains("limit reached")));
        if is_upstream_notice {
            notices.push(trimmed.trim_matches(['[', ']']).trim().to_owned());
        } else {
            kept.push(line);
        }
    }
    (kept.join("\n"), notices)
}

/// Remove internal Rust dependency paths (such as `office_oxide::limits::*`)
/// from diagnostic messages before exposing them to callers.
pub(crate) fn sanitize_warning(raw: &str) -> Option<String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return None;
    }
    let mut cleaned = trimmed.to_owned();
    while let Some(start) = cleaned.find("(office_oxide::") {
        if let Some(rel_end) = cleaned[start..].find(')') {
            let end = start + rel_end + 1;
            cleaned.replace_range(start..end, "");
        } else {
            cleaned.truncate(start);
            break;
        }
    }
    while let Some(pos) = cleaned.find("office_oxide::") {
        let end = cleaned[pos..]
            .find(|c: char| c.is_whitespace() || matches!(c, ')' | ']' | ',' | ';'))
            .map_or(cleaned.len(), |offset| pos + offset);
        cleaned.replace_range(pos..end, "parser limit");
    }
    let collapsed = cleaned.split_whitespace().collect::<Vec<_>>().join(" ");
    (!collapsed.is_empty()).then_some(collapsed)
}

fn panic_message(payload: &(dyn std::any::Any + Send)) -> String {
    if let Some(msg) = payload.downcast_ref::<&str>() {
        (*msg).to_owned()
    } else if let Some(msg) = payload.downcast_ref::<String>() {
        msg.clone()
    } else {
        "unknown panic payload".to_owned()
    }
}
