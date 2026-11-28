// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Microsoft Office extraction backend backed by `office_oxide`.

use std::io::Cursor;

use office_oxide::core::Error as CoreError;
use office_oxide::doc::DocError;
use office_oxide::docx::DocxError;
use office_oxide::options::{DamageKind, OnIncomplete, ParseOptions};
use office_oxide::ppt::PptError;
use office_oxide::pptx::PptxError;
use office_oxide::xls::XlsError;
use office_oxide::xlsx::XlsxError;
use office_oxide::{Document, DocumentFormat as OxideFormat, OfficeError};

use crate::{
    BackendOutcome, DocumentFormat, ExtractError, ExtractLimits, FormatBackend, RawExtraction,
    sanitize_warning,
};

pub(crate) struct OfficeBackend;

impl FormatBackend for OfficeBackend {
    fn extract(
        &self,
        bytes: &[u8],
        format: DocumentFormat,
        limits: &ExtractLimits,
    ) -> Result<BackendOutcome, ExtractError> {
        let oxide_format = to_oxide_format(format);
        let options = build_parse_options(limits);

        let doc =
            match Document::from_reader_with(Cursor::new(bytes.to_vec()), oxide_format, &options) {
                Ok(doc) => doc,
                Err(err) => return map_office_error(err),
            };

        let mut warnings = Vec::new();
        for damage in doc.damage() {
            let sanitized = sanitize_warning(&damage.to_string())
                .unwrap_or_else(|| "document parsing issue".to_owned());
            match damage.kind {
                DamageKind::UnreadablePart => {
                    if is_limit_message(&damage.detail) {
                        return Err(ExtractError::LimitExceeded(sanitized));
                    }
                    return Err(ExtractError::Invalid(sanitized));
                }
                DamageKind::Truncated => {
                    if is_limit_message(&damage.detail) {
                        return Err(ExtractError::LimitExceeded(sanitized));
                    }
                    warnings.push(sanitized);
                }
                DamageKind::Recovered => {
                    warnings.push(sanitized);
                }
                _ => {
                    warnings.push(sanitized);
                }
            }
        }

        let ir = doc.to_ir();
        if ir.metadata.text_truncated {
            return Err(ExtractError::LimitExceeded(
                "document expanded past text budget".to_owned(),
            ));
        }

        let title = ir.metadata.title;
        let mut markdown = doc.to_markdown();
        let mut plain_text = doc.plain_text();

        // Guard against text-budget truncation that surfaced in-band without a
        // DamageKind::Truncated entry.
        if contains_upstream_limit_marker(&markdown) || contains_upstream_limit_marker(&plain_text)
        {
            return Err(ExtractError::LimitExceeded(
                "document expanded past text budget".to_owned(),
            ));
        }

        // Valid text-free documents (blank spreadsheets, image-only slides,
        // empty docs) produce zero content elements in the IR even when
        // Markdown emits a bare slide/sheet heading.
        if ir.sections.iter().all(|s| s.elements.is_empty()) {
            markdown.clear();
            plain_text.clear();
        }

        Ok(BackendOutcome::Extracted(RawExtraction {
            format,
            title,
            markdown,
            plain_text,
            warnings,
        }))
    }
}

fn to_oxide_format(format: DocumentFormat) -> OxideFormat {
    match format {
        DocumentFormat::Docx => OxideFormat::Docx,
        DocumentFormat::Xlsx | DocumentFormat::Xlsb => OxideFormat::Xlsx,
        DocumentFormat::Pptx => OxideFormat::Pptx,
        DocumentFormat::Doc => OxideFormat::Doc,
        DocumentFormat::Xls => OxideFormat::Xls,
        DocumentFormat::Ppt => OxideFormat::Ppt,
    }
}

fn build_parse_options(limits: &ExtractLimits) -> ParseOptions {
    let mut options = ParseOptions::default();
    options.limits.package_entries = limits.max_package_entries;
    options.limits.package_bytes = limits.max_uncompressed_bytes;
    options.limits.part_bytes = limits.max_part_bytes;
    options.limits.inflate_ratio = limits.max_inflate_ratio;
    options.limits.embedded_depth = 1;
    options.limits.embedded_packages = (limits.max_package_entries / 500).clamp(1, 16);
    options.limits.text_chars = limits.max_expanded_text_chars;
    options.limits.memory_bytes = limits.max_memory_bytes;
    options.include_hidden_text = false;
    options.include_deleted_text = false;
    options.on_incomplete = OnIncomplete::Report;
    options.lazy_pictures = true;
    options
}

fn map_office_error(err: OfficeError) -> Result<BackendOutcome, ExtractError> {
    match err {
        OfficeError::Doc(DocError::Encrypted)
        | OfficeError::Xls(XlsError::Encrypted | XlsError::EncryptionNotSupported(_))
        | OfficeError::Ppt(PptError::Encrypted) => Ok(BackendOutcome::Encrypted),

        OfficeError::UnsupportedFormat(msg)
            if msg.to_ascii_lowercase().contains("encrypted")
                || msg.to_ascii_lowercase().contains("password") =>
        {
            Ok(BackendOutcome::Encrypted)
        }

        OfficeError::Core(CoreError::DecompressionLimit { .. } | CoreError::PackageLimit(_))
        | OfficeError::Docx(DocxError::Core(
            CoreError::DecompressionLimit { .. } | CoreError::PackageLimit(_),
        ))
        | OfficeError::Xlsx(XlsxError::Core(
            CoreError::DecompressionLimit { .. } | CoreError::PackageLimit(_),
        ))
        | OfficeError::Pptx(PptxError::Core(
            CoreError::DecompressionLimit { .. } | CoreError::PackageLimit(_),
        )) => Err(ExtractError::LimitExceeded(
            sanitize_warning(&err.to_string())
                .unwrap_or_else(|| "container limit exceeded".to_owned()),
        )),

        OfficeError::Panic(msg) => Err(ExtractError::Internal(
            sanitize_warning(&msg).unwrap_or_else(|| "parser panicked".to_owned()),
        )),

        OfficeError::Incomplete(msg) => {
            let clean = sanitize_warning(&msg).unwrap_or_else(|| "incomplete document".to_owned());
            if is_limit_message(&msg) {
                Err(ExtractError::LimitExceeded(clean))
            } else {
                Err(ExtractError::Invalid(clean))
            }
        }

        other => {
            let raw = other.to_string();
            let clean = sanitize_warning(&raw).unwrap_or_else(|| "unreadable document".to_owned());
            if is_limit_message(&raw) {
                Err(ExtractError::LimitExceeded(clean))
            } else {
                Err(ExtractError::Invalid(clean))
            }
        }
    }
}

fn is_limit_message(detail: &str) -> bool {
    let lower = detail.to_ascii_lowercase();
    lower.contains("decompression limit")
        || lower.contains("package limit")
        || lower.contains("part size limit")
        || lower.contains("exceeds limit")
        || lower.contains("exceeds the")
        || lower.contains("character budget")
        || lower.contains("text budget")
        || lower.contains("text limit")
        || lower.contains("memory limit")
        || lower.contains("memory budget")
        || lower.contains("inflate ratio")
        || lower.contains("output truncated")
        || lower.contains("text is incomplete")
        || lower.contains("set_max_")
}

fn contains_upstream_limit_marker(text: &str) -> bool {
    text.contains("office_oxide::limits::") || text.contains("[output truncated:")
}
