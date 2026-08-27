// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Black-box contracts for native `solstone call profile` routes.

use std::fs;
use std::path::Path;

use axum::{
    body::{Body, to_bytes},
    http::{HeaderMap, Request, StatusCode, header},
};
use chrono::{Duration, Utc};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use solstone_core_convey_shell::router;
use tower::ServiceExt;

struct Fixture {
    root: tempfile::TempDir,
}

impl Fixture {
    fn new() -> Self {
        let root = tempfile::TempDir::new_in("/var/tmp").expect("fixture");
        for path in ["config", "entities", "facets"] {
            fs::create_dir_all(root.path().join(path)).expect("journal directory");
        }
        Self { root }
    }

    fn established(&self) {
        write_json(
            self.root.path().join("config/journal.json"),
            json!({"setup":{"completed_at":1},"identity":{"name":"Profile Owner","timezone":"UTC"},"providers":{"active":{"provider":"test"}}}),
        );
    }

    fn entity(&self, id: &str, name: &str, aka: &[&str], entity_type: &str, is_principal: bool) {
        write_json(
            self.root.path().join(format!("entities/{id}/entity.json")),
            json!({"id":id,"name":name,"aka":aka,"type":entity_type,"is_principal":is_principal}),
        );
    }

    fn facet(&self, name: &str, muted: bool) {
        write_json(
            self.root.path().join(format!("facets/{name}/facet.json")),
            json!({"name":name,"muted":muted}),
        );
    }

    fn relationship(&self, facet: &str, entity_id: &str, description: &str) {
        write_json(
            self.root
                .path()
                .join(format!("facets/{facet}/entities/{entity_id}/entity.json")),
            json!({"entity_id":entity_id,"description":description}),
        );
    }

    fn activities(&self, facet: &str, day: &str, rows: &[Value]) {
        write_jsonl(
            self.root
                .path()
                .join(format!("facets/{facet}/activities/{day}.jsonl")),
            rows,
        );
    }
}

fn write_json(path: impl AsRef<Path>, value: Value) {
    let path = path.as_ref();
    fs::create_dir_all(path.parent().expect("parent")).expect("parent directory");
    fs::write(
        path,
        format!("{}\n", serde_json::to_string(&value).expect("JSON")),
    )
    .expect("write JSON");
}

fn write_jsonl(path: impl AsRef<Path>, rows: &[Value]) {
    let path = path.as_ref();
    fs::create_dir_all(path.parent().expect("parent")).expect("parent directory");
    fs::write(
        path,
        format!(
            "{}\n",
            rows.iter()
                .map(|row| serde_json::to_string(row).expect("JSON"))
                .collect::<Vec<_>>()
                .join("\n")
        ),
    )
    .expect("write JSONL");
}

async fn get(path: &str, fixture: &Fixture) -> (StatusCode, HeaderMap, Vec<u8>) {
    let response = router(fixture.root.path().to_path_buf())
        .oneshot(Request::get(path).body(Body::empty()).expect("request"))
        .await
        .expect("response");
    let status = response.status();
    let headers = response.headers().clone();
    let body = to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("body")
        .to_vec();
    (status, headers, body)
}

fn body_json(bytes: &[u8]) -> Value {
    serde_json::from_slice(bytes).expect("JSON response")
}

fn day_ago(days: i64) -> String {
    (Utc::now().date_naive() - Duration::days(days))
        .format("%Y%m%d")
        .to_string()
}

fn timestamp_ago(days: i64) -> i64 {
    (Utc::now() - Duration::days(days)).timestamp_millis()
}

fn folded_id(parts: &[&str]) -> String {
    let mut digest = Sha256::new();
    digest.update(parts.join("|"));
    format!("{:x}", digest.finalize())[..16].to_owned()
}

fn attendee(id: &str, created_at: i64, entity_id: &str) -> Value {
    json!({"id":id,"created_at":created_at,"participation":[{"entity_id":entity_id,"role":"attendee"}]})
}

