// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::collections::{HashMap, HashSet};

use serde_json::Value;

use super::{JsonObject, ProducedChunks, display_value, json_falsy, recorded_chunk};

pub(super) fn render(records: &[JsonObject]) -> ProducedChunks {
    let mut chunks = Vec::new();
    let mut attention_memory: HashMap<(Option<String>, Option<String>), String> = HashMap::new();

    for record in records {
        let kind = clean_text(record.get("t"));
        let markdown = match kind.as_str() {
            "segment_start" => {
                let (md, attention) = format_snapshot(record);
                if !md.is_empty() {
                    let key = memory_key(record);
                    // Snapshot resets attention memory for this key; empty attention removes it.
                    if attention.is_empty() {
                        attention_memory.remove(&key);
                    } else {
                        attention_memory.insert(key, attention);
                    }
                }
                md
            }
            "delta" => {
                let block_text = format_delta(record);
                if block_text.is_empty() {
                    String::new()
                } else {
                    let attention = parse_attention_text(record);
                    if attention.is_empty() {
                        block_text
                    } else {
                        let key = memory_key(record);
                        let remembered = attention_memory.get(&key);
                        if remembered.map(String::as_str) != Some(attention.as_str()) {
                            let prefixed = format!("context: {attention}\n{block_text}");
                            attention_memory.insert(key, attention);
                            prefixed
                        } else {
                            block_text
                        }
                    }
                }
            }
            _ => String::new(),
        };
        if !markdown.is_empty() {
            let occurrence = record.get("ts").and_then(integer_milliseconds).unwrap_or(0);
            chunks.push(recorded_chunk(markdown, occurrence, record));
        }
    }

    ProducedChunks {
        chunks,
        agent_override: Some("browser".to_string()),
        header: None,
        error: None,
        warnings: Vec::new(),
    }
}

/// Said when no page in front of a focused window sent anything, so the list below it is all there was.
const NOTHING_IN_FRONT: &str =
    "No page in front of a focused window sent anything in this segment.";

/// Browser content as a talent reads it to learn what the owner did. A read the browser marks as not in front
/// of a focused window (a background tab, or a window behind another app) is not what the owner was reading:
/// what such a page shows by itself (a live clock, a video's captions, an ad) is left out, and the page is
/// listed once per state after the pages that were in front. Pausing the solstone app does not change this,
/// and a record without tab state counts as in front.
pub(super) fn render_attended(records: &[JsonObject]) -> ProducedChunks {
    let (in_front, open) = split_attention(records);
    let mut produced = render(&in_front);
    if !open.is_empty() {
        let mut text = String::new();
        if produced.chunks.is_empty() {
            text.push_str(NOTHING_IN_FRONT);
            text.push_str("\n\n");
        }
        text.push_str("## Open, not in front of a focused window\n");
        text.push_str(
            &open
                .iter()
                .map(|line| format!("- {line}"))
                .collect::<Vec<_>>()
                .join("\n"),
        );
        produced.chunks.push(super::IndexChunk {
            content: text,
            occurrence_time_ms: None,
            source: None,
            group_offset_ms: None,
        });
    }
    produced
}

/// The records a talent reads as in front of the owner, and one line per state for each page that was not.
fn split_attention(records: &[JsonObject]) -> (Vec<JsonObject>, Vec<String>) {
    let mut in_front = Vec::with_capacity(records.len());
    let mut pages: HashMap<(Option<String>, Option<String>), &JsonObject> = HashMap::new();
    let mut open: Vec<String> = Vec::new();
    for record in records {
        let kind = record.get("t").and_then(Value::as_str);
        if kind == Some("segment_start") {
            pages.insert(memory_key(record), record);
        }
        if !matches!(kind, Some("segment_start" | "delta")) || attended(record) {
            in_front.push(record.clone());
            continue;
        }
        if let Some(page) = pages.get(&memory_key(record)) {
            let line = open_page_line(page, record);
            if !open.contains(&line) {
                open.push(line);
            }
        }
    }
    (in_front, open)
}

/// One open page: its title, its site when that says more, and the state the browser reported.
fn open_page_line(page: &JsonObject, state: &JsonObject) -> String {
    let title = clean_text(page.get("title"));
    let site = clean_text(page.get("site"));
    let url = clean_text(page.get("url"));
    let heading =
        first_non_empty(&[title.as_str(), site.as_str(), url.as_str()]).unwrap_or("Browser Page");
    let mut line = heading.to_string();
    if !site.is_empty() && site != heading {
        line.push_str(&format!(" ({site})"));
    }
    let attention = parse_attention_text(state);
    if !attention.is_empty() {
        line.push_str(&format!(" · {attention}"));
    }
    line
}

fn attended(record: &JsonObject) -> bool {
    let Some(Value::Object(tab)) = record.get("tab") else {
        return true;
    };
    tab.get("front").and_then(Value::as_bool) != Some(false)
        && tab.get("focused").and_then(Value::as_bool) != Some(false)
}

pub(super) fn integer_milliseconds(value: &Value) -> Option<i64> {
    value.as_i64().or_else(|| {
        let number = value.as_f64()?;
        (number.is_finite()
            && number.fract() == 0.0
            && (-9_223_372_036_854_775_808.0..9_223_372_036_854_775_808.0).contains(&number))
        .then_some(number as i64)
    })
}

fn memory_key(row: &JsonObject) -> (Option<String>, Option<String>) {
    let inst = row.get("inst").and_then(Value::as_str).map(str::to_string);
    let ctx = row.get("ctx").and_then(Value::as_str).map(str::to_string);
    (inst, ctx)
}

fn collapsed_bounded_string(value: Option<&Value>) -> Option<String> {
    let raw = value?.as_str()?;
    let collapsed = raw.split_whitespace().collect::<Vec<_>>().join(" ");
    if !collapsed.is_empty() && collapsed.chars().count() <= 64 {
        Some(collapsed)
    } else {
        None
    }
}

fn format_duration(ms: i64) -> String {
    // Duration: largest whole unit plus next non-zero smaller unit; seconds only under one minute.
    let s = ms / 1000;
    if s < 60 {
        return format!("{s}s");
    }
    let m = s / 60;
    if m < 60 {
        return format!("{m}m");
    }
    let h = m / 60;
    let rem_m = m % 60;
    if h < 24 {
        if rem_m > 0 {
            return format!("{h}h {rem_m}m");
        } else {
            return format!("{h}h");
        }
    }
    let d = h / 24;
    let rem_h = h % 24;
    if rem_h > 0 {
        format!("{d}d {rem_h}h")
    } else {
        format!("{d}d")
    }
}

fn parse_attention_parts(row: &JsonObject) -> Vec<&'static str> {
    let mut parts = Vec::new();
    if let Some(Value::Object(tab)) = row.get("tab") {
        let front = tab.get("front").and_then(Value::as_bool);
        let focused = tab.get("focused").and_then(Value::as_bool);
        match front {
            Some(true) => match focused {
                Some(true) => parts.push("front tab, window focused"),
                Some(false) => parts.push("front tab, window not focused"),
                None => parts.push("front tab"),
            },
            Some(false) => parts.push("background tab"),
            None => {}
        }
    }
    if let Some(Value::Object(media)) = row.get("media")
        && media.get("playing").and_then(Value::as_bool) == Some(true)
    {
        if media.get("muted").and_then(Value::as_bool) == Some(true) {
            parts.push("media playing, muted");
        } else {
            parts.push("media playing");
        }
    }
    if row.get("solstone_paused").and_then(Value::as_bool) == Some(true) {
        parts.push("solstone app paused");
    }
    parts
}

fn parse_attention_text(row: &JsonObject) -> String {
    parse_attention_parts(row).join(" · ")
}

