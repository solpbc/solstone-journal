// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! What one tmux session displayed during a segment, as entries a reader can
//! follow: each pane's screen when first seen, then only the lines that later
//! appeared on it, each line once, and the input the owner typed.
//!
//! Captures repeat a whole screen every few seconds, so most of a raw segment
//! is the same lines again, wrapped in terminal styling codes. Here a line is
//! shown at its first appearance only. A line rewritten in place (a spinner, a
//! timer, a progress count, an input line being typed) keeps one entry that
//! shows its latest version. Styling is dropped except strikethrough, which
//! agent task lists use to mark finished steps, and dim text after the cursor,
//! which is a placeholder or suggestion rather than typing, is left out.

use super::cursor;
use super::diff_matcher::{OpcodeTag, SequenceMatcher};
use regex::Regex;
use serde_json::Value;
use std::collections::{HashMap, HashSet};
use std::sync::LazyLock;

static SGR: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^\x1b\[([0-9;:]*)m").unwrap());
/// Any other escape: CSI, OSC (hyperlink targets included; their label stays
/// as text) and two-byte forms.
static ESCAPE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^\x1b(?:\[[0-?]*[ -/]*[@-~]|\][^\x07\x1b]*(?:\x07|\x1b\\)?|[ -~])").unwrap()
});

/// A run of one rule or border character this long is drawn, not written.
const RULE_RUN: usize = 8;
/// At most this many earlier rows are replaced when lines are rewritten in place.
const IN_PLACE_ROWS: usize = 3;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Kind {
    /// A pane's whole screen at its first capture in the segment.
    FirstSeen,
    /// Lines that appeared on a pane since its previous capture.
    NewText,
    /// Input the owner typed on a pane.
    Typed,
}

#[derive(Clone, Debug)]
pub(super) struct Item {
    pub kind: Kind,
    /// Seconds from the segment start.
    pub time: f64,
    pub pane: usize,
    pub lines: Vec<String>,
}

#[derive(Clone, Debug)]
pub(super) struct Pane {
    pub window: String,
    pub id: String,
}

#[derive(Clone, Debug, Default)]
pub(super) struct Session {
    pub name: String,
    pub windows: Vec<String>,
    pub panes: Vec<Pane>,
    pub items: Vec<Item>,
    pub captures: usize,
    /// Typed inputs left out because all typed input exceeded its budget.
    pub typed_left_out: usize,
}

struct PaneState {
    index: usize,
    previous: Vec<String>,
    seen: HashSet<String>,
    /// Where each line shown in a new-text entry sits, so an in-place rewrite
    /// can update it: (item index, line index).
    shown: HashMap<String, (usize, usize)>,
}

/// One displayed row as text. `dim_from` drops dim text from that column on;
/// `strike` marks struck-through runs as `~~text~~`.
pub(super) fn display_line(row: &str, dim_from: Option<usize>, strike: bool) -> String {
    let mut out = String::new();
    let mut run = String::new();
    let (mut column, mut at) = (0usize, 0usize);
    let (mut dim, mut conceal, mut struck) = (false, false, false);
    let flush = |out: &mut String, run: &mut String, struck: bool| {
        if run.is_empty() {
            return;
        }
        if struck && strike && run.trim().chars().any(char::is_alphanumeric) {
            let lead = run.len() - run.trim_start().len();
            let body = run.trim();
            out.push_str(&run[..lead]);
            out.push_str("~~");
            out.push_str(body);
            out.push_str("~~");
            out.push_str(&run[lead + body.len()..]);
        } else {
            out.push_str(run);
        }
        run.clear();
    };
    while at < row.len() {
        let rest = &row[at..];
        if let Some(captures) = SGR.captures(rest) {
            let was = struck;
            let mut codes = captures[1]
                .split([';', ':'])
                .map(|p| p.parse::<u32>().unwrap_or(0));
            while let Some(code) = codes.next() {
                match code {
                    0 => (dim, conceal, struck) = (false, false, false),
                    2 => dim = true,
                    8 => conceal = true,
                    9 => struck = true,
                    22 => dim = false,
                    28 => conceal = false,
                    29 => struck = false,
                    38 | 48 | 58 => match codes.next() {
                        Some(5) => {
                            codes.next();
                        }
                        Some(2) => {
                            codes.nth(2);
                        }
                        _ => {}
                    },
                    _ => {}
                }
            }
            if was != struck {
                flush(&mut out, &mut run, was);
            }
            at += captures[0].len();
            continue;
        }
        if rest.starts_with('\x1b') {
            at += ESCAPE.find(rest).map_or(1, |m| m.end());
            continue;
        }
        let c = rest.chars().next().unwrap_or(' ');
        at += c.len_utf8();
        let c = if c == '\t' { ' ' } else { c };
        if c.is_control() {
            continue;
        }
        if !conceal && !(dim && dim_from.is_some_and(|x| column >= x)) {
            run.push(c);
        }
        column += 1;
    }
    flush(&mut out, &mut run, struck);
    collapse_rules(out.trim_end())
}

