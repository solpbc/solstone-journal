// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::fs::{self, File, OpenOptions};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use chrono::{DateTime, Utc};
#[cfg(unix)]
use nix::fcntl::{Flock, FlockArg};
use serde_json::{Map, Value};
use sha2::{Digest, Sha256};

use crate::fingerprint::{CanonicalInput, canonical_fingerprint, canonical_json, hmac_sha256};
use crate::fixture::{brain_state_keys, local_contract, projection_fixture};
use crate::inspect::{FINGERPRINT_KEY_BYTES, project_brain_state};
use crate::record::validate_brain_state_record;
use crate::runtime_health::{inspect_runtime_health, inspection_from_fixture};
use crate::{
    InspectionStatus, brain_fingerprint_key_path, brain_refresh_lease_path, brain_state_path,
    build_active_brain_fingerprint, derive_active_brain_lane, inspect_brain_state,
    load_existing_fingerprint_key, probe_file_lease_held,
};

static TEMP_COUNTER: AtomicUsize = AtomicUsize::new(0);

struct TestJournal {
    path: PathBuf,
}

impl TestJournal {
    fn new() -> Self {
        let unique = TEMP_COUNTER.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "solstone-core-brain-test-{}-{}-{unique}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("time after epoch")
                .as_nanos()
        ));
        fs::create_dir_all(&path).expect("test journal directory");
        Self { path }
    }

    fn health(&self) {
        fs::create_dir_all(self.path.join("health")).expect("health directory");
    }
}

impl Drop for TestJournal {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}

fn bundled_config() -> &'static Map<String, Value> {
    object(
        projection_fixture()
            .configs
            .get("lane_bundled")
            .expect("bundled config"),
    )
}

fn valid_bundled_record() -> &'static Value {
    projection_fixture()
        .records
        .get("lane_bundled/ready")
        .expect("bundled ready record")
}

fn write_valid_bundled_record(journal: &TestJournal) {
    journal.health();
    fs::write(
        brain_state_path(&journal.path),
        serde_json::to_vec(valid_bundled_record()).expect("record JSON"),
    )
    .expect("write record");
}

fn snapshot_tree(root: &Path) -> Vec<(PathBuf, Option<String>, u128, u32)> {
    fn walk(root: &Path, current: &Path, output: &mut Vec<(PathBuf, Option<String>, u128, u32)>) {
        let metadata = fs::metadata(current).expect("snapshot metadata");
        let relative = current
            .strip_prefix(root)
            .expect("under root")
            .to_path_buf();
        let hash = metadata.is_file().then(|| {
            let bytes = fs::read(current).expect("snapshot file");
            let mut hasher = Sha256::new();
            hasher.update(bytes);
            format!("{:x}", hasher.finalize())
        });
        #[cfg(unix)]
        let mode = {
            use std::os::unix::fs::PermissionsExt;
            metadata.permissions().mode()
        };
        #[cfg(not(unix))]
        let mode = 0;
        let modified = metadata
            .modified()
            .expect("mtime")
            .duration_since(UNIX_EPOCH)
            .expect("mtime after epoch")
            .as_nanos();
        output.push((relative, hash, modified, mode));
        if metadata.is_dir() {
            for child in fs::read_dir(current).expect("snapshot directory") {
                walk(root, &child.expect("directory entry").path(), output);
            }
        }
    }
    let mut output = Vec::new();
    walk(root, root, &mut output);
    output.sort_by(|left, right| left.0.cmp(&right.0));
    output
}

fn fixture_now() -> DateTime<Utc> {
    DateTime::parse_from_rfc3339(&projection_fixture().now)
        .expect("fixture now is RFC3339")
        .with_timezone(&Utc)
}

fn hex(value: &str) -> Vec<u8> {
    (0..value.len())
        .step_by(2)
        .map(|position| u8::from_str_radix(&value[position..position + 2], 16).expect("hex"))
        .collect()
}

fn object(value: &Value) -> &Map<String, Value> {
    value.as_object().expect("fixture config is object")
}

