// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::path::PathBuf;

use axum::Json;
use axum::response::{IntoResponse, Response};
use jiff::Timestamp;
use serde_json::{Value, json};
use solstone_core_home::HomeContext;
use solstone_core_home::readers::get_capture_health;

/// What this journal understands from a linked device, beyond the ingest
/// protocol itself. A device sends a kind of content only to a journal that
/// lists it here.
const CAPABILITIES: &[&str] = &[
    "calendar_notes",
    "folder",
    "obsidian_notes",
    "resumable_import_uploads",
    "screenshots",
    "shared_sources",
];

/// What the journal's own update check last learned. The status route reads
/// only this, so no page or device reading it causes a request to the update
/// origin.
#[cfg(target_os = "linux")]
static UPDATE_LISTING: solstone_core_update::StoredCatalogue =
    solstone_core_update::StoredCatalogue::new();

/// How long a verified answer stands.
#[cfg(target_os = "linux")]
const UPDATE_CHECK_EVERY: std::time::Duration = std::time::Duration::from_secs(24 * 60 * 60);

/// How often to look at the clock, which is also how soon a failed check is
/// retried. Sleep alone would stretch a day on a machine that suspends.
#[cfg(target_os = "linux")]
const UPDATE_CHECK_TICK: std::time::Duration = std::time::Duration::from_secs(15 * 60);

/// Start the update check, once per process, on a schedule of its own: its
/// requests follow this process and the clock, never whether a page is open.
/// The same tick tells the owner when automatic updates have stopped.
pub(crate) fn start_update_check(journal: PathBuf) {
    #[cfg(not(target_os = "linux"))]
    let _ = journal;
    #[cfg(target_os = "linux")]
    {
        static STARTED: std::sync::Once = std::sync::Once::new();
        STARTED.call_once(|| {
            let spawned = std::thread::Builder::new()
                .name("update-check".to_owned())
                .spawn(move || run_update_check(&journal));
            if let Err(error) = spawned {
                log::warn!("update check did not start: {error}");
            }
        });
    }
}

#[cfg(target_os = "linux")]
fn run_update_check(journal: &std::path::Path) {
    let mut verified_at = None;
    loop {
        if update_check_due(verified_at, std::time::SystemTime::now()) && refresh_update_listing() {
            verified_at = Some(std::time::SystemTime::now());
        }
        #[cfg(feature = "host")]
        if let Some(data_dir) = solstone_core_update::owner_data_dir() {
            reconcile_paused_notice(journal, &data_dir, &send_owner_notice);
        }
        #[cfg(not(feature = "host"))]
        let _ = journal;
        std::thread::sleep(UPDATE_CHECK_TICK);
    }
}

/// The owner-notice family for automatic updates, which is also the push kind.
#[cfg(all(target_os = "linux", feature = "host"))]
const AUTOMATIC_FAMILY: &str = "automatic_update";
#[cfg(all(target_os = "linux", feature = "host"))]
const PAUSED_KEY: &str = "automatic_update:paused";

/// Tell the owner once when automatic updates stop after failing twice, and end
/// the notice when they run again. Read from the kept state, so a pause that
/// happened while the journal was down is still told.
#[cfg(all(target_os = "linux", feature = "host"))]
fn reconcile_paused_notice(
    journal: &std::path::Path,
    data_dir: &std::path::Path,
    send: &dyn Fn(&std::path::Path, &Value),
) {
    use crate::window_catalog::{ENGLISH, text};
    use solstone_core_update::automatic;
    let paused = automatic::read_choice(data_dir) == automatic::Choice::On
        && automatic::read_state(data_dir).paused;
    let active = solstone_core_push::owner_notice::active_keys(journal, AUTOMATIC_FAMILY)
        .iter()
        .any(|key| key == PAUSED_KEY);
    if paused && !active {
        send(
            journal,
            &json!({
                "tract": "owner_notice",
                "event": "raise",
                "key": PAUSED_KEY,
                "title": text(ENGLISH, "journal.window.automatic.update.notice.paused.title"),
                "message": text(ENGLISH, "journal.window.automatic.update.notice.paused.body"),
                "action": "/app/home/",
            }),
        );
    } else if !paused && active {
        send(
            journal,
            &json!({"tract": "owner_notice", "event": "clear", "key": PAUSED_KEY}),
        );
    }
}

