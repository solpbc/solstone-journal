// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::fs;
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use rusqlite::Connection;
use serde_json::Value;
use sha2::{Digest, Sha256};
use solstone_core_body_rebuild::{BodyRebuildErrorKind, rebuild_body_store};

const NATIVE_CASE: &str = "oura_retain_parsed_one_row";
const NATIVE_BUNDLE: &str = "body-01J9ZK2F5M7Q8R3S4T6V0W1X2Z";
const DEDUPE_KEY: &str = "sha256:cf5b6fc199a3bcbc4d9361346d957f9098c356fe75f226803d2bd57580d95258";
const LEGACY_APPLE_KEY: &str = "apple-health:synthetic:legacy-1";
const VALUE_HASH: &str = "sha256:f3d64f3c75d8c78ebe82d09f697c4c050c2002d4ea1bb1a945a4e5ac1cb64297";

static TEMP_SEQUENCE: AtomicU64 = AtomicU64::new(0);

#[cfg(unix)]
fn symlink_file(original: &Path, link: &Path) -> std::io::Result<()> {
    std::os::unix::fs::symlink(original, link)
}

#[cfg(windows)]
fn symlink_file(original: &Path, link: &Path) -> std::io::Result<()> {
    std::os::windows::fs::symlink_file(original, link)
}

#[cfg(unix)]
fn symlink_dir(original: &Path, link: &Path) -> std::io::Result<()> {
    std::os::unix::fs::symlink(original, link)
}

#[cfg(windows)]
fn symlink_dir(original: &Path, link: &Path) -> std::io::Result<()> {
    std::os::windows::fs::symlink_dir(original, link)
}

struct TempDir {
    path: PathBuf,
}

impl TempDir {
    fn new() -> Self {
        let sequence = TEMP_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock after epoch")
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "solstone-body-rebuild-{}-{nanos}-{sequence}",
            std::process::id()
        ));
        fs::create_dir(&path).expect("temporary journal root creates");
        let path = fs::canonicalize(path).expect("temporary journal root canonicalizes");
        Self { path }
    }

    fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}

fn fixture() -> Value {
    let fixture_path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../../core/fixtures/body_source_native_bundle_v1.json");
    serde_json::from_slice(&fs::read(fixture_path).expect("fixture reads")).expect("fixture parses")
}

fn codec_fixture() -> Value {
    let fixture_path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../../core/fixtures/body_source_codec_rows.json");
    serde_json::from_slice(&fs::read(fixture_path).expect("codec fixture reads"))
        .expect("codec fixture parses")
}

fn native_case() -> Value {
    fixture()["cases"]
        .as_array()
        .expect("fixture cases")
        .iter()
        .find(|case| case["name"].as_str() == Some(NATIVE_CASE))
        .expect("native case exists")
        .clone()
}

fn discard_case() -> Value {
    fixture()["cases"]
        .as_array()
        .expect("fixture cases")
        .iter()
        .find(|case| case["name"].as_str() == Some("oura_discard_zero_rows"))
        .expect("discard case exists")
        .clone()
}

fn write_empty_native_bundle(journal: &Path, case: &Value) -> PathBuf {
    let directory = journal
        .join("imports")
        .join(case["directory"].as_str().expect("directory"));
    fs::create_dir_all(&directory).expect("native directory creates");
    fs::write(
        directory.join("manifest.json"),
        serde_json::to_vec(&case["manifest"]).expect("manifest serializes"),
    )
    .expect("manifest writes");
    fs::write(
        directory.join("body-bundle.json"),
        case["expected_envelope_jsonl"]
            .as_str()
            .expect("envelope bytes"),
    )
    .expect("envelope writes");
    fs::write(
        directory.join("body-ledger.jsonl"),
        case["expected_ledger_jsonl"]
            .as_str()
            .expect("ledger bytes"),
    )
    .expect("ledger writes");
    directory
}

