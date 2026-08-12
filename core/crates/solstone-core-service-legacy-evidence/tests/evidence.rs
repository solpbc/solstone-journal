// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use plist::Value as PlistValue;
use serde_json::{Map, Value, json};
use solstone_core_service_legacy_evidence::{embedded, embedded_map, manifest_bytes, sha256_hex};
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

fn json(bytes: &[u8]) -> Value {
    serde_json::from_slice(bytes).expect("embedded JSON parses")
}

fn manifest() -> Value {
    json(manifest_bytes())
}

fn object(value: &Value) -> &Map<String, Value> {
    value.as_object().expect("JSON object")
}

fn array(value: &Value) -> &[Value] {
    value.as_array().expect("JSON array")
}

fn exact_keys(value: &Value, expected: &[&str]) {
    let found: BTreeSet<_> = object(value).keys().map(String::as_str).collect();
    let expected: BTreeSet<_> = expected.iter().copied().collect();
    assert_eq!(found, expected, "object key set is exact");
}

fn text<'a>(value: &'a Value, key: &str) -> &'a str {
    object(value)[key].as_str().expect("string field")
}

fn fixture(path: &str) -> Value {
    json(embedded(path).expect("fixture is compile-time embedded"))
}

fn repository_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(3)
        .expect("standalone crate has repository parent")
        .to_path_buf()
}

fn reference_bytes(path: &str) -> Vec<u8> {
    embedded(path)
        .map(<[u8]>::to_vec)
        .unwrap_or_else(|| fs::read(repository_root().join(path)).expect("reference file reads"))
}

fn reference_hash(value: &Value) {
    let path = text(value, "path");
    assert_eq!(sha256_hex(&reference_bytes(path)), text(value, "sha256"));
}

fn exact_reference(value: &Value) {
    exact_keys(value, &["path", "sha256"]);
    reference_hash(value);
}

fn git_blob(commit: &str, path: &str) -> Vec<u8> {
    let output = Command::new("/usr/bin/git")
        .args(["show", &format!("{commit}:{path}")])
        .current_dir(repository_root())
        .output()
        .expect("Git blob reads");
    assert!(
        output.status.success(),
        "Git blob is unavailable: {commit}:{path}"
    );
    output.stdout
}

fn canonical(value: &Value) -> String {
    serde_json::to_string(value).expect("canonical JSON serializes")
}

fn plist_json(encoded: &str) -> Value {
    let bytes = base64(encoded);
    serde_json::to_value(
        PlistValue::from_reader_xml(bytes.as_slice()).expect("strict plist parses"),
    )
    .expect("plist converts to JSON")
}

fn duplicate_plist_key(encoded: &str) -> bool {
    let xml = String::from_utf8(base64(encoded)).expect("plist XML is UTF-8");
    let mut dictionaries: Vec<BTreeSet<String>> = Vec::new();
    let mut cursor = 0;
    while let Some(offset) = xml[cursor..].find('<') {
        let start = cursor + offset;
        if xml[start..].starts_with("<dict>") {
            dictionaries.push(BTreeSet::new());
            cursor = start + 6;
        } else if xml[start..].starts_with("</dict>") {
            dictionaries.pop();
            cursor = start + 7;
        } else if xml[start..].starts_with("<key>") {
            let content = start + 5;
            let Some(end_offset) = xml[content..].find("</key>") else {
                return true;
            };
            let key = &xml[content..content + end_offset];
            if dictionaries
                .last_mut()
                .is_none_or(|keys| !keys.insert(key.to_owned()))
            {
                return true;
            }
            cursor = content + end_offset + 6;
        } else {
            cursor = start + 1;
        }
    }
    false
}

fn base64(value: &str) -> Vec<u8> {
    const TABLE: &str = "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = Vec::new();
    let mut bits = 0_u32;
    let mut count = 0;
    for byte in value.bytes().filter(|byte| *byte != b'=') {
        let index = TABLE.find(char::from(byte)).expect("base64 alphabet") as u32;
        bits = (bits << 6) | index;
        count += 6;
        if count >= 8 {
            count -= 8;
            out.push(((bits >> count) & 0xff) as u8);
        }
    }
    out
}

