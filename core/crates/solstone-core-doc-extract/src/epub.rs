// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Bounded pure-Rust EPUB 2 & 3 reader (`META-INF/container.xml` -> OPF
//! `<manifest>`/`<spine>` -> chapter XHTML/HTML rendered in reading order).
//!
//! Security invariants:
//! - Fail-closed DRM inspection on `META-INF/encryption.xml` (case-insensitive,
//!   path-normalized across all central-directory entries). Only allowlisted
//!   IDPF/Adobe font obfuscation targeting `.otf`/`.ttf`/`.woff`/`.woff2`
//!   passes through; malformed `encryption.xml` or any non-font encrypted
//!   resource returns `BackendOutcome::Encrypted`.
//! - Every ZIP member read uses `ZipBudget` (enforcing actual inflated bytes,
//!   never trusting central-directory declared sizes).
//! - Path percent-decoding and `#fragment`/`?query` stripping happen BEFORE
//!   path-traversal validation (`..`, `\`, leading `/`, NUL).
//! - Spine `idref`s and resolved chapter paths are deduplicated and capped to
//!   prevent spine amplification.

use std::collections::{HashMap, HashSet};
use std::io::Cursor;

use quick_xml::Reader;
use quick_xml::events::{BytesStart, Event};

use crate::html_render;
use crate::sniff::{
    ZipBudget, ZipReadOutcome, find_all_zip_entries_case_insensitive,
    find_zip_entry_case_insensitive, normalize_zip_path,
};
use crate::{BackendOutcome, DocumentFormat, ExtractError, ExtractLimits, RawExtraction};

const MAX_XML_DEPTH: usize = 256;

const ALLOWED_FONT_OBFUSCATION_ALGORITHMS: &[&str] = &[
    "http://www.idpf.org/2008/embedding",
    "http://ns.adobe.com/pdf/enc#RC",
];

const ALLOWED_FONT_EXTENSIONS: &[&str] = &[".otf", ".ttf", ".woff", ".woff2"];

