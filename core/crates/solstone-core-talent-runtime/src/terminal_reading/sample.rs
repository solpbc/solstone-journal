// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Fit a terminal segment's entries to a fixed token budget, the way Describe
//! fits frames: decide how deeply to read each entry first, and which entries
//! to read only when that is not enough.
//!
//! Typed input is always kept. Every other entry keeps whole up to a line cap,
//! one cap for all entries, chosen as the largest that fits: short entries stay
//! whole and only long ones (a build log, a diff) are shortened to their start
//! and end. Below the smallest useful cap, new-text entries are chosen by time
//! spread, first and last before the ones farthest in time from those kept, and
//! each run of entries left out is named by its time span and size.

use super::clock;
use super::screen::{Item, Kind, Session};
use std::collections::HashSet;

/// A shorter entry no longer shows enough of what it was to be read.
const MIN_LINE_CAP: usize = 4;

pub(super) const GUIDE: &str = "Terminal capture of this segment, sampled to fit. Each session first lists \
what was typed at the keyboard, in order, then its entries in time order, each with its time, window and pane. \
\"typed\" is keyboard input at the cursor: a shell command or a message written to an assistant. \"new text\" is \
what appeared on that pane since its previous capture: program output, or an assistant's or agent's replies, plans \
and reports, which are that program's work. \"already on screen\" is a pane at its first capture here: earlier \
work, from before what this segment shows. Each line appears once, at its first appearance; a line rewritten in \
place shows its latest version. Long entries show their start and end with the number of lines left out, and \
entries left out are named with their time span.\n";

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub(super) struct Fit {
    /// The rendered text for each session, in order.
    pub texts: Vec<String>,
    pub tokens: u32,
    /// The cap every entry was read to, when one was needed.
    pub line_cap: Option<usize>,
    pub entries: usize,
    pub entries_left_out: usize,
    pub lines: usize,
    pub lines_shown: usize,
}

struct Choice<'a> {
    cap: Option<usize>,
    dropped: &'a HashSet<(usize, usize)>,
}

fn pane_label(session: &Session, pane: usize) -> String {
    let p = &session.panes[pane];
    let shared = session
        .panes
        .iter()
        .filter(|other| other.window == p.window)
        .count()
        > 1;
    if shared {
        format!("window \"{}\" pane {}", p.window, p.id)
    } else {
        format!("window \"{}\"", p.window)
    }
}

fn capped(item: &Item, cap: Option<usize>) -> (Vec<String>, usize) {
    let Some(cap) = cap.filter(|cap| item.kind != Kind::Typed && item.lines.len() > *cap) else {
        return (item.lines.clone(), item.lines.len());
    };
    let left = item.lines.len() - cap;
    let mut out = Vec::with_capacity(cap + 1);
    if item.kind == Kind::FirstSeen {
        // The end of a screen is its latest state: the prompt, the reply being written.
        out.push(format!("[{left} earlier lines left out]"));
        out.extend_from_slice(&item.lines[left..]);
    } else {
        let head = cap.div_ceil(2);
        out.extend_from_slice(&item.lines[..head]);
        out.push(format!("[{left} lines left out]"));
        out.extend_from_slice(&item.lines[item.lines.len() - (cap - head)..]);
    }
    (out, cap)
}

fn render(sessions: &[Session], base_ms: i64, choice: &Choice) -> (Vec<String>, usize) {
    let mut shown = 0;
    let mut texts = Vec::with_capacity(sessions.len());
    for (s, session) in sessions.iter().enumerate() {
        let mut text = String::new();
        if s == 0 {
            text.push_str(GUIDE);
            text.push('\n');
        }
        let span = match (session.items.first(), session.items.last()) {
            (Some(first), Some(last)) => format!(
                "{}–{}",
                clock(base_ms, first.time),
                clock(base_ms, last.time)
            ),
            _ => String::new(),
        };
        text.push_str(&format!(
            "Terminal session \"{}\", {span}, {} captures. Windows: {}.\n",
            session.name,
            session.captures,
            session.windows.join(", ")
        ));
        let typed: Vec<&Item> = session
            .items
            .iter()
            .filter(|item| item.kind == Kind::Typed)
            .collect();
        if !typed.is_empty() {
            text.push_str("Typed at the keyboard in this segment, in order:\n");
            for item in typed {
                text.push_str(&format!(
                    "- {} · {}: {}\n",
                    clock(base_ms, item.time),
                    pane_label(session, item.pane),
                    item.lines.join(" ")
                ));
            }
        }
        if session.typed_left_out > 0 {
            text.push_str(&format!(
                "[{} earlier typed inputs left out]\n",
                session.typed_left_out
            ));
        }
        let mut gap: Option<(f64, f64, usize, usize)> = None;
        let close_gap = |text: &mut String, gap: &mut Option<(f64, f64, usize, usize)>| {
            if let Some((from, to, entries, lines)) = gap.take() {
                text.push_str(&format!(
                    "\n### {}–{} · {entries} new-text entries left out ({lines} lines)\n",
                    clock(base_ms, from),
                    clock(base_ms, to)
                ));
            }
        };
        for (i, item) in session.items.iter().enumerate() {
            if choice.dropped.contains(&(s, i)) {
                let g = gap.get_or_insert((item.time, item.time, 0, 0));
                g.1 = item.time;
                g.2 += 1;
                g.3 += item.lines.len();
                continue;
            }
            close_gap(&mut text, &mut gap);
            let at = clock(base_ms, item.time);
            let label = pane_label(session, item.pane);
            match item.kind {
                Kind::Typed => {
                    text.push_str(&format!("\n### {at} · {label} · typed\n"));
                    text.push_str(&item.lines.join("\n"));
                    text.push('\n');
                    shown += item.lines.len();
                }
                Kind::FirstSeen | Kind::NewText => {
                    let what = if item.kind == Kind::FirstSeen {
                        "already on screen at its first capture (earlier work)"
                    } else {
                        "new text"
                    };
                    let (lines, count) = capped(item, choice.cap);
                    shown += count;
                    text.push_str(&format!("\n### {at} · {label} · {what}\n```\n"));
                    if lines.is_empty() {
                        text.push_str("(blank)\n");
                    }
                    for line in lines {
                        // Displayed text cannot close its own fence and pose as an entry.
                        if line.trim_start().starts_with("```") {
                            text.push_str("    ");
                        }
                        text.push_str(&line);
                        text.push('\n');
                    }
                    text.push_str("```\n");
                }
            }
        }
        close_gap(&mut text, &mut gap);
        texts.push(text);
    }
    (texts, shown)
}