#[test]
fn canonical_fingerprint_corpus_has_exact_count() {
    let fixture = &local_contract().canonical_fingerprint;
    assert_eq!(fixture.vectors.len(), 18);
    for vector in &fixture.vectors {
        let input = match vector.name.as_str() {
            "tuple_becomes_list" => CanonicalInput::Object(vec![(
                "pins".to_owned(),
                CanonicalInput::Tuple(vec![
                    CanonicalInput::Json(Value::from("b")),
                    CanonicalInput::Json(Value::from("a")),
                ]),
            )]),
            "path_becomes_string" => CanonicalInput::Object(vec![
                (
                    "binary".to_owned(),
                    CanonicalInput::Path(PathBuf::from("/opt/llama/llama-server")),
                ),
                (
                    "model".to_owned(),
                    CanonicalInput::Path(PathBuf::from("m.gguf")),
                ),
            ]),
            _ => CanonicalInput::Json(
                serde_json::from_str(vector.input_json.as_deref().expect("JSON input"))
                    .expect("fixture canonical input JSON"),
            ),
        };
        assert_eq!(
            canonical_json(&input).unwrap(),
            vector.canonical_json,
            "{}",
            vector.name
        );
        assert_eq!(
            canonical_fingerprint(&input).unwrap(),
            vector.sha256,
            "{}",
            vector.name
        );
    }
}

#[test]
fn canonical_digest_corpus_has_exact_count() {
    let fixture = &local_contract().canonical_fingerprint;
    assert_eq!(fixture.canonical_digest_vectors.len(), 4);
    let key = hex(&fixture.vector_hmac_key_hex);
    for vector in &fixture.canonical_digest_vectors {
        let value = serde_json::from_str::<Value>(&vector.input_json).expect("digest input JSON");
        let wrapped = CanonicalInput::Json(serde_json::json!({"value": value}));
        let text = canonical_json(&wrapped).unwrap();
        assert_eq!(text, vector.wrapped_canonical_json, "{}", vector.name);
        assert_eq!(
            hmac_sha256(&key, &text),
            vector.hmac_sha256,
            "{}",
            vector.name
        );
    }
}

#[test]
fn numbers_preserve_integer_and_float_forms() {
    let numbers = local_contract()
        .canonical_fingerprint
        .vectors
        .iter()
        .find(|vector| vector.name == "numbers")
        .expect("numbers vector");
    let value = serde_json::from_str(numbers.input_json.as_deref().unwrap()).unwrap();
    let text = canonical_json(&CanonicalInput::Json(value)).unwrap();
    assert!(text.contains("9007199254740993"));
    assert!(text.contains("1.0"));
    assert!(text.contains("-0.0"));
}

#[test]
fn config_fingerprint_corpus_has_exact_count() {
    let fixture = projection_fixture();
    assert_eq!(fixture.configs.len(), 9);
    let key = hex(&fixture.hmac_key_hex);
    let mut executed = 0;
    for (name, config) in &fixture.configs {
        let resolution = derive_active_brain_lane(object(config));
        let runtime = (resolution.lane.as_deref() == Some("bundled"))
            .then(|| Value::String(fixture.unrelated_fingerprint.clone()));
        let fingerprint = build_active_brain_fingerprint(object(config), &key, runtime);
        executed += 1;
        if name == "config_missing_provider" || name == "lane_none" {
            assert_eq!(resolution.lane.as_deref(), Some("none"));
            let expected = local_contract()
                .canonical_fingerprint
                .vectors
                .iter()
                .find(|vector| vector.name == "lane_none_shape")
                .expect("none-lane canonical vector");
            assert_eq!(
                fingerprint.unwrap().as_deref(),
                Some(expected.sha256.as_str())
            );
            continue;
        }
        if name.contains("partial") || name.contains("unmatched") || name.contains("unknown") {
            assert_eq!(resolution.lane, None, "{name}");
            assert!(fingerprint.is_err(), "{name}");
            continue;
        }
        assert!(fingerprint.is_ok(), "{name}");
    }
    assert_eq!(executed, 9);
}

#[test]
fn validation_corpus_has_exact_count_and_paths() {
    let fixture = projection_fixture();
    assert_eq!(fixture.validation.len(), 100);
    let mut accepted = 0;
    let mut refused = 0;
    for case in &fixture.validation {
        let record = fixture
            .records
            .get(&case.name)
            .filter(|record| !record.is_null())
            .or_else(|| fixture.malformed_records.get(&case.name))
            .expect("named validation record");
        let result = validate_brain_state_record(record, fixture_now());
        assert_eq!(result.is_ok(), case.accepted, "{}: {result:?}", case.name);
        if case.accepted {
            accepted += 1;
        } else {
            refused += 1;
            let error = case.error.as_deref().expect("refusal error");
            let remainder = error
                .strip_prefix("BrainStateValidationError: ")
                .expect("stable exception prefix");
            let (path, _) = remainder.split_once(": ").expect("path separator");
            assert_eq!(result.unwrap_err().path, path, "{}", case.name);
        }
    }
    assert_eq!(accepted, 74);
    assert_eq!(refused, 26);
}

