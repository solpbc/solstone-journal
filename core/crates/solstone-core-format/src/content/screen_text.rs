// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Producer contract for the `screen_text` record family.
//!
//! - **Reserved path**: Matches `*/*/*/talents/screen_text.md` exclusively.
//! - **Heading grammar**: Headings anchored at column 0 define timed groups.
//! - **Literal values**: Producers append every recognized value through
//!   `append_screen_text_line`, including values containing newlines. Only code
//!   emits the unquoted time/category headings; OCR cannot open a heading, HTML
//!   element, link, image or code fence.
//! - **Preamble**: Text before the first heading is whatever bytes the file contains,
//!   retaining byte fidelity with no synthesized provenance.
//! - **Chunks**: Derived on UTF-8 char boundaries (<= 65536 bytes) carrying the group offset.
//! - **Fetch**: Returns exact file bytes without lossy conversions or Markdown chunk fallbacks.
//!   A display for the owner may name each heading's clock time (`screen_text_with_clock`).

use std::borrow::Cow;

use unicode_normalization::UnicodeNormalization;

use super::{IndexChunk, ProducedChunks};

pub const SCREEN_TEXT_CHUNK_LIMIT: usize = 65536;

/// Append untrusted recognized text as literal Markdown list content.
pub fn append_screen_text_line(record: &mut String, text: &str) {
    for line in text.lines() {
        record.push_str("- ");
        record.push_str(&crate::markdown::literal_text(line));
        record.push('\n');
    }
}

/// Best-effort masking of credential shapes in recognized or captured screen
/// text, one published line per non-empty input line. Every producer of this
/// family masks through here: a password prompt's value (and, for a bare prompt,
/// the line after it, carried in `redact_following`), secret URL parameters and
/// long high-entropy tokens. Nothing here claims complete redaction.
pub fn safe_lines_with_state(text: &str, redact_following: &mut bool) -> Vec<String> {
    let normalized = text.nfc().collect::<String>();
    let mut result = Vec::new();
    for line in normalized.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        if *redact_following {
            result.push("[REDACTED]".to_owned());
            *redact_following = mask_password_line(trimmed).1;
            continue;
        }
        let (line, next_secret) = mask_password_line(trimmed);
        *redact_following = next_secret;
        let line = mask_secret_url_parameters(&line);
        let line = mask_high_entropy_tokens(&line);
        let line = line.trim().to_owned();
        if line.chars().any(char::is_alphanumeric) {
            result.push(line);
        }
    }
    result
}

fn mask_password_line(line: &str) -> (String, bool) {
    let lower = line.to_ascii_lowercase();
    let Some(position) = ["password", "passphrase", "passwd"]
        .iter()
        .filter_map(|needle| lower.find(needle))
        .min()
    else {
        return (line.to_owned(), false);
    };
    let suffix = &line[position..];
    if let Some(separator) = suffix.find([':', '=']) {
        let value_start = position + separator + 1;
        if line[value_start..].trim().is_empty() {
            return ("[REDACTED]".to_owned(), true);
        }
        let mut masked = line[..value_start].to_owned();
        masked.push_str("[REDACTED]");
        return (masked, false);
    }
    (line.to_owned(), false)
}

fn mask_secret_url_parameters(line: &str) -> String {
    let mut output = String::with_capacity(line.len());
    let mut cursor = 0;
    while cursor < line.len() {
        let rest = &line[cursor..];
        let start = ["https://", "http://"]
            .iter()
            .filter_map(|scheme| rest.find(scheme).map(|offset| cursor + offset))
            .min();
        let Some(start) = start else {
            output.push_str(rest);
            break;
        };
        output.push_str(&line[cursor..start]);
        let end = line[start..]
            .find(char::is_whitespace)
            .map_or(line.len(), |offset| start + offset);
        let (url, suffix) = trim_url_suffix(&line[start..end]);
        output.push_str(&mask_url_query(url));
        output.push_str(suffix);
        cursor = end;
    }
    output
}

