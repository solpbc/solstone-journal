// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Deterministic email text extraction that never invents words.
//!
//! A span is removed as quoted text only when two independent signals agree
//! and at least one establishes provenance (attribution line, header block,
//! or alignment with the answered parent message). Anything uncertain stays in
//! the turn and is recorded with its exact UTF-8 byte range in `uncertain_spans`
//! so the reader folds it as `possibly quoted`. Forwarded message blocks are
//! preserved alongside the owner's comment under the 20,000-character cap.

use std::collections::{BTreeSet, HashMap};

use serde::{Deserialize, Serialize};

use crate::html;
use crate::text::{
    coverage, header_block, header_key, is_attribution, is_forward_marker, is_mobile_footer,
    is_rule, loose, tokens,
};

pub const EXTRACTOR_VERSION: &str = "email-extract-1";

/// Maximum kept characters for an answered or forwarded message body.
pub const CAP_CHARS: usize = 20_000;

/// Share of a span's words that must align with the answered message for
/// parent-match provenance; below it, a parent in hand vetoes removal.
pub const PARENT_MATCH: f64 = 0.8;

const MIN_MATCH_WORDS: usize = 4;

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct Removal {
    pub kind: String,
    pub signals: Vec<String>,
    pub chars: usize,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct Held {
    pub reason: String,
    pub chars: usize,
    #[serde(default)]
    pub text: String,
}

/// Exact UTF-8 half-open byte range inside `Extracted.text` for a span that
/// looks quoted without two agreeing signals.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct ByteSpan {
    pub start: usize,
    pub end: usize,
}

/// Forwarded message content kept beside the owner's comment under the cap.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct ForwardedBlock {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub from: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub date: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub subject: Option<String>,
    pub text: String,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub truncated: bool,
    #[serde(default, skip_serializing_if = "is_zero")]
    pub cut_chars: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cut_at: Option<String>,
}

fn is_zero(v: &usize) -> bool {
    *v == 0
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct Extracted {
    pub text: String,
    /// `plain`, `plain_flowed`, `html`, or `none`.
    pub text_source: String,
    pub removed: Vec<Removal>,
    pub held: Vec<Held>,
    pub uncertain_spans: Vec<ByteSpan>,
    pub forwarded: Vec<ForwardedBlock>,
    pub extractor: String,
}

pub struct Evidence<'a> {
    pub parent: Option<&'a str>,
    /// Normalized blocks closing >= 3 of the owner's messages after `-- `.
    pub recurring_signatures: &'a BTreeSet<String>,
}

/// Result of applying the 20,000-character cap in paragraph -> line -> word -> hard order.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CappedText {
    pub text: String,
    pub truncated: bool,
    pub cut_chars: usize,
    pub cut_at: Option<&'static str>,
}

/// Cut `text` at the last paragraph end within `cap` (else line end, else word
/// end, never below half the cap, else exact char boundary), returning the
/// kept text and exact count of omitted characters.
#[must_use]
pub fn cap_text(text: &str, cap: usize) -> CappedText {
    let total = text.chars().count();
    if total <= cap {
        return CappedText {
            text: text.to_owned(),
            truncated: false,
            cut_chars: 0,
            cut_at: None,
        };
    }
    let byte_cap = text
        .char_indices()
        .nth(cap)
        .map_or(text.len(), |(idx, _)| idx);
    let head = &text[..byte_cap];
    let floor = byte_cap / 2;
    let (end, how) = if let Some(i) = head.rfind("\n\n").filter(|&i| i >= floor) {
        (i, "paragraph")
    } else if let Some(i) = head.rfind('\n').filter(|&i| i >= floor) {
        (i, "line")
    } else if let Some(i) = head.rfind(char::is_whitespace).filter(|&i| i >= floor) {
        (i, "word")
    } else {
        (byte_cap, "hard")
    };
    let kept = text[..end].trim_end();
    let cut = total.saturating_sub(kept.chars().count());
    CappedText {
        text: kept.to_owned(),
        truncated: true,
        cut_chars: cut,
        cut_at: Some(how),
    }
}

