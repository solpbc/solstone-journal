// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::collections::BTreeSet;
use std::fs;
use std::path::PathBuf;

use chrono::Local;
use rusqlite::{Connection, params};
use serde::Serialize;
use solstone_core_indexer_store::db::{db_path, open_index};

use crate::edges::EVIDENCE_ORDER_SQL;
use crate::test_support::reserve_temp_path;
use crate::{
    EdgeEvidenceRequest, EdgeFilters, EdgeQueryError, NETWORK_EVIDENCE_LIMIT_MAX,
    NETWORK_NEIGHBOR_LIMIT_MAX, NetworkOverviewRequest, NetworkRequest, load_edge_evidence,
    load_entity_network, load_network_overview, network_bound_detail, open_edges_reader,
};

const ATTENDANCE: &[&str] = &["attended-with", "co-present", "scheduled-with"];

#[derive(Clone)]
struct SeedEdge<'a> {
    src: &'a str,
    dst: &'a str,
    kind: &'a str,
    directed: i64,
    day: Option<&'a str>,
    src_name: Option<&'a str>,
    dst_name: Option<&'a str>,
    facet: &'a str,
    path: &'a str,
    anchor: Option<&'a str>,
    ts: Option<i64>,
    weight: i64,
}

impl<'a> SeedEdge<'a> {
    fn new(src: &'a str, dst: &'a str, kind: &'a str, day: Option<&'a str>, path: &'a str) -> Self {
        Self {
            src,
            dst,
            kind,
            directed: 0,
            day,
            src_name: None,
            dst_name: None,
            facet: "work",
            path,
            anchor: None,
            ts: Some(1),
            weight: 1,
        }
    }
}

fn root(name: &str) -> PathBuf {
    reserve_temp_path(&format!("solstone-edge-query-{name}"))
}

