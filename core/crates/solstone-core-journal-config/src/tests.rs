// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::cell::RefCell;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};

use crate::defaults::{
    IdentityError, IdentitySource, IdentitySourceGuard, fallback_identity, install_identity_source,
};
use crate::read::{ReadSource, install_read_source};
use crate::test_support::TempDir;
use crate::{
    ConfigLoadError, get_journal_config_path, load_mutation_base, materialized_defaults,
    plain_defaults, read_journal_config,
};

const REAL: &str = "Ada Lovelace";
const USER: &str = "ada";
const TZ: &str = "America/Denver";
const REAL_ONLY: &str = "Grace Hopper";
const COMMA: &str = "Lovelace, Ada";
const PADDED: &str = "\u{001c}  Ada Lovelace  \u{001f}";
const CONTROL: &str = "Ada\u{0001}Lovelace";
const NUL: &str = "Ada\0Lovelace";
const USER_NUL: &str = "ada\0hopper";
const TZ_CONTROL: &str = "America\u{0001}Denver";
const TZ_NUL: &str = "America\0Denver";
const UNICODE_REAL_PADDED: &str = "  Ada Löveläce 好  ";
const UNICODE_REAL: &str = "Ada Löveläce 好";
const UNICODE_USER_PADDED: &str = "\u{001c}  adá  \u{001f}";
const UNICODE_USER: &str = "adá";

type ScriptedResult = Result<String, IdentityError>;

struct ScriptedReadSource {
    result: RefCell<Option<io::Result<Vec<u8>>>>,
    calls: Arc<AtomicUsize>,
}

impl ScriptedReadSource {
    fn new(result: io::Result<Vec<u8>>) -> Self {
        Self {
            result: RefCell::new(Some(result)),
            calls: Arc::new(AtomicUsize::new(0)),
        }
    }
}

impl ReadSource for ScriptedReadSource {
    fn read_bytes(&self, _path: &Path) -> io::Result<Vec<u8>> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        self.result
            .borrow_mut()
            .take()
            .expect("read seam must be called once")
    }
}

struct ScriptedIdentitySource {
    real_name: RefCell<Option<ScriptedResult>>,
    user_name: RefCell<Option<ScriptedResult>>,
    timezone: RefCell<Option<ScriptedResult>>,
}

impl ScriptedIdentitySource {
    fn new(real_name: ScriptedResult, user_name: ScriptedResult, timezone: ScriptedResult) -> Self {
        Self {
            real_name: RefCell::new(Some(real_name)),
            user_name: RefCell::new(Some(user_name)),
            timezone: RefCell::new(Some(timezone)),
        }
    }
}

impl IdentitySource for ScriptedIdentitySource {
    fn real_name(&self) -> ScriptedResult {
        self.real_name
            .borrow_mut()
            .take()
            .expect("real_name seam must be called once")
    }

    fn user_name(&self) -> ScriptedResult {
        self.user_name
            .borrow_mut()
            .take()
            .expect("user_name seam must be called once")
    }

    fn timezone(&self) -> ScriptedResult {
        self.timezone
            .borrow_mut()
            .take()
            .expect("timezone seam must be called once")
    }
}

fn injected_error() -> IdentityError {
    IdentityError::from("injected source failure")
}

fn ok(value: &str) -> ScriptedResult {
    Ok(value.to_owned())
}

fn err() -> ScriptedResult {
    Err(injected_error())
}

fn install(
    real_name: ScriptedResult,
    user_name: ScriptedResult,
    timezone: ScriptedResult,
) -> IdentitySourceGuard {
    install_identity_source(Rc::new(ScriptedIdentitySource::new(
        real_name, user_name, timezone,
    )))
}

fn reference_defaults() -> Map<String, Value> {
    let path =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/journal_default.json");
    let contents = fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("read reference defaults at {}: {error}", path.display()));
    serde_json::from_str::<Value>(&contents)
        .expect("parse reference defaults")
        .as_object()
        .cloned()
        .expect("reference defaults must be an object")
}

fn write_config(temporary: &TempDir, bytes: &[u8]) -> PathBuf {
    let path = get_journal_config_path(temporary.path());
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(&path, bytes).unwrap();
    path
}

