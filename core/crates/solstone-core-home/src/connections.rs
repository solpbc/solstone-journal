// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Pure connections-card projection.

use serde_json::{Value, json};
use solstone_core_entities::{ATTENDANCE_KINDS, ENTITIES_COPY, compose_connections_horizon_note};
use solstone_core_facets::ConnectionsHorizon;

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
    horizon: Option<ConnectionsHorizon>,
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
    // The shelf's job is to summarize who is in the owner's life. An unnamed
    // voice cluster is a speakers task, not a connection, so it is dropped here
    // the way the speakers surface drops it. X-02.
    let named = neighbors
        .iter()
        .filter_map(Value::as_object)
        .filter(|row| !is_placeholder_speaker(row) && !is_mention_only_word(row))
        .collect::<Vec<_>>();
    if named.is_empty() {
        return json!({"state":"unnamed"});
    }
    let mut attendance_kinds = ATTENDANCE_KINDS.to_vec();
    attendance_kinds.sort_unstable();
    let mut card = json!({"state":"ok","neighbors":named.iter().copied().map(trim_neighbor).collect::<Vec<_>>(),"total":network.get("total_neighbors").and_then(Value::as_i64).unwrap_or(0),"kind_words":kind_words(),"attendance_kinds":attendance_kinds});
    if let Some(horizon) = horizon {
        let object = card.as_object_mut().expect("ok card is an object");
        object.insert("horizon_day".to_owned(), Value::String(horizon.day));
        object.insert(
            "horizon_note".to_owned(),
            Value::String(compose_connections_horizon_note(horizon.earlier_days)),
        );
    }
    card
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
/// The speakers surface hides a voice it has no name for; the same cluster
/// arriving here as `Speaker 1` is the same non-answer. Mirrors the
/// `PLACEHOLDER_SPEAKER_NAME` rule in `convey-shell/assets/speakers/workspace.html`.
fn is_placeholder_speaker(row: &serde_json::Map<String, Value>) -> bool {
    let name = row.get("name").and_then(Value::as_str).unwrap_or("").trim();
    let id = row
        .get("entity_id")
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim();
    is_placeholder_speaker_token(name, ' ') || is_placeholder_speaker_token(id, '_')
}

fn is_placeholder_speaker_token(value: &str, separator: char) -> bool {
    let lower = value.to_lowercase();
    let Some(rest) = lower.strip_prefix("speaker") else {
        return false;
    };
    let rest = rest.trim_start_matches(separator);
    !rest.is_empty() && rest.bytes().all(|byte| byte.is_ascii_digit())
}

