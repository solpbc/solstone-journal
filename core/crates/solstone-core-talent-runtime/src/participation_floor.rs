// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::collections::HashSet;
use std::fs;
use std::path::Path;

use serde_json::{Map, Value, json};
use solstone_core_transcripts::{ENTRY_SOURCE_MAX_BYTES, EntrySource, is_speech_transcript_file};

use crate::prepare::SPARSE_INPUT_NOTE;

const FRAMING_TOKENS: u32 = 256;
const MIN_ENTRY_CAP: usize = 4;
const INCOMPLETE_MEMORY_NOTICE: &str = "agent memory context is incomplete.";

const ENTRY_HEADINGS: &[&str] = &[
    "### Transcript",
    "### ChatGPT Conversation",
    "### Claude Conversation",
    "### Gemini Conversation",
    "### Calendar Event",
    "### Note",
    "### Highlights",
    "### agent memory original",
    "### Screen Activity",
    "### Browser Content",
    "### Protected video",
    "### desktop activity",
    "### sense summary",
];

#[derive(Debug, Clone)]
pub(crate) struct FloorSample {
    pub(crate) transcript: String,
    pub(crate) event: Value,
}

fn format_disclosure_transcript(
    has_sparse: bool,
    disclosure: &str,
    has_incomplete_trailer: bool,
) -> String {
    let mut out = String::new();
    if has_sparse {
        out.push_str(SPARSE_INPUT_NOTE);
    }
    out.push_str(disclosure);
    out.push('\n');
    if has_incomplete_trailer {
        out.push_str(&format!("\n{INCOMPLETE_MEMORY_NOTICE}"));
    }
    out
}

pub(crate) fn apply(
    composed: &mut Map<String, Value>,
    sources: &[Option<EntrySource>],
    journal: &Path,
) -> Result<Value, String> {
    let max_output_tokens = match composed.get("max_output_tokens") {
        Some(Value::Number(n)) if n.as_u64().is_some() => n.as_u64().unwrap() as u32,
        _ => return Err("missing or invalid max_output_tokens".to_owned()),
    };
    let floor_input = solstone_core_local::generate::compute_input_budget(
        max_output_tokens,
        solstone_core_local::plan::LOCAL_MIN_CONTEXT_TOKENS,
    );

    let preserved = [
        composed.get("user_instruction").and_then(Value::as_str),
        composed.get("prompt").and_then(Value::as_str),
        composed.get("_daily_location_text").and_then(Value::as_str),
    ];
    let preserved_tokens: u32 = preserved
        .into_iter()
        .flatten()
        .filter(|t| !t.is_empty())
        .map(solstone_core_tokens::count)
        .sum();

    let transcript = composed
        .get("transcript")
        .and_then(Value::as_str)
        .unwrap_or("");
    let day = composed
        .get("day")
        .and_then(Value::as_str)
        .unwrap_or_default();

    let (has_sparse, text_without_sparse) = strip_sparse_prefix(transcript);
    let (has_incomplete_trailer, text_to_parse) = strip_incomplete_trailer(text_without_sparse);

    if preserved_tokens.saturating_add(FRAMING_TOKENS) > floor_input {
        // Over-floor path: request framing alone exceeds floor
        let parsed = parse_all(text_to_parse, sources, journal, day)?;
        let disclosure = over_floor_disclosure(
            parsed.total_speech_lines,
            parsed.total_sense_segments,
            parsed.total_pool_entries,
        );
        let out_text =
            format_disclosure_transcript(has_sparse, &disclosure, has_incomplete_trailer);
        composed.insert("transcript".to_owned(), Value::String(out_text));
        let event = build_event(
            0,
            parsed.total_speech_lines,
            0,
            parsed.total_sense_segments,
            0,
            parsed.total_pool_entries,
            0,
            0,
            Some(disclosure),
        );
        return Ok(event);
    }

    if solstone_core_tokens::count(transcript)
        .saturating_add(preserved_tokens)
        .saturating_add(FRAMING_TOKENS)
        <= floor_input
    {
        // Fits whole without sampling
        let parsed = parse_all(text_to_parse, sources, journal, day)?;
        let event = build_event(
            parsed.total_speech_lines,
            0,
            parsed.total_sense_segments,
            0,
            parsed.total_pool_entries,
            0,
            0,
            0,
            None,
        );
        return Ok(event);
    }

    let transcript_budget = floor_input - preserved_tokens - FRAMING_TOKENS;
    let sampled = sample_to_budget(transcript, sources, journal, day, transcript_budget)?;
    composed.insert("transcript".to_owned(), Value::String(sampled.transcript));
    Ok(sampled.event)
}

