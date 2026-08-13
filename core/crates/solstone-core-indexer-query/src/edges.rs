// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Read-only derived entity-edge queries over the journal index.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use chrono::{Local, NaiveDate};
use rusqlite::{
    Connection, Error as SqlError, OpenFlags, OptionalExtension, Row, params_from_iter,
};
use serde::Serialize;

use solstone_core_indexer::edges::KINDS;

// Source of truth: solstone/think/indexer/edges.py:69-87.
const KIND_WEIGHTS: &[(&str, f64)] = &[
    ("committed-to", 5.0),
    ("works-with", 4.0),
    ("works-at", 4.0),
    ("reports-to", 4.0),
    ("family-of", 4.0),
    ("knows", 4.0),
    ("uses", 4.0),
    ("created", 4.0),
    ("other", 4.0),
    ("decided-with", 4.0),
    ("spoke-with", 4.0),
    ("mentioned", 3.0),
    ("attended-with", 3.0),
    ("messaged-with", 3.0),
    ("party-of", 3.0),
    ("scheduled-with", 2.0),
    ("co-present", 1.0),
];
// Source of truth: solstone/think/indexer/edges.py:91.
const HALF_LIFE_DAYS: f64 = 90.0;

/// The complete stable ordering for pair evidence.
pub(crate) const EVIDENCE_ORDER_SQL: &str = "ORDER BY day IS NULL ASC, day DESC,\n         ts IS NULL ASC, ts DESC,\n         path ASC, anchor IS NULL ASC, anchor ASC, rowid ASC";

/// A caller-provided canonical entity type lookup. `None` is an ordinary missing type.
pub type EntityTypeLookup<'a> = dyn Fn(&str) -> Option<String> + 'a;

/// Common requested edge filters.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct EdgeFilters {
    pub kinds: Option<Vec<String>>,
    pub facet: Option<String>,
    pub day_from: Option<String>,
    pub day_to: Option<String>,
}

/// Network query options.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NetworkRequest {
    pub filters: EdgeFilters,
    pub include_principal: bool,
    pub limit: i64,
    pub evidence_limit: i64,
    pub reference_day: Option<String>,
}

impl Default for NetworkRequest {
    fn default() -> Self {
        Self {
            filters: EdgeFilters::default(),
            include_principal: false,
            limit: 25,
            evidence_limit: 5,
            reference_day: None,
        }
    }
}

/// Pair-evidence query options.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EdgeEvidenceRequest {
    pub filters: EdgeFilters,
    pub limit: i64,
    pub offset: i64,
}

impl Default for EdgeEvidenceRequest {
    fn default() -> Self {
        Self {
            filters: EdgeFilters::default(),
            limit: 50,
            offset: 0,
        }
    }
}

/// Overview query options.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NetworkOverviewRequest {
    pub filters: EdgeFilters,
    pub limit: i64,
    pub reference_day: Option<String>,
}

impl Default for NetworkOverviewRequest {
    fn default() -> Self {
        Self {
            filters: EdgeFilters::default(),
            limit: 25,
            reference_day: None,
        }
    }
}

/// A usable edge index could not be read, a request was invalid, or stored data was corrupt.
#[derive(Debug)]
pub enum EdgeQueryError {
    EdgeIndexUnavailable { path: PathBuf, detail: String },
    InvalidRequestValue { detail: String },
    Internal { detail: String },
}

