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
//! captures is read from the row the cursor just left. Text changed in the
//! middle keeps its start, so a row that keeps a long common start with the
//! input before it is that input edited. An input longer than its field wraps:
//! when the cursor moves down one row while the row above it, now mostly full,
//! still holds what was being typed, both rows are one input.

use super::screen::display_line;
use serde_json::Value;

/// A longer cursor row is screen output the cursor happens to sit on.
const INPUT_MAX_BYTES: usize = 600;
/// A wrapped row fills at least this share of the pane's width, in tenths: an
/// input field has margins and wraps at a word.
const WRAP_TENTHS: usize = 7;
/// An edit keeps at least this much of the input's start, and half of it.
const EDIT_COMMON_START: usize = 12;

fn edited(before: &str, after: &str) -> bool {
    let common = before
        .chars()
        .zip(after.chars())
        .take_while(|(a, b)| a == b)
        .count();
    let shorter = before.chars().count().min(after.chars().count());
    common >= EDIT_COMMON_START && common * 2 >= shorter
}

/// One typed input: the longest text the cursor row held while each reading
/// only extended or shortened the one before it.
#[derive(Clone, Debug)]
pub(super) struct Typed {
    pub timestamp: f64,
    pub window: String,
    pub pane: String,
    /// The input's earlier wrapped rows, joined.
    head: String,
    /// The row the cursor is on.
    text: String,
    row: usize,
    changed: bool,
}

impl Typed {
    /// The whole input, wrapped rows joined.
    pub(super) fn text(&self) -> String {
        if self.head.is_empty() {
            self.text.clone()
        } else {
            format!("{} {}", self.head, self.text)
        }
    }
}

/// Lines as a terminal splits them, each keeping its line break.
pub(super) fn split_lines(text: &str) -> Vec<&str> {
    let mut lines = Vec::new();
    let mut start = 0;
    let mut chars = text.char_indices().peekable();
    while let Some((i, c)) = chars.next() {
        if matches!(
            c,
            '\n' | '\r'
                | '\u{b}'
                | '\u{c}'
                | '\u{1c}'
                | '\u{1d}'
                | '\u{1e}'
                | '\u{85}'
                | '\u{2028}'
                | '\u{2029}'
        ) {
            let mut end = i + c.len_utf8();
            if c == '\r' && chars.peek().is_some_and(|(_, c)| *c == '\n') {
                let (j, c) = chars.next().unwrap();
                end = j + c.len_utf8();
            }
            lines.push(&text[start..end]);
            start = end;
        }
    }
    if start < text.len() {
        lines.push(&text[start..]);
    }
    lines
}

/// The cursor row's typed text.
fn typed_row(pane: &Value) -> Option<String> {
    if pane["cursor_visible"] != true || pane["pane_in_mode"] != false {
        return None;
    }
    let x = usize::try_from(pane["cursor_x"].as_u64()?).ok()?;
    let y = usize::try_from(pane["cursor_y"].as_u64()?).ok()?;
    row_text(pane, y, x)
}

/// A row's text, leaving out concealed text and dim text from column `x` on.
fn row_text(pane: &Value, y: usize, x: usize) -> Option<String> {
    let row = *split_lines(pane["content"].as_str()?).get(y)?;
    let text = display_line(row.trim_end_matches(['\r', '\n']), Some(x), false)
        .trim()
        .to_owned();
    (!text.is_empty() && text.len() <= INPUT_MAX_BYTES).then_some(text)
}