#[cfg(all(target_os = "linux", feature = "host"))]
fn send_owner_notice(journal: &std::path::Path, row: &Value) {
    let sender = solstone_core_callosum::CallosumOneShotSender::new(
        journal.join("health/callosum.sock"),
        std::time::Duration::from_secs(1),
    );
    if sender.send_line(&format!("{row}\n")).is_err() {
        log::debug!("automatic update owner notice unavailable");
    }
}

/// A clock set back past the last answer counts as due, rather than holding
/// the answer until the clock catches up.
#[cfg(target_os = "linux")]
fn update_check_due(
    verified_at: Option<std::time::SystemTime>,
    now: std::time::SystemTime,
) -> bool {
    verified_at.is_none_or(|at| {
        now.duration_since(at)
            .map_or(true, |age| age >= UPDATE_CHECK_EVERY)
    })
}

#[cfg(target_os = "linux")]
fn refresh_update_listing() -> bool {
    let (Some(data_dir), Ok(executable)) = (
        solstone_core_update::owner_data_dir(),
        std::env::current_exe(),
    ) else {
        return false;
    };
    solstone_core_update::refresh_stored_catalogue(
        &UPDATE_LISTING,
        &solstone_core_update::RealPlatformCatalogue,
        &solstone_core_update::RealPackageProbe,
        &executable,
        &data_dir,
        std::path::Path::new("/etc"),
    )
}

/// The status body every platform answers with; only how the version is
/// worked out differs between them.
fn status_body(
    version: Value,
    capture: Value,
    rev: Option<String>,
    integrity: solstone_core_system_health::IntegritySummary,
    photos_library_enabled: bool,
) -> Value {
    let mut capabilities: Vec<&'static str> = CAPABILITIES.to_vec();
    if photos_library_enabled {
        capabilities.push("photos_library");
    }
    let mut body = json!({
        "version": version,
        "capture": capture,
        "capabilities": capabilities,
        "sources_rev": rev,
        "integrity": integrity,
        "ok": true,
    });
    // What the security advisory check last found, on a desktop journal.
    #[cfg(feature = "host")]
    if let Some(security) = crate::security_advisory::status() {
        body["security"] = security;
    }
    body
}

#[allow(clippy::needless_return)]
pub async fn status(
    journal_root: PathBuf,
    revisions: std::sync::Arc<std::sync::Mutex<solstone_core_ingest::known_sources::RevisionCache>>,
) -> Response {
    let registry_root = journal_root.clone();
    let integrity_root = journal_root.clone();
    let rev = tokio::task::spawn_blocking(move || revisions.lock().ok()?.revision(&registry_root))
        .await
        .ok()
        .flatten();
    let integrity = tokio::task::spawn_blocking(move || {
        solstone_core_system_health::read_integrity_summary(&integrity_root)
    })
    .await
    .unwrap_or(solstone_core_system_health::IntegritySummary {
        state: solstone_core_system_health::IntegrityState::Unavailable,
        urgent_count: 0,
        incomplete: false,
        unavailable: true,
        disk_low: false,
        checked_at_ms: None,
    });
    let photos_library = solstone_core_photos_library::photos_library_enabled(&journal_root);
    let context = HomeContext::new(journal_root, Timestamp::now());
    let capture = get_capture_health(&context);
    #[cfg(target_os = "linux")]
    {
        let current_exe = std::env::current_exe();
        let version_report = tokio::task::spawn_blocking(move || {
            let Some(data_dir) = solstone_core_update::owner_data_dir() else {
                return solstone_core_update::UpdateReport::owner_context_unknown();
            };
            let exe = match current_exe {
                Ok(ref p) => p.as_path(),
                Err(_) => {
                    return solstone_core_update::UpdateReport::not_owned_unknown(
                        solstone_core_update::read_hold_state(&data_dir),
                    );
                }
            };
            let etc_dir = std::path::Path::new("/etc");
            let probe = RememberedProbe {
                inner: &solstone_core_update::RealPackageProbe,
                answers: &PAYLOAD_ANSWERS,
            };
            linux_version_status(&UPDATE_LISTING, &probe, exe, &data_dir, etc_dir)
        })
        .await
        .unwrap_or_else(|_| {
            solstone_core_update::UpdateReport::not_owned_unknown(
                solstone_core_update::owner_data_dir()
                    .map(|dir| solstone_core_update::read_hold_state(&dir))
                    .unwrap_or(solstone_core_update::HoldState::Unknown),
            )
        });
        let version = tokio::task::spawn_blocking({
            let current_exe = std::env::current_exe();
            move || linux_version_value(with_running_version(version_report), current_exe.ok())
        })
        .await
        .unwrap_or_else(|_| json!({"current": env!("CARGO_PKG_VERSION")}));
        return Json(status_body(
            version,
            json!(capture),
            rev,
            integrity,
            photos_library,
        ))
        .into_response();
    }

    #[cfg(not(target_os = "linux"))]
    {
        return Json(status_body(
            json!({
                "current": env!("CARGO_PKG_VERSION"),
                "latest": env!("CARGO_PKG_VERSION"),
                "update_available": false,
            }),
            json!(capture),
            rev,
            integrity,
            photos_library,
        ))
        .into_response();
    }
}

