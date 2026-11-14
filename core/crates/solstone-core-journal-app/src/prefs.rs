// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! What the app remembers between runs: its update settings and the mark it
//! last drew as its icon. It lives beside the journal's own per-user state,
//! outside the install root, so an update or reinstall leaves it alone.

use std::path::{Path, PathBuf};
use std::sync::Mutex;

use serde::{Deserialize, Serialize};
use serde_json::Value;

pub const POLICY_DAILY_BACKGROUND: &str = "daily-background";

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum CheckInterval {
    #[default]
    Day,
    Week,
    Month,
}

impl CheckInterval {
    pub fn seconds(self) -> u64 {
        match self {
            Self::Day => 86_400,
            Self::Week => 604_800,
            Self::Month => 2_592_000,
        }
    }
}

fn default_on() -> bool {
    true
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(default)]
pub struct OwnerChoices {
    pub check: bool,
    pub interval: bool,
    pub download: bool,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(default)]
pub struct Prefs {
    #[serde(default = "default_on")]
    pub auto_check: bool,
    #[serde(default = "default_on")]
    pub auto_download: bool,
    pub interval: CheckInterval,
    #[serde(default)]
    pub policy: Option<String>,
    pub last_checked_at: Option<u64>,
    /// The mark the app's icon was last drawn from, so a launch can put the
    /// icon back without drawing it again.
    pub icon_mark: Option<Value>,
    #[serde(default)]
    pub owner_choices: OwnerChoices,
}

impl Default for Prefs {
    fn default() -> Self {
        Self {
            auto_check: true,
            auto_download: true,
            interval: CheckInterval::Day,
            policy: Some(POLICY_DAILY_BACKGROUND.to_owned()),
            last_checked_at: None,
            icon_mark: None,
            owner_choices: OwnerChoices::default(),
        }
    }
}

thread_local! {
    static TEST_STATE_DIR: std::cell::RefCell<Option<PathBuf>> = const { std::cell::RefCell::new(None) };
}

#[cfg(all(test, feature = "full-tests"))]
pub fn with_test_state_dir<T>(dir: &Path, f: impl FnOnce() -> T) -> T {
    TEST_STATE_DIR.with(|cell| {
        let previous = cell.borrow_mut().replace(dir.to_path_buf());
        let result = f();
        *cell.borrow_mut() = previous;
        result
    })
}

/// `%LOCALAPPDATA%\solstone-journal`, the journal's per-user state directory.
pub fn state_dir() -> PathBuf {
    if let Some(dir) = TEST_STATE_DIR.with(|cell| cell.borrow().clone()) {
        return dir;
    }
    std::env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir)
        .join("solstone-journal")
}

pub fn prefs_path() -> PathBuf {
    state_dir().join("journal-app.json")
}

/// The pending update record: exactly `{"file":"<plain asset filename>"}`.
pub fn pending_update_path() -> PathBuf {
    state_dir().join("pending-update.json")
}

/// The icon file drawn from the journal's mark.
pub fn mark_icon_path() -> PathBuf {
    state_dir().join("journal-mark.ico")
}

/// Where the window's web view keeps its own data.
pub fn webview_dir() -> PathBuf {
    state_dir().join("journal-app-webview")
}

static PREFS_LOCK: Mutex<()> = Mutex::new(());

#[derive(Debug)]
enum DiskState {
    Missing,
    Unreadable,
    Loaded(Prefs),
}

fn read_disk(path: &Path) -> DiskState {
    match std::fs::read(path) {
        Ok(bytes) => match serde_json::from_slice::<Prefs>(&bytes) {
            Ok(prefs) => DiskState::Loaded(prefs),
            Err(_) => DiskState::Unreadable,
        },
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => DiskState::Missing,
        Err(_) => DiskState::Unreadable,
    }
}

fn write_file(path: &Path, prefs: &Prefs) -> bool {
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if let Ok(bytes) = serde_json::to_vec_pretty(prefs) {
        let temporary = path.with_extension("json.tmp");
        if std::fs::write(&temporary, bytes).is_ok() {
            return std::fs::rename(&temporary, path).is_ok();
        }
    }
    false
}

pub fn compute_owner_choices(
    prior: Option<&Prefs>,
    auto_check: bool,
    auto_download: bool,
    interval: CheckInterval,
) -> OwnerChoices {
    let baseline = Prefs::default();
    let (prior_choices, base_check, base_download, base_interval) = match prior {
        Some(p) => (p.owner_choices, p.auto_check, p.auto_download, p.interval),
        None => (
            OwnerChoices::default(),
            baseline.auto_check,
            baseline.auto_download,
            baseline.interval,
        ),
    };
    OwnerChoices {
        check: prior_choices.check || auto_check != base_check,
        download: prior_choices.download || auto_download != base_download,
        interval: prior_choices.interval || interval != base_interval,
    }
}

