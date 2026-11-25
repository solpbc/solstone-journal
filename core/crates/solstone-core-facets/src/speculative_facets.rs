// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Side-effect-free aggregation of recurring speculative facet proposals.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::Path;

use jiff::civil::Date;
use serde::Serialize;
use serde_json::Value;
use solstone_core_journal_io::{PathError, PathOrDay, day_dirs, iter_segments};

/// Number of local calendar days scanned for recurring facet proposals.
pub const FACET_CANDIDATE_WINDOW_DAYS: i64 = 14;
/// Minimum segment proposals required to surface a candidate.
pub const FACET_CANDIDATE_MIN_SEGMENTS: usize = 3;
/// Minimum segment proposals required across a historical backfill span.
pub const BACKFILL_FACET_CANDIDATE_MIN_SEGMENTS: usize = 5;
/// Maximum historical-span-only facet proposals surfaced per pass.
pub const BACKFILL_FACET_CANDIDATE_CAP: usize = 5;
const SAMPLE_LIMIT: usize = 3;

/// One segment that proposed a speculative facet.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SpeculativeFacetSample {
    pub day: String,
    pub stream: String,
    pub segment: String,
    /// Present when stream or basename cannot be named in UTF-8.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub unrepresentable: bool,
}

/// A recurring speculative facet proposal and its bounded evidence samples.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SpeculativeFacetCandidate {
    pub name: String,
    pub name_key: String,
    pub count: usize,
    pub window_days: i64,
    pub samples: Vec<SpeculativeFacetSample>,
}

/// Aggregate recurring speculative facet proposals without mutating journal state.
///
/// This mirrors Python's `aggregate_speculative_facets`. `today` is the sole
/// clock seam: it determines the rolling local-day cutoff, rather than allowing
/// callers to bypass that boundary with an explicit list of days.
///
/// Case-folds via `caseless::default_case_fold_str` without NFKC normalization,
/// matching Python's `str.casefold()` exactly. Do not reuse
/// `solstone_core_entity_matching`'s normalizers here: they NFKC-normalize
/// first, which `.casefold()` does not, and would regroup
/// compatibility-equivalent names Python keeps separate.
pub fn aggregate_speculative_facets(
    journal_root: &Path,
    today: Date,
    min_count: usize,
) -> Result<Vec<SpeculativeFacetCandidate>, PathError> {
    let cutoff = solstone_core_time::day_key(
        today
            .checked_sub(jiff::Span::new().days(FACET_CANDIDATE_WINDOW_DAYS))
            .expect("candidate window start is a real date"),
    );
    let mut scan_days: Vec<_> = day_dirs(journal_root)?
        .into_keys()
        .filter(|day| day >= &cutoff)
        .collect();
    scan_days.sort();

    let mut groups = BTreeMap::<String, SpeculativeFacetCandidate>::new();
    for day in scan_days {
        for segment in iter_segments(journal_root, PathOrDay::Day(&day))? {
            let sense_path = segment.path().join("talents").join("sense.json");
            let Ok(bytes) = fs::read(sense_path) else {
                continue;
            };
            let Ok(Value::Object(data)) = serde_json::from_slice::<Value>(&bytes) else {
                continue;
            };
            let Some(raw_name) = data.get("speculative_facet").and_then(Value::as_str) else {
                continue;
            };
            let display = collapse_python_whitespace(raw_name);
            if display.is_empty() {
                continue;
            }
            let name_key = caseless::default_case_fold_str(&display);
            let group =
                groups
                    .entry(name_key.clone())
                    .or_insert_with(|| SpeculativeFacetCandidate {
                        name: display,
                        name_key,
                        count: 0,
                        window_days: FACET_CANDIDATE_WINDOW_DAYS,
                        samples: Vec::new(),
                    });
            group.count += 1;
            match segment.record_identity() {
                Ok(identity) => {
                    let representable = group
                        .samples
                        .iter()
                        .filter(|sample| !sample.unrepresentable)
                        .count();
                    if representable < SAMPLE_LIMIT {
                        group.samples.push(SpeculativeFacetSample {
                            day: day.clone(),
                            stream: identity.stream.to_owned(),
                            segment: identity.key.to_owned(),
                            unrepresentable: false,
                        });
                    }
                }
                Err(_) => {
                    if !group.samples.iter().any(|sample| sample.unrepresentable) {
                        group.samples.push(SpeculativeFacetSample {
                            day: day.clone(),
                            stream: String::new(),
                            segment: segment.key().to_owned(),
                            unrepresentable: true,
                        });
                    }
                }
            }
        }
    }

    let mut candidates: Vec<_> = groups
        .into_values()
        .filter(|candidate| candidate.count >= min_count)
        .collect();
    candidates.sort_by(|left, right| {
        right
            .count
            .cmp(&left.count)
            .then_with(|| left.name_key.cmp(&right.name_key))
    });
    Ok(candidates)
}