/// The checker's report, naming the version this journal is running when the
/// checker could not.
#[cfg(target_os = "linux")]
fn with_running_version(
    mut report: solstone_core_update::UpdateReport,
) -> solstone_core_update::UpdateReport {
    report
        .current
        .get_or_insert_with(|| env!("CARGO_PKG_VERSION").to_owned());
    report
}

/// The version status with what automatic updates and a package install add:
/// a newer journal already installed and waiting for the next start, the
/// owner's switch, and where a package install gets its updates.
#[cfg(target_os = "linux")]
fn linux_version_value(
    report: solstone_core_update::UpdateReport,
    executable: Option<PathBuf>,
) -> Value {
    use solstone_core_update::{Availability, OwnershipResult, PackageProbe, RouteKind, automatic};
    let Some(data_dir) = solstone_core_update::owner_data_dir() else {
        return json!(report);
    };
    let etc_dir = std::path::Path::new("/etc");
    let probe = RememberedProbe {
        inner: &solstone_core_update::RealPackageProbe,
        answers: &PAYLOAD_ANSWERS,
    };
    let report = executable.as_deref().map_or(report.clone(), |exe| {
        with_pending_restart(report, exe, &data_dir)
    });
    let ownership = executable
        .as_deref()
        .map(|exe| solstone_core_update::classify_ownership(exe, &data_dir, etc_dir, &probe));
    let pos = match &ownership {
        Some(OwnershipResult::Positive(pos)) => Some(pos),
        _ => None,
    };
    let mut automatic = automatic::status(pos, &data_dir);
    // A tree install whose newer journal waits for the next start is the same
    // install, so it can update itself as before.
    automatic.eligible |= report.availability == Availability::Installed
        && automatic::receipt_tree_prefix(&data_dir).is_some();
    let mut value = json!(report);
    value["automatic"] = json!(automatic);
    let package = pos.is_some_and(|pos| {
        matches!(
            pos.route_kind,
            RouteKind::PlatformPackage | RouteKind::PlatformMixed
        ) && pos
            .components
            .iter()
            .any(|c| (c.name == "journal" || c.name == "cli") && c.route != "tree")
    });
    if package {
        value["package"] = json!({"repository": probe.has_repo_source(etc_dir)});
    }
    value
}

/// A journal still running the version an update replaced is not "unknown":
/// the newer one is installed and starts the next time the journal does.
#[cfg(target_os = "linux")]
fn with_pending_restart(
    mut report: solstone_core_update::UpdateReport,
    executable: &std::path::Path,
    data_dir: &std::path::Path,
) -> solstone_core_update::UpdateReport {
    if report.code.as_deref() != Some("install-not-owned") {
        return report;
    }
    let Some(installed) =
        solstone_core_update::pending_restart(executable, data_dir, env!("CARGO_PKG_VERSION"))
    else {
        return report;
    };
    report.availability = solstone_core_update::Availability::Installed;
    report.current = Some(env!("CARGO_PKG_VERSION").to_owned());
    report.target = Some(solstone_core_update::UpdateTarget {
        platform_version: None,
        journal_version: installed,
    });
    report.code = None;
    report.message = None;
    report
}