/// Initialize preferences at app startup:
/// - Missing file: writes `Prefs::default()`.
/// - Unreadable bytes: returns `Prefs::default()` in memory without writing.
/// - Unmarked valid JSON: migrates to daily background policy, saves, and preserves `icon_mark` / `last_checked_at`.
/// - Marked JSON (`policy == daily-background`): loads as stored.
pub fn initialize() -> Prefs {
    let _guard = PREFS_LOCK.lock().expect("prefs lock");
    let path = prefs_path();
    match read_disk(&path) {
        DiskState::Missing => {
            let default_prefs = Prefs::default();
            write_file(&path, &default_prefs);
            default_prefs
        }
        DiskState::Unreadable => Prefs::default(),
        DiskState::Loaded(record) => {
            if record.policy.as_deref() != Some(POLICY_DAILY_BACKGROUND) {
                let migrated = Prefs {
                    auto_check: true,
                    auto_download: true,
                    interval: CheckInterval::Day,
                    policy: Some(POLICY_DAILY_BACKGROUND.to_owned()),
                    last_checked_at: record.last_checked_at,
                    icon_mark: record.icon_mark,
                    owner_choices: OwnerChoices::default(),
                };
                write_file(&path, &migrated);
                migrated
            } else {
                record
            }
        }
    }
}

pub fn load() -> Prefs {
    let _guard = PREFS_LOCK.lock().expect("prefs lock");
    match read_disk(&prefs_path()) {
        DiskState::Loaded(record) => record,
        _ => Prefs::default(),
    }
}

pub fn load_if_readable() -> Option<Prefs> {
    let _guard = PREFS_LOCK.lock().expect("prefs lock");
    match read_disk(&prefs_path()) {
        DiskState::Loaded(record) => Some(record),
        _ => None,
    }
}

pub fn update_last_checked_at(checked_at: u64) {
    let _guard = PREFS_LOCK.lock().expect("prefs lock");
    let path = prefs_path();
    match read_disk(&path) {
        DiskState::Loaded(mut record) => {
            record.last_checked_at = Some(checked_at);
            write_file(&path, &record);
        }
        DiskState::Missing => {
            let record = Prefs {
                last_checked_at: Some(checked_at),
                ..Default::default()
            };
            write_file(&path, &record);
        }
        DiskState::Unreadable => {}
    }
}

pub fn update_settings(auto_check: bool, auto_download: bool, interval: CheckInterval) -> Prefs {
    let _guard = PREFS_LOCK.lock().expect("prefs lock");
    let path = prefs_path();
    match read_disk(&path) {
        DiskState::Loaded(mut record) => {
            record.owner_choices =
                compute_owner_choices(Some(&record), auto_check, auto_download, interval);
            record.auto_check = auto_check;
            record.auto_download = auto_download;
            record.interval = interval;
            record.policy = Some(POLICY_DAILY_BACKGROUND.to_owned());
            write_file(&path, &record);
            record
        }
        DiskState::Missing => {
            let owner_choices = compute_owner_choices(None, auto_check, auto_download, interval);
            let record = Prefs {
                auto_check,
                auto_download,
                interval,
                policy: Some(POLICY_DAILY_BACKGROUND.to_owned()),
                last_checked_at: None,
                icon_mark: None,
                owner_choices,
            };
            write_file(&path, &record);
            record
        }
        DiskState::Unreadable => {
            let owner_choices = compute_owner_choices(None, auto_check, auto_download, interval);
            Prefs {
                auto_check,
                auto_download,
                interval,
                policy: Some(POLICY_DAILY_BACKGROUND.to_owned()),
                last_checked_at: None,
                icon_mark: None,
                owner_choices,
            }
        }
    }
}

pub fn update_icon_mark(icon_mark: Option<Value>) {
    let _guard = PREFS_LOCK.lock().expect("prefs lock");
    let path = prefs_path();
    match read_disk(&path) {
        DiskState::Loaded(mut record) => {
            record.icon_mark = icon_mark;
            write_file(&path, &record);
        }
        DiskState::Missing => {
            let record = Prefs {
                icon_mark,
                ..Default::default()
            };
            write_file(&path, &record);
        }
        DiskState::Unreadable => {}
    }
}