fn write_native_bundle(journal: &Path, case: &Value) {
    let directory = journal
        .join("imports")
        .join(case["directory"].as_str().expect("directory"));
    let normalized = directory.join("normalized");
    fs::create_dir_all(&normalized).expect("native directories create");
    let raw_bytes = b"{\"synthetic\":true}\n";
    let raw_digest = format!("sha256:{:x}", Sha256::digest(raw_bytes));
    let inventory = format!(
        "{{\"bytes\":{},\"path\":\"oura/daily_readiness-0001.json\",\"sha256\":\"{raw_digest}\"}}\n",
        raw_bytes.len()
    );
    let inventory_digest = format!("sha256:{:x}", Sha256::digest(inventory.as_bytes()));

    let mut row: Value = serde_json::from_str(
        case["expected_normalized_jsonl"]
            .as_str()
            .expect("normalized bytes"),
    )
    .expect("normalized row parses");
    row["raw_inventory_sha256"] = Value::String(inventory_digest);
    let mut row_bytes = serde_json::to_vec(&row).expect("normalized row serializes");
    row_bytes.push(b'\n');
    let row_digest = format!("sha256:{:x}", Sha256::digest(&row_bytes));

    let mut event: Value = serde_json::from_str(
        case["expected_ledger_jsonl"]
            .as_str()
            .expect("ledger bytes"),
    )
    .expect("ledger event parses");
    event["row_sha256"] = Value::String(row_digest);
    let mut ledger_bytes = serde_json::to_vec(&event).expect("ledger event serializes");
    ledger_bytes.push(b'\n');

    let mut envelope: Value = serde_json::from_str(
        case["expected_envelope_jsonl"]
            .as_str()
            .expect("envelope bytes"),
    )
    .expect("envelope parses");
    envelope["shards"][0]["bytes"] = Value::from(row_bytes.len() as u64);
    envelope["shards"][0]["sha256"] =
        Value::String(format!("sha256:{:x}", Sha256::digest(&row_bytes)));
    envelope["ledger"]["bytes"] = Value::from(ledger_bytes.len() as u64);
    envelope["ledger"]["sha256"] =
        Value::String(format!("sha256:{:x}", Sha256::digest(&ledger_bytes)));
    let mut envelope_bytes = serde_json::to_vec(&envelope).expect("envelope serializes");
    envelope_bytes.push(b'\n');

    let mut manifest = case["manifest"].clone();
    manifest["body_bundle_sha256"] =
        Value::String(format!("sha256:{:x}", Sha256::digest(&envelope_bytes)));

    fs::write(
        directory.join("manifest.json"),
        serde_json::to_vec(&manifest).expect("manifest serializes"),
    )
    .expect("manifest writes");
    fs::write(directory.join("body-bundle.json"), envelope_bytes).expect("envelope writes");
    fs::write(directory.join("body-ledger.jsonl"), ledger_bytes).expect("ledger writes");
    fs::write(normalized.join("2026-01.jsonl"), row_bytes).expect("normalized shard writes");
    fs::create_dir_all(directory.join("raw/oura")).expect("raw directory creates");
    fs::write(
        directory.join("raw/oura/daily_readiness-0001.json"),
        raw_bytes,
    )
    .expect("raw asset writes");
    fs::write(directory.join("body-raw-inventory.jsonl"), inventory).expect("raw inventory writes");
}

fn write_legacy_predecessor(journal: &Path, case: &Value) {
    let directory = journal.join("imports/synthetic-import/normalized");
    fs::create_dir_all(&directory).expect("legacy directory creates");
    let mut row: Value = serde_json::from_str(
        case["expected_normalized_jsonl"]
            .as_str()
            .expect("normalized row"),
    )
    .expect("normalized row parses");
    row["import_id"] = Value::String("synthetic-import".to_owned());
    row["normalized_ref"] =
        Value::String("imports/synthetic-import/normalized/2026-01.jsonl#L1".to_owned());
    row["raw_ref"] = Value::String("raw/oura/daily_readiness-0001.json#item-0".to_owned());
    let mut bytes = serde_json::to_vec(&row).expect("legacy Oura row serializes");
    bytes.push(b'\n');
    let codec = codec_fixture();
    let mut apple = codec["rows"]
        .as_array()
        .expect("codec rows")
        .iter()
        .find(|row| row["name"].as_str() == Some("legacy_normalized_v1"))
        .expect("legacy Apple row exists")["row"]
        .clone();
    apple["import_id"] = Value::String("synthetic-import".to_owned());
    apple["normalized_ref"] =
        Value::String("imports/synthetic-import/normalized/2026-01.jsonl#L2".to_owned());
    bytes.extend_from_slice(&serde_json::to_vec(&apple).expect("legacy Apple row serializes"));
    bytes.push(b'\n');
    fs::write(directory.join("2026-01.jsonl"), bytes).expect("legacy row writes");
}

fn sqlite_rows(path: &Path) -> Vec<Vec<Option<String>>> {
    let connection = Connection::open(path).expect("database opens");
    let mut statement = connection
        .prepare(
            "SELECT dedupe_key, source_family, source_record_id, record_type,
                    start_time, end_time, value_hash, first_import_id,
                    last_seen_import_id, normalized_ref, raw_ref
             FROM health_dedupe ORDER BY dedupe_key",
        )
        .expect("row query prepares");
    statement
        .query_map([], |row| {
            (0..11)
                .map(|index| row.get(index))
                .collect::<rusqlite::Result<Vec<Option<String>>>>()
        })
        .expect("row query runs")
        .collect::<rusqlite::Result<Vec<_>>>()
        .expect("rows decode")
}