fn trim_url_suffix(url: &str) -> (&str, &str) {
    let end = url
        .char_indices()
        .rev()
        .find(|(_, ch)| !matches!(ch, '.' | ',' | ';' | ':' | '!' | '?' | ')' | ']' | '}'))
        .map_or(0, |(index, ch)| index + ch.len_utf8());
    (&url[..end], &url[end..])
}

fn mask_url_query(url: &str) -> String {
    let Some(query_start) = url.find('?') else {
        return url.to_owned();
    };
    let query_end = url[query_start + 1..]
        .find('#')
        .map_or(url.len(), |offset| query_start + 1 + offset);
    let mut output = url[..query_start + 1].to_owned();
    for (index, pair) in url[query_start + 1..query_end].split('&').enumerate() {
        if index > 0 {
            output.push('&');
        }
        let (key, value) = pair.split_once('=').unwrap_or((pair, ""));
        output.push_str(key);
        if pair.contains('=') {
            output.push('=');
            if is_secret_parameter(key) && !value.is_empty() {
                output.push_str("[REDACTED]");
            } else {
                output.push_str(value);
            }
        }
    }
    output.push_str(&url[query_end..]);
    output
}

fn is_secret_parameter(key: &str) -> bool {
    let key = key.to_ascii_lowercase().replace(['-', '.'], "_");
    [
        "token",
        "access_token",
        "refresh_token",
        "auth",
        "authorization",
        "secret",
        "password",
        "passwd",
        "api_key",
        "apikey",
        "credential",
        "session",
        "signature",
        "sig",
    ]
    .iter()
    .any(|secret| key == *secret || key.ends_with(&format!("_{secret}")))
}

fn mask_high_entropy_tokens(line: &str) -> String {
    let mut output = String::with_capacity(line.len());
    for token in line.split_inclusive(char::is_whitespace) {
        let trimmed = token.trim_end_matches(char::is_whitespace);
        let whitespace = &token[trimmed.len()..];
        let (leading, candidate, trailing) = secret_token_parts(trimmed);
        if looks_like_secret(candidate) {
            output.push_str(leading);
            output.push_str("[REDACTED]");
            output.push_str(trailing);
        } else if let Some((key, value)) = candidate.split_once('=')
            && !key.contains("://")
            && looks_like_secret(value)
        {
            // An assignment, as a shell shows `export API_KEY=…`.
            output.push_str(leading);
            output.push_str(key);
            output.push_str("=[REDACTED]");
            output.push_str(trailing);
        } else {
            output.push_str(trimmed);
        }
        output.push_str(whitespace);
    }
    output
}

fn secret_token_parts(token: &str) -> (&str, &str, &str) {
    let Some(start) = token.find(|ch: char| ch.is_ascii_alphanumeric() || ch == '_' || ch == '-')
    else {
        return (token, "", "");
    };
    let end = token
        .rfind(|ch: char| ch.is_ascii_alphanumeric() || ch == '_' || ch == '-')
        .map_or(start, |index| index + 1);
    (&token[..start], &token[start..end], &token[end..])
}

fn looks_like_secret(token: &str) -> bool {
    if token.len() < 32 || token.starts_with("http") {
        return false;
    }
    let mut counts = [0_u32; 256];
    for byte in token.bytes() {
        if !byte.is_ascii_alphanumeric() && byte != b'_' && byte != b'-' {
            return false;
        }
        counts[usize::from(byte)] += 1;
    }
    let length = token.len() as f64;
    let entropy = counts
        .iter()
        .filter(|count| **count > 0)
        .map(|count| {
            let probability = f64::from(*count) / length;
            -probability * probability.log2()
        })
        .sum::<f64>();
    entropy >= 4.0
}