pub(crate) fn sample_to_budget(
    transcript: &str,
    sources: &[Option<EntrySource>],
    journal: &Path,
    day: &str,
    transcript_budget: u32,
) -> Result<FloorSample, String> {
    let (has_sparse, text_without_sparse) = strip_sparse_prefix(transcript);
    let (has_incomplete_trailer, text_to_parse) = strip_incomplete_trailer(text_without_sparse);

    let parsed = parse_all(text_to_parse, sources, journal, day)?;

    if solstone_core_tokens::count(transcript) <= transcript_budget {
        return Ok(FloorSample {
            transcript: transcript.to_owned(),
            event: build_event(
                parsed.total_speech_lines,
                0,
                parsed.total_sense_segments,
                0,
                parsed.total_pool_entries,
                0,
                0,
                0,
                None,
            ),
        });
    }

    let sparse_tokens = if has_sparse {
        solstone_core_tokens::count(SPARSE_INPUT_NOTE)
    } else {
        0
    };
    let trailer_tokens = if has_incomplete_trailer {
        solstone_core_tokens::count(INCOMPLETE_MEMORY_NOTICE) + 1
    } else {
        0
    };

    let room = transcript_budget.saturating_sub(sparse_tokens + trailer_tokens);
    if room == 0 {
        let disclosure = over_floor_disclosure(
            parsed.total_speech_lines,
            parsed.total_sense_segments,
            parsed.total_pool_entries,
        );
        let out_text =
            format_disclosure_transcript(has_sparse, &disclosure, has_incomplete_trailer);
        return Ok(FloorSample {
            transcript: out_text,
            event: build_event(
                0,
                parsed.total_speech_lines,
                0,
                parsed.total_sense_segments,
                0,
                parsed.total_pool_entries,
                0,
                0,
                Some(disclosure),
            ),
        });
    }

    let heading_tokens: u32 = parsed
        .groups
        .iter()
        .map(|g| solstone_core_tokens::count(&g.heading_line) + 1)
        .sum();
    let room_entries = room.saturating_sub(heading_tokens);
    if room_entries == 0 {
        let disclosure = over_floor_disclosure(
            parsed.total_speech_lines,
            parsed.total_sense_segments,
            parsed.total_pool_entries,
        );
        let out_text =
            format_disclosure_transcript(has_sparse, &disclosure, has_incomplete_trailer);
        return Ok(FloorSample {
            transcript: out_text,
            event: build_event(
                0,
                parsed.total_speech_lines,
                0,
                parsed.total_sense_segments,
                0,
                parsed.total_pool_entries,
                0,
                0,
                Some(disclosure),
            ),
        });
    }

    let share = (room_entries - room_entries / 4) as u64;
    let full_speech_tokens = parsed.full_speech_tokens;
    let full_sense_tokens = parsed.full_sense_tokens;

    let (speech_budget, sense_budget) = if full_speech_tokens == 0 && full_sense_tokens == 0 {
        (0, 0)
    } else if full_speech_tokens == 0 {
        (0, share as u32)
    } else if full_sense_tokens == 0 {
        (share as u32, 0)
    } else if full_speech_tokens + full_sense_tokens <= share as u32 {
        (full_speech_tokens, full_sense_tokens)
    } else if share < 2 {
        (0, 0)
    } else if full_speech_tokens as u64 > share && full_sense_tokens as u64 > share {
        let raw = (share * full_speech_tokens as u64)
            / (full_speech_tokens as u64 + full_sense_tokens as u64);
        let sb = raw.clamp(1, share - 1);
        (sb as u32, (share - sb) as u32)
    } else {
        let raw = (share * full_speech_tokens as u64)
            / (full_speech_tokens as u64 + full_sense_tokens as u64);
        let sb = raw.clamp(1, share - 1);
        let sense_b = share - sb;
        if full_speech_tokens as u64 <= sb {
            (
                full_speech_tokens,
                (share - full_speech_tokens as u64) as u32,
            )
        } else if full_sense_tokens as u64 <= sense_b {
            ((share - full_sense_tokens as u64) as u32, full_sense_tokens)
        } else {
            (sb as u32, sense_b as u32)
        }
    };

    let mut speech_sample = sample_speech(&parsed, speech_budget);
    let mut sense_sample = sample_sense(&parsed, sense_budget);

    let used_share = speech_sample.tokens + sense_sample.tokens;
    let pool_budget = room_entries.saturating_sub(used_share);

    let initial_pool = sample_pool(&parsed, pool_budget);

    // Binary search on pool kept-entry count to fit within transcript_budget
    let upper_pool_kept = initial_pool.kept_count;
    let mut best_pool = sample_pool_with_count(&parsed, pool_budget, 0);
    let mut best_transcript = render_assembled(
        &parsed,
        &speech_sample,
        &sense_sample,
        &best_pool,
        has_sparse,
        has_incomplete_trailer,
    );

    let pool_sample = if solstone_core_tokens::count(&best_transcript) <= transcript_budget {
        let whole_transcript = render_assembled(
            &parsed,
            &speech_sample,
            &sense_sample,
            &initial_pool,
            has_sparse,
            has_incomplete_trailer,
        );
        if solstone_core_tokens::count(&whole_transcript) <= transcript_budget {
            best_pool = initial_pool;
            best_transcript = whole_transcript;
        } else {
            let mut low = 0usize;
            let mut high = upper_pool_kept;
            while low < high {
                let mid = low + (high - low).div_ceil(2);
                let candidate_pool = sample_pool_with_count(&parsed, pool_budget, mid);
                let candidate_text = render_assembled(
                    &parsed,
                    &speech_sample,
                    &sense_sample,
                    &candidate_pool,
                    has_sparse,
                    has_incomplete_trailer,
                );
                if solstone_core_tokens::count(&candidate_text) <= transcript_budget {
                    low = mid;
                    best_pool = candidate_pool;
                    best_transcript = candidate_text;
                } else {
                    high = mid - 1;
                }
            }
        }
        best_pool
    } else {
        // Even with pool kept_count = 0, it doesn't fit! Drop Sense segments:
        let upper_sense_kept = sense_sample.kept;
        let mut best_sense = sample_sense_with_count(&parsed, 0);
        let mut s_transcript = render_assembled(
            &parsed,
            &speech_sample,
            &best_sense,
            &best_pool,
            has_sparse,
            has_incomplete_trailer,
        );

        if solstone_core_tokens::count(&s_transcript) <= transcript_budget {
            let mut s_low = 0usize;
            let mut s_high = upper_sense_kept;
            while s_low < s_high {
                let mid = s_low + (s_high - s_low).div_ceil(2);
                let candidate_sense = sample_sense_with_count(&parsed, mid);
                let candidate_text = render_assembled(
                    &parsed,
                    &speech_sample,
                    &candidate_sense,
                    &best_pool,
                    has_sparse,
                    has_incomplete_trailer,
                );
                if solstone_core_tokens::count(&candidate_text) <= transcript_budget {
                    s_low = mid;
                    best_sense = candidate_sense;
                    s_transcript = candidate_text;
                } else {
                    s_high = mid - 1;
                }
            }
            sense_sample = best_sense;
            best_transcript = s_transcript;
        } else {
            // Even with sense kept = 0, it doesn't fit! Drop speech lines:
            sense_sample = best_sense;
            let upper_speech_kept = speech_sample.kept;
            let mut best_speech = sample_speech_with_count(&parsed, 0);
            let mut sp_transcript = render_assembled(
                &parsed,
                &best_speech,
                &sense_sample,
                &best_pool,
                has_sparse,
                has_incomplete_trailer,
            );
            let mut sp_low = 0usize;
            let mut sp_high = upper_speech_kept;
            while sp_low < sp_high {
                let mid = sp_low + (sp_high - sp_low).div_ceil(2);
                let candidate_speech = sample_speech_with_count(&parsed, mid);
                let candidate_text = render_assembled(
                    &parsed,
                    &candidate_speech,
                    &sense_sample,
                    &best_pool,
                    has_sparse,
                    has_incomplete_trailer,
                );
                if solstone_core_tokens::count(&candidate_text) <= transcript_budget {
                    sp_low = mid;
                    best_speech = candidate_speech;
                    sp_transcript = candidate_text;
                } else {
                    sp_high = mid - 1;
                }
            }
            speech_sample = best_speech;
            best_transcript = sp_transcript;
        }
        best_pool
    };

    let final_transcript = best_transcript;

    if solstone_core_tokens::count(&final_transcript) > transcript_budget {
        let disclosure = over_floor_disclosure(
            parsed.total_speech_lines,
            parsed.total_sense_segments,
            parsed.total_pool_entries,
        );
        let out_text =
            format_disclosure_transcript(has_sparse, &disclosure, has_incomplete_trailer);
        return Ok(FloorSample {
            transcript: out_text,
            event: build_event(
                0,
                parsed.total_speech_lines,
                0,
                parsed.total_sense_segments,
                0,
                parsed.total_pool_entries,
                0,
                0,
                Some(disclosure),
            ),
        });
    }

    if speech_sample.omitted == 0
        && sense_sample.omitted == 0
        && pool_sample.omitted == 0
        && pool_sample.frames_omitted == 0
        && pool_sample.lines_omitted == 0
    {
        return Ok(FloorSample {
            transcript: transcript.to_owned(),
            event: build_event(
                parsed.total_speech_lines,
                0,
                parsed.total_sense_segments,
                0,
                parsed.total_pool_entries,
                0,
                0,
                0,
                None,
            ),
        });
    }

    let event = build_event(
        speech_sample.kept,
        speech_sample.omitted,
        sense_sample.kept,
        sense_sample.omitted,
        pool_sample.kept_count,
        pool_sample.omitted,
        pool_sample.frames_omitted,
        pool_sample.lines_omitted,
        None,
    );

    Ok(FloorSample {
        transcript: final_transcript,
        event,
    })
}