fn assert_corrupt<T>(result: Result<T, ConfigLoadError>) -> ConfigLoadError {
    match result {
        Err(error @ ConfigLoadError::Corrupt { .. }) => error,
        Ok(_) => panic!("expected corrupt configuration error"),
    }
}

fn identity(config: &Map<String, Value>) -> &Map<String, Value> {
    config
        .get("identity")
        .and_then(Value::as_object)
        .expect("identity object")
}

fn assert_identity(config: &Map<String, Value>, name: &str, preferred: &str, timezone: &str) {
    let identity = identity(config);
    assert_eq!(identity["name"], json!(name));
    assert_eq!(identity["preferred"], json!(preferred));
    assert_eq!(identity["timezone"], json!(timezone));
}

fn assert_fallback(
    real_name: ScriptedResult,
    user_name: ScriptedResult,
    timezone: ScriptedResult,
    name: &str,
    preferred: &str,
    zone: &str,
) {
    let got = fallback_identity(real_name, user_name, timezone);
    assert_eq!(
        got,
        (name.to_owned(), preferred.to_owned(), zone.to_owned())
    );
}

#[test]
fn plain_defaults_match_the_python_reference_with_top_level_order() {
    let reference = reference_defaults();
    let defaults = plain_defaults();

    assert_eq!(defaults, reference);
    assert_eq!(
        defaults.keys().collect::<Vec<_>>(),
        reference.keys().collect::<Vec<_>>()
    );
}

#[test]
fn materialized_defaults_only_change_identity_resolution_fields() {
    let _guard = install(ok(REAL), ok(USER), ok(TZ));
    let plain = plain_defaults();
    let materialized = materialized_defaults();

    assert_eq!(plain.len(), 9);
    assert_eq!(materialized.len(), 9);
    let mut plain_without_identity = plain.clone();
    let mut materialized_without_identity = materialized.clone();
    plain_without_identity.remove("identity");
    materialized_without_identity.remove("identity");
    assert_eq!(plain_without_identity, materialized_without_identity);

    let plain_identity = identity(&plain);
    let materialized_identity = identity(&materialized);
    for key in ["name", "preferred", "timezone"] {
        assert_ne!(plain_identity.get(key), materialized_identity.get(key));
    }
    let mut plain_identity = plain_identity.clone();
    let mut materialized_identity = materialized_identity.clone();
    for key in ["name", "preferred", "timezone"] {
        plain_identity.remove(key);
        materialized_identity.remove(key);
    }
    assert_eq!(plain_identity, materialized_identity);
}

#[test]
fn materialized_defaults_preserve_reference_contract_fields() {
    let reference = reference_defaults();
    let _guard = install(err(), err(), err());
    let materialized = materialized_defaults();

    assert_eq!(
        materialized["retention"]["journal_logs"]["days"],
        reference["retention"]["journal_logs"]["days"]
    );
    assert_eq!(
        materialized["retention"]["storage_warning_disk_percent"],
        reference["retention"]["storage_warning_disk_percent"]
    );
    assert_eq!(
        materialized["retention"]["per_stream"],
        reference["retention"]["per_stream"]
    );
    assert_eq!(
        materialized["transcribe"]["max_concurrent"],
        reference["transcribe"]["max_concurrent"]
    );
    assert_eq!(
        materialized["describe"]["redact"],
        reference["describe"]["redact"]
    );
}

#[test]
fn materialization_uses_os_identity_and_timezone_but_missing_read_does_not() {
    let temporary = TempDir::new();
    let _guard = install(ok(REAL), ok(USER), ok(TZ));
    let materialized = materialized_defaults();
    let reader = read_journal_config(temporary.path()).unwrap();

    assert_identity(&materialized, REAL, USER, TZ);
    assert!(!reader.present);
    assert_eq!(reader.config, None);
    assert_eq!(identity(&plain_defaults())["name"], json!(""));
    assert_eq!(identity(&plain_defaults())["preferred"], json!(""));
    assert_eq!(identity(&plain_defaults())["timezone"], json!(""));
}