/// Every typed input in the frames, oldest first.
pub(super) fn typed(frames: &[Value]) -> Vec<Typed> {
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
        let timestamp = frame["timestamp"].as_f64().unwrap_or(0.0);
        let window = tmux["window"]["id"].as_str().unwrap_or("").to_owned();
        let Some(panes) = tmux["panes"].as_array() else {
            continue;
        };
        for pane in panes.iter().filter(|p| p["active"] == true) {
            let Some(text) = typed_row(pane) else {
                continue;
            };
            let id = pane["id"].as_str().unwrap_or("").to_owned();
            let row = usize::try_from(pane["cursor_y"].as_u64().unwrap_or(0)).unwrap_or(0);
            let last = out.iter().rposition(|t| t.window == window && t.pane == id);
            // The cursor left a row: what that row shows now is what was sent.
            if let Some(i) = last.filter(|&i| out[i].row != row)
                && let Some(sent) = row_text(pane, out[i].row, usize::MAX)
                && sent.len() > out[i].text.len()
                && sent.starts_with(&out[i].text)
            {
                out[i].text = sent;
                out[i].timestamp = timestamp;
                out[i].changed = true;
            }
            let reading = Typed {
                timestamp,
                window: window.clone(),
                pane: id,
                head: String::new(),
                text,
                row,
                changed: false,
            };
            let same = last.filter(|&i| {
                let t = &out[i];
                t.row == reading.row
                    && (t.text.starts_with(&reading.text)
                        || reading.text.starts_with(&t.text)
                        || edited(&t.text, &reading.text))
            });
            // The input wrapped: the row above the cursor now holds what was being
            // typed, mostly full, whether the cursor moved down or the field grew up.
            let width = usize::try_from(pane["width"].as_u64().unwrap_or(0)).unwrap_or(0);
            if same.is_none()
                && let Some(i) = last
                && let Some(above) = row
                    .checked_sub(1)
                    .and_then(|y| row_text(pane, y, usize::MAX))
                && above.chars().count() * 10 >= width * WRAP_TENTHS
                && (above.starts_with(&out[i].text) || out[i].text.starts_with(&above))
            {
                let mut input = out.remove(i);
                input.head = if input.head.is_empty() {
                    above
                } else {
                    format!("{} {above}", input.head)
                };
                input.text = reading.text;
                input.row = row;
                input.timestamp = timestamp;
                input.changed = true;
                out.push(input);
                continue;
            }
            match same {
                Some(i) if out[i].text == reading.text => {}
                Some(i) if out[i].text.starts_with(&reading.text) => out[i].changed = true,
                Some(i) => {
                    let mut grown = reading;
                    grown.changed = true;
                    grown.head = std::mem::take(&mut out[i].head);
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
    // A prompt glyph left alone is an emptied input line, not typing.
    out.retain(|t| t.changed && t.text.chars().any(char::is_alphanumeric));
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn pane(content: &str, x: u64, y: u64, visible: bool, mode: bool) -> Value {
        json!({"id":"%0","active":true,"content":content,"cursor_x":x,"cursor_y":y,"cursor_visible":visible,"alternate_on":true,"pane_in_mode":mode})
    }
    fn frame(time: f64, pane: Value) -> Value {
        json!({"timestamp":time,"content":{"tmux":{"session":"s","window":{"id":"@0"},"panes":[pane]}}})
    }
    fn texts(frames: &[Value]) -> Vec<String> {
        typed(frames).iter().map(Typed::text).collect()
    }

    #[test]
    fn the_cursor_row_is_the_input_and_dim_text_after_it_is_not() {
        let screen = "output line\n> fix the bu\x1b[2mg in parser\x1b[0m\nstatus bar\n";
        assert_eq!(
            typed_row(&pane(screen, 11, 1, true, false)).unwrap(),
            "> fix the bu"
        );
        // Dim text before the cursor was typed in a dim style, so it stays.
        assert_eq!(
            typed_row(&pane("\x1b[2m$ ls\x1b[0m\n", 4, 0, true, false)).unwrap(),
            "$ ls"
        );
    }

    #[test]
    fn a_hidden_cursor_a_mode_a_blank_row_or_only_a_placeholder_is_no_input() {
        assert!(typed_row(&pane("\x1b[2mTry a command\x1b[0m\n", 0, 0, true, false)).is_none());
        assert!(typed_row(&pane("> typed\n", 7, 0, false, false)).is_none());
        assert!(typed_row(&pane("> typed\n", 7, 0, true, true)).is_none());
        assert!(typed_row(&pane("log line\n\n", 0, 1, true, false)).is_none());
        assert!(typed_row(&pane("> typed\n", 7, 5, true, false)).is_none());
        let mut old = pane("> typed\n", 7, 0, true, false);
        old.as_object_mut().unwrap().remove("cursor_x");
        assert!(typed_row(&old).is_none());
        assert!(typed_row(&pane("\x1b[8m> secret\x1b[0m\n", 8, 0, true, false)).is_none());
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
        assert_eq!(texts(&frames), ["> fix the bug", "> now run tests"]);
        // Text changed in the middle is the same input, edited.
        let edit = vec![
            frame(1.0, pane("> \n", 2, 0, true, false)),
            frame(
                2.0,
                pane("> why is the build missing\n", 26, 0, true, false),
            ),
            frame(
                3.0,
                pane("> why is the exact build missing\n", 32, 0, true, false),
            ),
        ];
        assert_eq!(texts(&edit), ["> why is the exact build missing"]);
        assert_eq!(typed(&frames).last().unwrap().timestamp, 5.0);
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
        assert!(typed(&idle).is_empty());
        let sent = vec![
            frame(1.0, pane("~/proj >\n", 8, 0, true, false)),
            frame(
                6.0,
                pane("~/proj > cargo test\nok\n~/proj >\n", 8, 2, true, false),
            ),
        ];
        assert_eq!(texts(&sent), ["~/proj > cargo test"]);
    }

    #[test]
    fn an_input_that_wraps_onto_the_next_row_is_one_input() {
        let wide = |time: f64, content: &str, x: u64, y: u64| {
            let mut p = pane(content, x, y, true, false);
            p["width"] = json!(30);
            frame(time, p)
        };
        let frames = vec![
            wide(1.0, "> \n", 2, 0),
            wide(2.0, "> please look at the parser\n", 27, 0),
            wide(3.0, "> please look at the parser\n  and fix it\n", 12, 1),
            wide(
                4.0,
                "> please look at the parser\n  and fix its tests\n",
                18,
                1,
            ),
        ];
        assert_eq!(
            texts(&frames),
            ["> please look at the parser and fix its tests"]
        );
        // An input field that grows upward keeps the cursor on its last row.
        let up = vec![
            wide(1.0, "───\n> \n", 2, 1),
            wide(2.0, "───\n> please look at the parser\n", 27, 1),
            wide(3.0, "> please look at the parser\n  and fix it\n", 12, 1),
        ];
        assert_eq!(texts(&up), ["> please look at the parser and fix it"]);
        // A short command and a new prompt on the next row are not one input.
        let shell = vec![
            wide(1.0, "~ >\n", 4, 0),
            wide(2.0, "~ > cd x\n", 8, 0),
            wide(3.0, "~ > cd x\nx > l\n", 5, 1),
            wide(4.0, "~ > cd x\nx > ls\n", 6, 1),
        ];
        assert_eq!(texts(&shell), ["~ > cd x", "x > ls"]);
    }

    #[test]
    fn segments_without_cursor_fields_have_no_input() {
        let old = json!({"id":"%0","active":true,"content":"> fix the bug\n"});
        assert!(typed(&[frame(1.0, old)]).is_empty());
    }
}
