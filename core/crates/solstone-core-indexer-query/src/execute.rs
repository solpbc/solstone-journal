// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use chrono::NaiveDate;
use rusqlite::types::Value;
use rusqlite::{
    Connection, Error, ErrorCode, OpenFlags, OptionalExtension, params, params_from_iter,
};
use solstone_core_indexer_store::db::AUTHORED_CHAT_PATH_PREDICATE;

use crate::compile::{CompileOutcome, compile_query};
use crate::ladder::relaxed_plan;
use crate::predicate::{EffectiveDateConstraint, PredicateInput, QueryPredicate};
use crate::temporal::TemporalExtraction;
use crate::types::{
    AdmittedCategory, ConnectionBoundary, ConnectionCorpusRefusal, ConnectionIndexDegraded,
    ConnectionScope, ConnectionSearchHit, ConnectionSearchRequest, ConnectionSearchResponse,
    CountsResponse, CoverageResponse, CoverageState, IndexAccessError, IndexBuildCounts,
    IndexDegraded, Order, OwnerBoundary, QueryBoundary, SearchHit, SearchMetadata, SearchRequest,
    SearchResponse,
};

/// Execute one journal search.
///
/// Reads committed index rows without acquiring a writer lock.
pub fn search(
    journal: &Path,
    _boundary: OwnerBoundary,
    request: &SearchRequest,
    reference_date: NaiveDate,
) -> Result<SearchResponse, IndexAccessError> {
    let compilation = compile_query(&request.query, reference_date);
    if matches!(compilation.outcome, CompileOutcome::NoTokenizableTerm) {
        return Ok(SearchResponse {
            results: Vec::new(),
            order: order_for_plan(false),
            relaxed: false,
            total: None,
            counts: None,
            reason: Some("not_tokenizable".to_string()),
            cleaned_query: compilation.temporal.remaining_text.clone(),
            degraded: None,
        });
    }
    let mut connection = open_index_reader(journal, &QueryBoundary::Owner)?;
    search_on_connection(&mut connection, request, reference_date, compilation)
}

/// Execute journal aggregation independently of a search invocation.
pub fn search_counts(
    journal: &Path,
    _boundary: OwnerBoundary,
    request: &SearchRequest,
    reference_date: NaiveDate,
) -> Result<CountsResponse, IndexAccessError> {
    let compilation = compile_query(&request.query, reference_date);
    if matches!(compilation.outcome, CompileOutcome::NoTokenizableTerm) {
        return Ok(CountsResponse::default());
    }
    let mut connection = open_index_reader(journal, &QueryBoundary::Owner)?;
    let (plan, relaxed) = resolve_plan(&mut connection, request, reference_date, compilation)?;
    let mut counts = connection.aggregate_counts(&plan, relaxed)?;
    counts.degraded = connection.index_degraded()?;
    Ok(counts)
}

/// Connection aggregation is deliberately not a scoped approximation of the
/// owner histogram: it requires the owner corpus boundary.
pub fn search_counts_connection(
    _journal: &Path,
    _boundary: &ConnectionBoundary,
) -> Result<CountsResponse, IndexAccessError> {
    Err(IndexAccessError::ConnectionCorpusRefusal(
        ConnectionCorpusRefusal::Counts,
    ))
}

pub fn search_connection(
    journal: &Path,
    boundary: &ConnectionBoundary,
    request: &ConnectionSearchRequest,
    reference_date: NaiveDate,
) -> Result<ConnectionSearchResponse, IndexAccessError> {
    let compilation = compile_query(&request.query, reference_date);
    let no_tokenizable_term = matches!(compilation.outcome, CompileOutcome::NoTokenizableTerm);
    let boundary = QueryBoundary::Connection(boundary.clone());
    let mut connection = match open_index_reader(journal, &boundary) {
        Ok(connection) => connection,
        Err(IndexAccessError::Absent { .. } | IndexAccessError::Empty { .. }) => {
            return Ok(empty_connection_response(
                compilation.temporal.remaining_text.clone(),
            ));
        }
        Err(error) => return Err(error),
    };
    if no_tokenizable_term {
        return Ok(ConnectionSearchResponse {
            coverage_complete: connection.classification_coverage_complete()?,
            degraded: connection.connection_index_degraded()?,
            ..empty_connection_response(compilation.temporal.remaining_text.clone())
        });
    }
    let owner_request = owner_request_for_connection(request);
    let cleaned_query = compilation.temporal.remaining_text.clone();
    let (mut plan, relaxed) = resolve_plan_with_boundary(
        &mut connection,
        &owner_request,
        reference_date,
        compilation,
        Some(boundary.connection().expect("connection boundary")),
    )?;
    append_connection_start_after(&mut plan, request.start_after.as_ref());
    let order = Order::Recency;
    let results = connection
        .fetch_connection_hits(&plan, request.limit)?
        .into_iter()
        .map(|hit| ConnectionSearchHit {
            row_id: hit.row_id,
            id: hit.id,
            text: hit.text,
            metadata: hit.metadata,
        })
        .collect();
    Ok(ConnectionSearchResponse {
        results,
        order,
        relaxed,
        cleaned_query,
        coverage_complete: connection.classification_coverage_complete()?,
        degraded: connection.connection_index_degraded()?,
    })
}