#[test]
fn fallback_identity_fills_both_name_fields_from_username() {
    assert_fallback(err(), ok(USER), ok(TZ), USER, USER, TZ);
    assert_fallback(ok(""), ok(USER), ok(TZ), USER, USER, TZ);
    assert_fallback(ok(CONTROL), ok(USER), ok(TZ), USER, USER, TZ);
    assert_fallback(ok(NUL), ok(USER), ok(TZ), USER, USER, TZ);
}

#[test]
fn materialized_defaults_fill_both_name_fields_from_username_on_real_name_error() {
    let _guard = install(err(), ok(USER), ok(TZ));
    assert_identity(&materialized_defaults(), USER, USER, TZ);
}

#[test]
fn materialized_defaults_fill_both_name_fields_from_username_on_real_name_empty() {
    let _guard = install(ok(""), ok(USER), ok(TZ));
    assert_identity(&materialized_defaults(), USER, USER, TZ);
}

#[test]
fn materialized_defaults_fill_both_name_fields_from_username_on_real_name_rejected() {
    let _guard = install(ok(CONTROL), ok(USER), ok(TZ));
    assert_identity(&materialized_defaults(), USER, USER, TZ);
    drop(_guard);
    let _guard = install(ok(NUL), ok(USER), ok(TZ));
    assert_identity(&materialized_defaults(), USER, USER, TZ);
}

#[test]
fn fallback_identity_keeps_distinct_real_name_when_username_fails() {
    assert_fallback(ok(REAL_ONLY), err(), ok(TZ), REAL_ONLY, "", TZ);
    assert_fallback(ok(REAL_ONLY), ok(CONTROL), ok(TZ), REAL_ONLY, "", TZ);
}

#[test]
fn materialized_defaults_keep_distinct_real_name_when_username_fails() {
    let _guard = install(ok(REAL_ONLY), err(), ok(TZ));
    assert_identity(&materialized_defaults(), REAL_ONLY, "", TZ);
    drop(_guard);
    let _guard = install(ok(REAL_ONLY), ok(CONTROL), ok(TZ));
    assert_identity(&materialized_defaults(), REAL_ONLY, "", TZ);
}

#[test]
fn materialization_handles_independent_os_source_failures() {
    assert_fallback(err(), err(), ok(TZ), "", "", TZ);
    let _guard = install(err(), err(), ok(TZ));
    assert_identity(&materialized_defaults(), "", "", TZ);
    drop(_guard);

    assert_fallback(ok(REAL), ok(USER), err(), REAL, USER, "");
    let _guard = install(ok(REAL), ok(USER), err());
    assert_identity(&materialized_defaults(), REAL, USER, "");
}

#[test]
fn fallback_identity_rejects_timezone_nul_and_control_without_touching_names() {
    assert_fallback(ok(REAL_ONLY), ok(USER), ok(TZ_NUL), REAL_ONLY, USER, "");
    assert_fallback(ok(REAL_ONLY), ok(USER), ok(TZ_CONTROL), REAL_ONLY, USER, "");
}

#[test]
fn materialized_defaults_reject_timezone_nul_and_control_without_touching_names() {
    let _guard = install(ok(REAL_ONLY), ok(USER), ok(TZ_NUL));
    assert_identity(&materialized_defaults(), REAL_ONLY, USER, "");
    drop(_guard);
    let _guard = install(ok(REAL_ONLY), ok(USER), ok(TZ_CONTROL));
    assert_identity(&materialized_defaults(), REAL_ONLY, USER, "");
}

#[test]
fn fallback_identity_rejects_username_nul_and_empty_without_erasing_real_name() {
    assert_fallback(ok(REAL_ONLY), ok(USER_NUL), ok(TZ), REAL_ONLY, "", TZ);
    assert_fallback(ok(REAL_ONLY), ok(""), ok(TZ), REAL_ONLY, "", TZ);
}

#[test]
fn materialized_defaults_reject_username_nul_and_empty_without_erasing_real_name() {
    let _guard = install(ok(REAL_ONLY), ok(USER_NUL), ok(TZ));
    assert_identity(&materialized_defaults(), REAL_ONLY, "", TZ);
    drop(_guard);
    let _guard = install(ok(REAL_ONLY), ok(""), ok(TZ));
    assert_identity(&materialized_defaults(), REAL_ONLY, "", TZ);
}

