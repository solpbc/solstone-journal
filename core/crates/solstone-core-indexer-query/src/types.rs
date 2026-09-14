// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

use serde::{Deserialize, Deserializer, Serialize};
pub use solstone_core_format::content::AdmittedCategory;

/// Explicit owner token. Owner access is not a widest connection boundary.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct OwnerBoundary;

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum QueryBoundary {
    /// Owner is a prefilter choice at query time, not live re-verification of
    /// journal sources. It is not the widest connection and does not consult
    /// the admitted-family map.
    Owner,
    Connection(ConnectionBoundary),
}

impl QueryBoundary {
    pub fn connection(&self) -> Option<&ConnectionBoundary> {
        match self {
            Self::Owner => None,
            Self::Connection(boundary) => Some(boundary),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ConnectionScope {
    WholeJournal,
    ChosenFacets { ids: BTreeSet<String> },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ConnectionBoundary {
    categories: BTreeSet<AdmittedCategory>,
    scope: ConnectionScope,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ConnectionBoundaryError {
    UnknownCategory { token: String },
}

impl ConnectionBoundary {
    pub fn from_category_tokens<I, S>(
        tokens: I,
        scope: ConnectionScope,
    ) -> Result<Self, ConnectionBoundaryError>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        let mut categories = BTreeSet::new();
        for token in tokens {
            let category = match token.as_ref() {
                "Transcripts" | "transcripts" => AdmittedCategory::Transcripts,
                "Entities" | "entities" => AdmittedCategory::Entities,
                "Facets" | "facets" => AdmittedCategory::Facets,
                value => {
                    return Err(ConnectionBoundaryError::UnknownCategory {
                        token: value.to_string(),
                    });
                }
            };
            categories.insert(category);
        }
        Ok(Self { categories, scope })
    }

    pub fn categories(&self) -> &BTreeSet<AdmittedCategory> {
        &self.categories
    }

    pub fn scope(&self) -> &ConnectionScope {
        &self.scope
    }
}

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

/// Connection search deliberately has no offset and no aggregate-count mode.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ConnectionSearchRequest {
    pub query: String,
    pub limit: usize,
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
    pub order: Order,
}

impl Default for ConnectionSearchRequest {
    fn default() -> Self {
        Self {
            query: String::new(),
            limit: 10,
            day: None,
            day_from: None,
            day_to: None,
            facet: None,
            agent: None,
            stream: None,
            time_bucket: None,
            relax: false,
            order: Order::Relevance,
        }
    }
}

/// One FTS row, shaped like the Python journal search result.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct SearchHit {
    #[serde(skip)]
    pub row_id: i64,
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
pub struct ConnectionSearchHit {
    pub id: String,
    pub text: String,
    pub metadata: SearchMetadata,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ConnectionIndexDegraded {
    Building { state_schema_version: i64 },
    Unknown,
}

/// Connection coverage is intentionally boolean and unquantifiable.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct ConnectionSearchResponse {
    pub results: Vec<ConnectionSearchHit>,
    pub order: Order,
    pub relaxed: bool,
    pub cleaned_query: String,
    pub coverage_complete: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub degraded: Option<ConnectionIndexDegraded>,
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
    ConnectionCorpusRefusal(ConnectionCorpusRefusal),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ConnectionCorpusRefusal {
    Counts,
    Agents,
    CoverageSpan,
    IndexedEntityIds,
    HitAt,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct NeedsOwner;

impl ConnectionCorpusRefusal {
    pub fn needs_owner(self) -> NeedsOwner {
        NeedsOwner
    }
}

impl IndexAccessError {
    /// Stable machine-readable error reason for the CLI JSON envelope.
    pub fn reason(&self) -> &'static str {
        match self {
            Self::Absent { .. } => "index_absent",
            Self::Unreadable { .. } => "index_unreadable",
            Self::Locked { .. } => "index_locked",
            Self::Empty { .. } => "empty_index",
            Self::ConnectionCorpusRefusal(_) => "connection_corpus_refusal",
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
            Self::ConnectionCorpusRefusal(_) => {
                write!(formatter, "connection query needs the owner")
            }
        }
    }
}

impl std::error::Error for IndexAccessError {}
