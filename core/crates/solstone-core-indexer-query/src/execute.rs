// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use chrono::NaiveDate;
use rusqlite::functions::FunctionFlags;
use rusqlite::types::Value;
use rusqlite::{
    Connection, Error, ErrorCode, OpenFlags, OptionalExtension, params, params_from_iter,
};
use solstone_core_indexer_store::db::AUTHORED_CHAT_PATH_PREDICATE;

use crate::compile::{CompileOutcome, QueryCompilation, compile_query};
use crate::ladder::relaxed_plan;
use crate::predicate::{EffectiveDateConstraint, PredicateInput, QueryPredicate};
use crate::temporal::TemporalExtraction;
use crate::types::{
    AdmittedCategory, CalendarHit, ConnectionBoundary, ConnectionCorpusRefusal,
    ConnectionIndexDegraded, ConnectionScope, ConnectionSearchHit, ConnectionSearchRequest,
    ConnectionSearchResponse, CountsResponse, CoverageResponse, CoverageState, IndexAccessError,
    IndexBuildCounts, IndexDegraded, Order, OwnerBoundary, QueryBoundary, SearchHit,
    SearchMetadata, SearchRequest, SearchResponse,
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
    if let Some(response) = untokenizable_response(&compilation) {
        return Ok(response);
    }
    let mut connection = open_index_reader(journal, &QueryBoundary::Owner)?;
    search_on_connection(&mut connection, request, reference_date, compilation)
}

fn untokenizable_response(compilation: &QueryCompilation) -> Option<SearchResponse> {
    matches!(compilation.outcome, CompileOutcome::NoTokenizableTerm).then(|| SearchResponse {
        results: Vec::new(),
        order: order_for_plan(false),
        relaxed: false,
        total: None,
        counts: None,
        reason: Some("not_tokenizable".to_string()),
        cleaned_query: compilation.temporal.remaining_text.clone(),
        degraded: None,
    })
}