#[test]
fn fallback_identity_normalizes_and_rejects_control_empty_and_keeps_comma() {
    assert_fallback(ok(PADDED), ok(USER), ok(TZ), REAL, USER, TZ);
    assert_fallback(ok("   "), ok(USER), ok(TZ), USER, USER, TZ);
    assert_fallback(ok(CONTROL), ok(USER), ok(TZ), USER, USER, TZ);
    assert_fallback(ok(NUL), ok(USER), ok(TZ), USER, USER, TZ);
    assert_fallback(ok(COMMA), ok(USER), ok(TZ), COMMA, USER, TZ);
}

#[test]
fn fallback_identity_preserves_non_ascii_apart_from_edge_trimming() {
    assert_fallback(
        ok(UNICODE_REAL_PADDED),
        ok(UNICODE_USER_PADDED),
        ok(TZ),
        UNICODE_REAL,
        UNICODE_USER,
        TZ,
    );
}

#[test]
fn materialized_defaults_preserve_non_ascii_identity_apart_from_edge_trimming() {
    let _guard = install(ok(UNICODE_REAL_PADDED), ok(UNICODE_USER_PADDED), ok(TZ));
    assert_identity(&materialized_defaults(), UNICODE_REAL, UNICODE_USER, TZ);
}

#[test]
fn defaults_do_not_add_setup() {
    let _guard = install(err(), err(), err());
    assert!(!plain_defaults().contains_key("setup"));
    assert!(!materialized_defaults().contains_key("setup"));
}

#[ignore = "live OS identity oracle; run on Windows/MSVC and macOS"]
#[test]
fn materialized_defaults_match_live_os_identity_oracle() {
    let real_name = whoami::realname().map_err(IdentityError::from);
    let user_name = whoami::username().map_err(IdentityError::from);
    let timezone = iana_time_zone::get_timezone().map_err(IdentityError::from);
    let (name, preferred, zone) = fallback_identity(real_name, user_name, timezone);
    let materialized = materialized_defaults();
    assert_identity(&materialized, &name, &preferred, &zone);

    let mut plain = plain_defaults();
    let mut materialized_rest = materialized.clone();
    for config in [&mut plain, &mut materialized_rest] {
        let identity = config
            .get_mut("identity")
            .and_then(Value::as_object_mut)
            .expect("identity object");
        identity.remove("name");
        identity.remove("preferred");
        identity.remove("timezone");
    }
    assert_eq!(plain, materialized_rest);
}

#[test]
fn permission_denied_is_corrupt_for_both_entry_points_without_modifying_disk() {
    let temporary = TempDir::new();
    let original = b"{\"existing\":true}\n";
    let path = write_config(&temporary, original);

    let source = Rc::new(ScriptedReadSource::new(Err(io::Error::new(
        io::ErrorKind::PermissionDenied,
        "injected permission denied",
    ))));
    let _guard = install_read_source(source);
    assert_corrupt(read_journal_config(temporary.path()));
    assert_eq!(fs::read(&path).unwrap(), original);
    drop(_guard);

    let source = Rc::new(ScriptedReadSource::new(Err(io::Error::new(
        io::ErrorKind::PermissionDenied,
        "injected permission denied",
    ))));
    let _guard = install_read_source(source);
    assert_corrupt(load_mutation_base(temporary.path()));
    assert_eq!(fs::read(&path).unwrap(), original);
}

#[test]
fn injected_not_found_is_missing_even_when_the_file_exists() {
    let temporary = TempDir::new();
    let original = b"{\"existing\":true}\n";
    let path = write_config(&temporary, original);
    let source = Rc::new(ScriptedReadSource::new(Err(io::Error::new(
        io::ErrorKind::NotFound,
        "injected mid-flight disappearance",
    ))));
    let _guard = install_read_source(source);

    let read = read_journal_config(temporary.path()).unwrap();
    assert!(!read.present);
    assert_eq!(read.sha256, None);
    assert_eq!(read.config, None);
    assert_eq!(fs::read(&path).unwrap(), original);
}

