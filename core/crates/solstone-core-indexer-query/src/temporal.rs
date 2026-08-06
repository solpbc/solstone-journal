// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use chrono::{Datelike, Duration, NaiveDate};

/// The temporal portion removed from a raw query before FTS compilation.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TemporalExtraction {
    pub remaining_text: String,
    pub day_from: Option<String>,
    pub day_to: Option<String>,
}

#[derive(Clone, Copy)]
enum Handler {
    Yesterday,
    Today,
    LastWeek,
    ThisWeek,
    LastMonth,
    ThisMonth,
    LastDay(u32),
    Weekend,
}

struct Match {
    start: usize,
    end: usize,
    handler: Handler,
}

/// Extract the first unquoted temporal phrase using a caller-supplied date.
pub fn extract_temporal_references(query: &str, reference_date: NaiveDate) -> TemporalExtraction {
    if query.is_empty() {
        return TemporalExtraction {
            remaining_text: String::new(),
            day_from: None,
            day_to: None,
        };
    }

    let mut segments = split_quoted_segments(query);
    let mut best: Option<(usize, Match)> = None;
    for (index, segment) in segments.iter().enumerate() {
        if segment.quoted {
            continue;
        }
        let Some(candidate) = find_temporal(&segment.text) else {
            continue;
        };
        if best.as_ref().is_none_or(|(best_index, best_match)| {
            index < *best_index || (index == *best_index && candidate.start < best_match.start)
        }) {
            best = Some((index, candidate));
        }
    }

    let Some((index, matched)) = best else {
        return TemporalExtraction {
            remaining_text: query.to_string(),
            day_from: None,
            day_to: None,
        };
    };

    let segment = &mut segments[index].text;
    segment.replace_range(matched.start..matched.end, "");
    let joined: String = segments
        .iter()
        .map(|segment| segment.text.as_str())
        .collect();
    let remaining_text = collapse_whitespace_runs(&joined);
    let (day_from, day_to) = resolve(matched.handler, reference_date);
    TemporalExtraction {
        remaining_text,
        day_from: Some(day_from),
        day_to: Some(day_to),
    }
}

struct Segment {
    text: String,
    quoted: bool,
}

impl Segment {
    fn unquoted(text: &str) -> Self {
        Self {
            text: text.to_string(),
            quoted: false,
        }
    }
}

/// Mirror the reference regex split: only paired quotes protect their span.
fn split_quoted_segments(query: &str) -> Vec<Segment> {
    let mut segments = Vec::new();
    let mut cursor = 0;
    while let Some(open_relative) = query[cursor..].find('"') {
        let open = cursor + open_relative;
        let after_open = open + 1;
        let Some(close_relative) = query[after_open..].find('"') else {
            segments.push(Segment::unquoted(&query[cursor..]));
            return segments;
        };
        let close = after_open + close_relative;
        segments.push(Segment::unquoted(&query[cursor..open]));
        segments.push(Segment {
            text: query[open..=close].to_string(),
            quoted: true,
        });
        cursor = close + 1;
    }
    segments.push(Segment::unquoted(&query[cursor..]));
    segments
}

fn find_temporal(segment: &str) -> Option<Match> {
    let mut best = None;
    for (start, _) in segment.char_indices() {
        if start > 0 && previous_char(segment, start).is_some_and(is_word_char) {
            continue;
        }
        for (words, handler) in patterns() {
            if let Some(end) = match_words(segment, start, words) {
                if best
                    .as_ref()
                    .is_none_or(|best_match: &Match| start < best_match.start)
                {
                    best = Some(Match {
                        start,
                        end,
                        handler,
                    });
                }
                break;
            }
        }
    }
    best
}

fn patterns() -> [(&'static [&'static str], Handler); 15] {
    [
        (&["over", "the", "weekend"], Handler::Weekend),
        (&["on", "the", "weekend"], Handler::Weekend),
        (&["last", "monday"], Handler::LastDay(0)),
        (&["last", "tuesday"], Handler::LastDay(1)),
        (&["last", "wednesday"], Handler::LastDay(2)),
        (&["last", "thursday"], Handler::LastDay(3)),
        (&["last", "friday"], Handler::LastDay(4)),
        (&["last", "saturday"], Handler::LastDay(5)),
        (&["last", "sunday"], Handler::LastDay(6)),
        (&["last", "week"], Handler::LastWeek),
        (&["this", "week"], Handler::ThisWeek),
        (&["last", "month"], Handler::LastMonth),
        (&["this", "month"], Handler::ThisMonth),
        (&["yesterday"], Handler::Yesterday),
        (&["today"], Handler::Today),
    ]
}