fn empty_connection_response(cleaned_query: String) -> ConnectionSearchResponse {
    ConnectionSearchResponse {
        results: Vec::new(),
        order: Order::Recency,
        relaxed: false,
        cleaned_query,
        coverage_complete: false,
        degraded: None,
    }
}

fn owner_request_for_connection(request: &ConnectionSearchRequest) -> SearchRequest {
    SearchRequest {
        query: request.query.clone(),
        limit: request.limit,
        offset: 0,
        day: request.day.clone(),
        day_from: request.day_from.clone(),
        day_to: request.day_to.clone(),
        facet: request.facet.clone(),
        agent: request.agent.clone(),
        stream: request.stream.clone(),
        time_bucket: request.time_bucket.clone(),
        relax: request.relax,
        counts: false,
        order: Order::Recency,
    }
}

fn append_connection_start_after(
    plan: &mut SqlPlan,
    start_after: Option<&crate::types::ConnectionStartAfter>,
) {
    let Some(start_after) = start_after else {
        return;
    };
    plan.where_clause.push_str(
        " AND (COALESCE(day, '') < ? OR (COALESCE(day, '') = ? AND (path < ? OR (path = ? AND idx < CAST(? AS INTEGER)))))",
    );
    plan.params.extend([
        start_after.day.clone(),
        start_after.day.clone(),
        start_after.path.clone(),
        start_after.path.clone(),
        start_after.idx.to_string(),
    ]);
}

/// Return whether one exact journal path and chunk index are represented in the index.
pub fn hit_at(
    journal: &Path,
    boundary: QueryBoundary,
    path: &str,
    idx: i64,
) -> Result<bool, IndexAccessError> {
    if matches!(boundary, QueryBoundary::Connection(_)) {
        return Err(IndexAccessError::ConnectionCorpusRefusal(
            ConnectionCorpusRefusal::HitAt,
        ));
    }
    let mut connection = open_index_reader(journal, &boundary)?;
    connection.hit_at(path, idx)
}

/// Return the distinct nonempty indexed agents. Search never calls this query.
pub fn agents(journal: &Path, boundary: QueryBoundary) -> Result<Vec<String>, IndexAccessError> {
    if matches!(boundary, QueryBoundary::Connection(_)) {
        return Err(IndexAccessError::ConnectionCorpusRefusal(
            ConnectionCorpusRefusal::Agents,
        ));
    }
    let mut connection = open_index_reader(journal, &boundary)?;
    connection.agents()
}

/// Return the dated span of a nonempty index.
pub fn coverage(
    journal: &Path,
    boundary: QueryBoundary,
) -> Result<CoverageResponse, IndexAccessError> {
    if matches!(boundary, QueryBoundary::Connection(_)) {
        return Err(IndexAccessError::ConnectionCorpusRefusal(
            ConnectionCorpusRefusal::CoverageSpan,
        ));
    }
    let mut connection = open_index_reader(journal, &boundary)?;
    let mut coverage = connection.coverage()?;
    coverage.degraded = connection.index_degraded()?;
    Ok(coverage)
}

/// Return the canonical entity IDs represented by indexed entity-search rows.
pub fn indexed_entity_ids(
    journal: &Path,
    boundary: QueryBoundary,
) -> Result<BTreeSet<String>, IndexAccessError> {
    if matches!(boundary, QueryBoundary::Connection(_)) {
        return Err(IndexAccessError::ConnectionCorpusRefusal(
            ConnectionCorpusRefusal::IndexedEntityIds,
        ));
    }
    let mut connection = open_index_reader(journal, &boundary)?;
    connection.indexed_entity_ids()
}

