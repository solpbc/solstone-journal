// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use regex::Regex;
use std::sync::LazyLock;

static CSI: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^\x1b\[([0-9;:]*)m").unwrap());
static OSC: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^\x1b\]8;([^;]*);([^\x07\x1b]*)(?:\x07|\x1b\\)").unwrap());

pub(crate) fn render(text: &str) -> Result<String, &'static str> {
    let mut chunks = Vec::new();
    let mut cursor = 0;
    let mut foreground: Option<String> = None;
    let mut strike = false;
    let mut link: Option<String> = None;

    let mut pending = Vec::new();
    let mut prior: Option<(Option<String>, bool, Option<String>)> = None;

    let flush = |pending: &mut Vec<&str>,
                 prior: &Option<(Option<String>, bool, Option<String>)>,
                 chunks: &mut Vec<String>| {
        if pending.is_empty() {
            return;
        }
        let raw = pending.concat();
        pending.clear();
        let mut value = html_escape(&raw);
        if let Some((fg, deleted, url)) = prior {
            let substantive = raw
                .chars()
                .any(|c| c.is_alphanumeric() || "+-!?$/".contains(c));
            if let Some(fg_code) = fg
                && substantive
            {
                value = format!("<color sgr=\"{fg_code}\">{value}</color>");
            }
            if *deleted {
                value = format!("<strike>{value}</strike>");
            }
            if let Some(url_str) = url {
                let escaped_url = html_escape_quote(url_str);
                value = format!("<a href=\"{escaped_url}\">{value}</a>");
            }
        }
        chunks.push(value);
    };

    while cursor < text.len() {
        let remaining = &text[cursor..];
        if !remaining.starts_with('\x1b') {
            let end_rel = remaining.find('\x1b').unwrap_or(remaining.len());
            let val = &remaining[..end_rel];
            for c in val.chars() {
                let u = c as u32;
                if (u < 32 && c != '\n' && c != '\r' && c != '\t') || (0x7f..0xa0).contains(&u) {
                    return Err("unsupported control");
                }
            }
            let state = (foreground.clone(), strike, link.clone());
            if prior.as_ref() != Some(&state) {
                flush(&mut pending, &prior, &mut chunks);
                prior = Some(state);
            }
            pending.push(val);
            cursor += end_rel;
            continue;
        }

        if let Some(m) = CSI.find(remaining) {
            let sgr_str = CSI.captures(remaining).unwrap().get(1).unwrap().as_str();
            if sgr_str.contains(':') {
                return Err("noncanonical colon SGR requires original input");
            }
            let parts: Vec<&str> = sgr_str.split(';').collect();
            let mut params = Vec::new();
            for p in parts {
                params.push(if p.is_empty() {
                    0
                } else {
                    p.parse::<u32>().map_err(|_| "invalid SGR parameter")?
                });
            }

            let mut i = 0;
            while i < params.len() {
                let code = params[i];
                i += 1;
                if code == 0 {
                    foreground = None;
                    strike = false;
                } else if code == 8 {
                    return Err("conceal styling requires original input");
                } else if code == 9 {
                    strike = true;
                } else if code == 29 {
                    strike = false;
                } else if code == 39 {
                    foreground = None;
                } else if (30..=37).contains(&code) || (90..=97).contains(&code) {
                    foreground = Some(code.to_string());
                } else if code == 38 || code == 48 {
                    if i >= params.len() {
                        return Err("incomplete extended color");
                    }
                    let mode = params[i];
                    let n = match mode {
                        5 => 2,
                        2 => 4,
                        _ => return Err("unsupported extended color"),
                    };
                    if i + n > params.len() {
                        return Err("unsupported extended color");
                    }
                    let values = &params[i..i + n];
                    i += n;
                    if code == 38 {
                        let mut full = vec![code];
                        full.extend_from_slice(values);
                        let sgr_val = full
                            .iter()
                            .map(|v| v.to_string())
                            .collect::<Vec<_>>()
                            .join(";");
                        foreground = Some(sgr_val);
                    }
                } else if !matches!(
                    code,
                    1 | 2
                        | 3
                        | 4
                        | 5
                        | 6
                        | 7
                        | 21
                        | 22
                        | 23
                        | 24
                        | 25
                        | 27
                        | 28
                        | 49
                        | 53
                        | 55
                        | 40..=47
                        | 100..=107
                ) {
                    return Err("unsupported SGR");
                }
            }
            cursor += m.end();
            continue;
        }

        if let Some(m) = OSC.find(remaining) {
            let caps = OSC.captures(remaining).unwrap();
            let url = caps.get(2).map(|c| c.as_str()).unwrap_or("");
            link = if url.is_empty() {
                None
            } else {
                Some(url.to_owned())
            };
            cursor += m.end();
            continue;
        }

        return Err("unsupported escape");
    }

    flush(&mut pending, &prior, &mut chunks);
    Ok(chunks.concat())
}

fn html_escape(raw: &str) -> String {
    let mut out = String::new();
    for c in raw.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            _ => out.push(c),
        }
    }
    out
}

fn html_escape_quote(raw: &str) -> String {
    let mut out = String::new();
    for c in raw.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            _ => out.push(c),
        }
    }
    out
}