fn shape(value: &Value) -> Value {
    match value {
        Value::Object(map) => Value::Object(
            map.iter()
                .map(|(key, value)| (key.clone(), shape(value)))
                .collect(),
        ),
        Value::Array(values) => Value::Array(values.iter().map(shape).collect()),
        Value::String(_) => json!("string"),
        Value::Number(_) => json!("number"),
        Value::Bool(_) => json!("boolean"),
        Value::Null => json!("null"),
    }
}

fn unit_shape(unit: &str) -> Option<Vec<(String, String)>> {
    let mut section = None;
    let mut fields = Vec::new();
    let mut seen = BTreeSet::new();
    for line in unit.lines().filter(|line| !line.is_empty()) {
        if line.starts_with('[') && line.ends_with(']') {
            section = Some(line[1..line.len() - 1].to_owned());
            continue;
        }
        let (key, value) = line.split_once('=')?;
        let section = section.as_ref()?;
        let identity = if key == "Environment" {
            format!("{section}.{key}.{}", value.split_once('=')?.0)
        } else {
            format!("{section}.{key}")
        };
        if !seen.insert(identity.clone()) {
            return None;
        }
        fields.push((identity, key.to_owned()));
    }
    Some(fields)
}

fn normalize(mut plist: Value, unit: &str, inputs: &Value) -> Option<Value> {
    let env = object(inputs.get("env")?);
    let plist_env = object(plist.get("EnvironmentVariables")?);
    if plist_env != env {
        return None;
    }
    let plist_object = plist.as_object_mut()?;
    let env_object = plist_object
        .get_mut("EnvironmentVariables")?
        .as_object_mut()?;
    for (key, value) in env_object {
        *value = match key.as_str() {
            "HOME" => json!("<HOME>"),
            "PATH" => json!("<PATH>"),
            "SOLSTONE_JOURNAL" | "_SOLSTONE_JOURNAL_OVERRIDE" => json!("<JOURNAL>"),
            key if key.ends_with("API_KEY")
                || matches!(key, "REVAI_ACCESS_TOKEN" | "PLAUD_ACCESS_TOKEN") =>
            {
                json!(format!("<API_KEY:{key}>"))
            }
            _ => value.clone(),
        };
    }
    let port = inputs.get("port")?;
    let args = plist_object.get_mut("ProgramArguments")?.as_array_mut()?;
    let launcher = args.first()?.as_str()?.to_owned();
    args[0] = json!("<LAUNCHER_BIN>");
    for value in args.iter_mut().skip(1) {
        if value == port
            || value.as_str() == port.as_i64().map(|number| number.to_string()).as_deref()
        {
            *value = json!("<PORT>");
        }
    }
    let journal = inputs.get("journal_path")?.as_str()?;
    for key in ["StandardOutPath", "StandardErrorPath"] {
        if let Some(value) = plist_object.get_mut(key) {
            let path = value.as_str()?.to_owned();
            *value = json!(format!("<JOURNAL>{}", path.strip_prefix(journal)?));
        }
    }
    let mut systemd_env = BTreeMap::new();
    let mut normalized = Vec::new();
    for line in unit.lines() {
        let line = if let Some(command) = line.strip_prefix("ExecStart=") {
            let suffix = command.strip_prefix(&launcher)?;
            let port_suffix = format!(" {}", port.as_i64()?);
            format!(
                "ExecStart=<LAUNCHER_BIN>{}",
                suffix
                    .strip_suffix(&port_suffix)
                    .map_or(suffix.to_owned(), |prefix| format!("{prefix} <PORT>"))
            )
        } else if let Some(assignment) = line.strip_prefix("Environment=") {
            let (key, value) = assignment.split_once('=')?;
            systemd_env.insert(key.to_owned(), value.to_owned());
            let token = match key {
                "HOME" => "<HOME>".to_owned(),
                "PATH" => "<PATH>".to_owned(),
                "SOLSTONE_JOURNAL" | "_SOLSTONE_JOURNAL_OVERRIDE" => "<JOURNAL>".to_owned(),
                key if key.ends_with("API_KEY")
                    || matches!(key, "REVAI_ACCESS_TOKEN" | "PLAUD_ACCESS_TOKEN") =>
                {
                    format!("<API_KEY:{key}>")
                }
                _ => value.to_owned(),
            };
            format!("Environment={key}={token}")
        } else if let Some(value) = line.strip_prefix("StandardOutput=append:") {
            format!(
                "StandardOutput=append:<JOURNAL>{}",
                value.strip_prefix(journal)?
            )
        } else if let Some(value) = line.strip_prefix("StandardError=append:") {
            format!(
                "StandardError=append:<JOURNAL>{}",
                value.strip_prefix(journal)?
            )
        } else {
            line.to_owned()
        };
        normalized.push(line);
    }
    if systemd_env
        != env
            .iter()
            .map(|(key, value)| (key.clone(), value.as_str().expect("string env").to_owned()))
            .collect()
    {
        return None;
    }
    Some(json!({"plist": plist, "systemd_unit": format!("{}\n", normalized.join("\n"))}))
}