pub(crate) fn extract_epub(
    bytes: &[u8],
    limits: &ExtractLimits,
) -> Result<BackendOutcome, ExtractError> {
    let mut archive = zip::ZipArchive::new(Cursor::new(bytes))
        .map_err(|err| ExtractError::Invalid(format!("invalid EPUB archive: {err}")))?;
    let mut budget = ZipBudget::new();

    // 1. Fail-closed DRM check across all case/separator variants of META-INF/encryption.xml.
    let encryption_entries =
        find_all_zip_entries_case_insensitive(&archive, "meta-inf/encryption.xml");
    for idx in encryption_entries {
        let xml_bytes = match budget.read_entry_bounded(&mut archive, idx, limits)? {
            ZipReadOutcome::Encrypted => return Ok(BackendOutcome::Encrypted),
            ZipReadOutcome::Bytes(data) => data,
        };
        if !is_font_obfuscation_only(&xml_bytes) {
            return Ok(BackendOutcome::Encrypted);
        }
    }

    // 2. Locate OPF package path via META-INF/container.xml.
    let container_idx = find_zip_entry_case_insensitive(&archive, "meta-inf/container.xml")
        .ok_or_else(|| {
            ExtractError::Invalid("EPUB is missing META-INF/container.xml".to_owned())
        })?;
    let container_bytes = match budget.read_entry_bounded(&mut archive, container_idx, limits)? {
        ZipReadOutcome::Encrypted => return Ok(BackendOutcome::Encrypted),
        ZipReadOutcome::Bytes(data) => data,
    };
    let raw_rootfile = parse_container_rootfile(&container_bytes)?;
    let opf_path = resolve_epub_path("", &raw_rootfile)?;
    let opf_dir = parent_dir(&opf_path);

    // 3. Read and parse OPF manifest, spine, and title.
    let opf_idx = find_zip_entry_case_insensitive(&archive, &opf_path.to_ascii_lowercase())
        .ok_or_else(|| {
            ExtractError::Invalid(format!(
                "EPUB OPF package '{opf_path}' not found in archive"
            ))
        })?;
    let opf_bytes = match budget.read_entry_bounded(&mut archive, opf_idx, limits)? {
        ZipReadOutcome::Encrypted => return Ok(BackendOutcome::Encrypted),
        ZipReadOutcome::Bytes(data) => data,
    };
    let opf = parse_opf(&opf_bytes, limits)?;

    // 4. Walk unique spine items in reading order.
    let mut seen_idrefs: HashSet<String> = HashSet::new();
    let mut seen_paths: HashSet<String> = HashSet::new();
    let mut markdown_parts: Vec<String> = Vec::new();
    let mut plain_parts: Vec<String> = Vec::new();
    let mut warnings: Vec<String> = Vec::new();
    let mut total_md_chars: usize = 0;
    let mut skipped_non_html: usize = 0;
    let mut missing_chapters: usize = 0;

    for idref in &opf.spine {
        if !seen_idrefs.insert(idref.clone()) {
            continue;
        }
        let Some(item) = opf.manifest.get(idref) else {
            missing_chapters += 1;
            continue;
        };
        let resolved_path = resolve_epub_path(opf_dir, &item.href)?;
        let lookup_key = resolved_path.to_ascii_lowercase();
        if !seen_paths.insert(lookup_key.clone()) {
            continue;
        }
        if !is_html_spine_item(&item.media_type, &resolved_path) {
            skipped_non_html += 1;
            continue;
        }
        let Some(chapter_idx) = find_zip_entry_case_insensitive(&archive, &lookup_key) else {
            missing_chapters += 1;
            continue;
        };
        let chapter_bytes = match budget.read_entry_bounded(&mut archive, chapter_idx, limits)? {
            ZipReadOutcome::Encrypted => return Ok(BackendOutcome::Encrypted),
            ZipReadOutcome::Bytes(data) => data,
        };
        let rendered = html_render::render_html_bytes(&chapter_bytes, limits)?;
        if !rendered.markdown.is_empty() || !rendered.plain_text.is_empty() {
            total_md_chars = total_md_chars.saturating_add(rendered.markdown.chars().count());
            markdown_parts.push(rendered.markdown);
            plain_parts.push(rendered.plain_text);
            // Stop reading further chapters once we already have enough text to
            // fill `max_output_chars`, letting common finalization truncate cleanly.
            if total_md_chars > limits.max_output_chars.saturating_add(2048) {
                break;
            }
        }
    }

    if skipped_non_html > 0 {
        warnings.push(format!(
            "skipped {skipped_non_html} non-HTML spine item(s) in EPUB"
        ));
    }
    if missing_chapters > 0 {
        warnings.push(format!(
            "skipped {missing_chapters} missing spine chapter(s) in EPUB"
        ));
    }

    Ok(BackendOutcome::Extracted(RawExtraction {
        format: DocumentFormat::Epub,
        title: opf.title,
        markdown: markdown_parts.join("\n\n---\n\n"),
        plain_text: plain_parts.join("\n\n"),
        truncated: false,
        warnings,
    }))
}