#[test]
fn projection_corpus_has_exact_count_and_coverage() {
    let fixture = projection_fixture();
    assert_eq!(fixture.projection.len(), 553);
    let key: [u8; 32] = hex(&fixture.hmac_key_hex).try_into().expect("32 byte key");
    let mut aggregates = std::collections::BTreeSet::new();
    let mut reasons = std::collections::BTreeSet::new();
    let mut null_reason = false;
    let mut transition = false;
    let mut executed = 0;
    for case in &fixture.projection {
        let record = if case.record == "absent" {
            None
        } else {
            let record = fixture
                .records
                .get(&case.record)
                .filter(|record| !record.is_null())
                .or_else(|| fixture.malformed_records.get(&case.record))
                .unwrap_or_else(|| panic!("named record {}", case.record));
            Some(validate_brain_state_record(record, fixture_now()).expect("valid corpus record"))
        };
        let config = fixture.configs.get(&case.config).expect("named config");
        let runtime_value = fixture
            .runtime_health
            .get(&case.runtime_health)
            .expect("runtime case");
        let runtime = inspection_from_fixture(runtime_value);
        let result = project_brain_state(
            record.as_ref(),
            object(config),
            case.refresh_permit_active,
            case.hmac_key_present.then_some(&key),
            Some(&runtime),
            fixture_now(),
        );
        executed += 1;
        let expected = &case.projection;
        assert_eq!(
            result.aggregate_state, expected.aggregate_state,
            "{:?} got {result:?}",
            case
        );
        assert_eq!(result.reason_code, expected.reason_code, "{:?}", case);
        assert_eq!(result.active_lane, expected.active_lane, "{:?}", case);
        assert_eq!(
            result.active_provider, expected.active_provider,
            "{:?}",
            case
        );
        assert_eq!(result.active_model, expected.active_model, "{:?}", case);
        assert_eq!(
            result.fingerprint_sha256, expected.fingerprint_sha256,
            "{:?}",
            case
        );
        assert_eq!(
            result.runtime_transition_in_progress, expected.runtime_transition_in_progress,
            "{:?}",
            case
        );
        aggregates.insert(result.aggregate_state);
        if let Some(reason) = result.reason_code {
            reasons.insert(reason);
        } else {
            null_reason = true;
        }
        transition |= result.runtime_transition_in_progress;
    }
    assert_eq!(executed, 553);
    assert_eq!(aggregates.len(), 5);
    assert!(reasons.len() >= 15);
    assert!(null_reason);
    assert!(transition);
}

#[test]
fn reason_to_aggregate_corpus_has_exact_count_and_partition() {
    let vocabulary = &local_contract().brain_state;
    assert_eq!(vocabulary.reason_to_aggregate.len(), 42);
    for (reason, aggregate) in &vocabulary.reason_to_aggregate {
        assert!(
            vocabulary
                .reason_codes
                .iter()
                .any(|candidate| candidate == reason)
        );
        assert!(
            vocabulary
                .aggregate_states
                .iter()
                .any(|candidate| candidate == aggregate)
        );
    }
    let evidence = vocabulary
        .evidence_reason_codes
        .values()
        .flatten()
        .collect::<std::collections::BTreeSet<_>>();
    let projection = vocabulary
        .projection_only_reason_codes
        .iter()
        .collect::<std::collections::BTreeSet<_>>();
    assert!(evidence.is_disjoint(&projection));
    assert_eq!(evidence.union(&projection).count(), 42);
    assert!(
        vocabulary
            .aggregate_states
            .iter()
            .all(|aggregate| !vocabulary.reason_codes.contains(aggregate))
    );
}