impl std::fmt::Display for EdgeQueryError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::EdgeIndexUnavailable { path, detail } => {
                write!(f, "edge index unavailable ({}): {detail}", path.display())
            }
            Self::InvalidRequestValue { detail } | Self::Internal { detail } => f.write_str(detail),
        }
    }
}
impl std::error::Error for EdgeQueryError {}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct EdgeFiltersPayload {
    pub kinds: Option<Vec<String>>,
    pub facet: Option<String>,
    pub day_from: Option<String>,
    pub day_to: Option<String>,
}
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct NetworkFilters {
    pub kinds: Option<Vec<String>>,
    pub facet: Option<String>,
    pub day_from: Option<String>,
    pub day_to: Option<String>,
    pub include_principal: bool,
}
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct KindSummary {
    pub count: i64,
    pub weighted: f64,
}
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct DirectedCounts {
    pub out: i64,
    pub r#in: i64,
}
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct EvidenceRow {
    pub src: String,
    pub dst: String,
    pub kind: String,
    pub directed: bool,
    pub src_name: Option<String>,
    pub dst_name: Option<String>,
    pub day: Option<String>,
    pub facet: Option<String>,
    pub source: String,
    pub path: String,
    pub anchor: Option<String>,
    pub label: Option<String>,
    pub ts: Option<i64>,
    pub weight: i64,
}
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct NetworkNeighbor {
    pub entity_id: String,
    pub name: Option<String>,
    pub score: f64,
    pub count: i64,
    pub first_seen: Option<String>,
    pub last_seen: Option<String>,
    pub directed: DirectedCounts,
    pub kinds: BTreeMap<String, KindSummary>,
    pub evidence: Vec<EvidenceRow>,
    pub evidence_class: String,
}
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct NetworkResponse {
    pub entity_id: String,
    pub reference_day: String,
    pub filters: NetworkFilters,
    pub limit: i64,
    pub evidence_limit: i64,
    pub total_neighbors: usize,
    pub neighbors: Vec<NetworkNeighbor>,
}
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct EdgeEvidenceResponse {
    pub entity_id: String,
    pub peer_id: String,
    pub peer_name: Option<String>,
    pub filters: EdgeFiltersPayload,
    pub total: i64,
    pub limit: i64,
    pub offset: i64,
    pub evidence: Vec<EvidenceRow>,
}
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct OverviewTotals {
    pub edges: i64,
    pub entities: usize,
}
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct OverviewEntity {
    pub entity_id: String,
    pub name: Option<String>,
    pub r#type: Option<String>,
    pub score: f64,
    pub count: i64,
    pub first_seen: Option<String>,
    pub last_seen: Option<String>,
    pub kinds: BTreeMap<String, KindSummary>,
    pub evidence_class: String,
}
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct NetworkOverviewResponse {
    pub reference_day: String,
    pub filters: EdgeFiltersPayload,
    pub limit: i64,
    pub totals: OverviewTotals,
    pub kinds: BTreeMap<String, KindSummary>,
    pub entities: Vec<OverviewEntity>,
}

/// Open the edge index without creating, migrating, or otherwise mutating it.
pub fn open_edges_reader(journal: &Path) -> Result<Connection, EdgeQueryError> {
    let path = solstone_core_indexer_store::db::db_path(journal);
    if !path.is_file() {
        return Err(EdgeQueryError::EdgeIndexUnavailable {
            path,
            detail: "edge index database is absent".to_string(),
        });
    }
    let connection = Connection::open_with_flags(&path, OpenFlags::SQLITE_OPEN_READ_ONLY)
        .map_err(|error| unavailable(path.clone(), error))?;
    connection
        .execute_batch("PRAGMA query_only=ON;")
        .map_err(|error| unavailable(path.clone(), error))?;
    Ok(connection)
}