/// Parse a raw forwarded message block (optional forward marker + header lines +
/// body) into a capped [`ForwardedBlock`].
#[must_use]
pub fn parse_forwarded_headers_and_body(raw: &str) -> ForwardedBlock {
    let lines: Vec<&str> = raw.split('\n').map(|l| l.trim_end_matches('\r')).collect();
    let mut idx = 0;
    while idx < lines.len()
        && (lines[idx].trim().is_empty()
            || is_forward_marker(lines[idx])
            || is_rule(lines[idx])
            || lines[idx]
                .trim()
                .to_lowercase()
                .contains("forwarded message"))
    {
        idx += 1;
    }
    let mut from = None;
    let mut date = None;
    let mut subject = None;
    while idx < lines.len() {
        let trimmed = lines[idx].trim();
        if trimmed.is_empty() {
            idx += 1;
            break;
        }
        let Some(key) = header_key(trimmed) else {
            break;
        };
        let value = trimmed
            .split_once(':')
            .map(|(_, rhs)| rhs.trim())
            .filter(|v| !v.is_empty())
            .map(str::to_owned);
        match key {
            "from:" | "von:" | "de :" | "de:" | "da:" | "van:" => {
                from = from.or(value);
            }
            "date:" | "sent:" | "gesendet:" | "envoyé :" | "envoyé:" | "enviado:"
            | "enviado el:" | "inviato:" | "verzonden:" => {
                date = date.or(value);
            }
            "subject:" | "betreff:" | "objet :" | "objet:" | "assunto:" | "asunto:"
            | "oggetto:" | "onderwerp:" => {
                subject = subject.or(value);
            }
            _ => {}
        }
        idx += 1;
    }
    while idx < lines.len() && lines[idx].trim().is_empty() {
        idx += 1;
    }
    let body_raw = lines[idx..].join("\n").trim().to_owned();
    let capped = cap_text(&body_raw, CAP_CHARS);
    ForwardedBlock {
        from,
        date,
        subject,
        text: capped.text,
        truncated: capped.truncated,
        cut_chars: capped.cut_chars,
        cut_at: capped.cut_at.map(str::to_owned),
    }
}

/// Never-empty fallback text for an owner turn that has no written text of its own.
#[must_use]
pub fn owner_fallback_text(
    file_attachments: usize,
    inline_images: usize,
    has_forwarded: bool,
) -> &'static str {
    match (file_attachments, inline_images) {
        (1, 0) => "sent with an attachment",
        (n, 0) if n > 1 => "sent with attachments",
        (0, 1) => "sent with an image",
        (0, m) if m > 1 => "sent with images",
        (1, 1) => "sent with an attachment and an image",
        (1, m) if m > 1 => "sent with an attachment and images",
        (n, 1) if n > 1 => "sent with attachments and an image",
        (n, m) if n > 1 && m > 1 => "sent with attachments and images",
        _ if has_forwarded => "forwarded without comment",
        _ => "sent with no text",
    }
}

/// Never-empty fallback text for an answered message inside `in reply to ...`.
pub const ANSWERED_NO_TEXT_FALLBACK: &str = "this message had no text";

/// RFC 3676: join soft-broken lines of the same quote depth, undo space stuffing.
#[must_use]
pub fn unflow(text: &str, delsp: bool) -> String {
    let mut out: Vec<String> = Vec::new();
    let mut open: Option<(usize, String)> = None;
    for raw in text.split('\n') {
        let depth = raw.chars().take_while(|c| *c == '>').count();
        let mut body = &raw[depth..];
        if let Some(rest) = body.strip_prefix(' ') {
            body = rest;
        }
        let soft = body.ends_with(' ') && body != "-- ";
        let piece = if soft && delsp {
            &body[..body.len() - 1]
        } else {
            body
        };
        match open.take() {
            Some((d, mut acc)) if d == depth => {
                acc.push_str(piece);
                if soft {
                    open = Some((d, acc));
                } else {
                    out.push(prefixed(d, &acc));
                }
            }
            prev => {
                if let Some((d, acc)) = prev {
                    out.push(prefixed(d, &acc));
                }
                if soft {
                    open = Some((depth, piece.to_owned()));
                } else {
                    out.push(prefixed(depth, piece));
                }
            }
        }
    }
    if let Some((d, acc)) = open {
        out.push(prefixed(d, &acc));
    }
    out.join("\n")
}

fn prefixed(depth: usize, body: &str) -> String {
    if depth == 0 {
        body.to_owned()
    } else {
        format!("{} {body}", ">".repeat(depth))
    }
}

fn depth(line: &str) -> usize {
    let mut d = 0;
    for c in line.chars() {
        match c {
            '>' => d += 1,
            ' ' if d > 0 => {}
            _ => break,
        }
    }
    d
}