fn seed(name: &str, rows: &[SeedEdge<'_>]) -> PathBuf {
    let root = root(name);
    let connection = open_index(&root).expect("seed native schema");
    insert_all(&connection, rows);
    drop(connection);
    root
}

fn insert_all(connection: &Connection, rows: &[SeedEdge<'_>]) {
    for row in rows {
        connection.execute(
            "INSERT INTO edges(src,dst,kind,directed,src_name,dst_name,day,facet,source,path,anchor,label,ts,weight) VALUES(?,?,?,?,?,?,?,?, 'test',?,?,?, ?, ?)",
            params![row.src, row.dst, row.kind, row.directed, row.src_name, row.dst_name, row.day, row.facet, row.path, row.anchor, row.path, row.ts, row.weight],
        ).expect("seed edge");
    }
}

fn cleanup(root: PathBuf) {
    fs::remove_dir_all(root).expect("remove temporary journal");
}
fn json_keys(value: &impl Serialize) -> BTreeSet<String> {
    serde_json::to_value(value)
        .unwrap()
        .as_object()
        .unwrap()
        .keys()
        .cloned()
        .collect()
}
fn default_network(reference_day: &str) -> NetworkRequest {
    NetworkRequest {
        reference_day: Some(reference_day.to_string()),
        ..NetworkRequest::default()
    }
}
fn default_overview(reference_day: &str) -> NetworkOverviewRequest {
    NetworkOverviewRequest {
        reference_day: Some(reference_day.to_string()),
        ..NetworkOverviewRequest::default()
    }
}

#[test]
fn network_has_exact_shape_undated_rows_and_truncation() {
    let mut alpha_dated = SeedEdge::new("self", "alpha", "works-with", Some("20260501"), "a");
    alpha_dated.dst_name = Some("Alpha");
    alpha_dated.weight = 2;
    let mut alpha_undated = SeedEdge::new("self", "alpha", "works-with", None, "a-undated");
    alpha_undated.dst_name = Some("Alpha");
    let mut beta = SeedEdge::new("self", "beta", "attended-with", None, "b");
    beta.dst_name = Some("Beta");
    let gamma = SeedEdge::new("self", "gamma", "mentioned", Some("20260502"), "c");
    let root = seed("network-shape", &[alpha_dated, alpha_undated, beta, gamma]);
    let mut request = default_network("20260530");
    request.limit = 1;
    request.evidence_limit = 1;
    let response = load_entity_network(&root, "self", &request, None, ATTENDANCE).unwrap();
    assert_eq!(
        json_keys(&response),
        BTreeSet::from(
            [
                "entity_id",
                "evidence_limit",
                "filters",
                "limit",
                "neighbors",
                "reference_day",
                "total_neighbors"
            ]
            .map(str::to_string)
        )
    );
    assert_eq!(
        json_keys(&response.filters),
        BTreeSet::from(
            ["day_from", "day_to", "facet", "include_principal", "kinds"].map(str::to_string)
        )
    );
    assert_eq!(response.total_neighbors, 3);
    assert_eq!(response.neighbors.len(), 1);
    let neighbor = &response.neighbors[0];
    assert_eq!(neighbor.entity_id, "alpha");
    assert_eq!(neighbor.count, 2);
    let dated_decay = (-(29.0_f64) * std::f64::consts::LN_2 / 90.0).exp();
    assert!((neighbor.score - (8.0 * dated_decay + 4.0)).abs() < 1e-12);
    assert_eq!(
        json_keys(neighbor),
        BTreeSet::from(
            [
                "count",
                "directed",
                "entity_id",
                "evidence",
                "evidence_class",
                "first_seen",
                "kinds",
                "last_seen",
                "name",
                "score"
            ]
            .map(str::to_string)
        )
    );
    assert_eq!(
        json_keys(&neighbor.directed),
        BTreeSet::from(["in", "out"].map(str::to_string))
    );
    assert_eq!(
        json_keys(neighbor.kinds.values().next().unwrap()),
        BTreeSet::from(["count", "weighted"].map(str::to_string))
    );
    cleanup(root);
}

#[test]
fn history_has_exact_shape_boolean_direction_and_keeps_future_rows() {
    let mut old = SeedEdge::new("a", "b", "mentioned", Some("20260501"), "same");
    old.directed = 1;
    old.ts = Some(1);
    old.anchor = Some("z");
    let mut future = SeedEdge::new("b", "a", "mentioned", Some("20270101"), "future");
    future.directed = 1;
    future.ts = Some(2);
    let root = seed("history-shape", &[old, future]);
    let response = load_edge_evidence(&root, "a", "b", &EdgeEvidenceRequest::default()).unwrap();
    assert_eq!(
        json_keys(&response),
        BTreeSet::from(
            [
                "entity_id",
                "evidence",
                "filters",
                "limit",
                "offset",
                "peer_id",
                "peer_name",
                "total"
            ]
            .map(str::to_string)
        )
    );
    assert_eq!(
        json_keys(&response.filters),
        BTreeSet::from(["day_from", "day_to", "facet", "kinds"].map(str::to_string))
    );
    assert_eq!(response.total, 2);
    assert_eq!(response.evidence[0].day.as_deref(), Some("20270101"));
    assert!(response.evidence[0].directed);
    assert_eq!(
        json_keys(&response.evidence[0]),
        BTreeSet::from(
            [
                "anchor", "day", "directed", "dst", "dst_name", "facet", "kind", "label", "path",
                "source", "src", "src_name", "ts", "weight"
            ]
            .map(str::to_string)
        )
    );
    let network =
        load_entity_network(&root, "a", &default_network("20260530"), None, ATTENDANCE).unwrap();
    let neighbor = network
        .neighbors
        .iter()
        .find(|item| item.entity_id == "b")
        .unwrap();
    assert_eq!(neighbor.evidence.len(), 1);
    assert_eq!(neighbor.evidence[0].day.as_deref(), Some("20260501"));
    cleanup(root);
}

#[test]
fn evidence_order_is_reference_chain_and_rowid_breaks_full_ties() {
    assert_eq!(
        EVIDENCE_ORDER_SQL,
        "ORDER BY day IS NULL ASC, day DESC,\n         ts IS NULL ASC, ts DESC,\n         path ASC, anchor IS NULL ASC, anchor ASC, rowid ASC"
    );
    let first = SeedEdge::new("a", "b", "works-with", Some("20260501"), "same");
    let second = SeedEdge::new("a", "b", "attended-with", Some("20260501"), "same");
    let root = seed("evidence-order", &[first, second]);
    let response = load_edge_evidence(&root, "a", "b", &EdgeEvidenceRequest::default()).unwrap();
    assert_eq!(
        response
            .evidence
            .iter()
            .map(|row| row.kind.as_str())
            .collect::<Vec<_>>(),
        vec!["works-with", "attended-with"]
    );
    cleanup(root);
}

#[test]
fn overview_has_exact_shape_counts_self_edge_once_and_uses_best_effort_types() {
    let mut self_edge = SeedEdge::new("a", "a", "works-with", Some("20260501"), "self");
    self_edge.src_name = Some("Alice");
    let mut peer = SeedEdge::new("a", "b", "attended-with", Some("20260502"), "peer");
    peer.dst_name = Some("Bob");
    let root = seed("overview-shape", &[self_edge, peer]);
    let response = load_network_overview(&root, &default_overview("20260530"), ATTENDANCE, &|id| {
        (id == "a").then(|| "person".to_string())
    })
    .unwrap();
    assert_eq!(
        json_keys(&response),
        BTreeSet::from(
            [
                "entities",
                "filters",
                "kinds",
                "limit",
                "reference_day",
                "totals"
            ]
            .map(str::to_string)
        )
    );
    assert_eq!(
        json_keys(&response.filters),
        BTreeSet::from(["day_from", "day_to", "facet", "kinds"].map(str::to_string))
    );
    assert_eq!(response.totals.edges, 2);
    assert_eq!(response.totals.entities, 2);
    let a = response
        .entities
        .iter()
        .find(|item| item.entity_id == "a")
        .unwrap();
    assert_eq!(a.count, 2);
    assert_eq!(a.r#type.as_deref(), Some("person"));
    let b = response
        .entities
        .iter()
        .find(|item| item.entity_id == "b")
        .unwrap();
    assert!(b.r#type.is_none());
    assert_eq!(
        json_keys(a),
        BTreeSet::from(
            [
                "count",
                "entity_id",
                "evidence_class",
                "first_seen",
                "kinds",
                "last_seen",
                "name",
                "score",
                "type"
            ]
            .map(str::to_string)
        )
    );
    cleanup(root);
}

#[test]
fn overview_truncation_keeps_totals_and_highest_scoring_entity() {
    let root = seed(
        "overview-truncation",
        &[
            SeedEdge::new("a", "b", "works-with", Some("20260501"), "high"),
            SeedEdge::new("a", "c", "mentioned", Some("20260501"), "low"),
        ],
    );
    let mut request = default_overview("20260530");
    request.limit = 1;
    let response = load_network_overview(&root, &request, ATTENDANCE, &|_| None).unwrap();
    assert_eq!(response.entities.len(), 1);
    assert_eq!(response.totals.entities, 3);
    assert_eq!(response.entities[0].entity_id, "a");
    cleanup(root);
}

#[test]
fn network_orders_equal_scores_by_entity_id() {
    let root = seed(
        "network-tiebreak",
        &[
            SeedEdge::new("self", "zeta", "works-with", Some("20260501"), "z"),
            SeedEdge::new("self", "alpha", "works-with", Some("20260501"), "a"),
        ],
    );
    let response = load_entity_network(
        &root,
        "self",
        &default_network("20260530"),
        None,
        ATTENDANCE,
    )
    .unwrap();
    assert_eq!(
        response
            .neighbors
            .iter()
            .map(|item| item.entity_id.as_str())
            .collect::<Vec<_>>(),
        vec!["alpha", "zeta"]
    );
    cleanup(root);
}

#[test]
fn network_principal_and_self_guards_match_reference() {
    let root = seed(
        "principal",
        &[
            SeedEdge::new("subject", "principal", "works-with", Some("20260501"), "p"),
            SeedEdge::new("subject", "subject", "works-with", Some("20260502"), "self"),
        ],
    );
    let hidden = load_entity_network(
        &root,
        "subject",
        &default_network("20260530"),
        Some("principal"),
        ATTENDANCE,
    )
    .unwrap();
    assert!(hidden.neighbors.is_empty());
    let mut request = default_network("20260530");
    request.include_principal = true;
    let shown =
        load_entity_network(&root, "subject", &request, Some("principal"), ATTENDANCE).unwrap();
    assert_eq!(
        shown
            .neighbors
            .iter()
            .map(|item| item.entity_id.as_str())
            .collect::<Vec<_>>(),
        vec!["principal"]
    );
    cleanup(root);
}

#[test]
fn network_treats_an_empty_principal_id_as_absent() {
    let root = seed(
        "empty-principal",
        &[SeedEdge::new(
            "subject",
            "",
            "works-with",
            Some("20260501"),
            "empty-peer",
        )],
    );
    let response = load_entity_network(
        &root,
        "subject",
        &default_network("20260530"),
        Some(""),
        ATTENDANCE,
    )
    .unwrap();
    assert_eq!(
        response
            .neighbors
            .iter()
            .map(|item| item.entity_id.as_str())
            .collect::<Vec<_>>(),
        vec![""]
    );
    cleanup(root);
}

#[test]
fn evidence_class_and_future_ranking_cap_match_reference() {
    let root = seed(
        "classes",
        &[
            SeedEdge::new("self", "att", "attended-with", Some("20260501"), "a"),
            SeedEdge::new("self", "mixed", "attended-with", Some("20260501"), "b"),
            SeedEdge::new("self", "mixed", "works-with", Some("20260501"), "c"),
            SeedEdge::new("self", "sem", "works-with", Some("20260501"), "semantic"),
            SeedEdge::new("self", "future", "works-with", Some("20270101"), "d"),
        ],
    );
    let response = load_entity_network(
        &root,
        "self",
        &default_network("20260530"),
        None,
        ATTENDANCE,
    )
    .unwrap();
    let classes = response
        .neighbors
        .iter()
        .map(|item| (item.entity_id.as_str(), item.evidence_class.as_str()))
        .collect::<Vec<_>>();
    assert!(classes.contains(&("att", "attendance")));
    assert!(classes.contains(&("mixed", "mixed")));
    assert!(classes.contains(&("sem", "semantic")));
    assert!(!classes.iter().any(|(id, _)| *id == "future"));
    cleanup(root);
}

#[test]
fn filters_and_negative_pagination_are_invalid_before_query() {
    let root = seed(
        "filters",
        &[SeedEdge::new("a", "b", "works-with", Some("20260501"), "x")],
    );
    let invalid_kind = EdgeEvidenceRequest {
        filters: EdgeFilters {
            kinds: Some(vec!["bogus".to_string()]),
            ..EdgeFilters::default()
        },
        ..EdgeEvidenceRequest::default()
    };
    assert!(matches!(
        load_edge_evidence(&root, "a", "b", &invalid_kind),
        Err(EdgeQueryError::InvalidRequestValue { .. })
    ));
    let invalid_limit = EdgeEvidenceRequest {
        limit: -1,
        ..EdgeEvidenceRequest::default()
    };
    match load_edge_evidence(&root, "a", "b", &invalid_limit) {
        Err(EdgeQueryError::InvalidRequestValue { detail }) => {
            assert_eq!(detail, "limit must be >= 0");
        }
        result => panic!("expected invalid limit, got {result:?}"),
    }
    let invalid_evidence_limit = NetworkRequest {
        evidence_limit: -1,
        ..NetworkRequest::default()
    };
    match load_entity_network(&root, "a", &invalid_evidence_limit, None, ATTENDANCE) {
        Err(EdgeQueryError::InvalidRequestValue { detail }) => {
            assert_eq!(detail, "evidence_limit must be between 0 and 5");
        }
        result => panic!("expected invalid evidence limit, got {result:?}"),
    }
    let invalid_offset = EdgeEvidenceRequest {
        offset: -1,
        ..EdgeEvidenceRequest::default()
    };
    match load_edge_evidence(&root, "a", "b", &invalid_offset) {
        Err(EdgeQueryError::InvalidRequestValue { detail }) => {
            assert_eq!(detail, "offset must be >= 0");
        }
        result => panic!("expected invalid offset, got {result:?}"),
    }
    let empty_kinds = EdgeEvidenceRequest {
        filters: EdgeFilters {
            kinds: Some(Vec::new()),
            ..EdgeFilters::default()
        },
        ..EdgeEvidenceRequest::default()
    };
    assert_eq!(
        load_edge_evidence(&root, "a", "b", &empty_kinds)
            .unwrap()
            .total,
        0
    );
    let empty_facet = EdgeEvidenceRequest {
        filters: EdgeFilters {
            facet: Some(String::new()),
            ..EdgeFilters::default()
        },
        ..EdgeEvidenceRequest::default()
    };
    let response = load_edge_evidence(&root, "a", "b", &empty_facet).unwrap();
    assert_eq!(response.filters.facet.as_deref(), Some(""));
    assert_eq!(response.total, 0);
    cleanup(root);
}

#[test]
fn empty_partial_and_absent_indexes_have_distinct_results_without_writes() {
    let empty = seed("empty", &[]);
    let network =
        load_entity_network(&empty, "a", &default_network("20260530"), None, ATTENDANCE).unwrap();
    assert_eq!(
        json_keys(&network),
        BTreeSet::from(
            [
                "entity_id",
                "evidence_limit",
                "filters",
                "limit",
                "neighbors",
                "reference_day",
                "total_neighbors"
            ]
            .map(str::to_string)
        )
    );
    assert_eq!(network.total_neighbors, 0);
    assert!(network.neighbors.is_empty());
    let history = load_edge_evidence(&empty, "a", "b", &EdgeEvidenceRequest::default()).unwrap();
    assert_eq!(
        json_keys(&history),
        BTreeSet::from(
            [
                "entity_id",
                "evidence",
                "filters",
                "limit",
                "offset",
                "peer_id",
                "peer_name",
                "total"
            ]
            .map(str::to_string)
        )
    );
    assert_eq!(history.total, 0);
    assert!(history.evidence.is_empty());
    let overview =
        load_network_overview(&empty, &default_overview("20260530"), ATTENDANCE, &|_| None)
            .unwrap();
    assert_eq!(
        json_keys(&overview),
        BTreeSet::from(
            [
                "entities",
                "filters",
                "kinds",
                "limit",
                "reference_day",
                "totals"
            ]
            .map(str::to_string)
        )
    );
    assert_eq!(overview.totals.edges, 0);
    assert_eq!(overview.totals.entities, 0);
    assert!(overview.entities.is_empty());
    cleanup(empty);
    let partial = seed("partial", &[]);
    let conn = Connection::open(db_path(&partial)).unwrap();
    conn.execute("DROP TABLE edges", []).unwrap();
    drop(conn);
    assert!(matches!(
        load_network_overview(&partial, &default_overview("20260530"), ATTENDANCE, &|_| {
            None
        }),
        Err(EdgeQueryError::EdgeIndexUnavailable { .. })
    ));
    let conn = Connection::open(db_path(&partial)).unwrap();
    assert_eq!(
        conn.query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='edges'",
            [],
            |row| row.get::<_, i64>(0)
        )
        .unwrap(),
        0
    );
    cleanup(partial);
    let absent = root("absent");
    fs::create_dir_all(&absent).unwrap();
    assert!(matches!(
        open_edges_reader(&absent),
        Err(EdgeQueryError::EdgeIndexUnavailable { .. })
    ));
    assert!(!absent.join("indexer").exists());
    cleanup(absent);
}

#[test]
fn stored_unknown_kind_fails_loudly_as_internal_error() {
    let root = seed("stored-kind", &[]);
    let connection = Connection::open(db_path(&root)).unwrap();
    connection.execute("INSERT INTO edges(src,dst,kind,directed,source,path,weight) VALUES ('a','b','foreign',0,'test','foreign',1)", []).unwrap();
    drop(connection);
    assert!(matches!(
        load_network_overview(&root, &default_overview("20260530"), ATTENDANCE, &|_| None),
        Err(EdgeQueryError::Internal { .. })
    ));
    cleanup(root);
}

#[test]
fn stored_decode_failures_are_internal_not_index_unavailable() {
    let root = seed("decode-failure", &[]);
    let connection = Connection::open(db_path(&root)).unwrap();
    connection
        .execute(
            "INSERT INTO edges(src,dst,kind,directed,source,path,weight) VALUES ('a','b','works-with',X'FF','test','bad-directed',1)",
            [],
        )
        .unwrap();
    drop(connection);
    assert!(matches!(
        load_edge_evidence(&root, "a", "b", &EdgeEvidenceRequest::default()),
        Err(EdgeQueryError::Internal { .. })
    ));
    cleanup(root);
}

#[test]
fn peer_and_endpoint_names_choose_first_nonnull_evidence_order() {
    let mut older = SeedEdge::new("a", "b", "works-with", Some("20260501"), "old");
    older.dst_name = Some("Old");
    let mut newer = SeedEdge::new("a", "b", "works-with", Some("20260502"), "new");
    newer.dst_name = None;
    let root = seed("names", &[older, newer]);
    let history = load_edge_evidence(&root, "a", "b", &EdgeEvidenceRequest::default()).unwrap();
    assert_eq!(history.peer_name.as_deref(), Some("Old"));
    let overview =
        load_network_overview(&root, &default_overview("20260530"), ATTENDANCE, &|_| None).unwrap();
    assert_eq!(
        overview
            .entities
            .iter()
            .find(|item| item.entity_id == "b")
            .unwrap()
            .name
            .as_deref(),
        Some("Old")
    );
    cleanup(root);
}

#[test]
fn overview_type_lookup_skips_unsafe_entity_component() {
    let root = seed(
        "unsafe-type",
        &[SeedEdge::new(
            "safe",
            "../unsafe",
            "works-with",
            Some("20260501"),
            "unsafe",
        )],
    );
    let calls = std::cell::RefCell::new(Vec::new());
    let response = load_network_overview(&root, &default_overview("20260530"), ATTENDANCE, &|id| {
        calls.borrow_mut().push(id.to_string());
        Some("person".to_string())
    })
    .unwrap();
    let calls = calls.into_inner();
    assert!(calls.contains(&"safe".to_string()));
    assert!(!calls.contains(&"../unsafe".to_string()));
    assert!(
        response
            .entities
            .iter()
            .find(|item| item.entity_id == "../unsafe")
            .unwrap()
            .r#type
            .is_none()
    );
    cleanup(root);
}

#[test]
fn reference_day_defaults_to_local_time() {
    let root = seed("local-day", &[]);
    let response = load_network_overview(
        &root,
        &NetworkOverviewRequest::default(),
        ATTENDANCE,
        &|_| None,
    )
    .unwrap();
    assert_eq!(
        response.reference_day,
        Local::now().format("%Y%m%d").to_string()
    );
    cleanup(root);
}

#[test]
fn reader_opens_real_schema_without_creating_a_second_database() {
    let root = seed("reader", &[]);
    assert!(db_path(&root).is_file());
    let connection = open_edges_reader(&root).unwrap();
    assert_eq!(
        connection
            .query_row("PRAGMA query_only", [], |row| row.get::<_, i64>(0))
            .unwrap(),
        1
    );
    cleanup(root);
}

#[test]
fn network_limits_reject_before_the_index_is_opened() {
    let root = root("network-limits-reject");
    fs::create_dir(&root).expect("create dir");

    let invalid_limit_neg = NetworkRequest {
        limit: -1,
        ..NetworkRequest::default()
    };
    match load_entity_network(&root, "a", &invalid_limit_neg, None, ATTENDANCE) {
        Err(EdgeQueryError::InvalidRequestValue { detail }) => {
            assert_eq!(
                detail,
                network_bound_detail("limit", NETWORK_NEIGHBOR_LIMIT_MAX)
            );
            assert_eq!(detail, "limit must be between 0 and 100");
        }
        result => panic!("expected invalid limit, got {result:?}"),
    }

    let invalid_limit_high = NetworkRequest {
        limit: 101,
        ..NetworkRequest::default()
    };
    match load_entity_network(&root, "a", &invalid_limit_high, None, ATTENDANCE) {
        Err(EdgeQueryError::InvalidRequestValue { detail }) => {
            assert_eq!(
                detail,
                network_bound_detail("limit", NETWORK_NEIGHBOR_LIMIT_MAX)
            );
            assert_eq!(detail, "limit must be between 0 and 100");
        }
        result => panic!("expected invalid limit, got {result:?}"),
    }

    let invalid_ev_neg = NetworkRequest {
        evidence_limit: -1,
        ..NetworkRequest::default()
    };
    match load_entity_network(&root, "a", &invalid_ev_neg, None, ATTENDANCE) {
        Err(EdgeQueryError::InvalidRequestValue { detail }) => {
            assert_eq!(
                detail,
                network_bound_detail("evidence_limit", NETWORK_EVIDENCE_LIMIT_MAX)
            );
            assert_eq!(detail, "evidence_limit must be between 0 and 5");
        }
        result => panic!("expected invalid evidence_limit, got {result:?}"),
    }

    let invalid_ev_high = NetworkRequest {
        evidence_limit: 6,
        ..NetworkRequest::default()
    };
    match load_entity_network(&root, "a", &invalid_ev_high, None, ATTENDANCE) {
        Err(EdgeQueryError::InvalidRequestValue { detail }) => {
            assert_eq!(
                detail,
                network_bound_detail("evidence_limit", NETWORK_EVIDENCE_LIMIT_MAX)
            );
            assert_eq!(detail, "evidence_limit must be between 0 and 5");
        }
        result => panic!("expected invalid evidence_limit, got {result:?}"),
    }

    // Inclusive ends pass validation and attempt to open the index, failing with EdgeIndexUnavailable
    for limit in [0, 100] {
        let req = NetworkRequest {
            limit,
            ..NetworkRequest::default()
        };
        assert!(matches!(
            load_entity_network(&root, "a", &req, None, ATTENDANCE),
            Err(EdgeQueryError::EdgeIndexUnavailable { .. })
        ));
    }
    for evidence_limit in [0, 5] {
        let req = NetworkRequest {
            evidence_limit,
            ..NetworkRequest::default()
        };
        assert!(matches!(
            load_entity_network(&root, "a", &req, None, ATTENDANCE),
            Err(EdgeQueryError::EdgeIndexUnavailable { .. })
        ));
    }

    assert!(!solstone_core_indexer_store::db::db_path(&root).exists());
    cleanup(root);
}

#[test]
fn network_preview_caps_high_degree_and_zero_limit() {
    let ids: Vec<String> = (0..=100).map(|n| format!("p{n:03}")).collect();
    let mut rows: Vec<SeedEdge<'_>> = ids
        .iter()
        .map(|id| SeedEdge::new("self", id, "works-with", None, "p"))
        .collect();
    let mut prin = SeedEdge::new("self", "prin", "committed-to", None, "prin");
    prin.weight = 100;
    rows.push(prin);

    let mut future = SeedEdge::new("self", "future", "works-with", Some("20270101"), "future");
    future.weight = 100;
    rows.push(future);

    let root = seed("network-high-degree", &rows);

    let req = NetworkRequest {
        limit: 100,
        evidence_limit: 1,
        include_principal: false,
        reference_day: Some("20260530".to_string()),
        ..NetworkRequest::default()
    };
    let response = load_entity_network(&root, "self", &req, Some("prin"), ATTENDANCE).unwrap();
    assert_eq!(response.total_neighbors, 101);
    assert_eq!(response.neighbors.len(), 100);
    for n in 0..100 {
        assert_eq!(response.neighbors[n].entity_id, format!("p{n:03}"));
    }
    assert!(!response.neighbors.iter().any(|n| n.entity_id == "prin"));
    assert!(!response.neighbors.iter().any(|n| n.entity_id == "future"));
    assert_eq!(response.neighbors[0].score, response.neighbors[99].score);

    let zero_req = NetworkRequest {
        limit: 0,
        evidence_limit: 0,
        include_principal: false,
        reference_day: Some("20260530".to_string()),
        ..NetworkRequest::default()
    };
    let zero_resp =
        load_entity_network(&root, "self", &zero_req, Some("prin"), ATTENDANCE).unwrap();
    assert!(zero_resp.neighbors.is_empty());
    assert_eq!(zero_resp.total_neighbors, 101);

    cleanup(root);
}

#[test]
fn network_preview_rank_filters_and_evidence_follow_the_request() {
    // 1. Multi-group score beats a heavier single group.
    {
        let root = seed(
            "score-groups",
            &[
                SeedEdge::new("self", "summed", "works-with", Some("20260530"), "s1"),
                SeedEdge::new("self", "summed", "works-with", Some("20260501"), "s2"),
                SeedEdge::new("self", "heavy", "committed-to", Some("20260530"), "h1"),
            ],
        );
        let req = NetworkRequest {
            limit: 1,
            reference_day: Some("20260530".to_string()),
            ..NetworkRequest::default()
        };
        let resp = load_entity_network(&root, "self", &req, None, ATTENDANCE).unwrap();
        assert_eq!(resp.total_neighbors, 2);
        assert_eq!(resp.neighbors.len(), 1);
        let n = &resp.neighbors[0];
        assert_eq!(n.entity_id, "summed");
        assert_eq!(n.first_seen.as_deref(), Some("20260501"));
        assert_eq!(n.last_seen.as_deref(), Some("20260530"));
        let expected_score = 4.0 + 4.0 * (-29.0 * std::f64::consts::LN_2 / 90.0).exp();
        assert!((n.score - expected_score).abs() < 1e-12);
        cleanup(root);
    }

    // 2. Principal toggle.
    {
        let mut prin_edge = SeedEdge::new("self", "prin", "committed-to", Some("20260530"), "p");
        prin_edge.weight = 10;
        let root = seed(
            "principal-toggle",
            &[
                prin_edge,
                SeedEdge::new("self", "alpha", "works-with", Some("20260530"), "a"),
                SeedEdge::new("self", "beta", "mentioned", Some("20260530"), "b"),
            ],
        );
        let req_excl = NetworkRequest {
            limit: 1,
            include_principal: false,
            reference_day: Some("20260530".to_string()),
            ..NetworkRequest::default()
        };
        let resp_excl =
            load_entity_network(&root, "self", &req_excl, Some("prin"), ATTENDANCE).unwrap();
        assert_eq!(resp_excl.total_neighbors, 2);
        assert_eq!(resp_excl.neighbors.len(), 1);
        assert_eq!(resp_excl.neighbors[0].entity_id, "alpha");
        assert!(!resp_excl.neighbors.iter().any(|n| n.entity_id == "prin"));

        let req_incl = NetworkRequest {
            limit: 1,
            include_principal: true,
            reference_day: Some("20260530".to_string()),
            ..NetworkRequest::default()
        };
        let resp_incl =
            load_entity_network(&root, "self", &req_incl, Some("prin"), ATTENDANCE).unwrap();
        assert_eq!(resp_incl.total_neighbors, 3);
        assert_eq!(resp_incl.neighbors.len(), 1);
        assert_eq!(resp_incl.neighbors[0].entity_id, "prin");
        cleanup(root);
    }

    // 3. Evidence window and name outside the cap.
    {
        let mut e1 = SeedEdge::new("self", "solo", "works-with", Some("20260501"), "e1");
        e1.dst_name = Some("Canonical");
        let root = seed(
            "evidence-window",
            &[
                SeedEdge::new("self", "solo", "works-with", Some("20260506"), "e6"),
                SeedEdge::new("self", "solo", "works-with", Some("20260505"), "e5"),
                SeedEdge::new("self", "solo", "works-with", Some("20260504"), "e4"),
                SeedEdge::new("self", "solo", "works-with", Some("20260503"), "e3"),
                SeedEdge::new("self", "solo", "works-with", Some("20260502"), "e2"),
                e1,
            ],
        );

        let req5 = NetworkRequest {
            evidence_limit: 5,
            reference_day: Some("20260530".to_string()),
            ..NetworkRequest::default()
        };
        let resp5 = load_entity_network(&root, "self", &req5, None, ATTENDANCE).unwrap();
        assert_eq!(resp5.neighbors.len(), 1);
        assert_eq!(resp5.neighbors[0].name.as_deref(), Some("Canonical"));
        let paths5: Vec<_> = resp5.neighbors[0]
            .evidence
            .iter()
            .map(|e| e.path.as_str())
            .collect();
        assert_eq!(paths5, vec!["e6", "e5", "e4", "e3", "e2"]);

        let req1 = NetworkRequest {
            evidence_limit: 1,
            reference_day: Some("20260530".to_string()),
            ..NetworkRequest::default()
        };
        let resp1 = load_entity_network(&root, "self", &req1, None, ATTENDANCE).unwrap();
        assert_eq!(resp1.neighbors[0].name.as_deref(), Some("Canonical"));
        let paths1: Vec<_> = resp1.neighbors[0]
            .evidence
            .iter()
            .map(|e| e.path.as_str())
            .collect();
        assert_eq!(paths1, vec!["e6"]);

        let req0 = NetworkRequest {
            evidence_limit: 0,
            reference_day: Some("20260530".to_string()),
            ..NetworkRequest::default()
        };
        let resp0 = load_entity_network(&root, "self", &req0, None, ATTENDANCE).unwrap();
        assert_eq!(resp0.neighbors[0].name.as_deref(), Some("Canonical"));
        assert!(resp0.neighbors[0].evidence.is_empty());
        cleanup(root);
    }

    // 4. Combined filters drop higher peers, and kept peer's name and evidence ignore non-matching rows.
    {
        let mut hi_future = SeedEdge::new(
            "self",
            "hi-future",
            "works-with",
            Some("20270101"),
            "hi-future",
        );
        hi_future.weight = 100;
        let mut hi_kind = SeedEdge::new(
            "self",
            "hi-kind",
            "committed-to",
            Some("20260510"),
            "hi-kind",
        );
        hi_kind.weight = 100;
        let mut hi_facet = SeedEdge::new(
            "self",
            "hi-facet",
            "works-with",
            Some("20260510"),
            "hi-facet",
        );
        hi_facet.facet = "other";
        hi_facet.weight = 100;
        let mut hi_early = SeedEdge::new(
            "self",
            "hi-early",
            "works-with",
            Some("20260101"),
            "hi-early",
        );
        hi_early.weight = 100;
        let mut hi_late =
            SeedEdge::new("self", "hi-late", "works-with", Some("20260615"), "hi-late");
        hi_late.weight = 100;

        let mut kept_match = SeedEdge::new("self", "kept", "works-with", Some("20260510"), "kept");
        kept_match.dst_name = Some("Kept");
        let mut kept_future = SeedEdge::new(
            "self",
            "kept",
            "works-with",
            Some("20270101"),
            "kept-future",
        );
        kept_future.dst_name = Some("Future");
        let mut kept_late =
            SeedEdge::new("self", "kept", "works-with", Some("20260615"), "kept-late");
        kept_late.dst_name = Some("AfterTo");
        let mut kept_kind = SeedEdge::new(
            "self",
            "kept",
            "attended-with",
            Some("20260520"),
            "kept-kind",
        );
        kept_kind.dst_name = Some("WrongKind");
        let mut kept_facet =
            SeedEdge::new("self", "kept", "works-with", Some("20260518"), "kept-facet");
        kept_facet.facet = "other";
        kept_facet.dst_name = Some("WrongFacet");
        let mut kept_early =
            SeedEdge::new("self", "kept", "works-with", Some("20260401"), "kept-early");
        kept_early.dst_name = Some("BeforeFrom");

        let root = seed(
            "combined-filters",
            &[
                hi_future,
                hi_kind,
                hi_facet,
                hi_early,
                hi_late,
                kept_match,
                kept_future,
                kept_late,
                kept_kind,
                kept_facet,
                kept_early,
            ],
        );

        let req = NetworkRequest {
            filters: EdgeFilters {
                kinds: Some(vec!["works-with".to_string()]),
                facet: Some("work".to_string()),
                day_from: Some("20260501".to_string()),
                day_to: Some("20260530".to_string()),
            },
            limit: 25,
            evidence_limit: 5,
            reference_day: Some("20260620".to_string()),
            ..NetworkRequest::default()
        };
        let resp = load_entity_network(&root, "self", &req, None, ATTENDANCE).unwrap();
        assert_eq!(resp.total_neighbors, 1);
        assert_eq!(resp.neighbors.len(), 1);
        let neighbor = &resp.neighbors[0];
        assert_eq!(neighbor.entity_id, "kept");
        assert_eq!(neighbor.name.as_deref(), Some("Kept"));
        let paths: Vec<_> = neighbor.evidence.iter().map(|e| e.path.as_str()).collect();
        assert_eq!(paths, vec!["kept"]);
        let expected_score = 4.0 * (-41.0 * std::f64::consts::LN_2 / 90.0).exp();
        assert!((neighbor.score - expected_score).abs() < 1e-12);
        cleanup(root);
    }
}

#[test]
fn edge_repair_state_reader_and_query_behavior() {
    let root = seed(
        "repair-state",
        &[SeedEdge::new(
            "alice",
            "bob",
            "works-with",
            Some("20260101"),
            "seed",
        )],
    );

    let net_req = default_network("20260102");
    let ev_req = EdgeEvidenceRequest::default();
    let ov_req = default_overview("20260102");
    let no_type = |_: &str| None;

    // 1. Healthy populated
    let net = load_entity_network(&root, "alice", &net_req, None, ATTENDANCE).unwrap();
    assert_eq!(net.total_neighbors, 1);
    assert!(net.edge_repair_state.is_none());
    assert!(!json_keys(&net).contains("edge_repair_state"));

    let ev = load_edge_evidence(&root, "alice", "bob", &ev_req).unwrap();
    assert_eq!(ev.total, 1);
    assert!(ev.edge_repair_state.is_none());
    assert!(!json_keys(&ev).contains("edge_repair_state"));

    let ov = load_network_overview(&root, &ov_req, ATTENDANCE, &no_type).unwrap();
    assert_eq!(ov.totals.edges, 1);
    assert!(ov.edge_repair_state.is_none());
    assert!(!json_keys(&ov).contains("edge_repair_state"));

    // 2. Pending job
    let jobs_dir = root.join("health/entity-edge-repair/jobs");
    fs::create_dir_all(&jobs_dir).unwrap();
    fs::write(jobs_dir.join("merge-m1.json"), b"{}").unwrap();

    let net = load_entity_network(&root, "alice", &net_req, None, ATTENDANCE).unwrap();
    assert_eq!(net.total_neighbors, 0);
    assert_eq!(net.neighbors.len(), 0);
    assert_eq!(net.edge_repair_state.as_deref(), Some("pending"));
    assert!(json_keys(&net).contains("edge_repair_state"));

    let ev = load_edge_evidence(&root, "alice", "bob", &ev_req).unwrap();
    assert_eq!(ev.total, 0);
    assert_eq!(ev.evidence.len(), 0);
    assert_eq!(ev.edge_repair_state.as_deref(), Some("pending"));

    let ov = load_network_overview(&root, &ov_req, ATTENDANCE, &no_type).unwrap();
    assert_eq!(ov.totals.edges, 0);
    assert_eq!(ov.entities.len(), 0);
    assert_eq!(ov.edge_repair_state.as_deref(), Some("pending"));

    // 3. Interrupted job (progress file exists)
    let prog_dir = root.join("health/entity-edge-repair/progress");
    fs::create_dir_all(&prog_dir).unwrap();
    fs::write(prog_dir.join("merge-m1.json"), b"{}").unwrap();

    let net = load_entity_network(&root, "alice", &net_req, None, ATTENDANCE).unwrap();
    assert_eq!(net.edge_repair_state.as_deref(), Some("interrupted"));

    let ev = load_edge_evidence(&root, "alice", "bob", &ev_req).unwrap();
    assert_eq!(ev.edge_repair_state.as_deref(), Some("interrupted"));

    let ov = load_network_overview(&root, &ov_req, ATTENDANCE, &no_type).unwrap();
    assert_eq!(ov.edge_repair_state.as_deref(), Some("interrupted"));

    // 4. Failed job (failure file exists)
    let fail_dir = root.join("health/entity-edge-repair/failures");
    fs::create_dir_all(&fail_dir).unwrap();
    fs::write(fail_dir.join("merge-m1.json"), b"error").unwrap();

    let net = load_entity_network(&root, "alice", &net_req, None, ATTENDANCE).unwrap();
    assert_eq!(net.edge_repair_state.as_deref(), Some("failed"));

    let ev = load_edge_evidence(&root, "alice", "bob", &ev_req).unwrap();
    assert_eq!(ev.edge_repair_state.as_deref(), Some("failed"));

    let ov = load_network_overview(&root, &ov_req, ATTENDANCE, &no_type).unwrap();
    assert_eq!(ov.edge_repair_state.as_deref(), Some("failed"));

    // 5. Completion exists -> healthy again
    let comp_dir = root.join("health/entity-edge-repair/completions");
    fs::create_dir_all(&comp_dir).unwrap();
    fs::write(comp_dir.join("merge-m1.json"), b"{}").unwrap();

    let net = load_entity_network(&root, "alice", &net_req, None, ATTENDANCE).unwrap();
    assert_eq!(net.total_neighbors, 1);
    assert!(net.edge_repair_state.is_none());

    cleanup(root);
}
