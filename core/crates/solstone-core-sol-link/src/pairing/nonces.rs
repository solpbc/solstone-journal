// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Durable, one-shot pairing nonces.

use std::collections::BTreeMap;
use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use solstone_core_journal_io::{JsonWriteOptions, LockOptions, hold_lock, write_json};

/// Pairing-window lifetime in seconds.
pub const NONCE_TTL_SECONDS: i64 = 300;

/// The authority transport carried by a persisted pairing nonce.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum NonceKind {
    #[default]
    Direct,
    RelayV06,
}

/// One persisted pairing nonce.
#[derive(Clone, Deserialize, Eq, PartialEq, Serialize)]
pub struct Nonce {
    pub value: String,
    pub device_label: String,
    pub issued_at: i64,
    pub expires_at: i64,
    pub used: bool,
    pub role: String,
    pub same_machine: bool,
    #[serde(default)]
    pub kind: NonceKind,
}

impl fmt::Debug for Nonce {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("Nonce")
            .field("value", &"<redacted>")
            .field("device_label", &self.device_label)
            .field("issued_at", &self.issued_at)
            .field("expires_at", &self.expires_at)
            .field("used", &self.used)
            .field("role", &self.role)
            .field("same_machine", &self.same_machine)
            .field("kind", &self.kind)
            .finish()
    }
}

/// Failure while mutating the nonce store.
#[derive(Debug)]
pub enum NonceStoreError {
    Lock(solstone_core_journal_io::LockError),
    Write(solstone_core_journal_io::AtomicWriteError),
}

impl fmt::Display for NonceStoreError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Lock(error) => error.fmt(formatter),
            Self::Write(error) => error.fmt(formatter),
        }
    }
}

impl std::error::Error for NonceStoreError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Lock(error) => Some(error),
            Self::Write(error) => Some(error),
        }
    }
}

/// The `link/nonces.json` owner.
#[derive(Clone, Debug)]
pub struct NonceStore {
    path: PathBuf,
}