fn parse_snapshot_context(row: &JsonObject) -> (Option<String>, String) {
    let mut parts: Vec<String> = parse_attention_parts(row)
        .into_iter()
        .map(str::to_string)
        .collect();
    let attention_text = parts.join(" · ");

    if let Some(Value::Object(tab)) = row.get("tab")
        && let Some(ms) = tab.get("age_ms").and_then(integer_milliseconds)
        && ms >= 0
    {
        parts.push(format!("tab open at least {}", format_duration(ms)));
    }
    if let Some(lang) = collapsed_bounded_string(row.get("lang")) {
        parts.push(format!("language {lang}"));
    }
    if let Some(Value::Object(browser)) = row.get("browser") {
        let vendor = collapsed_bounded_string(browser.get("vendor"));
        let version = collapsed_bounded_string(browser.get("version"));
        match (vendor, version) {
            (Some(v), Some(ver)) => parts.push(format!("{v} {ver}")),
            (Some(v), None) => parts.push(v),
            (None, _) => {}
        }
    }

    let context_line = if parts.is_empty() {
        None
    } else {
        Some(format!("context: {}", parts.join(" · ")))
    };

    (context_line, attention_text)
}

/// A snapshot's heading lines, and its attention state.
fn snapshot_heading(row: &JsonObject) -> (Vec<String>, String) {
    let title = clean_text(row.get("title"));
    let site = clean_text(row.get("site"));
    let url = clean_text(row.get("url"));
    let heading =
        first_non_empty(&[title.as_str(), site.as_str(), url.as_str()]).unwrap_or("Browser Page");
    let mut lines = vec![format!("## {heading}")];

    let adapter = clean_text(row.get("adapter"));
    let subline = [adapter.as_str(), site.as_str()]
        .into_iter()
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join(" · ");
    if !subline.is_empty() {
        lines.push(String::new());
        lines.push(subline);
    }

    let (context_line, attention_text) = parse_snapshot_context(row);
    if let Some(context_line) = context_line {
        lines.push(context_line);
    }
    (lines, attention_text)
}

fn format_snapshot(row: &JsonObject) -> (String, String) {
    let (mut lines, attention_text) = snapshot_heading(row);
    let block_lines = format_blocks(row.get("blocks"));
    if !block_lines.is_empty() {
        lines.push(String::new());
        lines.extend(block_lines);
    }
    (lines.join("\n").trim().to_string(), attention_text)
}

fn format_blocks(blocks: Option<&Value>) -> Vec<String> {
    let Some(Value::Array(blocks)) = blocks else {
        return Vec::new();
    };

    let mut lines = Vec::new();
    for block in blocks {
        let Value::Object(block) = block else {
            continue;
        };
        let text = clean_text(block.get("text"));
        if text.is_empty() {
            continue;
        }
        if block
            .get("type")
            .and_then(Value::as_str)
            .unwrap_or("")
            .trim()
            == "heading"
        {
            lines.push(format!("### {text}"));
        } else {
            lines.push(text);
        }
    }
    lines
}

fn format_delta(row: &JsonObject) -> String {
    match clean_text(row.get("op")).as_str() {
        "add" | "update" => {}
        _ => return String::new(),
    }
    let Some(Value::Object(block)) = row.get("block") else {
        return String::new();
    };
    clean_text(block.get("text"))
}

fn first_non_empty<'a>(values: &[&'a str]) -> Option<&'a str> {
    values.iter().copied().find(|value| !value.is_empty())
}

fn clean_text(value: Option<&Value>) -> String {
    if json_falsy(value) {
        String::new()
    } else {
        value
            .map(display_value)
            .unwrap_or_default()
            .trim()
            .to_string()
    }
}

/// A browser segment too large for the floor window, read so that no page disappears: what a page already showed
/// once, every page's heading, and one shared cap on what each page shows (docs/THINK.md § Browser segments).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BrowserReading {
    pub text: String,
    /// Lines the reading does not show. Zero means repeats were all it removed.
    pub lines_left_out: usize,
    pub chars_left_out: usize,
}

const PAGE_LINES_NOTE: &str = "[Long browsing segment: every page is named. A long page shows its opening, its newest text and the start of some sections.]";
/// Of a capped page's room: the opening, and the newest text when the page changed while it was read.
const OPENING_SHARE_OF_8: u32 = 4;
const OPENING_SHARE_OF_8_UNCHANGED: u32 = 5;
const NEWEST_SHARE_OF_8: u32 = 2;
/// Of the whole reading's room, at most this share goes to the open-page list.
const OPEN_LIST_SHARE: u32 = 8;
/// Characters a token stands for when a single block alone is over its page's cap.
const CLIP_CHARS_PER_TOKEN: usize = 3;
/// A later capture of a page repeats a line only as part of a run this long the page already showed. Cells repeat
/// within a table or feed ("talk", "14:20"), and dropping them alone runs one row into the next.
const REPEAT_RUN_LINES: usize = 3;
/// A change this long that matches an earlier line is the same text arriving again. A shorter one may be a cell.
const REPEAT_CHANGE_CHARS: usize = 24;
/// When a page's plain lines are this short at the median it is a table or a feed, whose links are its content.
const TABLE_CELL_CHARS: usize = 24;

struct ReadLine {
    text: String,
    is_heading: bool,
    /// A link or button on its own says little: it gives up its room first.
    is_fragment: bool,
    tokens: u32,
    /// When it last arrived as a change to the page, not as part of a snapshot.
    changed_at: Option<usize>,
}

#[derive(Default)]
struct ReadPage {
    heading: String,
    heading_tokens: u32,
    lines: Vec<ReadLine>,
    seen: HashMap<String, usize>,
    runs: HashSet<String>,
    attention: String,
}

impl ReadPage {
    fn new(heading: String, attention: String, count: &dyn Fn(&str) -> u32) -> Self {
        let heading_tokens = count(&heading);
        Self {
            heading,
            heading_tokens,
            attention,
            ..Self::default()
        }
    }

    fn push(
        &mut self,
        text: String,
        kind: &str,
        changed_at: Option<usize>,
        count: &dyn Fn(&str) -> u32,
    ) {
        self.seen.insert(text.clone(), self.lines.len());
        self.lines.push(ReadLine {
            is_heading: text.starts_with("### "),
            is_fragment: matches!(kind, "link" | "button"),
            tokens: count(&text),
            text,
            changed_at,
        });
    }

    /// One capture of the page. The first stands whole. A later one adds what is not part of a run the page already
    /// showed, and what it adds is the page's newest text.
    fn push_capture(
        &mut self,
        lines: Vec<(String, &str)>,
        sequence: usize,
        count: &dyn Fn(&str) -> u32,
    ) {
        let changed_at = (!self.lines.is_empty()).then_some(sequence);
        let mut repeated = vec![false; lines.len()];
        if lines.len() < REPEAT_RUN_LINES {
            for (flag, (text, _)) in repeated.iter_mut().zip(&lines) {
                *flag = self.seen.contains_key(text);
            }
        }
        let keys: Vec<String> = lines
            .windows(REPEAT_RUN_LINES)
            .map(|run| {
                run.iter()
                    .map(|(text, _)| text.as_str())
                    .collect::<Vec<_>>()
                    .join("\n")
            })
            .collect();
        for (start, key) in keys.iter().enumerate() {
            if self.runs.contains(key) {
                repeated[start..start + REPEAT_RUN_LINES].fill(true);
            }
        }
        self.runs.extend(keys);
        for ((text, kind), repeated) in lines.into_iter().zip(repeated) {
            if !repeated {
                self.push(text, kind, changed_at, count);
            }
        }
    }

    /// One change to the page. Long text the page already holds counts as that line changing again.
    fn push_change(
        &mut self,
        text: String,
        kind: &str,
        sequence: usize,
        count: &dyn Fn(&str) -> u32,
    ) {
        if text.chars().count() >= REPEAT_CHANGE_CHARS
            && let Some(&index) = self.seen.get(&text)
        {
            self.lines[index].changed_at = Some(sequence);
            return;
        }
        self.push(text, kind, Some(sequence), count);
    }

    /// Short plain lines between its links: a table or a feed, read in order, links and all.
    fn reads_as_table(&self) -> bool {
        let mut plain: Vec<usize> = self
            .lines
            .iter()
            .filter(|line| !line.is_fragment && !line.is_heading)
            .map(|line| line.text.chars().count())
            .collect();
        if plain.is_empty() {
            return true;
        }
        plain.sort_unstable();
        plain[plain.len() / 2] < TABLE_CELL_CHARS
    }

