// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Words the journal adds to a body day. Text slots refer to the source Markdown
//! when stored; labels, recorder names and private words stay in that document.

use crate::calendar::{MONTHS, WEEKDAYS};
use jiff::civil::Date;
use serde::{Deserialize, Serialize};
use solstone_core_window_catalog::{Value, render, strings, text};

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TallyKind {
    Stage,
    Reading,
    Count,
    Logged,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Copy<T = String> {
    Heading {
        date: Date,
    },
    RawDay {
        day: T,
    },
    Source {
        source: T,
    },
    Experience {
        continuing: bool,
        experience: Option<T>,
        name: T,
        start: T,
        end: T,
        recorder: Option<T>,
        words: Option<T>,
    },
    Dated {
        name: T,
        value: T,
    },
    Documents {
        count: usize,
        source: T,
        leading: Option<T>,
        last: T,
    },
    Tally {
        section: TallyKind,
        count: u64,
        name: T,
        start: Option<T>,
        end: Option<T>,
        recorder: Option<T>,
        entered: bool,
    },
    Words {
        owner: bool,
        words: T,
    },
}

impl<T> Copy<T> {
    pub(super) fn map_text<U>(&self, mut f: impl FnMut(&T) -> Option<U>) -> Option<Copy<U>> {
        fn slot<T, U>(value: &Option<T>, f: &mut impl FnMut(&T) -> Option<U>) -> Option<Option<U>> {
            match value {
                Some(value) => Some(Some(f(value)?)),
                None => Some(None),
            }
        }
        // Stored metadata is optional. Only shapes the writer can produce may render.
        match self {
            Self::Tally {
                section,
                count,
                start,
                end,
                entered,
                ..
            } if *count == 0
                || i32::try_from(*count).is_err()
                || (end.is_some() && (start.is_none() || *count == 1))
                || (*entered && *section != TallyKind::Logged) =>
            {
                return None;
            }
            Self::Documents { count, leading, .. }
                if *count == 0
                    || i32::try_from(*count).is_err()
                    || (*count == 1 && leading.is_some()) =>
            {
                return None;
            }
            Self::Experience {
                continuing: true,
                words: Some(_),
                ..
            } => return None,
            _ => {}
        }
        Some(match self {
            Self::Heading { date } => Copy::Heading { date: *date },
            Self::RawDay { day } => Copy::RawDay { day: f(day)? },
            Self::Source { source } => Copy::Source { source: f(source)? },
            Self::Dated { name, value } => Copy::Dated {
                name: f(name)?,
                value: f(value)?,
            },
            Self::Words { owner, words } => Copy::Words {
                owner: *owner,
                words: f(words)?,
            },
            Self::Documents {
                count,
                source,
                leading,
                last,
            } => Copy::Documents {
                count: *count,
                source: f(source)?,
                leading: slot(leading, &mut f)?,
                last: f(last)?,
            },
            Self::Experience {
                continuing,
                experience,
                name,
                start,
                end,
                recorder,
                words,
            } => Copy::Experience {
                continuing: *continuing,
                experience: slot(experience, &mut f)?,
                name: f(name)?,
                start: f(start)?,
                end: f(end)?,
                recorder: slot(recorder, &mut f)?,
                words: slot(words, &mut f)?,
            },
            Self::Tally {
                section,
                count,
                name,
                start,
                end,
                recorder,
                entered,
            } => Copy::Tally {
                section: *section,
                count: *count,
                name: f(name)?,
                start: slot(start, &mut f)?,
                end: slot(end, &mut f)?,
                recorder: slot(recorder, &mut f)?,
                entered: *entered,
            },
        })
    }
}

fn experience_id(continuing: bool, named: bool, recorder: bool, words: bool) -> &'static str {
    match (continuing, named, recorder, words) {
        (false, false, false, false) => "journal.window.body.document.experience.generic",
        (false, false, false, true) => "journal.window.body.document.experience.generic.words",
        (false, false, true, false) => "journal.window.body.document.experience.generic.recorder",
        (false, false, true, true) => {
            "journal.window.body.document.experience.generic.recorder.words"
        }
        (false, true, false, false) => "journal.window.body.document.experience.named",
        (false, true, false, true) => "journal.window.body.document.experience.named.words",
        (false, true, true, false) => "journal.window.body.document.experience.named.recorder",
        (false, true, true, true) => "journal.window.body.document.experience.named.recorder.words",
        (true, false, false, _) => "journal.window.body.document.experience.continuing.generic",
        (true, false, true, _) => {
            "journal.window.body.document.experience.continuing.generic.recorder"
        }
        (true, true, false, _) => "journal.window.body.document.experience.continuing.named",
        (true, true, true, _) => {
            "journal.window.body.document.experience.continuing.named.recorder"
        }
    }
}

