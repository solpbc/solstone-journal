// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Version-one transfer manifest parsing and validation.

use std::collections::BTreeMap;
use std::fs;
use std::io;
use std::path::Path;

use serde::{Deserialize, Serialize};
use solstone_core_journal_io::contained_path;

use crate::TransferError;

pub const MANIFEST_NAME: &str = "manifest.json";
pub const MANIFEST_VERSION: u64 = 1;

/// The v1 archive manifest emitted by Python and native transfer.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct TransferManifest {
    pub version: u64,
    pub day: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub created_at: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub host: Option<String>,
    pub segments: BTreeMap<String, SegmentManifest>,
}

/// Manifest contents for one stream/key segment.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct SegmentManifest {
    pub files: Vec<ManifestFile>,
}

/// Integrity metadata for one archived file.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ManifestFile {
    pub name: String,
    pub sha256: String,
    pub size: u64,
}

/// A validated expected archive member.
#[derive(Debug, Clone)]
pub(crate) struct ExpectedMember {
    pub route: SegmentRoute,
    pub file: ManifestFile,
}

/// Validated stream and segment-key route.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct SegmentRoute {
    pub stream: String,
    pub key: String,
}

impl SegmentRoute {
    pub fn parse(value: &str) -> Result<Self, TransferError> {
        let mut parts = value.split('/');
        let (Some(stream), Some(key), None) = (parts.next(), parts.next(), parts.next()) else {
            return Err(TransferError::Manifest(format!(
                "segment key {value:?} must be stream/segment-key"
            )));
        };
        if stream.is_empty() || key.is_empty() {
            return Err(TransferError::Manifest(format!(
                "segment key {value:?} contains an empty component"
            )));
        }
        Ok(Self {
            stream: stream.to_owned(),
            key: key.to_owned(),
        })
    }

    pub fn archive_key(&self) -> String {
        format!("{}/{}", self.stream, self.key)
    }
}

/// Decode JSON and validate all shape fields required by v1.
pub(crate) fn parse_manifest(bytes: &[u8]) -> Result<TransferManifest, TransferError> {
    let manifest: TransferManifest = serde_json::from_slice(bytes)
        .map_err(|error| TransferError::Manifest(error.to_string()))?;
    // Deliberately stricter than Python's loose version comparison and deferred
    // day check: reject malformed archive control data before any path use.
    if manifest.version != MANIFEST_VERSION {
        return Err(TransferError::Manifest(format!(
            "version must be integer {MANIFEST_VERSION}"
        )));
    }
    if !is_day(&manifest.day) {
        return Err(TransferError::InvalidDay);
    }
    for (route, segment) in &manifest.segments {
        let route = SegmentRoute::parse(route)?;
        let mut names = std::collections::BTreeSet::new();
        for file in &segment.files {
            if !names.insert(&file.name) {
                return Err(TransferError::Manifest(format!(
                    "duplicate file {} in {}",
                    file.name,
                    route.archive_key()
                )));
            }
            validate_sha256(&file.sha256)?;
        }
    }
    Ok(manifest)
}

/// Refuse a symlinked day root before archive-controlled paths use it as containment root.
pub(crate) fn reject_symlink_day_directory(day_directory: &Path) -> Result<(), TransferError> {
    match fs::symlink_metadata(day_directory) {
        Ok(metadata) if metadata.file_type().is_symlink() => Err(
            TransferError::PoisonedDayDirectory(day_directory.to_path_buf()),
        ),
        Ok(_) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(TransferError::Io(error)),
    }
}

/// Build an expected tar-member map after validating containment below `day`.
pub(crate) fn expected_members(
    manifest: &TransferManifest,
    day_directory: &std::path::Path,
) -> Result<BTreeMap<String, ExpectedMember>, TransferError> {
    let mut expected = BTreeMap::new();
    for (route_value, segment) in &manifest.segments {
        let route = SegmentRoute::parse(route_value)?;
        let route_value = route.archive_key();
        let segment_directory = contained_path(day_directory, &route_value)?;
        validate_segment_key(&route.key)?;
        for file in &segment.files {
            // Validate every archive-controlled route before extraction or any
            // target probing. The map below is subsequently the only member
            // name authority used while streaming tar entries.
            contained_path(&segment_directory, &file.name)?;
            let member_name = format!("{route_value}/{}", file.name);
            if expected
                .insert(
                    member_name.clone(),
                    ExpectedMember {
                        route: route.clone(),
                        file: file.clone(),
                    },
                )
                .is_some()
            {
                return Err(TransferError::Manifest(format!(
                    "duplicate archive member {member_name}"
                )));
            }
        }
    }
    Ok(expected)
}

pub(crate) fn is_day(value: &str) -> bool {
    value.len() == 8 && value.bytes().all(|byte| byte.is_ascii_digit())
}

fn validate_segment_key(value: &str) -> Result<(), TransferError> {
    let Some((time, length)) = value.split_once('_') else {
        return Err(TransferError::Manifest(format!(
            "invalid segment key {value:?}"
        )));
    };
    if time.len() != 6
        || !time.bytes().all(|byte| byte.is_ascii_digit())
        || length.is_empty()
        || !length.bytes().all(|byte| byte.is_ascii_digit())
        || length
            .parse::<u64>()
            .ok()
            .filter(|value| *value > 0)
            .is_none()
    {
        return Err(TransferError::Manifest(format!(
            "invalid segment key {value:?}"
        )));
    }
    Ok(())
}

fn validate_sha256(value: &str) -> Result<(), TransferError> {
    if value.len() != 64 || !value.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(TransferError::Manifest(format!("invalid sha256 {value:?}")));
    }
    Ok(())
}

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