fn strip(line: &str, levels: usize) -> &str {
    let mut rest = line;
    for _ in 0..levels {
        rest = rest.trim_start_matches(' ');
        match rest.strip_prefix('>') {
            Some(r) => rest = r,
            None => break,
        }
    }
    rest.strip_prefix(' ').unwrap_or(rest)
}

struct Work<'a> {
    lines: Vec<&'a str>,
    keep: Vec<bool>,
    uncertain: Vec<bool>,
    removed: Vec<Removal>,
    held: Vec<Held>,
    forwarded: Vec<ForwardedBlock>,
}

impl Work<'_> {
    fn text(&self, range: std::ops::Range<usize>, levels: usize) -> String {
        self.lines[range]
            .iter()
            .map(|l| strip(l, levels))
            .collect::<Vec<_>>()
            .join("\n")
    }

    fn drop(&mut self, range: std::ops::Range<usize>, kind: &str, signals: Vec<String>) {
        let chars = self.lines[range.clone()]
            .iter()
            .map(|l| l.chars().count())
            .sum();
        for idx in range {
            self.keep[idx] = false;
            self.uncertain[idx] = false;
        }
        self.removed.push(Removal {
            kind: kind.into(),
            signals,
            chars,
        });
    }

    fn hold(&mut self, range: std::ops::Range<usize>, reason: &str) {
        let chars = self.lines[range.clone()]
            .iter()
            .map(|l| l.chars().count())
            .sum();
        for idx in range.clone() {
            if self.keep[idx] {
                self.uncertain[idx] = true;
            }
        }
        self.held.push(Held {
            reason: reason.into(),
            chars,
            text: self.lines[range].join("\n"),
        });
    }
}

fn prev_nonblank(w: &Work<'_>, i: usize) -> Option<usize> {
    (0..i)
        .rev()
        .find(|&j| w.keep[j] && !strip(w.lines[j], depth(w.lines[j])).trim().is_empty())
}

#[derive(Clone, Copy, Default)]
struct HtmlSide<'a> {
    quote: &'a str,
    novel: &'a str,
}

impl HtmlSide<'_> {
    fn in_quote(&self, span: &str) -> bool {
        !self.quote.is_empty()
            && coverage(span, self.quote) >= 0.8
            && (self.novel.is_empty() || coverage(span, self.novel) < 0.5)
    }
}

fn parent_matches(span: &str, parent: Option<&str>) -> bool {
    parent.is_some_and(|p| coverage(span, p) >= PARENT_MATCH)
}

fn quote_blocks(
    w: &mut Work<'_>,
    range: std::ops::Range<usize>,
    d: usize,
    ev: &Evidence<'_>,
    hs: HtmlSide<'_>,
) {
    let mut i = range.start;
    while i < range.end {
        if depth(w.lines[i]) < d || !w.keep[i] {
            i += 1;
            continue;
        }
        let start = i;
        let mut end = i;
        let mut j = i;
        while j < range.end {
            if depth(w.lines[j]) >= d {
                end = j + 1;
                j += 1;
            } else if w.lines[j].trim().is_empty() {
                j += 1;
            } else {
                break;
            }
        }
        let body = w.text(start..end, d);
        let first = body.lines().find(|l| !l.trim().is_empty()).unwrap_or("");
        let attr_inside = is_attribution(strip(first, 0));
        let above = prev_nonblank(w, start)
            .filter(|&k| depth(w.lines[k]) + 1 == d)
            .filter(|&k| is_attribution(strip(w.lines[k], d - 1)));
        let above = above.or_else(|| {
            let k = prev_nonblank(w, start)?;
            let k0 = prev_nonblank(w, k)?;
            (depth(w.lines[k]) + 1 == d
                && depth(w.lines[k0]) + 1 == d
                && is_attribution(&format!(
                    "{} {}",
                    strip(w.lines[k0], d - 1),
                    strip(w.lines[k], d - 1)
                )))
            .then_some(k0)
        });
        let quoted: String = if attr_inside {
            body.lines()
                .skip_while(|l| !is_attribution(l))
                .skip(1)
                .collect::<Vec<_>>()
                .join("\n")
        } else {
            body.clone()
        };
        let parent = ev
            .parent
            .is_some_and(|p| coverage(&quoted, p) >= PARENT_MATCH);
        let disagrees = ev.parent.is_some() && !parent;
        let container = hs.in_quote(&body);
        let mut signals = vec!["quote_prefix".to_owned()];
        if container {
            signals.push("quote_container".into());
        }
        if attr_inside || above.is_some() {
            signals.push("attribution".into());
        }
        if parent {
            signals.push("parent_match".into());
        }
        let provenance = attr_inside || above.is_some() || parent;
        if provenance && signals.len() >= 2 && !disagrees {
            let from = above.unwrap_or(start);
            w.drop(from..end, "quote", signals);
        } else {
            w.hold(
                start..end,
                if disagrees && provenance {
                    "quote_disagrees_with_parent"
                } else if container {
                    "quote_prefix_and_container_only"
                } else {
                    "quote_prefix_only"
                },
            );
            quote_blocks(w, start..end, d + 1, ev, hs);
        }
        i = end.max(i + 1);
    }
}

