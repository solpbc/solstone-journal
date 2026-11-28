// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Stack-safe, bounded HTML-to-Markdown and plain-text rendering shared across
//! standalone `.html`/`.htm` files, EPUB XHTML chapters, and `.eml` HTML
//! bodies.
//!
//! Runs a linear, non-recursive HTML pre-filter before calling `html2text` to
//! enforce nesting-depth and tag-count limits (preventing stack overflow aborts
//! on deeply nested DOMs) and strip non-content subtrees (`<script>`,
//! `<style>`, `<noscript>`, `<template>`).

use std::io::Cursor;

use crate::{ExtractError, ExtractLimits};

/// Maximum open element nesting depth allowed before invoking `html2text`.
pub(crate) const MAX_HTML_NESTING_DEPTH: usize = 128;

/// Maximum HTML tag tokens allowed in a single rendered HTML document/chapter.
pub(crate) const MAX_HTML_TAGS: usize = 200_000;

/// Rendered HTML output before common crate-level finalization.
pub(crate) struct RenderedHtml {
    pub(crate) title: Option<String>,
    pub(crate) markdown: String,
    pub(crate) plain_text: String,
}

/// Decode raw HTML bytes (UTF-8, UTF-16 BOM, or clean Latin-1 fallback) and
/// render to bounded Markdown + plain text.
pub(crate) fn render_html_bytes(
    bytes: &[u8],
    limits: &ExtractLimits,
) -> Result<RenderedHtml, ExtractError> {
    if (bytes.len() as u64) > limits.max_part_bytes.max(limits.max_input_bytes) {
        return Err(ExtractError::LimitExceeded(format!(
            "HTML input size {} bytes exceeds limit",
            bytes.len()
        )));
    }
    let decoded = decode_html_text(bytes)?;
    render_html_str(&decoded, limits)
}

/// Render an HTML string to bounded Markdown + plain text with stack-safety
/// pre-filtering and `TooNarrow` fallback.
pub(crate) fn render_html_str(
    html: &str,
    limits: &ExtractLimits,
) -> Result<RenderedHtml, ExtractError> {
    let max_bytes = limits.max_part_bytes.max(limits.max_input_bytes);
    if (html.len() as u64) > max_bytes {
        return Err(ExtractError::LimitExceeded(format!(
            "HTML input size {} bytes exceeds limit of {max_bytes} bytes",
            html.len()
        )));
    }
    let filtered = prefilter_html(html)?;
    let bytes = filtered.html.as_bytes();

    let markdown = render_with_fallback(bytes, true)?;
    let plain_text = render_with_fallback(bytes, false)?;

    let markdown = normalize_rendered_text(&markdown);
    let plain_text = normalize_rendered_text(&plain_text);

    if markdown.chars().count() > limits.max_expanded_text_chars
        || plain_text.chars().count() > limits.max_expanded_text_chars
    {
        return Err(ExtractError::LimitExceeded(format!(
            "rendered HTML exceeded expanded text limit of {} characters",
            limits.max_expanded_text_chars
        )));
    }

    Ok(RenderedHtml {
        title: filtered.title,
        markdown,
        plain_text,
    })
}

fn render_with_fallback(bytes: &[u8], decorate: bool) -> Result<String, ExtractError> {
    let primary = if decorate {
        html2text::config::plain()
            .allow_width_overflow()
            .string_from_read(Cursor::new(bytes), 100)
    } else {
        html2text::config::plain_no_decorate()
            .allow_width_overflow()
            .string_from_read(Cursor::new(bytes), 100)
    };
    if let Ok(text) = primary {
        return Ok(text);
    }

    let fallback = if decorate {
        html2text::config::plain()
            .raw_mode(true)
            .string_from_read(Cursor::new(bytes), 10_000)
    } else {
        html2text::config::plain_no_decorate()
            .raw_mode(true)
            .string_from_read(Cursor::new(bytes), 10_000)
    };
    fallback.map_err(|err| ExtractError::Invalid(format!("HTML rendering failed: {err}")))
}

struct FilteredHtml {
    title: Option<String>,
    html: String,
}