fn strip_sparse_prefix(text: &str) -> (bool, &str) {
    if let Some(stripped) = text.strip_prefix(SPARSE_INPUT_NOTE) {
        (true, stripped)
    } else {
        (false, text)
    }
}

fn strip_incomplete_trailer(text: &str) -> (bool, &str) {
    let trimmed = text.trim_end_matches('\n');
    if let Some(stripped) = trimmed.strip_suffix(INCOMPLETE_MEMORY_NOTICE) {
        (true, stripped.trim_end_matches('\n'))
    } else {
        (false, text)
    }
}

fn over_floor_disclosure(
    speech_lines: usize,
    sense_segments: usize,
    pool_entries: usize,
) -> String {
    let speech_str = if speech_lines == 1 {
        "speech line"
    } else {
        "speech lines"
    };
    let sense_str = if sense_segments == 1 {
        "Sense segment"
    } else {
        "Sense segments"
    };
    let entry_str = if pool_entries == 1 {
        "other entry"
    } else {
        "other entries"
    };
    format!(
        "Activity input omitted to fit context floor: {speech_lines} {speech_str} left out, {sense_segments} {sense_str} left out, {pool_entries} {entry_str} left out."
    )
}

#[allow(clippy::too_many_arguments)]
fn build_event(
    speech_kept: usize,
    speech_omitted: usize,
    sense_kept: usize,
    sense_omitted: usize,
    entries_kept: usize,
    entries_omitted: usize,
    frames_omitted: usize,
    lines_omitted: usize,
    disclosure: Option<String>,
) -> Value {
    json!({
        "event": "participation_floor",
        "talent": "participation",
        "speech_lines_kept": speech_kept,
        "speech_lines_omitted": speech_omitted,
        "sense_kept": sense_kept,
        "sense_omitted": sense_omitted,
        "entries_kept": entries_kept,
        "entries_omitted": entries_omitted,
        "frames_omitted": frames_omitted,
        "lines_omitted": lines_omitted,
        "disclosure": disclosure,
    })
}

fn is_segment_group_heading(line: &str) -> bool {
    let Some(rest) = line.strip_prefix("## ") else {
        return false;
    };
    if rest.len() != 30 {
        return false;
    }
    let bytes = rest.as_bytes();
    bytes[0..4].iter().all(u8::is_ascii_digit)
        && bytes[4] == b'-'
        && bytes[5..7].iter().all(u8::is_ascii_digit)
        && bytes[7] == b'-'
        && bytes[8..10].iter().all(u8::is_ascii_digit)
        && bytes[10] == b' '
        && bytes[11..13].iter().all(u8::is_ascii_digit)
        && bytes[13] == b':'
        && bytes[14..16].iter().all(u8::is_ascii_digit)
        && bytes[16] == b':'
        && bytes[17..19].iter().all(u8::is_ascii_digit)
        && &bytes[19..22] == b" - "
        && bytes[22..24].iter().all(u8::is_ascii_digit)
        && bytes[24] == b':'
        && bytes[25..27].iter().all(u8::is_ascii_digit)
        && bytes[27] == b':'
        && bytes[28..30].iter().all(u8::is_ascii_digit)
}