fn matching_run(
    w: &Work<'_>,
    j: usize,
    ev: &Evidence<'_>,
    hs: HtmlSide<'_>,
    signals: &mut Vec<String>,
) -> Option<usize> {
    let n = w.lines.len();
    let mut k = j;
    let mut last = None;
    while k < n {
        let start = k;
        while k < n && !w.lines[k].trim().is_empty() {
            k += 1;
        }
        let para = w.text(start..k, 0);
        let parent = parent_matches(&para, ev.parent);
        let container = hs.in_quote(&para);
        if !(parent || container) || (ev.parent.is_some() && !parent) {
            break;
        }
        for (hit, name) in [(parent, "parent_match"), (container, "quote_container")] {
            if hit && !signals.iter().any(|s| s == name) {
                signals.push(name.into());
            }
        }
        last = Some(k);
        while k < n && w.lines[k].trim().is_empty() {
            k += 1;
        }
    }
    last
}

fn bottom_regions(w: &mut Work<'_>, ev: &Evidence<'_>, hs: HtmlSide<'_>, is_reply: bool) {
    let n = w.lines.len();
    let mut i = 0;
    while i < n {
        if !w.keep[i] || depth(w.lines[i]) > 0 {
            i += 1;
            continue;
        }
        let line = w.lines[i];
        let next = (i + 1..n).find(|&j| !w.lines[j].trim().is_empty());
        if (is_forward_marker(line) || is_rule(line))
            && let Some(h) = next.filter(|&j| header_block(&w.lines, j).is_some())
        {
            let fwd = line.to_lowercase().contains("forward");
            let body_start = header_block(&w.lines, h).unwrap();
            let mut signals = vec![
                if fwd {
                    "forward_marker"
                } else if is_rule(line) {
                    "rule"
                } else {
                    "original_message_marker"
                }
                .to_owned(),
                "header_block".to_owned(),
            ];
            let kind = if fwd || (!is_reply && is_rule(line)) {
                "forward"
            } else {
                "quote"
            };
            let end = if kind == "quote" && ev.parent.is_some() {
                let first = (body_start..n)
                    .find(|&j| !w.lines[j].trim().is_empty())
                    .unwrap_or(n);
                match matching_run(w, first, ev, hs, &mut signals) {
                    Some(e) => e,
                    None => {
                        w.hold(i..n, "quote_disagrees_with_parent");
                        i += 1;
                        continue;
                    }
                }
            } else {
                n
            };
            if kind == "forward" {
                let raw_fwd = w.lines[i..end].join("\n");
                w.forwarded.push(parse_forwarded_headers_and_body(&raw_fwd));
            }
            w.drop(i..end, kind, signals);
            if end >= n {
                return;
            }
            i = end;
            continue;
        }
        if let Some(end) = header_block(&w.lines, i) {
            let tail = w.text(end..n, 0);
            let parent = parent_matches(&tail, ev.parent);
            let container = hs.in_quote(&w.text(i..n, 0));
            if parent || container {
                let mut signals = vec!["header_block".to_owned()];
                if parent {
                    signals.push("parent_match".into());
                }
                if container {
                    signals.push("quote_container".into());
                }
                w.drop(i..n, "quote", signals);
                return;
            }
            w.hold(i..end, "header_block_only");
            i = end;
            continue;
        }
        if is_attribution(line)
            && let Some(j) = next.filter(|&j| depth(w.lines[j]) == 0)
        {
            let mut signals = vec!["attribution".to_owned()];
            let last_match = matching_run(w, j, ev, hs, &mut signals);
            match last_match {
                Some(end) => {
                    w.drop(i..end, "quote", signals);
                    i = end;
                    continue;
                }
                None => w.hold(i..n, "attribution_only"),
            }
        }
        i += 1;
    }
}

