// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! The security advisory check, on every desktop journal.
//!
//! Once a day the journal fetches sol pbc's signed security advisory and keeps
//! the last one it verified. Every quarter hour it compares its own version,
//! and each paired desktop solstone app's, with the floors in that advisory.
//! A version below its floor starts one owner notice, pushed once, and the
//! dashboard shows it for as long as it holds. An advisory that cannot be
//! confirmed flags nothing, and ends any notice it had started.
//!
//! The fetch is a bare GET of one public document. Nothing about this journal,
//! its owner or its devices is sent anywhere.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use solstone_core_callosum::CallosumOneShotSender;
use solstone_core_sol_link::client_description::current_display_label;
use solstone_core_sol_link::client_description_store::read_descriptions;
use solstone_core_sol_link::ledger::{
    AuthorizationLedger, AuthorizedClientsRead, read_authorized_clients,
};
use solstone_core_update::advisory::{self, Advisory};

/// The owner-notice family, which is also the push kind.
const FAMILY: &str = "security_update";
/// Where an owner learns more: the dashboard shows the banner and its links.
const NOTICE_OPENS: &str = "/app/home/";
/// The desktop OSes this check runs on. A journal on a phone does not fetch.
const DESKTOP: &[&str] = &["linux", "macos", "windows"];
/// How long a verified fetch stands before the next one.
const FETCH_EVERY: Duration = Duration::from_secs(24 * 60 * 60);
/// How often the floors are compared, and how soon a failed fetch is retried.
const TICK: Duration = Duration::from_secs(15 * 60);

/// What the status route reads. `None` until the first comparison.
static REPORT: Mutex<Option<Report>> = Mutex::new(None);

/// What the journal knows about security updates right now.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub(crate) struct Report {
    /// `confirmed` when a current signed advisory was read, else `unconfirmed`.
    state: &'static str,
    flags: Vec<Flag>,
}

/// One install below its floor.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub(crate) struct Flag {
    /// `journal` for this journal, `device` for a paired app.
    subject: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    device: Option<String>,
    version: String,
    fixed_in: String,
    notes: String,
    #[serde(skip)]
    key: String,
}

/// A paired device as the journal last heard it describe itself.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct Device {
    pub(crate) cid: String,
    pub(crate) label: String,
    pub(crate) app_id: String,
    pub(crate) app_version: String,
}

/// The advisory as fetched, kept so a restart or an offline day still knows
/// the floors until the advisory expires.
#[derive(Deserialize, Serialize)]
struct Stored {
    document: String,
    signature: String,
}

fn stored_path(journal: &Path) -> PathBuf {
    journal.join("health/security-advisory.json")
}

/// Start the check once per process, on desktop OSes only.
pub(crate) fn start(journal: PathBuf) {
    if !DESKTOP.contains(&std::env::consts::OS) {
        return;
    }
    static STARTED: std::sync::Once = std::sync::Once::new();
    STARTED.call_once(|| {
        let spawned = std::thread::Builder::new()
            .name("security-advisory".to_owned())
            .spawn(move || run(&journal));
        if let Err(error) = spawned {
            log::warn!("security advisory check did not start: {error}");
        }
    });
}

/// The status route's `security` field, on a desktop journal.
pub(crate) fn status() -> Option<Value> {
    if !DESKTOP.contains(&std::env::consts::OS) {
        return None;
    }
    let report = REPORT.lock().unwrap_or_else(|p| p.into_inner()).clone();
    Some(report.map_or_else(
        || json!({"state": "checking", "flags": []}),
        |report| json!(report),
    ))
}

fn run(journal: &Path) {
    let key = match advisory::platform_key() {
        Ok(key) => Some(key),
        Err(_) => {
            log::error!("security advisory key in this build is unreadable");
            None
        }
    };
    let mut fetched_at: Option<SystemTime> = None;
    loop {
        let now = SystemTime::now();
        let now_unix = now.duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs());
        let current = key.as_ref().and_then(|key| {
            if fetch_due(fetched_at, now) && refresh(journal, key, now_unix) {
                fetched_at = Some(now);
            }
            current(journal, key, now_unix)
        });
        let report = compare(
            current.as_ref(),
            std::env::consts::OS,
            env!("CARGO_PKG_VERSION"),
            &paired_devices(journal),
        );
        reconcile(journal, &report, &send);
        *REPORT.lock().unwrap_or_else(|p| p.into_inner()) = Some(report);
        std::thread::sleep(TICK);
    }
}