#[tokio::test]
async fn all_profile_routes_redirect_to_init_when_unestablished() {
    let fixture = Fixture::new();
    for path in [
        "/api/profile/missing",
        "/api/profile/missing/brief",
        "/api/profile/missing/cadence",
        "/api/profiles/active",
    ] {
        let (status, headers, _) = get(path, &fixture).await;
        assert_eq!(status, StatusCode::FOUND, "{path}");
        assert_eq!(headers.get(header::LOCATION).expect("location"), "/init");
    }
}

#[tokio::test]
async fn name_keyed_routes_return_the_legacy_not_found_envelope() {
    let fixture = Fixture::new();
    fixture.established();
    for path in [
        "/api/profile/Nobody",
        "/api/profile/Nobody/brief",
        "/api/profile/Nobody/cadence",
    ] {
        let (status, _, bytes) = get(path, &fixture).await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{path}");
        assert_eq!(
            body_json(&bytes),
            json!({"error":"I couldn't find that entity.","reason_code":"entity_not_found","detail":"no entity named 'Nobody'"})
        );
    }
}

#[tokio::test]
async fn encoded_name_and_id_name_aka_and_fuzzy_resolution_succeed() {
    let fixture = Fixture::new();
    fixture.established();
    fixture.entity("ada", "Ada Lovelace/É", &["Countess Ada"], "person", false);
    fixture.facet("work", false);
    fixture.relationship("work", "ada", "Mathematician");

    for path in [
        "/api/profile/Ada%20Lovelace%2F%C3%89/brief",
        "/api/profile/ada/brief",
        "/api/profile/Countess%20Ada/brief",
        "/api/profile/Ada%20Lovelace%2FZ/brief",
    ] {
        let (status, _, _) = get(path, &fixture).await;
        assert_eq!(status, StatusCode::OK, "{path}");
    }

    fixture.entity("duplicate_a", "Same Name", &[], "person", false);
    fixture.entity("duplicate_b", "Same Name", &[], "person", false);
    let (status, _, bytes) = get("/api/profile/Same%20Name", &fixture).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(body_json(&bytes)["reason_code"], "entity_not_found");
}

#[tokio::test]
async fn cadence_handles_zero_single_multi_distinct_and_quiet_boundaries() {
    let fixture = Fixture::new();
    fixture.established();
    fixture.facet("work", false);
    for id in ["zero", "single", "multi", "boundary", "quiet", "distinct"] {
        fixture.entity(id, id, &[], "person", false);
    }
    fixture.activities(
        "work",
        &day_ago(8),
        &[attendee("quiet-first", timestamp_ago(8), "quiet")],
    );
    fixture.activities(
        "work",
        &day_ago(6),
        &[
            attendee("multi-first", timestamp_ago(6), "multi"),
            attendee("boundary-first", timestamp_ago(6), "boundary"),
            attendee("quiet-last", timestamp_ago(6), "quiet"),
        ],
    );
    fixture.activities(
        "work",
        &day_ago(4),
        &[
            attendee("boundary-last", timestamp_ago(4), "boundary"),
            attendee("distinct-1", timestamp_ago(4), "distinct"),
            attendee("distinct-2", timestamp_ago(4), "distinct"),
            attendee("distinct-3", timestamp_ago(4), "distinct"),
        ],
    );
    fixture.activities(
        "work",
        &day_ago(3),
        &[attendee("multi-middle", timestamp_ago(3), "multi")],
    );
    fixture.activities(
        "work",
        &day_ago(2),
        &[attendee("distinct-last", timestamp_ago(2), "distinct")],
    );
    fixture.activities(
        "work",
        &day_ago(1),
        &[attendee("single", timestamp_ago(1), "single")],
    );
    fixture.activities(
        "work",
        &day_ago(0),
        &[attendee("multi-last", timestamp_ago(0), "multi")],
    );

    let (_, _, zero) = get("/api/profile/zero/cadence", &fixture).await;
    assert_eq!(
        body_json(&zero),
        json!({"recent_interactions_count_30d":0,"last_seen":null,"avg_interval_days":null,"gone_quiet_since":null})
    );
    let (_, _, single) = get("/api/profile/single/cadence", &fixture).await;
    assert_eq!(body_json(&single)["avg_interval_days"], Value::Null);
    let (_, _, multi) = get("/api/profile/multi/cadence", &fixture).await;
    assert_eq!(body_json(&multi)["avg_interval_days"], 3.0);
    assert_eq!(body_json(&multi)["gone_quiet_since"], Value::Null);
    let (_, _, boundary) = get("/api/profile/boundary/cadence", &fixture).await;
    assert_eq!(body_json(&boundary)["avg_interval_days"], 2.0);
    assert_eq!(body_json(&boundary)["gone_quiet_since"], Value::Null);
    let (_, _, quiet) = get("/api/profile/quiet/cadence", &fixture).await;
    assert_eq!(body_json(&quiet)["avg_interval_days"], 2.0);
    assert_eq!(body_json(&quiet)["gone_quiet_since"], 6);
    let (_, _, distinct) = get("/api/profile/distinct/cadence", &fixture).await;
    assert_eq!(body_json(&distinct)["recent_interactions_count_30d"], 4);
    assert_eq!(body_json(&distinct)["avg_interval_days"], 2.0);
}