#[test]
fn corrupt_content_is_rejected_without_modifying_disk_for_both_entry_points() {
    let cases = [
        ("truncated", b"{\"partial\":".as_slice()),
        ("non-json", b"not json".as_slice()),
        ("empty", b"".as_slice()),
        ("array", b"[]".as_slice()),
        ("scalar", b"42".as_slice()),
        ("non-utf8", b"\xff\xfe".as_slice()),
    ];
    for (name, original) in cases {
        let temporary = TempDir::new();
        let path = write_config(&temporary, original);
        assert_corrupt(read_journal_config(temporary.path()));
        assert_eq!(fs::read(&path).unwrap(), original, "reader changed {name}");
        assert_corrupt(load_mutation_base(temporary.path()));
        assert_eq!(
            fs::read(&path).unwrap(),
            original,
            "mutation base changed {name}"
        );
    }
}

#[test]
fn bare_nan_is_corrupt_without_modifying_disk() {
    let temporary = TempDir::new();
    let original = b"{\"value\":NaN}\n";
    let path = write_config(&temporary, original);

    assert_corrupt(read_journal_config(temporary.path()));
    assert_eq!(fs::read(path).unwrap(), original);
}

#[test]
fn u64_max_round_trips_through_the_config_map() {
    let temporary = TempDir::new();
    write_config(&temporary, b"{\"maximum\":18446744073709551615}\n");

    let read = read_journal_config(temporary.path()).unwrap();
    let config = read.config.expect("present config map");
    assert_eq!(config["maximum"].as_u64(), Some(u64::MAX));
}

#[test]
fn corrupt_display_preserves_the_owner_voice_message() {
    let temporary = TempDir::new();
    let path = get_journal_config_path(temporary.path());
    let error = ConfigLoadError::Corrupt {
        path: path.clone(),
        source: Box::new(io::Error::other("bad config")),
    };

    assert_eq!(
        error.to_string(),
        format!(
            "your settings file at {} couldn't be read. your settings were NOT changed. repair the file or restore config/journal.json from a backup, then try again.",
            path.display()
        )
    );
}

#[test]
fn real_missing_config_is_read_once_and_does_not_materialize() {
    let temporary = TempDir::new();
    let read = read_journal_config(temporary.path()).unwrap();

    assert!(!read.present);
    assert_eq!(read.sha256, None);
    assert_eq!(read.config, None);
    assert!(!get_journal_config_path(temporary.path()).exists());
    assert!(!temporary.path().join("config").exists());
}

#[test]
fn each_entry_point_invokes_the_read_seam_once() {
    let temporary = TempDir::new();
    let source = Rc::new(ScriptedReadSource::new(Ok(b"{}".to_vec())));
    let calls = Arc::clone(&source.calls);
    let _guard = install_read_source(source);
    read_journal_config(temporary.path()).unwrap();
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    drop(_guard);

    let _identity = install(err(), err(), err());
    let source = Rc::new(ScriptedReadSource::new(Ok(b"{}".to_vec())));
    let calls = Arc::clone(&source.calls);
    let _guard = install_read_source(source);
    load_mutation_base(temporary.path()).unwrap();
    assert_eq!(calls.load(Ordering::SeqCst), 1);
}

#[test]
fn digest_and_map_come_from_the_same_seam_bytes() {
    let temporary = TempDir::new();
    let bytes = b"{\"known\":\"value\"}\n".to_vec();
    let expected = format!("sha256:{:x}", Sha256::digest(&bytes));
    let source = Rc::new(ScriptedReadSource::new(Ok(bytes)));
    let _guard = install_read_source(source);

    let read = read_journal_config(temporary.path()).unwrap();
    assert!(read.present);
    assert_eq!(read.sha256.as_deref(), Some(expected.as_str()));
    assert_eq!(
        read.config,
        Some(Map::from_iter([("known".to_owned(), json!("value"))]))
    );
}

#[test]
fn empty_json_object_is_valid() {
    let temporary = TempDir::new();
    write_config(&temporary, b"{}");

    let read = read_journal_config(temporary.path()).unwrap();
    assert!(read.present);
    assert_eq!(read.config, Some(Map::new()));
    let base = load_mutation_base(temporary.path()).unwrap();
    assert!(!base.materialized);
    assert_eq!(base.config, Map::new());
}