fn parse_segment_key_and_times(heading: &str) -> (String, String, String, f64) {
    let rest = heading.strip_prefix("## ").unwrap_or(heading);
    let start_time = rest[11..19].to_owned();
    let end_time = rest[22..30].to_owned();

    let s_h: u32 = rest[11..13].parse().unwrap_or(0);
    let s_m: u32 = rest[14..16].parse().unwrap_or(0);
    let s_s: u32 = rest[17..19].parse().unwrap_or(0);

    let e_h: u32 = rest[22..24].parse().unwrap_or(0);
    let e_m: u32 = rest[25..27].parse().unwrap_or(0);
    let e_s: u32 = rest[28..30].parse().unwrap_or(0);

    let start_secs = s_h * 3600 + s_m * 60 + s_s;
    let end_secs = e_h * 3600 + e_m * 60 + e_s;
    let len = if end_secs <= start_secs {
        (end_secs + 86400) - start_secs
    } else {
        end_secs - start_secs
    } as usize;
    let seg_key = format!("{:02}{:02}{:02}_{len}", s_h, s_m, s_s);

    (seg_key, start_time, end_time, start_secs as f64)
}

fn is_entry_heading(line: &str) -> bool {
    ENTRY_HEADINGS.contains(&line.trim())
}

fn is_frame_heading(line: &str) -> bool {
    let Some(rest) = line.strip_prefix("### ") else {
        return false;
    };
    if rest.len() != 8 && rest.len() != 12 {
        return false;
    }
    let bytes = rest.as_bytes();
    bytes[0..2].iter().all(u8::is_ascii_digit)
        && bytes[2] == b':'
        && bytes[3..5].iter().all(u8::is_ascii_digit)
        && bytes[5] == b':'
        && bytes[6..8].iter().all(u8::is_ascii_digit)
        && (rest.len() == 8 || (bytes[8] == b'.' && bytes[9..12].iter().all(u8::is_ascii_digit)))
}

fn is_time_bracket_line(line: &str) -> bool {
    let bytes = line.as_bytes();
    bytes.len() >= 6
        && bytes[0] == b'['
        && bytes[1].is_ascii_digit()
        && bytes[2].is_ascii_digit()
        && bytes[3] == b':'
        && bytes[4].is_ascii_digit()
        && bytes[5].is_ascii_digit()
}

fn has_oversized_speech_file(journal: &Path, day: &str, segment_key: &str) -> bool {
    let chronicle = journal.join("chronicle").join(day);
    let direct = chronicle.join(segment_key);
    if check_dir_for_oversized_speech(&direct) {
        return true;
    }
    if let Ok(entries) = fs::read_dir(&chronicle) {
        for entry in entries.flatten() {
            if entry.path().is_dir() {
                let sub = entry.path().join(segment_key);
                if sub.is_dir() && check_dir_for_oversized_speech(&sub) {
                    return true;
                }
            }
        }
    }
    false
}

fn check_dir_for_oversized_speech(dir: &Path) -> bool {
    let Ok(entries) = fs::read_dir(dir) else {
        return false;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_file()
            && is_speech_transcript_file(&path)
            && fs::metadata(&path).map(|m| m.len()).unwrap_or(0) > ENTRY_SOURCE_MAX_BYTES as u64
        {
            return true;
        }
    }
    false
}

#[derive(Clone, Debug)]
struct ParsedGroup {
    heading_line: String,
    start_time: String,
    end_time: String,
    time_secs: f64,
    _segment_key: String,
    entries: Vec<ParsedEntry>,
}

#[derive(Clone, Debug)]
enum EntryKind {
    Speech {
        header: Option<String>,
        lines: Vec<String>,
    },
    Sense {
        body: String,
    },
    Pool(PoolKind),
}

#[derive(Clone, Debug)]
enum PoolKind {
    TimedFrames(Vec<String>),
    Ordinary(Vec<String>),
}

#[derive(Clone, Debug)]
struct ParsedEntry {
    heading: String,
    kind: EntryKind,
}

struct ParsedAll {
    groups: Vec<ParsedGroup>,
    total_speech_lines: usize,
    full_speech_tokens: u32,
    total_sense_segments: usize,
    full_sense_tokens: u32,
    total_pool_entries: usize,
}