#[tokio::test]
async fn cadence_truthy_mentions_values_are_exact_and_trimmed() {
    let fixture = Fixture::new();
    fixture.established();
    fixture.entity("pat", "Pat", &[], "person", false);
    fixture.facet("work", false);
    fixture.activities(
        "work",
        &day_ago(0),
        &[json!({"id":"mention","created_at":1,"participation":[{"entity_id":"pat","role":"mentioned"}]})],
    );

    for value in ["1", "TrUe", "yes", "on", "%20YES%20"] {
        let (_, _, body) = get(
            &format!("/api/profile/pat/cadence?include_mentions={value}"),
            &fixture,
        )
        .await;
        assert_eq!(
            body_json(&body)["recent_interactions_count_30d"],
            1,
            "{value}"
        );
    }
    for path in [
        "/api/profile/pat/cadence",
        "/api/profile/pat/cadence?include_mentions=false",
    ] {
        let (_, _, body) = get(path, &fixture).await;
        assert_eq!(
            body_json(&body)["recent_interactions_count_30d"],
            0,
            "{path}"
        );
    }
}

#[tokio::test]
async fn facet_filter_changes_display_only_and_self_flag_tracks_principal() {
    let fixture = Fixture::new();
    fixture.established();
    fixture.entity("pat", "Pat", &[], "person", true);
    fixture.entity("other", "Other", &[], "person", false);
    for (facet, description) in [("work", "Work friend"), ("math", "Math friend")] {
        fixture.facet(facet, false);
        fixture.relationship(facet, "pat", description);
        fixture.activities(
            facet,
            &day_ago(0),
            &[attendee(&format!("{facet}-meeting"), 1, "pat")],
        );
    }

    let (_, _, unfiltered) = get("/api/profile/pat", &fixture).await;
    let (_, _, filtered) = get("/api/profile/pat?facets=work,missing,work", &fixture).await;
    let unfiltered = body_json(&unfiltered);
    let filtered = body_json(&filtered);
    assert_eq!(unfiltered["facets"], json!(["math", "work"]));
    assert_eq!(filtered["facets"], json!(["work", "work"]));
    assert_eq!(filtered["description"], "Work friend | Work friend");
    assert_eq!(unfiltered["cadence"], filtered["cadence"]);
    assert_eq!(unfiltered["is_self"], true);
    let (_, _, other) = get("/api/profile/other", &fixture).await;
    assert_eq!(body_json(&other)["is_self"], false);
}