/// Fail-closed check of `META-INF/encryption.xml`. Returns `true` ONLY when the
/// XML parses cleanly and every `<EncryptedData>` entry uses an allowlisted
/// IDPF/Adobe font obfuscation algorithm on a font resource.
fn is_font_obfuscation_only(xml_bytes: &[u8]) -> bool {
    let mut reader = Reader::from_reader(Cursor::new(xml_bytes));
    let mut buf = Vec::new();
    let mut depth = 0usize;
    let mut in_encrypted_data = false;
    let mut saw_any_encrypted_data = false;
    let mut current_algo: Option<String> = None;
    let mut current_uri: Option<String> = None;

    loop {
        buf.clear();
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(e)) => {
                depth += 1;
                if depth > MAX_XML_DEPTH {
                    return false;
                }
                let local = local_name_string(e.name().as_ref());
                if local.eq_ignore_ascii_case("EncryptedData") {
                    if in_encrypted_data {
                        return false;
                    }
                    in_encrypted_data = true;
                    saw_any_encrypted_data = true;
                    current_algo = None;
                    current_uri = None;
                } else if in_encrypted_data {
                    inspect_encryption_child(&e, &local, &mut current_algo, &mut current_uri);
                }
            }
            Ok(Event::Empty(e)) => {
                if depth + 1 > MAX_XML_DEPTH {
                    return false;
                }
                let local = local_name_string(e.name().as_ref());
                if local.eq_ignore_ascii_case("EncryptedData") {
                    // Empty `<EncryptedData/>` has neither valid algorithm nor font URI.
                    return false;
                }
                if in_encrypted_data {
                    inspect_encryption_child(&e, &local, &mut current_algo, &mut current_uri);
                }
            }
            Ok(Event::End(e)) => {
                depth = depth.saturating_sub(1);
                let local = local_name_string(e.name().as_ref());
                if local.eq_ignore_ascii_case("EncryptedData") {
                    if !is_allowed_font_obfuscation(current_algo.as_deref(), current_uri.as_deref())
                    {
                        return false;
                    }
                    in_encrypted_data = false;
                    current_algo = None;
                    current_uri = None;
                }
            }
            Ok(Event::Eof) => break,
            Err(_) => return false,
            _ => {}
        }
    }

    if in_encrypted_data {
        return false;
    }
    // If encryption.xml existed and parsed cleanly with zero `<EncryptedData>`
    // blocks, fail closed unless at least one valid font-obfuscated entry was
    // present (an unknown schema inside encryption.xml must not bypass DRM).
    saw_any_encrypted_data
}

fn inspect_encryption_child(
    e: &BytesStart<'_>,
    local: &str,
    current_algo: &mut Option<String>,
    current_uri: &mut Option<String>,
) {
    if local.eq_ignore_ascii_case("EncryptionMethod")
        && let Some(val) = attr_value_local(e, "Algorithm")
    {
        *current_algo = Some(val);
    } else if local.eq_ignore_ascii_case("CipherReference")
        && let Some(val) = attr_value_local(e, "URI")
    {
        *current_uri = Some(val);
    }
}

fn is_allowed_font_obfuscation(algo: Option<&str>, uri: Option<&str>) -> bool {
    let (Some(algo), Some(uri)) = (algo, uri) else {
        return false;
    };
    if !ALLOWED_FONT_OBFUSCATION_ALGORITHMS
        .iter()
        .any(|allowed| algo.trim().eq_ignore_ascii_case(allowed))
    {
        return false;
    }
    let Some(decoded) = percent_decode_str(strip_query_and_fragment(uri.trim())) else {
        return false;
    };
    let lower = decoded.to_ascii_lowercase();
    ALLOWED_FONT_EXTENSIONS
        .iter()
        .any(|ext| lower.ends_with(ext))
}

fn parse_container_rootfile(xml_bytes: &[u8]) -> Result<String, ExtractError> {
    let mut reader = Reader::from_reader(Cursor::new(xml_bytes));
    let mut buf = Vec::new();
    let mut depth = 0usize;

    loop {
        buf.clear();
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(e)) => {
                depth += 1;
                if depth > MAX_XML_DEPTH {
                    return Err(ExtractError::LimitExceeded(
                        "EPUB container.xml exceeds XML nesting depth limit".to_owned(),
                    ));
                }
                if local_name_eq(e.name().as_ref(), b"rootfile")
                    && let Some(path) = attr_value_local(&e, "full-path")
                {
                    return Ok(path);
                }
            }
            Ok(Event::Empty(e)) => {
                if local_name_eq(e.name().as_ref(), b"rootfile")
                    && let Some(path) = attr_value_local(&e, "full-path")
                {
                    return Ok(path);
                }
            }
            Ok(Event::End(_)) => depth = depth.saturating_sub(1),
            Ok(Event::Eof) => break,
            Err(err) => {
                return Err(ExtractError::Invalid(format!(
                    "malformed EPUB container.xml: {err}"
                )));
            }
            _ => {}
        }
    }

    Err(ExtractError::Invalid(
        "EPUB container.xml has no rootfile full-path".to_owned(),
    ))
}

struct ManifestItem {
    href: String,
    media_type: String,
}

struct ParsedOpf {
    title: Option<String>,
    manifest: HashMap<String, ManifestItem>,
    spine: Vec<String>,
}