/// Fallback parser for the `"prefs"` UI message arguments.
/// Default: auto_check true, auto_download true, interval Day.
pub fn parse_prefs_args(args: &Value) -> (bool, bool, CheckInterval) {
    let auto_check = args
        .get("auto_check")
        .and_then(Value::as_bool)
        .unwrap_or(true);
    let auto_download = args
        .get("auto_download")
        .and_then(Value::as_bool)
        .unwrap_or(true);
    let interval = match args.get("interval").and_then(Value::as_str) {
        Some("month") => CheckInterval::Month,
        Some("week") => CheckInterval::Week,
        _ => CheckInterval::Day,
    };
    (auto_check, auto_download, interval)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn routine_hand_authored_json_deserialization_and_migration() {
        // Hand-authored old JSON: includes icon_mark and last_checked_at, interval: week, auto_check: false, no policy
        let old_json = r#"{
            "auto_check": false,
            "auto_download": false,
            "interval": "week",
            "last_checked_at": 1700000000,
            "icon_mark": {"color": "blue"}
        }"#;
        let parsed: Prefs = serde_json::from_str(old_json).expect("parse old JSON");
        assert_eq!(parsed.policy, None);
        assert!(!parsed.auto_check);
        assert!(!parsed.auto_download);
        assert_eq!(parsed.interval, CheckInterval::Week);
        assert_eq!(parsed.last_checked_at, Some(1700000000));
        assert!(parsed.icon_mark.is_some());
        assert_eq!(parsed.owner_choices, OwnerChoices::default());

        // Hand-authored marked JSON: includes policy: daily-background and owner choices that migration would have overridden
        let marked_json = r#"{
            "auto_check": false,
            "auto_download": false,
            "interval": "week",
            "policy": "daily-background",
            "last_checked_at": 1700000000,
            "icon_mark": {"color": "blue"}
        }"#;
        let parsed_marked: Prefs = serde_json::from_str(marked_json).expect("parse marked JSON");
        assert_eq!(
            parsed_marked.policy.as_deref(),
            Some(POLICY_DAILY_BACKGROUND)
        );
        assert!(!parsed_marked.auto_check);
        assert!(!parsed_marked.auto_download);
        assert_eq!(parsed_marked.interval, CheckInterval::Week);
        assert_eq!(parsed_marked.owner_choices, OwnerChoices::default());

        // Marked object with an explicit owner_choices of one true bit loads as stored
        let marked_with_choices_json = r#"{
            "auto_check": true,
            "auto_download": false,
            "interval": "day",
            "policy": "daily-background",
            "owner_choices": {
                "check": false,
                "interval": false,
                "download": true
            }
        }"#;
        let parsed_choices: Prefs =
            serde_json::from_str(marked_with_choices_json).expect("parse choices JSON");
        assert_eq!(
            parsed_choices.owner_choices,
            OwnerChoices {
                check: false,
                interval: false,
                download: true,
            }
        );

        // compute_owner_choices routine check
        let choices_diff = compute_owner_choices(None, true, false, CheckInterval::Day);
        assert_eq!(
            choices_diff,
            OwnerChoices {
                check: false,
                interval: false,
                download: true,
            }
        );
        let prior = Prefs {
            auto_download: false,
            owner_choices: OwnerChoices {
                check: false,
                interval: false,
                download: true,
            },
            ..Default::default()
        };
        let choices_sticky = compute_owner_choices(Some(&prior), true, true, CheckInterval::Day);
        assert!(choices_sticky.download);

        // Missing auto_check, auto_download, interval in JSON defaults to true and Day
        let minimal_json = r#"{}"#;
        let parsed_minimal: Prefs = serde_json::from_str(minimal_json).expect("parse minimal JSON");
        assert!(parsed_minimal.auto_check);
        assert!(parsed_minimal.auto_download);
        assert_eq!(parsed_minimal.interval, CheckInterval::Day);
        assert_eq!(parsed_minimal.policy, None);
        assert_eq!(parsed_minimal.owner_choices, OwnerChoices::default());
    }

    #[test]
    fn parse_prefs_args_defaults() {
        let (check, download, interval) = parse_prefs_args(&serde_json::json!({}));
        assert!(check);
        assert!(download);
        assert_eq!(interval, CheckInterval::Day);

        let (check, download, interval) = parse_prefs_args(&serde_json::json!({
            "auto_check": false,
            "auto_download": false,
            "interval": "week"
        }));
        assert!(!check);
        assert!(!download);
        assert_eq!(interval, CheckInterval::Week);
    }

    #[cfg(all(test, feature = "full-tests"))]
    #[test]
    fn full_tests_prefs_migration_and_isolation() {
        let temp = tempfile::tempdir().expect("tempdir");
        with_test_state_dir(temp.path(), || {
            // 1. Missing file -> initialize writes default with all three bits false
            let initial = initialize();
            assert_eq!(initial.interval, CheckInterval::Day);
            assert!(initial.auto_check);
            assert!(initial.auto_download);
            assert_eq!(initial.policy.as_deref(), Some(POLICY_DAILY_BACKGROUND));
            assert_eq!(initial.owner_choices, OwnerChoices::default());

            let read_bytes = std::fs::read(prefs_path()).expect("read prefs");
            let on_disk: Prefs = serde_json::from_slice(&read_bytes).expect("parse");
            assert_eq!(on_disk, initial);

            // 2. Unmarked old object (weekly or auto-check off, with icon_mark and last_checked_at, no policy).
            // After initialize in the full-tests dir: daily, both downloads on, policy set, icon and timestamp kept,
            // all three bits false on disk.
            let unmarked_old = r#"{
                "auto_check": false,
                "auto_download": false,
                "interval": "week",
                "last_checked_at": 1700000000,
                "icon_mark": {"color": "blue"},
                "owner_choices": {
                    "check": true,
                    "interval": true,
                    "download": true
                }
            }"#;
            std::fs::write(prefs_path(), unmarked_old).expect("write unmarked old");
            let migrated = initialize();
            assert_eq!(migrated.interval, CheckInterval::Day);
            assert!(migrated.auto_check);
            assert!(migrated.auto_download);
            assert_eq!(migrated.policy.as_deref(), Some(POLICY_DAILY_BACKGROUND));
            assert_eq!(migrated.last_checked_at, Some(1700000000));
            assert_eq!(
                migrated.icon_mark,
                Some(serde_json::json!({"color": "blue"}))
            );
            assert_eq!(migrated.owner_choices, OwnerChoices::default());

            let on_disk_migrated: Prefs =
                serde_json::from_slice(&std::fs::read(prefs_path()).unwrap()).unwrap();
            assert_eq!(on_disk_migrated.interval, CheckInterval::Day);
            assert!(on_disk_migrated.auto_check);
            assert!(on_disk_migrated.auto_download);
            assert_eq!(
                on_disk_migrated.policy.as_deref(),
                Some(POLICY_DAILY_BACKGROUND)
            );
            assert_eq!(on_disk_migrated.last_checked_at, Some(1700000000));
            assert_eq!(
                on_disk_migrated.icon_mark,
                Some(serde_json::json!({"color": "blue"}))
            );
            assert_eq!(on_disk_migrated.owner_choices, OwnerChoices::default());

            // 3. Marked object with an explicit owner_choices of one true bit loads as stored.
            let marked_one_bit = r#"{
                "auto_check": true,
                "auto_download": false,
                "interval": "day",
                "policy": "daily-background",
                "last_checked_at": 1700000000,
                "icon_mark": {"color": "blue"},
                "owner_choices": {
                    "check": false,
                    "interval": false,
                    "download": true
                }
            }"#;
            std::fs::write(prefs_path(), marked_one_bit).expect("write marked one bit");
            let loaded = initialize();
            assert_eq!(
                loaded.owner_choices,
                OwnerChoices {
                    check: false,
                    interval: false,
                    download: true,
                }
            );
            assert_eq!(
                load().owner_choices,
                OwnerChoices {
                    check: false,
                    interval: false,
                    download: true,
                }
            );

            // 4. update_settings that changes only auto_download sets only download.
            // A following icon write and last-checked write keep that bit and do not set the others.
            let fresh_marked = r#"{
                "auto_check": true,
                "auto_download": true,
                "interval": "day",
                "policy": "daily-background",
                "owner_choices": {
                    "check": false,
                    "interval": false,
                    "download": false
                }
            }"#;
            std::fs::write(prefs_path(), fresh_marked).expect("write fresh marked");
            let after_change = update_settings(true, false, CheckInterval::Day);
            assert_eq!(
                after_change.owner_choices,
                OwnerChoices {
                    check: false,
                    interval: false,
                    download: true,
                }
            );
            update_icon_mark(Some(serde_json::json!({"mark": 42})));
            assert_eq!(
                load().owner_choices,
                OwnerChoices {
                    check: false,
                    interval: false,
                    download: true,
                }
            );
            update_last_checked_at(123456);
            assert_eq!(
                load().owner_choices,
                OwnerChoices {
                    check: false,
                    interval: false,
                    download: true,
                }
            );
            let on_disk_final: Prefs =
                serde_json::from_slice(&std::fs::read(prefs_path()).unwrap()).unwrap();
            assert_eq!(
                on_disk_final.owner_choices,
                OwnerChoices {
                    check: false,
                    interval: false,
                    download: true,
                }
            );

            // 5. Unreadable file bytes stay exactly as written.
            std::fs::write(prefs_path(), b"INVALID JSON").expect("corrupt file");
            update_last_checked_at(999999);
            update_settings(true, true, CheckInterval::Day);
            update_icon_mark(None);
            assert_eq!(std::fs::read(prefs_path()).unwrap(), b"INVALID JSON");
        });
    }
}