fn parse_all(
    text: &str,
    sources: &[Option<EntrySource>],
    journal: &Path,
    day: &str,
) -> Result<ParsedAll, String> {
    let mut raw_groups = Vec::new();
    let mut current_heading: Option<String> = None;
    let mut current_entries: Vec<(String, String)> = Vec::new();
    let mut current_entry_heading: Option<String> = None;
    let mut current_entry_body = String::new();

    let flush_entry =
        |entries: &mut Vec<(String, String)>, h: &mut Option<String>, b: &mut String| {
            if let Some(heading) = h.take() {
                entries.push((heading, std::mem::take(b)));
            }
        };

    let flush_group = |groups: &mut Vec<(String, Vec<(String, String)>)>,
                       gh: &mut Option<String>,
                       entries: &mut Vec<(String, String)>,
                       h: &mut Option<String>,
                       b: &mut String| {
        flush_entry(entries, h, b);
        if let Some(heading) = gh.take() {
            groups.push((heading, std::mem::take(entries)));
        }
    };

    for line in text.split_inclusive('\n') {
        let trimmed_line = line.trim();
        if is_segment_group_heading(trimmed_line) {
            flush_group(
                &mut raw_groups,
                &mut current_heading,
                &mut current_entries,
                &mut current_entry_heading,
                &mut current_entry_body,
            );
            current_heading = Some(trimmed_line.to_owned());
        } else if is_entry_heading(trimmed_line) {
            flush_entry(
                &mut current_entries,
                &mut current_entry_heading,
                &mut current_entry_body,
            );
            current_entry_heading = Some(trimmed_line.to_owned());
        } else if current_entry_heading.is_some() {
            current_entry_body.push_str(line);
        }
    }
    flush_group(
        &mut raw_groups,
        &mut current_heading,
        &mut current_entries,
        &mut current_entry_heading,
        &mut current_entry_body,
    );

    let total_entries: usize = raw_groups.iter().map(|(_, es)| es.len()).sum();
    if total_entries != sources.len() {
        return Err(format!(
            "entry count mismatch: parsed {} entries but have {} entry sources",
            total_entries,
            sources.len()
        ));
    }

    let mut entry_idx = 0;
    let mut groups = Vec::new();
    let mut total_speech_lines = 0;
    let mut full_speech_tokens = 0;
    let mut total_sense_segments = 0;
    let mut full_sense_tokens = 0;
    let mut total_pool_entries = 0;

    for (gh, es) in raw_groups {
        let (segment_key, start_time, end_time, time_secs) = parse_segment_key_and_times(&gh);
        let mut parsed_entries = Vec::new();

        for (heading, body) in es {
            let src = &sources[entry_idx];
            entry_idx += 1;

            if heading == "### sense summary" {
                total_sense_segments += 1;
                full_sense_tokens += solstone_core_tokens::count(&format!("{heading}\n{body}"));
                parsed_entries.push(ParsedEntry {
                    heading,
                    kind: EntryKind::Sense { body },
                });
            } else {
                let is_speech = if let Some(s) = src {
                    s.is_speech_transcript()
                } else {
                    heading != "### agent memory original"
                        && body.lines().any(is_time_bracket_line)
                        && has_oversized_speech_file(journal, day, &segment_key)
                };

                if is_speech {
                    let all_lines: Vec<&str> = body.lines().collect();
                    let first_ts = all_lines
                        .iter()
                        .position(|l| is_time_bracket_line(l.trim()));
                    let (header, speech_lines) = if let Some(idx) = first_ts {
                        let h = all_lines[..idx]
                            .iter()
                            .map(|l| l.trim())
                            .filter(|l| !l.is_empty())
                            .collect::<Vec<_>>()
                            .join("\n");
                        let header_opt = if h.is_empty() { None } else { Some(h) };
                        let sl = all_lines[idx..]
                            .iter()
                            .map(|l| l.trim())
                            .filter(|l| !l.is_empty())
                            .map(str::to_owned)
                            .collect::<Vec<_>>();
                        (header_opt, sl)
                    } else {
                        let sl = all_lines
                            .iter()
                            .map(|l| l.trim())
                            .filter(|l| !l.is_empty())
                            .map(str::to_owned)
                            .collect::<Vec<_>>();
                        (None, sl)
                    };
                    total_speech_lines += speech_lines.len();
                    full_speech_tokens +=
                        solstone_core_tokens::count(&format!("{heading}\n{body}"));
                    parsed_entries.push(ParsedEntry {
                        heading,
                        kind: EntryKind::Speech {
                            header,
                            lines: speech_lines,
                        },
                    });
                } else {
                    total_pool_entries += 1;
                    let is_timed =
                        heading == "### Screen Activity" && body.lines().any(is_frame_heading);
                    if is_timed {
                        let mut frames = Vec::new();
                        let mut cur_frame = String::new();
                        for line in body.split_inclusive('\n') {
                            if is_frame_heading(line.trim()) && !cur_frame.is_empty() {
                                frames.push(std::mem::take(&mut cur_frame));
                            }
                            cur_frame.push_str(line);
                        }
                        if !cur_frame.is_empty() {
                            frames.push(cur_frame);
                        }
                        parsed_entries.push(ParsedEntry {
                            heading,
                            kind: EntryKind::Pool(PoolKind::TimedFrames(frames)),
                        });
                    } else {
                        let lines: Vec<String> =
                            body.split_inclusive('\n').map(str::to_owned).collect();
                        parsed_entries.push(ParsedEntry {
                            heading,
                            kind: EntryKind::Pool(PoolKind::Ordinary(lines)),
                        });
                    }
                }
            }
        }

        groups.push(ParsedGroup {
            heading_line: gh,
            start_time,
            end_time,
            time_secs,
            _segment_key: segment_key,
            entries: parsed_entries,
        });
    }

    Ok(ParsedAll {
        groups,
        total_speech_lines,
        full_speech_tokens,
        total_sense_segments,
        full_sense_tokens,
        total_pool_entries,
    })
}

struct SpeechSample {
    kept: usize,
    omitted: usize,
    tokens: u32,
    lines_by_entry: Vec<(usize, usize, Vec<String>)>,
}

fn get_speech_lines(parsed: &ParsedAll) -> Vec<(usize, usize, String)> {
    let mut all_lines: Vec<(usize, usize, String)> = Vec::new();
    for (g_idx, group) in parsed.groups.iter().enumerate() {
        for (e_idx, entry) in group.entries.iter().enumerate() {
            if let EntryKind::Speech { lines, .. } = &entry.kind {
                for line in lines {
                    all_lines.push((g_idx, e_idx, line.clone()));
                }
            }
        }
    }
    all_lines
}

