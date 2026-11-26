// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Choice-before-sync evaluation, 3-unit processing estimates (`segments`,
//! `days`, `thoughts`), folder-cap detection, and exact AC 5 accounting.

use std::collections::{BTreeMap, BTreeSet};

use jiff::Timestamp;
use jiff::tz::TimeZone;
use serde::{Deserialize, Serialize};

use crate::model::{MessageType, RangePreset};
use crate::select::{Decision, LeftOutCounts};

/// Gmail IMAP folder-limit round numbers that trigger the quiet advisory.
pub const GMAIL_IMAP_FOLDER_CAPS: &[u64] = &[1_000, 2_000, 5_000, 10_000];

/// Returns `Some(count)` if the scanned mailbox count lands on a known Gmail
/// IMAP folder-limit cap.
#[must_use]
pub fn detect_folder_cap(exists_count: u64) -> Option<u64> {
    GMAIL_IMAP_FOLDER_CAPS
        .contains(&exists_count)
        .then_some(exists_count)
}

/// Owner's confirmed selective-sync choice (`config/email/choice.json`).
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Choice {
    pub range: RangePreset,
    pub types: Vec<MessageType>,
    /// Start of the historical window in Unix ms (fixed when range is confirmed
    /// or widened so "outside your choice" stays stable, or `None` for `all` / `none`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub since_ms: Option<i64>,
    /// Earliest timestamp (`confirmed_at`) from which future newly sent messages
    /// come in automatically when `range == none`.
    pub confirmed_at_ms: i64,
    pub updated_at: String,
}

impl Choice {
    /// Whether `sent_ms` falls within this choice's date range.
    #[must_use]
    pub fn includes_time(&self, sent_ms: Option<i64>) -> bool {
        match self.range {
            RangePreset::All => true,
            RangePreset::None => sent_ms.is_some_and(|ms| ms >= self.confirmed_at_ms),
            RangePreset::Last30d | RangePreset::Last6m | RangePreset::Last1y => match self.since_ms
            {
                Some(cutoff) => sent_ms.is_some_and(|ms| ms >= cutoff),
                None => true,
            },
        }
    }

    #[must_use]
    pub fn includes_decision(&self, d: &Decision) -> bool {
        if d.outcome != "selected" {
            return false;
        }
        let Some(mt) = d.message_type else {
            return false;
        };
        self.types.contains(&mt) && self.includes_time(d.sent_ms)
    }
}

/// Compute the inclusive Unix-ms cutoff for a range preset relative to `now`.
#[must_use]
pub fn range_cutoff_ms(preset: RangePreset, now: Timestamp, zone: &TimeZone) -> Option<i64> {
    let zoned = now.to_zoned_datetime(zone.clone());
    match preset {
        RangePreset::All => None,
        RangePreset::None => Some(now.as_millisecond()),
        RangePreset::Last30d => {
            let days_ms: i64 = 30 * 86_400 * 1_000;
            Some(now.as_millisecond().saturating_sub(days_ms))
        }
        RangePreset::Last6m => {
            let cutoff = zoned
                .date()
                .checked_sub(jiff::Span::new().months(6))
                .ok()
                .and_then(|d| {
                    zone.to_ambiguous_zoned_datetime(d.at(0, 0, 0, 0))
                        .compatible()
                        .ok()
                })
                .map_or_else(
                    || now.as_millisecond().saturating_sub(183 * 86_400 * 1_000),
                    |z| z.timestamp().as_millisecond(),
                );
            Some(cutoff)
        }
        RangePreset::Last1y => {
            let cutoff = zoned
                .date()
                .checked_sub(jiff::Span::new().years(1))
                .ok()
                .and_then(|d| {
                    zone.to_ambiguous_zoned_datetime(d.at(0, 0, 0, 0))
                        .compatible()
                        .ok()
                })
                .map_or_else(
                    || now.as_millisecond().saturating_sub(365 * 86_400 * 1_000),
                    |z| z.timestamp().as_millisecond(),
                );
            Some(cutoff)
        }
    }
}

/// Processing estimate in solstone's three units (never money).
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct ProcessingEstimate {
    pub segments: usize,
    pub days: usize,
    pub thoughts: usize,
}