/// Execute journal aggregation independently of a search invocation.
pub fn search_counts(
    journal: &Path,
    _boundary: OwnerBoundary,
    request: &SearchRequest,
    reference_date: NaiveDate,
) -> Result<CountsResponse, IndexAccessError> {
    let mut owner_index = open_owner_index(journal, OwnerBoundary)?;
    let resolved = owner_index.resolve_counts(request, reference_date)?;
    Ok(resolved.counts)
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
    // A query with no searchable word still lists a bounded day range, as an
    // empty query does; without a day bound it finds nothing.
    let day_bounded =
        request.day.is_some() || request.day_from.is_some() || request.day_to.is_some();
    if no_tokenizable_term && !day_bounded {
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
    let results = if let Some(journal) = connection.journal_root() {
        let mut filtered = Vec::with_capacity(results.len());
        let mut source_checks = BTreeMap::new();
        for hit in results {
            if solstone_core_format::content::screen_text::is_screen_text_path(&hit.metadata.path)
                && !*source_checks
                    .entry((
                        hit.metadata.path.clone(),
                        hit.metadata.source_digest.clone(),
                    ))
                    .or_insert_with(|| {
                        solstone_core_indexer_store::screen_text_current_digest(
                            &connection.connection,
                            journal,
                            &hit.metadata.path,
                        )
                        .ok()
                        .flatten()
                        .is_some_and(|digest| Some(&digest) == hit.metadata.source_digest.as_ref())
                    })
            {
                continue;
            }
            filtered.push(hit);
        }
        filtered
    } else {
        results
    };
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
    if !boundary
        .categories()
        .contains(&AdmittedCategory::ScreenText)
    {
        plan.where_clause
            .push_str(" AND NOT solstone_screen_text_path(chunks.path)");
    }
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
        AdmittedCategory::ScreenText => "screen_text",
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

/// An opaque resolved query plan.
pub struct SearchPlan(pub(crate) SqlPlan);

/// Resolved counts and corresponding opaque query plan.
pub struct ResolvedCounts {
    pub counts: CountsResponse,
    pub plan: SearchPlan,
}

/// A shared reader session for owner index queries.
pub struct OwnerIndex {
    pub(crate) connection: QueryConnection,
}

/// Open an owner index reader session.
pub fn open_owner_index(
    journal_root: &Path,
    _boundary: OwnerBoundary,
) -> Result<OwnerIndex, IndexAccessError> {
    let connection = open_index_reader(journal_root, &QueryBoundary::Owner)?;
    Ok(OwnerIndex { connection })
}

impl OwnerIndex {
    /// Check the source identity captured with a screen-text hit before release.
    /// Ordinary families retain their existing release rules.
    pub fn screen_text_hit_current(&self, hit: &SearchHit) -> bool {
        if !solstone_core_format::content::screen_text::is_screen_text_path(&hit.metadata.path) {
            return true;
        }
        let Some(journal) = self.connection.journal_root() else {
            return false;
        };
        solstone_core_indexer_store::screen_text_current_digest(
            &self.connection.connection,
            journal,
            &hit.metadata.path,
        )
        .ok()
        .flatten()
        .is_some_and(|digest| Some(&digest) == hit.metadata.source_digest.as_ref())
    }

    /// Search using this reader without opening another SQLite connection.
    ///
    /// Each invocation finishes its statements; it does not retain a read
    /// transaction between calls. Commits to the same index become visible on
    /// the next call. Reopen this session if the index file itself is replaced.
    pub fn search(
        &mut self,
        request: &SearchRequest,
        reference_date: NaiveDate,
    ) -> Result<SearchResponse, IndexAccessError> {
        let compilation = compile_query(&request.query, reference_date);
        if let Some(response) = untokenizable_response(&compilation) {
            return Ok(response);
        }
        search_on_connection(&mut self.connection, request, reference_date, compilation)
    }

    pub fn resolve_counts(
        &mut self,
        request: &SearchRequest,
        reference: NaiveDate,
    ) -> Result<ResolvedCounts, IndexAccessError> {
        let compilation = compile_query(&request.query, reference);
        if matches!(compilation.outcome, CompileOutcome::NoTokenizableTerm) {
            let plan = plan_from_outcome(compilation.outcome, &compilation.temporal, request);
            return Ok(ResolvedCounts {
                counts: CountsResponse::default(),
                plan: SearchPlan(plan),
            });
        }
        let (plan, relaxed) = resolve_plan(&mut self.connection, request, reference, compilation)?;
        let mut counts = self.connection.aggregate_counts(&plan, relaxed)?;
        counts.degraded = self.connection.index_degraded()?;
        Ok(ResolvedCounts {
            counts,
            plan: SearchPlan(plan),
        })
    }

    pub fn fetch_day_hits(
        &mut self,
        plan: &SearchPlan,
        days: &[String],
        per_day_limit: usize,
    ) -> Result<Vec<(String, Vec<SearchHit>)>, IndexAccessError> {
        self.connection.fetch_day_hits(&plan.0, days, per_day_limit)
    }

    #[cfg(any(test, feature = "test-hooks"))]
    pub fn query_counters(&self) -> QueryCounters {
        QueryCounters {
            aggregate_calls: self.connection.aggregate_calls,
            fetch_hits_calls: self.connection.fetch_hits_calls,
            agents_calls: self.connection.agents_calls,
        }
    }

    #[cfg(any(test, feature = "test-hooks"))]
    pub fn inject_aggregate_failure(&mut self) {
        self.connection.injected_aggregate_failure = true;
    }

    #[cfg(any(test, feature = "test-hooks"))]
    pub fn inject_fetch_failure(&mut self) {
        self.connection.injected_fetch_failure = true;
    }

    pub fn raw_connection(&self) -> &Connection {
        &self.connection.connection
    }
}

#[cfg(test)]
impl OwnerIndex {
    pub(crate) fn trace_v2(
        &self,
        flags: rusqlite::trace::TraceEventCodes,
        callback: Option<fn(rusqlite::trace::TraceEvent<'_>)>,
    ) {
        self.connection.connection.trace_v2(flags, callback);
    }
}

pub(crate) struct QueryConnection {
    connection: Connection,
    path: PathBuf,
    #[cfg(any(test, feature = "test-hooks"))]
    aggregate_calls: usize,
    #[cfg(any(test, feature = "test-hooks"))]
    fetch_hits_calls: usize,
    #[cfg(any(test, feature = "test-hooks"))]
    agents_calls: usize,
    #[cfg(any(test, feature = "test-hooks"))]
    injected_aggregate_failure: bool,
    #[cfg(any(test, feature = "test-hooks"))]
    injected_fetch_failure: bool,
}

impl QueryConnection {
    pub(crate) fn journal_root(&self) -> Option<&Path> {
        self.path.parent().and_then(|p| p.parent())
    }
}

fn visible_rows(clause: &str) -> String {
    format!("{clause} AND NOT ({AUTHORED_CHAT_PATH_PREDICATE})")
}

fn open_index_reader(
    journal: &Path,
    boundary: &QueryBoundary,
) -> Result<QueryConnection, IndexAccessError> {
    if matches!(boundary, QueryBoundary::OwnMemory { .. }) {
        return Err(IndexAccessError::Unreadable {
            path: solstone_core_indexer_store::db::db_path(journal),
            detail: "own-memory reads use the dedicated private index opener".to_owned(),
        });
    }
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
    let mut connection = QueryConnection::new(connection, path)?;
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

/// Indexed activity text, the file digest that produced it, and its stored
/// hidden flag. Owner search results do not carry the digest.
#[derive(Debug, PartialEq, Eq)]
pub struct ActivityObservation {
    pub content: String,
    pub digest: [u8; 32],
    pub hidden: bool,
}

/// One bounded observation of text, written family, and optional Activity proof.
/// Missing rows never authorize an earlier search candidate. Legacy rows whose
/// agent cannot be Activity remain readable; ambiguous rows need writer refresh.
#[derive(Debug, PartialEq, Eq)]
pub enum ActivityObservationRead {
    Absent,
    Unproven,
    Unavailable,
    NonActivity(String),
    Ready(ActivityObservation),
}

/// Reads text and family together; malformed digests are projected to NULL
/// before allocation. This reader never repairs the index or opens a sidecar.
pub fn read_activity_observation(
    journal: &Path,
    boundary: QueryBoundary,
    path: &str,
    idx: i64,
    row_id: i64,
    max_bytes: u64,
) -> Result<ActivityObservationRead, IndexAccessError> {
    let reader = match open_index_reader(journal, &boundary) {
        Ok(reader) => reader,
        Err(IndexAccessError::Absent { .. } | IndexAccessError::Empty { .. }) => {
            return Ok(ActivityObservationRead::Absent);
        }
        Err(error) => return Err(error),
    };
    let mut clause = String::from(
        "chunks.rowid=? AND chunks.path=? AND chunks.idx=? AND NOT (\
         chunks.path LIKE '________/chat/%/chat.jsonl' OR \
         chunks.path LIKE 'chronicle/________/chat/%/chat.jsonl')",
    );
    let mut values = vec![
        Value::Integer(i64::try_from(max_bytes).unwrap_or(i64::MAX)),
        Value::Integer(row_id),
        Value::Text(path.to_string()),
        Value::Integer(idx),
    ];
    if let QueryBoundary::Connection(connection_boundary) = &boundary {
        let mut plan = SqlPlan {
            where_clause: clause,
            params: Vec::new(),
            has_live_match_expression: false,
        };
        append_connection_prefilter(&mut plan, connection_boundary);
        clause = plan.where_clause;
        values.extend(plan.params.into_iter().map(Value::Text));
    }
    let table_exists = |table: &str| -> Result<bool, IndexAccessError> {
        reader
            .connection
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name=?)",
                [table],
                |row| row.get(0),
            )
            .map_err(|e| reader.classify(e))
    };
    let proof_table = table_exists("activity_provenance")?;
    let kind_table = table_exists("activity_membership")?;
    let proof_columns = if proof_table {
        "CASE WHEN p.rowid IS NOT NULL THEN 1 ELSE 0 END,
         CASE WHEN typeof(p.digest)='blob' AND length(p.digest)=32 THEN p.digest ELSE NULL END,
         CASE WHEN typeof(p.hidden)='integer' AND p.hidden IN (0,1) THEN p.hidden ELSE NULL END"
    } else {
        "0, NULL, NULL"
    };
    let kind_column = if kind_table {
        "CASE WHEN typeof(m.is_activity)='integer' AND m.is_activity IN (0,1) THEN m.is_activity ELSE NULL END"
    } else {
        "NULL"
    };
    let proof_join = if proof_table {
        "LEFT JOIN activity_provenance AS p ON p.rowid=chunks.rowid"
    } else {
        ""
    };
    let kind_join = if kind_table {
        "LEFT JOIN activity_membership AS m ON m.rowid=chunks.rowid"
    } else {
        ""
    };
    let found = reader.connection.query_row(
        &format!("SELECT CASE WHEN length(CAST(chunks.content AS BLOB)) <= ? THEN chunks.content ELSE NULL END,
                    {proof_columns}, {kind_column},
                    CASE WHEN chunks.agent IS NOT NULL AND chunks.agent != 'activity' THEN 1 ELSE 0 END
             FROM chunks {proof_join} {kind_join} WHERE {clause} LIMIT 1"),
        params_from_iter(values.iter()),
        |row| Ok((row.get::<_,Option<String>>(0)?, row.get::<_,i64>(1)?,
            row.get::<_,Option<Vec<u8>>>(2)?, row.get::<_,Option<i64>>(3)?,
            row.get::<_,Option<i64>>(4)?, row.get::<_,bool>(5)?)),
    ).optional().map_err(|error| reader.classify(error))?;
    let Some((content, proof_present, digest, hidden, kind, legacy_non_activity)) = found else {
        return Ok(ActivityObservationRead::Absent);
    };
    let Some(content) = content else {
        return Ok(ActivityObservationRead::Unavailable);
    };
    if kind == Some(0) {
        return Ok(if proof_present == 0 {
            ActivityObservationRead::NonActivity(content)
        } else {
            ActivityObservationRead::Unavailable
        });
    }
    if proof_present == 0 {
        let canonical_activity = solstone_core_format::content::classify(path)
            == solstone_core_format::content::ContentResolution::Indexed(
                solstone_core_format::content::Family::Activity,
            );
        return Ok(
            if kind.is_none() && legacy_non_activity && !canonical_activity {
                ActivityObservationRead::NonActivity(content)
            } else {
                ActivityObservationRead::Unproven
            },
        );
    }
    let (Some(digest), Some(hidden)) = (digest, hidden) else {
        return Ok(ActivityObservationRead::Unavailable);
    };
    let Ok(digest) = <[u8; 32]>::try_from(digest.as_slice()) else {
        return Ok(ActivityObservationRead::Unavailable);
    };
    Ok(ActivityObservationRead::Ready(ActivityObservation {
        content,
        digest,
        hidden: hidden == 1,
    }))
}

/// The indexed entries one source file produced, in file order, inside a
/// connection's boundary.
pub struct IndexedPathEntries {
    pub entries: Vec<String>,
    /// More entries exist than were returned, or one was too large to return.
    pub truncated: bool,
}

/// Read up to `max_entries` indexed entries of one path, without opening its
/// source file, admitting only rows the connection's boundary admits.
///
/// An absent or empty index is no entries: the path has nothing indexed yet.
pub fn read_indexed_path_entries(
    journal: &Path,
    boundary: &ConnectionBoundary,
    path: &str,
    max_entries: usize,
    max_entry_bytes: u64,
) -> Result<IndexedPathEntries, IndexAccessError> {
    let query_boundary = QueryBoundary::Connection(boundary.clone());
    let reader = match open_index_reader(journal, &query_boundary) {
        Ok(reader) => reader,
        Err(IndexAccessError::Absent { .. } | IndexAccessError::Empty { .. }) => {
            return Ok(IndexedPathEntries {
                entries: Vec::new(),
                truncated: false,
            });
        }
        Err(error) => return Err(error),
    };
    let mut plan = SqlPlan {
        where_clause: visible_rows("path=?"),
        params: Vec::new(),
        has_live_match_expression: false,
    };
    append_connection_prefilter(&mut plan, boundary);
    let mut values = vec![
        Value::Integer(i64::try_from(max_entry_bytes).unwrap_or(i64::MAX)),
        Value::Text(path.to_string()),
    ];
    values.extend(plan.params.into_iter().map(Value::Text));
    values.push(Value::Integer(
        i64::try_from(max_entries.saturating_add(1)).unwrap_or(i64::MAX),
    ));
    let mut statement = reader
        .connection
        .prepare(&format!(
            "SELECT CASE WHEN length(CAST(content AS BLOB)) <= ? THEN content ELSE NULL END
             FROM chunks WHERE {} ORDER BY idx LIMIT ?",
            plan.where_clause
        ))
        .map_err(|error| reader.classify(error))?;
    let rows = statement
        .query_map(params_from_iter(values.iter()), |row| {
            row.get::<_, Option<String>>(0)
        })
        .map_err(|error| reader.classify(error))?;
    let mut entries = Vec::new();
    let mut truncated = false;
    for row in rows {
        match row.map_err(|error| reader.classify(error))? {
            _ if entries.len() == max_entries => {
                truncated = true;
                break;
            }
            Some(content) => entries.push(content),
            None => truncated = true,
        }
    }
    Ok(IndexedPathEntries { entries, truncated })
}

/// Whether this connection's boundary admits a chunk of `path` after index
/// zero. Two rows that both sit at index zero are still one indexed chunk.
/// An absent or empty index has no later chunk.
pub fn indexed_path_has_later_chunk(
    journal: &Path,
    boundary: &ConnectionBoundary,
    path: &str,
) -> Result<bool, IndexAccessError> {
    let query_boundary = QueryBoundary::Connection(boundary.clone());
    let reader = match open_index_reader(journal, &query_boundary) {
        Ok(reader) => reader,
        Err(IndexAccessError::Absent { .. } | IndexAccessError::Empty { .. }) => {
            return Ok(false);
        }
        Err(error) => return Err(error),
    };
    let mut plan = SqlPlan {
        where_clause: visible_rows("path=? AND idx>0"),
        params: Vec::new(),
        has_live_match_expression: false,
    };
    append_connection_prefilter(&mut plan, boundary);
    let mut values = vec![Value::Text(path.to_string())];
    values.extend(plan.params.into_iter().map(Value::Text));
    let found: Option<i64> = reader
        .connection
        .query_row(
            &format!("SELECT 1 FROM chunks WHERE {} LIMIT 1", plan.where_clause),
            params_from_iter(values.iter()),
            |row| row.get(0),
        )
        .optional()
        .map_err(|error| reader.classify(error))?;
    Ok(found.is_some())
}

impl QueryConnection {
    pub(crate) fn new(connection: Connection, path: PathBuf) -> Result<Self, IndexAccessError> {
        connection
            .create_scalar_function(
                "solstone_screen_text_path",
                1,
                FunctionFlags::SQLITE_UTF8
                    | FunctionFlags::SQLITE_DETERMINISTIC
                    | FunctionFlags::SQLITE_INNOCUOUS,
                |context| {
                    let path: String = context.get(0)?;
                    Ok(solstone_core_format::content::screen_text::is_screen_text_path(&path))
                },
            )
            .map_err(|error| classify_sql_error(path.clone(), error))?;
        Ok(Self {
            connection,
            path,
            #[cfg(any(test, feature = "test-hooks"))]
            aggregate_calls: 0,
            #[cfg(any(test, feature = "test-hooks"))]
            fetch_hits_calls: 0,
            #[cfg(any(test, feature = "test-hooks"))]
            agents_calls: 0,
            #[cfg(any(test, feature = "test-hooks"))]
            injected_aggregate_failure: false,
            #[cfg(any(test, feature = "test-hooks"))]
            injected_fetch_failure: false,
        })
    }

    /// Extra per-hit columns, in order: the screen-text group offset, the
    /// screen-text source digest, then a calendar hit's version, recorded time
    /// and continuation. Each is NULL where its table does not exist.
    fn hit_projection(&self) -> Result<(String, String), IndexAccessError> {
        let has_table = |name: &str| -> Result<bool, IndexAccessError> {
            self.connection
                .query_row(
                    "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name=?)",
                    [name],
                    |row| row.get(0),
                )
                .map_err(|error| self.classify(error))
        };
        let mut joins = String::new();
        let mut columns = Vec::new();
        if has_table("chunk_group_offset")? {
            joins.push_str(
                " LEFT JOIN (SELECT path, idx, offset_ms FROM chunk_group_offset) USING(path, idx)",
            );
            columns.push("offset_ms");
        } else {
            columns.push("NULL");
        }
        if has_table("screen_text_source")? {
            joins.push_str(" LEFT JOIN (SELECT path, sha256 AS screen_text_digest FROM screen_text_source) USING(path)");
            columns.push("screen_text_digest");
        } else {
            columns.push("NULL");
        }
        // Matching path and position too keeps a reused row id from borrowing
        // another hit's version.
        if has_table("calendar_hits")? {
            joins.push_str(" LEFT JOIN (SELECT rowid AS calendar_rowid, path AS calendar_path, idx AS calendar_idx, version AS calendar_version, changed_at AS calendar_changed_at, continues AS calendar_continues FROM calendar_hits) ON calendar_rowid=chunks.rowid AND calendar_path=chunks.path AND calendar_idx=chunks.idx");
            columns.extend([
                "calendar_version",
                "calendar_changed_at",
                "calendar_continues",
            ]);
        } else {
            columns.extend(["NULL", "NULL", "NULL"]);
        }
        Ok((columns.join(", "), joins))
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
        #[cfg(any(test, feature = "test-hooks"))]
        {
            if self.injected_fetch_failure {
                self.injected_fetch_failure = false;
                return Err(IndexAccessError::Unreadable {
                    path: self.path.clone(),
                    detail: "injected fetch failure".to_string(),
                });
            }
            self.fetch_hits_calls += 1;
        }
        let (extra_columns, joins) = self.hit_projection()?;
        let sql = format!(
            "SELECT content, path, day, facet, agent, stream, idx, bm25(chunks), chunks.rowid, {extra_columns} FROM chunks{joins} WHERE {} {ordering} LIMIT ? OFFSET ?",
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
                let row_id: i64 = row.get(8)?;
                let group_offset_ms: Option<i64> = row.get(9)?;
                let source_digest: Option<String> = row.get(10)?;
                let calendar = CalendarHit::from_columns(row.get(11)?, row.get(12)?, row.get(13)?);
                let agent = agent.unwrap_or_default();
                Ok(SearchHit {
                    row_id,
                    id: format!("{path}:{idx}"),
                    text: content,
                    metadata: SearchMetadata {
                        day: day.unwrap_or_default(),
                        facet: facet.unwrap_or_default(),
                        agent: agent.clone(),
                        stream: stream.unwrap_or_default(),
                        path,
                        idx,
                        group_offset_ms,
                        source_digest,
                        calendar,
                    },
                    score,
                })
            })
            .map_err(|error| self.classify(error))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|error| self.classify(error))?;
        Ok(rows)
    }

    fn fetch_day_hits(
        &mut self,
        plan: &SqlPlan,
        days: &[String],
        per_day_limit: usize,
    ) -> Result<Vec<(String, Vec<SearchHit>)>, IndexAccessError> {
        if days.is_empty() {
            return Ok(Vec::new());
        }
        #[cfg(any(test, feature = "test-hooks"))]
        {
            if self.injected_fetch_failure {
                self.injected_fetch_failure = false;
                return Err(IndexAccessError::Unreadable {
                    path: self.path.clone(),
                    detail: "injected fetch failure".to_string(),
                });
            }
            self.fetch_hits_calls += 1;
        }
        if per_day_limit == 0 {
            return Ok(days.iter().map(|day| (day.clone(), Vec::new())).collect());
        }

        let placeholders = std::iter::repeat_n("?", days.len())
            .collect::<Vec<_>>()
            .join(", ");
        let (score_expr, order_clause) = if plan.has_live_match_expression {
            (
                "bm25(chunks)",
                "ORDER BY day DESC, bm25(chunks) ASC, rowid ASC",
            )
        } else {
            ("0.0", "ORDER BY day DESC, rowid DESC")
        };
        let (extra_columns, joins) = self.hit_projection()?;
        let sql = format!(
            "SELECT content, path, day, facet, agent, stream, idx, {score_expr}, chunks.rowid, {extra_columns} FROM chunks{joins} WHERE {} AND day IN ({placeholders}) {order_clause}",
            plan.where_clause
        );
        let mut values = plan.params.clone();
        values.extend(days.iter().cloned());

        let mut statement = self
            .connection
            .prepare(&sql)
            .map_err(|error| self.classify(error))?;

        let mut day_hits_map: BTreeMap<String, Vec<SearchHit>> =
            days.iter().map(|d| (d.clone(), Vec::new())).collect();
        let mut filled_days = 0_usize;
        let total_days_needed = days.len();

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
                let row_id: i64 = row.get(8)?;
                let group_offset_ms: Option<i64> = row.get(9)?;
                let source_digest: Option<String> = row.get(10)?;
                let calendar = CalendarHit::from_columns(row.get(11)?, row.get(12)?, row.get(13)?);
                let agent = agent.unwrap_or_default();
                Ok(SearchHit {
                    row_id,
                    id: format!("{path}:{idx}"),
                    text: content,
                    metadata: SearchMetadata {
                        day: day.unwrap_or_default(),
                        facet: facet.unwrap_or_default(),
                        agent: agent.clone(),
                        stream: stream.unwrap_or_default(),
                        path,
                        idx,
                        group_offset_ms,
                        source_digest,
                        calendar,
                    },
                    score,
                })
            })
            .map_err(|error| self.classify(error))?;

        let journal_root = self.journal_root().map(Path::to_path_buf);
        let mut source_checks = BTreeMap::new();
        for hit_res in rows {
            let hit = hit_res.map_err(|error| self.classify(error))?;
            if day_hits_map
                .get(&hit.metadata.day)
                .is_none_or(|hits| hits.len() >= per_day_limit)
            {
                continue;
            }
            if solstone_core_format::content::screen_text::is_screen_text_path(&hit.metadata.path)
                && let Some(journal) = journal_root.as_deref()
                && !*source_checks
                    .entry((
                        hit.metadata.path.clone(),
                        hit.metadata.source_digest.clone(),
                    ))
                    .or_insert_with(|| {
                        solstone_core_indexer_store::screen_text_current_digest(
                            &self.connection,
                            journal,
                            &hit.metadata.path,
                        )
                        .ok()
                        .flatten()
                        .is_some_and(|digest| Some(&digest) == hit.metadata.source_digest.as_ref())
                    })
            {
                continue;
            }
            let day = &hit.metadata.day;
            if let Some(list) = day_hits_map.get_mut(day)
                && list.len() < per_day_limit
            {
                list.push(hit);
                if list.len() == per_day_limit {
                    filled_days += 1;
                    if filled_days == total_days_needed {
                        break;
                    }
                }
            }
        }

        let mut result = Vec::with_capacity(days.len());
        for day in days {
            let hits = day_hits_map.remove(day).unwrap_or_default();
            result.push((day.clone(), hits));
        }
        Ok(result)
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
        #[cfg(any(test, feature = "test-hooks"))]
        {
            if self.injected_aggregate_failure {
                self.injected_aggregate_failure = false;
                return Err(IndexAccessError::Unreadable {
                    path: self.path.clone(),
                    detail: "injected aggregate failure".to_string(),
                });
            }
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
        #[cfg(any(test, feature = "test-hooks"))]
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

/// Counters for observable query execution phases in tests.
#[cfg(any(test, feature = "test-hooks"))]
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct QueryCounters {
    pub aggregate_calls: usize,
    pub fetch_hits_calls: usize,
    pub agents_calls: usize,
}

#[cfg(test)]
pub(crate) fn search_with_connection_for_test(
    connection: Connection,
    path: PathBuf,
    request: &SearchRequest,
    reference_date: NaiveDate,
) -> Result<(SearchResponse, QueryCounters), IndexAccessError> {
    let mut connection = QueryConnection::new(connection, path)?;
    let compilation = compile_query(&request.query, reference_date);
    let response = search_on_connection(&mut connection, request, reference_date, compilation)?;
    Ok((
        response,
        QueryCounters {
            aggregate_calls: connection.aggregate_calls,
            fetch_hits_calls: connection.fetch_hits_calls,
            agents_calls: connection.agents_calls,
        },
    ))
}

#[cfg(test)]
pub(crate) fn agents_with_connection_for_test(
    connection: Connection,
    path: PathBuf,
) -> Result<(Vec<String>, QueryCounters), IndexAccessError> {
    let mut connection = QueryConnection::new(connection, path)?;
    let agents = connection.agents()?;
    Ok((
        agents,
        QueryCounters {
            aggregate_calls: connection.aggregate_calls,
            fetch_hits_calls: connection.fetch_hits_calls,
            agents_calls: connection.agents_calls,
        },
    ))
}