fn parse_opf(xml_bytes: &[u8], limits: &ExtractLimits) -> Result<ParsedOpf, ExtractError> {
    let mut reader = Reader::from_reader(Cursor::new(xml_bytes));
    let mut buf = Vec::new();
    let mut depth = 0usize;
    let mut in_title = false;
    let mut title_buf = String::new();
    let mut title: Option<String> = None;
    let mut manifest: HashMap<String, ManifestItem> = HashMap::new();
    let mut spine: Vec<String> = Vec::new();
    let max_entries = limits.max_package_entries;

    loop {
        buf.clear();
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(e)) => {
                depth += 1;
                if depth > MAX_XML_DEPTH {
                    return Err(ExtractError::LimitExceeded(
                        "EPUB OPF exceeds XML nesting depth limit".to_owned(),
                    ));
                }
                let name = e.name();
                if local_name_eq(name.as_ref(), b"title") && title.is_none() {
                    in_title = true;
                } else if local_name_eq(name.as_ref(), b"item") {
                    record_manifest_item(&e, &mut manifest, max_entries)?;
                } else if local_name_eq(name.as_ref(), b"itemref") {
                    record_spine_item(&e, &mut spine, max_entries)?;
                }
            }
            Ok(Event::Empty(e)) => {
                let name = e.name();
                if local_name_eq(name.as_ref(), b"item") {
                    record_manifest_item(&e, &mut manifest, max_entries)?;
                } else if local_name_eq(name.as_ref(), b"itemref") {
                    record_spine_item(&e, &mut spine, max_entries)?;
                }
            }
            Ok(Event::Text(t)) => {
                if in_title
                    && title.is_none()
                    && title_buf.len() < 1024
                    && let Ok(text) = t.decode()
                {
                    title_buf.push_str(&text);
                }
            }
            Ok(Event::GeneralRef(r)) => {
                if in_title && title.is_none() && title_buf.len() < 1024 {
                    if let Ok(Some(ch)) = r.resolve_char_ref() {
                        title_buf.push(ch);
                    } else if let Ok(name) = r.decode() {
                        match name.as_ref() {
                            "amp" => title_buf.push('&'),
                            "lt" => title_buf.push('<'),
                            "gt" => title_buf.push('>'),
                            "quot" => title_buf.push('"'),
                            "apos" => title_buf.push('\''),
                            _ => {}
                        }
                    }
                }
            }
            Ok(Event::End(e)) => {
                depth = depth.saturating_sub(1);
                if local_name_eq(e.name().as_ref(), b"title") && in_title {
                    in_title = false;
                    let clean = title_buf.split_whitespace().collect::<Vec<_>>().join(" ");
                    if !clean.is_empty() {
                        title = Some(clean);
                    }
                }
            }
            Ok(Event::Eof) => break,
            Err(err) => {
                return Err(ExtractError::Invalid(format!(
                    "malformed EPUB OPF package: {err}"
                )));
            }
            _ => {}
        }
    }

    Ok(ParsedOpf {
        title,
        manifest,
        spine,
    })
}

fn record_manifest_item(
    e: &BytesStart<'_>,
    manifest: &mut HashMap<String, ManifestItem>,
    max_entries: usize,
) -> Result<(), ExtractError> {
    if manifest.len() >= max_entries {
        return Err(ExtractError::LimitExceeded(format!(
            "EPUB manifest exceeds entry limit of {max_entries}"
        )));
    }
    if let (Some(id), Some(href)) = (attr_value_local(e, "id"), attr_value_local(e, "href")) {
        let media_type = attr_value_local(e, "media-type").unwrap_or_default();
        manifest.insert(id, ManifestItem { href, media_type });
    }
    Ok(())
}

fn record_spine_item(
    e: &BytesStart<'_>,
    spine: &mut Vec<String>,
    max_entries: usize,
) -> Result<(), ExtractError> {
    if spine.len() >= max_entries {
        return Err(ExtractError::LimitExceeded(format!(
            "EPUB spine exceeds item limit of {max_entries}"
        )));
    }
    if let Some(idref) = attr_value_local(e, "idref")
        && !idref.is_empty()
    {
        spine.push(idref);
    }
    Ok(())
}

