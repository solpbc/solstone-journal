// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::path::Path;

use chrono::{DateTime, Utc};
use serde::Deserialize;
use serde_json::Value;
use solstone_core_indexer_query::{
    IndexAccessError, Order, SearchRequest, SearchResponse, search as search_index,
};

use super::ToolError;

const MAX_QUERY_BYTES: usize = 4_096;
const MAX_LIMIT: usize = 100;
const MAX_OFFSET: usize = 10_000;

/// A search call that has passed the MCP input bounds before auditing.
pub(crate) struct ValidatedSearch {
    request: SearchRequest,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SearchParams {
    query: String,
    #[serde(default = "default_limit")]
    limit: usize,
    #[serde(default)]
    offset: usize,
    #[serde(default)]
    day: Option<String>,
    #[serde(default)]
    day_from: Option<String>,
    #[serde(default)]
    day_to: Option<String>,
    #[serde(default)]
    facet: Option<String>,
    #[serde(default)]
    agent: Option<String>,
    #[serde(default)]
    stream: Option<String>,
    #[serde(default)]
    time_bucket: Option<String>,
    #[serde(default)]
    relax: bool,
    #[serde(default)]
    counts: bool,
    #[serde(default)]
    order: Option<Order>,
}

const fn default_limit() -> usize {
    10
}

pub(crate) fn validate(params: Option<&Value>) -> Result<ValidatedSearch, ToolError> {
    let params = params.cloned().ok_or(ToolError::InvalidInput)?;
    let params =
        serde_json::from_value::<SearchParams>(params).map_err(|_| ToolError::InvalidInput)?;
    if params.query.is_empty()
        || params.query.len() > MAX_QUERY_BYTES
        || !(1..=MAX_LIMIT).contains(&params.limit)
        || params.offset > MAX_OFFSET
    {
        return Err(ToolError::InvalidInput);
    }
    Ok(ValidatedSearch {
        request: SearchRequest {
            query: params.query,
            limit: params.limit,
            offset: params.offset,
            day: params.day,
            day_from: params.day_from,
            day_to: params.day_to,
            facet: params.facet,
            agent: params.agent,
            stream: params.stream,
            time_bucket: params.time_bucket,
            relax: params.relax,
            counts: params.counts,
            order: params.order.unwrap_or(Order::Relevance),
        },
    })
}

pub(crate) fn execute(
    journal_root: &Path,
    request: &ValidatedSearch,
    now: DateTime<Utc>,
) -> Result<Value, ToolError> {
    let response =
        search_index(journal_root, &request.request, now.date_naive()).map_err(map_index_error)?;
    serialize_response(response)
}

fn serialize_response(response: SearchResponse) -> Result<Value, ToolError> {
    serde_json::to_value(response).map_err(|_| ToolError::Serialization)
}

fn map_index_error(error: IndexAccessError) -> ToolError {
    match error {
        IndexAccessError::Absent { .. } => ToolError::IndexAbsent,
        IndexAccessError::Unreadable { .. } => ToolError::IndexUnreadable,
        IndexAccessError::Locked { .. } => ToolError::IndexLocked,
        IndexAccessError::Empty { .. } => ToolError::EmptyIndex,
    }
}

#[cfg(all(test, not(feature = "full-tests")))]
mod tests {
    use std::collections::BTreeMap;

    use serde_json::json;
    use solstone_core_indexer_query::{
        CountsResponse, IndexBuildCounts, IndexDegraded, Order, SearchHit, SearchMetadata,
        SearchResponse,
    };

    use super::{serialize_response, validate};
    use crate::tools::ToolError;

    #[test]
    fn validation_keeps_only_bounded_declared_search_fields() {
        let defaulted = validate(Some(&json!({"query": "needle"}))).unwrap();
        assert_eq!(defaulted.request.limit, 10);
        assert_eq!(defaulted.request.offset, 0);
        assert!(
            validate(Some(&json!({
                "query": "needle",
                "limit": 100,
                "offset": 10_000,
                "relax": true,
                "counts": true,
                "order": "recency",
            })))
            .is_ok()
        );
        for value in [
            json!({"query": "", "limit": 1, "offset": 0}),
            json!({"query": "x", "limit": 0, "offset": 0}),
            json!({"query": "x", "limit": 101, "offset": 0}),
            json!({"query": "x", "limit": 1, "offset": 10_001}),
            json!({"query": "x", "limit": 1, "offset": 0, "extra": true}),
        ] {
            assert!(matches!(
                validate(Some(&value)),
                Err(ToolError::InvalidInput)
            ));
        }
        let oversized = json!({"query": "x".repeat(4_097), "limit": 1, "offset": 0});
        assert!(matches!(
            validate(Some(&oversized)),
            Err(ToolError::InvalidInput)
        ));
    }

    #[test]
    fn execution_serializes_the_index_response_without_collapsing_its_state() {
        let degraded = IndexDegraded::Building {
            state_schema_version: 1,
            recorded_counts: IndexBuildCounts {
                files: 0,
                chunks: 0,
            },
            observed_counts: IndexBuildCounts {
                files: 1,
                chunks: 1,
            },
        };
        let result = serialize_response(SearchResponse {
            results: vec![SearchHit {
                id: "notes/jose.txt:0".to_owned(),
                text: "José handoff".to_owned(),
                metadata: SearchMetadata {
                    day: "20260831".to_owned(),
                    facet: "work".to_owned(),
                    agent: "operator".to_owned(),
                    stream: "default".to_owned(),
                    path: "notes/jose.txt".to_owned(),
                    idx: 0,
                },
                score: 0.5,
            }],
            order: Order::Recency,
            relaxed: true,
            total: Some(1),
            counts: Some(CountsResponse {
                total: 1,
                facets: BTreeMap::from([("work".to_owned(), 1)]),
                agents: BTreeMap::from([("operator".to_owned(), 1)]),
                days: BTreeMap::from([("20260831".to_owned(), 1)]),
                streams: BTreeMap::from([("default".to_owned(), 1)]),
                relaxed: true,
                degraded: Some(degraded.clone()),
            }),
            reason: None,
            cleaned_query: "qué José".to_owned(),
            degraded: Some(degraded),
        })
        .expect("response serializes");

        assert_eq!(result["relaxed"], true);
        assert_eq!(result["cleaned_query"], "qué José");
        assert_eq!(result["counts"]["total"], 1);
        assert_eq!(result["degraded"]["kind"], "building");
        assert_eq!(result["counts"]["degraded"]["kind"], "building");
        assert_eq!(result["results"][0]["metadata"]["path"], "notes/jose.txt");
    }
}