fn sample_speech_candidate(
    parsed: &ParsedAll,
    all_lines: &[(usize, usize, String)],
    n: usize,
) -> SpeechSample {
    let total = all_lines.len();
    if total == 0 {
        return SpeechSample {
            kept: 0,
            omitted: 0,
            tokens: 0,
            lines_by_entry: Vec::new(),
        };
    }

    let n = n.min(total);
    let indices: Vec<usize> = if n == 0 {
        Vec::new()
    } else if n == 1 {
        vec![total - 1]
    } else {
        (0..n).map(|i| i * (total - 1) / (n - 1)).collect()
    };

    let omitted = total - indices.len();
    let mut grouped: Vec<(usize, usize, Vec<String>)> = Vec::new();

    for &idx in &indices {
        let (g, e, ref line) = all_lines[idx];
        if let Some(last) = grouped.last_mut()
            && last.0 == g
            && last.1 == e
        {
            last.2.push(line.clone());
            continue;
        }
        grouped.push((g, e, vec![line.clone()]));
    }

    let mut rendered = String::new();
    if omitted > 0 {
        rendered.push_str(&format!("Speech lines left out: {omitted}\n\n"));
    }
    for (g, e, lines) in &grouped {
        let entry = &parsed.groups[*g].entries[*e];
        let heading = &entry.heading;
        if let EntryKind::Speech {
            header: Some(h), ..
        } = &entry.kind
        {
            rendered.push_str(&format!("{heading}\n{h}\n{}\n\n", lines.join("\n")));
        } else {
            rendered.push_str(&format!("{heading}\n{}\n\n", lines.join("\n")));
        }
    }

    let tokens = solstone_core_tokens::count(&rendered);
    SpeechSample {
        kept: indices.len(),
        omitted,
        tokens,
        lines_by_entry: grouped,
    }
}

fn sample_speech_with_count(parsed: &ParsedAll, n: usize) -> SpeechSample {
    let all_lines = get_speech_lines(parsed);
    sample_speech_candidate(parsed, &all_lines, n)
}

fn sample_speech(parsed: &ParsedAll, budget: u32) -> SpeechSample {
    let all_lines = get_speech_lines(parsed);
    let total = all_lines.len();
    if total == 0 {
        return SpeechSample {
            kept: 0,
            omitted: 0,
            tokens: 0,
            lines_by_entry: Vec::new(),
        };
    }

    if parsed.full_speech_tokens <= budget {
        return sample_speech_candidate(parsed, &all_lines, total);
    }

    if sample_speech_candidate(parsed, &all_lines, 2).tokens <= budget {
        let mut low = 2usize;
        let mut high = total;
        let mut best_n = 2usize;
        while low < high {
            let mid = low + (high - low).div_ceil(2);
            let tried = sample_speech_candidate(parsed, &all_lines, mid);
            if tried.tokens <= budget {
                low = mid;
                best_n = mid;
            } else {
                high = mid - 1;
            }
        }
        sample_speech_candidate(parsed, &all_lines, best_n)
    } else {
        let try1 = sample_speech_candidate(parsed, &all_lines, 1);
        if try1.tokens <= budget {
            try1
        } else {
            sample_speech_candidate(parsed, &all_lines, 0)
        }
    }
}

struct SenseSample {
    kept: usize,
    omitted: usize,
    tokens: u32,
    kept_group_indices: HashSet<usize>,
}

fn get_sense_items(parsed: &ParsedAll) -> (Vec<(usize, f64, usize)>, Vec<usize>) {
    let mut sense_items: Vec<(usize, f64, usize)> = Vec::new();
    for (g_idx, group) in parsed.groups.iter().enumerate() {
        for entry in &group.entries {
            if let EntryKind::Sense { body } = &entry.kind {
                sense_items.push((g_idx, group.time_secs, body.len()));
            }
        }
    }
    let order = keep_order_items(&sense_items);
    (sense_items, order)
}

fn sample_sense_candidate(
    parsed: &ParsedAll,
    total: usize,
    order: &[usize],
    k: usize,
) -> SenseSample {
    if total == 0 {
        return SenseSample {
            kept: 0,
            omitted: 0,
            tokens: 0,
            kept_group_indices: HashSet::new(),
        };
    }

    let k = k.min(total);
    let kept_set: HashSet<usize> = if k == 0 {
        HashSet::new()
    } else {
        order[..k].iter().copied().collect()
    };

    let mut rendered = String::new();
    let mut kept_count = 0;
    for (g_idx, group) in parsed.groups.iter().enumerate() {
        for entry in &group.entries {
            if let EntryKind::Sense { body } = &entry.kind {
                if kept_set.contains(&g_idx) {
                    kept_count += 1;
                    rendered.push_str(&format!("### sense summary\n{body}\n\n"));
                } else {
                    rendered.push_str(&format!(
                        "Sense left out {} - {}\n\n",
                        group.start_time, group.end_time
                    ));
                }
            }
        }
    }

    let tokens = solstone_core_tokens::count(&rendered);
    SenseSample {
        kept: kept_count,
        omitted: total - kept_count,
        tokens,
        kept_group_indices: kept_set,
    }
}

fn sample_sense_with_count(parsed: &ParsedAll, k: usize) -> SenseSample {
    let (sense_items, order) = get_sense_items(parsed);
    sample_sense_candidate(parsed, sense_items.len(), &order, k)
}

fn sample_sense(parsed: &ParsedAll, budget: u32) -> SenseSample {
    let (sense_items, order) = get_sense_items(parsed);
    let total = sense_items.len();
    if total == 0 {
        return SenseSample {
            kept: 0,
            omitted: 0,
            tokens: 0,
            kept_group_indices: HashSet::new(),
        };
    }

    if parsed.full_sense_tokens <= budget {
        return sample_sense_candidate(parsed, total, &order, total);
    }

    let mut low = 0usize;
    let mut high = total;
    let mut best_k = 0usize;

    while low < high {
        let mid = low + (high - low).div_ceil(2);
        let tried = sample_sense_candidate(parsed, total, &order, mid);
        if tried.tokens <= budget {
            low = mid;
            best_k = mid;
        } else {
            high = mid - 1;
        }
    }

    sample_sense_candidate(parsed, total, &order, best_k)
}

#[derive(Clone, Debug)]
struct DroppedRun {
    group_idx: usize,
    start_t: String,
    end_t: String,
    count: usize,
    size: usize,
    unit: &'static str,
}

#[derive(Clone)]
struct PoolSample {
    kept_count: usize,
    omitted: usize,
    frames_omitted: usize,
    lines_omitted: usize,
    entry_renderings: Map<String, Value>,
    dropped_runs: Vec<DroppedRun>,
}

