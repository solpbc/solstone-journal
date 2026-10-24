// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use super::{
    digest, prompt_attention, prompt_lines, readable_terminal, state_codec::split_lines_keepends,
};
use regex::Regex;
use serde_json::{Value, json};
use std::sync::LazyLock;

static CSI: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\x1b\[([0-9;:]*)m").unwrap());
static OSC: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\x1b\]8;([^;]*);([^\x07\x1b]*)(?:\x07|\x1b\\)").unwrap());
static OPTION: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^\s*(?:❯\s*)?\d+\.\s").unwrap());
static HIGHLIGHT: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^\s*❯\s*\d+\.\s").unwrap());
static PROMPT: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^[ \t]*❯[ \u{00a0}]?").unwrap());

pub(super) fn plain(text: &str) -> Result<String, &'static str> {
    readable_terminal::render(text)?;
    let value = OSC.replace_all(text, "");
    let value = CSI.replace_all(&value, "").into_owned();
    if value.contains('\x1b') {
        return Err("unknown terminal control");
    }
    Ok(value)
}

#[derive(Clone, Debug)]
pub(super) struct Bound {
    pub kind: &'static str,
    pub current: String,
    pub current_sha: String,
    pub screen_sha: String,
    pub timestamp: Value,
    pub scope: Value,
    pub screen: String,
    pub start: usize,
}

fn box_evidence(text: &str, start: usize, end: usize) -> bool {
    let Ok(display) = plain(text) else {
        return false;
    };
    let raw = split_lines_keepends(text);
    let lines = split_lines_keepends(&display);
    if raw.len() != lines.len() || !(0 < start && start < end && end < lines.len()) {
        return false;
    }
    if !lines[start].trim_start().starts_with('❯') {
        return false;
    }
    let border = |line: &str| {
        let s = line.trim();
        s.chars().count() >= 20 && s.chars().all(|c| c == '─' || c == '━')
    };
    if !border(&lines[start - 1])
        || !border(&lines[end])
        || lines[start - 1].trim().chars().count() != lines[end].trim().chars().count()
    {
        return false;
    }
    let tail = &lines[end + 1..];
    tail.len() <= 8
        && tail
            .iter()
            .any(|s| s.contains("shift+tab to cycle") && s.contains("permissions"))
}

fn kind(text: &str, row: &Value) -> &'static str {
    let start = row["start_line"].as_u64().unwrap_or(u64::MAX) as usize;
    let end = row["end_line_exclusive"].as_u64().unwrap_or(u64::MAX) as usize;
    if box_evidence(text, start, end) {
        return "editable_input_box_visible";
    }
    let Ok(display) = plain(text) else {
        return "unclassified_display";
    };
    let lines = split_lines_keepends(&display);
    if lines.len() != split_lines_keepends(text).len() {
        return "unclassified_display";
    }
    let options: Vec<_> = lines
        .iter()
        .enumerate()
        .filter(|(_, s)| OPTION.is_match(s))
        .map(|(i, _)| i)
        .collect();
    let footer: Vec<_> = lines
        .iter()
        .enumerate()
        .filter(|(_, s)| {
            s.starts_with("Enter to select")
                && s.contains("↑/↓ to navigate")
                && s.contains("Esc to cancel")
        })
        .map(|(i, _)| i)
        .collect();
    if footer.len() == 1
        && footer[0] >= lines.len().saturating_sub(3)
        && options.len() >= 2
        && options.contains(&start)
        && HIGHLIGHT.is_match(&lines[start])
        && lines[..start].iter().any(|s| s.contains('☐'))
    {
        "selection_menu_with_highlight_visible"
    } else {
        "unclassified_display"
    }
}