impl NonceStore {
    pub fn new(journal_root: &Path) -> Self {
        Self {
            path: journal_root.join("link").join("nonces.json"),
        }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Persist a new nonce after collecting the current locked state.
    pub fn add(
        &self,
        value: String,
        device_label: String,
        role: String,
        same_machine: bool,
        now: i64,
    ) -> Result<Nonce, NonceStoreError> {
        self.add_with_kind(
            value,
            device_label,
            role,
            same_machine,
            NonceKind::Direct,
            now,
        )
    }

    /// Persist a relay-v06 nonce after its remote registration succeeds.
    pub fn add_relay(
        &self,
        value: String,
        device_label: String,
        role: String,
        now: i64,
    ) -> Result<Nonce, NonceStoreError> {
        self.add_with_kind(value, device_label, role, false, NonceKind::RelayV06, now)
    }

    fn add_with_kind(
        &self,
        value: String,
        device_label: String,
        role: String,
        same_machine: bool,
        kind: NonceKind,
        now: i64,
    ) -> Result<Nonce, NonceStoreError> {
        let entry = Nonce {
            value: value.clone(),
            device_label,
            issued_at: now,
            expires_at: now + NONCE_TTL_SECONDS,
            used: false,
            role,
            same_machine,
            kind,
        };
        let _lock = hold_lock(&self.path, LockOptions::default()).map_err(NonceStoreError::Lock)?;
        let mut entries = self.read_entries();
        gc_entries(&mut entries, now);
        entries.insert(value, entry.clone());
        self.write_entries(&entries)?;
        Ok(entry)
    }

    /// Consume a currently valid nonce. GC intentionally runs before lookup.
    pub fn consume(&self, value: &str, now: i64) -> Result<Option<Nonce>, NonceStoreError> {
        let _lock = hold_lock(&self.path, LockOptions::default()).map_err(NonceStoreError::Lock)?;
        let mut entries = self.read_entries();
        let before = entries.len();
        gc_entries(&mut entries, now);
        if entries.len() != before {
            self.write_entries(&entries)?;
        }
        let Some(entry) = entries.get_mut(value) else {
            return Ok(None);
        };
        if entry.used || entry.expires_at <= now {
            return Ok(None);
        }
        entry.used = true;
        let consumed = entry.clone();
        self.write_entries(&entries)?;
        Ok(Some(consumed))
    }

    /// Read one nonce without locking or collecting. This is deliberately not a
    /// writer: the door's pairing-window predicate must observe `used` entries.
    pub fn peek(&self, value: &str) -> Option<Nonce> {
        self.read_entries().remove(value)
    }

    /// Read the current snapshot without locking or collecting.
    pub fn snapshot(&self) -> Vec<Nonce> {
        self.read_entries().into_values().collect()
    }

    /// Explicitly collect consumed and expired entries under the store lock.
    pub fn gc(&self, now: i64) -> Result<(), NonceStoreError> {
        let _lock = hold_lock(&self.path, LockOptions::default()).map_err(NonceStoreError::Lock)?;
        let mut entries = self.read_entries();
        let before = entries.len();
        gc_entries(&mut entries, now);
        if entries.len() != before {
            self.write_entries(&entries)?;
        }
        Ok(())
    }

    /// Cancel live relay-v06 windows without altering direct-pair authority.
    ///
    /// Consumed and expired relay entries are left for ordinary GC, so this
    /// returns only the windows whose authority was actively cancelled.
    pub fn cancel_all_relay_windows(&self, now: i64) -> Result<usize, NonceStoreError> {
        let _lock = hold_lock(&self.path, LockOptions::default()).map_err(NonceStoreError::Lock)?;
        let mut entries = self.read_entries();
        let before = entries.len();
        entries.retain(|_, entry| {
            entry.kind != NonceKind::RelayV06 || entry.used || entry.expires_at <= now
        });
        let removed = before - entries.len();
        if removed != 0 {
            self.write_entries(&entries)?;
        }
        Ok(removed)
    }

    /// Cancel one live relay-v06 window without altering any other authority.
    ///
    /// A consumed, expired, direct, or missing entry is retained and returns
    /// `false`, making repeated cancellation safe.
    pub fn cancel(&self, value: &str, now: i64) -> Result<bool, NonceStoreError> {
        let _lock = hold_lock(&self.path, LockOptions::default()).map_err(NonceStoreError::Lock)?;
        let mut entries = self.read_entries();
        let removable = entries.get(value).is_some_and(|entry| {
            entry.kind == NonceKind::RelayV06 && !entry.used && entry.expires_at > now
        });
        if removable {
            entries.remove(value);
            self.write_entries(&entries)?;
        }
        Ok(removable)
    }

    /// Missing, malformed, and unreadable files are deliberately an empty
    /// snapshot, exactly as the reference implementation specifies.
    fn read_entries(&self) -> BTreeMap<String, Nonce> {
        let Ok(bytes) = fs::read(&self.path) else {
            return BTreeMap::new();
        };
        let Ok(raw) = serde_json::from_slice::<serde_json::Value>(&bytes) else {
            return BTreeMap::new();
        };
        let Some(items) = raw.as_array() else {
            return BTreeMap::new();
        };
        items
            .iter()
            .filter_map(nonce_from_json)
            .map(|entry| (entry.value.clone(), entry))
            .collect()
    }

    fn write_entries(&self, entries: &BTreeMap<String, Nonce>) -> Result<(), NonceStoreError> {
        let values = entries.values().collect::<Vec<_>>();
        write_json(&self.path, &values, JsonWriteOptions::default()).map_err(NonceStoreError::Write)
    }
}

/// Whether any direct pairing nonce is live.
///
/// This predicate has no error channel. An unreadable store is a closed
/// window, never a request-time 500.
pub fn direct_pairing_window_open(store: &NonceStore, now: i64) -> bool {
    store
        .snapshot()
        .into_iter()
        .any(|nonce| nonce.kind == NonceKind::Direct && !nonce.used && nonce.expires_at > now)
}

/// Whether this exact relay-v06 nonce is live.
///
/// This is deliberately an exact-value predicate: a live relay nonce does not
/// authorize another relay carrier.
pub fn relay_pairing_nonce_open(store: &NonceStore, nonce_value: &str, now: i64) -> bool {
    store.peek(nonce_value).is_some_and(|nonce| {
        nonce.kind == NonceKind::RelayV06 && !nonce.used && nonce.expires_at > now
    })
}

fn gc_entries(entries: &mut BTreeMap<String, Nonce>, now: i64) {
    entries.retain(|_, entry| !entry.used && entry.expires_at > now);
}

fn nonce_from_json(value: &serde_json::Value) -> Option<Nonce> {
    let object = value.as_object()?;
    let nonce = object.get("value")?.as_str()?.to_owned();
    Some(Nonce {
        value: nonce,
        device_label: python_string(object.get("device_label")),
        issued_at: python_int(object.get("issued_at")),
        expires_at: python_int(object.get("expires_at")),
        used: python_bool(object.get("used")),
        role: object
            .get("role")
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default()
            .to_owned(),
        same_machine: python_bool(object.get("same_machine")),
        kind: nonce_kind(object.get("kind")),
    })
}

fn nonce_kind(value: Option<&serde_json::Value>) -> NonceKind {
    match value.and_then(serde_json::Value::as_str) {
        Some("relay_v06") => NonceKind::RelayV06,
        _ => NonceKind::Direct,
    }
}

fn python_string(value: Option<&serde_json::Value>) -> String {
    match value {
        Some(serde_json::Value::String(value)) => value.clone(),
        Some(value) => value.to_string(),
        None => String::new(),
    }
}

fn python_int(value: Option<&serde_json::Value>) -> i64 {
    match value {
        Some(serde_json::Value::Number(value)) => value.as_i64().unwrap_or_default(),
        Some(serde_json::Value::String(value)) => value.parse().unwrap_or_default(),
        _ => 0,
    }
}

fn python_bool(value: Option<&serde_json::Value>) -> bool {
    match value {
        Some(serde_json::Value::Bool(value)) => *value,
        Some(serde_json::Value::Number(value)) => value.as_i64().unwrap_or_default() != 0,
        Some(serde_json::Value::String(value)) => !value.is_empty(),
        Some(serde_json::Value::Array(value)) => !value.is_empty(),
        Some(serde_json::Value::Object(value)) => !value.is_empty(),
        Some(serde_json::Value::Null) | None => false,
    }
}

#[cfg(all(test, feature = "full-tests"))]
mod tests {
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::{SystemTime, UNIX_EPOCH};

