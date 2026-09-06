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
        .filter(|row| !is_placeholder_speaker(row) && !is_bare_word_name(row))
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

/// A neighbor whose whole display name is one bare lowercase word (`make`,
/// `just`, `think`, `build`) is a semantic-extraction artifact, not somebody in
/// the owner's life. The test is structural rather than a stop-word list: one
/// whitespace-separated token, no uppercase letter, no digit. A capitalised
/// single name (`Ada`) and every multi-word name stay, so the rule can never
/// take a real connection off the shelf. Applies regardless of evidence class,
/// and covers both the shelf and the "mentioned in your journal" disclosure,
/// which read the same list. X-02.
fn is_bare_word_name(row: &serde_json::Map<String, Value>) -> bool {
    let name = row.get("name").and_then(Value::as_str).unwrap_or("").trim();
    let mut tokens = name.split_whitespace();
    let Some(only) = tokens.next() else {
        return false;
    };
    if tokens.next().is_some() {
        return false;
    }
    !only
        .chars()
        .any(|character| character.is_uppercase() || character.is_numeric())
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
    /// journal (`x-home-pulse.json`). Names and evidence classes are verbatim.
    fn captured_burn_in_neighbors() -> Value {
        json!({"total_neighbors":12,"neighbors":[
            {"entity_id":"gallery_at_reunion_the","name":"Gallery At Reunion (The)","evidence_class":"semantic","count":4731,"last_seen":"20260905","kinds":{}},
            {"entity_id":"just","name":"just","evidence_class":"semantic","count":3249,"last_seen":"20260905","kinds":{}},
            {"entity_id":"speaker_1","name":"Speaker 1","evidence_class":"mixed","count":2591,"last_seen":"20260905","kinds":{}},
            {"entity_id":"think","name":"think","evidence_class":"semantic","count":2010,"last_seen":"20260904","kinds":{}},
            {"entity_id":"more","name":"more","evidence_class":"semantic","count":1606,"last_seen":"20260904","kinds":{}},
            {"entity_id":"speaker_2","name":"Speaker 2","evidence_class":"mixed","count":1276,"last_seen":"20260905","kinds":{}},
            {"entity_id":"make","name":"make","evidence_class":"mixed","count":1333,"last_seen":"20260904","kinds":{}},
            {"entity_id":"own","name":"Own","evidence_class":"semantic","count":991,"last_seen":"20260904","kinds":{}},
            {"entity_id":"build","name":"build","evidence_class":"semantic","count":890,"last_seen":"20260904","kinds":{}},
            {"entity_id":"whole","name":"Whole","evidence_class":"semantic","count":815,"last_seen":"20260904","kinds":{}},
            {"entity_id":"company","name":"Company","evidence_class":"semantic","count":656,"last_seen":"20260904","kinds":{}},
            {"entity_id":"able","name":"Able","evidence_class":"semantic","count":612,"last_seen":"20260904","kinds":{}}
        ]})
    }

    #[test]
    fn bare_lowercase_words_are_not_connections() {
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

        // Every bare lowercase token goes, and the two unnamed voice clusters
        // stay gone. `Own`, `Whole`, `Company` and `Able` survive on purpose:
        // they are capitalised, so no structural test separates them from a
        // real one-name person, and the rule refuses to guess. See the sweep
        // report's open question.
        assert_eq!(
            names,
            vec![
                "Gallery At Reunion (The)".to_owned(),
                "Own".to_owned(),
                "Whole".to_owned(),
                "Company".to_owned(),
                "Able".to_owned(),
            ]
        );
        for dropped in ["just", "think", "more", "make", "build"] {
            assert!(
                !names.iter().any(|name| name == dropped),
                "{dropped} is a bare word, not a connection"
            );
        }
    }

    #[test]
    fn the_bare_word_rule_keeps_every_real_name_shape() {
        // Capitalised single name, multi-word name, hyphenated, accented,
        // digit-bearing, and a lowercase name that is not alone in its field.
        for kept in [
            "Ada", "Ada Lovelace", "jean-luc picard", "Élan", "studio 54", "de Havilland",
        ] {
            assert!(
                !is_bare_word_name(json!({"name":kept}).as_object().unwrap()),
                "{kept} is a real name and must stay on the shelf"
            );
        }
        for dropped in ["make", "just", "think", "more", "own", "build", "whole", "company", "able"]
        {
            assert!(
                is_bare_word_name(json!({"name":dropped}).as_object().unwrap()),
                "{dropped} is a bare word"
            );
        }
        // A missing or blank name is somebody else's problem, not this rule's.
        assert!(!is_bare_word_name(json!({}).as_object().unwrap()));
        assert!(!is_bare_word_name(json!({"name":"   "}).as_object().unwrap()));
    }
}