fn match_words(segment: &str, start: usize, words: &[&str]) -> Option<usize> {
    let mut cursor = start;
    for (index, word) in words.iter().enumerate() {
        let candidate = segment.get(cursor..)?;
        let prefix = candidate.as_bytes().get(..word.len())?;
        if !prefix.eq_ignore_ascii_case(word.as_bytes()) {
            return None;
        }
        cursor += word.len();
        if index + 1 < words.len() {
            let whitespace_start = cursor;
            while let Some(character) = segment[cursor..].chars().next() {
                if !character.is_whitespace() {
                    break;
                }
                cursor += character.len_utf8();
            }
            if cursor == whitespace_start {
                return None;
            }
        }
    }
    if segment[cursor..].chars().next().is_some_and(is_word_char) {
        None
    } else {
        Some(cursor)
    }
}

fn previous_char(text: &str, index: usize) -> Option<char> {
    text[..index].chars().next_back()
}

fn is_word_char(character: char) -> bool {
    character.is_alphanumeric() || character == '_'
}

fn collapse_whitespace_runs(text: &str) -> String {
    let trimmed = text.trim();
    let mut output = String::new();
    let mut whitespace = String::new();
    for character in trimmed.chars() {
        if character.is_whitespace() {
            whitespace.push(character);
            continue;
        }
        if whitespace.len() > 1 {
            output.push(' ');
        } else {
            output.push_str(&whitespace);
        }
        whitespace.clear();
        output.push(character);
    }
    if whitespace.len() > 1 {
        output.push(' ');
    } else {
        output.push_str(&whitespace);
    }
    output
}

fn resolve(handler: Handler, reference: NaiveDate) -> (String, String) {
    let format = |date: NaiveDate| date.format("%Y%m%d").to_string();
    let range = match handler {
        Handler::Yesterday => {
            let day = reference - Duration::days(1);
            (day, day)
        }
        Handler::Today => (reference, reference),
        Handler::LastWeek => {
            let monday_this =
                reference - Duration::days(reference.weekday().num_days_from_monday().into());
            let monday_last = monday_this - Duration::days(7);
            (monday_last, monday_last + Duration::days(6))
        }
        Handler::ThisWeek => {
            let monday =
                reference - Duration::days(reference.weekday().num_days_from_monday().into());
            (monday, monday + Duration::days(6))
        }
        Handler::LastMonth => {
            let first_this = NaiveDate::from_ymd_opt(reference.year(), reference.month(), 1)
                .expect("reference month has a first day");
            let last_previous = first_this - Duration::days(1);
            (
                NaiveDate::from_ymd_opt(last_previous.year(), last_previous.month(), 1)
                    .expect("previous month has a first day"),
                last_previous,
            )
        }
        Handler::ThisMonth => {
            let first = NaiveDate::from_ymd_opt(reference.year(), reference.month(), 1)
                .expect("reference month has a first day");
            let next_month = if reference.month() == 12 {
                NaiveDate::from_ymd_opt(reference.year() + 1, 1, 1)
            } else {
                NaiveDate::from_ymd_opt(reference.year(), reference.month() + 1, 1)
            }
            .expect("next month has a first day");
            (first, next_month - Duration::days(1))
        }
        Handler::LastDay(target) => {
            let current = reference.weekday().num_days_from_monday();
            let mut days_back = (current + 7 - target) % 7;
            if days_back == 0 {
                days_back = 7;
            }
            let day = reference - Duration::days(days_back.into());
            (day, day)
        }
        Handler::Weekend => {
            let weekday = reference.weekday().num_days_from_monday();
            let saturday = if weekday >= 5 {
                reference - Duration::days((weekday - 5).into())
            } else {
                reference - Duration::days((weekday + 2).into())
            };
            (saturday, saturday + Duration::days(1))
        }
    };
    (format(range.0), format(range.1))
}