/// `POST /api/system/updates/automatic` with `{"on": true|false}`: the pane's
/// one-time question and its switch. It reads the tree receipt, so a journal
/// waiting for its next start can still change it.
#[allow(clippy::needless_return)]
pub async fn set_automatic(body: Option<Json<Value>>) -> Response {
    #[cfg(target_os = "linux")]
    {
        use solstone_core_update::automatic;
        let Some(on) = body.and_then(|Json(body)| body.get("on").and_then(Value::as_bool)) else {
            return update_error(
                "invalid_request",
                "send {\"on\": true} or {\"on\": false}.",
                axum::http::StatusCode::BAD_REQUEST,
            );
        };
        let switched = tokio::task::spawn_blocking(move || {
            let (Some(data_dir), Some(home)) = (
                solstone_core_update::owner_data_dir(),
                std::env::var_os("HOME").map(PathBuf::from),
            ) else {
                return Err((
                    "owner-context-unknown",
                    "couldn't find your data directory.".to_owned(),
                ));
            };
            let result = if on {
                let Some(prefix) = automatic::receipt_tree_prefix(&data_dir) else {
                    return Err((
                        "not-eligible",
                        "this install can't update itself.".to_owned(),
                    ));
                };
                automatic::turn_on_at(&prefix, &data_dir, &home, &automatic::RealSystemctl)
            } else {
                automatic::turn_off(&data_dir, &home, &automatic::RealSystemctl)
            };
            result.map_err(|refusal| (refusal.code, refusal.message))
        })
        .await
        .unwrap_or_else(|_| {
            Err((
                "automatic_update_failed",
                "couldn't change automatic updates.".to_owned(),
            ))
        });
        return match switched {
            Ok(()) => Json(json!({"ok": true, "on": on})).into_response(),
            Err((code, message)) => update_error(code, &message, axum::http::StatusCode::CONFLICT),
        };
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = body;
        axum::http::StatusCode::NOT_FOUND.into_response()
    }
}

/// `POST /api/system/updates/retry`: the pane's "try again" after a pause.
#[allow(clippy::needless_return)]
pub async fn retry_automatic() -> Response {
    #[cfg(target_os = "linux")]
    {
        let done = tokio::task::spawn_blocking(|| {
            let data_dir = solstone_core_update::owner_data_dir()
                .ok_or_else(|| "couldn't find your data directory.".to_owned())?;
            solstone_core_update::automatic::try_again(
                &data_dir,
                &solstone_core_update::automatic::RealSystemctl,
            )
            .map_err(|refusal| refusal.message)
        })
        .await
        .unwrap_or_else(|_| Err("couldn't start the update.".to_owned()));
        return match done {
            Ok(()) => Json(json!({"ok": true})).into_response(),
            Err(message) => update_error(
                "automatic_update_failed",
                &message,
                axum::http::StatusCode::INTERNAL_SERVER_ERROR,
            ),
        };
    }
    #[cfg(not(target_os = "linux"))]
    axum::http::StatusCode::NOT_FOUND.into_response()
}

/// `POST /api/system/restart`: the owner's press on "restart your journal".
/// The restart is queued, not waited for: it ends this process too.
#[allow(clippy::needless_return)]
pub async fn restart_journal() -> Response {
    #[cfg(target_os = "linux")]
    {
        let queued = tokio::task::spawn_blocking(solstone_core_update::automatic::restart_journal)
            .await
            .unwrap_or(false);
        return if queued {
            (axum::http::StatusCode::ACCEPTED, Json(json!({"ok": true}))).into_response()
        } else {
            update_error(
                "restart_failed",
                "couldn't restart your journal. run solstone journal service restart.",
                axum::http::StatusCode::INTERNAL_SERVER_ERROR,
            )
        };
    }
    #[cfg(not(target_os = "linux"))]
    axum::http::StatusCode::NOT_FOUND.into_response()
}

#[cfg(target_os = "linux")]
fn update_error(code: &str, message: &str, status: axum::http::StatusCode) -> Response {
    solstone_core_convey_http::envelope::error_envelope(code, message, code, status).into_response()
}