fn search_on_connection(
    connection: &mut QueryConnection,
    request: &SearchRequest,
    reference_date: NaiveDate,
    compilation: crate::QueryCompilation,
) -> Result<SearchResponse, IndexAccessError> {
    let cleaned_query = compilation.temporal.remaining_text.clone();
    let (plan, relaxed) = resolve_plan(connection, request, reference_date, compilation)?;
    let order = order_for_plan(plan.has_live_match_expression);
    let results = connection.fetch_hits(&plan, request.limit, request.offset, order)?;
    let counts = request
        .counts
        .then(|| connection.aggregate_counts(&plan, relaxed))
        .transpose()?;
    Ok(SearchResponse {
        results,
        order,
        relaxed,
        total: counts.as_ref().map(|value| value.total),
        counts,
        reason: None,
        cleaned_query,
        degraded: connection.index_degraded()?,
    })
}

fn resolve_plan(
    connection: &mut QueryConnection,
    request: &SearchRequest,
    reference_date: NaiveDate,
    compilation: crate::QueryCompilation,
) -> Result<(SqlPlan, bool), IndexAccessError> {
    resolve_plan_with_boundary(connection, request, reference_date, compilation, None)
}

fn resolve_plan_with_boundary(
    connection: &mut QueryConnection,
    request: &SearchRequest,
    reference_date: NaiveDate,
    compilation: crate::QueryCompilation,
    boundary: Option<&ConnectionBoundary>,
) -> Result<(SqlPlan, bool), IndexAccessError> {
    let mut plan = match boundary {
        Some(boundary) => plan_from_outcome_with_boundary(
            compilation.outcome.clone(),
            &compilation.temporal,
            request,
            Some(boundary),
        ),
        None => plan_from_outcome(compilation.outcome.clone(), &compilation.temporal, request),
    };
    let mut relaxed = false;
    if request.relax
        && !connection.has_rows(&plan)?
        && let Some(candidate) =
            relaxed_plan(connection, &compilation, request, reference_date, boundary)?
    {
        plan = candidate;
        relaxed = true;
    }
    Ok((plan, relaxed))
}

/// Derive ordering only from the final plan's executable MATCH state.
pub(crate) fn order_for_plan(has_live_match_expression: bool) -> Order {
    if has_live_match_expression {
        Order::Relevance
    } else {
        Order::Recency
    }
}

pub(crate) fn plan_from_outcome(
    outcome: CompileOutcome,
    temporal: &TemporalExtraction,
    request: &SearchRequest,
) -> SqlPlan {
    plan_from_outcome_with_boundary(outcome, temporal, request, None)
}

pub(crate) fn plan_from_outcome_with_boundary(
    outcome: CompileOutcome,
    temporal: &TemporalExtraction,
    request: &SearchRequest,
    boundary: Option<&ConnectionBoundary>,
) -> SqlPlan {
    let predicate = QueryPredicate::new(outcome, temporal, predicate_input(request));
    let mut plan = match &predicate.outcome {
        CompileOutcome::Compiled { expression } => SqlPlan {
            where_clause: "chunks MATCH ?".to_string(),
            params: vec![expression.clone()],
            has_live_match_expression: true,
        },
        CompileOutcome::NoInput
        | CompileOutcome::FiltersOnly
        | CompileOutcome::NoTokenizableTerm => SqlPlan {
            where_clause: "1=1".to_string(),
            params: Vec::new(),
            has_live_match_expression: false,
        },
    };
    plan.where_clause = visible_rows(&plan.where_clause);
    if let Some(boundary) = boundary {
        append_connection_prefilter(&mut plan, boundary);
    }
    append_filters(&mut plan, &predicate);
    plan
}

fn predicate_input(request: &SearchRequest) -> PredicateInput {
    PredicateInput {
        day: request.day.clone(),
        day_from: request.day_from.clone(),
        day_to: request.day_to.clone(),
        facet: request.facet.clone(),
        agent: request.agent.clone(),
        stream: request.stream.clone(),
        time_bucket: request.time_bucket.clone(),
    }
}