#[tokio::test]
async fn rich_full_brief_cadence_and_active_responses_match_complete_json() {
    let fixture = Fixture::new();
    fixture.established();
    fixture.entity("ada", "Ada", &["Countess"], "person", false);
    for (facet, description) in [("math", "Mathematician"), ("work", "Engineer")] {
        fixture.facet(facet, false);
        fixture.relationship(facet, "ada", description);
    }
    let prior = day_ago(2);
    let today = day_ago(0);
    fixture.activities("math", &prior, &[attendee("z-math", 20, "ada")]);
    fixture.activities("work", &today, &[attendee("a-work", 30, "ada")]);

    let (_, _, full) = get("/api/profile/ada", &fixture).await;
    let mut full = body_json(&full);
    full.as_object_mut()
        .expect("full object")
        .remove("generated_at");
    assert_eq!(
        full,
        json!({
            "entity_id":"ada","name":"Ada","type":"person","aka":["Countess"],"is_self":false,
            "facets":["math","work"],"description":"Mathematician | Engineer",
            "cadence":{"recent_interactions_count_30d":2,"last_seen":today,"avg_interval_days":2.0,"gone_quiet_since":null},
            "open_with_them":[],"closed_with_them_30d":[],"decisions_involving_them":[],
            "sources":[
                {"facet":"math","day":prior,"activity_id":"z-math","field":"participation","created_at":20},
                {"facet":"work","day":today,"activity_id":"a-work","field":"participation","created_at":30}
            ]
        })
    );

    let (_, _, brief) = get("/api/profile/ada/brief", &fixture).await;
    assert_eq!(
        body_json(&brief),
        json!({"entity_id":"ada","name":"Ada","type":"person","description":"Mathematician | Engineer","last_seen":today,"open_loop_count":0,"decisions_count_30d":0})
    );
    let (_, _, cadence) = get("/api/profile/ada/cadence", &fixture).await;
    assert_eq!(
        body_json(&cadence),
        json!({"recent_interactions_count_30d":2,"last_seen":today,"avg_interval_days":2.0,"gone_quiet_since":null})
    );
    let (_, _, active) = get("/api/profiles/active", &fixture).await;
    assert_eq!(body_json(&active), json!({"items":["ada"],"total":1}));
}

