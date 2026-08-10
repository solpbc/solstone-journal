// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::sync::OnceLock;

use chrono::{Duration, NaiveDate};
use regex::{Captures, Regex};

/// The read-scope fields relevant to native day-root resolution.
pub struct ReadScopeConfig<'a> {
    pub read_scope: Option<&'a [String]>,
    pub read_scope_span: Option<i64>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ReadScopeError {
    InvalidDay(String),
}

/// Resolve configured scope roots or an inclusive chronicle day range.
pub fn resolve_read_scope(
    config: ReadScopeConfig<'_>,
    day: &str,
    span: i64,
) -> Result<Vec<String>, ReadScopeError> {
    let base_day = parse_day(day)?;

    if let Some(scope) = config.read_scope.filter(|scope| !scope.is_empty()) {
        return scope
            .iter()
            .map(|value| expand_day_placeholders_from(value, base_day))
            .collect();
    }

    let effective_span = config.read_scope_span.unwrap_or(span);
    if effective_span <= 0 {
        return Ok(vec![format!("chronicle/{day}")]);
    }
    Ok((0..=effective_span)
        .rev()
        .map(|offset| {
            format!(
                "chronicle/{}",
                (base_day - Duration::days(offset)).format("%Y%m%d")
            )
        })
        .collect())
}

fn expand_day_placeholders_from(
    value: &str,
    base_day: NaiveDate,
) -> Result<String, ReadScopeError> {
    Ok(day_placeholder()
        .replace_all(value, |captures: &Captures<'_>| {
            let offset = captures.name("offset").map_or(0, |value| {
                value.as_str().parse::<i64>().expect("day offset fits i64")
            });
            (base_day - Duration::days(offset))
                .format("%Y%m%d")
                .to_string()
        })
        .into_owned())
}

fn parse_day(day: &str) -> Result<NaiveDate, ReadScopeError> {
    NaiveDate::parse_from_str(day, "%Y%m%d").map_err(|_| ReadScopeError::InvalidDay(day.to_owned()))
}

fn day_placeholder() -> &'static Regex {
    static DAY_PLACEHOLDER: OnceLock<Regex> = OnceLock::new();
    DAY_PLACEHOLDER
        .get_or_init(|| Regex::new(r"<day(?:-(?<offset>\d+))?>").expect("valid day regex"))
}

#[cfg(test)]
mod tests {
    use serde_json::Value;

    use super::*;
    use crate::oracle;

    #[test]
    fn read_scope_vectors_match_the_oracle() {
        let fixture = oracle::fixture();
        assert_eq!(fixture.read_scope.len(), 14);
        for vector in &fixture.read_scope {
            let scope = vector
                .talent_config
                .get("read_scope")
                .and_then(Value::as_array)
                .map(|scope| {
                    scope
                        .iter()
                        .map(|value| value.as_str().expect("oracle scope is string").to_owned())
                        .collect::<Vec<_>>()
                });
            let actual = resolve_read_scope(
                ReadScopeConfig {
                    read_scope: scope.as_deref(),
                    read_scope_span: vector
                        .talent_config
                        .get("read_scope_span")
                        .and_then(Value::as_i64),
                },
                &vector.day,
                vector.span,
            )
            .expect("oracle day parses");
            assert_eq!(actual, vector.expect, "{}", vector.id);
        }
    }
}
