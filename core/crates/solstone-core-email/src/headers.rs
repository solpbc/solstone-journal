// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Header field parsing and MIME part decoding via `mail-parser`.

use mail_parser::{
    Address, HeaderValue, Message, MessageParser, MessagePart, MimeHeaders, PartType,
};

use crate::model::{Addr, Header, Part};

/// The exact header fields requested during a header-only scan.
pub const SCAN_FIELDS: &[&str] = &[
    "MESSAGE-ID",
    "IN-REPLY-TO",
    "REFERENCES",
    "FROM",
    "TO",
    "CC",
    "DATE",
    "SUBJECT",
    "AUTO-SUBMITTED",
    "LIST-ID",
];

fn ids(value: &HeaderValue<'_>) -> Vec<String> {
    match value {
        HeaderValue::Text(t) => split_ids(t),
        HeaderValue::TextList(list) => list.iter().flat_map(|t| split_ids(t)).collect(),
        _ => Vec::new(),
    }
}

fn split_ids(raw: &str) -> Vec<String> {
    raw.split(|c: char| c.is_whitespace() || c == ',')
        .map(|t| t.trim().trim_matches(['<', '>']).to_owned())
        .filter(|t| !t.is_empty())
        .collect()
}

fn addrs(value: Option<&Address<'_>>) -> Vec<Addr> {
    value
        .map(|a| {
            a.iter()
                .map(|x| Addr {
                    name: x.name().map(str::to_owned),
                    address: x.address().map(str::to_owned),
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Populate the RFC 5322 header fields of `h` from a parsed header block.
pub fn fill(h: &mut Header, m: &Message<'_>) {
    h.message_id = m
        .message_id()
        .map(|s| s.trim().trim_matches(['<', '>']).to_owned())
        .filter(|s| !s.is_empty());
    h.in_reply_to = ids(m.in_reply_to());
    h.references = ids(m.references());
    h.from = addrs(m.from());
    h.to = addrs(m.to());
    h.cc = addrs(m.cc());
    h.date_ms = m.date().map(|d| d.to_timestamp() * 1000);
    h.subject = m.subject().map(str::to_owned);
    h.auto_submitted = m
        .header_raw("Auto-Submitted")
        .map(|s| s.trim().to_ascii_lowercase());
    h.list_id = m.header_raw("List-Id").map(|s| s.trim().to_owned());
}

/// Parse a raw RFC 5322 header block (as returned by `BODY.PEEK[HEADER.FIELDS ...]`).
pub fn fill_from_raw_headers(h: &mut Header, raw: &[u8]) {
    let mut block = raw.to_vec();
    if !block.ends_with(b"\r\n\r\n") {
        if !block.ends_with(b"\r\n") {
            block.extend_from_slice(b"\r\n");
        }
        block.extend_from_slice(b"\r\n");
    }
    if let Some(m) = MessageParser::default().parse_headers(&block) {
        fill(h, &m);
    }
}

fn ctype_param(part: &MessagePart<'_>, name: &str) -> Option<String> {
    part.content_type()
        .and_then(|c| c.attribute(name))
        .map(str::to_owned)
}

/// Build the IMAP-numbered MIME tree of a complete parsed message.
#[must_use]
pub fn tree(m: &Message<'_>) -> Part {
    let root = m.root_part();
    match &root.body {
        PartType::Multipart(_) => node(m, 0, Vec::new()),
        _ => node(m, 0, vec![1]),
    }
}

fn node(m: &Message<'_>, id: u32, path: Vec<u32>) -> Part {
    let part = m.part(id).expect("part id from mail-parser");
    let mime = part
        .content_type()
        .map(|c| {
            format!(
                "{}/{}",
                c.ctype(),
                c.subtype()
                    .unwrap_or(if c.ctype() == "text" { "plain" } else { "" })
            )
            .to_ascii_lowercase()
        })
        .unwrap_or_else(|| "text/plain".to_owned());
    let disposition = part
        .content_disposition()
        .map(|d| d.ctype().to_ascii_lowercase());
    let filename = part
        .content_disposition()
        .and_then(|d| d.attribute("filename"))
        .or_else(|| part.content_type().and_then(|c| c.attribute("name")))
        .map(str::to_owned);
    let mut out = Part {
        path: path.clone(),
        mime,
        charset: ctype_param(part, "charset"),
        format: ctype_param(part, "format"),
        delsp: ctype_param(part, "delsp"),
        method: ctype_param(part, "method"),
        start: ctype_param(part, "start"),
        content_id: part.content_id().map(str::to_owned),
        encoding: None,
        disposition,
        filename,
        size: u64::from(part.offset_end.saturating_sub(part.offset_body)),
        children: Vec::new(),
    };
    if let PartType::Multipart(children) = &part.body {
        for (i, child) in children.iter().enumerate() {
            let mut p = path.clone();
            p.push(i as u32 + 1);
            out.children.push(node(m, *child, p));
        }
    }
    out
}

/// Decode one text/HTML leaf from a parsed message by its IMAP part path.
#[must_use]
pub fn leaf_text(m: &Message<'_>, path: &[u32]) -> Option<String> {
    let mut id = 0u32;
    let root = m.root_part();
    let root_is_multi = matches!(root.body, PartType::Multipart(_));
    let steps: &[u32] = if root_is_multi { path } else { &path[1..] };
    for step in steps {
        let part = m.part(id)?;
        let PartType::Multipart(children) = &part.body else {
            return None;
        };
        id = *children.get((*step as usize).checked_sub(1)?)?;
    }
    match &m.part(id)?.body {
        PartType::Text(t) | PartType::Html(t) => Some(t.replace("\r\n", "\n")),
        _ => None,
    }
}

/// Decode one fetched MIME part from its `BODY.PEEK[<part>.MIME]` header and
/// `BODY.PEEK[<part>]` body bytes, falling back to `Part` metadata if the
/// server omitted the `.MIME` section on a single-part message.
#[must_use]
pub fn decode_fetched_part(part: &Part, mime_header: Option<&[u8]>, body: &[u8]) -> Option<String> {
    let mut message = Vec::new();
    match mime_header.filter(|h| !h.is_empty()) {
        Some(hdr) => {
            message.extend_from_slice(hdr);
            if !message.ends_with(b"\r\n\r\n") {
                if !message.ends_with(b"\r\n") {
                    message.extend_from_slice(b"\r\n");
                }
                message.extend_from_slice(b"\r\n");
            }
        }
        None => {
            let mut ct = format!("Content-Type: {}", part.mime);
            if let Some(cs) = part.charset.as_deref().filter(|s| !s.is_empty()) {
                ct.push_str(&format!("; charset=\"{cs}\""));
            }
            ct.push_str("\r\n");
            message.extend_from_slice(ct.as_bytes());
            if let Some(enc) = part.encoding.as_deref().filter(|s| !s.is_empty()) {
                message
                    .extend_from_slice(format!("Content-Transfer-Encoding: {enc}\r\n").as_bytes());
            }
            message.extend_from_slice(b"\r\n");
        }
    }
    message.extend_from_slice(body);
    let parsed = MessageParser::default().parse(&message)?;
    leaf_text(&parsed, &[1])
}

/// Decode the MIME transfer encoding without treating a textual attachment as
/// UTF-8 or normalizing its bytes. The caller has already bounded the wire input.
pub(crate) fn decode_attachment_bytes(
    encoding: Option<&str>,
    mut bytes: Vec<u8>,
    decoded_limit: usize,
) -> crate::error::Result<Vec<u8>> {
    use crate::error::EmailError;
    use mail_parser::decoders::{
        base64::base64_decode, quoted_printable::quoted_printable_decode_char,
    };

    let invalid = || EmailError::InvalidInput("malformed attachment transfer encoding".into());
    let too_large = || EmailError::InvalidInput("attachment exceeds decoded budget".into());
    match encoding.unwrap_or("7bit").to_ascii_lowercase().as_str() {
        "7bit" | "8bit" | "binary" => (),
        "base64" => {
            bytes.retain(|c| !matches!(c, b' ' | b'\t' | b'\r' | b'\n'));
            let pad = bytes.iter().rev().take_while(|c| **c == b'=').count();
            let data = &bytes[..bytes.len() - pad];
            if bytes.is_empty()
                || !bytes.len().is_multiple_of(4)
                || pad > 2
                || !data
                    .iter()
                    .all(|c| c.is_ascii_alphanumeric() || matches!(c, b'+' | b'/'))
            {
                return Err(invalid());
            }
            if bytes.len() / 4 * 3 - pad > decoded_limit {
                return Err(too_large());
            }
            bytes = base64_decode(&bytes).ok_or_else(invalid)?;
        }
        "quoted-printable" => {
            let mut read = 0;
            let mut written = 0;
            while read < bytes.len() {
                let ch = bytes[read];
                read += 1;
                let decoded = if ch == b'=' {
                    if bytes[read..].starts_with(b"\r\n") {
                        read += 2;
                        continue;
                    }
                    if bytes.get(read) == Some(&b'\n') {
                        read += 1;
                        continue;
                    }
                    let pair = bytes.get(read..read + 2).ok_or_else(invalid)?;
                    read += 2;
                    quoted_printable_decode_char(pair[0], pair[1]).ok_or_else(invalid)?
                } else {
                    ch
                };
                if written == decoded_limit {
                    return Err(too_large());
                }
                bytes[written] = decoded;
                written += 1;
            }
            bytes.truncate(written);
        }
        _ => {
            return Err(EmailError::InvalidInput(
                "unsupported attachment transfer encoding".into(),
            ));
        }
    }
    if bytes.len() > decoded_limit {
        return Err(too_large());
    }
    Ok(bytes)
}

/// Decode RFC 2231 parameter bytes or RFC 2392 CID URL bytes. A plus sign is literal.
pub(crate) fn percent_decoded(value: &str) -> Option<Vec<u8>> {
    let mut bytes = value.bytes();
    let mut out = Vec::with_capacity(value.len());
    while let Some(b) = bytes.next() {
        if b == b'%' {
            out.push(
                mail_parser::decoders::quoted_printable::quoted_printable_decode_char(
                    bytes.next()?,
                    bytes.next()?,
                )?,
            );
        } else {
            out.push(b);
        }
    }
    Some(out)
}
