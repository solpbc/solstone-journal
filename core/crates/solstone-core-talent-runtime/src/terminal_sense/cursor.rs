// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! The owner's typed input, read from where the terminal's cursor sits.
//!
//! A pane whose cursor is visible, and which tmux is not showing in copy or
//! another mode, takes keyboard input on the cursor's row. That holds for a
//! shell prompt and for a full-screen application's input field alike, so no
//! application's screen layout is recognized here. Text after the cursor in a
//! dim style is a placeholder or a suggestion, not typing, and is left out.
//! A row counts as typed input only once it changes under the cursor, so an
//! empty prompt is not mistaken for one; a command typed and sent between two
//! captures is read from the row the cursor just left.

use super::{digest, readable_terminal, state_codec::split_lines_keepends};
use regex::Regex;
use serde_json::{Value, json};
use std::sync::LazyLock;

pub(super) static CSI: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\x1b\[[0-?]*[ -/]*[@-~]").unwrap());
static SGR: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\x1b\[([0-9;:]*)m").unwrap());
static OSC: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\x1b\]8;([^;]*);([^\x07\x1b]*)(?:\x07|\x1b\\)").unwrap());

/// Inputs whose total text exceeds this keep only the newest that fit.
const INPUT_BUDGET_BYTES: usize = 6000;
/// A longer cursor row is screen output the cursor happens to sit on.
const INPUT_MAX_BYTES: usize = 600;

pub(super) fn plain(text: &str) -> Result<String, &'static str> {
    readable_terminal::render(text)?;
    let value = OSC.replace_all(text, "");
    let value = SGR.replace_all(&value, "").into_owned();
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

/// One typed input: the longest text the cursor row held while each reading
/// only extended or shortened the one before it.
#[derive(Clone, Debug)]
struct Typed {
    timestamp: Value,
    scope: Value,
    text: String,
    screen: String,
    start: usize,
    row: usize,
    changed: bool,
}

/// The cursor row's typed text and the row's byte offset in the pane text.
fn typed(pane: &Value) -> Option<(String, usize)> {
    if pane["cursor_visible"] != true || pane["pane_in_mode"] != false {
        return None;
    }
    let x = usize::try_from(pane["cursor_x"].as_u64()?).ok()?;
    let y = usize::try_from(pane["cursor_y"].as_u64()?).ok()?;
    row_text(pane, y, x)
}

/// A row's text, leaving out concealed text and dim text from column `x` on.
fn row_text(pane: &Value, y: usize, x: usize) -> Option<(String, usize)> {
    let content = pane["content"].as_str()?;
    let lines = split_lines_keepends(content);
    let row = lines.get(y)?;
    let start: usize = lines[..y].iter().map(String::len).sum();
    let row = OSC.replace_all(row.trim_end_matches(['\r', '\n']), "");
    let mut text = String::new();
    let (mut column, mut cursor, mut dim, mut conceal) = (0, 0, false, false);
    while cursor < row.len() {
        let rest = &row[cursor..];
        if let Some(m) = SGR.find(rest).filter(|m| m.start() == 0) {
            let params = &rest[2..m.end() - 1];
            let mut codes = params
                .split([';', ':'])
                .map(|p| p.parse::<u32>().unwrap_or(0));
            while let Some(code) = codes.next() {
                match code {
                    0 => (dim, conceal) = (false, false),
                    2 => dim = true,
                    8 => conceal = true,
                    22 => dim = false,
                    28 => conceal = false,
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
            cursor += m.end();
            continue;
        }
        if rest.starts_with('\x1b') {
            return None;
        }
        let c = rest.chars().next()?;
        if !conceal && !(dim && column >= x) {
            text.push(c);
        }
        column += 1;
        cursor += c.len_utf8();
    }
    let text = text.trim().to_owned();
    if text.is_empty() || text.len() > INPUT_MAX_BYTES {
        return None;
    }
    Some((text, start))
}

fn history(frames: &[Value]) -> Vec<Typed> {
    let mut ordered: Vec<&Value> = frames.iter().collect();
    ordered.sort_by(|a, b| {
        a["timestamp"]
            .as_f64()
            .unwrap_or(0.0)
            .total_cmp(&b["timestamp"].as_f64().unwrap_or(0.0))
    });
    let mut out: Vec<Typed> = Vec::new();
    for frame in ordered {
        let tmux = &frame["content"]["tmux"];
        let Some(panes) = tmux["panes"].as_array() else {
            continue;
        };
        for pane in panes.iter().filter(|p| p["active"] == true) {
            let Some((text, start)) = typed(pane) else {
                continue;
            };
            let scope = json!([tmux["session"], tmux["window"]["id"], pane["id"]]);
            let row = usize::try_from(pane["cursor_y"].as_u64().unwrap_or(0)).unwrap_or(0);
            let screen = pane["content"].as_str().unwrap_or("").to_owned();
            let last = out.iter().rposition(|t| t.scope == scope);
            // The cursor left a row: what that row shows now is what was sent.
            if let Some(i) = last.filter(|&i| out[i].row != row)
                && let Some((sent, sent_start)) = row_text(pane, out[i].row, usize::MAX)
                && sent.len() > out[i].text.len()
                && sent.starts_with(&out[i].text)
            {
                out[i].text = sent;
                out[i].start = sent_start;
                out[i].screen.clone_from(&screen);
                out[i].timestamp = frame["timestamp"].clone();
                out[i].changed = true;
            }
            let reading = Typed {
                timestamp: frame["timestamp"].clone(),
                scope,
                text,
                screen,
                start,
                row,
                changed: false,
            };
            let same = last.filter(|&i| {
                let t = &out[i];
                t.row == reading.row
                    && (t.text.starts_with(&reading.text) || reading.text.starts_with(&t.text))
            });
            match same {
                Some(i) if out[i].text == reading.text => {}
                Some(i) if out[i].text.len() > reading.text.len() => out[i].changed = true,
                Some(i) => {
                    let mut grown = reading;
                    grown.changed = true;
                    out.remove(i);
                    out.push(grown);
                }
                None => {
                    // New text on the row the cursor was already on is typing too.
                    let mut reading = reading;
                    reading.changed = last.is_some_and(|i| out[i].row == reading.row);
                    out.push(reading);
                }
            }
        }
    }
    out.retain(|t| t.changed);
    out
}

/// The latest typed input, and every typed input as issued cards, newest last.
pub(super) fn choose(frames: &[Value]) -> Option<(Bound, Vec<Value>)> {
    let mut inputs = history(frames);
    let latest = inputs.last()?.clone();
    let mut total = 0;
    let keep = inputs
        .iter()
        .rev()
        .take_while(|t| {
            total += t.text.len();
            total <= INPUT_BUDGET_BYTES
        })
        .count();
    inputs.drain(..inputs.len() - keep);
    let cards = inputs
        .iter()
        .map(|t| json!({"timestamp":t.timestamp,"scope":t.scope,"text":t.text}))
        .collect();
    let current = format!("Composed this input: “{}”", latest.text);
    Some((
        Bound {
            kind: "cursor_input_line",
            current_sha: digest(&current),
            current,
            screen_sha: digest(&latest.screen),
            timestamp: latest.timestamp,
            scope: latest.scope,
            screen: latest.screen,
            start: latest.start,
        },
        cards,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pane(content: &str, x: u64, y: u64, visible: bool, mode: bool) -> Value {
        json!({"id":"%0","active":true,"content":content,"cursor_x":x,"cursor_y":y,"cursor_visible":visible,"alternate_on":true,"pane_in_mode":mode})
    }
    fn frame(time: f64, pane: Value) -> Value {
        json!({"timestamp":time,"content":{"tmux":{"session":"s","window":{"id":"@0"},"panes":[pane]}}})
    }

    #[test]
    fn the_cursor_row_is_the_input_and_dim_text_after_it_is_not() {
        let screen = "output line\n> fix the bu\x1b[2mg in parser\x1b[0m\nstatus bar\n";
        let (text, start) = typed(&pane(screen, 11, 1, true, false)).unwrap();
        assert_eq!(text, "> fix the bu");
        assert_eq!(start, "output line\n".len());
        // Dim text before the cursor was typed in a dim style, so it stays.
        let (text, _) = typed(&pane("\x1b[2m$ ls\x1b[0m\n", 4, 0, true, false)).unwrap();
        assert_eq!(text, "$ ls");
    }

    #[test]
    fn a_hidden_cursor_a_mode_a_blank_row_or_only_a_placeholder_is_no_input() {
        assert!(typed(&pane("\x1b[2mTry a command\x1b[0m\n", 0, 0, true, false)).is_none());
        assert!(typed(&pane("> typed\n", 7, 0, false, false)).is_none());
        assert!(typed(&pane("> typed\n", 7, 0, true, true)).is_none());
        assert!(typed(&pane("log line\n\n", 0, 1, true, false)).is_none());
        assert!(typed(&pane("> typed\n", 7, 5, true, false)).is_none());
        let mut old = pane("> typed\n", 7, 0, true, false);
        old.as_object_mut().unwrap().remove("cursor_x");
        assert!(typed(&old).is_none());
        assert!(typed(&pane("\x1b[8m> secret\x1b[0m\n", 8, 0, true, false)).is_none());
    }

    #[test]
    fn growing_text_is_one_input_and_new_text_starts_another() {
        let frames = vec![
            frame(1.0, pane("> fix\n", 5, 0, true, false)),
            frame(2.0, pane("> fix the bug\n", 13, 0, true, false)),
            frame(3.0, pane("> fix the\n", 9, 0, true, false)),
            frame(4.0, pane("working\n", 0, 0, false, false)),
            frame(5.0, pane("> now run tests\n", 15, 0, true, false)),
        ];
        let (bound, cards) = choose(&frames).unwrap();
        let texts: Vec<_> = cards.iter().map(|c| c["text"].as_str().unwrap()).collect();
        assert_eq!(texts, ["> fix the bug", "> now run tests"]);
        assert_eq!(bound.current, "Composed this input: “> now run tests”");
        assert_eq!(bound.kind, "cursor_input_line");
        assert_eq!(bound.timestamp, json!(5.0));
    }

    #[test]
    fn an_unchanged_prompt_is_not_input_but_a_command_sent_between_captures_is() {
        let idle = vec![
            frame(
                1.0,
                pane("~/proj > \x1b[2mTry a command\x1b[0m\n", 9, 0, true, false),
            ),
            frame(
                2.0,
                pane("~/proj > \x1b[2mTry a command\x1b[0m\n", 9, 0, true, false),
            ),
        ];
        assert!(choose(&idle).is_none());
        let sent = vec![
            frame(1.0, pane("~/proj >\n", 8, 0, true, false)),
            frame(
                6.0,
                pane("~/proj > cargo test\nok\n~/proj >\n", 8, 2, true, false),
            ),
        ];
        let (bound, cards) = choose(&sent).unwrap();
        assert_eq!(cards.len(), 1);
        assert_eq!(bound.current, "Composed this input: “~/proj > cargo test”");
        assert_eq!(bound.start, 0);
    }

    #[test]
    fn segments_without_cursor_fields_have_no_input() {
        let old = json!({"id":"%0","active":true,"content":"> fix the bug\n"});
        assert!(choose(&[frame(1.0, old)]).is_none());
    }

    #[test]
    fn inputs_past_the_budget_keep_the_newest() {
        let frames: Vec<_> = (0..20)
            .map(|i| {
                let text = format!("> {i:02} {}", "x".repeat(500));
                frame(
                    f64::from(i),
                    pane(&format!("{text}\n"), 510, 0, true, false),
                )
            })
            .collect();
        let (_, cards) = choose(&frames).unwrap();
        let total: usize = cards
            .iter()
            .map(|c| c["text"].as_str().unwrap().len())
            .sum();
        assert!(total <= INPUT_BUDGET_BYTES);
        assert!(
            cards.last().unwrap()["text"]
                .as_str()
                .unwrap()
                .starts_with("> 19")
        );
    }
}