fn is_rule(c: char) -> bool {
    matches!(
        c,
        '\u{2500}'..='\u{259f}' | '-' | '=' | '_' | '.' | '·' | '•' | '*' | '~' | '…'
    )
}

fn collapse_rules(text: &str) -> String {
    let chars: Vec<char> = text.chars().collect();
    let mut out = String::with_capacity(text.len());
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        let mut j = i;
        while j < chars.len() && chars[j] == c {
            j += 1;
        }
        let n = if is_rule(c) && j - i >= RULE_RUN {
            3
        } else {
            j - i
        };
        out.extend(std::iter::repeat_n(c, n));
        i = j;
    }
    out
}

/// A line rewritten in place keeps its identity: the same text apart from
/// numbers and a leading spinner glyph, or the same text grown or shortened
/// at its end.
fn rewritten(old: &str, new: &str) -> bool {
    fn key(text: &str) -> String {
        let text = text.trim_start_matches(|c: char| !c.is_alphanumeric());
        let mut out = String::new();
        let mut digits = false;
        for c in text.chars() {
            if c.is_ascii_digit() {
                if !digits {
                    out.push('0');
                }
                digits = true;
            } else {
                digits = false;
                out.push(c);
            }
        }
        out
    }
    let (old, new) = (old.trim(), new.trim());
    if old.is_empty() || new.is_empty() {
        return false;
    }
    key(old) == key(new)
        || (old.chars().count().min(new.chars().count()) >= 2
            && (new.starts_with(old) || old.starts_with(new)))
}

/// Blank lines collapse to one and never start or end an entry.
fn tidy(lines: Vec<String>) -> Vec<String> {
    let mut out: Vec<String> = Vec::with_capacity(lines.len());
    for line in lines {
        if line.trim().is_empty() {
            if out.last().is_some_and(|l| !l.is_empty()) {
                out.push(String::new());
            }
        } else {
            out.push(line);
        }
    }
    while out.last().is_some_and(String::is_empty) {
        out.pop();
    }
    out
}

fn pane_lines(pane: &Value) -> Vec<String> {
    let content = pane["content"].as_str().unwrap_or("");
    let cursor_row =
        (pane["active"] == true && pane["cursor_visible"] == true && pane["pane_in_mode"] == false)
            .then(|| Some((pane["cursor_y"].as_u64()?, pane["cursor_x"].as_u64()?)))
            .flatten();
    cursor::split_lines(content)
        .iter()
        .enumerate()
        .map(|(row, line)| {
            let dim_from = cursor_row
                .filter(|(y, _)| *y as usize == row)
                .map(|(_, x)| x as usize);
            display_line(line.trim_end_matches(['\r', '\n']), dim_from, true)
        })
        .collect()
}