/// Group timestamps into 5-minute moment windows on the owner's local day
/// (identical to landing) to compute exact `segments`, `days`, and `thoughts`.
#[must_use]
pub fn estimate_for_timestamps(timestamps_ms: &[i64], zone: &TimeZone) -> ProcessingEstimate {
    if timestamps_ms.is_empty() {
        return ProcessingEstimate::default();
    }
    let mut sorted = timestamps_ms.to_vec();
    sorted.sort_unstable();
    let mut days = BTreeSet::new();
    let mut segments = 0usize;
    let mut open_day: Option<String> = None;
    let mut open_start_ms: i64 = 0;

    for ms in sorted {
        let ts = Timestamp::from_millisecond(ms).unwrap_or(Timestamp::UNIX_EPOCH);
        let z = ts.to_zoned_datetime(zone.clone());
        let day = z.strftime("%Y%m%d").to_string();
        days.insert(day.clone());
        if open_day.as_deref() == Some(day.as_str()) && ms.saturating_sub(open_start_ms) < 300_000 {
            continue;
        }
        open_day = Some(day);
        open_start_ms = ms;
        segments += 1;
    }

    // Backfill strategy: ~1.1 thoughts per segment (`sense_static` per segment +
    // Story/entity synthesis), zero when no segments are selected.
    let thoughts = if segments == 0 {
        0
    } else {
        (segments * 11 + 5) / 10
    };
    ProcessingEstimate {
        segments,
        days: days.len(),
        thoughts,
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct RangeCounts {
    #[serde(rename = "30d")]
    pub last_30d: usize,
    #[serde(rename = "6m")]
    pub last_6m: usize,
    #[serde(rename = "1y")]
    pub last_1y: usize,
    pub all: usize,
    pub none: usize,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct TypeCounts {
    pub replies: usize,
    pub new_messages: usize,
    pub forwards: usize,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct ChoicePreview {
    pub range: RangePreset,
    pub types: Vec<MessageType>,
    pub ranges: RangeCounts,
    pub types_in_selected_range: TypeCounts,
    pub selected_messages: usize,
    pub delta_messages: usize,
    pub estimate: ProcessingEstimate,
    pub delta_estimate: ProcessingEstimate,
    pub left_out: LeftOutCounts,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub folder_cap_warning: Option<u64>,
}

/// Compute a live choice preview directly from persisted selection decisions
/// (zero message bodies read).
#[must_use]
pub fn preview_choice(
    decisions: &[Decision],
    landed_keys: &BTreeSet<String>,
    range: RangePreset,
    types: &[MessageType],
    now: Timestamp,
    zone: &TimeZone,
    folder_cap_warning: Option<u64>,
) -> ChoicePreview {
    let mut left_out = LeftOutCounts::default();
    let mut eligible: Vec<&Decision> = Vec::new();
    for d in decisions {
        if d.outcome == "left_out" {
            if let Some(bucket) = d.left_out_bucket {
                left_out.record(bucket);
            }
        } else if d.outcome == "selected" {
            eligible.push(d);
        }
    }

    let cut_30d = range_cutoff_ms(RangePreset::Last30d, now, zone);
    let cut_6m = range_cutoff_ms(RangePreset::Last6m, now, zone);
    let cut_1y = range_cutoff_ms(RangePreset::Last1y, now, zone);

    let in_preset = |d: &Decision, preset: RangePreset| -> bool {
        match preset {
            RangePreset::All => true,
            RangePreset::None => false,
            RangePreset::Last30d => cut_30d.is_none_or(|c| d.sent_ms.is_some_and(|ms| ms >= c)),
            RangePreset::Last6m => cut_6m.is_none_or(|c| d.sent_ms.is_some_and(|ms| ms >= c)),
            RangePreset::Last1y => cut_1y.is_none_or(|c| d.sent_ms.is_some_and(|ms| ms >= c)),
        }
    };

    let type_ticked = |d: &Decision| d.message_type.is_some_and(|t| types.contains(&t));

    let mut ranges = RangeCounts {
        last_30d: 0,
        last_6m: 0,
        last_1y: 0,
        all: 0,
        none: 0,
    };
    let mut types_in_selected_range = TypeCounts {
        replies: 0,
        new_messages: 0,
        forwards: 0,
    };

    let mut selected_ts = Vec::new();
    let mut delta_ts = Vec::new();

    for d in &eligible {
        if in_preset(d, range)
            && let Some(mt) = d.message_type
        {
            match mt {
                MessageType::Replies => types_in_selected_range.replies += 1,
                MessageType::NewMessages => types_in_selected_range.new_messages += 1,
                MessageType::Forwards => types_in_selected_range.forwards += 1,
            }
        }
        if !type_ticked(d) {
            continue;
        }
        if in_preset(d, RangePreset::Last30d) {
            ranges.last_30d += 1;
        }
        if in_preset(d, RangePreset::Last6m) {
            ranges.last_6m += 1;
        }
        if in_preset(d, RangePreset::Last1y) {
            ranges.last_1y += 1;
        }
        ranges.all += 1;

        if in_preset(d, range) {
            let ms = d.sent_ms.unwrap_or(0);
            selected_ts.push(ms);
            if !landed_keys.contains(&d.key) {
                delta_ts.push(ms);
            }
        }
    }

    let estimate = estimate_for_timestamps(&selected_ts, zone);
    let delta_estimate = estimate_for_timestamps(&delta_ts, zone);

    ChoicePreview {
        range,
        types: types.to_vec(),
        ranges,
        types_in_selected_range,
        selected_messages: selected_ts.len(),
        delta_messages: delta_ts.len(),
        estimate,
        delta_estimate,
        left_out,
        folder_cap_warning,
    }
}

/// Exact AC 5 reconciliation block:
/// `sent_total == in_journal + removed_by_owner + outside_choice + left_out.total`
/// (with `pending_in_choice` counted under `outside_choice` until landed? No:
/// during steady state `pending_in_choice == 0`; while fetching, we include
/// `pending_in_choice` explicitly OR count unlanded-in-choice separately so
/// steady state sums identically and fetching still reconciles!).
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct Accounting {
    pub sent_total: usize,
    pub in_journal: usize,
    pub removed_by_owner: usize,
    pub outside_choice: usize,
    #[serde(default, skip_serializing_if = "is_zero")]
    pub pending_in_choice: usize,
    pub left_out: LeftOutCounts,
}

fn is_zero(v: &usize) -> bool {
    *v == 0
}

/// Reconcile every owner-side decision against the live journal and choice.
///
/// Invariant:
/// `sent_total == in_journal + removed_by_owner + pending_in_choice + outside_choice + left_out.total`.
#[must_use]
pub fn compute_accounting(
    decisions: &[Decision],
    landed_live: &BTreeMap<String, bool>,
    choice: Option<&Choice>,
) -> Accounting {
    let mut acc = Accounting::default();
    for d in decisions {
        if d.outcome == "left_out" {
            if let Some(bucket) = d.left_out_bucket {
                acc.left_out.record(bucket);
                acc.sent_total += 1;
            }
            continue;
        }
        if d.outcome != "selected" {
            continue;
        }
        acc.sent_total += 1;
        if let Some(&is_live) = landed_live.get(&d.key) {
            if is_live {
                acc.in_journal += 1;
            } else {
                acc.removed_by_owner += 1;
            }
        } else if choice.is_some_and(|c| c.includes_decision(d)) {
            acc.pending_in_choice += 1;
        } else {
            acc.outside_choice += 1;
        }
    }
    acc
}

/// Earliest month/year label (`"march 2011"`) across eligible sent messages.
#[must_use]
pub fn earliest_sent_month(decisions: &[Decision], zone: &TimeZone) -> Option<String> {
    let min_ms = decisions
        .iter()
        .filter(|d| d.outcome == "selected")
        .filter_map(|d| d.sent_ms)
        .min()?;
    let ts = Timestamp::from_millisecond(min_ms).ok()?;
    let z = ts.to_zoned_datetime(zone.clone());
    let month = match z.month() {
        1 => "january",
        2 => "february",
        3 => "march",
        4 => "april",
        5 => "may",
        6 => "june",
        7 => "july",
        8 => "august",
        9 => "september",
        10 => "october",
        11 => "november",
        12 => "december",
        _ => return None,
    };
    Some(format!("{month} {}", z.year()))
}
