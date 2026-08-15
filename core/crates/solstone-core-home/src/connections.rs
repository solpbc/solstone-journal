// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Pure connections-card projection.

use serde_json::{Value, json};
use solstone_core_entities::{ATTENDANCE_KINDS, ENTITIES_COPY};

const CONNECTION_KIND_KEYS: [&str; 15] = [
    "works-with",
    "works-at",
    "reports-to",
    "family-of",
    "knows",
    "uses",
    "created",
    "decided-with",
    "committed-to",
    "spoke-with",
    "mentioned",
    "messaged-with",
    "scheduled-with",
    "party-of",
    "other",
];

pub fn build_connections_card(
    principal: Result<Option<Value>, ()>,
    network: Result<Value, ()>,
) -> Value {
    let Ok(principal) = principal else {
        return json!({"state":"unavailable"});
    };
    let Some(principal) = principal else {
        return json!({"state":"empty"});
    };
    if principal
        .get("id")
        .and_then(Value::as_str)
        .is_none_or(str::is_empty)
    {
        return json!({"state":"empty"});
    }
    let Ok(network) = network else {
        return json!({"state":"unavailable"});
    };
    let Some(neighbors) = network.get("neighbors").and_then(Value::as_array) else {
        return json!({"state":"empty"});
    };
    if neighbors.is_empty() {
        return json!({"state":"empty"});
    }
    let mut attendance_kinds = ATTENDANCE_KINDS.to_vec();
    attendance_kinds.sort_unstable();
    json!({"state":"ok","neighbors":neighbors.iter().filter_map(Value::as_object).map(trim_neighbor).collect::<Vec<_>>(),"total":network.get("total_neighbors").and_then(Value::as_i64).unwrap_or(0),"kind_words":kind_words(),"attendance_kinds":attendance_kinds})
}

