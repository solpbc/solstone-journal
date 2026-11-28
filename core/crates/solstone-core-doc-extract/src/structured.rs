// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Bounded pure-Rust extractors for general structured text documents:
//! `.csv`, `.tsv`, `.html`/`.htm`, and `.eml` (RFC 5322 saved messages).

use std::io::Cursor;

use mail_parser::{HeaderName, MessageParser, MimeHeaders, PartType};

use crate::html_render;
use crate::{BackendOutcome, DocumentFormat, ExtractError, ExtractLimits, RawExtraction};

const MAX_CSV_COLS: usize = 256;
const UNHINTED_HTML_SNIFF_WINDOW: usize = 8192;

fn max_csv_rows(limits: &ExtractLimits) -> usize {
    if limits.max_memory_bytes <= 24 * 1024 * 1024 {
        20_000
    } else {
        50_000
    }
}

pub(crate) fn extract_structured(
    bytes: &[u8],
    format: DocumentFormat,
    limits: &ExtractLimits,
) -> Result<BackendOutcome, ExtractError> {
    match format {
        DocumentFormat::Csv => extract_delimited(bytes, b',', DocumentFormat::Csv, limits),
        DocumentFormat::Tsv => extract_delimited(bytes, b'\t', DocumentFormat::Tsv, limits),
        DocumentFormat::Html => extract_html(bytes, limits),
        DocumentFormat::Eml => extract_eml(bytes, limits),
        _ => Err(ExtractError::Invalid(format!(
            "format '{format}' is not a structured text format"
        ))),
    }
}

/// Sniff whether an unhinted non-container buffer begins with HTML
/// (`<!DOCTYPE html` or `<html`), skipping bounded leading UTF-8 BOM, ASCII
/// whitespace, `<?xml ...?>` declarations, and `<!-- ... -->` comments.
pub(crate) fn sniff_unhinted_html(bytes: &[u8]) -> bool {
    let slice = &bytes[..bytes.len().min(UNHINTED_HTML_SNIFF_WINDOW)];
    let mut i = if slice.starts_with(&[0xEF, 0xBB, 0xBF]) {
        3
    } else {
        0
    };

    loop {
        while i < slice.len() && slice[i].is_ascii_whitespace() {
            i += 1;
        }
        if slice[i..].starts_with(b"<!--") {
            let Some(rel_end) = slice[i + 4..].windows(3).position(|w| w == b"-->") else {
                return false;
            };
            i = i + 4 + rel_end + 3;
            continue;
        }
        if slice[i..].starts_with(b"<?") {
            let Some(rel_end) = slice[i + 2..].windows(2).position(|w| w == b"?>") else {
                return false;
            };
            i = i + 2 + rel_end + 2;
            continue;
        }
        break;
    }

    let rest = &slice[i..];
    starts_with_ascii_ci(rest, b"<!doctype html")
        || (starts_with_ascii_ci(rest, b"<html")
            && rest
                .get(5)
                .is_some_and(|&b| b.is_ascii_whitespace() || matches!(b, b'>' | b'/')))
}

fn starts_with_ascii_ci(haystack: &[u8], prefix: &[u8]) -> bool {
    haystack.len() >= prefix.len() && haystack[..prefix.len()].eq_ignore_ascii_case(prefix)
}