    fn tokens(&self) -> u32 {
        self.lines.iter().map(|line| line.tokens).sum()
    }
}

/// The pages in front of the owner, each once, in first-seen order: a page's first capture whole, then what each
/// later capture or change adds. A capture taken while its page was behind adds no text, but it still names the
/// page if the owner then brings it to the front.
fn read_pages(records: &[JsonObject], count: &dyn Fn(&str) -> u32) -> Vec<ReadPage> {
    let mut pages: Vec<ReadPage> = Vec::new();
    let mut by_tab: HashMap<(Option<String>, Option<String>), usize> = HashMap::new();
    let mut behind: HashMap<(Option<String>, Option<String>), &JsonObject> = HashMap::new();
    let mut by_identity: HashMap<(String, String), usize> = HashMap::new();
    for (sequence, record) in records.iter().enumerate() {
        match record.get("t").and_then(Value::as_str) {
            Some("segment_start") if !attended(record) => {
                by_tab.remove(&memory_key(record));
                behind.insert(memory_key(record), record);
            }
            Some("segment_start") => {
                let index = page_of_capture(&mut pages, &mut by_identity, record, count);
                by_tab.insert(memory_key(record), index);
                behind.remove(&memory_key(record));
                if let Some(Value::Array(blocks)) = record.get("blocks") {
                    let mut lines = Vec::with_capacity(blocks.len());
                    for block in blocks.iter().filter_map(Value::as_object) {
                        let text = clean_text(block.get("text"));
                        if text.is_empty() {
                            continue;
                        }
                        let kind = block
                            .get("type")
                            .and_then(Value::as_str)
                            .unwrap_or("")
                            .trim();
                        let line = if kind == "heading" {
                            format!("### {text}")
                        } else {
                            text
                        };
                        lines.push((line, kind));
                    }
                    pages[index].push_capture(lines, sequence, count);
                }
            }
            Some("delta") if attended(record) => {
                let text = format_delta(record);
                if text.is_empty() {
                    continue;
                }
                let index = match by_tab.get(&memory_key(record)) {
                    Some(index) => *index,
                    None => {
                        let index = if let Some(capture) = behind.get(&memory_key(record)) {
                            page_of_capture(&mut pages, &mut by_identity, capture, count)
                        } else {
                            // A page opened before this segment began has no capture in it.
                            let name = first_non_empty(&[
                                clean_text(record.get("title")).as_str(),
                                clean_text(record.get("site")).as_str(),
                                clean_text(record.get("url")).as_str(),
                                "Browser Page",
                            ])
                            .unwrap_or("Browser Page")
                            .to_owned();
                            pages.push(ReadPage::new(format!("## {name}"), String::new(), count));
                            pages.len() - 1
                        };
                        by_tab.insert(memory_key(record), index);
                        index
                    }
                };
                let page = &mut pages[index];
                let attention = parse_attention_text(record);
                if !attention.is_empty() && attention != page.attention {
                    let line = format!("context: {attention}");
                    if !page.seen.contains_key(&line) {
                        page.push(line, "", None, count);
                    }
                    page.attention = attention;
                }
                let kind = record
                    .get("block")
                    .and_then(|block| block.get("type"))
                    .and_then(Value::as_str)
                    .unwrap_or("");
                let line = if kind == "heading" {
                    format!("### {text}")
                } else {
                    text
                };
                page.push_change(line, kind, sequence, count);
            }
            _ => {}
        }
    }
    pages
}

/// The page a capture belongs to, opening it if this is its first. Pages merge by url and title; one with neither
/// stays apart by site and tab.
fn page_of_capture(
    pages: &mut Vec<ReadPage>,
    by_identity: &mut HashMap<(String, String), usize>,
    record: &JsonObject,
    count: &dyn Fn(&str) -> u32,
) -> usize {
    let (url, title) = (
        clean_text(record.get("url")),
        clean_text(record.get("title")),
    );
    let identity = if url.is_empty() && title.is_empty() {
        (
            clean_text(record.get("site")),
            format!("{:?}", memory_key(record)),
        )
    } else {
        (url, title)
    };
    *by_identity.entry(identity).or_insert_with(|| {
        let (heading, attention) = snapshot_heading(record);
        pages.push(ReadPage::new(heading.join("\n"), attention, count));
        pages.len() - 1
    })
}

/// Which of a page's lines fit `cap`: its opening, its newest changes, then the start of sections spread evenly
/// across it, and whatever room is left goes back to the opening.
fn select_lines(page: &ReadPage, cap: u32) -> Vec<bool> {
    let mut keep = vec![false; page.lines.len()];
    if page.tokens() <= cap {
        keep.fill(true);
        return keep;
    }
    let changed = page.lines.iter().any(|line| line.changed_at.is_some());
    let opening_share = if changed {
        OPENING_SHARE_OF_8
    } else {
        OPENING_SHARE_OF_8_UNCHANGED
    };
    let mut spent = 0;
    let take = |keep: &mut Vec<bool>, index: usize, room: u32, spent: &mut u32| -> bool {
        if keep[index] {
            return true;
        }
        if *spent + page.lines[index].tokens > room {
            return false;
        }
        keep[index] = true;
        *spent += page.lines[index].tokens;
        true
    };
    let opening_room = cap / 8 * opening_share;
    let table = page.reads_as_table();
    for index in (0..page.lines.len()).filter(|index| table || !page.lines[*index].is_fragment) {
        if !take(&mut keep, index, opening_room, &mut spent) {
            break;
        }
    }
    let newest_room = (cap / 8 * NEWEST_SHARE_OF_8).saturating_add(spent);
    let mut newest: Vec<usize> = (0..page.lines.len())
        .filter(|index| page.lines[*index].changed_at.is_some())
        .collect();
    newest.sort_by_key(|index| std::cmp::Reverse(page.lines[*index].changed_at));
    for index in newest {
        if !take(&mut keep, index, newest_room, &mut spent) {
            break;
        }
    }
    let sections: Vec<usize> = (0..page.lines.len())
        .filter(|index| page.lines[*index].is_heading && !keep[*index])
        .collect();
    if !sections.is_empty() {
        let tokens: u32 = sections.iter().map(|index| page.lines[*index].tokens).sum();
        let average = (tokens / sections.len() as u32).max(1);
        let shown = sections
            .len()
            .min((cap.saturating_sub(spent) / average) as usize);
        let spread: Vec<usize> = (0..shown)
            .map(|step| sections[step * sections.len() / shown])
            .collect();
        for &index in &spread {
            take(&mut keep, index, cap, &mut spent);
        }
        for &index in &spread {
            if index + 1 < page.lines.len() && keep[index] {
                take(&mut keep, index + 1, cap, &mut spent);
            }
        }
    }
    // What is left goes back to the opening, sentences before fragments unless the page is a table.
    for fragments in [false, true] {
        // A stand-alone link gets room only once every sentence has it.
        if fragments
            && page
                .lines
                .iter()
                .zip(&keep)
                .any(|(line, kept)| !line.is_fragment && !kept)
        {
            break;
        }
        for index in (0..page.lines.len()).filter(|index| {
            if table {
                !fragments
            } else {
                page.lines[*index].is_fragment == fragments
            }
        }) {
            take(&mut keep, index, cap, &mut spent);
        }
    }
    keep
}

/// The largest cap whose pages, with their headings, fit `room`.
fn page_cap(pages: &[ReadPage], room: u32) -> u32 {
    let tokens: Vec<u32> = pages.iter().map(ReadPage::tokens).collect();
    let (mut low, mut high) = (0, tokens.iter().copied().max().unwrap_or(0));
    let fits = |cap: u32| tokens.iter().map(|page| (*page).min(cap)).sum::<u32>() <= room;
    while low < high {
        let middle = low + (high - low).div_ceil(2);
        if fits(middle) {
            low = middle;
        } else {
            high = middle - 1;
        }
    }
    low
}

