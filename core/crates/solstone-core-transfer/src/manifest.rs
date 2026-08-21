// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Transfer-local manifest target and peer route helpers.

fn journal_door_path(key_prefix: &str, suffix: &str) -> String {
    format!("/app/import/journal/{key_prefix}/{suffix}")
}

/// Return the segment-ingest route for a journal source identified by `key_prefix`.
///
/// The server rule carries no day suffix. The day travels in the multipart metadata
/// body, so one constructor keeps segment upload callers aligned with the server.
pub(crate) fn segment_ingest_path(key_prefix: &str) -> String {
    journal_door_path(key_prefix, "ingest/segments")
}

pub(crate) fn entities_ingest_path(key_prefix: &str) -> String {
    journal_door_path(key_prefix, "ingest/entities")
}

pub(crate) fn facets_ingest_path(key_prefix: &str) -> String {
    journal_door_path(key_prefix, "ingest/facets")
}

pub(crate) fn imports_ingest_path(key_prefix: &str) -> String {
    journal_door_path(key_prefix, "ingest/imports")
}

pub(crate) fn config_ingest_path(key_prefix: &str) -> String {
    journal_door_path(key_prefix, "ingest/config")
}

pub(crate) fn manifest_path(key_prefix: &str, area: &str) -> String {
    journal_door_path(key_prefix, &format!("manifest/{area}"))
}

#[cfg(test)]
mod import_ingest_door_routes_tests {
    use super::{
        config_ingest_path, entities_ingest_path, facets_ingest_path, imports_ingest_path,
        manifest_path, segment_ingest_path,
    };

    use serde_json::Value;

    const ROUTES: &str = include_str!("../../../fixtures/import_ingest_door_routes.json");

    fn fixture() -> Value {
        serde_json::from_str(ROUTES).expect("route fixture is valid JSON")
    }

    fn rules(fixture: &Value) -> &[Value] {
        fixture["rules"]
            .as_array()
            .expect("route fixture contains a rules array")
    }

    fn matches_rule(rule: &str, path: &str) -> bool {
        let rule_segments = rule.split('/').collect::<Vec<_>>();
        let path_segments = path.split('/').collect::<Vec<_>>();
        rule_segments.len() == path_segments.len()
            && rule_segments
                .iter()
                .zip(path_segments)
                .all(|(rule_segment, path_segment)| {
                    let wildcard = rule_segment
                        .strip_prefix('<')
                        .and_then(|segment| segment.strip_suffix('>'));
                    wildcard.is_some_and(|name| !name.is_empty() && !path_segment.is_empty())
                        || wildcard.is_none() && rule_segment == &path_segment
                })
    }

    fn matching_rules<'a>(fixture: &'a Value, path: &str) -> Vec<&'a Value> {
        rules(fixture)
            .iter()
            .filter(|entry| {
                matches_rule(
                    entry["rule"].as_str().expect("route rule is a string"),
                    path,
                )
            })
            .collect()
    }

    fn has_method(rule: &Value, method: &str) -> bool {
        rule["methods"]
            .as_array()
            .expect("route methods are an array")
            .iter()
            .any(|candidate| candidate.as_str() == Some(method))
    }

    fn client_paths() -> Vec<(&'static str, String)> {
        let key_prefix = "clientkey";
        vec![
            ("POST", segment_ingest_path(key_prefix)),
            ("POST", entities_ingest_path(key_prefix)),
            ("POST", facets_ingest_path(key_prefix)),
            ("POST", imports_ingest_path(key_prefix)),
            ("POST", config_ingest_path(key_prefix)),
            ("GET", manifest_path(key_prefix, "segments")),
        ]
    }

    #[test]
    fn every_client_path_matches_one_rule_with_its_method() {
        let fixture = fixture();
        for (method, path) in client_paths() {
            let matches = matching_rules(&fixture, &path);
            assert_eq!(
                matches.len(),
                1,
                "{method} {path} must match one recorded rule"
            );
            assert!(
                has_method(matches[0], method),
                "recorded rule for {path} must include {method}"
            );
        }
    }

    #[test]
    fn client_paths_cover_every_recorded_rule() {
        let fixture = fixture();
        let covered = client_paths()
            .into_iter()
            .map(|(_, path)| {
                let matches = matching_rules(&fixture, &path);
                assert_eq!(matches.len(), 1, "{path} must match one recorded rule");
                matches[0]["rule"]
                    .as_str()
                    .expect("route rule is a string")
                    .to_owned()
            })
            .collect::<std::collections::BTreeSet<_>>();
        assert_eq!(covered.len(), rules(&fixture).len());
        assert_eq!(covered.len(), 6);
    }

    #[test]
    fn segment_path_rejects_a_day_suffix_and_accepts_its_original_shape() {
        let fixture = fixture();
        let path = segment_ingest_path("clientkey");
        let day_suffixed = format!("{path}/20260801");
        assert!(matching_rules(&fixture, &day_suffixed).is_empty());
        let stripped = day_suffixed
            .rsplit_once('/')
            .expect("day-suffixed path has a final part")
            .0;
        let matches = matching_rules(&fixture, stripped);
        assert_eq!(matches.len(), 1);
        assert!(has_method(matches[0], "POST"));
    }

    #[test]
    fn fixture_is_non_empty_and_contains_the_segments_post_rule() {
        let fixture = fixture();
        assert!(!rules(&fixture).is_empty());
        let matches = matching_rules(&fixture, &segment_ingest_path("clientkey"));
        assert_eq!(matches.len(), 1);
        assert!(has_method(matches[0], "POST"));
    }

    #[test]
    fn fixture_has_the_expected_door_census() {
        let fixture = fixture();
        let mut post_ingest = 0;
        let mut get_manifest = 0;
        for rule in rules(&fixture) {
            let segments = rule["rule"]
                .as_str()
                .expect("route rule is a string")
                .split('/')
                .filter(|segment| !segment.is_empty())
                .collect::<Vec<_>>();
            if segments.get(4) == Some(&"ingest") && has_method(rule, "POST") {
                post_ingest += 1;
            }
            if segments.get(4) == Some(&"manifest") && has_method(rule, "GET") {
                get_manifest += 1;
            }
        }
        assert_eq!(
            rules(&fixture).len(),
            6,
            "a red result means the door shape changed and someone must decide whether that is intended"
        );
        assert_eq!(post_ingest, 5);
        assert_eq!(get_manifest, 1);
    }
}

