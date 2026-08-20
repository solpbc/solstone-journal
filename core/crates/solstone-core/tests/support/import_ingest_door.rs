// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use serde_json::Value;

const DOOR_ROUTES: &str = include_str!("../../../../fixtures/import_ingest_door_routes.json");

pub(crate) fn door(method: &str, key_prefix: &str, kind: &str, area: &str) -> String {
    let fixture: Value = serde_json::from_str(DOOR_ROUTES).expect("door fixture");
    let requested = format!("/app/import/journal/{key_prefix}/{kind}/{area}");
    let matches = fixture["rules"]
        .as_array()
        .expect("rules")
        .iter()
        .filter(|rule| {
            rule["methods"]
                .as_array()
                .into_iter()
                .flatten()
                .any(|candidate| candidate.as_str() == Some(method))
                && rule["rule"]
                    .as_str()
                    .is_some_and(|rule| matches_rule(rule, &requested))
        })
        .collect::<Vec<_>>();
    assert_eq!(
        matches.len(),
        1,
        "expected exactly one {method} {kind}/{area} door rule"
    );
    materialize(
        matches[0]["rule"].as_str().expect("door rule is a string"),
        key_prefix,
        area,
    )
}

fn matches_rule(rule: &str, path: &str) -> bool {
    let rule_segments = rule.split('/').collect::<Vec<_>>();
    let path_segments = path.split('/').collect::<Vec<_>>();
    rule_segments.len() == path_segments.len()
        && rule_segments
            .iter()
            .zip(path_segments)
            .all(|(rule_segment, path_segment)| {
                placeholder(rule_segment).is_some_and(|_| !path_segment.is_empty())
                    || placeholder(rule_segment).is_none() && rule_segment == &path_segment
            })
}

fn placeholder(segment: &str) -> Option<&str> {
    segment
        .strip_prefix('<')
        .and_then(|segment| segment.strip_suffix('>'))
        .filter(|name| !name.is_empty())
}

fn materialize(rule: &str, key_prefix: &str, area: &str) -> String {
    let path = rule
        .split('/')
        .map(|segment| match placeholder(segment) {
            Some("key_prefix") => key_prefix,
            Some("area") => area,
            Some(name) => panic!("unsupported door-rule placeholder <{name}>"),
            None => segment,
        })
        .collect::<Vec<_>>()
        .join("/");
    assert!(
        !path.contains(['<', '>']),
        "unsubstituted placeholder in door path {path}"
    );
    path
}

#[test]
fn generic_manifest_and_literal_ingest_rules_materialize_concrete_doors() {
    assert_eq!(
        door("GET", "remote-i", "manifest", "entities"),
        "/app/import/journal/remote-i/manifest/entities"
    );
    assert_eq!(
        door("POST", "remote-i", "ingest", "config"),
        "/app/import/journal/remote-i/ingest/config"
    );
}
