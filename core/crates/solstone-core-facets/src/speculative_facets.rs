// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Side-effect-free aggregation of recurring speculative facet proposals.

use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

use chrono::{NaiveDate, TimeDelta};
use serde::Serialize;
use serde_json::Value;
use solstone_core_journal_io::{PathError, PathOrDay, day_dirs, iter_segments};

/// Number of local calendar days scanned for recurring facet proposals.
pub const FACET_CANDIDATE_WINDOW_DAYS: i64 = 14;
/// Minimum segment proposals required to surface a candidate.
pub const FACET_CANDIDATE_MIN_SEGMENTS: usize = 3;
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
    today: NaiveDate,
    min_count: usize,
) -> Result<Vec<SpeculativeFacetCandidate>, PathError> {
    let cutoff = (today - TimeDelta::days(FACET_CANDIDATE_WINDOW_DAYS))
        .format("%Y%m%d")
        .to_string();
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
                Some(identity) => {
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
                None => {
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