#[cfg(target_os = "linux")]
fn linux_version_status(
    catalogue: &dyn solstone_core_update::PlatformCatalogue,
    probe: &dyn solstone_core_update::PackageProbe,
    executable: &std::path::Path,
    data: &std::path::Path,
    etc: &std::path::Path,
) -> solstone_core_update::UpdateReport {
    solstone_core_update::check(catalogue, probe, executable, data, etc, true, None)
}

/// Whether the installed tree matches its manifest, as last checked for an
/// executable file that has not changed since. The check hashes the whole
/// installed tree: on a processor without SHA instructions that took 7 to 9 s
/// a read, longer than a phone waits for status, so the phone never learned
/// what this journal understands. A changed file is checked again.
#[cfg(target_os = "linux")]
static PAYLOAD_ANSWERS: PayloadAnswers = PayloadAnswers::new();

#[cfg(target_os = "linux")]
type MatchMap = std::collections::HashMap<PayloadKey, bool>;

#[cfg(target_os = "linux")]
type VersionMap = std::collections::HashMap<PayloadKey, Option<String>>;

#[cfg(target_os = "linux")]
struct PayloadAnswers {
    matches: std::sync::Mutex<Option<MatchMap>>,
    versions: std::sync::Mutex<Option<VersionMap>>,
}

#[cfg(target_os = "linux")]
#[derive(Clone, PartialEq, Eq, Hash)]
struct PayloadKey {
    executable: PathBuf,
    version: String,
    len: u64,
    modified: std::time::SystemTime,
}

#[cfg(target_os = "linux")]
impl PayloadAnswers {
    const fn new() -> Self {
        Self {
            matches: std::sync::Mutex::new(None),
            versions: std::sync::Mutex::new(None),
        }
    }

    fn key_for(executable: &std::path::Path, version: &str) -> Option<PayloadKey> {
        let normalized = solstone_core_update::normalize_executable_path(executable);
        std::fs::canonicalize(normalized)
            .and_then(|path| {
                let meta = std::fs::metadata(&path)?;
                Ok(PayloadKey {
                    executable: path,
                    version: version.to_owned(),
                    len: meta.len(),
                    modified: meta.modified()?,
                })
            })
            .ok()
    }

    fn matches(
        &self,
        inner: &dyn solstone_core_update::PackageProbe,
        executable: &std::path::Path,
        version: &str,
    ) -> bool {
        let Some(key) = Self::key_for(executable, version) else {
            return inner.payload_matches(executable, version);
        };
        // Held across the check, so reads that arrive together hash once.
        let mut answers = self
            .matches
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let answers = answers.get_or_insert_with(Default::default);
        if let Some(matches) = answers.get(&key) {
            return *matches;
        }
        let matches = inner.payload_matches(executable, version);
        answers
            .retain(|known, _| known.executable != key.executable || known.version != key.version);
        answers.insert(key, matches);
        matches
    }

    fn journal_version(
        &self,
        inner: &dyn solstone_core_update::PackageProbe,
        executable: &std::path::Path,
    ) -> Option<String> {
        let Some(key) = Self::key_for(executable, "") else {
            return inner.journal_version(executable);
        };
        let mut versions = self
            .versions
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let versions = versions.get_or_insert_with(Default::default);
        if let Some(found) = versions.get(&key) {
            return found.clone();
        }
        let found = inner.journal_version(executable);
        versions.retain(|known, _| known.executable != key.executable);
        versions.insert(key, found.clone());
        found
    }
}

#[cfg(target_os = "linux")]
struct RememberedProbe<'a> {
    inner: &'a dyn solstone_core_update::PackageProbe,
    answers: &'a PayloadAnswers,
}

#[cfg(target_os = "linux")]
impl solstone_core_update::PackageProbe for RememberedProbe<'_> {
    fn owns_journal(&self, executable: &std::path::Path) -> bool {
        self.inner.owns_journal(executable)
    }

    fn journal_version(&self, executable: &std::path::Path) -> Option<String> {
        let version = env!("CARGO_PKG_VERSION");
        if self.payload_matches(executable, version) {
            return Some(version.to_owned());
        }
        self.answers.journal_version(self.inner, executable)
    }

    fn payload_matches(&self, executable: &std::path::Path, version: &str) -> bool {
        self.answers.matches(self.inner, executable, version)
    }

    fn package_route(&self, executable: &std::path::Path) -> Option<String> {
        self.inner.package_route(executable)
    }

    fn installed_package_version(&self, package_name: &str) -> Option<String> {
        self.inner.installed_package_version(package_name)
    }

    fn has_repo_source(&self, etc_dir: &std::path::Path) -> bool {
        self.inner.has_repo_source(etc_dir)
    }
}