/// Parse a column 0 heading defining a group offset.
pub fn parse_heading_offset(line: &str) -> Option<i64> {
    let bytes = line.as_bytes();
    if bytes.is_empty() || bytes[0] != b'#' {
        return None;
    }
    let mut i = 0;
    while i < bytes.len() && bytes[i] == b'#' {
        i += 1;
    }
    if !(1..=6).contains(&i) {
        return None;
    }
    let rest = &line[i..];
    let after_hashes = rest.trim_start_matches([' ', '\t']);
    if after_hashes.len() == rest.len() {
        return None;
    }
    let after_clock = after_hashes;
    let after_clock_bytes = after_clock.as_bytes();
    let clock_len = if after_clock_bytes.len() >= 8
        && after_clock_bytes.get(2) == Some(&b':')
        && after_clock_bytes.get(5) == Some(&b':')
    {
        8
    } else if after_clock_bytes.len() >= 5 && after_clock_bytes.get(2) == Some(&b':') {
        5
    } else {
        return None;
    };
    let clock_bytes = after_clock_bytes.get(..clock_len)?;
    if !clock_bytes.iter().all(|&b| b.is_ascii_digit() || b == b':') {
        return None;
    }
    let clock_str = &after_clock[..clock_len];
    let offset_ms = parse_clock(clock_str)?;
    let remaining = &after_clock[clock_len..];
    let after_clock_ws = remaining.trim_start_matches([' ', '\t']);
    if after_clock_ws.len() == remaining.len() {
        return None;
    }
    if !after_clock_ws.starts_with('·') {
        return None;
    }
    let after_dot = &after_clock_ws['·'.len_utf8()..];
    let title = after_dot.trim_start_matches([' ', '\t']);
    if title.len() == after_dot.len() {
        return None;
    }
    if title.trim().is_empty() {
        return None;
    }
    Some(offset_ms)
}

fn parse_clock(clock: &str) -> Option<i64> {
    let parts = clock.split(':').collect::<Vec<_>>();
    match parts.as_slice() {
        [mm, ss] => {
            if mm.len() != 2 || ss.len() != 2 {
                return None;
            }
            if !mm.chars().all(|c| c.is_ascii_digit()) || !ss.chars().all(|c| c.is_ascii_digit()) {
                return None;
            }
            let mm = mm.parse::<i64>().ok()?;
            let ss = ss.parse::<i64>().ok()?;
            if (0..=59).contains(&mm) && (0..=59).contains(&ss) {
                Some((mm * 60 + ss) * 1000)
            } else {
                None
            }
        }
        [hh, mm, ss] => {
            if hh.len() != 2 || mm.len() != 2 || ss.len() != 2 {
                return None;
            }
            if !hh.chars().all(|c| c.is_ascii_digit())
                || !mm.chars().all(|c| c.is_ascii_digit())
                || !ss.chars().all(|c| c.is_ascii_digit())
            {
                return None;
            }
            let hh = hh.parse::<i64>().ok()?;
            let mm = mm.parse::<i64>().ok()?;
            let ss = ss.parse::<i64>().ok()?;
            if (0..=99).contains(&hh) && (0..=59).contains(&mm) && (0..=59).contains(&ss) {
                Some((hh * 3600 + mm * 60 + ss) * 1000)
            } else {
                None
            }
        }
        _ => None,
    }
}

/// Format an offset in milliseconds into a minute clock string.
///
/// `MM:SS` when hours == 0, otherwise `HH:MM:SS` with hours padded to at least 2 digits.
/// Returns an empty string for negative offsets.
pub fn screen_text_minute(offset_ms: i64) -> String {
    if offset_ms < 0 {
        return String::new();
    }
    let total_seconds = offset_ms / 1000;
    let hours = total_seconds / 3600;
    let minutes = (total_seconds % 3600) / 60;
    let seconds = total_seconds % 60;
    if hours == 0 {
        format!("{minutes:02}:{seconds:02}")
    } else {
        format!("{hours:02}:{minutes:02}:{seconds:02}")
    }
}