#[test]
fn brain_state_vocabulary_has_an_explicit_consumed_or_deferred_owner() {
    const CONSUMED: &[&str] = &[
        "aggregate_states",
        "cloud_byo_providers",
        "component_order",
        "component_statuses",
        "config_diagnostic_fields",
        "diagnostic_metadata_schemas",
        "evidence_reason_codes",
        "fingerprint_key_bytes",
        "fingerprint_schema_version",
        "inspection_statuses",
        "incoherent_runtime_phase_reason_codes",
        "lanes",
        "lane_components",
        "projection_only_reason_codes",
        "paths",
        "reason_codes",
        "reason_to_aggregate",
        "record_fields",
        "runtime_phase_to_reason",
        "runtime_phases",
        "runtime_reason_codes",
        "runtime_reason_to_brain_reason",
        "runtime_transition_phases",
        "schema_version",
    ];
    // Write-side constants deliberately deferred to the next writer-port wave.
    const WRITE_SIDE_DEFERRED: &[&str] = &[
        "checking_ttl_seconds",
        "default_ready_evidence_ttl_seconds",
        "file_mode_octal",
        "prerequisite_renewal_statuses",
        "provider_env_by_name",
        "runtime_failure_aggregates",
        "runtime_failure_components",
        "runtime_failure_rejected_reasons",
    ];
    assert_eq!(brain_state_keys().len(), 32);
    for key in brain_state_keys() {
        assert!(
            CONSUMED.contains(&key.as_str()) || WRITE_SIDE_DEFERRED.contains(&key.as_str()),
            "unclassified brain_state vocabulary key: {key}"
        );
    }
}

#[test]
fn fixed_fingerprint_key_length_config_diagnostic_alias_and_inspection_statuses_match_the_fixture()
{
    let vocabulary = &local_contract().brain_state;
    assert_eq!(vocabulary.fingerprint_key_bytes, FINGERPRINT_KEY_BYTES);
    assert_eq!(
        vocabulary.config_diagnostic_fields,
        vocabulary
            .diagnostic_metadata_schemas
            .get("configuration_invalid")
            .and_then(|schema| schema.get("field"))
            .cloned()
            .expect("configuration-invalid field schema")
    );
    let mut statuses = vocabulary.inspection_statuses.clone();
    statuses.sort();
    assert_eq!(
        statuses,
        vec![
            "corrupt".to_owned(),
            "ok".to_owned(),
            "unavailable".to_owned(),
        ]
    );
}

#[test]
fn closed_schema_reports_alphabetically_smallest_unknown_key() {
    let value = serde_json::json!({
        "zebra": true,
        "alpha": true,
    });
    let error = validate_brain_state_record(&value, fixture_now()).unwrap_err();
    assert_eq!(error.path, "alpha");
}

#[test]
fn diagnostics_and_component_reason_rules_are_reason_scoped() {
    let mut top_level = valid_bundled_record().clone();
    top_level["aggregate_state"] = Value::String("unknown".to_owned());
    top_level["reason_code"] = Value::String("configuration_invalid".to_owned());
    top_level["diagnostic"] = serde_json::json!({"field": "not-a-config-field"});
    assert_eq!(
        validate_brain_state_record(&top_level, fixture_now())
            .unwrap_err()
            .path,
        "diagnostic.field"
    );

    let mut component = valid_bundled_record().clone();
    component["evidence"]["configuration"]["status"] = Value::String("blocked".to_owned());
    component["evidence"]["configuration"]["reason_code"] =
        Value::String("thinking_engine_not_chosen".to_owned());
    component["evidence"]["configuration"]["diagnostic"] = serde_json::json!({"field": "x"});
    assert_eq!(
        validate_brain_state_record(&component, fixture_now())
            .unwrap_err()
            .path,
        "evidence.configuration.diagnostic.field"
    );

    let mut not_attempted = valid_bundled_record().clone();
    not_attempted["evidence"]["configuration"]["status"] =
        Value::String("not_attempted".to_owned());
    not_attempted["evidence"]["configuration"]["reason_code"] =
        Value::String("thinking_engine_not_chosen".to_owned());
    assert_eq!(
        validate_brain_state_record(&not_attempted, fixture_now())
            .unwrap_err()
            .path,
        "evidence.configuration.status"
    );
}

#[test]
fn evidence_must_be_a_present_object() {
    let mut missing = projection_fixture()
        .records
        .get("lane_none/ready")
        .expect("none-lane ready record")
        .clone();
    missing
        .as_object_mut()
        .expect("record object")
        .remove("evidence");
    let missing_error = validate_brain_state_record(&missing, fixture_now()).unwrap_err();
    assert_eq!(missing_error.path, "evidence");
    assert_eq!(missing_error.reason, "must be an object");

    let mut null = projection_fixture()
        .records
        .get("lane_none/ready")
        .expect("none-lane ready record")
        .clone();
    null["evidence"] = Value::Null;
    let null_error = validate_brain_state_record(&null, fixture_now()).unwrap_err();
    assert_eq!(null_error.path, "evidence");
    assert_eq!(null_error.reason, "must be an object");
}

