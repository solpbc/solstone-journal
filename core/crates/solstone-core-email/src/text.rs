// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Deterministic text matching helpers for quote/signature/forward detection.
//!
//! Normalization is only ever used for comparison; stored text is copied
//! verbatim from the message.

use similar::{Algorithm, DiffTag, capture_diff_slices};

fn is_cjk(c: char) -> bool {
    matches!(
        c as u32,
        0x3040..=0x30ff
            | 0x3400..=0x4dbf
            | 0x4e00..=0x9fff
            | 0xac00..=0xd7af
            | 0xf900..=0xfaff
            | 0x20000..=0x2fa1f
    )
}

/// Lowercased word tokens; CJK characters are one token each.
#[must_use]
pub fn tokens(s: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    for c in s.chars() {
        if is_cjk(c) {
            if !cur.is_empty() {
                out.push(std::mem::take(&mut cur));
            }
            out.push(c.to_string());
        } else if c.is_alphanumeric() {
            cur.extend(c.to_lowercase());
        } else if !cur.is_empty() {
            out.push(std::mem::take(&mut cur));
        }
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}

/// Share of `span`'s tokens that align in order with `reference`.
#[must_use]
pub fn coverage(span: &str, reference: &str) -> f64 {
    let a = tokens(span);
    if a.is_empty() {
        return 0.0;
    }
    let b = tokens(reference);
    if b.is_empty() {
        return 0.0;
    }
    let ops = capture_diff_slices(Algorithm::Myers, &a, &b);
    let equal: usize = ops
        .iter()
        .filter(|op| op.tag() == DiffTag::Equal)
        .map(|op| op.old_range().len())
        .sum();
    equal as f64 / a.len() as f64
}

/// Whitespace-collapsed, `>`-prefix-free form used for comparison and recurrence.
#[must_use]
pub fn loose(s: &str) -> String {
    s.lines()
        .map(|l| l.trim_start().trim_start_matches(['>', ' ']))
        .collect::<Vec<_>>()
        .join(" ")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

const ATTRIBUTION_ENDINGS: &[&str] = &[
    "wrote:",
    "writes:",
    "schrieb:",
    "a écrit :",
    "a écrit:",
    "escribió:",
    "ha scritto:",
    "schreef:",
    "skrev:",
    "kirjoitti:",
    "napisał:",
    "napisała:",
    "escreveu:",
    "написал:",
    "написала:",
    "写道：",
    "写道:",
    "님이 작성:",
];

const ATTRIBUTION_OPENERS: &[&str] = &[
    "on ", "am ", "le ", "el ", "il ", "op ", "em ", "den ", "w dniu ", "dne ",
];

fn has_clock_or_year(l: &str) -> bool {
    let b = l.as_bytes();
    let clock = b.windows(4).any(|w| {
        w[0].is_ascii_digit() && w[1] == b':' && w[2].is_ascii_digit() && w[3].is_ascii_digit()
    }) || b.windows(5).any(|w| {
        w[0].is_ascii_digit()
            && w[1].is_ascii_digit()
            && w[2] == b':'
            && w[3].is_ascii_digit()
            && w[4].is_ascii_digit()
    });
    let year = b
        .windows(4)
        .any(|w| w.iter().all(u8::is_ascii_digit) && (w[0] == b'1' || w[0] == b'2'));
    clock || year
}

/// An attribution line as mail clients write them ("On ... wrote:", localized
/// equivalents, or unprefixed date/address/colon lines).
#[must_use]
pub fn is_attribution(line: &str) -> bool {
    let l = line.trim();
    if l.len() < 6 || l.len() > 400 {
        return false;
    }
    let lower = l.to_lowercase();
    let angle_addr = l
        .find('<')
        .and_then(|i| l[i..].find('>').map(|j| &l[i..i + j]))
        .is_some_and(|inner| inner.contains('@'));
    if ATTRIBUTION_ENDINGS.iter().any(|e| lower.ends_with(e)) {
        return ATTRIBUTION_OPENERS.iter().any(|o| lower.starts_with(o)) || l.contains('@');
    }
    let ends_colon = l.ends_with(':') || l.ends_with('：');
    ends_colon && angle_addr && has_clock_or_year(l)
}

const HEADER_KEYS: &[&str] = &[
    "from:",
    "sent:",
    "date:",
    "to:",
    "subject:",
    "cc:",
    "von:",
    "gesendet:",
    "an:",
    "betreff:",
    "de :",
    "envoyé :",
    "à :",
    "objet :",
    "de:",
    "envoyé:",
    "à:",
    "objet:",
    "enviado:",
    "enviado el:",
    "para:",
    "assunto:",
    "asunto:",
    "inviato:",
    "da:",
    "oggetto:",
    "van:",
    "verzonden:",
    "aan:",
    "onderwerp:",
];

#[must_use]
pub fn header_key(line: &str) -> Option<&'static str> {
    let l = line.trim_start().to_lowercase();
    HEADER_KEYS.iter().copied().find(|k| l.starts_with(k))
}

/// A From/Sent/To/Subject header block starting at `i`: returns its exclusive
/// end index when it opens with From and holds at least two other header keys.
#[must_use]
pub fn header_block(lines: &[&str], i: usize) -> Option<usize> {
    let first = header_key(lines.get(i)?)?;
    if !matches!(first, "from:" | "von:" | "de :" | "de:" | "da:" | "van:") {
        return None;
    }
    let mut j = i + 1;
    let mut others = 0;
    while j < lines.len() && j < i + 8 {
        match header_key(lines[j]) {
            Some(_) => {
                others += 1;
                j += 1;
            }
            None => break,
        }
    }
    (others >= 2).then_some(j)
}

#[must_use]
pub fn is_forward_marker(line: &str) -> bool {
    let l = line.trim().to_lowercase();
    (l.starts_with("-----") && l.contains("forwarded message") && l.ends_with('-'))
        || l == "begin forwarded message:"
        || (l.contains("original message") && l.starts_with("-----") && l.ends_with("-----"))
}

#[must_use]
pub fn is_rule(line: &str) -> bool {
    let l = line.trim();
    l.len() >= 10 && l.chars().all(|c| c == '_')
}

const MOBILE_FOOTERS: &[&str] = &[
    "sent from my iphone",
    "sent from my ipad",
    "sent from my android",
    "sent from my android phone",
    "sent from my galaxy",
    "sent from my pixel",
    "sent from my mobile",
    "sent from mail for windows",
    "get outlook for ios",
    "get outlook for android",
    "sent from outlook for ios",
    "sent from outlook for android",
    "sent from yahoo mail on android",
    "sent from yahoo mail for iphone",
    "sent from proton mail mobile",
    "sent with proton mail secure email",
];

#[must_use]
pub fn is_mobile_footer(line: &str) -> bool {
    let l = line.trim().trim_end_matches('.').to_lowercase();
    MOBILE_FOOTERS.contains(&l.as_str())
}

/// Whether a subject line starts with a standard forward prefix (`Fwd:`, `Fw:`,
/// `Wg:`, `Tr:`, `Rv:`, `Enc:`, `I:`).
#[must_use]
pub fn is_forward_subject(subject: Option<&str>) -> bool {
    let Some(s) = subject.map(str::trim).filter(|s| !s.is_empty()) else {
        return false;
    };
    let lower = s.to_lowercase();
    ["fwd:", "fw:", "wg:", "tr:", "rv:", "enc:", "i:"]
        .iter()
        .any(|prefix| lower.starts_with(prefix))
}

/// Strip reply/forward prefixes for subject comparison.
#[must_use]
pub fn bare_subject(s: &str) -> String {
    let mut t = s.trim();
    loop {
        let lower = t.to_lowercase();
        let cut = [
            "re:", "fwd:", "fw:", "aw:", "wg:", "tr:", "sv:", "antw:", "rv:", "réf:", "enc:", "i:",
        ]
        .iter()
        .find(|p| lower.starts_with(*p))
        .map(|p| p.len());
        match cut {
            Some(n) => t = t[n..].trim_start(),
            None => break,
        }
    }
    t.to_lowercase()
}