// Each group provides the complete line with no time, one time or a range.
fn tally_ids(section: TallyKind, recorder: bool, entered: bool) -> [&'static str; 3] {
    match (section, recorder, entered) {
        (TallyKind::Stage, false, _) => [
            "journal.window.body.document.tally.stage.plain.count",
            "journal.window.body.document.tally.stage.plain.at",
            "journal.window.body.document.tally.stage.plain.range",
        ],
        (TallyKind::Stage, true, _) => [
            "journal.window.body.document.tally.stage.recorder.count",
            "journal.window.body.document.tally.stage.recorder.at",
            "journal.window.body.document.tally.stage.recorder.range",
        ],
        (TallyKind::Reading, false, _) => [
            "journal.window.body.document.tally.reading.plain.count",
            "journal.window.body.document.tally.reading.plain.at",
            "journal.window.body.document.tally.reading.plain.range",
        ],
        (TallyKind::Reading, true, _) => [
            "journal.window.body.document.tally.reading.recorder.count",
            "journal.window.body.document.tally.reading.recorder.at",
            "journal.window.body.document.tally.reading.recorder.range",
        ],
        (TallyKind::Count, false, _) => [
            "journal.window.body.document.tally.count.plain.count",
            "journal.window.body.document.tally.count.plain.at",
            "journal.window.body.document.tally.count.plain.range",
        ],
        (TallyKind::Count, true, _) => [
            "journal.window.body.document.tally.count.recorder.count",
            "journal.window.body.document.tally.count.recorder.at",
            "journal.window.body.document.tally.count.recorder.range",
        ],
        (TallyKind::Logged, false, false) => [
            "journal.window.body.document.tally.logged.plain.count",
            "journal.window.body.document.tally.logged.plain.at",
            "journal.window.body.document.tally.logged.plain.range",
        ],
        (TallyKind::Logged, false, true) => [
            "journal.window.body.document.tally.logged.owner.count",
            "journal.window.body.document.tally.logged.owner.at",
            "journal.window.body.document.tally.logged.owner.range",
        ],
        (TallyKind::Logged, true, false) => [
            "journal.window.body.document.tally.logged.recorder.count",
            "journal.window.body.document.tally.logged.recorder.at",
            "journal.window.body.document.tally.logged.recorder.range",
        ],
        (TallyKind::Logged, true, true) => [
            "journal.window.body.document.tally.logged.owner.recorder.count",
            "journal.window.body.document.tally.logged.owner.recorder.at",
            "journal.window.body.document.tally.logged.owner.recorder.range",
        ],
    }
}

impl Copy {
    pub fn render(&self, locale: &str) -> String {
        match self {
            Self::Heading { date } => strings(
                locale,
                "journal.window.body.document.heading",
                [
                    (
                        "weekday",
                        &text(
                            locale,
                            WEEKDAYS[date.weekday().to_monday_zero_offset() as usize],
                        ),
                    ),
                    ("month", &text(locale, MONTHS[(date.month() - 1) as usize])),
                    ("day", &date.day().to_string()),
                    ("year", &date.strftime("%Y").to_string()),
                ],
            ),
            Self::RawDay { day } => strings(
                locale,
                "journal.window.body.document.heading.raw",
                [("day", day)],
            ),
            Self::Source { source } => strings(
                locale,
                "journal.window.shared.date.format.imported.source.label",
                [("source_name", source)],
            ),
            Self::Dated { name, value } => strings(
                locale,
                "journal.window.body.document.dated",
                [("name", name), ("value", value)],
            ),
            Self::Words { owner, words } => strings(
                locale,
                if *owner {
                    "journal.window.body.document.words.owner"
                } else {
                    "journal.window.body.document.words.source"
                },
                [("words", words)],
            ),
            Self::Documents {
                count,
                source,
                leading,
                last,
            } => {
                let mut args = vec![
                    ("quantity", Value::Cardinal(*count as i64)),
                    ("source", Value::String(source)),
                    ("last", Value::String(last)),
                ];
                let id = if let Some(leading) = leading {
                    args.push(("leading", Value::String(leading)));
                    "journal.window.body.document.dated.list"
                } else {
                    "journal.window.body.document.dated.one"
                };
                render(locale, id, &args)
            }
            Self::Experience {
                continuing,
                experience,
                name,
                start,
                end,
                recorder,
                words,
            } => {
                let id = experience_id(
                    *continuing,
                    experience.is_some(),
                    recorder.is_some(),
                    words.is_some(),
                );
                let mut args = vec![
                    ("name", Value::String(name)),
                    ("start", Value::String(start)),
                    ("end", Value::String(end)),
                ];
                if let Some(kind) = experience {
                    args.push(("kind", Value::String(kind)));
                }
                if let Some(recorder) = recorder {
                    args.push(("recorder", Value::String(recorder)));
                }
                if !continuing && let Some(words) = words {
                    args.push(("words", Value::String(words)));
                }
                render(locale, id, &args)
            }
            Self::Tally {
                section,
                count,
                name,
                start,
                end,
                recorder,
                entered,
            } => {
                let ids = tally_ids(*section, recorder.is_some(), *entered);
                let count_text = count.to_string();
                let mut args = vec![
                    ("quantity", Value::Cardinal(*count as i64)),
                    ("count", Value::String(&count_text)),
                    ("name", Value::String(name)),
                ];
                if let Some(recorder) = recorder {
                    args.push(("recorder", Value::String(recorder)));
                }
                let time = if let Some(start) = start {
                    args.push(("start", Value::String(start)));
                    if let Some(end) = end {
                        args.push(("end", Value::String(end)));
                        2
                    } else {
                        1
                    }
                } else {
                    0
                };
                render(locale, ids[time], &args)
            }
        }
    }
}