fn is_html_spine_item(media_type: &str, path: &str) -> bool {
    let mt = media_type
        .split(';')
        .next()
        .unwrap_or_default()
        .trim()
        .to_ascii_lowercase();
    if matches!(
        mt.as_str(),
        "application/xhtml+xml" | "text/html" | "application/x-dtbook+xml" | "text/x-oeb1-document"
    ) {
        return true;
    }
    if mt.is_empty() {
        let lower = path.to_ascii_lowercase();
        return lower.ends_with(".xhtml")
            || lower.ends_with(".html")
            || lower.ends_with(".htm")
            || lower.ends_with(".xml");
    }
    false
}

fn parent_dir(path: &str) -> &str {
    match path.rfind('/') {
        Some(idx) => &path[..idx],
        None => "",
    }
}

fn strip_query_and_fragment(raw: &str) -> &str {
    let without_frag = raw.split('#').next().unwrap_or(raw);
    without_frag.split('?').next().unwrap_or(without_frag)
}

/// Resolve an EPUB OPF/spine path relative to `base_dir`, percent-decoding and
/// stripping `#fragment`/`?query` BEFORE validating against path traversal
/// (`..`, `\`, leading `/`, NUL).
fn resolve_epub_path(base_dir: &str, raw_href: &str) -> Result<String, ExtractError> {
    let stripped = strip_query_and_fragment(raw_href.trim());
    let decoded = percent_decode_str(stripped).ok_or_else(|| {
        ExtractError::Invalid("EPUB path contains invalid percent-encoding".to_owned())
    })?;
    if decoded.is_empty()
        || decoded.contains('\0')
        || decoded.starts_with('/')
        || decoded.starts_with('\\')
    {
        return Err(ExtractError::Invalid(format!(
            "invalid EPUB internal path '{raw_href}'"
        )));
    }
    if decoded.contains('\\') {
        return Err(ExtractError::Invalid(format!(
            "EPUB internal path contains backslash '{raw_href}'"
        )));
    }

    let mut segments: Vec<&str> = Vec::new();
    if !base_dir.is_empty() {
        for seg in base_dir.split('/') {
            if !seg.is_empty() {
                segments.push(seg);
            }
        }
    }
    for seg in decoded.split('/') {
        match seg {
            "" | "." => continue,
            ".." => {
                // Reject any `..` traversal in decoded EPUB paths outright so
                // `../` or `%2e%2e/` can never escape container boundaries.
                return Err(ExtractError::Invalid(format!(
                    "EPUB internal path contains traversal segment '{raw_href}'"
                )));
            }
            normal => segments.push(normal),
        }
    }
    if segments.is_empty() {
        return Err(ExtractError::Invalid(format!(
            "EPUB internal path '{raw_href}' resolved empty"
        )));
    }
    Ok(normalize_zip_path(&segments.join("/")))
}

fn percent_decode_str(input: &str) -> Option<String> {
    let bytes = input.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' {
            if i + 2 >= bytes.len() {
                return None;
            }
            let hi = hex_val(bytes[i + 1])?;
            let lo = hex_val(bytes[i + 2])?;
            out.push((hi << 4) | lo);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8(out).ok()
}

fn hex_val(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}

fn local_name_bytes(raw: &[u8]) -> &[u8] {
    match raw.iter().rposition(|&b| b == b':') {
        Some(idx) => &raw[idx + 1..],
        None => raw,
    }
}

fn local_name_eq(raw: &[u8], expected: &[u8]) -> bool {
    local_name_bytes(raw).eq_ignore_ascii_case(expected)
}

fn local_name_string(raw: &[u8]) -> String {
    String::from_utf8_lossy(local_name_bytes(raw)).into_owned()
}

fn attr_value_local(e: &BytesStart<'_>, expected_local: &str) -> Option<String> {
    for attr in e.attributes().flatten() {
        if local_name_eq(attr.key.as_ref(), expected_local.as_bytes())
            && let Ok(val) = attr.normalized_value(quick_xml::XmlVersion::Implicit1_0)
        {
            return Some(val.into_owned());
        }
    }
    None
}