fn extract_delimited(
    bytes: &[u8],
    delimiter: u8,
    format: DocumentFormat,
    limits: &ExtractLimits,
) -> Result<BackendOutcome, ExtractError> {
    let text = html_render::decode_html_text(bytes)?;
    let max_rows = max_csv_rows(limits);
    let stop_chars = limits.max_output_chars.saturating_add(4096);

    let mut reader = csv::ReaderBuilder::new()
        .delimiter(delimiter)
        .flexible(true)
        .has_headers(false)
        .from_reader(Cursor::new(text.as_bytes()));

    let mut md = String::new();
    let mut plain = String::new();
    let mut warnings: Vec<String> = Vec::new();
    let mut header_cols: usize = 0;
    let mut rows_emitted: usize = 0;
    let mut md_chars: usize = 0;
    let mut truncated_cols = false;
    let mut truncated_rows = false;
    let mut record = csv::StringRecord::new();

    loop {
        match reader.read_record(&mut record) {
            Ok(false) => break,
            Ok(true) => {}
            Err(err) => {
                return Err(ExtractError::Invalid(format!(
                    "malformed {format} record: {err}"
                )));
            }
        }

        let mut cells: Vec<String> = Vec::with_capacity(record.len().min(MAX_CSV_COLS));
        for (idx, field) in record.iter().enumerate() {
            if idx >= MAX_CSV_COLS {
                truncated_cols = true;
                break;
            }
            cells.push(sanitize_cell_text(field));
        }
        while cells.last().is_some_and(String::is_empty) {
            cells.pop();
        }
        if cells.is_empty() {
            continue;
        }

        if rows_emitted >= max_rows {
            truncated_rows = true;
            break;
        }
        if md_chars > stop_chars {
            break;
        }

        if rows_emitted == 0 {
            header_cols = cells.len();
            let row_md = format_gfm_row(&cells, header_cols);
            let sep_md = format_gfm_separator(header_cols);
            md_chars = md_chars
                .saturating_add(row_md.chars().count())
                .saturating_add(sep_md.chars().count());
            md.push_str(&row_md);
            md.push('\n');
            md.push_str(&sep_md);
            md.push('\n');
            plain.push_str(&cells.join("\t"));
            plain.push('\n');
        } else {
            let width = header_cols.max(cells.len()).min(MAX_CSV_COLS);
            let row_md = format_gfm_row(&cells, width);
            md_chars = md_chars.saturating_add(row_md.chars().count());
            md.push_str(&row_md);
            md.push('\n');
            plain.push_str(&cells.join("\t"));
            plain.push('\n');
        }
        rows_emitted += 1;
    }

    if truncated_cols {
        warnings.push(format!("{format} columns truncated to {MAX_CSV_COLS}"));
    }
    if truncated_rows {
        warnings.push(format!("{format} rows truncated at {max_rows} row limit"));
    }

    Ok(BackendOutcome::Extracted(RawExtraction {
        format,
        title: None,
        markdown: md.trim_end().to_owned(),
        plain_text: plain.trim_end().to_owned(),
        truncated: truncated_cols || truncated_rows,
        warnings,
    }))
}

fn format_gfm_row(cells: &[String], width: usize) -> String {
    let mut row = String::from("|");
    for col in 0..width {
        let val = cells.get(col).map(String::as_str).unwrap_or("");
        row.push(' ');
        row.push_str(&val.replace('|', "\\|"));
        row.push_str(" |");
    }
    row
}

fn format_gfm_separator(width: usize) -> String {
    let mut sep = String::from("|");
    for _ in 0..width {
        sep.push_str(" --- |");
    }
    sep
}