/// The clock time a screen-text group was seen: its segment's start, read from
/// the segment key (`HHMMSS_LEN`), plus the group's offset, as `HH:MM:SS` on the
/// recording device's own clock, the clock the segment's times are on. `None`
/// when the key carries no start or the offset is negative.
///
/// The stored text keeps offsets, which [`parse_heading_offset`] and citations
/// read. This is for a reader: a screenshot's group is always at offset zero,
/// so its offset alone reads as midnight.
pub fn screen_text_clock(segment: &str, offset_ms: i64) -> Option<String> {
    if offset_ms < 0 {
        return None;
    }
    let start = segment.get(..6)?;
    if !start.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    let hours = start[..2].parse::<i64>().ok()?;
    let minutes = start[2..4].parse::<i64>().ok()?;
    let seconds = start[4..6].parse::<i64>().ok()?;
    if hours > 23 || minutes > 59 || seconds > 59 {
        return None;
    }
    let total = (hours * 3600 + minutes * 60 + seconds + offset_ms / 1000) % 86_400;
    Some(format!(
        "{:02}:{:02}:{:02}",
        total / 3600,
        (total % 3600) / 60,
        total % 60
    ))
}

/// Screen text for a reader: every group heading's offset rewritten as the
/// clock time it was seen ([`screen_text_clock`]). Everything else is kept as
/// it is, escapes included.
pub fn screen_text_with_clock<'a>(text: &'a str, segment: &str) -> Cow<'a, str> {
    let mut changed = false;
    let mut out = String::with_capacity(text.len());
    for line in text.split_inclusive('\n') {
        let body = line.trim_end_matches(['\n', '\r']);
        let clock =
            parse_heading_offset(body).and_then(|offset| screen_text_clock(segment, offset));
        let Some(clock) = clock else {
            out.push_str(line);
            continue;
        };
        let hashes = body.bytes().take_while(|&byte| byte == b'#').count();
        let after_hashes = &body[hashes..];
        let spaces = after_hashes.len() - after_hashes.trim_start_matches([' ', '\t']).len();
        let start = hashes + spaces;
        let rest = &body[start..];
        let clock_len = if rest.as_bytes().get(5) == Some(&b':') {
            8
        } else {
            5
        };
        out.push_str(&body[..start]);
        out.push_str(&clock);
        out.push_str(&line[start + clock_len..]);
        changed = true;
    }
    if changed {
        Cow::Owned(out)
    } else {
        Cow::Borrowed(text)
    }
}

/// Extract segment component (index 2) from a day-rooted path `YYYYMMDD/<stream>/<segment>/...`.
pub fn screen_text_segment(rel: &str) -> Option<&str> {
    let normalized = rel.strip_prefix("chronicle/").unwrap_or(rel);
    let mut parts = normalized.split('/');
    let _day = parts.next()?;
    let _stream = parts.next()?;
    parts.next()
}

/// Determine whether a path resolves strictly to `Family::ScreenText`.
pub fn is_screen_text_path(rel: &str) -> bool {
    let normalized = rel.strip_prefix("chronicle/").unwrap_or(rel);
    super::resolve_spec(normalized).is_some_and(|spec| spec.family == super::Family::ScreenText)
}