fn left_out_marker(lines: usize) -> String {
    if lines == 1 {
        "[1 line left out]".to_owned()
    } else {
        format!("[{lines} lines left out]")
    }
}

fn render_read_page(page: &ReadPage, cap: u32, left_out: &mut (usize, usize)) -> String {
    let keep = select_lines(page, cap);
    let mut text = page.heading.clone();
    let mut skipped = 0;
    let mut started = false;
    let mut shown = 0;
    for (line, kept) in page.lines.iter().zip(&keep) {
        if !kept {
            skipped += 1;
            left_out.0 += 1;
            left_out.1 += line.text.chars().count();
            continue;
        }
        text.push_str(if started { "\n" } else { "\n\n" });
        started = true;
        if skipped > 0 {
            text.push_str(&left_out_marker(skipped));
            text.push('\n');
            skipped = 0;
        }
        text.push_str(&line.text);
        shown += 1;
    }
    if shown == 0 {
        // A single block over the cap still opens its page.
        if let Some(first) = page.lines.first() {
            let clipped: String = first
                .text
                .chars()
                .take(cap as usize * CLIP_CHARS_PER_TOKEN)
                .collect();
            if !clipped.is_empty() {
                text.push_str(&format!("\n\n{clipped} …"));
            }
        }
    }
    if skipped > 0 {
        text.push_str(if started { "\n" } else { "\n\n" });
        text.push_str(&left_out_marker(skipped));
    }
    text
}