/// A clock set back past the last fetch counts as due.
fn fetch_due(fetched_at: Option<SystemTime>, now: SystemTime) -> bool {
    fetched_at.is_none_or(|at| {
        now.duration_since(at)
            .map_or(true, |age| age >= FETCH_EVERY)
    })
}

/// Fetch, verify and keep a newer advisory. Returns whether one verified.
fn refresh(journal: &Path, key: &advisory::PublicKey, now_unix: u64) -> bool {
    let accepted = read_stored(journal).and_then(|stored| {
        serde_json::from_str::<Value>(&stored.document)
            .ok()?
            .get("sequence")?
            .as_u64()
    });
    let Ok((document, signature)) = advisory::fetch() else {
        return false;
    };
    if advisory::verify(&document, &signature, key, now_unix, accepted).is_err() {
        return false;
    }
    let (Ok(document), Ok(signature)) = (String::from_utf8(document), String::from_utf8(signature))
    else {
        return false;
    };
    write_stored(
        journal,
        &Stored {
            document,
            signature,
        },
    )
}

/// The kept advisory, while it is still current.
fn current(journal: &Path, key: &advisory::PublicKey, now_unix: u64) -> Option<Advisory> {
    let stored = read_stored(journal)?;
    advisory::verify(
        stored.document.as_bytes(),
        stored.signature.as_bytes(),
        key,
        now_unix,
        None,
    )
    .ok()
}

fn read_stored(journal: &Path) -> Option<Stored> {
    std::fs::read(stored_path(journal))
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
}

fn write_stored(journal: &Path, stored: &Stored) -> bool {
    let _ = std::fs::create_dir_all(journal.join("health"));
    solstone_core_journal_io::write_json(
        stored_path(journal),
        stored,
        solstone_core_journal_io::JsonWriteOptions {
            mode: Some(0o600),
            ..solstone_core_journal_io::JsonWriteOptions::default()
        },
    )
    .is_ok()
}

/// The paired devices that have described their app to this journal.
fn paired_devices(journal: &Path) -> Vec<Device> {
    let ledger = AuthorizationLedger::new(journal);
    let AuthorizedClientsRead::Present(clients) =
        read_authorized_clients(ledger.authorized_clients_path())
    else {
        return Vec::new();
    };
    let descriptions = read_descriptions(journal).unwrap_or_default();
    clients
        .iter()
        .filter_map(|entry| {
            let stored = descriptions.get(&entry.fingerprint)?;
            let reported = stored.reported.as_ref()?;
            Some(Device {
                cid: entry.fingerprint.clone(),
                label: current_display_label(entry, Some(stored)),
                app_id: reported.app_id.clone()?,
                app_version: reported.app_version.clone()?,
            })
        })
        .collect()
}

/// Compare this journal and its paired devices with `advisory`.
pub(crate) fn compare(
    advisory: Option<&Advisory>,
    os: &str,
    journal_version: &str,
    devices: &[Device],
) -> Report {
    let Some(advisory) = advisory else {
        return Report {
            state: "unconfirmed",
            flags: Vec::new(),
        };
    };
    let mut flags = Vec::new();
    if let Some(floor) = advisory.journal.get(os)
        && advisory::is_below(journal_version, &floor.fixed_in)
    {
        flags.push(Flag {
            subject: "journal",
            device: None,
            version: journal_version.to_owned(),
            fixed_in: floor.fixed_in.clone(),
            notes: floor.notes.clone(),
            key: format!("{FAMILY}:journal:{journal_version}"),
        });
    }
    for device in devices {
        let Some(floor) = advisory.clients.get(&device.app_id) else {
            continue;
        };
        let Some(version) = advisory::version_of(&device.app_version) else {
            continue;
        };
        if advisory::is_below(version, &floor.fixed_in) {
            flags.push(Flag {
                subject: "device",
                device: Some(device.label.clone()),
                version: version.to_owned(),
                fixed_in: floor.fixed_in.clone(),
                notes: floor.notes.clone(),
                key: format!("{FAMILY}:device:{}:{version}", device.cid),
            });
        }
    }
    Report {
        state: "confirmed",
        flags,
    }
}

/// Start a notice for each flag not already announced, and end, without a
/// word, each one whose flag no longer holds.
fn reconcile(journal: &Path, report: &Report, send: &dyn Fn(&Path, &Value)) {
    let active: BTreeSet<String> = solstone_core_push::owner_notice::active_keys(journal, FAMILY)
        .into_iter()
        .collect();
    let wanted: BTreeSet<&str> = report.flags.iter().map(|flag| flag.key.as_str()).collect();
    for flag in &report.flags {
        if !active.contains(&flag.key) {
            send(journal, &raise(flag));
        }
    }
    for key in active.iter().filter(|key| !wanted.contains(key.as_str())) {
        send(
            journal,
            &json!({"tract": "owner_notice", "event": "clear", "key": key}),
        );
    }
}