    use super::*;

    static SEQUENCE: AtomicU64 = AtomicU64::new(0);

    struct TempDir(PathBuf);

    impl TempDir {
        fn new() -> Self {
            let sequence = SEQUENCE.fetch_add(1, Ordering::Relaxed);
            let nanos = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("clock")
                .as_nanos();
            let path =
                std::env::temp_dir().join(format!("solstone-pairing-nonces-{nanos}-{sequence}"));
            fs::create_dir(&path).expect("temporary journal creates");
            Self(path)
        }

        fn path(&self) -> &Path {
            &self.0
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn store() -> (TempDir, NonceStore) {
        let temporary = TempDir::new();
        let store = NonceStore::new(temporary.path());
        (temporary, store)
    }

    #[test]
    fn add_writes_reference_list_shape_and_only_lock_sidecar() {
        let (temporary, store) = store();
        let entry = store
            .add(
                "nonce".into(),
                "phone".into(),
                "observer".into(),
                false,
                100,
            )
            .expect("add nonce");
        assert_eq!(entry.expires_at, 400);
        let raw: serde_json::Value =
            serde_json::from_slice(&fs::read(store.path()).expect("store")).expect("json");
        assert!(raw.is_array());
        assert_eq!(raw[0]["value"], "nonce");
        assert_eq!(raw[0]["same_machine"], false);
        assert_eq!(raw[0]["kind"], "direct");
        let mut names = fs::read_dir(temporary.path().join("link"))
            .expect("link directory")
            .map(|entry| {
                entry
                    .expect("entry")
                    .file_name()
                    .into_string()
                    .expect("utf8")
            })
            .collect::<Vec<_>>();
        names.sort();
        assert_eq!(names, ["nonces.json", "nonces.json.lock"]);
        assert!(!names.iter().any(|name| name.starts_with(".tmp_")));
    }

    #[test]
    fn consume_gc_and_read_paths_follow_the_window_contract() {
        let (_temporary, store) = store();
        store
            .add("live".into(), "phone".into(), "".into(), false, 10)
            .expect("live");
        store
            .add("old".into(), "phone".into(), "".into(), false, -400)
            .expect("old");
        assert!(direct_pairing_window_open(&store, 11));
        assert!(store.consume("live", 11).expect("consume").is_some());
        assert!(store.peek("live").expect("used nonce").used);
        assert!(!direct_pairing_window_open(&store, 11));
        assert!(store.consume("live", 11).expect("repeat").is_none());
        assert!(
            store.peek("live").is_none(),
            "the next mutation collects a consumed nonce before lookup"
        );
        assert!(store.peek("old").is_none());
    }

    #[test]
    fn window_and_consume_treat_expires_at_as_the_closed_bound() {
        let (_temporary, store) = store();
        let first = store
            .add("bound-live".into(), "phone".into(), "".into(), false, 1_000)
            .expect("live");
        assert_eq!(first.expires_at, 1_000 + NONCE_TTL_SECONDS);
        assert!(direct_pairing_window_open(&store, first.expires_at - 1));
        assert!(!direct_pairing_window_open(&store, first.expires_at));
        assert!(
            store
                .consume("bound-live", first.expires_at - 1)
                .expect("consume inside the window")
                .is_some()
        );

        store
            .add(
                "bound-closed".into(),
                "phone".into(),
                "".into(),
                false,
                1_000,
            )
            .expect("closed");
        assert!(
            store
                .consume("bound-closed", 1_000 + NONCE_TTL_SECONDS)
                .expect("consume at the closed bound")
                .is_none()
        );
    }

    #[test]
    fn relay_window_predicate_requires_the_exact_live_relay_nonce() {
        let (_temporary, store) = store();
        store
            .add("direct".into(), "phone".into(), "".into(), false, 100)
            .expect("direct nonce");
        store
            .add_relay("relay-a".into(), "phone".into(), "".into(), 100)
            .expect("relay nonce");
        store
            .add_relay("relay-expired".into(), "phone".into(), "".into(), -201)
            .expect("expired relay nonce");

        assert!(direct_pairing_window_open(&store, 101));
        assert!(relay_pairing_nonce_open(&store, "relay-a", 101));
        assert!(!relay_pairing_nonce_open(&store, "direct", 101));
        assert!(!relay_pairing_nonce_open(&store, "relay-b", 101));
        assert!(!relay_pairing_nonce_open(&store, "relay-expired", 101));

        store.consume("relay-a", 101).expect("consume relay");
        assert!(!relay_pairing_nonce_open(&store, "relay-a", 101));
    }

    #[test]
    fn consume_reads_through_its_lock_and_window_maps_bad_states_to_closed() {
        let (temporary, store) = store();
        store
            .add("first".into(), "phone".into(), "".into(), false, 1)
            .expect("first");
        fs::write(
            store.path(),
            r#"[{"value":"external","device_label":"x","issued_at":1,"expires_at":301,"used":false,"role":"","same_machine":false}]"#,
        )
        .expect("external mutation");
        assert_eq!(
            store
                .consume("external", 2)
                .expect("consume")
                .unwrap()
                .value,
            "external"
        );
        for contents in ["not json", "{}"] {
            fs::write(store.path(), contents).expect("bad store");
            assert!(!direct_pairing_window_open(&store, 2));
        }
        fs::remove_file(store.path()).expect("remove store");
        assert!(!direct_pairing_window_open(&store, 2));
        assert!(!temporary.path().join("link").join("nonces.json").exists());
    }

    #[test]
    fn explicit_gc_is_the_only_read_independent_collection() {
        let (_temporary, store) = store();
        store
            .add("expired".into(), "phone".into(), "".into(), false, 0)
            .expect("expired");
        assert!(store.peek("expired").is_some());
        store.gc(300).expect("gc");
        assert!(store.peek("expired").is_none());
    }

    #[test]
    fn absent_or_unrecognized_kind_is_direct() {
        let (temporary, store) = store();
        fs::create_dir_all(temporary.path().join("link")).expect("link directory");
        fs::write(
            store.path(),
            r#"[
                {"value":"missing","device_label":"phone","issued_at":1,"expires_at":301,"used":false,"role":"observer","same_machine":false},
                {"value":"unknown","device_label":"phone","issued_at":1,"expires_at":301,"used":false,"role":"observer","same_machine":false,"kind":"future"},
                {"value":"malformed","device_label":"phone","issued_at":1,"expires_at":301,"used":false,"role":"observer","same_machine":false,"kind":true}
            ]"#,
        )
        .expect("write store");

        for value in ["missing", "unknown", "malformed"] {
            assert_eq!(store.peek(value).expect("nonce").kind, NonceKind::Direct);
        }
    }