#[cfg(test)]
mod archive_manifest_schema_tests {
    use jsonschema::{Draft, options};
    use serde_json::Value;

    const SCHEMA: &str = include_str!("../schema/archive-manifest.v1.schema.json");
    const SKIP: &[&str] = &["examples", "const", "enum", "default"];

    fn display_pointer(pointer: &str) -> &str {
        if pointer.is_empty() { "/" } else { pointer }
    }

    fn json_pointer_escape(segment: &str) -> String {
        segment.replace('~', "~0").replace('/', "~1")
    }

    fn assert_no_ref(node: &Value, pointer: &str) {
        match node {
            Value::Object(map) => {
                assert!(
                    !map.contains_key("$ref"),
                    "$ref found at {}/$ref; examples-bearing subschemas must be self-contained",
                    display_pointer(pointer)
                );
                for (key, child) in map {
                    if SKIP.contains(&key.as_str()) {
                        continue;
                    }
                    assert_no_ref(child, &format!("{pointer}/{}", json_pointer_escape(key)));
                }
            }
            Value::Array(items) => {
                for (index, child) in items.iter().enumerate() {
                    assert_no_ref(child, &format!("{pointer}/{index}"));
                }
            }
            _ => {}
        }
    }

    fn walk_examples(node: &Value, pointer: &str, count: &mut usize) {
        match node {
            Value::Object(map) => {
                if let Some(Value::Array(examples)) = map.get("examples") {
                    assert_no_ref(node, pointer);
                    let validator = options()
                        .with_draft(Draft::Draft202012)
                        .build(node)
                        .unwrap_or_else(|error| {
                            panic!(
                                "enclosing subschema at {} does not compile: {error}",
                                display_pointer(pointer)
                            )
                        });
                    for (index, entry) in examples.iter().enumerate() {
                        let errors: Vec<String> = validator
                            .iter_errors(entry)
                            .map(|error| error.to_string())
                            .collect();
                        assert!(
                            errors.is_empty(),
                            "example {index} at {} failed to validate: {}",
                            display_pointer(pointer),
                            errors.join("; ")
                        );
                    }
                    *count += examples.len();
                }
                for (key, child) in map {
                    if SKIP.contains(&key.as_str()) {
                        continue;
                    }
                    walk_examples(
                        child,
                        &format!("{pointer}/{}", json_pointer_escape(key)),
                        count,
                    );
                }
            }
            Value::Array(items) => {
                for (index, child) in items.iter().enumerate() {
                    walk_examples(child, &format!("{pointer}/{index}"), count);
                }
            }
            _ => {}
        }
    }

    #[test]
    fn schema_examples_validate_against_their_enclosing_subschema() {
        let schema: Value = serde_json::from_str(SCHEMA).expect("archive-manifest schema is JSON");
        options()
            .with_draft(Draft::Draft202012)
            .build(&schema)
            .expect("archive-manifest document is a valid draft 2020-12 schema");
        let mut count = 0;
        walk_examples(&schema, "", &mut count);
        assert!(
            count >= 2,
            "archive-manifest schema must carry at least 2 examples, found {count}"
        );
    }
}
