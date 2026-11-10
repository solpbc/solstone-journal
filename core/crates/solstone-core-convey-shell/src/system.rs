// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::path::PathBuf;

use axum::Json;
use axum::response::{IntoResponse, Response};
use chrono::Utc;
use serde_json::{Value, json};
use solstone_core_home::HomeContext;
use solstone_core_home::readers::get_capture_health;

/// What this journal understands from a linked device, beyond the ingest
/// protocol itself. A device sends a kind of content only to a journal that
/// lists it here.
const CAPABILITIES: &[&str] = &["calendar_notes", "obsidian_notes", "screenshots"];

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
pub(crate) fn start_update_check() {
    #[cfg(target_os = "linux")]
    {
        static STARTED: std::sync::Once = std::sync::Once::new();
        STARTED.call_once(|| {
            let spawned = std::thread::Builder::new()
                .name("update-check".to_owned())
                .spawn(run_update_check);
            if let Err(error) = spawned {
                log::warn!("update check did not start: {error}");
            }
        });
    }
}

#[cfg(target_os = "linux")]
fn run_update_check() {
    let mut verified_at = None;
    loop {
        if update_check_due(verified_at, std::time::SystemTime::now()) && refresh_update_listing() {
            verified_at = Some(std::time::SystemTime::now());
        }
        std::thread::sleep(UPDATE_CHECK_TICK);
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
fn status_body(version: Value, capture: Value) -> Value {
    let mut body = json!({
        "version": version,
        "capture": capture,
        "capabilities": CAPABILITIES,
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
pub async fn status(journal_root: PathBuf) -> Response {
    let context = HomeContext::new(journal_root, Utc::now());
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
            let probe = solstone_core_update::RealPackageProbe;
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

        let version_report = with_running_version(version_report);
        return Json(status_body(json!(version_report), json!(capture))).into_response();
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
            let body = status_body(version.clone(), capture.clone());
            let capabilities = body["capabilities"].as_array().unwrap();
            for capability in ["calendar_notes", "obsidian_notes", "screenshots"] {
                assert!(capabilities.iter().any(|value| value == capability));
            }
            assert_eq!(body["version"], version);
            assert_eq!(body["capture"], capture);
            assert_eq!(body["ok"], true);
        }
    }
}