/// A neighbor whose whole display name is one word and whose evidence is
/// almost entirely `mentioned` is a semantic-extraction artifact, not somebody
/// in the owner's life: the owner never spoke with it, attended anything with
/// it, was co-present with it, or messaged it. The test is the evidence rather
/// than the capitalisation, because capitalisation separated `Own` and `Whole`
/// from `make` and `just` while the journal says the same thing about all four.
/// Every multi-word name stays, and so does every one-word name with real
/// interaction evidence. A row carrying no kind counts at all is left alone —
/// with no evidence either way the rule refuses to guess. Covers both the shelf
/// and the "mentioned in your journal" disclosure, which read the same list.
/// X-02.
fn is_mention_only_word(row: &serde_json::Map<String, Value>) -> bool {
    let name = row.get("name").and_then(Value::as_str).unwrap_or("").trim();
    if name.split_whitespace().count() != 1 {
        return false;
    }
    let Some(kinds) = row.get("kinds").and_then(Value::as_object) else {
        return false;
    };
    let mut total: i64 = 0;
    let mut mentioned: i64 = 0;
    for (kind, value) in kinds {
        let count = value
            .as_object()
            .and_then(|value| value.get("count"))
            .and_then(Value::as_i64)
            .unwrap_or(0)
            .max(0);
        total += count;
        if kind == "mentioned" {
            mentioned += count;
        }
    }
    total > 0 && mentioned * 100 >= total * 99
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
            build_connections_card(Err(()), Ok(json!({})), None),
            json!({"state":"unavailable"})
        );
        assert_eq!(
            build_connections_card(Ok(None), Ok(json!({})), None),
            json!({"state":"empty"})
        );
        assert_eq!(
            build_connections_card(Ok(Some(json!({"id":"owner"}))), Err(()), None),
            json!({"state":"unavailable"})
        );
        assert_eq!(
            build_connections_card(Ok(Some(json!({}))), Ok(json!({})), None),
            json!({"state":"empty"})
        );
        assert_eq!(
            build_connections_card(Ok(Some(json!({"id":"owner"}))), Ok(json!({})), None),
            json!({"state":"empty"})
        );
        assert_eq!(
            build_connections_card(
                Ok(Some(json!({"id":"owner"}))),
                Ok(json!({"neighbors":"not a list"})),
                None,
            ),
            json!({"state":"empty"})
        );
        assert_eq!(
            build_connections_card(
                Ok(Some(json!({"id":"owner"}))),
                Ok(json!({"neighbors":[]})),
                None,
            ),
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
            None,
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
            None,
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
            None,
        );
        assert_eq!(kinds["neighbors"][0]["kinds"], json!([]));
        assert_eq!(kinds.as_object().unwrap().len(), 5);
        assert_eq!(kinds["kind_words"].as_object().unwrap().len(), 15);
    }

    fn ok_network() -> Value {
        json!({"total_neighbors":1,"neighbors":[{"entity_id":"person:one","name":"One","count":1}]})
    }

    #[test]
    fn populated_card_with_horizon_adds_two_keys() {
        let horizon = ConnectionsHorizon {
            day: "20260301".to_owned(),
            earlier_days: 3,
        };
        let card = build_connections_card(
            Ok(Some(json!({"id":"owner"}))),
            Ok(ok_network()),
            Some(horizon),
        );
        assert_eq!(card["state"], "ok");
        assert_eq!(card.as_object().unwrap().len(), 7);
        assert_eq!(card["horizon_day"], "20260301");
        assert_eq!(card["horizon_note"], compose_connections_horizon_note(3));
        assert!(card["horizon_note"].as_str().unwrap().contains("{day}"));
        assert!(!card["horizon_note"].as_str().unwrap().contains("{n}"));
    }

    #[test]
    fn horizon_note_is_byte_identical_for_one_and_three() {
        for earlier_days in [1_usize, 3] {
            let card = build_connections_card(
                Ok(Some(json!({"id":"owner"}))),
                Ok(ok_network()),
                Some(ConnectionsHorizon {
                    day: "20260301".to_owned(),
                    earlier_days,
                }),
            );
            assert_eq!(
                card["horizon_note"].as_str().unwrap(),
                compose_connections_horizon_note(earlier_days)
            );
        }
    }
    #[test]
    fn unnamed_voice_clusters_never_reach_the_shelf() {
        let network = json!({"total_neighbors":3,"neighbors":[
            {"entity_id":"speaker_1","name":"Speaker 1","evidence_class":"semantic","count":2591,"last_seen":"20260905","kinds":{}},
            {"entity_id":"speaker_2","name":"SPEAKER 2","evidence_class":"mixed","count":1276,"last_seen":"20260905","kinds":{}},
            {"entity_id":"person:ada","name":"Ada","evidence_class":"direct","count":4,"last_seen":"20260602","kinds":{}}
        ]});
        let card = build_connections_card(Ok(Some(json!({"id":"owner"}))), Ok(network), None);
        let names = card["neighbors"]
            .as_array()
            .unwrap()
            .iter()
            .map(|row| row["name"].as_str().unwrap().to_owned())
            .collect::<Vec<_>>();
        assert_eq!(names, vec!["Ada".to_owned()]);

        // A shelf with nothing but unnamed clusters says so; it does not show
        // them and it does not read as a journal with nobody in it.
        let only_placeholders = json!({"total_neighbors":1,"neighbors":[
            {"entity_id":"speaker_1","name":"Speaker 1","evidence_class":"semantic","count":2591,"last_seen":"20260905","kinds":{}}
        ]});
        assert_eq!(
            build_connections_card(Ok(Some(json!({"id":"owner"}))), Ok(only_placeholders), None),
            json!({"state":"unnamed"})
        );

        // A real name that merely begins with the word is not a placeholder.
        assert!(!is_placeholder_speaker(
            json!({"entity_id":"person:speakerbox","name":"Speaker Deck"})
                .as_object()
                .unwrap()
        ));
    }

    /// The twelve neighbors the burn-in review captured off the founder's own
    /// journal (`x-home-pulse.json`). Names, evidence classes and kind counts
    /// are verbatim; `kinds` carries the source shape the projection reads (a
    /// map of kind to count and weight), which the captured response renders
    /// as the sorted array.
    fn captured_burn_in_neighbors() -> Value {
        json!({"total_neighbors":12,"neighbors":[
            {"entity_id":"gallery_at_reunion_the","name":"Gallery At Reunion (The)","evidence_class":"semantic","count":4731,"last_seen":"20260905","kinds":{"mentioned":{"count":4731,"weighted":34945.681165493996}}},
            {"entity_id":"just","name":"just","evidence_class":"semantic","count":3249,"last_seen":"20260905","kinds":{"mentioned":{"count":3249,"weighted":11745.97663366838}}},
            {"entity_id":"speaker_1","name":"Speaker 1","evidence_class":"mixed","count":2591,"last_seen":"20260905","kinds":{"spoke-with":{"count":2547,"weighted":6852.98353624809},"attended-with":{"count":37,"weighted":17.136822248866938},"mentioned":{"count":4,"weighted":8.267718445734918},"co-present":{"count":3,"weighted":1.9245454626919196}}},
            {"entity_id":"think","name":"think","evidence_class":"semantic","count":2010,"last_seen":"20260904","kinds":{"mentioned":{"count":2010,"weighted":5652.853687785307}}},
            {"entity_id":"more","name":"more","evidence_class":"semantic","count":1606,"last_seen":"20260904","kinds":{"mentioned":{"count":1606,"weighted":3855.284950662275}}},
            {"entity_id":"speaker_2","name":"Speaker 2","evidence_class":"mixed","count":1276,"last_seen":"20260905","kinds":{"spoke-with":{"count":1221,"weighted":3622.505265905339},"attended-with":{"count":53,"weighted":23.41166025606998},"mentioned":{"count":2,"weighted":3.812261932217838}}},
            {"entity_id":"make","name":"make","evidence_class":"mixed","count":1333,"last_seen":"20260904","kinds":{"mentioned":{"count":1329,"weighted":2812.7364967997646},"co-present":{"count":4,"weighted":3.1815844406006253}}},
            {"entity_id":"own","name":"Own","evidence_class":"semantic","count":991,"last_seen":"20260904","kinds":{"mentioned":{"count":991,"weighted":2135.6252478430533}}},
            {"entity_id":"build","name":"build","evidence_class":"semantic","count":890,"last_seen":"20260904","kinds":{"mentioned":{"count":890,"weighted":2127.480501827544}}},
            {"entity_id":"whole","name":"Whole","evidence_class":"semantic","count":815,"last_seen":"20260904","kinds":{"mentioned":{"count":815,"weighted":1822.4314810784374}}},
            {"entity_id":"company","name":"Company","evidence_class":"semantic","count":656,"last_seen":"20260904","kinds":{"mentioned":{"count":656,"weighted":1816.1465022013701}}},
            {"entity_id":"able","name":"Able","evidence_class":"semantic","count":818,"last_seen":"20260904","kinds":{"mentioned":{"count":818,"weighted":1812.6602411961258}}}
        ]})
    }

    #[test]
    fn mention_only_words_are_not_connections() {
        let card = build_connections_card(
            Ok(Some(json!({"id":"owner"}))),
            Ok(captured_burn_in_neighbors()),
            None,
        );
        let names = card["neighbors"]
            .as_array()
            .unwrap()
            .iter()
            .map(|row| row["name"].as_str().unwrap().to_owned())
            .collect::<Vec<_>>();

        // One multi-word name survives. Every one-word name in the capture is
        // mentions and nothing else -- `make` is 1329 mentions of 1333, still
        // over the line -- so capitalisation buys `Own`, `Whole`, `Company`
        // and `Able` nothing. The two unnamed voice clusters stay gone on the
        // placeholder rule, not this one.
        assert_eq!(names, vec!["Gallery At Reunion (The)".to_owned()]);
        for dropped in [
            "just", "think", "more", "make", "build", "Own", "Whole", "Company", "Able",
        ] {
            assert!(
                !names.iter().any(|name| name == dropped),
                "{dropped} is a word the journal only ever mentioned, not a connection"
            );
        }
    }

    #[test]
    fn the_mention_only_rule_keeps_every_real_name_shape() {
        // A one-word name with any real interaction evidence stays, whatever
        // its case; every multi-word name stays even when it is pure mentions.
        for (kept, kinds) in [
            (
                "Ada",
                json!({"spoke-with":{"count":12},"mentioned":{"count":400}}),
            ),
            (
                "ada",
                json!({"attended-with":{"count":3},"mentioned":{"count":40}}),
            ),
            (
                "Sam",
                json!({"co-present":{"count":2},"mentioned":{"count":100}}),
            ),
            ("Ada Lovelace", json!({"mentioned":{"count":900}})),
            ("de Havilland", json!({"mentioned":{"count":900}})),
            ("studio 54", json!({"mentioned":{"count":900}})),
        ] {
            assert!(
                !is_mention_only_word(json!({"name":kept,"kinds":kinds}).as_object().unwrap()),
                "{kept} is a real connection and must stay on the shelf"
            );
        }
        // One word, mentions and nothing else -- or mentions past the 99% line.
        for (dropped, kinds) in [
            (
                "make",
                json!({"mentioned":{"count":1329},"co-present":{"count":4}}),
            ),
            ("just", json!({"mentioned":{"count":3249}})),
            ("Own", json!({"mentioned":{"count":991}})),
            ("Élan", json!({"mentioned":{"count":991}})),
            ("jean-luc", json!({"mentioned":{"count":991}})),
        ] {
            assert!(
                is_mention_only_word(json!({"name":dropped,"kinds":kinds}).as_object().unwrap()),
                "{dropped} is a word the journal only ever mentioned"
            );
        }
        // Just under the line: one spoken exchange in a hundred keeps a name.
        assert!(!is_mention_only_word(
            json!({"name":"Kai","kinds":{"mentioned":{"count":98},"spoke-with":{"count":2}}})
                .as_object()
                .unwrap()
        ));
        // No evidence either way is not evidence of absence: leave it alone.
        assert!(!is_mention_only_word(
            json!({"name":"make"}).as_object().unwrap()
        ));
        assert!(!is_mention_only_word(
            json!({"name":"make","kinds":{}}).as_object().unwrap()
        ));
        // A missing or blank name is somebody else's problem, not this rule's.
        assert!(!is_mention_only_word(json!({}).as_object().unwrap()));
        assert!(!is_mention_only_word(
            json!({"name":"   ","kinds":{"mentioned":{"count":5}}})
                .as_object()
                .unwrap()
        ));
    }
}