fn prefilter_html(input: &str) -> Result<FilteredHtml, ExtractError> {
    let bytes = input.as_bytes();
    let len = bytes.len();
    let mut out = String::with_capacity(len.min(1024 * 1024));
    let mut stack: Vec<String> = Vec::with_capacity(32);
    let mut tag_count: usize = 0;
    let mut skip_tag: Option<&'static str> = None;
    let mut in_title = false;
    let mut title_buf = String::new();
    let mut captured_title: Option<String> = None;

    let mut i = 0;
    while i < len {
        // While inside `<script>`/`<style>`/`<noscript>`/`<template>`, scan
        // raw text directly for the matching closing tag so JS/CSS strings,
        // quotes, or `<!--` sequences never swallow subsequent HTML body text.
        if let Some(active_skip) = skip_tag {
            if let Some(close_end) = find_closing_tag_end(bytes, i, active_skip.as_bytes()) {
                tag_count += 1;
                if tag_count > MAX_HTML_TAGS {
                    return Err(ExtractError::LimitExceeded(format!(
                        "HTML tag count exceeded limit of {MAX_HTML_TAGS}"
                    )));
                }
                skip_tag = None;
                i = close_end + 1;
                continue;
            }
            break;
        }

        if bytes[i] != b'<' {
            let start = i;
            while i < len && bytes[i] != b'<' {
                i += 1;
            }
            let chunk = &input[start..i];
            if in_title && captured_title.is_none() && title_buf.len() < 1024 {
                title_buf.push_str(chunk);
            }
            out.push_str(chunk);
            continue;
        }

        // Comment: <!-- ... -->
        if bytes[i..].starts_with(b"<!--") {
            if let Some(rel_end) = find_subslice(&bytes[i + 4..], b"-->") {
                i = i + 4 + rel_end + 3;
            } else {
                break;
            }
            continue;
        }

        // CDATA: <![CDATA[ ... ]]>
        if bytes[i..].starts_with(b"<![CDATA[") {
            if let Some(rel_end) = find_subslice(&bytes[i + 9..], b"]]>") {
                let text = &input[i + 9..i + 9 + rel_end];
                push_escaped_html_text(&mut out, text);
                i = i + 9 + rel_end + 3;
            } else {
                break;
            }
            continue;
        }

        // Declaration or processing instruction: <! ... > or <? ... ?>
        if i + 1 < len && matches!(bytes[i + 1], b'!' | b'?') {
            if let Some(end) = find_tag_end(bytes, i + 2) {
                i = end + 1;
            } else {
                break;
            }
            continue;
        }

        // Parse tag name and closing `>`.
        let Some(tag_end) = find_tag_end(bytes, i + 1) else {
            // Unclosed `<` at end of document: treat as literal text.
            out.push_str(&input[i..]);
            break;
        };

        let raw_tag = &input[i + 1..tag_end];
        let trimmed = raw_tag.trim();
        if trimmed.is_empty() {
            out.push_str(&input[i..=tag_end]);
            i = tag_end + 1;
            continue;
        }

        let is_closing = trimmed.starts_with('/');
        let name_slice = if is_closing {
            trimmed[1..].trim_start()
        } else {
            trimmed
        };
        let name_len = name_slice
            .bytes()
            .take_while(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b':' | b'_'))
            .count();
        if name_len == 0 {
            // Not an HTML tag (e.g. `1 < 2` in prose).
            out.push_str(&input[i..=tag_end]);
            i = tag_end + 1;
            continue;
        }

        tag_count += 1;
        if tag_count > MAX_HTML_TAGS {
            return Err(ExtractError::LimitExceeded(format!(
                "HTML tag count exceeded limit of {MAX_HTML_TAGS}"
            )));
        }

        let tag_name = name_slice[..name_len].to_ascii_lowercase();
        let self_closing = !is_closing && trimmed.ends_with('/');

        if !is_closing {
            if let Some(skipped) = skipped_subtree_tag(&tag_name) {
                if !self_closing {
                    skip_tag = Some(skipped);
                }
                i = tag_end + 1;
                continue;
            }
            if tag_name == "title" && !self_closing {
                in_title = true;
            }
            if !self_closing && !is_void_html_tag(&tag_name) {
                auto_close_implicit_siblings(&mut stack, &tag_name);
                if stack.len() >= MAX_HTML_NESTING_DEPTH {
                    return Err(ExtractError::LimitExceeded(format!(
                        "HTML nesting depth exceeded limit of {MAX_HTML_NESTING_DEPTH}"
                    )));
                }
                stack.push(tag_name);
            }
        } else {
            if tag_name == "title" {
                in_title = false;
                if captured_title.is_none() {
                    let decoded = decode_basic_entities(&title_buf);
                    let clean = decoded.split_whitespace().collect::<Vec<_>>().join(" ");
                    if !clean.is_empty() {
                        captured_title = Some(clean);
                    }
                }
            }
            if let Some(pos) = stack.iter().rposition(|open| open == &tag_name) {
                stack.truncate(pos);
            }
        }

        out.push_str(&input[i..=tag_end]);
        i = tag_end + 1;
    }

    Ok(FilteredHtml {
        title: captured_title,
        html: out,
    })
}