/// The order new-text entries are kept in when not all of them fit: the last
/// (where the segment ended), the first, then each the farthest in time from
/// every entry already kept, the larger first on a tie.
fn keep_order(sessions: &[Session]) -> Vec<(usize, usize)> {
    let mut rest: Vec<((usize, usize), f64, usize)> = sessions
        .iter()
        .enumerate()
        .flat_map(|(s, session)| {
            session
                .items
                .iter()
                .enumerate()
                .filter(|(_, item)| item.kind == Kind::NewText)
                .map(move |(i, item)| ((s, i), item.time, item.lines.len()))
        })
        .collect();
    rest.sort_by(|a, b| a.1.total_cmp(&b.1).then(a.0.cmp(&b.0)));
    let mut order = Vec::with_capacity(rest.len());
    let mut kept_times: Vec<f64> = Vec::new();
    if let Some(last) = rest.pop() {
        order.push(last.0);
        kept_times.push(last.1);
    }
    while !rest.is_empty() {
        let best = rest
            .iter()
            .enumerate()
            .max_by(|(_, a), (_, b)| {
                let da = kept_times
                    .iter()
                    .map(|t| (a.1 - t).abs())
                    .fold(f64::MAX, f64::min);
                let db = kept_times
                    .iter()
                    .map(|t| (b.1 - t).abs())
                    .fold(f64::MAX, f64::min);
                da.total_cmp(&db)
                    .then(a.2.cmp(&b.2))
                    .then(b.1.total_cmp(&a.1))
            })
            .map(|(at, _)| at)
            .unwrap_or(0);
        let chosen = rest.remove(best);
        order.push(chosen.0);
        kept_times.push(chosen.1);
    }
    order
}