#[test]
fn checking_and_runtime_marker_shapes_match_python_validation() {
    let mut checking = projection_fixture()
        .records
        .get("lane_bundled/checking_fresh")
        .expect("checking record")
        .clone();
    checking["checking"]["fingerprint_sha256"] = Value::Null;
    checking["checking"]["checking_revision"] = Value::from(-1);
    assert!(validate_brain_state_record(&checking, fixture_now()).is_ok());

    checking["checking"]["run_id"] = Value::String(String::new());
    assert_eq!(
        validate_brain_state_record(&checking, fixture_now())
            .unwrap_err()
            .path,
        "checking.run_id"
    );

    let mut marker = projection_fixture()
        .records
        .get("lane_bundled/marker_superseded")
        .expect("superseded marker record")
        .clone();
    marker["runtime_failure_marker"]["revision"] = Value::from(-1);
    assert!(validate_brain_state_record(&marker, fixture_now()).is_ok());
    marker["runtime_failure_marker"]["marker_id"] = Value::String(String::new());
    assert_eq!(
        validate_brain_state_record(&marker, fixture_now())
            .unwrap_err()
            .path,
        "runtime_failure_marker.marker_id"
    );
}

#[test]
fn invalid_runtime_health_files_are_corrupt() {
    let journal = TestJournal::new();
    let runtime_dir = journal.path.join("health/providers/runtime");
    fs::create_dir_all(&runtime_dir).expect("runtime directory");
    let absent = inspect_runtime_health(&journal.path);
    assert_eq!(absent.status, "ok");
    assert_eq!(absent.record_kind.as_deref(), Some("health"));
    assert_eq!(
        absent
            .record
            .as_ref()
            .and_then(Value::as_object)
            .and_then(|record| record.get("desired_fingerprint_sha256")),
        Some(&Value::Null)
    );
    for (name, field, value) in [
        ("phase", "phase", serde_json::json!("unknown-phase")),
        ("reason", "reason_code", serde_json::json!("unknown-reason")),
        ("revision", "revision", serde_json::json!(-1)),
        ("detail", "detail", serde_json::json!("not-an-object")),
        ("process", "process", serde_json::json!("not-an-object")),
        ("schema", "schema_version", serde_json::json!(2)),
    ] {
        let mut record = serde_json::json!({
            "phase": "ready",
            "detail": {},
            "process": null,
            "reason_code": null,
        });
        record[field] = value;
        fs::write(
            runtime_dir.join("local.json"),
            serde_json::to_vec(&record).expect("runtime JSON"),
        )
        .expect("write runtime record");
        let inspection = inspect_runtime_health(&journal.path);
        assert_eq!(inspection.status, "corrupt", "{name}");
        assert_eq!(inspection.record_kind.as_deref(), Some("health"), "{name}");
    }
}

#[test]
fn lease_probe_error_returns_interrupted_projection_with_error() {
    let journal = TestJournal::new();
    journal.health();
    let record = projection_fixture()
        .records
        .get("lane_bundled/checking_fresh")
        .expect("checking record");
    fs::write(
        brain_state_path(&journal.path),
        serde_json::to_vec(record).expect("record JSON"),
    )
    .expect("write record");
    fs::write(
        brain_fingerprint_key_path(&journal.path),
        hex(&projection_fixture().hmac_key_hex),
    )
    .expect("write key");
    fs::create_dir(brain_refresh_lease_path(&journal.path)).expect("lease directory");

    let inspection = inspect_brain_state(&journal.path, bundled_config(), fixture_now());
    assert_eq!(inspection.status, InspectionStatus::Ok);
    assert_eq!(
        inspection.projection.reason_code.as_deref(),
        Some("brain_check_interrupted")
    );
    assert_eq!(
        inspection.projection.active_lane.as_deref(),
        Some("bundled")
    );
    assert!(inspection.error.is_some());
}

#[test]
fn none_lane_inspection_without_key_reports_fingerprint_key_unavailable() {
    let journal = TestJournal::new();
    write_valid_bundled_record(&journal);
    let config = serde_json::json!({
        "providers": {"active": {"provider": "none"}},
    });

    let inspection = inspect_brain_state(&journal.path, object(&config), fixture_now());
    assert_eq!(inspection.status, InspectionStatus::Ok);
    assert_eq!(
        inspection.projection.reason_code.as_deref(),
        Some("fingerprint_key_unavailable")
    );
}

