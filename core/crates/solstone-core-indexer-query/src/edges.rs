// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

// Relationship-edge readers are outside the chunk-query boundary claim.

//! Read-only derived entity-edge queries over the journal index.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use chrono::{DateTime, NaiveDate, Utc};
use rusqlite::{
    Connection, Error as SqlError, OpenFlags, OptionalExtension, Row, params_from_iter,
};
use serde::Serialize;

use solstone_core_indexer::edges::KINDS;

// Rust-owned weights retained from the retired Python implementation.
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
// Rust-owned half-life retained from the retired Python implementation.
const HALF_LIFE_DAYS: f64 = 90.0;

/// The complete stable ordering for pair evidence.
pub(crate) const EVIDENCE_ORDER_SQL: &str = "ORDER BY day IS NULL ASC, day DESC,\n         ts IS NULL ASC, ts DESC,\n         path ASC, anchor IS NULL ASC, anchor ASC, rowid ASC";

/// Maximum neighbors allowed in a network query.
pub const NETWORK_NEIGHBOR_LIMIT_MAX: i64 = 100;
/// Maximum evidence rows per neighbor in a network query.
pub const NETWORK_EVIDENCE_LIMIT_MAX: i64 = 5;

/// Format the out-of-range detail for network limits.
pub fn network_bound_detail(name: &str, max: i64) -> String {
    format!("{name} must be between 0 and {max}")
}

/// A caller-provided canonical entity type lookup. `None` is an ordinary missing type.
pub type EntityTypeLookup<'a> = dyn Fn(&str) -> Option<String> + 'a;

/// Merged entity directories mapped to the live entity that absorbed them.
///
/// An edge row keeps the directory each endpoint had when its file was
/// extracted, so rows written before a merge still name the merged entity.
/// Every edge query reads rows through this map: an aliased endpoint counts
/// under its survivor and carries the survivor's name, and a merge needs no
/// re-extraction. An empty map reads the index exactly as stored.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct EdgeAliases {
    entries: BTreeMap<String, EdgeAlias>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct EdgeAlias {
    canonical: String,
    name: Option<String>,
}

