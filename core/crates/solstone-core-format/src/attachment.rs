// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! An extracted file before its source chooses a segment, grants and retention.

use std::path::Path;
use std::sync::Arc;

pub type ReadAttachment =
    dyn Fn(&str, &str, &[u8]) -> Result<PreparedAttachment, PrepareFailure> + Send + Sync;

/// One host's source-file readers, retained across the background sync thread.
#[derive(Clone)]
pub struct SharedAttachmentReader {
    pub read: Arc<ReadAttachment>,
    pub pdf_unavailable: bool,
}

impl SharedAttachmentReader {
    /// Both mobile entry points use the same capability until their native PDF
    /// reader can run untrusted mail with bounded resources.
    pub fn mobile(read: Arc<ReadAttachment>) -> Self {
        Self {
            read,
            pdf_unavailable: true,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PreparedAttachment {
    pub kind: &'static str,
    pub text: String,
    /// A freshly encoded, bounded, metadata-free image, subject to the image grant.
    pub preview_jpeg: Option<Vec<u8>>,
    /// Some source material could not be read under the current limits.
    pub partial: bool,
    /// Model requests dispatched during preparation.
    pub thoughts: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PrepareFailure {
    Encrypted,
    /// The provider returned a completion or refusal resulting from generation.
    DescriptionUnavailable,
    Unsupported,
    Unreadable,
    OverBudget,
    Unavailable,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FileType {
    Pdf,
    Document,
    Text,
    Jpeg,
    Png,
    WebP,
    Gif,
}

fn extension_input(name: &str) -> Option<FileType> {
    match Path::new(name)
        .extension()?
        .to_str()?
        .to_ascii_lowercase()
        .as_str()
    {
        "pdf" => Some(FileType::Pdf),
        "docx" | "docm" | "xlsx" | "xlsm" | "xlsb" | "pptx" | "pptm" | "ppsx" | "doc" | "xls"
        | "ppt" | "epub" | "odt" | "ods" | "odp" | "rtf" | "eml" | "csv" | "tsv" | "html"
        | "htm" => Some(FileType::Document),
        "txt" | "md" | "markdown" => Some(FileType::Text),
        "jpg" | "jpeg" => Some(FileType::Jpeg),
        "png" => Some(FileType::Png),
        "webp" => Some(FileType::WebP),
        "gif" => Some(FileType::Gif),
        _ => None,
    }
}

fn is_text_like_document_mime(base_mime: &str) -> bool {
    matches!(
        base_mime,
        "text/csv" | "application/csv" | "text/tab-separated-values" | "text/html"
    )
}

/// Candidate type from transport metadata. A decoder still has to confirm it.
pub fn file_type(name: &str, mime: &str) -> Option<FileType> {
    let base_mime = mime
        .split(';')
        .next()
        .unwrap_or_default()
        .trim()
        .to_ascii_lowercase();
    // Unnamed MIME parts can pass their own MIME type as `name` (for example
    // `application/vnd.openxmlformats-officedocument.wordprocessingml.document`).
    // Treat empty names and MIME-derived fallback names as extensionless so
    // dotted MIME subtypes are not mistaken for file extensions.
    let extensionless = name.is_empty() || name.eq_ignore_ascii_case(&base_mime);
    let extension = if extensionless {
        None
    } else {
        extension_input(name)
    };
    let has_raw_extension = !extensionless && Path::new(name).extension().is_some();
    let declared = match base_mime.as_str() {
        "" | "application/octet-stream" => return extension,
        "application/pdf" => FileType::Pdf,
        "application/msword"
        | "application/vnd.ms-excel"
        | "application/vnd.ms-powerpoint"
        | "application/vnd.openxmlformats-officedocument.wordprocessingml.document"
        | "application/vnd.ms-word.document.macroenabled.12"
        | "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet"
        | "application/vnd.ms-excel.sheet.macroenabled.12"
        | "application/vnd.ms-excel.sheet.binary.macroenabled.12"
        | "application/vnd.openxmlformats-officedocument.presentationml.presentation"
        | "application/vnd.ms-powerpoint.presentation.macroenabled.12"
        | "application/vnd.openxmlformats-officedocument.presentationml.slideshow"
        | "application/epub+zip"
        | "application/vnd.oasis.opendocument.text"
        | "application/vnd.oasis.opendocument.spreadsheet"
        | "application/vnd.oasis.opendocument.presentation"
        | "application/rtf"
        | "text/rtf"
        | "application/x-rtf"
        | "message/rfc822"
        | "text/csv"
        | "application/csv"
        | "text/tab-separated-values"
        | "text/html" => FileType::Document,
        "text/plain" | "text/markdown" | "text/x-markdown" => FileType::Text,
        "image/jpeg" => FileType::Jpeg,
        "image/png" => FileType::Png,
        "image/gif" => FileType::Gif,
        "image/webp" => FileType::WebP,
        _ => return None,
    };
    if extension.is_some_and(|ext| ext != declared)
        || (extension.is_none()
            && has_raw_extension
            && (declared == FileType::Text || is_text_like_document_mime(&base_mime)))
    {
        return None;
    }
    Some(declared)
}