#[test]
fn inspection_outcomes_are_real_filesystem_conditions() {
    let journal = TestJournal::new();
    let missing = inspect_brain_state(&journal.path, bundled_config(), fixture_now());
    assert_eq!(missing.status, InspectionStatus::Unavailable);
    assert_eq!(
        missing.projection.reason_code.as_deref(),
        Some("brain_record_missing")
    );

    fs::write(journal.path.join("health"), b"not a directory").expect("blocking health file");
    let unavailable = inspect_brain_state(&journal.path, bundled_config(), fixture_now());
    assert_eq!(unavailable.status, InspectionStatus::Unavailable);
    assert_eq!(
        unavailable.projection.reason_code.as_deref(),
        Some("brain_record_unavailable")
    );
    fs::remove_file(journal.path.join("health")).expect("remove blocking file");

    journal.health();
    fs::write(brain_state_path(&journal.path), b"{").expect("corrupt record");
    let corrupt = inspect_brain_state(&journal.path, bundled_config(), fixture_now());
    assert_eq!(corrupt.status, InspectionStatus::Corrupt);
    assert_eq!(
        corrupt.projection.reason_code.as_deref(),
        Some("brain_record_invalid")
    );

    write_valid_bundled_record(&journal);
    let key_missing = inspect_brain_state(&journal.path, bundled_config(), fixture_now());
    assert_eq!(key_missing.status, InspectionStatus::Ok);
    assert_eq!(
        key_missing.projection.reason_code.as_deref(),
        Some("fingerprint_key_unavailable")
    );
}

#[cfg(unix)]
#[test]
fn lease_and_fingerprint_key_probes_are_read_only() {
    let journal = TestJournal::new();
    let lease = brain_refresh_lease_path(&journal.path);
    assert!(!probe_file_lease_held(&lease).expect("absent lease"));
    assert!(!lease.exists(), "probe must not create an absent lease");

    journal.health();
    File::create(&lease).expect("lease file");
    assert!(!probe_file_lease_held(&lease).expect("unheld lease"));

    let lock_file = OpenOptions::new()
        .read(true)
        .write(true)
        .open(&lease)
        .expect("lease open");
    let lock = Flock::lock(lock_file, FlockArg::LockExclusive).expect("lease lock");
    assert!(probe_file_lease_held(&lease).expect("held lease"));
    drop(lock);

    assert_eq!(load_existing_fingerprint_key(&journal.path), None);
    assert!(!brain_fingerprint_key_path(&journal.path).exists());
    fs::write(brain_fingerprint_key_path(&journal.path), [7_u8; 16]).expect("short key");
    assert_eq!(load_existing_fingerprint_key(&journal.path), None);
}

#[cfg(unix)]
#[test]
fn read_operations_preserve_every_journal_file_across_the_full_matrix() {
    for record_kind in ["present", "absent", "corrupt"] {
        for key_kind in ["present", "absent", "wrong-length"] {
            for lease_kind in ["held", "free", "absent"] {
                let journal = TestJournal::new();
                journal.health();
                match record_kind {
                    "present" => write_valid_bundled_record(&journal),
                    "corrupt" => {
                        fs::write(brain_state_path(&journal.path), b"{").expect("corrupt record")
                    }
                    _ => {}
                }
                match key_kind {
                    "present" => fs::write(brain_fingerprint_key_path(&journal.path), [9_u8; 32])
                        .expect("full key"),
                    "wrong-length" => {
                        fs::write(brain_fingerprint_key_path(&journal.path), [9_u8; 16])
                            .expect("short key")
                    }
                    _ => {}
                }
                let lease_path = brain_refresh_lease_path(&journal.path);
                let held_lock = match lease_kind {
                    "held" => {
                        File::create(&lease_path).expect("lease file");
                        Some(
                            Flock::lock(
                                OpenOptions::new()
                                    .read(true)
                                    .write(true)
                                    .open(&lease_path)
                                    .expect("lease open"),
                                FlockArg::LockExclusive,
                            )
                            .expect("lease lock"),
                        )
                    }
                    "free" => {
                        File::create(&lease_path).expect("lease file");
                        None
                    }
                    _ => None,
                };
                let before = snapshot_tree(&journal.path);
                let _ = inspect_brain_state(&journal.path, bundled_config(), fixture_now());
                let _ = probe_file_lease_held(&lease_path).expect("lease probe");
                let _ = load_existing_fingerprint_key(&journal.path);
                let after = snapshot_tree(&journal.path);
                assert_eq!(
                    before, after,
                    "record={record_kind} key={key_kind} lease={lease_kind}"
                );
                drop(held_lock);
            }
        }
    }
}