/// Aggregate recurring speculative facet proposals across both the recent 14-day
/// live window and any historical backfill span outside that window.
///
/// Historical proposals require multi-episode recurrence (`>= 5` segments across
/// `>= 3` ISO weeks on `>= 21d` spans, or `>= 3` distinct days on shorter spans),
/// filter already-declared facets and accepted/dismissed review candidates, and
/// cap historical-only additions to [`BACKFILL_FACET_CANDIDATE_CAP`].
pub fn aggregate_speculative_facets_with_backfill(
    journal_root: &Path,
    today: Date,
    min_count: usize,
) -> Result<Vec<SpeculativeFacetCandidate>, PathError> {
    let mut recent = aggregate_speculative_facets(journal_root, today, min_count)?;
    let cutoff = solstone_core_time::day_key(
        today
            .checked_sub(jiff::Span::new().days(FACET_CANDIDATE_WINDOW_DAYS))
            .expect("candidate window start is a real date"),
    );
    let mut all_days: Vec<_> = day_dirs(journal_root)?.into_keys().collect();
    all_days.sort();
    if !all_days.iter().any(|day| day < &cutoff) {
        return Ok(recent);
    }

    let mut excluded_keys = BTreeSet::<String>::new();
    for c in &recent {
        excluded_keys.insert(c.name_key.clone());
    }
    if let Ok(declared) = crate::list_declared_facet_names(journal_root) {
        for slug in declared {
            excluded_keys.insert(caseless::default_case_fold_str(&slug));
            excluded_keys.insert(caseless::default_case_fold_str(&crate::humanize_facet_title(
                &slug,
            )));
            if let Ok(Some(decl)) = crate::observe_facet_declaration(journal_root, &slug) {
                excluded_keys.insert(caseless::default_case_fold_str(&decl.title));
            }
        }
    }
    if let Ok(existing) = crate::load_candidates(journal_root) {
        for row in existing {
            if matches!(
                row.get("status").and_then(Value::as_str),
                Some("accepted" | "dismissed")
            ) && let Some(key) = row.get("name_key").and_then(Value::as_str)
            {
                excluded_keys.insert(key.to_owned());
            }
        }
    }

    struct SpanGroup {
        candidate: SpeculativeFacetCandidate,
        days: BTreeSet<String>,
        iso_weeks: BTreeSet<(i16, i8)>,
    }

    let mut groups = BTreeMap::<String, SpanGroup>::new();
    let mut min_hist_date: Option<Date> = None;
    let mut max_hist_date: Option<Date> = None;

    for day in &all_days {
        let parsed_date = solstone_core_time::parse_day(day);
        if day < &cutoff
            && let Some(d) = parsed_date
        {
            min_hist_date = Some(min_hist_date.map_or(d, |cur| cur.min(d)));
            max_hist_date = Some(max_hist_date.map_or(d, |cur| cur.max(d)));
        }
        for segment in iter_segments(journal_root, PathOrDay::Day(day))? {
            let sense_path = segment.path().join("talents").join("sense.json");
            let Ok(bytes) = fs::read(sense_path) else {
                continue;
            };
            let Ok(Value::Object(data)) = serde_json::from_slice::<Value>(&bytes) else {
                continue;
            };
            let Some(raw_name) = data.get("speculative_facet").and_then(Value::as_str) else {
                continue;
            };
            let display = collapse_python_whitespace(raw_name);
            if display.is_empty() {
                continue;
            }
            let name_key = caseless::default_case_fold_str(&display);
            if excluded_keys.contains(&name_key) {
                continue;
            }
            let group = groups.entry(name_key.clone()).or_insert_with(|| SpanGroup {
                candidate: SpeculativeFacetCandidate {
                    name: display,
                    name_key,
                    count: 0,
                    window_days: FACET_CANDIDATE_WINDOW_DAYS,
                    samples: Vec::new(),
                },
                days: BTreeSet::new(),
                iso_weeks: BTreeSet::new(),
            });
            group.candidate.count += 1;
            group.days.insert(day.clone());
            if let Some(d) = parsed_date {
                let iso = d.iso_week_date();
                group.iso_weeks.insert((iso.year(), iso.week()));
            }
            if let Ok(identity) = segment.record_identity() {
                let representable = group
                    .candidate
                    .samples
                    .iter()
                    .filter(|s| !s.unrepresentable)
                    .count();
                if representable < SAMPLE_LIMIT {
                    group.candidate.samples.push(SpeculativeFacetSample {
                        day: day.clone(),
                        stream: identity.stream.to_owned(),
                        segment: identity.key.to_owned(),
                        unrepresentable: false,
                    });
                }
            }
        }
    }

    let span_days = min_hist_date
        .zip(max_hist_date)
        .map_or(0, |(lo, hi)| hi.duration_since(lo).as_hours() / 24 + 1);
    let min_segments = min_count.max(BACKFILL_FACET_CANDIDATE_MIN_SEGMENTS);

    let mut backfill_additions: Vec<(usize, SpeculativeFacetCandidate)> = groups
        .into_values()
        .filter_map(|mut g| {
            if g.candidate.count < min_segments {
                return None;
            }
            let spread_ok = if span_days >= 21 {
                g.iso_weeks.len() >= 3
            } else {
                g.days.len() >= 3
            };
            if !spread_ok {
                return None;
            }
            if let (Some(first), Some(last)) = (
                g.days.first().and_then(|s| solstone_core_time::parse_day(s)),
                g.days.last().and_then(|s| solstone_core_time::parse_day(s)),
            ) {
                g.candidate.window_days =
                    (today.duration_since(first).as_hours() / 24).max(
                        last.duration_since(first).as_hours() / 24 + 1,
                    );
            }
            Some((g.iso_weeks.len(), g.candidate))
        })
        .collect();

    backfill_additions.sort_by(|(lw, lc), (rw, rc)| {
        rc.count
            .cmp(&lc.count)
            .then_with(|| rw.cmp(lw))
            .then_with(|| lc.name_key.cmp(&rc.name_key))
    });
    backfill_additions.truncate(BACKFILL_FACET_CANDIDATE_CAP);
    recent.extend(backfill_additions.into_iter().map(|(_, c)| c));
    Ok(recent)
}

fn collapse_python_whitespace(value: &str) -> String {
    let mut output = String::with_capacity(value.len());
    let mut pending_space = false;
    for character in value.chars() {
        if is_python_whitespace(character) {
            pending_space = !output.is_empty();
        } else {
            if pending_space {
                output.push(' ');
                pending_space = false;
            }
            output.push(character);
        }
    }
    output
}

fn is_python_whitespace(character: char) -> bool {
    matches!(
        character,
        '\u{09}'..='\u{0D}'
            | '\u{1C}'..='\u{20}'
            | '\u{85}'
            | '\u{A0}'
            | '\u{1680}'
            | '\u{2000}'..='\u{200A}'
            | '\u{2028}'..='\u{2029}'
            | '\u{202F}'
            | '\u{205F}'
            | '\u{3000}'
    )
}