#[test]
fn rebuild_replays_legacy_then_native_and_publishes_private_deterministic_sqlite() {
    let temporary = TempDir::new();
    let case = native_case();
    write_legacy_predecessor(temporary.path(), &case);
    write_native_bundle(temporary.path(), &case);
    fs::create_dir_all(temporary.path().join("imports/unrelated-import"))
        .expect("unrelated import creates");
    #[cfg(unix)]
    fs::set_permissions(
        temporary.path().join("imports"),
        fs::Permissions::from_mode(0o755),
    )
    .expect("test loosens imports mode");

    let report = rebuild_body_store(temporary.path()).expect("rebuild succeeds");
    assert_eq!(report.legacy_bundles(), 1);
    assert_eq!(report.native_bundles(), 1);
    assert_eq!(report.rows(), 2);

    let database = temporary.path().join("imports/health-dedupe.sqlite");
    #[cfg(unix)]
    {
        assert_eq!(
            fs::metadata(temporary.path().join("imports"))
                .expect("imports metadata")
                .permissions()
                .mode()
                & 0o777,
            0o700
        );
        assert_eq!(
            fs::metadata(&database)
                .expect("database metadata")
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
    }
    assert!(
        !temporary
            .path()
            .join("imports/.health-dedupe.sqlite.rebuild")
            .exists()
    );
    for suffix in ["-wal", "-shm", "-journal"] {
        assert!(
            !temporary
                .path()
                .join(format!("imports/health-dedupe.sqlite{suffix}"))
                .exists()
        );
    }

    let rows = sqlite_rows(&database);
    assert_eq!(rows.len(), 2);
    let apple = rows
        .iter()
        .find(|row| row[0].as_deref() == Some(LEGACY_APPLE_KEY))
        .expect("legacy Apple row exists");
    assert_eq!(apple[1].as_deref(), Some("apple_health"));
    assert_eq!(apple[6], None);
    assert_eq!(apple[7].as_deref(), Some("synthetic-import"));
    assert_eq!(apple[8].as_deref(), Some("synthetic-import"));
    assert_eq!(
        apple[9].as_deref(),
        Some("imports/synthetic-import/normalized/2026-01.jsonl#L2")
    );

    let oura = rows
        .iter()
        .find(|row| row[0].as_deref() == Some(DEDUPE_KEY))
        .expect("Oura row exists");
    assert_eq!(oura[1].as_deref(), Some("oura_api"));
    assert_eq!(oura[2].as_deref(), Some("synthetic-readiness-1"));
    assert_eq!(oura[6].as_deref(), Some(VALUE_HASH));
    assert_eq!(oura[7].as_deref(), Some("synthetic-import"));
    assert_eq!(oura[8].as_deref(), Some(NATIVE_BUNDLE));
    assert_eq!(
        oura[9].as_deref(),
        Some("imports/body-01J9ZK2F5M7Q8R3S4T6V0W1X2Z/normalized/2026-01.jsonl#L1")
    );
    assert_eq!(
        oura[10].as_deref(),
        Some("imports/body-01J9ZK2F5M7Q8R3S4T6V0W1X2Z/raw/oura/daily_readiness-0001.json#item-0")
    );

    let connection = Connection::open(&database).expect("database reopens");
    let integrity: String = connection
        .query_row("PRAGMA integrity_check", [], |row| row.get(0))
        .expect("integrity query");
    assert_eq!(integrity, "ok");
    let indexes: Vec<String> = {
        let mut statement = connection
            .prepare(
                "SELECT name FROM sqlite_master
                 WHERE type='index' AND name LIKE 'idx_health_dedupe_%'
                 ORDER BY name",
            )
            .expect("index query prepares");
        statement
            .query_map([], |row| row.get(0))
            .expect("index query runs")
            .collect::<rusqlite::Result<Vec<_>>>()
            .expect("index names decode")
    };
    assert_eq!(
        indexes,
        [
            "idx_health_dedupe_record_time",
            "idx_health_dedupe_source_record"
        ]
    );
    drop(connection);

    let first_bytes = fs::read(&database).expect("first database reads");
    let second = rebuild_body_store(temporary.path()).expect("second rebuild succeeds");
    assert_eq!(second, report);
    assert_eq!(
        fs::read(&database).expect("second database reads"),
        first_bytes
    );
}

#[test]
fn native_authority_failure_preserves_the_previous_database_byte_for_byte() {
    let temporary = TempDir::new();
    let case = native_case();
    write_native_bundle(temporary.path(), &case);
    rebuild_body_store(temporary.path()).expect("baseline rebuild succeeds");
    let database = temporary.path().join("imports/health-dedupe.sqlite");
    let before = fs::read(&database).expect("baseline database reads");

    fs::create_dir_all(
        temporary
            .path()
            .join("imports/body-01J9ZK2F5M7Q8R3S4T6V0W1X32"),
    )
    .expect("torn native directory creates");
    let error = rebuild_body_store(temporary.path()).expect_err("torn native bundle refuses");
    assert_eq!(error.kind(), BodyRebuildErrorKind::Authority);
    assert_eq!(error.stage(), "native_authority");
    assert_eq!(
        error.to_string(),
        "body-rebuild authority: native_authority"
    );
    assert_eq!(format!("{error:?}"), error.to_string());
    assert_eq!(fs::read(&database).expect("database remains"), before);
    assert!(
        !temporary
            .path()
            .join("imports/.health-dedupe.sqlite.rebuild")
            .exists()
    );
}

#[test]
fn native_raw_retention_shape_is_fail_closed() {
    let retained = TempDir::new();
    let retained_case = native_case();
    write_native_bundle(retained.path(), &retained_case);
    fs::remove_file(
        retained
            .path()
            .join("imports")
            .join(retained_case["directory"].as_str().expect("directory"))
            .join("body-raw-inventory.jsonl"),
    )
    .expect("remove retained inventory");
    let error = rebuild_body_store(retained.path()).expect_err("retained inventory is required");
    assert_eq!(error.kind(), BodyRebuildErrorKind::NativeReplay);
    assert_eq!(error.stage(), "raw_inventory_missing");

    let discarded = TempDir::new();
    let discarded_bundle = write_empty_native_bundle(discarded.path(), &discard_case());
    fs::create_dir(discarded_bundle.join("raw")).expect("create forbidden raw directory");
    fs::write(
        discarded_bundle.join("raw/undeclared"),
        b"synthetic private body data",
    )
    .expect("write forbidden raw data");
    let error = rebuild_body_store(discarded.path()).expect_err("discard must exclude raw data");
    assert_eq!(error.kind(), BodyRebuildErrorKind::NativeReplay);
    assert_eq!(error.stage(), "raw_retention_mismatch");

    let deep = TempDir::new();
    let deep_case = native_case();
    write_native_bundle(deep.path(), &deep_case);
    let raw = deep
        .path()
        .join("imports")
        .join(deep_case["directory"].as_str().expect("directory"))
        .join("raw");
    let nested = (0..=128).fold(raw, |path, index| path.join(format!("d{index}")));
    fs::create_dir_all(nested).expect("create over-depth raw tree");
    let error = rebuild_body_store(deep.path()).expect_err("raw depth must be bounded");
    assert_eq!(error.kind(), BodyRebuildErrorKind::NativeReplay);
    assert_eq!(error.stage(), "raw_asset_depth_limit");
}

#[test]
fn escaping_imports_symlink_is_refused_before_any_outside_mutation() {
    let temporary = TempDir::new();
    let outside = temporary.path().join("outside");
    fs::create_dir(&outside).expect("outside directory creates");
    #[cfg(unix)]
    fs::set_permissions(&outside, fs::Permissions::from_mode(0o755)).expect("outside mode sets");
    symlink_dir(&outside, &temporary.path().join("imports")).expect("imports symlink creates");

    let error = rebuild_body_store(temporary.path()).expect_err("imports symlink refuses");
    assert_eq!(error.kind(), BodyRebuildErrorKind::Publication);
    assert_eq!(error.stage(), "imports_directory");
    #[cfg(unix)]
    assert_eq!(
        fs::metadata(&outside)
            .expect("outside metadata")
            .permissions()
            .mode()
            & 0o777,
        0o755
    );
    assert_eq!(fs::read_dir(&outside).expect("outside lists").count(), 0);
}

#[test]
fn existing_database_symlink_is_never_opened_or_replaced() {
    let temporary = TempDir::new();
    let imports = temporary.path().join("imports");
    fs::create_dir(&imports).expect("imports creates");
    let outside = temporary.path().join("outside.sqlite");
    fs::write(&outside, b"outside-owner-sentinel").expect("outside file writes");
    symlink_file(&outside, &imports.join("health-dedupe.sqlite"))
        .expect("database symlink creates");

    let error = rebuild_body_store(temporary.path()).expect_err("database symlink refuses");
    assert_eq!(error.kind(), BodyRebuildErrorKind::Publication);
    assert_eq!(error.stage(), "existing_database");
    assert_eq!(
        fs::read(&outside).expect("outside file remains"),
        b"outside-owner-sentinel"
    );
    assert!(imports.join("health-dedupe.sqlite").is_symlink());
    assert!(!imports.join(".health-dedupe.sqlite.rebuild").exists());
}