fn unmarked_paragraphs(w: &mut Work<'_>, ev: &Evidence<'_>, hs: HtmlSide<'_>) {
    let n = w.lines.len();
    let mut i = 0;
    while i < n {
        if !w.keep[i] || w.lines[i].trim().is_empty() || depth(w.lines[i]) > 0 {
            i += 1;
            continue;
        }
        let start = i;
        while i < n && w.keep[i] && !w.lines[i].trim().is_empty() && depth(w.lines[i]) == 0 {
            i += 1;
        }
        let para = w.text(start..i, 0);
        if tokens(&para).len() < MIN_MATCH_WORDS {
            continue;
        }
        let parent = parent_matches(&para, ev.parent);
        let container = hs.in_quote(&para);
        match (parent, container) {
            (true, true) => {
                w.drop(
                    start..i,
                    "quote",
                    vec!["parent_match".into(), "quote_container".into()],
                );
            }
            (true, false) => w.hold(start..i, "parent_match_only"),
            (false, true) => w.hold(start..i, "quote_container_only"),
            _ => {}
        }
    }
}

fn last_kept(w: &Work<'_>) -> Option<usize> {
    (0..w.lines.len())
        .rev()
        .find(|&j| w.keep[j] && !w.lines[j].trim().is_empty())
}

fn signatures(w: &mut Work<'_>, ev: &Evidence<'_>, html_sig: &str) {
    if let Some(j) = last_kept(w)
        .filter(|&j| is_mobile_footer(w.lines[j]))
        .filter(|&j| j == 0 || w.lines[j - 1].trim().is_empty())
    {
        w.drop(j..j + 1, "signature", vec!["mobile_footer".into()]);
    }
    let Some(end) = last_kept(w).map(|j| j + 1) else {
        return;
    };
    let delim = (0..end).rev().find(|&j| w.keep[j] && w.lines[j] == "-- ");
    if let Some(d) = delim {
        let block_lines: Vec<usize> = (d + 1..end)
            .filter(|&j| w.keep[j] && !w.lines[j].trim().is_empty())
            .collect();
        if !block_lines.is_empty() && block_lines.len() <= 8 {
            let block = w.text(d + 1..end, 0);
            let recurs = ev.recurring_signatures.contains(&loose(&block));
            let container = !html_sig.is_empty() && coverage(&block, html_sig) >= 0.8;
            if recurs || container {
                let mut signals = vec!["sig_delimiter".to_owned()];
                if recurs {
                    signals.push("recurs".into());
                }
                if container {
                    signals.push("signature_container".into());
                }
                w.drop(d..end, "signature", signals);
                return;
            }
            w.hold(d..end, "sig_delimiter_only");
            return;
        }
    }
    let mut start = end;
    while start > 0 && w.keep[start - 1] && !w.lines[start - 1].trim().is_empty() {
        start -= 1;
    }
    if start < end {
        let para = w.text(start..end, 0);
        if !html_sig.is_empty() && coverage(&para, html_sig) >= 0.8 {
            w.drop(start..end, "signature", vec!["signature_container".into()]);
        } else if ev.recurring_signatures.contains(&loose(&para)) {
            w.hold(start..end, "signature_recurrence_only");
        }
    }
}

fn assemble(w: &Work<'_>) -> (String, Vec<ByteSpan>) {
    let mut kept_lines: Vec<(&str, bool)> = Vec::new();
    for (i, &l) in w.lines.iter().enumerate() {
        if !w.keep[i] {
            continue;
        }
        if l.trim().is_empty()
            && kept_lines
                .last()
                .is_none_or(|(prev, _)| prev.trim().is_empty())
        {
            continue;
        }
        kept_lines.push((l, w.uncertain[i]));
    }
    while kept_lines.last().is_some_and(|(l, _)| l.trim().is_empty()) {
        kept_lines.pop();
    }
    let mut out = String::new();
    let mut spans: Vec<ByteSpan> = Vec::new();
    let mut open_start: Option<usize> = None;
    let mut open_end: usize = 0;

    for (idx, (line, is_uncertain)) in kept_lines.iter().enumerate() {
        if idx > 0 {
            out.push('\n');
        }
        let line_start = out.len();
        out.push_str(line);
        let line_end = out.len();
        if *is_uncertain && !line.trim().is_empty() {
            if open_start.is_none() {
                open_start = Some(line_start);
            }
            open_end = line_end;
        } else if !*is_uncertain
            && !line.trim().is_empty()
            && let Some(start) = open_start.take()
            && start < open_end
        {
            spans.push(ByteSpan {
                start,
                end: open_end,
            });
        }
    }
    if let Some(start) = open_start
        && start < open_end
    {
        spans.push(ByteSpan {
            start,
            end: open_end,
        });
    }
    (out, spans)
}

