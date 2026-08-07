// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::error::Error;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};

use crate::defaults::{
    LocaltimeSource, PasswdRecord, PasswdSource, install_localtime_source, install_passwd_source,
};
use crate::read::{ReadSource, install_read_source};
use crate::test_support::TempDir;
use crate::{
    ConfigLoadError, get_journal_config_path, load_mutation_base, materialized_defaults,
    plain_defaults, read_journal_config,
};

type PasswdResult = Result<Option<PasswdRecord>, Box<dyn Error + Send + Sync>>;
type LocaltimeResult = Result<PathBuf, Box<dyn Error + Send + Sync>>;

struct ScriptedReadSource {
    result: Mutex<Option<io::Result<Vec<u8>>>>,
    calls: Arc<AtomicUsize>,
}

impl ScriptedReadSource {
    fn new(result: io::Result<Vec<u8>>) -> Self {
        Self {
            result: Mutex::new(Some(result)),
            calls: Arc::new(AtomicUsize::new(0)),
        }
    }
}

impl ReadSource for ScriptedReadSource {
    fn read_bytes(&self, _path: &Path) -> io::Result<Vec<u8>> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        self.result
            .lock()
            .unwrap()
            .take()
            .expect("read seam must be called once")
    }
}

struct ScriptedPasswdSource {
    result: Mutex<Option<PasswdResult>>,
}

impl ScriptedPasswdSource {
    fn new(result: PasswdResult) -> Self {
        Self {
            result: Mutex::new(Some(result)),
        }
    }
}

impl PasswdSource for ScriptedPasswdSource {
    fn current_user(&self) -> PasswdResult {
        self.result
            .lock()
            .unwrap()
            .take()
            .expect("passwd seam must be called once")
    }
}

struct ScriptedLocaltimeSource {
    result: Mutex<Option<LocaltimeResult>>,
}

impl ScriptedLocaltimeSource {
    fn new(result: LocaltimeResult) -> Self {
        Self {
            result: Mutex::new(Some(result)),
        }
    }
}

impl LocaltimeSource for ScriptedLocaltimeSource {
    fn resolved_localtime(&self) -> LocaltimeResult {
        self.result
            .lock()
            .unwrap()
            .take()
            .expect("localtime seam must be called once")
    }
}

fn boxed_error(kind: io::ErrorKind) -> Box<dyn Error + Send + Sync> {
    Box::new(io::Error::new(kind, "injected source failure"))
}

fn passwd(gecos: &str, login: &str) -> PasswdRecord {
    PasswdRecord {
        gecos: gecos.to_owned(),
        login: login.to_owned(),
    }
}

fn reference_defaults() -> Map<String, Value> {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../../solstone/think/journal_default.json");
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
    let passwd_guard = install_passwd_source(Rc::new(ScriptedPasswdSource::new(Ok(Some(passwd(
        "Ada Lovelace,,,",
        "ada",
    ))))));
    let localtime_guard = install_localtime_source(Rc::new(ScriptedLocaltimeSource::new(Ok(
        PathBuf::from("/usr/share/zoneinfo/America/Denver"),
    ))));
    let plain = plain_defaults();
    let materialized = materialized_defaults();
    drop(localtime_guard);
    drop(passwd_guard);

    assert_eq!(plain.len(), 11);
    assert_eq!(materialized.len(), 11);
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
    let passwd_guard = install_passwd_source(Rc::new(ScriptedPasswdSource::new(Ok(None))));
    let localtime_guard = install_localtime_source(Rc::new(ScriptedLocaltimeSource::new(Err(
        boxed_error(io::ErrorKind::NotFound),
    ))));
    let materialized = materialized_defaults();
    drop(localtime_guard);
    drop(passwd_guard);

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
    assert_eq!(materialized["agent"]["name"], reference["agent"]["name"]);
}