#[cfg(all(test, target_os = "linux"))]
mod payload_answer_tests {
    use super::*;
    use solstone_core_update::PackageProbe;
    use std::sync::atomic::{AtomicUsize, Ordering};

    struct Counting(AtomicUsize);
    impl PackageProbe for Counting {
        fn owns_journal(&self, _: &std::path::Path) -> bool {
            false
        }
        fn payload_matches(&self, _: &std::path::Path, _: &str) -> bool {
            self.0.fetch_add(1, Ordering::SeqCst);
            true
        }
    }

    #[test]
    fn an_unchanged_executable_is_checked_once_and_a_changed_one_again() {
        let dir = tempfile::tempdir().unwrap();
        let exe = dir.path().join("solstone-core");
        std::fs::write(&exe, b"payload").unwrap();
        let inner = Counting(AtomicUsize::new(0));
        let answers = PayloadAnswers::new();
        let probe = RememberedProbe {
            inner: &inner,
            answers: &answers,
        };

        assert_eq!(
            probe.journal_version(&exe).as_deref(),
            Some(env!("CARGO_PKG_VERSION"))
        );
        assert!(probe.payload_matches(&exe, env!("CARGO_PKG_VERSION")));
        assert_eq!(inner.0.load(Ordering::SeqCst), 1);

        std::fs::write(&exe, b"a changed payload").unwrap();
        assert!(probe.payload_matches(&exe, env!("CARGO_PKG_VERSION")));
        assert_eq!(inner.0.load(Ordering::SeqCst), 2);
    }
}

#[cfg(all(test, feature = "full-tests", target_os = "linux"))]
mod update_tests {
    use super::*;
    use solstone_core_update::*;
    use std::path::{Path, PathBuf};
    struct Fetch(PathBuf);
    impl InstallerFetch for Fetch {
        fn fetch(&self, _: InstallerSource) -> std::io::Result<PathBuf> {
            Ok(self.0.clone())
        }
    }
    struct Catalogue(bool);
    impl PlatformCatalogue for Catalogue {
        fn listing(&self, _: &str, _: Option<&str>) -> Result<CatalogueListing, CatalogueRefusal> {
            if self.0 {
                Ok(CatalogueListing {
                    platform_version: "2.0.49-r2".to_owned(),
                    journal_version: "2.0.49".to_owned(),
                    desktop_version: None,
                    installer: None,
                })
            } else {
                Err(CatalogueRefusal {
                    code: "signature-invalid",
                    message: "the release catalogue failed its signature check.".to_owned(),
                })
            }
        }
    }
    struct Probe;
    impl PackageProbe for Probe {
        fn owns_journal(&self, _: &Path) -> bool {
            false
        }
        fn payload_matches(&self, _: &Path, _: &str) -> bool {
            true
        }
    }
    #[test]
    fn linux_status_and_cli_check_agree_for_available_and_failed_metadata() {
        let dir = tempfile::tempdir().unwrap();
        let prefix = dir.path().join("prefix");
        let entry = "2.0.48-0123456789ab";
        let version = prefix.join("versions").join(entry);
        std::fs::create_dir_all(version.join("bin")).unwrap();
        std::fs::write(version.join("bin/solstone"), "fixture").unwrap();
        std::fs::write(
            version.join(".archive-sha256"),
            "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef",
        )
        .unwrap();
        std::fs::write(version.join(".release"), "version=2.0.48\n").unwrap();
        std::fs::write(
            prefix.join("install-receipt"),
            "role=journal\nsignature_verification=verified\nservice_policy=start\n",
        )
        .unwrap();
        std::os::unix::fs::symlink(format!("versions/{entry}"), prefix.join("current")).unwrap();
        let data = dir.path().join("data");
        std::fs::create_dir_all(&data).unwrap();
        std::fs::write(data.join("install.conf"), format!("[solstone]\nschema_version=1\norigin=https://updates.solstone.app\nplatform_version=2.0.48-r1\nlane=release\nroute=tree\nphase=complete\nprefix={}\n[component:journal]\nroute=tree\nstatus=installed\n", prefix.display())).unwrap();
        let etc = dir.path().join("etc");
        let executable = prefix.join("current/bin/solstone");
        let fetch = Fetch(dir.path().join("installer.sh"));
        let args = parse_args(&[std::ffi::OsString::from("--check")]).unwrap();
        for good in [true, false] {
            let catalogue = Catalogue(good);
            let status = linux_version_status(&catalogue, &Probe, &executable, &data, &etc);
            let cli = perform(
                &catalogue,
                &RealInstallerRunner,
                &fetch,
                &Probe,
                &executable,
                &data,
                &etc,
                true,
                &args,
            )
            .unwrap_or_else(|(report, _)| report);
            assert_eq!(
                status.availability,
                if good {
                    Availability::Available
                } else {
                    Availability::Unknown
                }
            );
            assert_eq!(status.availability, cli.availability);
            assert_eq!(status.current, cli.current);
            assert_eq!(status.target, cli.target);
            assert_eq!(status.latest_platform_version, cli.latest_platform_version);
        }
    }