/// The attended view fitted to `budget` tokens. `None` when even the page headings do not fit.
pub(super) fn attended_reading(
    records: &[JsonObject],
    budget: u32,
    count: &dyn Fn(&str) -> u32,
) -> Option<BrowserReading> {
    let (_, open) = split_attention(records);
    let pages = read_pages(records, count);
    let open_section = |shown: &[String], more: usize| -> Option<String> {
        if open.is_empty() {
            return None;
        }
        let mut text = String::new();
        if pages.is_empty() {
            text.push_str(NOTHING_IN_FRONT);
            text.push_str("\n\n");
        }
        text.push_str("## Open, not in front of a focused window\n");
        text.push_str(
            &shown
                .iter()
                .map(|line| format!("- {line}"))
                .collect::<Vec<_>>()
                .join("\n"),
        );
        if more > 0 {
            text.push_str(&format!("\n[and {more} more open pages]"));
        }
        Some(text)
    };
    // Repeats gone, it may simply fit: nothing is reserved and nothing else is left out.
    let mut whole: Vec<String> = pages
        .iter()
        .map(|page| render_read_page(page, u32::MAX, &mut (0, 0)))
        .collect();
    whole.extend(open_section(&open, 0));
    let whole = whole.join("\n\n");
    if count(&whole) <= budget {
        return Some(BrowserReading {
            text: whole,
            lines_left_out: 0,
            chars_left_out: 0,
        });
    }
    let open_room = budget / OPEN_LIST_SHARE;
    let mut open_shown = Vec::new();
    let mut open_tokens = 0;
    for line in &open {
        let tokens = count(line) + 2;
        if open_tokens + tokens > open_room {
            break;
        }
        open_tokens += tokens;
        open_shown.push(line.clone());
    }
    let open_more = open.len() - open_shown.len();
    let open_text = open_section(&open_shown, open_more);
    let fixed = open_text.as_deref().map_or(0, count) + count(PAGE_LINES_NOTE);
    let headings: u32 = pages.iter().map(|page| page.heading_tokens + 4).sum();
    let mut room = budget.checked_sub(fixed.saturating_add(headings))?;
    loop {
        let cap = page_cap(&pages, room);
        let mut left_out = (0, 0);
        let mut parts: Vec<String> = pages
            .iter()
            .map(|page| render_read_page(page, cap, &mut left_out))
            .collect();
        let complete = left_out.0 == 0 && open_more == 0;
        if !complete {
            parts.insert(0, PAGE_LINES_NOTE.to_owned());
        }
        parts.extend(open_text.clone());
        let text = parts.join("\n\n");
        if count(&text) <= budget {
            return Some(BrowserReading {
                text,
                lines_left_out: left_out.0 + open_more,
                chars_left_out: left_out.1,
            });
        }
        if room == 0 {
            return None;
        }
        room = room / 20 * 19;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::content::parse_jsonl_objects;

    fn words(text: &str) -> u32 {
        text.split_whitespace().count() as u32 + text.lines().count() as u32
    }

    fn snapshot(url: &str, title: &str, lines: usize, front: bool) -> String {
        let blocks = (0..lines)
            .map(|line| {
                let (kind, text) = if line % 10 == 0 {
                    ("heading", format!("Section {line} of {title}"))
                } else {
                    (
                        "text",
                        format!("Line {line} of {title} says something plain here"),
                    )
                };
                format!(r#"{{"type":"{kind}","text":"{text}"}}"#)
            })
            .collect::<Vec<_>>()
            .join(",");
        format!(
            r#"{{"t":"segment_start","ts":1,"inst":"i","ctx":"{url}","url":"{url}","title":"{title}","site":"example.org","tab":{{"front":{front},"focused":true}},"blocks":[{blocks}]}}"#
        )
    }

    fn delta(ctx: &str, text: &str) -> String {
        format!(
            r#"{{"t":"delta","ts":2,"op":"add","inst":"i","ctx":"{ctx}","site":"example.org","tab":{{"front":true,"focused":true}},"block":{{"type":"text","text":"{text}"}}}}"#
        )
    }

    fn reading_of(rows: &[String], budget: u32) -> BrowserReading {
        attended_reading(&parse_jsonl_objects(&rows.join("\n")), budget, &words).expect("fits")
    }

    fn capture(url: &str, title: &str, blocks: &[(&str, &str)]) -> String {
        let blocks = blocks
            .iter()
            .map(|(kind, text)| format!(r#"{{"type":"{kind}","text":"{text}"}}"#))
            .collect::<Vec<_>>()
            .join(",");
        format!(
            r#"{{"t":"segment_start","ts":1,"inst":"i","ctx":"{url}","url":"{url}","title":"{title}","tab":{{"front":true,"focused":true}},"blocks":[{blocks}]}}"#
        )
    }

    fn feed_row(page: usize, user: usize) -> Vec<(&'static str, String)> {
        vec![
            ("link", "diff".to_owned()),
            ("link", format!("Page {page}")),
            ("text", "14:20".to_owned()),
            ("link", format!("User {user}")),
            ("link", "talk".to_owned()),
        ]
    }

    fn feed(rows: std::ops::Range<usize>) -> String {
        let cells: Vec<(&str, String)> = rows.flat_map(|row| feed_row(row, row)).collect();
        let cells: Vec<(&str, &str)> = cells.iter().map(|(k, t)| (*k, t.as_str())).collect();
        capture("https://f.example/", "Feed", &cells)
    }

    #[test]
    fn a_table_keeps_its_repeated_cells_so_rows_stay_rows() {
        let reading = reading_of(&[feed(0..3)], 2_000);
        assert_eq!(reading.text.matches("talk").count(), 3);
        assert_eq!(reading.text.matches("14:20").count(), 3);
        assert!(
            reading
                .text
                .contains("Page 1\n14:20\nUser 1\ntalk\ndiff\nPage 2")
        );
    }

    #[test]
    fn a_reload_adds_only_its_new_rows_whole_and_they_count_as_the_newest_text() {
        let reading = reading_of(&[feed(0..3), feed(2..5)], 2_000);
        assert_eq!(reading.text.matches("Page 2\n").count(), 1);
        assert!(reading.text.contains("Page 3\n14:20\nUser 3"));
        assert_eq!((reading.lines_left_out, reading.chars_left_out), (0, 0));

        let rows = vec![feed(0..400), feed(399..402)];
        let reading = reading_of(&rows, 700);
        assert!(reading.text.contains("Page 401\n14:20\nUser 401"));
    }

    #[test]
    fn a_table_too_long_for_its_cap_opens_on_whole_rows() {
        let reading = reading_of(&[feed(0..400)], 400);
        assert!(
            reading
                .text
                .contains("diff\nPage 0\n14:20\nUser 0\ntalk\ndiff\nPage 1")
        );
    }

    #[test]
    fn a_short_change_may_repeat_and_a_long_one_is_the_same_line_again() {
        let rows = vec![
            capture(
                "https://c.example/",
                "Chat",
                &[
                    ("text", "A sentence the page already says here"),
                    ("text", "Like"),
                ],
            ),
            delta("https://c.example/", "A new message"),
            delta("https://c.example/", "Like"),
            delta(
                "https://c.example/",
                "A sentence the page already says here",
            ),
        ];
        let reading = reading_of(&rows, 200);
        assert_eq!(reading.text.matches("Like").count(), 2);
        assert_eq!(reading.text.matches("A sentence the page").count(), 1);
    }

    #[test]
    fn a_page_brought_to_the_front_keeps_the_name_it_had_behind() {
        let rows = vec![
            format!(
                r#"{{"t":"segment_start","ts":1,"inst":"i","ctx":"t","url":"https://b.example/","title":"Quiet Title","site":"b.example","tab":{{"front":false,"focused":true}},"blocks":[{{"type":"text","text":"Text from behind"}}]}}"#
            ),
            delta("t", "Written after it came to the front"),
        ];
        let reading = reading_of(&rows, 300);
        assert!(reading.text.contains("## Quiet Title"));
        assert!(reading.text.contains("Written after it came to the front"));
        assert!(!reading.text.contains("Text from behind"));
    }

    #[test]
    fn a_change_goes_to_the_page_its_tab_now_shows() {
        let rows = vec![
            snapshot("https://a.example/", "Alpha", 3, true)
                .replace(r#""ctx":"https://a.example/""#, r#""ctx":"t""#),
            snapshot("https://b.example/", "Beta", 3, false)
                .replace(r#""ctx":"https://b.example/""#, r#""ctx":"t""#),
            delta("t", "Written on Beta"),
        ];
        let reading = reading_of(&rows, 300);
        let beta = reading.text.find("## Beta").expect("Beta is named");
        assert!(reading.text.find("Written on Beta").unwrap() > beta);
    }

    #[test]
    fn pages_with_no_url_or_title_stay_apart_by_site() {
        let page = |site: &str, ctx: &str| {
            format!(
                r#"{{"t":"segment_start","ts":1,"inst":"i","ctx":"{ctx}","site":"{site}","tab":{{"front":true,"focused":true}},"blocks":[{{"type":"text","text":"From {site}"}}]}}"#
            )
        };
        let reading = reading_of(&[page("a.example", "1"), page("b.example", "2")], 300);
        assert!(reading.text.contains("a.example"));
        assert!(reading.text.contains("b.example"));
    }

    #[test]
    fn a_line_too_long_for_what_is_left_does_not_stop_the_shorter_ones_after_it() {
        let long = "word ".repeat(300);
        let mut blocks = vec![format!(r#"{{"type":"text","text":"{long}"}}"#)];
        blocks.extend((0..40).map(|line| {
            format!(r#"{{"type":"text","text":"Short line {line} says something plain here"}}"#)
        }));
        let row = format!(
            r#"{{"t":"segment_start","ts":1,"inst":"i","ctx":"a","url":"https://a.example/","title":"Story","tab":{{"front":true,"focused":true}},"blocks":[{}]}}"#,
            blocks.join(",")
        );
        let reading = reading_of(&[row], 100);
        assert!(reading.text.contains("Short line 0"));
    }

    #[test]
    fn repeats_are_all_a_reading_removes_when_every_new_line_fits() {
        let page = snapshot("https://a.example/", "Alpha", 40, true);
        let rows = vec![
            page.clone(),
            page,
            delta(
                "https://a.example/",
                "Line 3 of Alpha says something plain here",
            ),
        ];
        let reading = reading_of(&rows, 2_000);
        assert_eq!((reading.lines_left_out, reading.chars_left_out), (0, 0));
        assert_eq!(reading.text.matches("Line 3 of Alpha").count(), 1);
        assert_eq!(reading.text.matches("## Alpha").count(), 1);
        assert!(!reading.text.contains(PAGE_LINES_NOTE));
    }

    #[test]
    fn every_page_keeps_its_name_within_the_budget_and_short_pages_stay_whole() {
        let rows = vec![
            snapshot("https://a.example/", "Alpha", 2_000, true),
            snapshot("https://b.example/", "Beta", 1_500, true),
            snapshot("https://c.example/", "Gamma", 6, true),
            snapshot("https://d.example/", "Hidden", 2_000, false),
        ];
        let reading = reading_of(&rows, 2_000);
        assert!(words(&reading.text) <= 2_000);
        for name in ["## Alpha", "## Beta", "## Gamma"] {
            assert!(reading.text.contains(name), "{name}");
        }
        assert!(reading.text.contains("Line 5 of Gamma"));
        assert!(reading.text.contains("Line 1 of Alpha"));
        assert!(reading.text.contains(PAGE_LINES_NOTE));
        assert!(!reading.text.contains("Line 1 of Hidden"));
        assert!(
            reading
                .text
                .contains("## Open, not in front of a focused window\n- Hidden")
        );
        assert!(reading.lines_left_out > 0 && reading.chars_left_out > 0);
    }

    #[test]
    fn a_long_page_shows_its_newest_change_and_the_start_of_later_sections() {
        let rows = vec![
            snapshot("https://a.example/", "Alpha", 3_000, true),
            delta("https://a.example/", "First arrival on this page"),
            delta("https://a.example/", "Newest arrival on this page"),
        ];
        let reading = reading_of(&rows, 800);
        assert!(reading.text.contains("Newest arrival on this page"));
        assert!(reading.text.contains("Line 1 of Alpha"));
        assert!(reading.text.contains("### Section 2"));
        assert!(reading.text.contains("lines left out]"));
        assert!(!reading.text.contains("[1 lines"));
        assert!(words(&reading.text) <= 800);
    }

    #[test]
    fn a_link_on_its_own_gives_up_its_room_before_a_sentence_and_a_page_of_links_still_reads() {
        let mut blocks = Vec::new();
        for line in 0..2_000 {
            let (kind, text) = if line % 2 == 0 {
                ("link", format!("Fragment{line}"))
            } else {
                (
                    "text",
                    format!("Sentence {line} says something plain and whole here"),
                )
            };
            blocks.push(format!(r#"{{"type":"{kind}","text":"{text}"}}"#));
        }
        let article = format!(
            r#"{{"t":"segment_start","ts":1,"inst":"i","ctx":"a","url":"https://a.example/","title":"Article","tab":{{"front":true,"focused":true}},"blocks":[{}]}}"#,
            blocks.join(",")
        );
        let reading = reading_of(&[article], 1_000);
        assert!(reading.text.contains("Sentence 1 says"));
        assert!(!reading.text.contains("Fragment0"));

        let links = (0..2_000)
            .map(|line| format!(r#"{{"type":"link","text":"Story {line} headline here"}}"#))
            .collect::<Vec<_>>()
            .join(",");
        let front_page = format!(
            r#"{{"t":"segment_start","ts":1,"inst":"i","ctx":"b","url":"https://b.example/","title":"Front","tab":{{"front":true,"focused":true}},"blocks":[{links}]}}"#
        );
        let reading = reading_of(&[front_page], 1_000);
        assert!(reading.text.contains("Story 0 headline"));
    }

    #[test]
    fn a_change_with_no_snapshot_in_the_segment_still_reads_under_its_site() {
        let rows = vec![delta("older", "A new message arrived")];
        let reading = reading_of(&rows, 200);
        assert!(reading.text.contains("## example.org"));
        assert!(reading.text.contains("A new message arrived"));
    }

    #[test]
    fn the_bound_holds_for_many_pages_and_a_budget_too_small_for_their_names_refuses() {
        let rows: Vec<String> = (0..300)
            .map(|page| {
                snapshot(
                    &format!("https://{page}.example/"),
                    &format!("Page {page}"),
                    200,
                    true,
                )
            })
            .collect();
        let reading = reading_of(&rows, 8_000);
        assert!(words(&reading.text) <= 8_000);
        assert_eq!(reading.text.matches("## Page ").count(), 300);
        assert_eq!(
            attended_reading(&parse_jsonl_objects(&rows.join("\n")), 100, &words),
            None
        );
    }

    #[test]
    fn integral_timestamp_representations_preserve_occurrence() {
        for (raw, expected) in [
            ("100", 100),
            ("100.0", 100),
            ("1e3", 1000),
            ("9007199254740991.0", 9_007_199_254_740_991),
        ] {
            let text = format!(r#"{{"t":"segment_start","ts":{raw},"blocks":[]}}"#);
            let produced = render(&parse_jsonl_objects(&text));
            assert_eq!(
                produced.chunks[0].occurrence_time_ms,
                Some(crate::content::OccurrenceTimeMs(expected))
            );
        }
        for raw in ["0.5", "9223372036854775808.0", "-9223372036854777856.0"] {
            let value: Value = serde_json::from_str(raw).unwrap();
            assert_eq!(integer_milliseconds(&value), None);
        }
    }

    #[test]
    fn renders_snapshots_and_deltas() {
        let records = parse_jsonl_objects(
            r#"{"t":"segment_start","ts":1,"site":"mail.google.com","url":"https://mail.google.com/mail/u/0/#inbox","title":"Inbox - Gmail","adapter":"gmail","blocks":[{"type":"heading","text":"Inbox"},{"type":"row","text":"Ari Patel - Browser stream contract review"},{"type":"row","text":"   "},{"type":"link","text":"Open pull request"}]}
{"t":"delta","ts":2,"op":"add","block":{"type":"row","text":"New: Casey Morgan - Lunch moved"}}
{"t":"delta","ts":3,"op":"update","block":{"type":"row","text":"Ari Patel - Browser stream contract review (2 replies)"}}
{"t":"delta","ts":4,"op":"remove","block":{"type":"row","text":"Promotions tab collapsed"}}
{"t":"ignored","ts":5,"text":"skip me"}
"#,
        );
        let produced = render(&records);
        let contents: Vec<&str> = produced
            .chunks
            .iter()
            .map(|chunk| chunk.content.as_str())
            .collect();

        assert_eq!(produced.agent_override.as_deref(), Some("browser"));
        assert_eq!(
            contents,
            vec![
                "## Inbox - Gmail\n\ngmail · mail.google.com\n\n### Inbox\nAri Patel - Browser stream contract review\nOpen pull request",
                "New: Casey Morgan - Lunch moved",
                "Ari Patel - Browser stream contract review (2 replies)",
            ]
        );
    }

    #[test]
    fn snapshot_uses_url_or_default_heading_and_delta_skip_rules() {
        let records = parse_jsonl_objects(
            r#"{"t":"segment_start","ts":1,"url":"https://example.com/fallback","blocks":[{"type":"text","text":"Fallback page text"}]}
{"t":"segment_start","ts":2,"blocks":[]}
{"t":"delta","ts":3,"op":"add","block":{"type":"row","text":"   "}}
{"t":"delta","ts":4,"op":"noop","block":{"type":"row","text":"skip"}}
{"t":"delta","ts":5,"op":"update","block":"not an object"}
"#,
        );
        let produced = render(&records);
        let contents: Vec<&str> = produced
            .chunks
            .iter()
            .map(|chunk| chunk.content.as_str())
            .collect();

        assert_eq!(
            contents,
            vec![
                "## https://example.com/fallback\n\nFallback page text",
                "## Browser Page",
            ]
        );
    }

    #[test]
    fn attended_reading_keeps_front_text_and_lists_other_pages_once() {
        use crate::content::produce_browser_attended_chunks;

        let text = r#"{"t":"segment_start","ts":1,"title":"Soil pH","site":"en.wikipedia.org","tab":{"front":true,"focused":true},"blocks":[{"type":"text","text":"Read in front"}]}
{"t":"segment_start","ts":2,"title":"Video","site":"www.youtube.com","inst":"i","ctx":"v","tab":{"front":false,"focused":true},"media":{"playing":true},"blocks":[{"type":"text","text":"Caption in the background"}]}
{"t":"delta","ts":3,"inst":"i","ctx":"v","op":"add","tab":{"front":false,"focused":true},"media":{"playing":true},"block":{"type":"text","text":"Another caption"}}
{"t":"segment_start","ts":4,"title":"Clock","site":"time.gov","inst":"i","ctx":"c","tab":{"front":true,"focused":true},"blocks":[{"type":"text","text":"11:20"}]}
{"t":"delta","ts":5,"inst":"i","ctx":"c","op":"add","tab":{"front":true,"focused":false},"block":{"type":"text","text":"11:21 behind another app"}}
{"t":"segment_start","ts":6,"title":"Guide","tab":{"front":true,"focused":true},"solstone_paused":true,"blocks":[{"type":"text","text":"Read while paused"}]}
{"t":"segment_start","ts":7,"title":"Older extension","blocks":[{"type":"text","text":"No tab state"}]}
"#;
        let contents: Vec<String> = produce_browser_attended_chunks(text)
            .chunks
            .into_iter()
            .map(|chunk| chunk.content)
            .collect();
        assert_eq!(
            contents,
            vec![
                "## Soil pH\n\nen.wikipedia.org\ncontext: front tab, window focused\n\nRead in front",
                "## Clock\n\ntime.gov\ncontext: front tab, window focused\n\n11:20",
                "## Guide\ncontext: front tab, window focused · solstone app paused\n\nRead while paused",
                "## Older extension\n\nNo tab state",
                "## Open, not in front of a focused window\n- Video (www.youtube.com) · background tab · media playing\n- Clock (time.gov) · front tab, window not focused",
            ]
        );
        let background_only = r#"{"t":"segment_start","ts":1,"title":"Video","tab":{"front":false,"focused":true},"blocks":[{"type":"text","text":"Caption"}]}"#;
        assert_eq!(
            produce_browser_attended_chunks(background_only).chunks[0].content,
            format!(
                "{NOTHING_IN_FRONT}\n\n## Open, not in front of a focused window\n- Video · background tab"
            )
        );
    }

    #[test]
    fn produce_chunks_integrates_browser_jsonl() {
        use crate::content::{Family, OccurrenceTimeMs, produce_chunks};

        let path = "chronicle/20260804/suze_browser/120000_10/browser_pages.jsonl";
        let content = "{\"t\":\"segment_start\",\"ts\":1000,\"title\":\"Doc\",\"blocks\":[{\"text\":\"Line 1\"}]}\n";
        let produced = produce_chunks(Family::Browser, path, content);
        assert_eq!(produced.chunks.len(), 1);
        assert_eq!(produced.chunks[0].content, "## Doc\n\nLine 1");
        assert_eq!(
            produced.chunks[0].occurrence_time_ms,
            Some(OccurrenceTimeMs(1000))
        );
        assert_eq!(produced.agent_override.as_deref(), Some("browser"));
    }

    #[test]
    fn snapshot_context_rendering_table() {
        let cases = [
            (
                r#"{"t":"segment_start","ts":1,"tab":{"front":true,"focused":true}}"#,
                "## Browser Page\ncontext: front tab, window focused",
            ),
            (
                r#"{"t":"segment_start","ts":1,"tab":{"front":true,"focused":false}}"#,
                "## Browser Page\ncontext: front tab, window not focused",
            ),
            (
                r#"{"t":"segment_start","ts":1,"tab":{"front":false,"focused":true}}"#,
                "## Browser Page\ncontext: background tab",
            ),
            (
                r#"{"t":"segment_start","ts":1,"tab":{"front":true}}"#,
                "## Browser Page\ncontext: front tab",
            ),
            (
                r#"{"t":"segment_start","ts":1,"tab":{"focused":true}}"#,
                "## Browser Page",
            ),
            (
                r#"{"t":"segment_start","ts":1,"media":{"playing":true}}"#,
                "## Browser Page\ncontext: media playing",
            ),
            (
                r#"{"t":"segment_start","ts":1,"media":{"playing":true,"muted":true}}"#,
                "## Browser Page\ncontext: media playing, muted",
            ),
            (
                r#"{"t":"segment_start","ts":1,"media":{"playing":false,"muted":true}}"#,
                "## Browser Page",
            ),
            (
                r#"{"t":"segment_start","ts":1,"solstone_paused":true}"#,
                "## Browser Page\ncontext: solstone app paused",
            ),
            (
                r#"{"t":"segment_start","ts":1,"solstone_paused":false}"#,
                "## Browser Page",
            ),
            (
                r#"{"t":"segment_start","ts":1,"tab":{"age_ms":0}}"#,
                "## Browser Page\ncontext: tab open at least 0s",
            ),
            (
                r#"{"t":"segment_start","ts":1,"tab":{"age_ms":45000}}"#,
                "## Browser Page\ncontext: tab open at least 45s",
            ),
            (
                r#"{"t":"segment_start","ts":1,"tab":{"age_ms":750000}}"#,
                "## Browser Page\ncontext: tab open at least 12m",
            ),
            (
                r#"{"t":"segment_start","ts":1,"tab":{"age_ms":10800000}}"#,
                "## Browser Page\ncontext: tab open at least 3h",
            ),
            (
                r#"{"t":"segment_start","ts":1,"tab":{"age_ms":11100000}}"#,
                "## Browser Page\ncontext: tab open at least 3h 5m",
            ),
            (
                r#"{"t":"segment_start","ts":1,"tab":{"age_ms":174600000}}"#,
                "## Browser Page\ncontext: tab open at least 2d",
            ),
            (
                r#"{"t":"segment_start","ts":1,"tab":{"age_ms":187200000}}"#,
                "## Browser Page\ncontext: tab open at least 2d 4h",
            ),
            (
                r#"{"t":"segment_start","ts":1,"lang":"en-US"}"#,
                "## Browser Page\ncontext: language en-US",
            ),
            (
                r#"{"t":"segment_start","ts":1,"lang":"fr"}"#,
                "## Browser Page\ncontext: language fr",
            ),
            (
                r#"{"t":"segment_start","ts":1,"browser":{"vendor":"safari","version":"18.1"}}"#,
                "## Browser Page\ncontext: safari 18.1",
            ),
            (
                r#"{"t":"segment_start","ts":1,"browser":{"vendor":"firefox"}}"#,
                "## Browser Page\ncontext: firefox",
            ),
            (
                r#"{"t":"segment_start","ts":1,"browser":{"version":"18.1"}}"#,
                "## Browser Page",
            ),
            (
                r#"{"t":"segment_start","ts":1,"title":"Example","tab":{"front":true},"blocks":[{"type":"text","text":"Hello"}]}"#,
                "## Example\ncontext: front tab\n\nHello",
            ),
            (
                r#"{"t":"segment_start","ts":1,"adapter":"gmail","site":"mail.google.com","title":"Inbox","ext":"EXT-SECRET-99","tab":{"id":"TAB-ID-77","front":false,"focused":true,"age_ms":11100000},"media":{"playing":true},"solstone_paused":true,"lang":"en-US","browser":{"vendor":"safari","version":"18.1"},"blocks":[{"type":"heading","text":"Inbox"}]}"#,
                "## Inbox\n\ngmail · mail.google.com\ncontext: background tab · media playing · solstone app paused · tab open at least 3h 5m · language en-US · safari 18.1\n\n### Inbox",
            ),
        ];

        for (json, expected) in cases {
            let records = parse_jsonl_objects(json);
            let produced = render(&records);
            assert!(produced.warnings.is_empty());
            assert_eq!(produced.error, None);
            assert_eq!(produced.chunks.len(), 1);
            assert_eq!(produced.chunks[0].content, expected);
            if json.contains("EXT-SECRET-99") {
                assert!(!produced.chunks[0].content.contains("EXT-SECRET-99"));
                assert!(!produced.chunks[0].content.contains("TAB-ID-77"));
            }
            if json.contains(r#""front":false,"focused":true"#) {
                assert!(!produced.chunks[0].content.contains("window focused"));
            }
        }
    }

    #[test]
    fn delta_attention_memory_and_reset() {
        let records_b = parse_jsonl_objects(
            r#"{"t":"segment_start","ts":1,"inst":"shared","ctx":"shared","tab":{"front":true,"focused":true,"age_ms":10800000},"blocks":[{"text":"Hello"}]}
{"t":"delta","ts":2,"op":"add","inst":"shared","ctx":"shared","tab":{"front":true,"focused":true,"age_ms":10800000},"lang":"en-US","block":{"text":"World"}}
"#,
        );
        let produced_b = render(&records_b);
        assert!(produced_b.warnings.is_empty());
        assert_eq!(produced_b.error, None);
        assert_eq!(produced_b.chunks.len(), 2);
        assert!(
            produced_b.chunks[0]
                .content
                .contains("tab open at least 3h")
        );
        assert!(produced_b.chunks[0].content.contains("context:"));
        assert_eq!(produced_b.chunks[1].content, "World");

        let records_a = parse_jsonl_objects(
            r#"{"t":"delta","ts":1,"op":"add","inst":"shared","ctx":"shared","tab":{"front":true,"focused":true,"age_ms":10800000},"lang":"en-US","browser":{"vendor":"safari"},"block":{"text":"Added"}}"#,
        );
        let produced_a = render(&records_a);
        assert!(produced_a.warnings.is_empty());
        assert_eq!(produced_a.error, None);
        assert_eq!(
            produced_a
                .chunks
                .iter()
                .map(|c| c.content.as_str())
                .collect::<Vec<_>>(),
            vec!["context: front tab, window focused\nAdded"]
        );
        assert!(!produced_a.chunks[0].content.contains("3h"));
        assert!(!produced_a.chunks[0].content.contains("language"));
        assert!(!produced_a.chunks[0].content.contains("safari"));

        let records_c = parse_jsonl_objects(
            r#"{"t":"segment_start","ts":1,"inst":"c","ctx":"c","tab":{"front":true,"focused":true}}
{"t":"delta","ts":2,"op":"update","inst":"c","ctx":"c","tab":{"front":false},"block":{"text":"Edited"}}
"#,
        );
        let produced_c = render(&records_c);
        assert!(produced_c.warnings.is_empty());
        assert_eq!(produced_c.error, None);
        assert_eq!(
            produced_c
                .chunks
                .iter()
                .map(|c| c.content.as_str())
                .collect::<Vec<_>>(),
            vec![
                "## Browser Page\ncontext: front tab, window focused",
                "context: background tab\nEdited",
            ]
        );

        let records_d1 = parse_jsonl_objects(
            r#"{"t":"segment_start","ts":1,"inst":"one","ctx":"alpha","tab":{"front":true,"focused":true}}
{"t":"delta","ts":2,"op":"add","inst":"two","ctx":"alpha","tab":{"front":true,"focused":true},"block":{"text":"Other"}}
"#,
        );
        let produced_d1 = render(&records_d1);
        assert!(produced_d1.warnings.is_empty());
        assert_eq!(produced_d1.error, None);
        assert_eq!(
            produced_d1
                .chunks
                .iter()
                .map(|c| c.content.as_str())
                .collect::<Vec<_>>(),
            vec![
                "## Browser Page\ncontext: front tab, window focused",
                "context: front tab, window focused\nOther",
            ]
        );

        let records_d2 = parse_jsonl_objects(
            r#"{"t":"segment_start","ts":1,"inst":"","ctx":"x","tab":{"front":true,"focused":true}}
{"t":"delta","ts":2,"op":"add","ctx":"x","tab":{"front":true,"focused":true},"block":{"text":"After1"}}
"#,
        );
        let produced_d2 = render(&records_d2);
        assert!(produced_d2.warnings.is_empty());
        assert_eq!(produced_d2.error, None);
        assert_eq!(
            produced_d2
                .chunks
                .iter()
                .map(|c| c.content.as_str())
                .collect::<Vec<_>>(),
            vec![
                "## Browser Page\ncontext: front tab, window focused",
                "context: front tab, window focused\nAfter1",
            ]
        );

        let records_d3 = parse_jsonl_objects(
            r#"{"t":"segment_start","ts":1,"inst":"a ","ctx":"x","tab":{"front":true,"focused":true}}
{"t":"delta","ts":2,"op":"add","inst":"a","ctx":"x","tab":{"front":true,"focused":true},"block":{"text":"After2"}}
"#,
        );
        let produced_d3 = render(&records_d3);
        assert!(produced_d3.warnings.is_empty());
        assert_eq!(produced_d3.error, None);
        assert_eq!(
            produced_d3
                .chunks
                .iter()
                .map(|c| c.content.as_str())
                .collect::<Vec<_>>(),
            vec![
                "## Browser Page\ncontext: front tab, window focused",
                "context: front tab, window focused\nAfter2",
            ]
        );

        let records_e = parse_jsonl_objects(
            r#"{"t":"segment_start","ts":1,"tab":{"front":true,"focused":true},"lang":"en-US"}
{"t":"delta","ts":2,"op":"add","block":{"text":"Legacy line"}}
{"t":"delta","ts":3,"op":"add","tab":{"front":true,"focused":true},"block":{"text":"Still"}}
"#,
        );
        let produced_e = render(&records_e);
        assert!(produced_e.warnings.is_empty());
        assert_eq!(produced_e.error, None);
        assert_eq!(
            produced_e
                .chunks
                .iter()
                .map(|c| c.content.as_str())
                .collect::<Vec<_>>(),
            vec![
                "## Browser Page\ncontext: front tab, window focused · language en-US",
                "Legacy line",
                "Still",
            ]
        );

        let records_f1 = parse_jsonl_objects(
            r#"{"t":"segment_start","ts":1,"tab":{"front":true,"focused":true}}
{"t":"delta","ts":2,"op":"remove","tab":{"front":false},"block":{"text":"gone"}}
{"t":"delta","ts":3,"op":"add","tab":{"front":false},"block":{"text":"kept"}}
"#,
        );
        let produced_f1 = render(&records_f1);
        assert!(produced_f1.warnings.is_empty());
        assert_eq!(produced_f1.error, None);
        assert_eq!(
            produced_f1
                .chunks
                .iter()
                .map(|c| c.content.as_str())
                .collect::<Vec<_>>(),
            vec![
                "## Browser Page\ncontext: front tab, window focused",
                "context: background tab\nkept",
            ]
        );

        let records_f2 = parse_jsonl_objects(
            r#"{"t":"segment_start","ts":1,"tab":{"front":true,"focused":true}}
{"t":"delta","ts":2,"op":"add","tab":{"front":false},"block":{"text":"   "}}
{"t":"delta","ts":3,"op":"add","tab":{"front":false},"block":{"text":"kept"}}
"#,
        );
        let produced_f2 = render(&records_f2);
        assert!(produced_f2.warnings.is_empty());
        assert_eq!(produced_f2.error, None);
        assert_eq!(
            produced_f2
                .chunks
                .iter()
                .map(|c| c.content.as_str())
                .collect::<Vec<_>>(),
            vec![
                "## Browser Page\ncontext: front tab, window focused",
                "context: background tab\nkept",
            ]
        );

        let records_g = parse_jsonl_objects(
            r#"{"t":"delta","ts":1,"op":"add","tab":{"front":false},"block":{"text":"First"}}
{"t":"segment_start","ts":2,"tab":{"front":true,"focused":true}}
{"t":"delta","ts":3,"op":"add","tab":{"front":false},"block":{"text":"Again"}}
"#,
        );
        let produced_g = render(&records_g);
        assert!(produced_g.warnings.is_empty());
        assert_eq!(produced_g.error, None);
        assert_eq!(
            produced_g
                .chunks
                .iter()
                .map(|c| c.content.as_str())
                .collect::<Vec<_>>(),
            vec![
                "context: background tab\nFirst",
                "## Browser Page\ncontext: front tab, window focused",
                "context: background tab\nAgain",
            ]
        );

        let records_h = parse_jsonl_objects(
            r#"{"t":"delta","ts":1,"op":"add","tab":{"front":true,"focused":true},"block":{"text":"   "}}"#,
        );
        let produced_h = render(&records_h);
        assert!(produced_h.warnings.is_empty());
        assert_eq!(produced_h.error, None);
        assert!(produced_h.chunks.is_empty());

        let records_reset = parse_jsonl_objects(
            r#"{"t":"segment_start","ts":1,"tab":{"front":true,"focused":true}}
{"t":"segment_start","ts":2,"lang":"en"}
{"t":"delta","ts":3,"op":"add","tab":{"front":true,"focused":true},"block":{"text":"After"}}
"#,
        );
        let produced_reset = render(&records_reset);
        assert!(produced_reset.warnings.is_empty());
        assert_eq!(produced_reset.error, None);
        assert_eq!(
            produced_reset
                .chunks
                .iter()
                .map(|c| c.content.as_str())
                .collect::<Vec<_>>(),
            vec![
                "## Browser Page\ncontext: front tab, window focused",
                "## Browser Page\ncontext: language en",
                "context: front tab, window focused\nAfter",
            ]
        );
    }

    #[test]
    fn malformed_field_handling() {
        let with_string_tab =
            parse_jsonl_objects(r#"{"t":"segment_start","ts":1,"tab":"x","lang":"en"}"#);
        let without_tab = parse_jsonl_objects(r#"{"t":"segment_start","ts":1,"lang":"en"}"#);
        assert_eq!(
            render(&with_string_tab).chunks[0].content,
            render(&without_tab).chunks[0].content
        );

        let with_string_media = parse_jsonl_objects(
            r#"{"t":"segment_start","ts":1,"media":{"playing":"yes"},"lang":"en"}"#,
        );
        let without_media = parse_jsonl_objects(r#"{"t":"segment_start","ts":1,"lang":"en"}"#);
        assert_eq!(
            render(&with_string_media).chunks[0].content,
            render(&without_media).chunks[0].content
        );

        let neg_age =
            parse_jsonl_objects(r#"{"t":"segment_start","ts":1,"tab":{"front":true,"age_ms":-5}}"#);
        let pos_tab = parse_jsonl_objects(r#"{"t":"segment_start","ts":1,"tab":{"front":true}}"#);
        let prod_neg = render(&neg_age);
        assert_eq!(
            prod_neg.chunks[0].content,
            render(&pos_tab).chunks[0].content
        );
        assert_eq!(
            prod_neg.chunks[0].content,
            "## Browser Page\ncontext: front tab"
        );

        let multiline_lang =
            parse_jsonl_objects(r#"{"t":"segment_start","ts":1,"lang":"en\n## x"}"#);
        let prod_lang = render(&multiline_lang);
        assert_eq!(
            prod_lang.chunks[0].content,
            "## Browser Page\ncontext: language en ## x"
        );
        assert_eq!(
            prod_lang.chunks[0]
                .content
                .lines()
                .filter(|l| l.starts_with("context:"))
                .count(),
            1
        );

        let v65 = "a".repeat(65);
        let v200 = "b".repeat(200);
        for v in [v65, v200] {
            let json = format!(
                r#"{{"t":"segment_start","ts":1,"lang":"en","browser":{{"vendor":"safari","version":"{v}"}}}}"#
            );
            let prod = render(&parse_jsonl_objects(&json));
            assert_eq!(
                prod.chunks[0].content,
                "## Browser Page\ncontext: language en · safari"
            );
            assert!(!prod.chunks[0].content.contains(&v));
        }

        let v64 = "c".repeat(64);
        let json64 = format!(
            r#"{{"t":"segment_start","ts":1,"lang":"en","browser":{{"vendor":"safari","version":"{v64}"}}}}"#
        );
        let prod64 = render(&parse_jsonl_objects(&json64));
        assert_eq!(
            prod64.chunks[0].content,
            format!("## Browser Page\ncontext: language en · safari {v64}")
        );
    }
}