fn sample_pool(parsed: &ParsedAll, budget: u32) -> PoolSample {
    sample_pool_internal(parsed, budget, None)
}

fn sample_pool_with_count(parsed: &ParsedAll, budget: u32, max_kept: usize) -> PoolSample {
    sample_pool_internal(parsed, budget, Some(max_kept))
}

fn sample_pool_internal(
    parsed: &ParsedAll,
    budget: u32,
    forced_max_kept: Option<usize>,
) -> PoolSample {
    let mut pool_entries: Vec<(usize, usize, f64, usize, bool)> = Vec::new();
    for (g_idx, group) in parsed.groups.iter().enumerate() {
        for (e_idx, entry) in group.entries.iter().enumerate() {
            if let EntryKind::Pool(p) = &entry.kind {
                let (size, is_frames) = match p {
                    PoolKind::TimedFrames(f) => (f.len(), true),
                    PoolKind::Ordinary(l) => (l.len(), false),
                };
                pool_entries.push((g_idx, e_idx, group.time_secs, size, is_frames));
            }
        }
    }

    let total = pool_entries.len();
    if total == 0 {
        return PoolSample {
            kept_count: 0,
            omitted: 0,
            frames_omitted: 0,
            lines_omitted: 0,
            entry_renderings: Map::new(),
            dropped_runs: Vec::new(),
        };
    }

    let render_entry = |g: usize, e: usize, cap: Option<usize>| -> (String, usize, usize) {
        let entry = &parsed.groups[g].entries[e];
        let EntryKind::Pool(p) = &entry.kind else {
            return (String::new(), 0, 0);
        };
        match p {
            PoolKind::TimedFrames(frames) => {
                if let Some(cap) = cap.filter(|c| frames.len() > *c) {
                    let cap = cap.max(1);
                    let left = frames.len() - cap;
                    let indices: Vec<usize> = if cap == 1 {
                        vec![frames.len() - 1]
                    } else {
                        (0..cap)
                            .map(|i| i * (frames.len() - 1) / (cap - 1))
                            .collect()
                    };
                    let head = cap.div_ceil(2);
                    let mut body = String::new();
                    for &idx in &indices[..head] {
                        body.push_str(&frames[idx]);
                    }
                    body.push_str(&format!("{left} frames left out\n"));
                    for &idx in &indices[head..] {
                        body.push_str(&frames[idx]);
                    }
                    (format!("{}\n{body}\n\n", entry.heading), left, 0)
                } else {
                    (format!("{}\n{}\n\n", entry.heading, frames.join("")), 0, 0)
                }
            }
            PoolKind::Ordinary(lines) => {
                if let Some(cap) = cap.filter(|c| lines.len() > *c) {
                    let cap = cap.max(1);
                    let left = lines.len() - cap;
                    let head = cap.div_ceil(2);
                    let tail = cap - head;
                    let mut body = String::new();
                    for l in &lines[..head] {
                        body.push_str(l);
                    }
                    body.push_str(&format!("{left} lines left out\n"));
                    for l in &lines[lines.len() - tail..] {
                        body.push_str(l);
                    }
                    (format!("{}\n{body}\n\n", entry.heading), 0, left)
                } else {
                    (format!("{}\n{}\n\n", entry.heading, lines.join("")), 0, 0)
                }
            }
        }
    };

    let attempt_cap = |cap: Option<usize>| -> (u32, Map<String, Value>, usize, usize) {
        let mut text = String::new();
        let mut map = Map::new();
        let mut total_frames_omitted = 0;
        let mut total_lines_omitted = 0;
        for &(g, e, _, _, _) in &pool_entries {
            let (rendered, f_o, l_o) = render_entry(g, e, cap);
            text.push_str(&rendered);
            total_frames_omitted += f_o;
            total_lines_omitted += l_o;
            map.insert(format!("{g}_{e}"), Value::String(rendered));
        }
        (
            solstone_core_tokens::count(&text),
            map,
            total_frames_omitted,
            total_lines_omitted,
        )
    };

    if forced_max_kept.is_none() {
        let (whole_tokens, whole_map, _, _) = attempt_cap(None);
        if whole_tokens <= budget {
            return PoolSample {
                kept_count: total,
                omitted: 0,
                frames_omitted: 0,
                lines_omitted: 0,
                entry_renderings: whole_map,
                dropped_runs: Vec::new(),
            };
        }

        let max_size = pool_entries.iter().map(|e| e.3).max().unwrap_or(4);
        let min_cap = MIN_ENTRY_CAP;
        let (min_tokens, _, _, _) = attempt_cap(Some(min_cap));

        if min_tokens <= budget {
            let mut low = min_cap;
            let mut high = max_size.max(min_cap);
            let mut best_cap = min_cap;

            while low < high {
                let mid = low + (high - low).div_ceil(2);
                let (tokens, _, _, _) = attempt_cap(Some(mid));
                if tokens <= budget {
                    low = mid;
                    best_cap = mid;
                } else {
                    high = mid - 1;
                }
            }

            let (_, best_map, f_o, l_o) = attempt_cap(Some(best_cap));
            return PoolSample {
                kept_count: total,
                omitted: 0,
                frames_omitted: f_o,
                lines_omitted: l_o,
                entry_renderings: best_map,
                dropped_runs: Vec::new(),
            };
        }
    }

    // Must drop pool entries
    let pool_items: Vec<(usize, f64, usize)> = pool_entries
        .iter()
        .enumerate()
        .map(|(i, &(_, _, t, size, _))| (i, t, size))
        .collect();
    let order = keep_order_items(&pool_items);

    let render_with_kept = |kept_k: usize| {
        let kept_indices: HashSet<usize> = order[..kept_k].iter().copied().collect();
        let mut text = String::new();
        let mut map = Map::new();
        let mut frames_omitted = 0;
        let mut lines_omitted = 0;
        let mut dropped_runs = Vec::new();
        let mut current_run: Vec<(usize, usize, usize, bool)> = Vec::new();

        let flush_run = |runs: &mut Vec<DroppedRun>,
                         text: &mut String,
                         run: &mut Vec<(usize, usize, usize, bool)>| {
            if run.is_empty() {
                return;
            }
            let first_g = run.first().unwrap().0;
            let last_g = run.last().unwrap().0;
            let start_t = parsed.groups[first_g].start_time.clone();
            let end_t = parsed.groups[last_g].end_time.clone();
            let count = run.len();
            let size: usize = run.iter().map(|r| r.2).sum();
            let is_frames = run[0].3;
            let unit = if is_frames { "frames" } else { "lines" };
            let notice =
                format!("{count} entries left out {start_t} - {end_t} ({size} {unit})\n\n");
            text.push_str(&notice);
            runs.push(DroppedRun {
                group_idx: first_g,
                start_t,
                end_t,
                count,
                size,
                unit,
            });
            run.clear();
        };

        for (p_idx, &(g, e, _, size, is_frames)) in pool_entries.iter().enumerate() {
            if kept_indices.contains(&p_idx) {
                flush_run(&mut dropped_runs, &mut text, &mut current_run);
                let (rendered, f_o, l_o) = render_entry(g, e, Some(MIN_ENTRY_CAP));
                text.push_str(&rendered);
                frames_omitted += f_o;
                lines_omitted += l_o;
                map.insert(format!("{g}_{e}"), Value::String(rendered));
            } else {
                if let Some(last) = current_run.last()
                    && last.3 != is_frames
                {
                    flush_run(&mut dropped_runs, &mut text, &mut current_run);
                }
                current_run.push((g, e, size, is_frames));
            }
        }
        flush_run(&mut dropped_runs, &mut text, &mut current_run);

        let tokens = solstone_core_tokens::count(&text);
        (
            tokens,
            map,
            dropped_runs,
            frames_omitted,
            lines_omitted,
            kept_k,
        )
    };

    let upper_bound = forced_max_kept.unwrap_or(total);
    let mut low = 0usize;
    let mut high = upper_bound;
    let mut best = render_with_kept(0);

    while low < high {
        let mid = low + (high - low).div_ceil(2);
        let tried = render_with_kept(mid);
        if tried.0 <= budget {
            low = mid;
            best = tried;
        } else {
            high = mid - 1;
        }
    }

    PoolSample {
        kept_count: best.5,
        omitted: total - best.5,
        frames_omitted: best.3,
        lines_omitted: best.4,
        entry_renderings: best.1,
        dropped_runs: best.2,
    }
}

