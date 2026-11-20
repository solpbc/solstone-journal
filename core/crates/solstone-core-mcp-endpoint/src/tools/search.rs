// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::collections::BTreeSet;

use serde::Deserialize;
use serde_json::Value;
use solstone_core_indexer_query::AdmittedCategory;

use super::{
    MAX_DAY_BYTES, MAX_FACET_BYTES, MAX_OPAQUE_REFERENCE_BYTES, ToolError, normalize_day,
    optional_string_within_limit,
};

pub(crate) const MAX_QUERY_BYTES: usize = 4_096;
pub(crate) const MAX_LIMIT: usize = 100;

/// Closed, validated agent search arguments.
pub(crate) struct ValidatedSearch {
    pub(crate) query: String,
    pub(crate) limit: usize,
    pub(crate) cursor: Option<String>,
    pub(crate) day: Option<String>,
    pub(crate) day_from: Option<String>,
    pub(crate) day_to: Option<String>,
    pub(crate) category: Option<AdmittedCategory>,
    pub(crate) content_kinds: Option<BTreeSet<AdmittedCategory>>,
    pub(crate) person: Option<String>,
    pub(crate) place: Option<String>,
    pub(crate) activity: Option<String>,
    pub(crate) facet_id: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SearchParams {
    #[serde(default)]
    query: String,
    #[serde(default = "default_limit")]
    limit: usize,
    #[serde(default)]
    cursor: Option<String>,
    #[serde(default)]
    day: Option<String>,
    #[serde(default, alias = "from")]
    day_from: Option<String>,
    #[serde(default, alias = "to")]
    day_to: Option<String>,
    #[serde(default)]
    category: Option<String>,
    #[serde(default)]
    kind: Option<ContentKinds>,
    #[serde(default)]
    person: Option<String>,
    #[serde(default)]
    place: Option<String>,
    #[serde(default)]
    activity: Option<String>,
    #[serde(default)]
    facet: Option<String>,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum ContentKinds {
    One(String),
    Several(Vec<String>),
}

impl ContentKinds {
    fn validate(self) -> Result<BTreeSet<AdmittedCategory>, ToolError> {
        let words = match self {
            Self::One(word) => vec![word],
            Self::Several(words) => words,
        };
        if words.is_empty() || words.len() > solstone_core_format::content::kinds::KINDS.len() {
            return Err(ToolError::InvalidInput);
        }
        words
            .iter()
            .map(|word| {
                AdmittedCategory::from_stored_word(word)
                    .filter(|category| category.kind_spec().option_of().is_none())
                    .ok_or(ToolError::InvalidInput)
            })
            .collect()
    }
}

const fn default_limit() -> usize {
    10
}

pub(crate) fn validate(params: Option<&Value>) -> Result<ValidatedSearch, ToolError> {
    let params = params.cloned().ok_or(ToolError::InvalidInput)?;
    let params =
        serde_json::from_value::<SearchParams>(params).map_err(|_| ToolError::InvalidInput)?;
    // Without keywords, require a date or a find filter.
    let day_bounded = params.day.is_some() || params.day_from.is_some() || params.day_to.is_some();
    if (params.query.trim().is_empty()
        && !day_bounded
        && params.kind.is_none()
        && params.category.is_none()
        && params.person.is_none()
        && params.place.is_none()
        && params.activity.is_none())
        || params.query.len() > MAX_QUERY_BYTES
        || !(1..=MAX_LIMIT).contains(&params.limit)
        || params
            .cursor
            .as_ref()
            .is_some_and(|cursor| cursor.is_empty() || cursor.len() > MAX_OPAQUE_REFERENCE_BYTES)
        || !optional_string_within_limit(&params.day, MAX_DAY_BYTES)
        || !optional_string_within_limit(&params.day_from, MAX_DAY_BYTES)
        || !optional_string_within_limit(&params.day_to, MAX_DAY_BYTES)
        || !optional_string_within_limit(&params.facet, MAX_FACET_BYTES)
        || [&params.person, &params.place, &params.activity]
            .iter()
            .any(|word| {
                !optional_string_within_limit(word, MAX_FACET_BYTES)
                    || word.as_deref().is_some_and(|word| word.trim().is_empty())
            })
        || (params.category.is_some() && params.kind.is_some())
    {
        return Err(ToolError::InvalidInput);
    }
    let category = params
        .category
        .as_deref()
        .map(|word| AdmittedCategory::from_stored_word(word).ok_or(ToolError::InvalidInput))
        .transpose()?;
    let day = normalize_day(params.day)?;
    let day_from = normalize_day(params.day_from)?;
    let day_to = normalize_day(params.day_to)?;
    Ok(ValidatedSearch {
        query: params.query,
        limit: params.limit,
        cursor: params.cursor,
        day,
        day_from,
        day_to,
        category,
        content_kinds: params.kind.map(ContentKinds::validate).transpose()?,
        person: params.person,
        place: params.place,
        activity: params.activity,
        facet_id: params.facet,
    })
}

#[cfg(all(test, not(feature = "full-tests")))]
mod tests {
    use serde_json::json;

    use super::validate;

    #[test]
    fn a_search_without_words_needs_a_day_bound() {
        for listed in [
            json!({"day": "20261005"}),
            json!({"query": "", "day": "2026-10-05"}),
            json!({"query": "  ", "day_from": "20261001", "day_to": "20261005"}),
            json!({"day_from": "20261001"}),
            json!({"day_to": "20261005"}),
        ] {
            let validated = validate(Some(&listed)).expect("a day-bounded listing is accepted");
            assert!(validated.query.trim().is_empty(), "{listed}");
        }
        for refused in [
            json!({}),
            json!({"query": ""}),
            json!({"query": "   "}),
            json!({"limit": 5}),
        ] {
            assert!(validate(Some(&refused)).is_err(), "{refused}");
        }
        assert!(validate(Some(&json!({"query": "launch"}))).is_ok());
    }
}