#[tokio::test]
async fn full_uses_enabled_ledger_folds_but_all_declared_profile_data() {
    let fixture = Fixture::new();
    fixture.established();
    fixture.entity("pat", "Pat", &[], "person", false);
    fixture.facet("work", false);
    fixture.facet("muted", true);
    fixture.relationship("work", "pat", "Work relationship");
    fixture.relationship("muted", "pat", "Muted relationship");
    let open_day = day_ago(5);
    let recent_commit_day = day_ago(4);
    let recent_close_day = day_ago(3);
    let stale_commit_day = day_ago(50);
    let stale_close_day = day_ago(40);
    let source_day = day_ago(1);
    let today = day_ago(0);
    let open_created_at = timestamp_ago(5);
    let stale_created_at = timestamp_ago(50);
    fixture.activities(
        "work",
        &open_day,
        &[json!({"id":"open","created_at":open_created_at,"participation":[{"entity_id":"pat","role":"attendee"}],"commitments":[{"owner":"Owner","owner_entity_id":"owner","counterparty":"Pat","counterparty_entity_id":"pat","action":"send notes","when":"tomorrow","context":"Send meeting notes"}]})],
    );
    fixture.activities(
        "work",
        &recent_commit_day,
        &[json!({"id":"recent-commit","created_at":timestamp_ago(4),"commitments":[{"owner":"Owner","owner_entity_id":"owner","counterparty":"Pat","counterparty_entity_id":"pat","action":"ship report"}]})],
    );
    fixture.activities(
        "work",
        &recent_close_day,
        &[json!({"id":"recent-close","created_at":timestamp_ago(3),"closures":[{"owner_entity_id":"owner","counterparty_entity_id":"pat","action":"ship the report"}]})],
    );
    fixture.activities(
        "work",
        &stale_commit_day,
        &[
            json!({"id":"stale-commit","created_at":stale_created_at,"commitments":[{"owner":"Owner","owner_entity_id":"owner","counterparty":"Pat","counterparty_entity_id":"pat","action":"old task"}]}),
            json!({"id":"old-decision","created_at":stale_created_at,"decisions":[{"owner":"Pat","owner_entity_id":"pat","action":"old decision","context":"still relevant"}]}),
        ],
    );
    fixture.activities(
        "work",
        &stale_close_day,
        &[json!({"id":"stale-close","created_at":timestamp_ago(40),"closures":[{"owner_entity_id":"owner","counterparty_entity_id":"pat","action":"old task"}]})],
    );
    fixture.activities(
        "muted",
        &today,
        &[json!({"id":"muted","created_at":1,"participation":[{"entity_id":"pat","role":"attendee"}],"commitments":[{"owner":"Owner","owner_entity_id":"owner","counterparty":"Pat","counterparty_entity_id":"pat","action":"hidden task"}],"decisions":[{"owner":"Pat","owner_entity_id":"pat","action":"hidden decision"}]})],
    );
    fixture.activities(
        "work",
        &source_day,
        &[
            attendee("z-source", timestamp_ago(1), "pat"),
            attendee("a-source", timestamp_ago(1), "pat"),
        ],
    );
    fixture.activities(
        "work",
        &today,
        &[attendee("z-last", timestamp_ago(0), "pat")],
    );

    let (_, _, full) = get("/api/profile/pat", &fixture).await;
    let full = body_json(&full);
    assert_eq!(
        full["description"],
        "Muted relationship | Work relationship"
    );
    assert_eq!(full["cadence"]["recent_interactions_count_30d"], 5);
    let open_id = folded_id(&["owner", "send notes", "pat"]);
    assert_eq!(
        full["open_with_them"],
        json!([{
            "id":open_id,
            "state":"open",
            "owner":"Owner",
            "owner_entity_id":"owner",
            "counterparty":"Pat",
            "counterparty_entity_id":"pat",
            "action":"send notes",
            "summary":"send notes",
            "when":"tomorrow",
            "context":"Send meeting notes",
            "opened_at":open_created_at,
            "closed_at":null,
            "age_days":5,
            "sources":[{
                "facet":"work",
                "day":open_day,
                "activity_id":"open",
                "field":"commitments",
                "created_at":open_created_at
            }]
        }])
    );
    assert_eq!(
        full["closed_with_them_30d"]
            .as_array()
            .expect("closed")
            .len(),
        1
    );
    assert_eq!(full["closed_with_them_30d"][0]["action"], "ship report");
    let decision_id = folded_id(&["pat", "old decision", &stale_commit_day]);
    assert_eq!(
        full["decisions_involving_them"],
        json!([{
            "id":decision_id,
            "owner":"Pat",
            "owner_entity_id":"pat",
            "action":"old decision",
            "context":"still relevant",
            "day":stale_commit_day,
            "created_at":stale_created_at,
            "source":{
                "facet":"work",
                "day":stale_commit_day,
                "activity_id":"old-decision",
                "field":"decisions",
                "created_at":stale_created_at
            }
        }])
    );
    let sources = full["sources"].as_array().expect("sources");
    let keys = sources
        .iter()
        .map(|source| {
            (
                source["day"].as_str().unwrap(),
                source["facet"].as_str().unwrap(),
                source["activity_id"].as_str().unwrap(),
            )
        })
        .collect::<Vec<_>>();
    assert_eq!(
        keys,
        vec![
            (open_day.as_str(), "work", "open"),
            (source_day.as_str(), "work", "a-source"),
            (source_day.as_str(), "work", "z-source"),
            (today.as_str(), "muted", "muted"),
            (today.as_str(), "work", "z-last"),
        ]
    );

    let (_, _, brief) = get("/api/profile/pat/brief", &fixture).await;
    let brief = body_json(&brief);
    let fields = brief.as_object().expect("brief object");
    assert_eq!(fields.len(), 7);
    assert_eq!(brief["open_loop_count"], 1);
    assert_eq!(brief["decisions_count_30d"], 0);
    assert!(brief.get("is_self").is_none());
    assert!(brief.get("generated_at").is_none());
}