fn keep_order_items(items: &[(usize, f64, usize)]) -> Vec<usize> {
    let mut rest = items.to_vec();
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

fn render_assembled(
    parsed: &ParsedAll,
    speech: &SpeechSample,
    sense: &SenseSample,
    pool: &PoolSample,
    has_sparse: bool,
    has_incomplete_trailer: bool,
) -> String {
    let mut out = String::new();
    if has_sparse {
        out.push_str(SPARSE_INPUT_NOTE);
    }

    let mut speech_lines_map: Map<String, Value> = Map::new();
    for (g, e, lines) in &speech.lines_by_entry {
        speech_lines_map.insert(
            format!("{g}_{e}"),
            Value::Array(lines.iter().map(|l| Value::String(l.clone())).collect()),
        );
    }

    let mut speech_note_written = false;

    for (g_idx, group) in parsed.groups.iter().enumerate() {
        let mut group_content = String::new();

        for (e_idx, entry) in group.entries.iter().enumerate() {
            let key = format!("{g_idx}_{e_idx}");
            match &entry.kind {
                EntryKind::Speech { header, .. } => {
                    let lines_vec: Vec<&str> = speech_lines_map
                        .get(&key)
                        .and_then(Value::as_array)
                        .map(|arr| arr.iter().filter_map(Value::as_str).collect())
                        .unwrap_or_default();

                    if !lines_vec.is_empty() {
                        if !speech_note_written && speech.omitted > 0 {
                            group_content.push_str(&format!(
                                "Speech lines left out: {}\n\n",
                                speech.omitted
                            ));
                            speech_note_written = true;
                        }
                        if let Some(h) = header {
                            group_content.push_str(&format!(
                                "{}\n{h}\n{}\n\n",
                                entry.heading,
                                lines_vec.join("\n")
                            ));
                        } else {
                            group_content.push_str(&format!(
                                "{}\n{}\n\n",
                                entry.heading,
                                lines_vec.join("\n")
                            ));
                        }
                    } else if !speech_note_written && speech.omitted > 0 && speech.kept == 0 {
                        group_content
                            .push_str(&format!("Speech lines left out: {}\n\n", speech.omitted));
                        speech_note_written = true;
                    }
                }
                EntryKind::Sense { body } => {
                    if sense.kept_group_indices.contains(&g_idx) {
                        group_content.push_str(&format!("### sense summary\n{body}\n\n"));
                    } else {
                        group_content.push_str(&format!(
                            "Sense left out {} - {}\n\n",
                            group.start_time, group.end_time
                        ));
                    }
                }
                EntryKind::Pool(_) => {
                    if let Some(Value::String(rendered)) = pool.entry_renderings.get(&key) {
                        group_content.push_str(rendered);
                    }
                }
            }
        }

        for run in &pool.dropped_runs {
            if run.group_idx == g_idx {
                group_content.push_str(&format!(
                    "{} entries left out {} - {} ({} {})\n\n",
                    run.count, run.start_t, run.end_t, run.size, run.unit
                ));
            }
        }

        if !group_content.is_empty() {
            out.push_str(&format!("{}\n\n", group.heading_line));
            out.push_str(&group_content);
        }
    }

    if has_incomplete_trailer {
        out.push_str(&format!("\n{INCOMPLETE_MEMORY_NOTICE}"));
    }

    out
}

#[cfg(test)]
mod tests;