    /// Stands in for the update origin and counts what reaches it.
    struct CountingOrigin(std::sync::atomic::AtomicUsize);
    impl PlatformCatalogue for CountingOrigin {
        fn listing(
            &self,
            lane: &str,
            version: Option<&str>,
        ) -> Result<CatalogueListing, CatalogueRefusal> {
            self.0.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Catalogue(true).listing(lane, version)
        }
    }

    #[test]
    fn status_reads_never_reach_the_update_origin_and_only_an_owned_install_checks() {
        use std::sync::atomic::{AtomicUsize, Ordering::SeqCst};
        let dir = tempfile::tempdir().unwrap();
        let prefix = dir.path().join("prefix");
        let entry = "2.0.48-0123456789ab";
        let version = prefix.join("versions").join(entry);
        std::fs::create_dir_all(version.join("bin")).unwrap();
        std::fs::write(version.join("bin/solstone"), "fixture").unwrap();
        std::fs::write(
            version.join(".archive-sha256"),
            "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef",
        )
        .unwrap();
        std::fs::write(version.join(".release"), "version=2.0.48\n").unwrap();
        std::fs::write(
            prefix.join("install-receipt"),
            "role=journal\nsignature_verification=verified\nservice_policy=start\n",
        )
        .unwrap();
        std::os::unix::fs::symlink(format!("versions/{entry}"), prefix.join("current")).unwrap();
        let data = dir.path().join("data");
        std::fs::create_dir_all(&data).unwrap();
        std::fs::write(data.join("install.conf"), format!("[solstone]\nschema_version=1\norigin=https://updates.solstone.app\nplatform_version=2.0.48-r1\nlane=release\nroute=tree\nphase=complete\nprefix={}\n[component:journal]\nroute=tree\nstatus=installed\n", prefix.display())).unwrap();
        let etc = dir.path().join("etc");
        let executable = prefix.join("current/bin/solstone");
        let origin = CountingOrigin(AtomicUsize::new(0));
        let stored = StoredCatalogue::new();

        // A copy with no owned install asks nobody.
        let empty = dir.path().join("empty");
        std::fs::create_dir_all(&empty).unwrap();
        let elsewhere = dir.path().join("elsewhere/solstone");
        assert!(!refresh_stored_catalogue(
            &stored, &origin, &Probe, &elsewhere, &empty, &etc
        ));
        assert_eq!(origin.0.load(SeqCst), 0);

        // Until the first check answers, status says so without asking.
        let status = linux_version_status(&stored, &Probe, &executable, &data, &etc);
        assert_eq!(status.availability, Availability::Unknown);
        assert_eq!(status.code.as_deref(), Some("update-check-pending"));
        assert_eq!(status.current.as_deref(), Some("2.0.48"));

        // One check, then any number of reads from tabs and devices.
        assert!(refresh_stored_catalogue(
            &stored,
            &origin,
            &Probe,
            &executable,
            &data,
            &etc
        ));
        for _ in 0..100 {
            let status = linux_version_status(&stored, &Probe, &executable, &data, &etc);
            assert_eq!(status.availability, Availability::Available);
            assert_eq!(status.latest.as_deref(), Some("2.0.49"));
        }
        assert_eq!(origin.0.load(SeqCst), 1);

        // Local facts stay current between checks: a hold shows at once.
        set_hold(&data).unwrap();
        let status = linux_version_status(&stored, &Probe, &executable, &data, &etc);
        assert_eq!(status.hold, HoldState::Held);
        assert_eq!(origin.0.load(SeqCst), 1);
    }
}