/// Read one session's frames, in time order, into entries.
pub(super) fn read(name: &str, frames: &[Value], typed_budget_bytes: usize) -> Session {
    let mut session = Session {
        name: name.to_owned(),
        captures: frames.len(),
        ..Default::default()
    };
    let mut states: HashMap<(String, String), PaneState> = HashMap::new();
    for frame in frames {
        let time = frame["timestamp"].as_f64().unwrap_or(0.0);
        let tmux = &frame["content"]["tmux"];
        let window_id = tmux["window"]["id"].as_str().unwrap_or("").to_owned();
        let window = tmux["window"]["name"]
            .as_str()
            .unwrap_or(&window_id)
            .to_owned();
        for listed in tmux["windows"].as_array().into_iter().flatten() {
            if let Some(listed) = listed["name"].as_str()
                && !session.windows.iter().any(|w| w == listed)
            {
                session.windows.push(listed.to_owned());
            }
        }
        if !session.windows.contains(&window) {
            session.windows.push(window.clone());
        }
        for pane in tmux["panes"].as_array().into_iter().flatten() {
            let id = pane["id"].as_str().unwrap_or("").to_owned();
            let lines = pane_lines(pane);
            let Some(state) = states.get_mut(&(window_id.clone(), id.clone())) else {
                let index = session.panes.len();
                session.panes.push(Pane {
                    window: window.clone(),
                    id: id.clone(),
                });
                let seen = lines
                    .iter()
                    .filter(|l| !l.trim().is_empty())
                    .cloned()
                    .collect();
                session.items.push(Item {
                    kind: Kind::FirstSeen,
                    time,
                    pane: index,
                    lines: tidy(lines.clone()),
                });
                states.insert(
                    (window_id.clone(), id),
                    PaneState {
                        index,
                        previous: lines,
                        seen,
                        shown: HashMap::new(),
                    },
                );
                continue;
            };
            if lines == state.previous {
                continue;
            }
            let mut matcher = SequenceMatcher::new(&state.previous, &lines);
            let opcodes = matcher.get_opcodes().to_vec();
            let mut fresh: Vec<String> = Vec::new();
            for op in opcodes {
                if !matches!(op.tag, OpcodeTag::Replace | OpcodeTag::Insert) {
                    continue;
                }
                if !fresh.is_empty() {
                    fresh.push(String::new());
                }
                for (k, line) in lines[op.j1..op.j2].iter().enumerate() {
                    if line.trim().is_empty() || state.seen.contains(line) {
                        continue;
                    }
                    state.seen.insert(line.clone());
                    // A status line, a spinner or an input being typed rewrites a row or
                    // two in place; a screen of new lines is new, however alike.
                    let in_place = op.tag == OpcodeTag::Replace
                        && op.i2 - op.i1 <= IN_PLACE_ROWS
                        && op.i1 + k < op.i2;
                    let old = in_place.then(|| &state.previous[op.i1 + k]);
                    if let Some(old) = old.filter(|old| rewritten(old, line))
                        && let Some((item, at)) = state.shown.remove(old)
                    {
                        session.items[item].lines[at].clone_from(line);
                        state.shown.insert(line.clone(), (item, at));
                        continue;
                    }
                    fresh.push(line.clone());
                }
            }
            let fresh = tidy(fresh);
            if !fresh.is_empty() {
                let item = session.items.len();
                for (at, line) in fresh.iter().enumerate() {
                    if !line.is_empty() {
                        state.shown.insert(line.clone(), (item, at));
                    }
                }
                session.items.push(Item {
                    kind: Kind::NewText,
                    time,
                    pane: state.index,
                    lines: fresh,
                });
            }
            state.previous = lines;
        }
    }
    let mut typed = cursor::typed(frames);
    let mut total = 0;
    let keep = typed
        .iter()
        .rev()
        .take_while(|t| {
            total += t.text().len();
            total <= typed_budget_bytes
        })
        .count();
    session.typed_left_out = typed.len() - keep;
    typed.drain(..typed.len() - keep);
    for input in typed {
        let Some(state) = states.get(&(input.window.clone(), input.pane.clone())) else {
            continue;
        };
        let text = input.text();
        // The pane's echo of a typed line is the same words: they show once, as typed.
        for item in &mut session.items {
            if item.kind == Kind::NewText && item.pane == state.index {
                item.lines.retain(|line| line.trim() != text);
                item.lines = tidy(std::mem::take(&mut item.lines));
            }
        }
        session.items.push(Item {
            kind: Kind::Typed,
            time: input.timestamp,
            pane: state.index,
            lines: vec![text],
        });
    }
    session
        .items
        .retain(|item| item.kind != Kind::NewText || !item.lines.is_empty());
    // Typed input sorts before the text that appeared at the same capture.
    session.items.sort_by(|a, b| {
        a.time
            .total_cmp(&b.time)
            .then_with(|| (a.kind != Kind::Typed).cmp(&(b.kind != Kind::Typed)))
    });
    session
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn frame(time: f64, panes: Value) -> Value {
        json!({"timestamp":time,"content":{"tmux":{"session":"s","window":{"id":"@1","name":"work"},"windows":[{"id":"@1","name":"work","active":true},{"id":"@2","name":"notes","active":false}],"panes":panes}}})
    }
    fn pane(content: &str) -> Value {
        json!([{"id":"%0","active":true,"content":content}])
    }

    #[test]
    fn styling_is_dropped_except_strikethrough_and_typed_placeholders() {
        assert_eq!(
            display_line(
                "\x1b[1;32mok\x1b[0m \x1b]8;;https://x.test\x1b\\link\x1b]8;;\x1b\\",
                None,
                true
            ),
            "ok link"
        );
        assert_eq!(
            display_line("\x1b[9m☒ Write tests\x1b[29m next", None, true),
            "~~☒ Write tests~~ next"
        );
        assert_eq!(
            display_line("> fix\x1b[2m the bug\x1b[22m", Some(5), true),
            "> fix"
        );
        assert_eq!(display_line("\x1b[2m> fix\x1b[22m", Some(5), true), "> fix");
        assert_eq!(display_line("╭────────────────╮", None, true), "╭───╮");
        assert_eq!(display_line("a\x1b[8msecret\x1b[28mb\t", None, true), "ab");
    }

    #[test]
    fn each_line_is_shown_once_and_a_line_rewritten_in_place_shows_its_latest_version() {
        let frames = vec![
            frame(0.0, pane("$ make\nbuilding\n")),
            frame(5.0, pane("$ make\nbuilding\n✻ Working (1s)\n")),
            frame(10.0, pane("$ make\nbuilding\n✶ Working (9s)\nerror: x\n")),
            frame(15.0, pane("$ make\nbuilding\n")),
            frame(20.0, pane("$ make\nbuilding\nerror: x\nwarning: y\n")),
        ];
        let s = read("s", &frames, 6000);
        let texts: Vec<_> = s
            .items
            .iter()
            .map(|i| (i.kind, i.lines.join("|")))
            .collect();
        assert_eq!(
            texts,
            [
                (Kind::FirstSeen, "$ make|building".to_owned()),
                (Kind::NewText, "✶ Working (9s)".to_owned()),
                (Kind::NewText, "error: x".to_owned()),
                (Kind::NewText, "warning: y".to_owned()),
            ]
        );
        assert_eq!(s.windows, ["work", "notes"]);
    }

    #[test]
    fn typed_input_is_an_entry_before_the_text_of_its_capture() {
        let typing = |time: f64, content: &str, x: u64, y: u64| {
            frame(
                time,
                json!([{"id":"%0","active":true,"content":content,"cursor_x":x,"cursor_y":y,"cursor_visible":true,"pane_in_mode":false}]),
            )
        };
        let frames = vec![
            typing(0.0, "> \n", 2, 0),
            typing(5.0, "> run the tests\n", 15, 0),
            typing(10.0, "> run the tests\n⏺ Running tests\n> \n", 2, 2),
        ];
        let s = read("s", &frames, 6000);
        let kinds: Vec<_> = s
            .items
            .iter()
            .map(|i| (i.kind, i.lines.join("|")))
            .collect();
        assert_eq!(
            kinds,
            [
                (Kind::FirstSeen, ">".to_owned()),
                (Kind::Typed, "> run the tests".to_owned()),
                (Kind::NewText, "⏺ Running tests".to_owned()),
            ]
        );
    }
}