fn kind_words() -> Value {
    let mut composed = ENTITIES_COPY
        .get("ENT_CONN_KIND_WORDS")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    if let Some(chips) = ENTITIES_COPY
        .get("ENT_CONN_KIND_CHIP_WORDS")
        .and_then(Value::as_object)
    {
        composed.extend(chips.clone());
    }
    Value::Object(
        CONNECTION_KIND_KEYS
            .into_iter()
            .filter_map(|key| composed.remove(key).map(|value| (key.to_owned(), value)))
            .collect(),
    )
}
fn trim_neighbor(row: &serde_json::Map<String, Value>) -> Value {
    let evidence = row
        .get("evidence")
        .and_then(Value::as_array)
        .and_then(|rows| rows.first())
        .and_then(Value::as_object);
    let mut kinds = row
        .get("kinds")
        .and_then(Value::as_object)
        .map(|kinds| {
            kinds
                .iter()
                .filter_map(|(kind, value)| {
                    let value = value.as_object()?;
                    let count = value.get("count").and_then(Value::as_i64).unwrap_or(0);
                    let weighted = value.get("weighted").and_then(Value::as_f64).unwrap_or(0.0);
                    (count > 0 || weighted > 0.0)
                        .then(|| json!({"kind":kind,"count":count,"weighted":weighted}))
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    kinds.sort_by(|left, right| {
        right["weighted"]
            .as_f64()
            .unwrap_or(0.0)
            .total_cmp(&left["weighted"].as_f64().unwrap_or(0.0))
            .then_with(|| left["kind"].as_str().cmp(&right["kind"].as_str()))
    });
    json!({"entity_id":row.get("entity_id"),"name":row.get("name"),"evidence_class":row.get("evidence_class"),"count":row.get("count").and_then(Value::as_i64).unwrap_or(0),"last_seen":row.get("last_seen"),"kinds":kinds,"latest_label":evidence.and_then(|row| row.get("label")),"latest_kind":evidence.and_then(|row| row.get("kind")),"latest_day":evidence.and_then(|row| row.get("day"))})
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use super::*;

    #[test]
    fn injected_read_failures_are_unavailable_and_absence_is_empty() {
        assert_eq!(
            build_connections_card(Err(()), Ok(json!({}))),
            json!({"state":"unavailable"})
        );
        assert_eq!(
            build_connections_card(Ok(None), Ok(json!({}))),
            json!({"state":"empty"})
        );
        assert_eq!(
            build_connections_card(Ok(Some(json!({"id":"owner"}))), Err(())),
            json!({"state":"unavailable"})
        );
        assert_eq!(
            build_connections_card(Ok(Some(json!({}))), Ok(json!({}))),
            json!({"state":"empty"})
        );
        assert_eq!(
            build_connections_card(Ok(Some(json!({"id":"owner"}))), Ok(json!({}))),
            json!({"state":"empty"})
        );
        assert_eq!(
            build_connections_card(
                Ok(Some(json!({"id":"owner"}))),
                Ok(json!({"neighbors":"not a list"})),
            ),
            json!({"state":"empty"})
        );
        assert_eq!(
            build_connections_card(Ok(Some(json!({"id":"owner"}))), Ok(json!({"neighbors":[]})),),
            json!({"state":"empty"})
        );
    }

    #[test]
    fn populated_card_keeps_the_source_derived_shape() {
        let card = build_connections_card(
            Ok(Some(json!({"id":"owner"}))),
            Ok(
                json!({"total_neighbors":4,"neighbors":[{"entity_id":"person:one","name":"One","evidence_class":"direct","count":4,"last_seen":"20260602","evidence":[{"label":"meeting","kind":"note","day":"20260601"}],"kinds":{"mentioned":{"count":0,"weighted":0},"works-with":{"count":1,"weighted":2},"created":{"count":3,"weighted":2},"knows":{"count":1,"weighted":3}}}]}),
            ),
        );
        assert_eq!(card["state"], "ok");
        assert_eq!(
            card.as_object()
                .unwrap()
                .keys()
                .cloned()
                .collect::<BTreeSet<_>>(),
            [
                "state",
                "neighbors",
                "total",
                "kind_words",
                "attendance_kinds"
            ]
            .into_iter()
            .map(str::to_owned)
            .collect(),
        );
        assert_eq!(card["total"], 4);
        assert_eq!(
            card["attendance_kinds"],
            json!(["attended-with", "co-present", "scheduled-with"])
        );
        assert!(card["kind_words"].get("attended-with").is_none());
        assert_eq!(card["kind_words"]["committed-to"], "commitments");
        assert_eq!(card["kind_words"]["mentioned"], "mentions");
        assert_eq!(
            card["neighbors"][0]["kinds"],
            json!([{ "kind":"knows", "count":1, "weighted":3.0 }, { "kind":"created", "count":3, "weighted":2.0 }, { "kind":"works-with", "count":1, "weighted":2.0 }])
        );
        assert_eq!(card["neighbors"][0]["latest_label"], "meeting");
    }

    #[test]
    fn connection_projection_preserves_edge_case_shapes() {
        let card = build_connections_card(
            Ok(Some(json!({"id":"owner"}))),
            Ok(json!({
                "total_neighbors": 9,
                "neighbors": [
                    "discarded",
                    {"entity_id":"one","kinds":{"z":{"count":0,"weighted":2.5},"a":{"count":1,"weighted":2.5},"drop":{"count":0,"weighted":0}},"evidence":"not a list"}
                ]
            })),
        );
        assert_eq!(card["total"], 9);
        assert_eq!(card["neighbors"].as_array().unwrap().len(), 1);
        assert_eq!(
            card["neighbors"][0]["kinds"],
            json!([{"kind":"a","count":1,"weighted":2.5},{"kind":"z","count":0,"weighted":2.5}])
        );
        assert_eq!(card["neighbors"][0]["latest_label"], Value::Null);
        assert_eq!(card["neighbors"][0]["latest_kind"], Value::Null);
        assert_eq!(card["neighbors"][0]["latest_day"], Value::Null);

        let kinds = build_connections_card(
            Ok(Some(json!({"id":"owner"}))),
            Ok(json!({"neighbors":[{"kinds":"invalid"}]})),
        );
        assert_eq!(kinds["neighbors"][0]["kinds"], json!([]));
        assert_eq!(kinds.as_object().unwrap().len(), 5);
        assert_eq!(kinds["kind_words"].as_object().unwrap().len(), 15);
    }
}