/// Produce chunks from screen text input.
pub fn produce_screen_text_chunks(text: &str) -> ProducedChunks {
    if text.is_empty() {
        return ProducedChunks {
            chunks: Vec::new(),
            agent_override: None,
            header: None,
            error: None,
            warnings: Vec::new(),
        };
    }

    // Find group boundaries and their offsets
    let mut groups: Vec<(usize, usize, Option<i64>)> = Vec::new();
    let mut current_offset: Option<i64> = None;
    let mut group_start = 0;

    let mut line_start = 0;
    for line in text.split_inclusive('\n') {
        let line_without_newline = line
            .strip_suffix("\r\n")
            .or_else(|| line.strip_suffix('\n'))
            .unwrap_or(line);
        if let Some(offset) = parse_heading_offset(line_without_newline) {
            if line_start > group_start {
                groups.push((group_start, line_start, current_offset));
                group_start = line_start;
            }
            current_offset = Some(offset);
        }
        line_start += line.len();
    }
    if text.len() > group_start {
        groups.push((group_start, text.len(), current_offset));
    }

    let mut chunks = Vec::new();
    for (start, end, offset_ms) in groups {
        let group_text = &text[start..end];
        if group_text.is_empty() {
            continue;
        }
        let mut chunk_start = 0;
        while chunk_start < group_text.len() {
            let mut chunk_end = (chunk_start + SCREEN_TEXT_CHUNK_LIMIT).min(group_text.len());
            while !group_text.is_char_boundary(chunk_end) {
                chunk_end -= 1;
            }
            if chunk_end < group_text.len() {
                let prefix = &group_text[chunk_start..chunk_end];
                let break_after = prefix.rfind('\n').map(|index| index + 1).or_else(|| {
                    prefix
                        .char_indices()
                        .rev()
                        .find(|(_, ch)| ch.is_whitespace())
                        .map(|(index, ch)| index + ch.len_utf8())
                });
                if let Some(break_after) = break_after.filter(|index| *index > 0) {
                    chunk_end = chunk_start + break_after;
                }
            }
            let chunk_content = group_text[chunk_start..chunk_end].to_string();
            chunks.push(IndexChunk {
                content: chunk_content,
                occurrence_time_ms: None,
                source: None,
                group_offset_ms: offset_ms,
            });
            chunk_start = chunk_end;
        }
    }

    ProducedChunks {
        chunks,
        agent_override: None,
        header: None,
        error: None,
        warnings: Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::content::{
        AdmittedCategory, ContentResolution, Family, IndexDisposition, ScopeBasis, produce_chunks,
        resolve_content_shape, resolve_spec,
    };
    use crate::test_support::reserve_temp_path;

    #[test]
    fn an_assigned_secret_is_masked_and_its_name_kept() {
        let secret = "q8XzT3vLm9Kp2RwN7sYbF4cHdJ6aUeG1oVi5";
        assert_eq!(
            safe_lines_with_state(&format!("$ export API_KEY={secret}"), &mut false),
            vec!["$ export API_KEY=[REDACTED]".to_owned()]
        );
        assert_eq!(
            safe_lines_with_state("$ export EDITOR=vim", &mut false),
            vec!["$ export EDITOR=vim".to_owned()]
        );
    }
    use std::fs;
    use std::path::PathBuf;

    struct TempDir {
        path: PathBuf,
    }

    impl TempDir {
        fn new(name: &str) -> Self {
            let path = reserve_temp_path(&format!("solstone-core-format-screen-text-{name}"));
            fs::create_dir_all(&path).expect("create temporary directory");
            Self { path }
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.path);
        }
    }

    #[test]
    fn screen_text_pattern_beats_talents_markdown() {
        let spec = resolve_spec("20260107/default/123456_300/talents/screen_text.md").unwrap();
        assert_eq!(spec.family, Family::ScreenText);
        assert_eq!(
            spec.disposition,
            IndexDisposition::Admitted {
                category: AdmittedCategory::ScreenText,
                basis: ScopeBasis::SegmentAssigned,
            }
        );

        let sibling_spec = resolve_spec("20260107/default/123456_300/talents/audio.md").unwrap();
        assert_eq!(sibling_spec.family, Family::Markdown);
        assert_eq!(
            sibling_spec.disposition,
            IndexDisposition::Admitted {
                category: AdmittedCategory::Transcripts,
                basis: ScopeBasis::SegmentAssigned,
            }
        );
    }

    #[test]
    fn sibling_shape_json_does_not_retarget_screen_text_and_retargets_ordinary_sibling() {
        let dir = TempDir::new("sibling-shape");
        let shape_json = dir.path.join("shape.json");
        fs::write(
            &shape_json,
            r#"{"screen_text.md": "Markdown", "custom.md": "Event"}"#,
        )
        .unwrap();

        let screen_text_path = dir.path.join("screen_text.md");
        fs::write(&screen_text_path, "## 00:01 · title\ncontent\n").unwrap();
        let custom_path = dir.path.join("custom.md");
        fs::write(&custom_path, "# Event title\n").unwrap();

        let resolution = resolve_content_shape(
            &screen_text_path,
            "20260107/default/123456_300/talents/screen_text.md",
        );
        assert_eq!(resolution, ContentResolution::Indexed(Family::ScreenText));

        let custom_resolution = resolve_content_shape(
            &custom_path,
            "20260107/default/123456_300/talents/custom.md",
        );
        assert_eq!(custom_resolution, ContentResolution::Indexed(Family::Event));
    }

    #[test]
    fn screen_text_heading_grammar_and_offsets() {
        assert_eq!(parse_heading_offset("## 00:12 · terminal"), Some(12_000));
        assert_eq!(
            parse_heading_offset("### 01:02:03 · window title"),
            Some(3_723_000)
        );
        assert_eq!(parse_heading_offset("- ## 00:12 · terminal"), None);
        assert_eq!(parse_heading_offset("> ## 00:12 · terminal"), None);
        assert_eq!(parse_heading_offset("## 00:12 - terminal"), None);
        assert_eq!(parse_heading_offset("## 1:02 · title"), None);
        assert_eq!(parse_heading_offset("## 00:12 · "), None);
        assert_eq!(parse_heading_offset("## 00:12 ·   \t"), None);
        assert_eq!(
            parse_heading_offset("clock in paragraph ## 00:12 · title"),
            None
        );
        assert_eq!(parse_heading_offset("## é:あ · terminal"), None);
    }

    #[test]
    fn screen_text_chunking_and_limits() {
        let preamble = "Pre-heading text\nmore preamble\n";
        let group1_heading = "## 00:12 · first\n";
        let long_line = format!("{}\n", "a".repeat(3000));
        let unicode_line = "Japanese text: こんにちは世界 · 画面テキスト\n";
        let input = format!("{preamble}{group1_heading}{long_line}{unicode_line}");

        let produced = produce_chunks(
            Family::ScreenText,
            "20260107/default/123456_300/talents/screen_text.md",
            &input,
        );
        assert_eq!(produced.chunks.len(), 2);
        assert_eq!(produced.chunks[0].content, preamble);
        assert_eq!(produced.chunks[0].group_offset_ms, None);
        assert_eq!(
            produced.chunks[1].content,
            format!("{group1_heading}{long_line}{unicode_line}")
        );
        assert_eq!(produced.chunks[1].group_offset_ms, Some(12_000));

        let concat = produced
            .chunks
            .iter()
            .map(|c| c.content.as_str())
            .collect::<Vec<_>>()
            .join("");
        assert_eq!(concat, input);

        // Huge group over 65536 bytes
        let huge_body = "x".repeat(100_000);
        let huge_input = format!("## 00:40 · huge\n{huge_body}");
        let huge_produced = produce_chunks(
            Family::ScreenText,
            "20260107/default/123456_300/talents/screen_text.md",
            &huge_input,
        );
        assert!(huge_produced.chunks.len() > 1);
        for chunk in &huge_produced.chunks {
            assert!(chunk.content.len() <= SCREEN_TEXT_CHUNK_LIMIT);
            assert_eq!(chunk.group_offset_ms, Some(40_000));
        }
        let huge_concat = huge_produced
            .chunks
            .iter()
            .map(|c| c.content.as_str())
            .collect::<Vec<_>>()
            .join("");
        assert_eq!(huge_concat, huge_input);
    }

    #[test]
    fn screen_text_helpers() {
        assert_eq!(screen_text_minute(0), "00:00");
        assert_eq!(
            screen_text_clock("100630_1", 0).as_deref(),
            Some("10:06:30")
        );
        assert_eq!(
            screen_text_clock("235950_300", 15_000).as_deref(),
            Some("00:00:05")
        );
        assert_eq!(
            screen_text_clock("093000_300", 72_500).as_deref(),
            Some("09:31:12")
        );
        assert_eq!(screen_text_clock("notakey", 0), None);
        assert_eq!(screen_text_clock("100630_1", -1), None);
        let text = "preamble\n## 00:00 · reading\n- 10\\:25 &lt; Display\n### 01:02:03 · window title\n- ## 00:12 · not a heading\n";
        assert_eq!(
            screen_text_with_clock(text, "100630_1"),
            "preamble\n## 10:06:30 · reading\n- 10\\:25 &lt; Display\n### 11:08:33 · window title\n- ## 00:12 · not a heading\n"
        );
        assert!(matches!(
            screen_text_with_clock("no headings", "100630_1"),
            Cow::Borrowed(_)
        ));
        assert!(matches!(
            screen_text_with_clock(text, "notakey"),
            Cow::Borrowed(_)
        ));
        assert_eq!(screen_text_minute(12_000), "00:12");
        assert_eq!(screen_text_minute(3_723_000), "01:02:03");
        assert_eq!(screen_text_minute(-1), "");

        assert_eq!(
            screen_text_segment("20260107/default/123456_300/talents/screen_text.md"),
            Some("123456_300")
        );
        assert_eq!(
            screen_text_segment("chronicle/20260107/default/123456_300/talents/screen_text.md"),
            Some("123456_300")
        );

        assert!(is_screen_text_path(
            "20260107/default/123456_300/talents/screen_text.md"
        ));
        assert!(is_screen_text_path(
            "chronicle/20260107/default/123456_300/talents/screen_text.md"
        ));
        assert!(!is_screen_text_path(
            "20260107/default/123456_300/talents/audio.md"
        ));
    }

    #[test]
    fn literal_line_escaping_keeps_the_existing_list_format() {
        let mut record = String::new();
        append_screen_text_line(&mut record, "a & <b> ![x](u)\n\n日本語\\");
        assert_eq!(
            record,
            "- a &amp; &lt;b&gt; \\!\\[x\\]\\(u\\)\n- \n- 日本語\\\\\n"
        );
    }

    #[test]
    fn recognized_markdown_cannot_forge_a_group_or_active_markup() {
        let mut record = "## 00:12 · terminal\n".to_owned();
        append_screen_text_line(
            &mut record,
            "```\n~~~\n~~paid~~\n## 59:59 · forged\n<img src=x> ![image](https://example.test/image)\n日本語",
        );
        let chunks = produce_screen_text_chunks(&record).chunks;
        assert_eq!(chunks.len(), 1);
        assert_eq!(chunks[0].group_offset_ms, Some(12_000));
        assert_eq!(chunks[0].content, record);
        assert!(!record.contains("<img"));
        assert!(!record.contains("![image]"));
        assert!(!record.contains("~~~"));
        assert!(!record.contains("~~paid~~"));
        let mut numbered = String::new();
        append_screen_text_line(
            &mut numbered,
            "1. approved\nhttps://example.test/a?x=1\nowner@example.test",
        );
        assert!(numbered.contains(r"1\. approved"));
        assert!(numbered.contains(r"https\:\/\/example\.test\/a\?x\=1"));
        assert!(numbered.contains(r"owner\@example\.test"));
        assert!(
            !record
                .lines()
                .skip(1)
                .any(|line| parse_heading_offset(line).is_some())
        );
        assert!(record.contains("日本語"));
    }
}
