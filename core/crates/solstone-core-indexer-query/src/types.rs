// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::collections::BTreeMap;
use std::path::PathBuf;

use serde::{Deserialize, Deserializer, Serialize};

/// Requested and reported ordering for journal search results.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Order {
    #[default]
    Relevance,
    Recency,
}

impl<'de> Deserialize<'de> for Order {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        match value.as_str() {
            "relevance" => Ok(Self::Relevance),
            "recency" => Ok(Self::Recency),
            _ => Err(serde::de::Error::unknown_variant(
                &value,
                &["relevance", "recency"],
            )),
        }
    }
}

/// Filters and options for one read-only journal search.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct SearchRequest {
    pub query: String,
    pub limit: usize,
    pub offset: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub day: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub day_from: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub day_to: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub facet: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub agent: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stream: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub time_bucket: Option<String>,
    #[serde(default)]
    pub relax: bool,
    #[serde(default)]
    pub counts: bool,
    #[serde(default)]
    pub order: Order,
}

impl Default for SearchRequest {
    fn default() -> Self {
        Self {
            query: String::new(),
            limit: 10,
            offset: 0,
            day: None,
            day_from: None,
            day_to: None,
            facet: None,
            agent: None,
            stream: None,
            time_bucket: None,
            relax: false,
            counts: false,
            order: Order::Relevance,
        }
    }
}

impl SearchRequest {
    pub fn new(query: impl Into<String>, order: Order) -> Self {
        Self {
            query: query.into(),
            order,
            ..Self::default()
        }
    }
}

/// One FTS row, shaped like the Python journal search result.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct SearchHit {
    pub id: String,
    pub text: String,
    pub metadata: SearchMetadata,
    pub score: f64,
}

/// Metadata attached to a journal search result.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct SearchMetadata {
    pub day: String,
    pub facet: String,
    pub agent: String,
    pub stream: String,
    pub path: String,
    pub idx: i64,
}

/// Search rows and optional requested counts.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct SearchResponse {
    pub results: Vec<SearchHit>,
    pub order: Order,
    pub relaxed: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub total: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub counts: Option<CountsResponse>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    pub cleaned_query: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub degraded: Option<IndexDegraded>,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum IndexDegraded {
    Building {
        state_schema_version: i64,
        recorded_counts: IndexBuildCounts,
        observed_counts: IndexBuildCounts,
    },
    Unknown,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct IndexBuildCounts {
    pub files: u64,
    pub chunks: u64,
}

/// Requested aggregation matching Python's ``search_counts`` fields.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct CountsResponse {
    pub total: u64,
    pub facets: BTreeMap<String, u64>,
    pub agents: BTreeMap<String, u64>,
    pub days: BTreeMap<String, u64>,
    pub streams: BTreeMap<String, u64>,
    pub relaxed: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub degraded: Option<IndexDegraded>,
}

/// The dated portion of a nonempty index.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CoverageState {
    Available,
    NoDatedChunks,
}

/// Corpus coverage, keeping an undated corpus distinct from an unavailable index.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct CoverageResponse {
    pub state: CoverageState,
    pub start: Option<String>,
    pub end: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub degraded: Option<IndexDegraded>,
}

/// Why the read-only executor could not use an index.
#[derive(Debug)]
pub enum IndexAccessError {
    Absent { path: PathBuf },
    Unreadable { path: PathBuf, detail: String },
    Locked { path: PathBuf, detail: String },
    Empty { path: PathBuf },
}

impl IndexAccessError {
    /// Stable machine-readable error reason for the CLI JSON envelope.
    pub fn reason(&self) -> &'static str {
        match self {
            Self::Absent { .. } => "index_absent",
            Self::Unreadable { .. } => "index_unreadable",
            Self::Locked { .. } => "index_locked",
            Self::Empty { .. } => "empty_index",
        }
    }
}

impl std::fmt::Display for IndexAccessError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Absent { path } => {
                write!(formatter, "journal index is absent: {}", path.display())
            }
            Self::Unreadable { path, detail } => {
                write!(
                    formatter,
                    "journal index is unreadable ({}): {detail}",
                    path.display()
                )
            }
            Self::Locked { path, detail } => {
                write!(
                    formatter,
                    "journal index is locked ({}): {detail}",
                    path.display()
                )
            }
            Self::Empty { path } => write!(formatter, "journal index is empty: {}", path.display()),
        }
    }
}

impl std::error::Error for IndexAccessError {}
