// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use chrono::NaiveDate;
use rusqlite::{
    Connection, Error, ErrorCode, OpenFlags, OptionalExtension, params, params_from_iter,
};

use crate::compile::{CompileOutcome, compile_query};
use crate::ladder::relaxed_plan;
use crate::predicate::{EffectiveDateConstraint, PredicateInput, QueryPredicate};
use crate::temporal::TemporalExtraction;
use crate::types::{
    CountsResponse, CoverageResponse, CoverageState, IndexAccessError, IndexBuildCounts,
    IndexDegraded, Order, SearchHit, SearchMetadata, SearchRequest, SearchResponse,
};

/// Execute one journal search.
///
/// The index is opened read-only after a brief derived-row cleanup on the
/// shared access path.
pub fn search(
    journal: &Path,
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
    let mut connection = open_read_only(journal)?;
    search_on_connection(&mut connection, request, reference_date, compilation)
}

/// Execute journal aggregation independently of a search invocation.
pub fn search_counts(
    journal: &Path,
    request: &SearchRequest,
    reference_date: NaiveDate,
) -> Result<CountsResponse, IndexAccessError> {
    let compilation = compile_query(&request.query, reference_date);
    if matches!(compilation.outcome, CompileOutcome::NoTokenizableTerm) {
        return Ok(CountsResponse::default());
    }
    let mut connection = open_read_only(journal)?;
    let (plan, relaxed) = resolve_plan(&mut connection, request, reference_date, compilation)?;
    let mut counts = connection.aggregate_counts(&plan, relaxed)?;
    counts.degraded = connection.index_degraded()?;
    Ok(counts)
}

/// Return whether one exact journal path and chunk index are represented in the index.
pub fn hit_at(journal: &Path, path: &str, idx: i64) -> Result<bool, IndexAccessError> {
    let mut connection = open_read_only(journal)?;
    connection.hit_at(path, idx)
}

/// Return the distinct nonempty indexed agents. Search never calls this query.
pub fn agents(journal: &Path) -> Result<Vec<String>, IndexAccessError> {
    let mut connection = open_read_only(journal)?;
    connection.agents()
}

/// Return the dated span of a nonempty index.
pub fn coverage(journal: &Path) -> Result<CoverageResponse, IndexAccessError> {
    let mut connection = open_read_only(journal)?;
    let mut coverage = connection.coverage()?;
    coverage.degraded = connection.index_degraded()?;
    Ok(coverage)
}

/// Return the canonical entity IDs represented by indexed entity-search rows.
pub fn indexed_entity_ids(journal: &Path) -> Result<BTreeSet<String>, IndexAccessError> {
    let mut connection = open_read_only(journal)?;
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
    let mut plan = plan_from_outcome(compilation.outcome.clone(), &compilation.temporal, request);
    let mut relaxed = false;
    if request.relax
        && !connection.has_rows(&plan)?
        && let Some(candidate) = relaxed_plan(connection, &compilation, request, reference_date)?
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

fn open_read_only(journal: &Path) -> Result<QueryConnection, IndexAccessError> {
    let path = solstone_core_indexer_store::db::db_path(journal);
    if !path.is_file() {
        return Err(IndexAccessError::Absent { path });
    }
    solstone_core_indexer_store::db::prune_authored_chat_paths(journal).map_err(
        |error| match error {
            solstone_core_indexer_store::StoreError::Sql(sql_error) => {
                classify_sql_error(path.clone(), sql_error)
            }
            other => IndexAccessError::Unreadable {
                path: path.clone(),
                detail: other.to_string(),
            },
        },
    )?;
    open_index_reader(journal)
}

fn open_index_reader(journal: &Path) -> Result<QueryConnection, IndexAccessError> {
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
    connection.require_nonempty_chunks()?;
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
    path: &str,
    idx: i64,
    row_id: i64,
    max_bytes: u64,
) -> Result<IndexedEntry, IndexAccessError> {
    let reader = open_index_reader(journal)?;
    let found: Option<(i64, Option<String>)> = reader
        .connection
        .query_row(
            "SELECT length(CAST(content AS BLOB)),
                CASE WHEN length(CAST(content AS BLOB)) <= ?4 THEN content ELSE NULL END
         FROM chunks WHERE rowid=?1 AND path=?2 AND idx=?3
           AND NOT (path LIKE '________/chat/%/chat.jsonl'
                    OR path LIKE 'chronicle/________/chat/%/chat.jsonl') LIMIT 1",
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
    Ok(match found {
        None => IndexedEntry::NotFound,
        Some((_, Some(content))) => IndexedEntry::Found(content),
        Some(_) => IndexedEntry::TooLarge,
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

    fn require_nonempty_chunks(&mut self) -> Result<(), IndexAccessError> {
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
        let found: Option<i64> = self
            .connection
            .query_row("SELECT 1 FROM chunks LIMIT 1", [], |row| row.get(0))
            .optional()
            .map_err(|error| self.classify(error))?;
        if found.is_none() {
            return Err(IndexAccessError::Empty {
                path: self.path.clone(),
            });
        }
        Ok(())
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
        let sql = format!(
            "SELECT content, path, day, facet, agent, stream, idx, bm25(chunks), rowid \
             FROM chunks WHERE {} {ordering} LIMIT ? OFFSET ?",
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
                "SELECT 1 FROM chunks WHERE path=?1 AND idx=?2 LIMIT 1",
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
            .map_err(|error| self.classify(error))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|error| self.classify(error))?;
        let mut counts = CountsResponse {
            total: rows.len() as u64,
            relaxed,
            ..CountsResponse::default()
        };
        for (facet, agent, day, stream) in rows {
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
            .prepare(
                "SELECT DISTINCT agent FROM chunks \
                 WHERE agent IS NOT NULL AND agent != '' ORDER BY agent ASC",
            )
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
                "SELECT MIN(day), MAX(day) FROM chunks WHERE day != ''",
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