#[test]
fn materialization_uses_os_identity_and_timezone_but_missing_read_does_not() {
    let temporary = TempDir::new();
    let passwd_guard = install_passwd_source(Rc::new(ScriptedPasswdSource::new(Ok(Some(passwd(
        "Ada Lovelace,,,",
        "ada",
    ))))));
    let localtime_guard = install_localtime_source(Rc::new(ScriptedLocaltimeSource::new(Ok(
        PathBuf::from("/usr/share/zoneinfo/America/Denver"),
    ))));
    let materialized = materialized_defaults();
    let reader = read_journal_config(temporary.path()).unwrap();
    drop(localtime_guard);
    drop(passwd_guard);

    assert_eq!(identity(&materialized)["name"], json!("Ada Lovelace"));
    assert_eq!(identity(&materialized)["preferred"], json!("ada"));
    assert_eq!(identity(&materialized)["timezone"], json!("America/Denver"));
    assert!(!reader.present);
    assert_eq!(reader.config, None);
    assert_eq!(identity(&plain_defaults())["name"], json!(""));
    assert_eq!(identity(&plain_defaults())["preferred"], json!(""));
    assert_eq!(identity(&plain_defaults())["timezone"], json!(""));
}

#[test]
fn materialization_handles_independent_os_source_failures() {
    let passwd_guard = install_passwd_source(Rc::new(ScriptedPasswdSource::new(Err(boxed_error(
        io::ErrorKind::PermissionDenied,
    )))));
    let localtime_guard = install_localtime_source(Rc::new(ScriptedLocaltimeSource::new(Ok(
        PathBuf::from("/usr/share/zoneinfo/America/Denver"),
    ))));
    let passwd_failure = materialized_defaults();
    drop(localtime_guard);
    drop(passwd_guard);

    assert_eq!(identity(&passwd_failure)["name"], json!(""));
    assert_eq!(identity(&passwd_failure)["preferred"], json!(""));
    assert_eq!(
        identity(&passwd_failure)["timezone"],
        json!("America/Denver")
    );

    let passwd_guard = install_passwd_source(Rc::new(ScriptedPasswdSource::new(Ok(Some(passwd(
        "Ada Lovelace",
        "ada",
    ))))));
    let localtime_guard = install_localtime_source(Rc::new(ScriptedLocaltimeSource::new(Err(
        boxed_error(io::ErrorKind::NotFound),
    ))));
    let timezone_failure = materialized_defaults();
    drop(localtime_guard);
    drop(passwd_guard);

    assert_eq!(identity(&timezone_failure)["name"], json!("Ada Lovelace"));
    assert_eq!(identity(&timezone_failure)["preferred"], json!("ada"));
    assert_eq!(identity(&timezone_failure)["timezone"], json!(""));
}

#[test]
fn defaults_do_not_add_setup() {
    let passwd_guard = install_passwd_source(Rc::new(ScriptedPasswdSource::new(Ok(None))));
    let localtime_guard = install_localtime_source(Rc::new(ScriptedLocaltimeSource::new(Err(
        boxed_error(io::ErrorKind::NotFound),
    ))));
    assert!(!plain_defaults().contains_key("setup"));
    assert!(!materialized_defaults().contains_key("setup"));
    drop(localtime_guard);
    drop(passwd_guard);
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
            "I couldn't read your settings file at {}. Your settings were NOT changed. Repair the file or restore config/journal.json from a backup, then try again.",
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

    let passwd_guard = install_passwd_source(Rc::new(ScriptedPasswdSource::new(Ok(None))));
    let localtime_guard = install_localtime_source(Rc::new(ScriptedLocaltimeSource::new(Err(
        boxed_error(io::ErrorKind::NotFound),
    ))));
    let source = Rc::new(ScriptedReadSource::new(Ok(b"{}".to_vec())));
    let calls = Arc::clone(&source.calls);
    let _guard = install_read_source(source);
    load_mutation_base(temporary.path()).unwrap();
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    drop(_guard);
    drop(localtime_guard);
    drop(passwd_guard);
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