fn skipped_subtree_tag(tag: &str) -> Option<&'static str> {
    match tag {
        "script" => Some("script"),
        "style" => Some("style"),
        "noscript" => Some("noscript"),
        "template" => Some("template"),
        _ => None,
    }
}

fn is_void_html_tag(tag: &str) -> bool {
    matches!(
        tag,
        "area"
            | "base"
            | "br"
            | "col"
            | "command"
            | "embed"
            | "hr"
            | "img"
            | "input"
            | "keygen"
            | "link"
            | "meta"
            | "param"
            | "source"
            | "track"
            | "wbr"
    )
}

/// Mirror HTML5 optional-closing-tag rules for common flat elements so real
/// documents with hundreds of unclosed `<p>` or `<li>` tags do not falsely
/// trip the deep-nesting stack-overflow guard.
fn auto_close_implicit_siblings(stack: &mut Vec<String>, incoming: &str) {
    let pop_if_top = |stack: &mut Vec<String>, matches_fn: fn(&str) -> bool| {
        if stack.last().is_some_and(|top| matches_fn(top)) {
            stack.pop();
        }
    };
    match incoming {
        "p" => pop_if_top(stack, |t| t == "p"),
        "li" => pop_if_top(stack, |t| t == "li"),
        "dt" | "dd" => pop_if_top(stack, |t| matches!(t, "dt" | "dd")),
        "option" => pop_if_top(stack, |t| t == "option"),
        "td" | "th" => pop_if_top(stack, |t| matches!(t, "td" | "th")),
        "tr" => {
            while stack
                .last()
                .is_some_and(|t| matches!(t.as_str(), "td" | "th" | "tr"))
            {
                let was_tr = stack.last().is_some_and(|t| t == "tr");
                stack.pop();
                if was_tr {
                    break;
                }
            }
        }
        _ => {}
    }
}

fn find_closing_tag_end(bytes: &[u8], start: usize, tag_lower: &[u8]) -> Option<usize> {
    let mut i = start;
    while i + 2 < bytes.len() {
        if bytes[i] == b'<' && bytes[i + 1] == b'/' {
            let mut j = i + 2;
            while j < bytes.len() && bytes[j].is_ascii_whitespace() {
                j += 1;
            }
            if j + tag_lower.len() <= bytes.len()
                && bytes[j..j + tag_lower.len()].eq_ignore_ascii_case(tag_lower)
            {
                let mut k = j + tag_lower.len();
                let next_ok = bytes
                    .get(k)
                    .is_some_and(|&b| b == b'>' || b.is_ascii_whitespace() || b == b'/');
                if next_ok {
                    while k < bytes.len() && bytes[k] != b'>' {
                        k += 1;
                    }
                    if k < bytes.len() {
                        return Some(k);
                    }
                }
            }
        }
        i += 1;
    }
    None
}