#[cfg(all(test, target_os = "linux"))]
mod update_schedule_tests {
    use super::{UPDATE_CHECK_EVERY, update_check_due};
    use std::time::{Duration, SystemTime};

    #[test]
    fn a_verified_answer_stands_for_a_day_and_nothing_else_does() {
        let at = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000);
        assert!(update_check_due(None, at));
        assert!(!update_check_due(
            Some(at),
            at + UPDATE_CHECK_EVERY - Duration::from_secs(1)
        ));
        assert!(update_check_due(Some(at), at + UPDATE_CHECK_EVERY));
        assert!(update_check_due(Some(at), at - Duration::from_secs(1)));
    }
}

#[cfg(all(test, target_os = "linux"))]
mod running_version_tests {
    use super::*;
    use solstone_core_update::{Availability, HoldState, UpdateReport};

    #[test]
    fn linux_status_names_the_running_version_when_the_install_is_not_identified() {
        // The update checker cannot classify the install, but the journal
        // still knows what it is running; only the update fields stay unknown.
        for report in [
            UpdateReport::not_owned_unknown(HoldState::Unheld),
            UpdateReport::owner_context_unknown(),
        ] {
            let expected_code = report.code.clone();
            let status = with_running_version(report);
            assert_eq!(status.current.as_deref(), Some(env!("CARGO_PKG_VERSION")));
            assert_eq!(status.availability, Availability::Unknown);
            assert_eq!(status.latest, None);
            assert_eq!(status.code, expected_code);
        }
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::status_body;

    #[test]
    fn every_platform_status_lists_calendar_notes_and_keeps_its_fields() {
        // The Linux answer carries an update report and other platforms a plain
        // version; both go through the one builder.
        for (version, capture) in [
            (json!({"availability": "available"}), json!({"state": "ok"})),
            (
                json!({"current": "1.0.0", "latest": "1.0.0", "update_available": false}),
                json!({"state": "stale"}),
            ),
        ] {
            let integrity = solstone_core_system_health::IntegritySummary {
                state: solstone_core_system_health::IntegrityState::Ok,
                urgent_count: 0,
                incomplete: false,
                unavailable: false,
                disk_low: false,
                checked_at_ms: Some(1_700_000_000_000),
            };
            let body = status_body(
                version.clone(),
                capture.clone(),
                Some("1".into()),
                integrity.clone(),
                false,
            );
            let capabilities = body["capabilities"].as_array().unwrap();
            assert!(!capabilities.iter().any(|value| value == "photos_library"));
            let enabled_body = status_body(
                version.clone(),
                capture.clone(),
                Some("1".into()),
                integrity,
                true,
            );
            assert!(
                enabled_body["capabilities"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|value| value == "photos_library")
            );
            for capability in [
                "calendar_notes",
                "obsidian_notes",
                "resumable_import_uploads",
                "screenshots",
            ] {
                assert!(capabilities.iter().any(|value| value == capability));
            }
            assert_eq!(body["version"], version);
            assert_eq!(body["capture"], capture);
            assert_eq!(body["integrity"]["state"], "ok");
            assert_eq!(body["integrity"]["urgent_count"], 0);
            assert_eq!(body["integrity"]["incomplete"], false);
            assert_eq!(body["integrity"]["unavailable"], false);
            assert_eq!(body["integrity"]["disk_low"], false);
            assert_eq!(body["integrity"]["checked_at_ms"], 1_700_000_000_000_i64);
            assert_eq!(body["ok"], true);
        }
    }
}