fn append_filters(plan: &mut SqlPlan, predicate: &QueryPredicate) {
    // `facet` is a lowercased chunks path-shape filter. It intersects the
    // boundary; a facet outside scope is indistinguishable from no such facet.
    match &predicate.effective_date {
        EffectiveDateConstraint::None => {}
        EffectiveDateConstraint::Exact(day) => append_filter(plan, "day=?", day.clone()),
        EffectiveDateConstraint::Range { day_from, day_to } => {
            if let Some(day_from) = day_from {
                append_filter(plan, "day>=?", day_from.clone());
            }
            if let Some(day_to) = day_to {
                append_filter(plan, "day<=?", day_to.clone());
            }
        }
    }
    if let Some(facet) = &predicate.facet {
        append_filter(plan, "facet=?", facet.clone());
    }
    if let Some(agent) = &predicate.agent {
        append_filter(plan, "agent=?", agent.clone());
    }
    if let Some(stream) = &predicate.stream {
        append_filter(plan, "stream=?", stream.clone());
    }
    if let Some(time_bucket) = &predicate.time_bucket {
        append_filter(plan, "time_bucket=?", time_bucket.clone());
    }
}

fn append_connection_prefilter(plan: &mut SqlPlan, boundary: &ConnectionBoundary) {
    if boundary.categories().is_empty()
        || matches!(boundary.scope(), ConnectionScope::ChosenFacets { ids } if ids.is_empty())
    {
        plan.where_clause.push_str(" AND 0");
        return;
    }
    let categories = category_placeholders(boundary.categories().len());
    let mut clause = format!(
        "EXISTS (SELECT 1 FROM chunk_classification cc WHERE cc.path=chunks.path AND cc.eligible=1 AND cc.unclassified=0 AND cc.category IN ({categories})"
    );
    for category in boundary.categories() {
        plan.params.push(category_sql_name(*category).to_string());
    }
    match boundary.scope() {
        ConnectionScope::WholeJournal => {}
        ConnectionScope::ChosenFacets { ids } => {
            let placeholders = category_placeholders(ids.len());
            clause.push_str(" AND ((cc.basis='facet_owned' AND EXISTS (SELECT 1 FROM chunk_classification_facets cf WHERE cf.path=chunks.path AND cf.facet_id IN (");
            clause.push_str(&placeholders);
            clause.push_str("))) OR (cc.basis='segment_assigned' AND EXISTS (SELECT 1 FROM chunk_classification_facets cf WHERE cf.path=chunks.path) AND NOT EXISTS (SELECT 1 FROM chunk_classification_facets cf WHERE cf.path=chunks.path AND cf.facet_id NOT IN (");
            clause.push_str(&placeholders);
            clause.push_str("))))");
            for _ in 0..2 {
                for id in ids {
                    plan.params.push(id.clone());
                }
            }
        }
    }
    clause.push(')');
    plan.where_clause.push_str(" AND ");
    plan.where_clause.push_str(&clause);
}

fn category_placeholders(count: usize) -> String {
    std::iter::repeat_n("?", count)
        .collect::<Vec<_>>()
        .join(", ")
}

fn category_sql_name(category: AdmittedCategory) -> &'static str {
    match category {
        AdmittedCategory::Transcripts => "transcripts",
        AdmittedCategory::Entities => "entities",
        AdmittedCategory::Facets => "facets",
    }
}

fn append_filter(plan: &mut SqlPlan, clause: &str, value: String) {
    plan.where_clause.push_str(" AND ");
    plan.where_clause.push_str(clause);
    plan.params.push(value);
}

pub(crate) struct SqlPlan {
    pub(crate) where_clause: String,
    pub(crate) params: Vec<String>,
    pub(crate) has_live_match_expression: bool,
}

pub(crate) struct QueryConnection {
    connection: Connection,
    path: PathBuf,
    #[cfg(test)]
    aggregate_calls: usize,
    #[cfg(test)]
    agents_calls: usize,
}

fn visible_rows(clause: &str) -> String {
    format!("{clause} AND NOT ({AUTHORED_CHAT_PATH_PREDICATE})")
}

fn open_index_reader(
    journal: &Path,
    boundary: &QueryBoundary,
) -> Result<QueryConnection, IndexAccessError> {
    let path = solstone_core_indexer_store::db::db_path(journal);
    if !path.is_file() {
        return Err(IndexAccessError::Absent { path });
    }
    let connection = Connection::open_with_flags(&path, OpenFlags::SQLITE_OPEN_READ_ONLY)
        .map_err(|error| classify_sql_error(path.clone(), error))?;
    // Match the store's openers: without a busy timeout a momentary indexer write
    // fails every concurrent search instantly with SQLITE_BUSY.
    connection
        .execute_batch("PRAGMA busy_timeout=5000;")
        .map_err(|error| classify_sql_error(path.clone(), error))?;
    let mut connection = QueryConnection::new(connection, path);
    connection.require_nonempty_chunks(boundary)?;
    Ok(connection)
}