/// Extract one message from its plain and/or HTML body parts.
#[must_use]
pub fn extract(
    plain: Option<&str>,
    flowed: Option<bool>,
    html_body: Option<&str>,
    is_reply: bool,
    ev: &Evidence<'_>,
) -> Extracted {
    let html_read = html_body.map(|h| html::read(h, ev.parent));
    let usable_plain = plain.filter(|p| !p.trim().is_empty());
    let Some(plain) = usable_plain else {
        let Some(read) = html_read else {
            return Extracted {
                text_source: "none".into(),
                extractor: EXTRACTOR_VERSION.into(),
                ..Extracted::default()
            };
        };
        let mut inner = from_plain(&read.text, ev, HtmlSide::default(), "", is_reply);
        let mut removed = read.removed;
        removed.append(&mut inner.removed);
        let mut held = read.held;
        // Map HTML-held container text onto byte spans inside `inner.text` when present.
        for h in &held {
            let trimmed = h.text.trim();
            if !trimmed.is_empty()
                && let Some(pos) = inner.text.find(trimmed)
            {
                let end = pos + trimmed.len();
                if inner.text.is_char_boundary(pos)
                    && inner.text.is_char_boundary(end)
                    && !inner
                        .uncertain_spans
                        .iter()
                        .any(|s| s.start <= pos && s.end >= end)
                {
                    inner.uncertain_spans.push(ByteSpan { start: pos, end });
                }
            }
        }
        held.append(&mut inner.held);
        let mut forwarded = read.forwarded;
        forwarded.append(&mut inner.forwarded);
        inner.removed = removed;
        inner.held = held;
        inner.forwarded = forwarded;
        inner.uncertain_spans.sort_by_key(|s| (s.start, s.end));
        inner.text_source = "html".into();
        return inner;
    };
    let plain = plain.replace("\r\n", "\n");
    let (text, source) = match flowed {
        Some(delsp) => (unflow(&plain, delsp), "plain_flowed"),
        None => (plain, "plain"),
    };
    let side = html_read
        .as_ref()
        .map(|r| HtmlSide {
            quote: &r.quote_text,
            novel: &r.novel_text,
        })
        .unwrap_or_default();
    let sig = html_read.as_ref().map_or("", |r| r.sig_text.as_str());
    let mut out = from_plain(&text, ev, side, sig, is_reply);
    if out.forwarded.is_empty()
        && let Some(read) = html_read
        && !read.forwarded.is_empty()
    {
        out.forwarded = read.forwarded;
    }
    out.text_source = source.into();
    out
}

fn from_plain(
    text: &str,
    ev: &Evidence<'_>,
    hs: HtmlSide<'_>,
    html_sig: &str,
    is_reply: bool,
) -> Extracted {
    let lines: Vec<&str> = text.split('\n').map(|l| l.trim_end_matches('\r')).collect();
    let n = lines.len();
    let mut w = Work {
        keep: vec![true; n],
        uncertain: vec![false; n],
        lines,
        removed: Vec::new(),
        held: Vec::new(),
        forwarded: Vec::new(),
    };
    quote_blocks(&mut w, 0..n, 1, ev, hs);
    bottom_regions(&mut w, ev, hs, is_reply);
    unmarked_paragraphs(&mut w, ev, hs);
    signatures(&mut w, ev, html_sig);
    let (assembled, uncertain_spans) = assemble(&w);
    Extracted {
        text: assembled,
        text_source: String::new(),
        removed: w.removed,
        held: w.held,
        uncertain_spans,
        forwarded: w.forwarded,
        extractor: EXTRACTOR_VERSION.into(),
    }
}

/// Blocks after the last `-- ` closing three or more of the owner's sent
/// messages: recurrence nominates a signature; the delimiter must agree.
#[must_use]
pub fn recurring_signatures(plains: &[String]) -> BTreeSet<String> {
    let mut counts: HashMap<String, usize> = HashMap::new();
    for p in plains {
        let lines: Vec<&str> = p.split('\n').map(|l| l.trim_end_matches('\r')).collect();
        if let Some(d) = lines.iter().rposition(|l| *l == "-- ") {
            let block = loose(&lines[d + 1..].join("\n"));
            if !block.is_empty() {
                *counts.entry(block).or_default() += 1;
            }
        }
    }
    counts
        .into_iter()
        .filter(|(_, c)| *c >= 3)
        .map(|(b, _)| b)
        .collect()
}