fn raise(flag: &Flag) -> Value {
    let (title, message) = match flag.device.as_deref().map(notice_name) {
        None => (
            "a security update is ready for your journal".to_owned(),
            format!(
                "your journal is missing a security fix. update it to {} or later.",
                flag.fixed_in
            ),
        ),
        Some(name) => (
            match name {
                Some(name) => {
                    format!("a security update is ready for the solstone app on \"{name}\"")
                }
                None => "a security update is ready for the solstone app on one of your devices"
                    .to_owned(),
            },
            format!(
                "the solstone app there is missing a security fix. update it to {} or later.",
                flag.fixed_in
            ),
        ),
    };
    json!({
        "tract": "owner_notice",
        "event": "raise",
        "key": flag.key,
        "title": title,
        "message": message,
        "action": NOTICE_OPENS,
    })
}

/// A device names itself, so a name too long or odd to read as one is left
/// out of the push.
fn notice_name(label: &str) -> Option<&str> {
    let label = label.trim();
    (!label.is_empty() && label.chars().count() <= 48 && !label.chars().any(char::is_control))
        .then_some(label)
}

fn send(journal: &Path, row: &Value) {
    let sender =
        CallosumOneShotSender::new(journal.join("health/callosum.sock"), Duration::from_secs(1));
    if sender.send_line(&format!("{row}\n")).is_err() {
        log::debug!("security advisory owner notice unavailable");
    }
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::collections::BTreeMap;

    use solstone_core_update::advisory::Floor;
    use time::OffsetDateTime;

    use super::*;

    fn advisory() -> Advisory {
        let floor = |fixed_in: &str| Floor {
            fixed_in: fixed_in.to_owned(),
            notes: format!("https://solstone.app/notes/{fixed_in}"),
        };
        Advisory {
            schema_version: 1,
            sequence: 3,
            issued_unix: 1,
            expires_unix: 2,
            journal: BTreeMap::from([
                ("linux".to_owned(), floor("2.0.66")),
                ("macos".to_owned(), floor("2.0.66")),
                ("windows".to_owned(), floor("2.0.71")),
            ]),
            clients: BTreeMap::from([
                ("app.solstone.observer".to_owned(), floor("2.0.64")),
                ("solstone-linux".to_owned(), floor("2.0.38")),
                ("app.solstone.windows".to_owned(), floor("2.0.45")),
            ]),
        }
    }

    fn device(cid: &str, label: &str, app_id: &str, app_version: &str) -> Device {
        Device {
            cid: cid.to_owned(),
            label: label.to_owned(),
            app_id: app_id.to_owned(),
            app_version: app_version.to_owned(),
        }
    }

    #[test]
    fn each_desktop_journal_compares_with_its_own_os_floor() {
        let advisory = advisory();
        for (os, version, flagged) in [
            ("linux", "2.0.65", true),
            ("linux", "2.0.66", false),
            ("macos", "2.0.65", true),
            ("windows", "2.0.70", true),
            ("windows", "2.0.71", false),
            ("freebsd", "1.0.0", false),
        ] {
            let report = compare(Some(&advisory), os, version, &[]);
            assert_eq!(report.state, "confirmed");
            assert_eq!(report.flags.len(), usize::from(flagged), "{os} {version}");
            if flagged {
                assert_eq!(report.flags[0].subject, "journal");
                assert_eq!(
                    report.flags[0].key,
                    format!("security_update:journal:{version}")
                );
            }
        }
    }

    #[test]
    fn a_paired_desktop_app_below_its_floor_is_named_and_others_are_not() {
        let devices = [
            device(
                "sha256:a",
                "studio mac",
                "app.solstone.observer",
                "2.0.63 (144)",
            ),
            device(
                "sha256:b",
                "laptop",
                "app.solstone.observer",
                "2.0.64 (145)",
            ),
            device("sha256:c", "desk", "app.solstone.windows", "2.0.44"),
            device("sha256:d", "phone", "app.solstone.swift", "2.0.1 (100)"),
            device("sha256:e", "box", "solstone-linux", "unknown"),
        ];
        let report = compare(Some(&advisory()), "linux", "2.0.74", &devices);
        let named: Vec<_> = report
            .flags
            .iter()
            .map(|flag| {
                (
                    flag.device.as_deref(),
                    flag.version.as_str(),
                    flag.key.as_str(),
                )
            })
            .collect();
        assert_eq!(
            named,
            [
                (
                    Some("studio mac"),
                    "2.0.63",
                    "security_update:device:sha256:a:2.0.63"
                ),
                (
                    Some("desk"),
                    "2.0.44",
                    "security_update:device:sha256:c:2.0.44"
                ),
            ]
        );
        let body = json!(report);
        assert_eq!(body["flags"][0]["device"], "studio mac");
        assert_eq!(
            body["flags"][0]["notes"],
            "https://solstone.app/notes/2.0.64"
        );
        assert!(body["flags"][0].get("key").is_none());
    }

    #[test]
    fn an_unconfirmed_advisory_flags_nothing() {
        let devices = [device("sha256:a", "mac", "app.solstone.observer", "1.0.0")];
        let report = compare(None, "linux", "0.0.1", &devices);
        assert_eq!(report.state, "unconfirmed");
        assert!(report.flags.is_empty());
    }

    /// Records each announcement and applies it the way the journal's
    /// owner-notice listener does, so episodes persist as they would.
    fn applying(journal: &Path, sent: &RefCell<Vec<Value>>) -> impl Fn(&Path, &Value) {
        move |_: &Path, row: &Value| {
            sent.borrow_mut().push(row.clone());
            let envelope: solstone_core_callosum::CallosumEnvelope =
                serde_json::from_value(row.clone()).unwrap();
            crate::owner_notices::handle(journal, "", &envelope, OffsetDateTime::UNIX_EPOCH);
        }
    }

    #[test]
    fn a_flag_notifies_once_and_ends_when_the_install_is_no_longer_below() {
        let dir = tempfile::tempdir().unwrap();
        let journal = dir.path();
        let sent = RefCell::new(Vec::new());
        let apply = applying(journal, &sent);
        let advisory = advisory();
        let devices = [device(
            "sha256:a",
            "studio mac",
            "app.solstone.observer",
            "2.0.60",
        )];

        let flagged = compare(Some(&advisory), "macos", "2.0.65", &devices);
        reconcile(journal, &flagged, &apply);
        reconcile(journal, &flagged, &apply);
        let raised: Vec<_> = sent
            .borrow()
            .iter()
            .map(|row| row["event"].clone())
            .collect();
        assert_eq!(raised, ["raise", "raise"]);
        assert_eq!(
            sent.borrow()[0]["title"],
            "a security update is ready for your journal"
        );
        assert_eq!(
            sent.borrow()[1]["title"],
            "a security update is ready for the solstone app on \"studio mac\""
        );
        assert_eq!(sent.borrow()[1]["action"], "/app/home/");

        // The journal updated; the mac did not.
        let updated = compare(Some(&advisory), "macos", "2.0.66", &devices);
        reconcile(journal, &updated, &apply);
        assert_eq!(sent.borrow().len(), 3);
        assert_eq!(sent.borrow()[2]["event"], "clear");
        assert_eq!(sent.borrow()[2]["key"], "security_update:journal:2.0.65");
        assert_eq!(
            solstone_core_push::owner_notice::active_keys(journal, FAMILY),
            ["security_update:device:sha256:a:2.0.60"]
        );

        // An advisory that can no longer be confirmed ends what is left.
        reconcile(journal, &compare(None, "macos", "2.0.66", &devices), &apply);
        assert_eq!(sent.borrow().len(), 4);
        assert!(solstone_core_push::owner_notice::active_keys(journal, FAMILY).is_empty());
    }

    #[test]
    fn a_long_or_odd_device_name_stays_out_of_the_push() {
        let flag = |label: &str| Flag {
            subject: "device",
            device: Some(label.to_owned()),
            version: "2.0.1".to_owned(),
            fixed_in: "2.0.2".to_owned(),
            notes: "https://solstone.app/n".to_owned(),
            key: "security_update:device:x:2.0.1".to_owned(),
        };
        assert_eq!(
            raise(&flag(&"x".repeat(60)))["title"],
            "a security update is ready for the solstone app on one of your devices"
        );
        assert_eq!(
            raise(&flag("a\u{7}b"))["title"],
            "a security update is ready for the solstone app on one of your devices"
        );
    }

    #[test]
    fn the_fetch_is_due_daily_and_on_a_clock_set_back() {
        let at = UNIX_EPOCH + Duration::from_secs(1_000_000);
        assert!(fetch_due(None, at));
        assert!(!fetch_due(
            Some(at),
            at + FETCH_EVERY - Duration::from_secs(1)
        ));
        assert!(fetch_due(Some(at), at + FETCH_EVERY));
        assert!(fetch_due(Some(at), at - Duration::from_secs(1)));
    }
}