/// A bounded snapshot of one entry as stored in the search index.
#[derive(Debug, PartialEq)]
pub enum IndexedEntry {
    Found(String),
    TooLarge,
    NotFound,
}

/// Read a result without modifying the index or opening its source file.
/// The path and chunk index guard against a row id reused by a later index build.
pub fn read_indexed_entry(
    journal: &Path,
    boundary: QueryBoundary,
    path: &str,
    idx: i64,
    row_id: i64,
    max_bytes: u64,
) -> Result<IndexedEntry, IndexAccessError> {
    let reader = match open_index_reader(journal, &boundary) {
        Ok(reader) => reader,
        Err(IndexAccessError::Absent { .. } | IndexAccessError::Empty { .. })
            if matches!(&boundary, QueryBoundary::Connection(_)) =>
        {
            return Ok(IndexedEntry::NotFound);
        }
        Err(error) => return Err(error),
    };
    if matches!(&boundary, QueryBoundary::Owner) {
        let found: Option<(i64, Option<String>)> = reader
            .connection
            .query_row(
                &format!(
                    "SELECT length(CAST(content AS BLOB)),
                    CASE WHEN length(CAST(content AS BLOB)) <= ?4 THEN content ELSE NULL END
             FROM chunks WHERE {} LIMIT 1",
                    visible_rows("rowid=?1 AND path=?2 AND idx=?3")
                ),
                params![
                    row_id,
                    path,
                    idx,
                    i64::try_from(max_bytes).unwrap_or(i64::MAX)
                ],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()
            .map_err(|error| reader.classify(error))?;
        return Ok(match found {
            None => IndexedEntry::NotFound,
            Some((_, Some(content))) => IndexedEntry::Found(content),
            Some(_) => IndexedEntry::TooLarge,
        });
    }
    let mut clause = visible_rows("rowid=? AND path=? AND idx=?");
    let mut classification_values = Vec::new();
    if let QueryBoundary::Connection(connection_boundary) = &boundary {
        let mut plan = SqlPlan {
            where_clause: clause,
            params: Vec::new(),
            has_live_match_expression: false,
        };
        append_connection_prefilter(&mut plan, connection_boundary);
        clause = plan.where_clause;
        classification_values = plan.params;
    }
    let mut values = vec![
        Value::Integer(i64::try_from(max_bytes).unwrap_or(i64::MAX)),
        Value::Integer(row_id),
        Value::Text(path.to_string()),
        Value::Integer(idx),
    ];
    values.extend(classification_values.into_iter().map(Value::Text));
    let found: Option<(i64, Option<String>)> = reader
        .connection
        .query_row(
            &format!(
                "SELECT length(CAST(content AS BLOB)),
                CASE WHEN length(CAST(content AS BLOB)) <= ? THEN content ELSE NULL END
         FROM chunks WHERE {clause} LIMIT 1"
            ),
            params_from_iter(values.iter()),
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()
        .map_err(|error| reader.classify(error))?;
    Ok(match found {
        None => IndexedEntry::NotFound,
        Some((_, Some(content))) => IndexedEntry::Found(content),
        Some(_) => IndexedEntry::NotFound,
    })
}

impl QueryConnection {
    fn new(connection: Connection, path: PathBuf) -> Self {
        Self {
            connection,
            path,
            #[cfg(test)]
            aggregate_calls: 0,
            #[cfg(test)]
            agents_calls: 0,
        }
    }

    fn require_nonempty_chunks(
        &mut self,
        boundary: &QueryBoundary,
    ) -> Result<(), IndexAccessError> {
        let chunks_exists: Option<i64> = self
            .connection
            .query_row(
                "SELECT 1 FROM sqlite_master WHERE type='table' AND name='chunks'",
                [],
                |row| row.get(0),
            )
            .optional()
            .map_err(|error| self.classify(error))?;
        if chunks_exists.is_none() {
            return Err(IndexAccessError::Empty {
                path: self.path.clone(),
            });
        }
        if matches!(boundary, QueryBoundary::Connection(_))
            && !self.classification_tables_exist()?
        {
            return Err(IndexAccessError::Empty {
                path: self.path.clone(),
            });
        }
        let found: Option<i64> = self
            .connection
            .query_row(
                &format!("SELECT 1 FROM chunks WHERE {} LIMIT 1", visible_rows("1=1")),
                [],
                |row| row.get(0),
            )
            .optional()
            .map_err(|error| self.classify(error))?;
        if found.is_none() {
            return Err(IndexAccessError::Empty {
                path: self.path.clone(),
            });
        }
        Ok(())
    }

    fn classification_tables_exist(&self) -> Result<bool, IndexAccessError> {
        solstone_core_indexer_store::db::chunk_classification_tables_exist(&self.connection)
            .map_err(|error| IndexAccessError::Unreadable {
                path: self.path.clone(),
                detail: error.to_string(),
            })
    }

    fn classification_coverage_complete(&self) -> Result<bool, IndexAccessError> {
        // Coverage is unquantifiable: it deliberately exposes no withheld count.
        if !self.classification_tables_exist()? {
            return Ok(false);
        }
        let state =
            solstone_core_indexer_store::db::read_chunk_classification_backfill(&self.connection)
                .map_err(|error| IndexAccessError::Unreadable {
                path: self.path.clone(),
                detail: error.to_string(),
            })?;
        let Some(state) = state else {
            return Ok(false);
        };
        if !state.completed || state.stalled {
            return Ok(false);
        }
        let incomplete: i64 = self.connection.query_row(
            "SELECT EXISTS(SELECT 1 FROM chunks c LEFT JOIN chunk_classification cc ON cc.path=c.path WHERE cc.path IS NULL) OR EXISTS(SELECT 1 FROM chunk_classification WHERE unclassified=1) OR (EXISTS(SELECT 1 FROM files) AND NOT EXISTS(SELECT 1 FROM chunk_classification))",
            [], |row| row.get(0),
        ).map_err(|error| self.classify(error))?;
        Ok(incomplete == 0)
    }

    fn connection_index_degraded(
        &self,
    ) -> Result<Option<ConnectionIndexDegraded>, IndexAccessError> {
        let state = solstone_core_indexer_store::db::read_index_build_state(&self.connection)
            .map_err(|error| IndexAccessError::Unreadable {
                path: self.path.clone(),
                detail: error.to_string(),
            })?;
        Ok(match state {
            None => Some(ConnectionIndexDegraded::Unknown),
            Some(state)
                if state.state
                    == solstone_core_indexer_store::db::IndexBuildLifecycle::Building =>
            {
                Some(ConnectionIndexDegraded::Building {
                    state_schema_version: state.schema_version,
                })
            }
            Some(_) => None,
        })
    }

    fn index_degraded(&self) -> Result<Option<IndexDegraded>, IndexAccessError> {
        let state = solstone_core_indexer_store::db::read_index_build_state(&self.connection)
            .map_err(|error| match error {
                solstone_core_indexer_store::StoreError::Sql(error) => self.classify(error),
                other => IndexAccessError::Unreadable {
                    path: self.path.clone(),
                    detail: other.to_string(),
                },
            })?;
        let Some(state) = state else {
            return Ok(Some(IndexDegraded::Unknown));
        };
        match state.state {
            solstone_core_indexer_store::db::IndexBuildLifecycle::Complete => Ok(None),
            solstone_core_indexer_store::db::IndexBuildLifecycle::Building => {
                let (files, chunks): (i64, i64) = self
                    .connection
                    .query_row(
                        "SELECT (SELECT count(*) FROM files), (SELECT count(*) FROM chunks)",
                        [],
                        |row| Ok((row.get(0)?, row.get(1)?)),
                    )
                    .map_err(|error| self.classify(error))?;
                Ok(Some(IndexDegraded::Building {
                    state_schema_version: state.schema_version,
                    recorded_counts: IndexBuildCounts {
                        files: state.files_count as u64,
                        chunks: state.chunks_count as u64,
                    },
                    observed_counts: IndexBuildCounts {
                        files: files as u64,
                        chunks: chunks as u64,
                    },
                }))
            }
        }
    }

    pub(crate) fn has_rows(&mut self, plan: &SqlPlan) -> Result<bool, IndexAccessError> {
        let sql = format!("SELECT 1 FROM chunks WHERE {} LIMIT 1", plan.where_clause);
        let found: Option<i64> = self
            .connection
            .query_row(&sql, params_from_iter(plan.params.iter()), |row| row.get(0))
            .optional()
            .map_err(|error| self.classify(error))?;
        Ok(found.is_some())
    }

    fn fetch_hits(
        &mut self,
        plan: &SqlPlan,
        limit: usize,
        offset: usize,
        order: Order,
    ) -> Result<Vec<SearchHit>, IndexAccessError> {
        let ordering = match order {
            Order::Relevance => "ORDER BY bm25(chunks) ASC, rowid ASC",
            Order::Recency => "ORDER BY day DESC, rowid DESC",
        };
        self.fetch_hits_with_ordering(plan, limit, offset, ordering)
    }

    fn fetch_connection_hits(
        &mut self,
        plan: &SqlPlan,
        limit: usize,
    ) -> Result<Vec<SearchHit>, IndexAccessError> {
        const CONNECTION_RECENCY_ORDER: &str =
            "ORDER BY COALESCE(day, '') DESC, path DESC, idx DESC";
        self.fetch_hits_with_ordering(plan, limit, 0, CONNECTION_RECENCY_ORDER)
    }

    fn fetch_hits_with_ordering(
        &mut self,
        plan: &SqlPlan,
        limit: usize,
        offset: usize,
        ordering: &str,
    ) -> Result<Vec<SearchHit>, IndexAccessError> {
        let sql = format!(
            "SELECT content, path, day, facet, agent, stream, idx, bm25(chunks), rowid FROM chunks WHERE {} {ordering} LIMIT ? OFFSET ?",
            plan.where_clause
        );
        let mut values = plan.params.clone();
        values.push(usize_to_sql(limit));
        values.push(usize_to_sql(offset));
        let mut statement = self
            .connection
            .prepare(&sql)
            .map_err(|error| self.classify(error))?;
        let rows = statement
            .query_map(params_from_iter(values.iter()), |row| {
                let content: String = row.get(0)?;
                let path: String = row.get(1)?;
                let day: Option<String> = row.get(2)?;
                let facet: Option<String> = row.get(3)?;
                let agent: Option<String> = row.get(4)?;
                let stream: Option<String> = row.get(5)?;
                let idx: i64 = row.get(6)?;
                let score: f64 = row.get(7)?;
                let agent = agent.unwrap_or_default();
                Ok(SearchHit {
                    row_id: row.get(8)?,
                    id: format!("{path}:{idx}"),
                    text: content,
                    metadata: SearchMetadata {
                        day: day.unwrap_or_default(),
                        facet: facet.unwrap_or_default(),
                        agent: agent.clone(),
                        stream: stream.unwrap_or_default(),
                        path,
                        idx,
                    },
                    score,
                })
            })
            .map_err(|error| self.classify(error))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|error| self.classify(error))?;
        Ok(rows)
    }

    fn hit_at(&mut self, path: &str, idx: i64) -> Result<bool, IndexAccessError> {
        let found: Option<i64> = self
            .connection
            .query_row(
                &format!(
                    "SELECT 1 FROM chunks WHERE {} LIMIT 1",
                    visible_rows("path=?1 AND idx=?2")
                ),
                params![path, idx],
                |row| row.get(0),
            )
            .optional()
            .map_err(|error| self.classify(error))?;
        Ok(found.is_some())
    }

    fn aggregate_counts(
        &mut self,
        plan: &SqlPlan,
        relaxed: bool,
    ) -> Result<CountsResponse, IndexAccessError> {
        #[cfg(test)]
        {
            self.aggregate_calls += 1;
        }
        let sql = format!(
            "SELECT facet, agent, day, stream FROM chunks WHERE {}",
            plan.where_clause
        );
        let mut statement = self
            .connection
            .prepare(&sql)
            .map_err(|error| self.classify(error))?;
        let rows = statement
            .query_map(params_from_iter(plan.params.iter()), |row| {
                Ok((
                    row.get::<_, Option<String>>(0)?,
                    row.get::<_, Option<String>>(1)?,
                    row.get::<_, Option<String>>(2)?,
                    row.get::<_, Option<String>>(3)?,
                ))
            })
            .map_err(|error| self.classify(error))?;
        let mut counts = CountsResponse {
            relaxed,
            ..CountsResponse::default()
        };
        // Counts retain only distinct keys, never one owned tuple per match.
        // Large journals otherwise allocate hundreds of MiB just to count rows.
        for row in rows {
            let (facet, agent, day, stream) = row.map_err(|error| self.classify(error))?;
            counts.total += 1;
            increment_nonempty(&mut counts.facets, facet);
            increment_nonempty(&mut counts.agents, agent);
            increment_nonempty(&mut counts.days, day);
            increment_nonempty(&mut counts.streams, stream);
        }
        Ok(counts)
    }

    fn agents(&mut self) -> Result<Vec<String>, IndexAccessError> {
        #[cfg(test)]
        {
            self.agents_calls += 1;
        }
        let mut statement = self
            .connection
            .prepare(&format!(
                "SELECT DISTINCT agent FROM chunks WHERE {} ORDER BY agent ASC",
                visible_rows("agent IS NOT NULL AND agent != ''")
            ))
            .map_err(|error| self.classify(error))?;
        statement
            .query_map([], |row| row.get::<_, String>(0))
            .map_err(|error| self.classify(error))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|error| self.classify(error))
    }

    fn indexed_entity_ids(&mut self) -> Result<BTreeSet<String>, IndexAccessError> {
        let mut statement = self
            .connection
            .prepare(
                "SELECT DISTINCT path FROM chunks \
                 WHERE agent='entity' AND path LIKE 'entity_search:%' \
                 ORDER BY path ASC",
            )
            .map_err(|error| self.classify(error))?;
        let paths = statement
            .query_map([], |row| row.get::<_, String>(0))
            .map_err(|error| self.classify(error))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|error| self.classify(error))?;
        Ok(paths
            .into_iter()
            .filter_map(|path| path.strip_prefix("entity_search:").map(str::to_owned))
            .filter(|entity_id| !entity_id.is_empty())
            .collect())
    }

    fn coverage(&mut self) -> Result<CoverageResponse, IndexAccessError> {
        let (start, end): (Option<String>, Option<String>) = self
            .connection
            .query_row(
                &format!(
                    "SELECT MIN(day), MAX(day) FROM chunks WHERE {}",
                    visible_rows("day != ''")
                ),
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .map_err(|error| self.classify(error))?;
        if start.is_none() || end.is_none() {
            return Ok(CoverageResponse {
                state: CoverageState::NoDatedChunks,
                start: None,
                end: None,
                degraded: None,
            });
        }
        Ok(CoverageResponse {
            state: CoverageState::Available,
            start,
            end,
            degraded: None,
        })
    }

    fn classify(&self, error: Error) -> IndexAccessError {
        classify_sql_error(self.path.clone(), error)
    }
}