fn find_tag_end(bytes: &[u8], start: usize) -> Option<usize> {
    let mut quote: Option<u8> = None;
    let mut i = start;
    while i < bytes.len() {
        let b = bytes[i];
        match quote {
            Some(q) => {
                if b == q {
                    quote = None;
                }
            }
            None => match b {
                b'"' | b'\'' => quote = Some(b),
                b'>' => return Some(i),
                _ => {}
            },
        }
        i += 1;
    }
    None
}

fn find_subslice(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack.windows(needle.len()).position(|w| w == needle)
}

fn push_escaped_html_text(out: &mut String, text: &str) {
    for ch in text.chars() {
        match ch {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            _ => out.push(ch),
        }
    }
}

fn decode_basic_entities(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    let mut rest = raw;
    while let Some(amp) = rest.find('&') {
        out.push_str(&rest[..amp]);
        let after = &rest[amp + 1..];
        if let Some(semi) = after.find(';')
            && semi <= 12
        {
            let entity = &after[..semi];
            let decoded = match entity {
                "amp" => Some('&'),
                "lt" => Some('<'),
                "gt" => Some('>'),
                "quot" => Some('"'),
                "apos" => Some('\''),
                "nbsp" => Some(' '),
                _ if entity.starts_with("#x") || entity.starts_with("#X") => {
                    u32::from_str_radix(&entity[2..], 16)
                        .ok()
                        .and_then(char::from_u32)
                }
                _ if entity.starts_with('#') => {
                    entity[1..].parse::<u32>().ok().and_then(char::from_u32)
                }
                _ => None,
            };
            if let Some(ch) = decoded {
                out.push(ch);
                rest = &after[semi + 1..];
                continue;
            }
        }
        out.push('&');
        rest = after;
    }
    out.push_str(rest);
    out
}

fn normalize_rendered_text(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    let mut blank_run = 0usize;
    for line in raw.lines() {
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

pub(crate) fn decode_html_text(bytes: &[u8]) -> Result<String, ExtractError> {
    if let Some(utf8) = bytes.strip_prefix(&[0xEF, 0xBB, 0xBF]) {
        if utf8.contains(&0) {
            return Err(ExtractError::Invalid(
                "text document contains binary NUL bytes".to_owned(),
            ));
        }
        return Ok(String::from_utf8_lossy(utf8).into_owned());
    }
    if let Some(raw) = bytes.strip_prefix(&[0xFF, 0xFE]) {
        return decode_utf16(raw, true);
    }
    if let Some(raw) = bytes.strip_prefix(&[0xFE, 0xFF]) {
        return decode_utf16(raw, false);
    }
    if bytes.contains(&0) {
        return Err(ExtractError::Invalid(
            "text document contains binary NUL bytes".to_owned(),
        ));
    }
    match std::str::from_utf8(bytes) {
        Ok(s) => Ok(s.to_owned()),
        Err(_) => {
            // Legacy ISO-8859-1 / Windows-1252 fallback when non-binary.
            let control = bytes
                .iter()
                .filter(|&&b| b < 0x20 && !matches!(b, b'\t' | b'\n' | b'\r'))
                .count();
            if control.saturating_mul(50) > bytes.len().max(1) {
                return Err(ExtractError::Invalid(
                    "text document appears to be binary data".to_owned(),
                ));
            }
            Ok(bytes.iter().map(|&b| char::from(b)).collect())
        }
    }
}

pub(crate) fn decode_utf16(raw: &[u8], little_endian: bool) -> Result<String, ExtractError> {
    if !raw.len().is_multiple_of(2) {
        return Err(ExtractError::Invalid(
            "UTF-16 document has odd byte length".to_owned(),
        ));
    }
    let units: Vec<u16> = raw
        .chunks_exact(2)
        .map(|chunk| {
            if little_endian {
                u16::from_le_bytes([chunk[0], chunk[1]])
            } else {
                u16::from_be_bytes([chunk[0], chunk[1]])
            }
        })
        .collect();
    String::from_utf16(&units)
        .map_err(|_| ExtractError::Invalid("invalid UTF-16 sequence in document".to_owned()))
}