#[tokio::test]
async fn muted_only_profile_data_stays_visible_while_muted_ledger_data_is_excluded() {
    let fixture = Fixture::new();
    fixture.established();
    fixture.entity("muted_only", "Muted Only", &[], "person", false);
    fixture.facet("muted", true);
    fixture.relationship("muted", "muted_only", "Muted relationship");
    fixture.activities(
        "muted",
        &day_ago(0),
        &[json!({"id":"muted-only","created_at":1,"participation":[{"entity_id":"muted_only","role":"attendee"}],"commitments":[{"owner":"Owner","owner_entity_id":"owner","counterparty":"Muted Only","counterparty_entity_id":"muted_only","action":"hidden task"}],"closures":[{"owner_entity_id":"owner","counterparty_entity_id":"muted_only","action":"hidden task"}],"decisions":[{"owner":"Muted Only","owner_entity_id":"muted_only","action":"hidden decision"}]})],
    );

    let (_, _, full) = get("/api/profile/muted_only", &fixture).await;
    assert_eq!(
        body_json(&full)["facets"],
        json!(["muted"]),
        "declared muted relationship remains visible"
    );
    let full = body_json(&full);
    assert_eq!(full["description"], "Muted relationship");
    assert_eq!(full["cadence"]["recent_interactions_count_30d"], 1);
    assert_eq!(full["open_with_them"], json!([]));
    assert_eq!(full["closed_with_them_30d"], json!([]));
    assert_eq!(full["decisions_involving_them"], json!([]));
}

#[tokio::test]
async fn active_list_is_attendee_only_sorted_deduplicated_and_windowed() {
    let fixture = Fixture::new();
    fixture.established();
    fixture.facet("work", false);
    fixture.activities(
        "work",
        &day_ago(0),
        &[json!({"id":"today","participation":[
            {"entity_id":"b","role":"attendee"},
            {"entity_id":"a","role":"attendee"},
            {"entity_id":"a","role":"attendee"},
            {"entity_id":"mention","role":"mentioned"}
        ]})],
    );
    fixture.activities("work", &day_ago(6), &[attendee("boundary", 1, "boundary")]);
    fixture.activities("work", &day_ago(7), &[attendee("outside", 1, "outside")]);
    let (_, _, body) = get("/api/profiles/active?window_days=7", &fixture).await;
    assert_eq!(
        body_json(&body),
        json!({"items":["a","b","boundary"],"total":3})
    );
}

#[tokio::test]
async fn active_pagination_matches_legacy_clamps_and_hundred_item_walk() {
    let fixture = Fixture::new();
    fixture.established();
    fixture.facet("work", false);
    let ids = (0..101)
        .map(|index| format!("person{index:03}"))
        .collect::<Vec<_>>();
    let participation = ids
        .iter()
        .map(|id| json!({"entity_id":id,"role":"attendee"}))
        .collect::<Vec<_>>();
    fixture.activities(
        "work",
        &day_ago(0),
        &[json!({"id":"crowd","participation":participation})],
    );

    let (_, _, default_page) = get("/api/profiles/active", &fixture).await;
    let default_page = body_json(&default_page);
    assert_eq!(default_page["total"], 101);
    assert_eq!(default_page["items"].as_array().expect("items").len(), 20);
    assert_eq!(default_page["items"][0], "person000");
    let (_, _, malformed) = get("/api/profiles/active?limit=bad&offset=nope", &fixture).await;
    assert_eq!(body_json(&malformed), default_page);
    let (_, _, clamped) = get("/api/profiles/active?limit=1000&offset=-4", &fixture).await;
    assert_eq!(
        body_json(&clamped)["items"]
            .as_array()
            .expect("items")
            .len(),
        100
    );

    let mut walked = Vec::new();
    for offset in [0, 100] {
        let (_, _, page) = get(
            &format!("/api/profiles/active?limit=100&offset={offset}"),
            &fixture,
        )
        .await;
        walked.extend(
            body_json(&page)["items"]
                .as_array()
                .expect("items")
                .iter()
                .map(|value| value.as_str().expect("ID").to_owned()),
        );
    }
    assert_eq!(walked, ids);
}

#[tokio::test]
async fn active_window_days_validation_uses_legacy_error_envelopes() {
    let fixture = Fixture::new();
    fixture.established();
    let (status, _, bytes) = get("/api/profiles/active?window_days=bad", &fixture).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(
        body_json(&bytes)["detail"],
        "window_days must be an integer"
    );
    for value in ["0", "-1", "-2"] {
        let (status, _, bytes) = get(
            &format!("/api/profiles/active?window_days={value}"),
            &fixture,
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{value}");
        assert_eq!(
            body_json(&bytes),
            json!({"error":"I couldn't use one of those values.","reason_code":"invalid_request_value","detail":"window_days must be positive"})
        );
    }
}