fn increment_nonempty(values: &mut BTreeMap<String, u64>, value: Option<String>) {
    if let Some(value) = value.filter(|value| !value.is_empty()) {
        *values.entry(value).or_default() += 1;
    }
}

fn usize_to_sql(value: usize) -> String {
    i64::try_from(value).unwrap_or(i64::MAX).to_string()
}

fn classify_sql_error(path: PathBuf, error: Error) -> IndexAccessError {
    let detail = error.to_string();
    if matches!(
        error,
        Error::SqliteFailure(ref sqlite, _)
            if matches!(sqlite.code, ErrorCode::DatabaseBusy | ErrorCode::DatabaseLocked)
    ) {
        IndexAccessError::Locked { path, detail }
    } else {
        IndexAccessError::Unreadable { path, detail }
    }
}

#[cfg(test)]
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) struct QueryCounters {
    pub(crate) aggregate_calls: usize,
    pub(crate) agents_calls: usize,
}

#[cfg(test)]
pub(crate) fn search_with_connection_for_test(
    connection: Connection,
    path: PathBuf,
    request: &SearchRequest,
    reference_date: NaiveDate,
) -> Result<(SearchResponse, QueryCounters), IndexAccessError> {
    let mut connection = QueryConnection::new(connection, path);
    let compilation = compile_query(&request.query, reference_date);
    let response = search_on_connection(&mut connection, request, reference_date, compilation)?;
    Ok((
        response,
        QueryCounters {
            aggregate_calls: connection.aggregate_calls,
            agents_calls: connection.agents_calls,
        },
    ))
}

#[cfg(test)]
pub(crate) fn agents_with_connection_for_test(
    connection: Connection,
    path: PathBuf,
) -> Result<(Vec<String>, QueryCounters), IndexAccessError> {
    let mut connection = QueryConnection::new(connection, path);
    let agents = connection.agents()?;
    Ok((
        agents,
        QueryCounters {
            aggregate_calls: connection.aggregate_calls,
            agents_calls: connection.agents_calls,
        },
    ))
}