#[test]
fn manifest_inventory_and_censuses_are_exact() {
    let manifest = manifest();
    exact_keys(
        &manifest,
        &[
            "schema",
            "schema_version",
            "source",
            "follow_census",
            "tag_census",
            "interpreters",
            "blobs",
            "semantic_deltas",
            "packaging_provenance",
            "inventory",
        ],
    );
    assert_eq!(text(&manifest, "schema"), "service-legacy-evidence");
    exact_keys(&manifest["source"], &["commit", "tooling"]);
    for tool in array(&manifest["source"]["tooling"]) {
        exact_reference(tool);
        assert_eq!(
            sha256_hex(&git_blob(
                text(&manifest["source"], "commit"),
                text(tool, "path")
            )),
            text(tool, "sha256")
        );
    }
    for key in ["follow_census", "tag_census", "semantic_deltas"] {
        exact_keys(&manifest[key], &["count", "path", "sha256"]);
        reference_hash(&manifest[key]);
    }
    exact_reference(&manifest["interpreters"]);
    exact_reference(&manifest["packaging_provenance"]);
    let inventory = array(&manifest["inventory"]);
    assert_eq!(inventory.len(), embedded_map().len());
    for row in inventory {
        exact_keys(row, &["path", "sha256"]);
        let path = text(row, "path");
        assert_eq!(
            sha256_hex(embedded(path).expect("inventory file embedded")),
            text(row, "sha256")
        );
    }
    let follow = fixture("core/fixtures/service_legacy_evidence/follow-census.json");
    let tags = fixture("core/fixtures/service_legacy_evidence/tag-census.json");
    exact_keys(
        &follow,
        &[
            "entries",
            "head_commit",
            "root_commit",
            "schema",
            "schema_version",
        ],
    );
    for entry in array(&follow["entries"]) {
        exact_keys(entry, &["blob", "commit", "index", "path"]);
    }
    exact_keys(&tags, &["schema", "schema_version", "tags"]);
    for tag in array(&tags["tags"]) {
        exact_keys(tag, &["blob", "path", "tag"]);
    }
    assert_eq!(array(&follow["entries"]).len(), 44);
    assert_eq!(array(&tags["tags"]).len(), 66);
    let follow_blobs: BTreeSet<_> = array(&follow["entries"])
        .iter()
        .map(|row| text(row, "blob"))
        .collect();
    let tagged: BTreeSet<_> = array(&tags["tags"])
        .iter()
        .filter_map(|row| row["blob"].as_str())
        .collect();
    assert_eq!(tagged.len(), 14);
    assert!(tagged.is_subset(&follow_blobs));

    let interpreters = fixture(text(&manifest["interpreters"], "path"));
    exact_keys(&interpreters, &["buckets", "schema", "schema_version"]);
    exact_keys(&interpreters["buckets"], &["cpython37", "cpython39"]);
    let expected_inventories = BTreeMap::from([
        (
            "cpython37",
            "b201e461f249322c261004b8799044bec73d0166a0426ea06d4cb4c496b5514c",
        ),
        (
            "cpython39",
            "f018c25a948d20946dce010d13e7804697247ecd27eba7a9ec3a30d9a43cbd3c",
        ),
    ]);
    for (name, bucket) in object(&interpreters["buckets"]) {
        exact_keys(
            bucket,
            &[
                "archive_sha256",
                "declared_floor",
                "executable",
                "executable_sha256",
                "inventory_sha256",
                "pin_rationale",
                "pinned_version",
                "platform",
                "release_tag",
                "url",
            ],
        );
        assert_eq!(
            text(bucket, "inventory_sha256"),
            expected_inventories[name.as_str()],
            "interpreter inventory pin is exact"
        );
    }

    let provenance = fixture(text(&manifest["packaging_provenance"], "path"));
    exact_keys(
        &provenance,
        &[
            "build",
            "launcher_chain",
            "schema",
            "schema_version",
            "source",
            "wheels",
        ],
    );
    assert_eq!(
        text(&provenance, "schema"),
        "service-legacy-packaging-provenance"
    );
    exact_keys(&provenance["source"], &["commit"]);
    exact_keys(
        &provenance["build"],
        &[
            "bundle",
            "environment",
            "host",
            "source_date_epoch",
            "tools",
            "wheel_commands",
        ],
    );
    exact_keys(
        &provenance["build"]["tools"],
        &["cargo", "maturin", "python", "rustc", "rustup", "uv"],
    );
    for role in [
        "CARGO_HOME",
        "CARGO_TARGET_DIR",
        "HOME",
        "RUSTUP_HOME",
        "TMPDIR",
        "UV_CACHE_DIR",
        "XDG_CONFIG_HOME",
    ] {
        assert!(
            text(&provenance["build"]["environment"], role).starts_with('<'),
            "per-run build root {role} must serialize as a role token"
        );
    }
    exact_keys(
        &provenance["wheels"],
        &["solstone_core_journal", "solstone_journal"],
    );
    let journal = &provenance["wheels"]["solstone_journal"];
    exact_keys(
        journal,
        &[
            "filename",
            "metadata",
            "project_script",
            "record",
            "script_files_launcher",
            "sha256",
        ],
    );
    exact_keys(
        &journal["project_script"],
        &[
            "entry_points",
            "mechanism",
            "name",
            "part_of_journal_launcher_chain",
            "target",
        ],
    );
    assert_eq!(
        text(&journal["project_script"], "mechanism"),
        "project.scripts"
    );
    assert_eq!(
        journal["project_script"]["part_of_journal_launcher_chain"],
        false
    );
    exact_keys(
        &journal["script_files_launcher"],
        &[
            "content_sha256",
            "mechanism",
            "path",
            "record_sha256",
            "record_size",
        ],
    );
    assert_eq!(
        text(&journal["script_files_launcher"], "mechanism"),
        "script-files"
    );
    let core = &provenance["wheels"]["solstone_core_journal"];
    exact_keys(
        core,
        &["binary", "filename", "metadata", "record", "sha256"],
    );
    exact_keys(
        &provenance["launcher_chain"],
        &[
            "journal_launcher",
            "native_binary",
            "native_dispatch",
            "native_dispatch_sources",
            "sibling_binary",
        ],
    );
    assert_eq!(
        provenance["launcher_chain"]["journal_launcher"]["wheel"],
        journal["script_files_launcher"]
    );
    assert_eq!(
        provenance["launcher_chain"]["native_binary"]["wheel"],
        core["binary"]
    );
    assert_eq!(
        text(&provenance["launcher_chain"], "sibling_binary"),
        "solstone-core-journal"
    );
    let commit = text(&provenance["source"], "commit");
    let launcher = &provenance["launcher_chain"]["journal_launcher"];
    assert_eq!(
        sha256_hex(&git_blob(commit, text(launcher, "source_path"))),
        text(launcher, "source_sha256")
    );
    let dispatch = array(&provenance["launcher_chain"]["native_dispatch_sources"]);
    assert_eq!(dispatch.len(), 3);
    for source in dispatch {
        exact_reference(source);
        assert_eq!(
            sha256_hex(&git_blob(commit, text(source, "path"))),
            text(source, "sha256")
        );
    }
}