impl EdgeAliases {
    /// Read rows keyed `raw` as `canonical`, labelled `name` when it is known.
    /// A mapping onto itself is ignored.
    pub fn insert(&mut self, raw: &str, canonical: &str, name: Option<String>) {
        if raw != canonical {
            self.entries.insert(
                raw.to_owned(),
                EdgeAlias {
                    canonical: canonical.to_owned(),
                    name,
                },
            );
        }
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// The id rows keyed `id` are counted under.
    pub fn canonical<'a>(&'a self, id: &'a str) -> &'a str {
        self.entries
            .get(id)
            .map_or(id, |alias| alias.canonical.as_str())
    }

    /// The current name of an entity that absorbed a merge. Such an entity is
    /// labelled with it wherever it appears, whatever older rows called it.
    fn survivor_name(&self, canonical: &str) -> Option<&str> {
        self.entries
            .values()
            .find(|alias| alias.canonical == canonical)
            .and_then(|alias| alias.name.as_deref())
    }

    /// Only the entries for stored ids in `groups`. A query binds the aliases
    /// its rows can carry, never the whole journal's.
    fn restricted_to(&self, groups: &[&[String]]) -> Self {
        let entries = groups
            .iter()
            .flat_map(|group| group.iter())
            .filter_map(|id| {
                self.entries
                    .get(id)
                    .map(|alias| (id.clone(), alias.clone()))
            })
            .collect();
        Self { entries }
    }

    /// Every stored key whose rows count under `canonical`, itself first.
    fn members(&self, canonical: &str) -> Vec<String> {
        let mut members = vec![canonical.to_owned()];
        members.extend(
            self.entries
                .iter()
                .filter(|(_, alias)| alias.canonical == canonical)
                .map(|(raw, _)| raw.clone()),
        );
        members
    }
}

/// The `WITH` head the pair queries start from: CTE `e` holds the stored
/// rows between two member groups, with `src`/`dst` the ids each row counts
/// under. Rows an alias would turn into self-edges are left out; stored
/// self-edges stay. Callers pass only the aliases of the two groups, so the
/// statement stays small however many merges the journal holds.
struct EdgeSource {
    sql: String,
    params: Vec<rusqlite::types::Value>,
}

fn borrowed(ids: &[String]) -> Vec<&str> {
    ids.iter().map(String::as_str).collect()
}

fn edge_source(aliases: &EdgeAliases, left: &[String], right: &[String]) -> EdgeSource {
    const COLUMNS: &str =
        "kind, directed, src_name, dst_name, day, facet, source, path, anchor, label, ts, weight";
    let mut params = Vec::new();
    if aliases.is_empty() {
        // No aliases: the stored rows as they are, filtered by the caller.
        return EdgeSource {
            sql: format!(
                "WITH e AS NOT MATERIALIZED (SELECT src, dst, {COLUMNS}, rowid AS rowid FROM edges)"
            ),
            params,
        };
    }
    let in_list = |ids: &[&str], params: &mut Vec<rusqlite::types::Value>| {
        params.extend(ids.iter().map(|id| text(id)));
        std::iter::repeat_n("?", ids.len())
            .collect::<Vec<_>>()
            .join(", ")
    };
    // Each endpoint maps through an inline CASE: the alias list is small, and
    // an expression keeps every row's lookup out of a join.
    let canonical = |column: &str, params: &mut Vec<rusqlite::types::Value>| {
        let mut sql = format!("CASE edges.{column}");
        for (raw, alias) in &aliases.entries {
            sql.push_str(" WHEN ? THEN ?");
            params.push(text(raw));
            params.push(text(&alias.canonical));
        }
        sql.push_str(&format!(" ELSE edges.{column} END"));
        sql
    };
    let src = canonical("src", &mut params);
    let dst = canonical("dst", &mut params);
    let columns = COLUMNS
        .split(", ")
        .map(|column| format!("edges.{column} AS {column}"))
        .collect::<Vec<_>>()
        .join(", ");
    // Only a row between two members of one alias group can become a
    // self-edge; the index-backed lists keep the mapping off every other row.
    let group: Vec<&str> = aliases
        .entries
        .iter()
        .flat_map(|(raw, alias)| [raw.as_str(), alias.canonical.as_str()])
        .collect();
    let group_src = in_list(&group, &mut params);
    let group_dst = in_list(&group, &mut params);
    let where_src = canonical("src", &mut params);
    let where_dst = canonical("dst", &mut params);
    let (left, right) = (borrowed(left), borrowed(right));
    let a = in_list(&left, &mut params);
    let b = in_list(&right, &mut params);
    let c = in_list(&right, &mut params);
    let d = in_list(&left, &mut params);
    let stored = format!(
        " AND ((edges.src IN ({a}) AND edges.dst IN ({b})) OR (edges.src IN ({c}) AND edges.dst IN ({d})))"
    );
    EdgeSource {
        sql: format!(
            "WITH e AS NOT MATERIALIZED (\n  \
             SELECT {src} AS src, {dst} AS dst, {columns}, edges.rowid AS rowid\n  \
             FROM edges\n  \
             WHERE NOT (edges.src != edges.dst AND edges.src IN ({group_src}) AND edges.dst IN ({group_dst})\n    \
             AND {where_src} = {where_dst}){stored}\n)"
        ),
        params,
    }
}

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

fn consider_neighbor(retained: &mut Vec<NetworkNeighbor>, neighbor: NetworkNeighbor, limit: usize) {
    if limit == 0 {
        return;
    }
    if retained.len() < limit {
        retained.push(neighbor);
        return;
    }
    let (worst_idx, worst) = retained
        .iter()
        .enumerate()
        .min_by(|(_, a), (_, b)| {
            a.score
                .total_cmp(&b.score)
                .then_with(|| b.entity_id.cmp(&a.entity_id))
        })
        .expect("retained is non-empty when limit > 0");
    if neighbor
        .score
        .total_cmp(&worst.score)
        .then_with(|| worst.entity_id.cmp(&neighbor.entity_id))
        .is_gt()
    {
        retained[worst_idx] = neighbor;
    }
}

/// Load one-hop neighbors. The caller supplies principal identity and attendance policy.
pub fn load_entity_network(
    journal: &Path,
    entity_id: &str,
    request: &NetworkRequest,
    principal_id: Option<&str>,
    attendance_kinds: &[&str],
    aliases: &EdgeAliases,
) -> Result<NetworkResponse, EdgeQueryError> {
    let requested_id = entity_id;
    let entity_id = aliases.canonical(entity_id);
    if !(0..=NETWORK_NEIGHBOR_LIMIT_MAX).contains(&request.limit) {
        return Err(invalid(network_bound_detail(
            "limit",
            NETWORK_NEIGHBOR_LIMIT_MAX,
        )));
    }
    if !(0..=NETWORK_EVIDENCE_LIMIT_MAX).contains(&request.evidence_limit) {
        return Err(invalid(network_bound_detail(
            "evidence_limit",
            NETWORK_EVIDENCE_LIMIT_MAX,
        )));
    }
    let filter = build_filters(&request.filters)?;
    let zone = solstone_core_journal_config::owner_zone(journal);
    let reference_day = reference_day(request.reference_day.as_deref(), zone, Utc::now())?;
    let reference = parse_reference_day(&reference_day)?;
    let ranking = filter.with_ranking_cap(&reference_day);
    let connection = open_edges_reader(journal)?;
    let members = aliases.members(entity_id);
    let neighbor_limit = request.limit as usize;
    let mut retained = Vec::with_capacity(neighbor_limit);
    let mut in_flight: Option<NetworkNeighbor> = None;
    let mut total_neighbors = 0;

    let finish_in_flight = |in_flight: &mut Option<NetworkNeighbor>,
                            retained: &mut Vec<NetworkNeighbor>,
                            total_neighbors: &mut usize| {
        if let Some(mut neighbor) = in_flight.take() {
            *total_neighbors += 1;
            neighbor.evidence_class = evidence_class(&neighbor.kinds, attendance_kinds);
            consider_neighbor(retained, neighbor, neighbor_limit);
        }
    };

    for_each_ranking_row(&connection, aliases, &members, &ranking, |row| {
        if in_flight
            .as_ref()
            .is_some_and(|current| current.entity_id != row.peer)
        {
            finish_in_flight(&mut in_flight, &mut retained, &mut total_neighbors);
        }
        // This carries Python's three-clause rule. The subject cannot be its own
        // peer because ranking SQL already excludes it, but fidelity keeps the clause.
        if !request.include_principal
            && principal_id.is_some_and(|principal| {
                !principal.is_empty() && principal != entity_id && row.peer == principal
            })
        {
            return Ok(());
        }
        let weighted = kind_weight(&row.kind, row.weight_sum, row.day.as_deref(), reference)?;
        let neighbor = in_flight.get_or_insert_with(|| NetworkNeighbor {
            entity_id: row.peer.clone(),
            name: None,
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
        Ok(())
    })?;
    finish_in_flight(&mut in_flight, &mut retained, &mut total_neighbors);

    retained.sort_by(|a, b| {
        b.score
            .total_cmp(&a.score)
            .then_with(|| a.entity_id.cmp(&b.entity_id))
    });
    for neighbor in &mut retained {
        let peer_members = aliases.members(&neighbor.entity_id);
        let source = edge_source(
            &aliases.restricted_to(&[&members, &peer_members]),
            &members,
            &peer_members,
        );
        neighbor.name = match aliases.survivor_name(&neighbor.entity_id) {
            Some(name) => Some(name.to_owned()),
            None => load_peer_name(
                &connection,
                &source,
                entity_id,
                &neighbor.entity_id,
                &ranking,
            )?,
        };
        // Network previews use the ranking cap; pair history below intentionally does not.
        neighbor.evidence = load_evidence_rows(
            &connection,
            &source,
            entity_id,
            &neighbor.entity_id,
            &ranking,
            request.evidence_limit,
            0,
        )?;
    }
    Ok(NetworkResponse {
        entity_id: requested_id.to_string(),
        reference_day,
        filters: NetworkFilters::from((&filter, request.include_principal)),
        limit: request.limit,
        evidence_limit: request.evidence_limit,
        total_neighbors,
        neighbors: retained,
    })
}

/// Load stable newest-first evidence for one pair. History deliberately uses plain filters.
pub fn load_edge_evidence(
    journal: &Path,
    entity_id: &str,
    peer_id: &str,
    request: &EdgeEvidenceRequest,
    aliases: &EdgeAliases,
) -> Result<EdgeEvidenceResponse, EdgeQueryError> {
    let (requested_id, requested_peer) = (entity_id, peer_id);
    let entity_id = aliases.canonical(entity_id);
    let peer_id = aliases.canonical(peer_id);
    validate_nonnegative("limit", request.limit)?;
    validate_nonnegative("offset", request.offset)?;
    let filter = build_filters(&request.filters)?;
    let connection = open_edges_reader(journal)?;
    let (members, peer_members) = (aliases.members(entity_id), aliases.members(peer_id));
    let source = edge_source(
        &aliases.restricted_to(&[&members, &peer_members]),
        &members,
        &peer_members,
    );
    let pair = pair_where();
    let mut params = source.params.clone();
    params.extend(pair_params(entity_id, peer_id));
    params.extend(filter.params.clone());
    let total: i64 = connection
        .query_row(
            &format!(
                "{} SELECT COUNT(*) FROM e {pair} {}",
                source.sql, filter.sql
            ),
            params_from_iter(params.iter()),
            |row| row.get(0),
        )
        .map_err(|error| unavailable_db(&connection, error))?;
    Ok(EdgeEvidenceResponse {
        entity_id: requested_id.to_string(),
        peer_id: requested_peer.to_string(),
        peer_name: match aliases.survivor_name(peer_id) {
            Some(name) => Some(name.to_owned()),
            None => load_peer_name(&connection, &source, entity_id, peer_id, &filter)?,
        },
        filters: EdgeFiltersPayload::from(&filter),
        total,
        limit: request.limit,
        offset: request.offset,
        evidence: load_evidence_rows(
            &connection,
            &source,
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
    aliases: &EdgeAliases,
) -> Result<NetworkOverviewResponse, EdgeQueryError> {
    validate_nonnegative("limit", request.limit)?;
    let filter = build_filters(&request.filters)?;
    let zone = solstone_core_journal_config::owner_zone(journal);
    let reference_day = reference_day(request.reference_day.as_deref(), zone, Utc::now())?;
    let reference = parse_reference_day(&reference_day)?;
    let ranking = filter.with_ranking_cap(&reference_day);
    let connection = open_edges_reader(journal)?;
    // The overview reads every row, so it runs on stored ids exactly as
    // indexed, then takes out alias-made self-edges and folds merged ids.
    let source = edge_source(&EdgeAliases::default(), &[], &[]);
    let merged_self = load_merged_self_edges(&connection, aliases, &ranking)?;
    let mut params = source.params.clone();
    params.extend(ranking.params.clone());
    let total_edges: i64 = connection
        .query_row(
            &format!(
                "{} SELECT COUNT(*) FROM e WHERE 1 = 1 {}",
                source.sql, ranking.sql
            ),
            params_from_iter(params.iter()),
            |row| row.get::<_, i64>(0),
        )
        .map_err(|error| unavailable_db(&connection, error))?
        - merged_self.len() as i64;
    let mut global_kinds = BTreeMap::new();
    let sql = format!(
        "{} SELECT kind, day, COUNT(*) AS count, SUM(weight) AS weight_sum FROM e WHERE 1 = 1 {} GROUP BY kind, day",
        source.sql, ranking.sql
    );
    let mut stmt = connection
        .prepare(&sql)
        .map_err(|error| unavailable_db(&connection, error))?;
    let rows = stmt
        .query_map(params_from_iter(params.iter()), ranking_row_from_global)
        .map_err(|error| unavailable_db(&connection, error))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| unavailable_db(&connection, error))?;
    let by_kind_day = merged_self.iter().map(|edge| {
        (
            String::new(),
            edge.kind.clone(),
            edge.day.clone(),
            edge.weight,
        )
    });
    for row in subtract_rows(rows, by_kind_day) {
        accumulate_kind(
            &mut global_kinds,
            &row.kind,
            row.count,
            row.weight_sum,
            row.day.as_deref(),
            reference,
        )?;
    }
    let names = fold_endpoint_names(
        aliases,
        load_endpoint_names(&connection, &source, &ranking)?,
    );
    // Keep dst != src on this second UNION leg only: self-edges count once,
    // deliberately, retaining the retired Python query behavior.
    let cte = format!(
        "{},\nendpoint_edges AS (\n  SELECT src AS entity_id, kind, day, weight\n  FROM e\n  WHERE 1 = 1 {}\n  UNION ALL\n  SELECT dst AS entity_id, kind, day, weight\n  FROM e\n  WHERE 1 = 1\n    AND dst != src {}\n)\nSELECT entity_id, kind, day, COUNT(*) AS count, SUM(weight) AS weight_sum\nFROM endpoint_edges\nGROUP BY entity_id, kind, day",
        source.sql, ranking.sql, ranking.sql
    );
    let mut params = source.params.clone();
    params.extend(ranking.params.clone());
    params.extend(ranking.params.clone());
    let mut stmt = connection
        .prepare(&cte)
        .map_err(|error| unavailable_db(&connection, error))?;
    let rows = stmt
        .query_map(params_from_iter(params.iter()), overview_row_from_sql)
        .map_err(|error| unavailable_db(&connection, error))?;
    let rows = rows
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| unavailable_db(&connection, error))?;
    let by_endpoint = merged_self.iter().flat_map(|edge| {
        [&edge.src, &edge.dst]
            .map(|id| (id.clone(), edge.kind.clone(), edge.day.clone(), edge.weight))
    });
    let rows = subtract_rows(rows, by_endpoint);
    let mut entities = BTreeMap::<String, OverviewEntity>::new();
    for row in fold_overview_rows(aliases, rows) {
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
        // cannot make a naive bounded callback walk outside entities/.
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

struct StoredEdge {
    src: String,
    dst: String,
    kind: String,
    day: Option<String>,
    weight: i64,
}

/// Stored rows between two ids of one merged entity: an alias turns each into
/// a self-edge, which no count includes. Only rows between alias group
/// members can qualify, so the index-backed lists find them.
fn load_merged_self_edges(
    connection: &Connection,
    aliases: &EdgeAliases,
    filter: &FilterSql,
) -> Result<Vec<StoredEdge>, EdgeQueryError> {
    if aliases.is_empty() {
        return Ok(Vec::new());
    }
    // One JSON parameter carries the whole group: the statement's size does
    // not grow with the number of merges.
    let group: Vec<&str> = aliases
        .entries
        .iter()
        .flat_map(|(raw, alias)| [raw.as_str(), alias.canonical.as_str()])
        .collect();
    let group = serde_json::to_string(&group).map_err(|error| EdgeQueryError::Internal {
        detail: error.to_string(),
    })?;
    let sql = format!(
        "WITH member(id) AS (SELECT DISTINCT value FROM json_each(?))\nSELECT src, dst, kind, day, weight FROM edges WHERE src != dst AND src IN (SELECT id FROM member) AND dst IN (SELECT id FROM member) {}",
        filter.sql
    );
    let mut params = vec![rusqlite::types::Value::Text(group)];
    params.extend(filter.params.clone());
    let mut statement = connection
        .prepare(&sql)
        .map_err(|error| unavailable_db(connection, error))?;
    let rows = statement
        .query_map(params_from_iter(params.iter()), |row| {
            Ok(StoredEdge {
                src: row.get(0)?,
                dst: row.get(1)?,
                kind: row.get(2)?,
                day: row.get(3)?,
                weight: row.get(4)?,
            })
        })
        .map_err(|error| unavailable_db(connection, error))?;
    let mut merged = Vec::new();
    for row in rows {
        let row = row.map_err(|error| unavailable_db(connection, error))?;
        if aliases.canonical(&row.src) == aliases.canonical(&row.dst) {
            merged.push(row);
        }
    }
    Ok(merged)
}

/// Take rows out of grouped counts. A group left empty is dropped, as if its
/// rows had never been counted.
fn subtract_rows(
    rows: Vec<OverviewRow>,
    remove: impl Iterator<Item = (String, String, Option<String>, i64)>,
) -> Vec<OverviewRow> {
    let mut removed = BTreeMap::<(String, String, Option<String>), (i64, i64)>::new();
    for (entity_id, kind, day, weight) in remove {
        let entry = removed.entry((entity_id, kind, day)).or_insert((0, 0));
        entry.0 += 1;
        entry.1 += weight;
    }
    if removed.is_empty() {
        return rows;
    }
    rows.into_iter()
        .filter_map(|mut row| {
            let key = (row.entity_id.clone(), row.kind.clone(), row.day.clone());
            if let Some((count, weight)) = removed.get(&key) {
                row.count -= count;
                row.weight_sum -= weight;
            }
            (row.count > 0).then_some(row)
        })
        .collect()
}

/// Count each stored id's rows under the entity they belong to. Counts and
/// weights are summed before scoring, as grouping by that entity would.
fn fold_overview_rows(aliases: &EdgeAliases, rows: Vec<OverviewRow>) -> Vec<OverviewRow> {
    if aliases.is_empty() {
        return rows;
    }
    let mut folded = BTreeMap::<(String, String, Option<String>), (i64, i64)>::new();
    for row in rows {
        let key = (
            aliases.canonical(&row.entity_id).to_owned(),
            row.kind,
            row.day,
        );
        let entry = folded.entry(key).or_insert((0, 0));
        entry.0 += row.count;
        entry.1 += row.weight_sum;
    }
    folded
        .into_iter()
        .map(
            |((entity_id, kind, day), (count, weight_sum))| OverviewRow {
                entity_id,
                kind,
                day,
                count,
                weight_sum,
            },
        )
        .collect()
}

/// Names by the entity each stored id belongs to: an entity that absorbed a
/// merge takes its current name; otherwise its own newest name wins, then a
/// merged id's.
fn fold_endpoint_names(
    aliases: &EdgeAliases,
    stored: BTreeMap<String, String>,
) -> BTreeMap<String, String> {
    if aliases.is_empty() {
        return stored;
    }
    let mut names = BTreeMap::new();
    for (id, name) in &stored {
        if aliases.canonical(id) == id {
            names.insert(id.clone(), name.clone());
        }
    }
    for (id, name) in stored {
        let canonical = aliases.canonical(&id);
        if canonical != id {
            names.entry(canonical.to_owned()).or_insert(name);
        }
    }
    for alias in aliases.entries.values() {
        if let Some(name) = &alias.name {
            names.insert(alias.canonical.clone(), name.clone());
        }
    }
    names
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
fn reference_day(
    input: Option<&str>,
    zone: chrono_tz::Tz,
    now: DateTime<Utc>,
) -> Result<String, EdgeQueryError> {
    let day = input
        .map(ToOwned::to_owned)
        .unwrap_or_else(|| now.with_timezone(&zone).format("%Y%m%d").to_string());
    parse_reference_day(&day)?;
    Ok(day)
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
pub fn is_safe_entity_id_component(entity_id: &str) -> bool {
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
    // unavailable; decoding, conversion, and binding failures remain Internal for this callback contract.
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
/// Visit ranking rows grouped by peer, peers in order. The query reads
/// stored rows touching the subject's members and leaves out rows between
/// two of them (stored self-edges and alias-made ones alike). A merged peer
/// is then counted under its survivor, so the statement binds only the
/// subject's own members however many merges the journal holds.
fn for_each_ranking_row(
    connection: &Connection,
    aliases: &EdgeAliases,
    members: &[String],
    filter: &FilterSql,
    mut visit: impl FnMut(RankingRow) -> Result<(), EdgeQueryError>,
) -> Result<(), EdgeQueryError> {
    let slots = std::iter::repeat_n("?", members.len())
        .collect::<Vec<_>>()
        .join(", ");
    // Peer rows must be contiguous because SQLite GROUP BY does not promise order.
    let sql = format!(
        "SELECT\n  CASE WHEN src IN ({slots}) THEN dst ELSE src END AS peer,\n  kind, day, COUNT(*) AS count, SUM(weight) AS weight_sum,\n  SUM(CASE WHEN directed = 1 AND src IN ({slots}) THEN 1 ELSE 0 END) AS directed_out,\n  SUM(CASE WHEN directed = 1 AND dst IN ({slots}) THEN 1 ELSE 0 END) AS directed_in\nFROM edges\nWHERE (src IN ({slots}) OR dst IN ({slots}))\n  AND NOT (src IN ({slots}) AND dst IN ({slots})) {}\nGROUP BY peer, kind, day\nORDER BY peer",
        filter.sql
    );
    let mut params = Vec::with_capacity(members.len() * 7 + filter.params.len());
    for _ in 0..7 {
        params.extend(members.iter().map(|member| text(member)));
    }
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
    if aliases.is_empty() {
        for row in rows {
            visit(row.map_err(|error| unavailable_db(connection, error))?)?;
        }
        return Ok(());
    }
    let mut folded = BTreeMap::<(String, String, Option<String>), RankingRow>::new();
    for row in rows {
        let row = row.map_err(|error| unavailable_db(connection, error))?;
        let peer = aliases.canonical(&row.peer).to_owned();
        match folded.entry((peer.clone(), row.kind.clone(), row.day.clone())) {
            std::collections::btree_map::Entry::Occupied(mut entry) => {
                let total = entry.get_mut();
                total.count += row.count;
                total.weight_sum += row.weight_sum;
                total.directed_out += row.directed_out;
                total.directed_in += row.directed_in;
            }
            std::collections::btree_map::Entry::Vacant(entry) => {
                entry.insert(RankingRow { peer, ..row });
            }
        }
    }
    for row in folded.into_values() {
        visit(row)?;
    }
    Ok(())
}
fn load_evidence_rows(
    connection: &Connection,
    source: &EdgeSource,
    entity_id: &str,
    peer_id: &str,
    filter: &FilterSql,
    limit: i64,
    offset: i64,
) -> Result<Vec<EvidenceRow>, EdgeQueryError> {
    let sql = format!(
        "{} SELECT src, dst, kind, directed, src_name, dst_name, day, facet, source, path, anchor, label, ts, weight FROM e {} {}\n{}\nLIMIT ? OFFSET ?",
        source.sql,
        pair_where(),
        filter.sql,
        EVIDENCE_ORDER_SQL
    );
    let mut params = source.params.clone();
    params.extend(pair_params(entity_id, peer_id));
    params.extend(filter.params.clone());
    params.push(limit.into());
    params.push(offset.into());
    let mut statement = connection
        .prepare_cached(&sql)
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
    source: &EdgeSource,
    entity_id: &str,
    peer_id: &str,
    filter: &FilterSql,
) -> Result<Option<String>, EdgeQueryError> {
    let sql = format!(
        "{} SELECT CASE WHEN src = ? THEN dst_name ELSE src_name END AS peer_name FROM e {} {}\n  AND (CASE WHEN src = ? THEN dst_name ELSE src_name END) IS NOT NULL\n{}\nLIMIT 1",
        source.sql,
        pair_where(),
        filter.sql,
        EVIDENCE_ORDER_SQL
    );
    let mut params = source.params.clone();
    params.push(text(entity_id));
    params.extend(pair_params(entity_id, peer_id));
    params.extend(filter.params.clone());
    params.push(text(entity_id));
    connection
        .prepare_cached(&sql)
        .and_then(|mut statement| {
            statement
                .query_row(params_from_iter(params.iter()), |row| row.get(0))
                .optional()
        })
        .map_err(|error| unavailable_db(connection, error))
}
fn load_endpoint_names(
    connection: &Connection,
    source: &EdgeSource,
    filter: &FilterSql,
) -> Result<BTreeMap<String, String>, EdgeQueryError> {
    // Keep dst != src on this second UNION leg only: self-edges count once,
    // deliberately, retaining the retired Python query behavior.
    let sql = format!(
        "{}, endpoint_edges AS ( SELECT src AS entity_id, src_name AS entity_name, day, ts, path, anchor, rowid AS edge_rowid FROM e WHERE 1 = 1 {} UNION ALL SELECT dst AS entity_id, dst_name AS entity_name, day, ts, path, anchor, rowid AS edge_rowid FROM e WHERE 1 = 1 AND dst != src {} ) SELECT entity_id, entity_name FROM endpoint_edges WHERE entity_name IS NOT NULL ORDER BY entity_id ASC, day IS NULL ASC, day DESC, ts IS NULL ASC, ts DESC, path ASC, anchor IS NULL ASC, anchor ASC, edge_rowid ASC",
        source.sql, filter.sql, filter.sql
    );
    let mut params = source.params.clone();
    params.extend(filter.params.clone());
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