fn sanitize_cell_text(raw: &str) -> String {
    raw.chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

fn extract_html(bytes: &[u8], limits: &ExtractLimits) -> Result<BackendOutcome, ExtractError> {
    let rendered = html_render::render_html_bytes(bytes, limits)?;
    Ok(BackendOutcome::Extracted(RawExtraction {
        format: DocumentFormat::Html,
        title: rendered.title,
        markdown: rendered.markdown,
        plain_text: rendered.plain_text,
        truncated: false,
        warnings: Vec::new(),
    }))
}

fn extract_eml(bytes: &[u8], limits: &ExtractLimits) -> Result<BackendOutcome, ExtractError> {
    // Reject NUL bytes in the RFC 5322 header section (or entire short buffer)
    // so random binary payloads renamed `.eml` never parse as empty/bogus mail.
    let header_end = find_header_end(bytes).unwrap_or(bytes.len());
    if bytes[..header_end].contains(&0) {
        return Err(ExtractError::Invalid(
            "email message header contains binary NUL bytes".to_owned(),
        ));
    }

    let message = MessageParser::default().parse(bytes).ok_or_else(|| {
        ExtractError::Invalid("failed to parse RFC 5322 email message".to_owned())
    })?;

    let has_recognized_header = message.from().is_some()
        || message.to().is_some()
        || message.subject().is_some()
        || message.date().is_some()
        || message.received().is_some()
        || message.header(HeaderName::MessageId).is_some()
        || message.header(HeaderName::MimeVersion).is_some()
        || message.header(HeaderName::ContentType).is_some();

    if !has_recognized_header {
        return Err(ExtractError::Invalid(
            "input does not contain recognized RFC 5322 email headers".to_owned(),
        ));
    }

    let subject = message
        .subject()
        .map(sanitize_header_line)
        .filter(|s| !s.is_empty());
    let from = message
        .from()
        .map(format_address_list)
        .filter(|s| !s.is_empty());
    let to = message
        .to()
        .map(format_address_list)
        .filter(|s| !s.is_empty());
    let date = message
        .date()
        .map(|d| sanitize_header_line(&d.to_rfc3339()))
        .filter(|s| !s.is_empty());

    // Prefer non-empty text/plain body; fall back to text/html rendered via the
    // shared stack-safe HTML renderer when text/plain is absent or whitespace-only.
    let (body_md, body_plain) = if let Some(text) = first_non_empty_text_body(&message) {
        let normalized = normalize_email_body(&text);
        (normalized.clone(), normalized)
    } else if let Some(html) = first_non_empty_html_body(&message) {
        let rendered = html_render::render_html_str(&html, limits)?;
        (rendered.markdown, rendered.plain_text)
    } else {
        (String::new(), String::new())
    };

    // Header-only messages with no body text return Empty via common finalization.
    if body_md.trim().is_empty() && body_plain.trim().is_empty() {
        return Ok(BackendOutcome::Extracted(RawExtraction {
            format: DocumentFormat::Eml,
            title: subject,
            markdown: String::new(),
            plain_text: String::new(),
            truncated: false,
            warnings: Vec::new(),
        }));
    }

    // Collect metadata-only attachment filenames without recursing into nested
    // attachments.
    let mut attachment_names: Vec<String> = Vec::new();
    for part in message.attachments() {
        if let Some(name) = part.attachment_name() {
            let clean = sanitize_header_line(name);
            if !clean.is_empty() && attachment_names.len() < 64 {
                attachment_names.push(clean);
            }
        }
    }

    let mut md_headers = Vec::new();
    let mut plain_headers = Vec::new();
    if let Some(ref value) = from {
        md_headers.push(format!("**From:** {value}"));
        plain_headers.push(format!("From: {value}"));
    }
    if let Some(ref value) = to {
        md_headers.push(format!("**To:** {value}"));
        plain_headers.push(format!("To: {value}"));
    }
    if let Some(ref value) = date {
        md_headers.push(format!("**Date:** {value}"));
        plain_headers.push(format!("Date: {value}"));
    }
    if let Some(ref value) = subject {
        md_headers.push(format!("**Subject:** {value}"));
        plain_headers.push(format!("Subject: {value}"));
    }
    if !attachment_names.is_empty() {
        let joined = attachment_names.join(", ");
        md_headers.push(format!("**Attachments:** {joined}"));
        plain_headers.push(format!("Attachments: {joined}"));
    }

    let markdown = if md_headers.is_empty() {
        body_md
    } else {
        format!("{}\n\n---\n\n{}", md_headers.join("\n"), body_md)
    };
    let plain_text = if plain_headers.is_empty() {
        body_plain
    } else {
        format!("{}\n\n{}", plain_headers.join("\n"), body_plain)
    };

    Ok(BackendOutcome::Extracted(RawExtraction {
        format: DocumentFormat::Eml,
        title: subject,
        markdown,
        plain_text,
        truncated: false,
        warnings: Vec::new(),
    }))
}

fn first_non_empty_text_body(message: &mail_parser::Message<'_>) -> Option<String> {
    for part in message.text_bodies() {
        if let PartType::Text(text) = &part.body
            && !text.trim().is_empty()
        {
            return Some(text.as_ref().to_owned());
        }
    }
    None
}

fn first_non_empty_html_body(message: &mail_parser::Message<'_>) -> Option<String> {
    for part in message.html_bodies() {
        if let PartType::Html(html) = &part.body
            && !html.trim().is_empty()
        {
            return Some(html.as_ref().to_owned());
        }
    }
    None
}

fn find_header_end(bytes: &[u8]) -> Option<usize> {
    let crlf = bytes.windows(4).position(|w| w == b"\r\n\r\n");
    let lf = bytes.windows(2).position(|w| w == b"\n\n");
    match (crlf, lf) {
        (Some(a), Some(b)) => Some(a.min(b)),
        (Some(a), None) => Some(a),
        (None, Some(b)) => Some(b),
        (None, None) => None,
    }
}

fn format_address_list(addr: &mail_parser::Address<'_>) -> String {
    let mut parts = Vec::new();
    for entry in addr.iter() {
        let name = entry
            .name()
            .map(sanitize_header_line)
            .filter(|s| !s.is_empty());
        let email = entry
            .address()
            .map(sanitize_header_line)
            .filter(|s| !s.is_empty());
        match (name, email) {
            (Some(n), Some(e)) => parts.push(format!("{n} <{e}>")),
            (Some(n), None) => parts.push(n),
            (None, Some(e)) => parts.push(e),
            (None, None) => {}
        }
        if parts.len() >= 32 {
            break;
        }
    }
    parts.join(", ")
}

fn sanitize_header_line(raw: &str) -> String {
    raw.chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

fn normalize_email_body(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    let mut blank_run = 0usize;
    for line in raw.replace("\r\n", "\n").lines() {
        let trimmed_end = line.trim_end();
        if trimmed_end.trim().is_empty() {
            blank_run += 1;
            if blank_run == 1 && !out.is_empty() {
                out.push('\n');
            }
            continue;
        }
        blank_run = 0;
        out.push_str(trimmed_end);
        out.push('\n');
    }
    out.trim().to_owned()
}