#[test]
fn dispositions_and_shared_bytes_are_rederived() {
    let manifest = manifest();
    let blobs = array(&manifest["blobs"]);
    assert_eq!(blobs.len(), 44);
    let follow = fixture("core/fixtures/service_legacy_evidence/follow-census.json");
    let tags = fixture("core/fixtures/service_legacy_evidence/tag-census.json");
    let deltas = fixture("core/fixtures/service_legacy_evidence/semantic-deltas.json");
    exact_keys(&deltas, &["deltas", "schema", "schema_version"]);
    assert_eq!(array(&deltas["deltas"]).len(), 43);
    for delta in array(&deltas["deltas"]) {
        exact_keys(delta, &["changes", "from_blob", "to_blob"]);
        for change in array(&delta["changes"]) {
            exact_keys(
                change,
                &["field", "new_value", "old_value", "operation", "platform"],
            );
        }
    }
    let mut tag_map: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
    for row in array(&tags["tags"]) {
        if let Some(blob) = row["blob"].as_str() {
            tag_map.entry(blob).or_default().push(text(row, "tag"));
        }
    }
    for (index, blob) in blobs.iter().enumerate() {
        let follow_entry = &array(&follow["entries"])[index];
        exact_keys(
            blob,
            &[
                "blob",
                "commit",
                "index",
                "interpreter_bucket",
                "negative",
                "normalized",
                "path",
                "profiles",
                "release_status",
                "shared_with",
            ],
        );
        assert_eq!(blob["index"], follow_entry["index"]);
        assert_eq!(blob["commit"], follow_entry["commit"]);
        assert_eq!(blob["blob"], follow_entry["blob"]);
        assert_eq!(blob["path"], follow_entry["path"]);
        let status = &blob["release_status"];
        match text(status, "kind") {
            "tagged" => {
                exact_keys(status, &["kind", "tags"]);
                assert_eq!(
                    array(&status["tags"]),
                    tag_map.get(text(blob, "blob")).expect("tag list")
                );
            }
            "unreleased_superseded" => {
                exact_keys(
                    status,
                    &[
                        "distance",
                        "follow_position",
                        "kind",
                        "successor_blob",
                        "successor_commit",
                        "tag_census_sha256",
                        "tag_matches",
                    ],
                );
                assert!(!tag_map.contains_key(text(blob, "blob")));
                assert_eq!(status["follow_position"], blob["index"]);
                assert_eq!(status["distance"], 1);
                assert!(array(&status["tag_matches"]).is_empty());
                assert_eq!(status["successor_blob"], blobs[index + 1]["blob"]);
                assert_eq!(status["successor_commit"], blobs[index + 1]["commit"]);
            }
            other => panic!("unknown release status {other}"),
        }
        exact_keys(&blob["normalized"], &["linux", "macos"]);
        exact_keys(&blob["negative"], &["linux", "macos"]);
        for platform in ["linux", "macos"] {
            exact_reference(&blob["normalized"][platform]);
            exact_reference(&blob["negative"][platform]);
            let normalized = fixture(text(&blob["normalized"][platform], "path"));
            exact_keys(
                &normalized,
                &[
                    "blob",
                    "commit",
                    "path",
                    "platform",
                    "schema",
                    "schema_version",
                    "variants",
                ],
            );
            for variant in object(&normalized["variants"]).values() {
                exact_keys(variant, &["plist", "profiles", "systemd_unit"]);
            }
        }
        for profile in array(&blob["profiles"]) {
            exact_keys(profile, &["name", "raw"]);
            exact_keys(&profile["raw"], &["linux", "macos"]);
            for platform in ["linux", "macos"] {
                exact_reference(&profile["raw"][platform]);
                let raw = fixture(text(&profile["raw"][platform], "path"));
                exact_keys(
                    &raw,
                    &[
                        "blob",
                        "commit",
                        "inputs",
                        "interpreter_bucket",
                        "path",
                        "platform",
                        "profile",
                        "raw",
                        "schema",
                        "schema_version",
                    ],
                );
                assert_eq!(text(&raw, "schema"), "service-legacy-raw-evidence");
                assert_eq!(raw["schema_version"], 1);
                exact_keys(&raw["inputs"], &["env", "journal_path", "port"]);
                exact_keys(&raw["raw"], &["plist_base64", "sha256", "systemd_unit"]);
            }
        }
        if let Some(previous) = blob["shared_with"].as_str() {
            assert_eq!(previous, text(&blobs[index - 1], "blob"));
            assert!(
                array(&deltas["deltas"])[index - 1]["changes"]
                    .as_array()
                    .expect("changes")
                    .is_empty()
            );
            for platform in ["linux", "macos"] {
                let current = fixture(text(&blob["normalized"][platform], "path"));
                let prior = fixture(text(&blobs[index - 1]["normalized"][platform], "path"));
                assert_eq!(
                    canonical(&current["variants"]),
                    canonical(&prior["variants"])
                );
            }
        }
    }
}