/// Load one-hop neighbors. The caller supplies principal identity and attendance policy.
pub fn load_entity_network(
    journal: &Path,
    entity_id: &str,
    request: &NetworkRequest,
    principal_id: Option<&str>,
    attendance_kinds: &[&str],
) -> Result<NetworkResponse, EdgeQueryError> {
    validate_nonnegative("limit", request.limit)?;
    validate_nonnegative("evidence_limit", request.evidence_limit)?;
    let filter = build_filters(&request.filters)?;
    let reference_day = reference_day(request.reference_day.as_deref())?;
    let reference = parse_reference_day(&reference_day)?;
    let ranking = filter.with_ranking_cap(&reference_day);
    let connection = open_edges_reader(journal)?;
    let rows = ranking_rows(&connection, entity_id, &ranking)?;
    let names = load_peer_names(&connection, entity_id, &ranking)?;
    let mut neighbors = BTreeMap::<String, NetworkNeighbor>::new();
    for row in rows {
        // This carries Python's three-clause rule. The subject cannot be its own
        // peer because ranking SQL already excludes it, but fidelity keeps the clause.
        if !request.include_principal
            && principal_id.is_some_and(|principal| {
                !principal.is_empty() && principal != entity_id && row.peer == principal
            })
        {
            continue;
        }
        let weighted = kind_weight(&row.kind, row.weight_sum, row.day.as_deref(), reference)?;
        let neighbor = neighbors
            .entry(row.peer.clone())
            .or_insert_with(|| NetworkNeighbor {
                entity_id: row.peer.clone(),
                name: names.get(&row.peer).cloned(),
                score: 0.0,
                count: 0,
                first_seen: None,
                last_seen: None,
                directed: DirectedCounts { out: 0, r#in: 0 },
                kinds: BTreeMap::new(),
                evidence: Vec::new(),
                evidence_class: String::new(),
            });
        let kind = neighbor.kinds.entry(row.kind).or_insert(KindSummary {
            count: 0,
            weighted: 0.0,
        });
        kind.count += row.count;
        kind.weighted += weighted;
        neighbor.count += row.count;
        neighbor.score += weighted;
        neighbor.directed.out += row.directed_out;
        neighbor.directed.r#in += row.directed_in;
        update_seen(&mut neighbor.first_seen, &mut neighbor.last_seen, row.day);
    }
    let mut ordered: Vec<_> = neighbors.into_values().collect();
    ordered.sort_by(|a, b| {
        b.score
            .total_cmp(&a.score)
            .then_with(|| a.entity_id.cmp(&b.entity_id))
    });
    for neighbor in &mut ordered {
        neighbor.evidence_class = evidence_class(&neighbor.kinds, attendance_kinds);
    }
    let total_neighbors = ordered.len();
    ordered.truncate(request.limit as usize);
    for neighbor in &mut ordered {
        // Network previews use the ranking cap; pair history below intentionally does not.
        neighbor.evidence = load_evidence_rows(
            &connection,
            entity_id,
            &neighbor.entity_id,
            &ranking,
            request.evidence_limit,
            0,
        )?;
    }
    Ok(NetworkResponse {
        entity_id: entity_id.to_string(),
        reference_day,
        filters: NetworkFilters::from((&filter, request.include_principal)),
        limit: request.limit,
        evidence_limit: request.evidence_limit,
        total_neighbors,
        neighbors: ordered,
    })
}

/// Load stable newest-first evidence for one pair. History deliberately uses plain filters.
pub fn load_edge_evidence(
    journal: &Path,
    entity_id: &str,
    peer_id: &str,
    request: &EdgeEvidenceRequest,
) -> Result<EdgeEvidenceResponse, EdgeQueryError> {
    validate_nonnegative("limit", request.limit)?;
    validate_nonnegative("offset", request.offset)?;
    let filter = build_filters(&request.filters)?;
    let connection = open_edges_reader(journal)?;
    let pair = pair_where();
    let mut params = pair_params(entity_id, peer_id);
    params.extend(filter.params.clone());
    let total: i64 = connection
        .query_row(
            &format!("SELECT COUNT(*) FROM edges {pair} {}", filter.sql),
            params_from_iter(params.iter()),
            |row| row.get(0),
        )
        .map_err(|error| unavailable_db(&connection, error))?;
    Ok(EdgeEvidenceResponse {
        entity_id: entity_id.to_string(),
        peer_id: peer_id.to_string(),
        peer_name: load_peer_name(&connection, entity_id, peer_id, &filter)?,
        filters: EdgeFiltersPayload::from(&filter),
        total,
        limit: request.limit,
        offset: request.offset,
        evidence: load_evidence_rows(
            &connection,
            entity_id,
            peer_id,
            &filter,
            request.limit,
            request.offset,
        )?,
    })
}

/// Load a global ranked edge overview. Type lookup is best-effort and only called for safe IDs.
pub fn load_network_overview(
    journal: &Path,
    request: &NetworkOverviewRequest,
    attendance_kinds: &[&str],
    entity_type_lookup: &EntityTypeLookup<'_>,
) -> Result<NetworkOverviewResponse, EdgeQueryError> {
    validate_nonnegative("limit", request.limit)?;
    let filter = build_filters(&request.filters)?;
    let reference_day = reference_day(request.reference_day.as_deref())?;
    let reference = parse_reference_day(&reference_day)?;
    let ranking = filter.with_ranking_cap(&reference_day);
    let connection = open_edges_reader(journal)?;
    let total_edges: i64 = connection
        .query_row(
            &format!("SELECT COUNT(*) FROM edges WHERE 1 = 1 {}", ranking.sql),
            params_from_iter(ranking.params.iter()),
            |row| row.get(0),
        )
        .map_err(|error| unavailable_db(&connection, error))?;
    let mut global_kinds = BTreeMap::new();
    let sql = format!(
        "SELECT kind, day, COUNT(*) AS count, SUM(weight) AS weight_sum FROM edges WHERE 1 = 1 {} GROUP BY kind, day",
        ranking.sql
    );
    let mut stmt = connection
        .prepare(&sql)
        .map_err(|error| unavailable_db(&connection, error))?;
    let rows = stmt
        .query_map(
            params_from_iter(ranking.params.iter()),
            ranking_row_from_global,
        )
        .map_err(|error| unavailable_db(&connection, error))?;
    for row in rows {
        let row = row.map_err(|error| unavailable_db(&connection, error))?;
        accumulate_kind(
            &mut global_kinds,
            &row.kind,
            row.count,
            row.weight_sum,
            row.day.as_deref(),
            reference,
        )?;
    }
    let names = load_endpoint_names(&connection, &ranking)?;
    // Keep dst != src on this second UNION leg only: self-edges count once, deliberately
    // (solstone/think/indexer/edges.py:925-930).
    let cte = format!(
        "WITH endpoint_edges AS (\n  SELECT src AS entity_id, kind, day, weight\n  FROM edges\n  WHERE 1 = 1 {}\n  UNION ALL\n  SELECT dst AS entity_id, kind, day, weight\n  FROM edges\n  WHERE 1 = 1\n    AND dst != src {}\n)\nSELECT entity_id, kind, day, COUNT(*) AS count, SUM(weight) AS weight_sum\nFROM endpoint_edges\nGROUP BY entity_id, kind, day",
        ranking.sql, ranking.sql
    );
    let mut params = ranking.params.clone();
    params.extend(ranking.params.clone());
    let mut stmt = connection
        .prepare(&cte)
        .map_err(|error| unavailable_db(&connection, error))?;
    let rows = stmt
        .query_map(params_from_iter(params.iter()), overview_row_from_sql)
        .map_err(|error| unavailable_db(&connection, error))?;
    let mut entities = BTreeMap::<String, OverviewEntity>::new();
    for row in rows {
        let row = row.map_err(|error| unavailable_db(&connection, error))?;
        let entity = entities
            .entry(row.entity_id.clone())
            .or_insert_with(|| OverviewEntity {
                entity_id: row.entity_id.clone(),
                name: names.get(&row.entity_id).cloned(),
                r#type: None,
                score: 0.0,
                count: 0,
                first_seen: None,
                last_seen: None,
                kinds: BTreeMap::new(),
                evidence_class: String::new(),
            });
        let weighted = kind_weight(&row.kind, row.weight_sum, row.day.as_deref(), reference)?;
        let kind = entity.kinds.entry(row.kind).or_insert(KindSummary {
            count: 0,
            weighted: 0.0,
        });
        kind.count += row.count;
        kind.weighted += weighted;
        entity.count += row.count;
        entity.score += weighted;
        update_seen(&mut entity.first_seen, &mut entity.last_seen, row.day);
    }
    let mut ordered: Vec<_> = entities.into_values().collect();
    for entity in &mut ordered {
        entity.evidence_class = evidence_class(&entity.kinds, attendance_kinds);
    }
    ordered.sort_by(|a, b| {
        b.score
            .total_cmp(&a.score)
            .then_with(|| a.entity_id.cmp(&b.entity_id))
    });
    let entity_total = ordered.len();
    ordered.truncate(request.limit as usize);
    for entity in &mut ordered {
        // Entity-file reading moved to the caller; this guard remains so hostile stored IDs
        // cannot make a naive W2b-2 callback walk outside entities/.
        if is_safe_entity_id_component(&entity.entity_id) {
            entity.r#type = entity_type_lookup(&entity.entity_id);
        }
    }
    Ok(NetworkOverviewResponse {
        reference_day,
        filters: EdgeFiltersPayload::from(&filter),
        limit: request.limit,
        totals: OverviewTotals {
            edges: total_edges,
            entities: entity_total,
        },
        kinds: global_kinds,
        entities: ordered,
    })
}

#[derive(Clone)]
struct FilterSql {
    sql: String,
    params: Vec<rusqlite::types::Value>,
    payload: EdgeFiltersPayload,
}
impl From<&FilterSql> for EdgeFiltersPayload {
    fn from(value: &FilterSql) -> Self {
        value.payload.clone()
    }
}
impl From<(&FilterSql, bool)> for NetworkFilters {
    fn from((value, include_principal): (&FilterSql, bool)) -> Self {
        Self {
            kinds: value.payload.kinds.clone(),
            facet: value.payload.facet.clone(),
            day_from: value.payload.day_from.clone(),
            day_to: value.payload.day_to.clone(),
            include_principal,
        }
    }
}
impl FilterSql {
    fn with_ranking_cap(&self, reference_day: &str) -> Self {
        let mut params = self.params.clone();
        params.push(text(reference_day));
        Self {
            // NULL days are undated evidence, not future evidence; a bare day <= :ref
            // would silently drop every undated edge (edges.py:342-346).
            sql: format!("{}\n  AND (day IS NULL OR day <= ?)", self.sql),
            params,
            payload: self.payload.clone(),
        }
    }
}

fn build_filters(filters: &EdgeFilters) -> Result<FilterSql, EdgeQueryError> {
    let mut clauses = Vec::new();
    let mut params = Vec::new();
    let kinds = filters.kinds.clone();
    if let Some(kinds) = &kinds {
        if kinds.is_empty() {
            // HTTP normalizes empty kind queries to None; only direct library callers reach this.
            clauses.push("0 = 1".to_string());
        } else {
            for kind in kinds {
                if !KINDS.contains(&kind.as_str()) {
                    return Err(invalid(format!("Unknown edge kind: {kind:?}")));
                }
            }
            clauses.push(format!(
                "kind IN ({})",
                std::iter::repeat_n("?", kinds.len())
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
            params.extend(kinds.iter().cloned().map(Into::into));
        }
    }
    // HTTP sends None for an empty facet (routes.py:292-333); direct callers retain
    // Some(\"\") to match edges.py:290-318.
    let facet = filters.facet.as_ref().map(|value| value.to_lowercase());
    if let Some(value) = &facet {
        clauses.push("facet = ?".to_string());
        params.push(value.clone().into());
    }
    if let Some(value) = &filters.day_from {
        clauses.push("day >= ?".to_string());
        params.push(value.clone().into());
    }
    if let Some(value) = &filters.day_to {
        clauses.push("day <= ?".to_string());
        params.push(value.clone().into());
    }
    Ok(FilterSql {
        sql: clauses
            .into_iter()
            .map(|clause| format!("\n  AND {clause}"))
            .collect(),
        params,
        payload: EdgeFiltersPayload {
            kinds,
            facet,
            day_from: filters.day_from.clone(),
            day_to: filters.day_to.clone(),
        },
    })
}

fn pair_where() -> &'static str {
    "WHERE ((src = ? AND dst = ?)\n   OR  (src = ? AND dst = ?))"
}
fn text(value: &str) -> rusqlite::types::Value {
    rusqlite::types::Value::Text(value.to_string())
}
fn pair_params(entity_id: &str, peer_id: &str) -> Vec<rusqlite::types::Value> {
    vec![
        text(entity_id),
        text(peer_id),
        text(peer_id),
        text(entity_id),
    ]
}
fn validate_nonnegative(name: &str, value: i64) -> Result<(), EdgeQueryError> {
    if value < 0 {
        Err(invalid(format!("{name} must be >= 0")))
    } else {
        Ok(())
    }
}
fn reference_day(input: Option<&str>) -> Result<String, EdgeQueryError> {
    let day = input.map(ToOwned::to_owned).unwrap_or_else(today_local);
    parse_reference_day(&day)?;
    Ok(day)
}
fn today_local() -> String {
    Local::now().format("%Y%m%d").to_string()
}
fn parse_reference_day(day: &str) -> Result<NaiveDate, EdgeQueryError> {
    NaiveDate::parse_from_str(day, "%Y%m%d")
        .map_err(|_| invalid(format!("Invalid edge day: {day:?}")))
}
fn parse_stored_day(day: &str) -> Result<NaiveDate, EdgeQueryError> {
    NaiveDate::parse_from_str(day, "%Y%m%d").map_err(|_| EdgeQueryError::Internal {
        detail: format!("Invalid stored edge day: {day:?}"),
    })
}
fn decay_factor(day: Option<&str>, reference: NaiveDate) -> Result<f64, EdgeQueryError> {
    let Some(day) = day else {
        return Ok(1.0);
    };
    let age = (reference - parse_stored_day(day)?).num_days().max(0);
    Ok((-(age as f64) * std::f64::consts::LN_2 / HALF_LIFE_DAYS).exp())
}
fn kind_weight(
    kind: &str,
    weight_sum: i64,
    day: Option<&str>,
    reference: NaiveDate,
) -> Result<f64, EdgeQueryError> {
    let Some((_, multiplier)) = KIND_WEIGHTS
        .iter()
        .find(|(candidate, _)| *candidate == kind)
    else {
        // The edges DDL has no CHECK on kind, so foreign/older writer rows can reach scoring.
        // Defaulting a weight would silently mis-rank connections; mirror Python's uncaught
        // KeyError in edges.py:547-550 by failing loudly as Internal.
        return Err(EdgeQueryError::Internal {
            detail: format!("Unknown stored edge kind: {kind:?}"),
        });
    };
    Ok(multiplier * weight_sum as f64 * decay_factor(day, reference)?)
}
fn evidence_class(kinds: &BTreeMap<String, KindSummary>, attendance_kinds: &[&str]) -> String {
    let attendance = kinds
        .keys()
        .any(|kind| attendance_kinds.contains(&kind.as_str()));
    let semantic = kinds
        .keys()
        .any(|kind| !attendance_kinds.contains(&kind.as_str()));
    if attendance && semantic {
        "mixed"
    } else if attendance {
        "attendance"
    } else {
        "semantic"
    }
    .to_string()
}
fn update_seen(first: &mut Option<String>, last: &mut Option<String>, day: Option<String>) {
    if let Some(day) = day {
        if first.as_ref().is_none_or(|value| &day < value) {
            *first = Some(day.clone());
        }
        if last.as_ref().is_none_or(|value| &day > value) {
            *last = Some(day);
        }
    }
}
fn is_safe_entity_id_component(entity_id: &str) -> bool {
    !matches!(entity_id, "" | "." | "..") && !entity_id.contains(['/', '\\', ':', '\0'])
}
fn invalid(detail: String) -> EdgeQueryError {
    EdgeQueryError::InvalidRequestValue { detail }
}
fn unavailable(path: PathBuf, error: SqlError) -> EdgeQueryError {
    EdgeQueryError::EdgeIndexUnavailable {
        path,
        detail: error.to_string(),
    }
}
fn unavailable_db(connection: &Connection, error: SqlError) -> EdgeQueryError {
    // Mirror routes.py:394-402: SQLite operational/schema failures mean the index is
    // unavailable; decoding, conversion, and binding failures remain Internal for W2b-2.
    match error {
        error @ SqlError::SqliteFailure(_, _) => unavailable(
            connection
                .path()
                .map(PathBuf::from)
                .unwrap_or_else(|| PathBuf::from("<edge-index>")),
            error,
        ),
        error => EdgeQueryError::Internal {
            detail: error.to_string(),
        },
    }
}

#[derive(Clone)]
struct RankingRow {
    peer: String,
    kind: String,
    day: Option<String>,
    count: i64,
    weight_sum: i64,
    directed_out: i64,
    directed_in: i64,
}
#[derive(Clone)]
struct OverviewRow {
    entity_id: String,
    kind: String,
    day: Option<String>,
    count: i64,
    weight_sum: i64,
}
fn ranking_rows(
    connection: &Connection,
    entity_id: &str,
    filter: &FilterSql,
) -> Result<Vec<RankingRow>, EdgeQueryError> {
    let sql = format!(
        "SELECT\n  CASE WHEN src = ? THEN dst ELSE src END AS peer,\n  kind, day, COUNT(*) AS count, SUM(weight) AS weight_sum,\n  SUM(CASE WHEN directed = 1 AND src = ? THEN 1 ELSE 0 END) AS directed_out,\n  SUM(CASE WHEN directed = 1 AND dst = ? THEN 1 ELSE 0 END) AS directed_in\nFROM edges\nWHERE (src = ? OR dst = ?)\n  AND (CASE WHEN src = ? THEN dst ELSE src END) != ? {}\nGROUP BY peer, kind, day",
        filter.sql
    );
    let mut params = vec![
        text(entity_id),
        text(entity_id),
        text(entity_id),
        text(entity_id),
        text(entity_id),
        text(entity_id),
        text(entity_id),
    ];
    params.extend(filter.params.clone());
    let mut statement = connection
        .prepare(&sql)
        .map_err(|error| unavailable_db(connection, error))?;
    let rows = statement
        .query_map(params_from_iter(params.iter()), |row| {
            Ok(RankingRow {
                peer: row.get(0)?,
                kind: row.get(1)?,
                day: row.get(2)?,
                count: row.get(3)?,
                weight_sum: row.get(4)?,
                directed_out: row.get(5)?,
                directed_in: row.get(6)?,
            })
        })
        .map_err(|error| unavailable_db(connection, error))?;
    rows.collect::<Result<_, _>>()
        .map_err(|error| unavailable_db(connection, error))
}
fn load_evidence_rows(
    connection: &Connection,
    entity_id: &str,
    peer_id: &str,
    filter: &FilterSql,
    limit: i64,
    offset: i64,
) -> Result<Vec<EvidenceRow>, EdgeQueryError> {
    let sql = format!(
        "SELECT src, dst, kind, directed, src_name, dst_name, day, facet, source, path, anchor, label, ts, weight FROM edges {} {}\n{}\nLIMIT ? OFFSET ?",
        pair_where(),
        filter.sql,
        EVIDENCE_ORDER_SQL
    );
    let mut params = pair_params(entity_id, peer_id);
    params.extend(filter.params.clone());
    params.push(limit.into());
    params.push(offset.into());
    let mut statement = connection
        .prepare(&sql)
        .map_err(|error| unavailable_db(connection, error))?;
    let rows = statement
        .query_map(params_from_iter(params.iter()), evidence_row_from_sql)
        .map_err(|error| unavailable_db(connection, error))?;
    rows.collect::<Result<_, _>>()
        .map_err(|error| unavailable_db(connection, error))
}
fn evidence_row_from_sql(row: &Row<'_>) -> rusqlite::Result<EvidenceRow> {
    Ok(EvidenceRow {
        src: row.get(0)?,
        dst: row.get(1)?,
        kind: row.get(2)?,
        directed: row.get::<_, i64>(3)? != 0,
        src_name: row.get(4)?,
        dst_name: row.get(5)?,
        day: row.get(6)?,
        facet: row.get(7)?,
        source: row.get(8)?,
        path: row.get(9)?,
        anchor: row.get(10)?,
        label: row.get(11)?,
        ts: row.get(12)?,
        weight: row.get(13)?,
    })
}
fn load_peer_name(
    connection: &Connection,
    entity_id: &str,
    peer_id: &str,
    filter: &FilterSql,
) -> Result<Option<String>, EdgeQueryError> {
    let sql = format!(
        "SELECT CASE WHEN src = ? THEN dst_name ELSE src_name END AS peer_name FROM edges {} {}\n  AND (CASE WHEN src = ? THEN dst_name ELSE src_name END) IS NOT NULL\n{}\nLIMIT 1",
        pair_where(),
        filter.sql,
        EVIDENCE_ORDER_SQL
    );
    let mut params = vec![text(entity_id)];
    params.extend(pair_params(entity_id, peer_id));
    params.extend(filter.params.clone());
    params.push(text(entity_id));
    connection
        .query_row(&sql, params_from_iter(params.iter()), |row| row.get(0))
        .optional()
        .map_err(|error| unavailable_db(connection, error))
}
fn load_peer_names(
    connection: &Connection,
    entity_id: &str,
    filter: &FilterSql,
) -> Result<BTreeMap<String, String>, EdgeQueryError> {
    let sql = format!(
        "SELECT CASE WHEN src = ? THEN dst ELSE src END AS peer, CASE WHEN src = ? THEN dst_name ELSE src_name END AS peer_name, day, ts, path, anchor, rowid FROM edges WHERE (src = ? OR dst = ?) AND (CASE WHEN src = ? THEN dst ELSE src END) != ? {}\n  AND (CASE WHEN src = ? THEN dst_name ELSE src_name END) IS NOT NULL\nORDER BY peer ASC, day IS NULL ASC, day DESC, ts IS NULL ASC, ts DESC, path ASC, anchor IS NULL ASC, anchor ASC, rowid ASC",
        filter.sql
    );
    let mut params = vec![
        text(entity_id),
        text(entity_id),
        text(entity_id),
        text(entity_id),
        text(entity_id),
        text(entity_id),
    ];
    params.extend(filter.params.clone());
    params.push(text(entity_id));
    let mut statement = connection
        .prepare(&sql)
        .map_err(|error| unavailable_db(connection, error))?;
    let rows = statement
        .query_map(params_from_iter(params.iter()), |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })
        .map_err(|error| unavailable_db(connection, error))?;
    let mut names = BTreeMap::new();
    for row in rows {
        let (id, name) = row.map_err(|error| unavailable_db(connection, error))?;
        names.entry(id).or_insert(name);
    }
    Ok(names)
}
fn load_endpoint_names(
    connection: &Connection,
    filter: &FilterSql,
) -> Result<BTreeMap<String, String>, EdgeQueryError> {
    // Keep dst != src on this second UNION leg only: self-edges count once, deliberately
    // (solstone/think/indexer/edges.py:522-528).
    let sql = format!(
        "WITH endpoint_edges AS ( SELECT src AS entity_id, src_name AS entity_name, day, ts, path, anchor, rowid AS edge_rowid FROM edges WHERE 1 = 1 {} UNION ALL SELECT dst AS entity_id, dst_name AS entity_name, day, ts, path, anchor, rowid AS edge_rowid FROM edges WHERE 1 = 1 AND dst != src {} ) SELECT entity_id, entity_name FROM endpoint_edges WHERE entity_name IS NOT NULL ORDER BY entity_id ASC, day IS NULL ASC, day DESC, ts IS NULL ASC, ts DESC, path ASC, anchor IS NULL ASC, anchor ASC, edge_rowid ASC",
        filter.sql, filter.sql
    );
    let mut params = filter.params.clone();
    params.extend(filter.params.clone());
    let mut statement = connection
        .prepare(&sql)
        .map_err(|error| unavailable_db(connection, error))?;
    let rows = statement
        .query_map(params_from_iter(params.iter()), |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })
        .map_err(|error| unavailable_db(connection, error))?;
    let mut names = BTreeMap::new();
    for row in rows {
        let (id, name) = row.map_err(|error| unavailable_db(connection, error))?;
        names.entry(id).or_insert(name);
    }
    Ok(names)
}
fn ranking_row_from_global(row: &Row<'_>) -> rusqlite::Result<OverviewRow> {
    Ok(OverviewRow {
        entity_id: String::new(),
        kind: row.get(0)?,
        day: row.get(1)?,
        count: row.get(2)?,
        weight_sum: row.get(3)?,
    })
}
fn overview_row_from_sql(row: &Row<'_>) -> rusqlite::Result<OverviewRow> {
    Ok(OverviewRow {
        entity_id: row.get(0)?,
        kind: row.get(1)?,
        day: row.get(2)?,
        count: row.get(3)?,
        weight_sum: row.get(4)?,
    })
}
fn accumulate_kind(
    target: &mut BTreeMap<String, KindSummary>,
    kind: &str,
    count: i64,
    weight_sum: i64,
    day: Option<&str>,
    reference: NaiveDate,
) -> Result<(), EdgeQueryError> {
    let weighted = kind_weight(kind, weight_sum, day, reference)?;
    let entry = target.entry(kind.to_string()).or_insert(KindSummary {
        count: 0,
        weighted: 0.0,
    });
    entry.count += count;
    entry.weighted += weighted;
    Ok(())
}