    #[test]
    fn add_relay_writes_relay_v06_with_off_machine_authority() {
        let (_temporary, store) = store();
        let relay = store
            .add_relay("relay".into(), "phone".into(), "observer".into(), 10)
            .expect("relay nonce");
        assert_eq!(relay.kind, NonceKind::RelayV06);
        assert!(!relay.same_machine);
        assert_eq!(
            store.peek("relay").expect("stored relay").kind,
            NonceKind::RelayV06
        );
    }

    #[test]
    fn relay_nonce_debug_redacts_the_secret_value() {
        let (_temporary, store) = store();
        let value = "0123456789abcdef";
        let relay = store
            .add_relay(value.into(), "phone".into(), "observer".into(), 10)
            .expect("relay nonce");

        assert!(!format!("{relay:?}").contains(value));
    }

    #[test]
    fn relay_cleanup_removes_only_live_relay_windows() {
        let (_temporary, store) = store();
        store
            .add(
                "direct-same".into(),
                "phone".into(),
                "observer".into(),
                true,
                10,
            )
            .expect("same-machine direct");
        store
            .add(
                "direct-remote".into(),
                "phone".into(),
                "observer".into(),
                false,
                10,
            )
            .expect("remote direct");
        store
            .add_relay("relay-live".into(), "phone".into(), "observer".into(), 10)
            .expect("live relay");
        store
            .add_relay("relay-used".into(), "phone".into(), "observer".into(), 10)
            .expect("used relay");
        assert!(
            store
                .consume("relay-used", 11)
                .expect("consume relay")
                .is_some()
        );

        assert_eq!(store.cancel_all_relay_windows(12).expect("cleanup"), 1);
        assert!(store.peek("relay-live").is_none());
        assert!(store.peek("direct-same").is_some());
        assert!(store.peek("direct-remote").is_some());
        assert!(store.peek("relay-used").expect("used relay remains").used);
        assert_eq!(
            store.cancel_all_relay_windows(12).expect("repeat cleanup"),
            0,
            "a consumed relay is neither removed nor counted twice"
        );
    }

    #[test]
    fn cancel_removes_only_one_live_relay_window() {
        let (_temporary, store) = store();
        store
            .add(
                "direct".into(),
                "phone".into(),
                "observer".into(),
                false,
                10,
            )
            .expect("direct nonce");
        store
            .add_relay("relay-live".into(), "phone".into(), "observer".into(), 10)
            .expect("live relay");
        store
            .add_relay("relay-used".into(), "phone".into(), "observer".into(), 10)
            .expect("used relay");
        assert!(
            store
                .consume("relay-used", 11)
                .expect("consume relay")
                .is_some()
        );

        assert!(store.cancel("relay-live", 12).expect("cancel relay"));
        assert!(store.peek("relay-live").is_none());
        assert!(!store.cancel("relay-live", 12).expect("repeat cancel"));
        assert!(!store.cancel("relay-used", 12).expect("used relay remains"));
        assert!(!store.cancel("direct", 12).expect("direct remains"));
        assert!(store.peek("relay-used").expect("used relay").used);
        assert!(store.peek("direct").is_some());
    }
}