#[test]
fn every_negative_twin_is_strictly_rejected_or_not_a_member() {
    let manifest = manifest();
    let mut members = BTreeSet::new();
    for blob in array(&manifest["blobs"]) {
        for platform in ["linux", "macos"] {
            let fixture = fixture(text(&blob["normalized"][platform], "path"));
            for variant in object(&fixture["variants"]).values() {
                members.insert(canonical(
                    &json!({"plist": variant["plist"], "systemd_unit": variant["systemd_unit"]}),
                ));
            }
        }
    }
    let mut count = 0;
    for blob in array(&manifest["blobs"]) {
        for platform in ["linux", "macos"] {
            let base = fixture(text(&blob["profiles"][0]["raw"][platform], "path"));
            let base_plist = plist_json(text(&base["raw"], "plist_base64"));
            let base_shape = shape(&base_plist);
            let base_unit_shape =
                unit_shape(text(&base["raw"], "systemd_unit")).expect("base unit shape");
            let negative_path = text(&blob["negative"][platform], "path");
            let negatives = fixture(negative_path);
            exact_keys(
                &negatives,
                &[
                    "base_profile",
                    "blob",
                    "platform",
                    "schema",
                    "schema_version",
                    "twins",
                ],
            );
            for twin in array(&negatives["twins"]) {
                exact_keys(
                    twin,
                    &[
                        "expected_rejection",
                        "field",
                        "id",
                        "mutated",
                        "mutation_kind",
                    ],
                );
                exact_keys(&twin["mutated"], &["plist_base64", "systemd_unit"]);
                count += 1;
                if duplicate_plist_key(text(&twin["mutated"], "plist_base64")) {
                    continue;
                }
                let parsed =
                    std::panic::catch_unwind(|| plist_json(text(&twin["mutated"], "plist_base64")))
                        .ok();
                let accepted_shape = parsed.as_ref().is_some_and(|plist| {
                    shape(plist) == base_shape
                        && unit_shape(text(&twin["mutated"], "systemd_unit"))
                            == Some(base_unit_shape.clone())
                });
                if accepted_shape {
                    let normalized = normalize(
                        parsed.expect("parsed twin"),
                        text(&twin["mutated"], "systemd_unit"),
                        &base["inputs"],
                    );
                    assert!(
                        normalized.is_none_or(|value| !members.contains(&canonical(&value))),
                        "twin accepted: {}",
                        text(twin, "id")
                    );
                }
            }
        }
    }
    assert_eq!(count, 13_146);
}

/// Expected red until the ownership-classifier wave introduces its specific
/// evidence classifier. It is ignored so ordinary native CI remains green.
#[test]
#[ignore = "expected red until the ownership-classifier wave lands"]
fn calling_session_classifier_contract_is_red() {
    panic!(
        "ownership classifier is not implemented yet; this is the AC6 calling-session red-state contract"
    );
}