/// Fit the sessions' entries to `budget` tokens as counted by `count`.
pub(super) fn fit(
    sessions: &[Session],
    base_ms: i64,
    budget: u32,
    count: &dyn Fn(&str) -> u32,
) -> Fit {
    let tokens = |texts: &[String]| texts.iter().map(|t| count(t)).sum::<u32>();
    let lines: usize = sessions
        .iter()
        .flat_map(|s| &s.items)
        .map(|i| i.lines.len())
        .sum();
    let entries = sessions.iter().map(|s| s.items.len()).sum();
    let none = HashSet::new();
    let attempt = |cap: Option<usize>, dropped: &HashSet<(usize, usize)>| {
        let (texts, shown) = render(sessions, base_ms, &Choice { cap, dropped });
        let used = tokens(&texts);
        (texts, shown, used)
    };
    let done = |(texts, shown, used): (Vec<String>, usize, u32), cap, left_out| Fit {
        texts,
        tokens: used,
        line_cap: cap,
        entries,
        entries_left_out: left_out,
        lines,
        lines_shown: shown,
    };
    let whole = attempt(None, &none);
    if whole.2 <= budget {
        return done(whole, None, 0);
    }
    let longest = sessions
        .iter()
        .flat_map(|s| &s.items)
        .filter(|i| i.kind != Kind::Typed)
        .map(|i| i.lines.len())
        .max()
        .unwrap_or(0);
    // The largest cap that fits, when even the smallest does.
    let (mut low, mut high) = (MIN_LINE_CAP, longest.max(MIN_LINE_CAP));
    let smallest = attempt(Some(MIN_LINE_CAP), &none);
    if smallest.2 <= budget {
        let mut best = smallest;
        while low < high {
            let middle = low + (high - low).div_ceil(2);
            let tried = attempt(Some(middle), &none);
            if tried.2 <= budget {
                low = middle;
                best = tried;
            } else {
                high = middle - 1;
            }
        }
        return done(best, Some(low), 0);
    }
    // At the smallest cap, keep the longest prefix of the time-spread order that fits.
    let order = keep_order(sessions);
    let dropped_after =
        |kept: usize| -> HashSet<(usize, usize)> { order[kept..].iter().copied().collect() };
    let (mut low, mut high) = (0usize, order.len());
    let mut best = attempt(Some(MIN_LINE_CAP), &dropped_after(0));
    while low < high {
        let middle = low + (high - low).div_ceil(2);
        let tried = attempt(Some(MIN_LINE_CAP), &dropped_after(middle));
        if tried.2 <= budget {
            low = middle;
            best = tried;
        } else {
            high = middle - 1;
        }
    }
    done(best, Some(MIN_LINE_CAP), order.len() - low)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::terminal_reading::screen::Pane;

    fn session(items: Vec<(Kind, f64, usize)>) -> Session {
        Session {
            name: "s".into(),
            windows: vec!["work".into()],
            panes: vec![Pane {
                window: "work".into(),
                id: "%0".into(),
            }],
            items: items
                .into_iter()
                .enumerate()
                .map(|(n, (kind, time, lines))| Item {
                    kind,
                    time,
                    pane: 0,
                    lines: (0..lines).map(|l| format!("entry {n} line {l}")).collect(),
                })
                .collect(),
            captures: 60,
            typed_left_out: 0,
        }
    }
    fn words(text: &str) -> u32 {
        text.split_whitespace().count() as u32
    }

    #[test]
    fn a_segment_that_fits_is_read_whole() {
        let s = vec![session(vec![
            (Kind::FirstSeen, 0.0, 3),
            (Kind::NewText, 5.0, 2),
        ])];
        let fit = fit(&s, 36_000_000, 10_000, &words);
        assert_eq!((fit.line_cap, fit.entries_left_out), (None, 0));
        assert_eq!(fit.lines_shown, 5);
        assert!(fit.texts[0].starts_with(GUIDE));
        assert!(
            fit.texts[0]
                .contains("### 10:00:05 · window \"work\" · new text\n```\nentry 1 line 0\n")
        );
    }

    #[test]
    fn displayed_text_cannot_close_its_fence() {
        let mut s = session(vec![(Kind::NewText, 5.0, 1)]);
        s.items[0].lines = vec![
            "```".into(),
            "### 10:00:09 · window \"work\" · typed".into(),
        ];
        let text = &fit(&[s], 0, u32::MAX, &words).texts[0];
        assert!(
            text.contains("new text\n```\n    ```\n### 10:00:09"),
            "{text}"
        );
    }

    #[test]
    fn long_entries_are_read_to_their_start_and_end_before_any_entry_is_left_out() {
        let s = vec![session(vec![
            (Kind::FirstSeen, 0.0, 40),
            (Kind::Typed, 4.0, 1),
            (Kind::NewText, 5.0, 3),
            (Kind::NewText, 9.0, 200),
        ])];
        let whole = fit(&s, 0, u32::MAX, &words).tokens;
        let fit = fit(&s, 0, whole / 3, &words);
        let cap = fit.line_cap.unwrap();
        assert!((MIN_LINE_CAP..40).contains(&cap));
        assert_eq!(fit.entries_left_out, 0);
        assert!(fit.tokens <= whole / 3);
        let text = &fit.texts[0];
        assert!(
            text.contains("entry 2 line 2\n"),
            "a short entry stays whole"
        );
        assert!(text.contains(&format!("[{} lines left out]", 200 - cap)));
        assert!(
            text.contains("entry 3 line 199\n"),
            "a long entry keeps its end"
        );
        assert!(text.contains(&format!("[{} earlier lines left out]", 40 - cap)));
        assert!(text.contains("· typed\nentry 1 line 0\n"));
    }

    #[test]
    fn below_the_smallest_cap_entries_are_chosen_by_time_spread() {
        let items = (0..30)
            .map(|n| (Kind::NewText, f64::from(n) * 10.0, 6))
            .collect();
        let s = vec![session(items)];
        let at_min = |dropped: usize| {
            let order = keep_order(&s);
            let dropped: HashSet<_> = order[order.len() - dropped..].iter().copied().collect();
            render(
                &s,
                0,
                &Choice {
                    cap: Some(MIN_LINE_CAP),
                    dropped: &dropped,
                },
            )
            .0
        };
        let budget = words(&at_min(20)[0]);
        let fit = fit(&s, 0, budget, &words);
        assert_eq!(fit.entries_left_out, 20);
        assert!(fit.tokens <= budget);
        let text = &fit.texts[0];
        assert!(text.contains("entry 29 line 0"), "the last entry is kept");
        assert!(text.contains("entry 0 line 0"), "then the first");
        assert!(text.contains("new-text entries left out"));
        let order = keep_order(&s);
        assert_eq!(&order[..3], &[(0, 29), (0, 0), (0, 14)]);
    }
}