/// Inspect full preceding SGR state; color arguments are not mode commands.
fn ambiguous(text: &str, start: usize, end: usize) -> bool {
    let mut flags = std::collections::BTreeSet::new();
    let mut cursor = 0;
    let mut marker = true;
    while cursor < end {
        let rest = &text[cursor..];
        if !rest.starts_with('\x1b') {
            let c = rest.chars().next().unwrap();
            if cursor >= start && !c.is_whitespace() {
                if marker && c == '❯' {
                    marker = false;
                } else {
                    marker = false;
                    if !flags.is_empty() {
                        return true;
                    }
                }
            }
            cursor += c.len_utf8();
            continue;
        }
        if let Some(c) = CSI
            .captures(rest)
            .filter(|c| c.get(0).unwrap().start() == 0)
        {
            if c[1].contains(':') {
                return true;
            }
            let Ok(params) = c[1]
                .split(';')
                .map(|p| {
                    if p.is_empty() {
                        Ok(0)
                    } else {
                        p.parse::<u32>()
                    }
                })
                .collect::<Result<Vec<_>, _>>()
            else {
                return true;
            };
            let mut i = 0;
            while i < params.len() {
                let code = params[i];
                i += 1;
                match code {
                    0 => flags.clear(),
                    2 | 7 | 8 | 9 => {
                        flags.insert(code);
                    }
                    22 | 27 | 28 | 29 => {
                        flags.remove(&(if code == 22 { 2 } else { code - 20 }));
                    }
                    38 | 48 => {
                        let n = match params.get(i) {
                            Some(2) => 4,
                            Some(5) => 2,
                            _ => return true,
                        };
                        if i + n > params.len() {
                            return true;
                        }
                        i += n;
                    }
                    _ => {}
                }
            }
            cursor += c.get(0).unwrap().end();
        } else if let Some(link) = OSC
            .find(rest)
            .filter(|m| m.start() == 0 && cursor + m.end() <= start)
        {
            cursor += link.end();
        } else {
            return true;
        }
    }
    false
}

pub(super) fn choose(frames: &[Value]) -> Option<Bound> {
    let screens = prompt_attention::select(frames, 24000, 4);
    let inputs = prompt_lines::excerpts(frames, 6000);
    let selected = inputs.last()?;
    let timestamp = &selected["timestamp"];
    let scope = selected["scope"].as_array()?;
    let last: Vec<_> = frames
        .iter()
        .filter(|f| {
            f["timestamp"] == *timestamp
                && f["content"]["tmux"]["session"] == scope[0]
                && f["content"]["tmux"]["window"]["id"] == scope[1]
        })
        .collect();
    if last.len() != 1 {
        return None;
    }
    let tmux = &last[0]["content"]["tmux"];
    let active: Vec<_> = tmux["panes"]
        .as_array()?
        .iter()
        .filter(|p| p["active"] == true)
        .collect();
    if active.len() != 1 {
        return None;
    }
    let actual_scope = json!([tmux["session"], tmux["window"]["id"], active[0]["id"]]);
    let matches: Vec<_> = inputs
        .iter()
        .filter(|r| r["timestamp"] == *timestamp && r["scope"] == actual_scope)
        .collect();
    if matches.len() != 1 {
        return None;
    }
    let input = matches[0];
    let screen = screens.iter().find(|s| {
        s["timestamp"] == *timestamp && s["scope"] == actual_scope && s["sha256"] == input["sha256"]
    })?;
    let text = screen["text"].as_str()?;
    let kind = kind(text, input);
    let start = input["start_utf8_byte"].as_u64()? as usize;
    let current = match kind {
        "selection_menu_with_highlight_visible" => {
            "Reviewed the available menu options.".to_owned()
        }
        "editable_input_box_visible" => {
            let raw = input["text"].as_str()?;
            if raw.contains("\x1b]") {
                return None;
            }
            let displayed = readable_terminal::render(raw).ok()?;
            if ["<strike>", "<reverse>", "<conceal>"]
                .iter()
                .any(|tag| displayed.contains(tag))
            {
                return None;
            }
            let input_text = plain(raw).ok()?;
            let input_text = PROMPT.replace(&input_text, "");
            let input_text = input_text.trim_end_matches(['\r', '\n']);
            if input_text.trim().is_empty()
                || input_text.len() > 600
                || ambiguous(text, start, start + raw.len())
            {
                return None;
            }
            format!("Composed this input: “{input_text}”")
        }
        _ => return None,
    };
    Some(Bound {
        kind,
        current_sha: digest(&current),
        current,
        screen_sha: screen["sha256"].as_str()?.to_owned(),
        timestamp: timestamp.clone(),
        scope: actual_scope,
        screen: text.to_owned(),
        start,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn style_state_precedes_field_and_rgb_arguments_are_not_flags() {
        for mode in [2, 7, 8, 9] {
            let text = format!("\x1b[{mode}mrest\n❯ question");
            assert!(ambiguous(&text, text.find('❯').unwrap(), text.len()));
        }
        let text = "\x1b[38;2;7;8;9mrest\n❯ question";
        assert!(!ambiguous(text, text.find('❯').unwrap(), text.len()));
        let text = "\x1b[48;5;9mrest\n❯ question";
        assert!(!ambiguous(text, text.find('❯').unwrap(), text.len()));
    }
}
