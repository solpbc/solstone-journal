// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::path::Path;
use std::sync::mpsc::sync_channel;
use std::thread;

use solstone_core_sol_link::http::{
    SetupDownloads, SetupHost, SetupProgressError, SetupRetryError, packaged_speech_retry,
    read_setup_downloads,
};
use solstone_core_thinking_copy::{
    DownloadMember, SetupCatalogFacts, current_setup_facts, sum_download_bytes,
};

pub(crate) struct ConveySetupHost;

impl SetupHost for ConveySetupHost {
    fn start_local(&self, journal_root: &Path) -> Result<(), String> {
        crate::thinking_install::start(journal_root, "local/qwen3.5-4b").map(|_| ())
    }

    fn cancel_local(&self, journal_root: &Path, attempt_id: &str) -> Result<(), String> {
        crate::thinking_install::cancel(journal_root, attempt_id).map(|_| ())
    }

    fn retry_speech(&self, journal_root: &Path) -> Result<(), SetupRetryError> {
        #[cfg(target_os = "windows")]
        let work = SpeechWork::WindowsPackage(
            solstone_core_local::install::parakeet_readiness::verified_windows_parakeet_package()
                .map(|_| ()),
        );

        #[cfg(target_os = "macos")]
        let work = SpeechWork::Coreml;

        #[cfg(all(not(target_os = "windows"), not(target_os = "macos")))]
        let work = SpeechWork::Linux {
            os: std::env::consts::OS,
            arch: std::env::consts::ARCH,
        };

        retry_speech_with(journal_root, work)
    }

    fn project_downloads(&self, journal_root: &Path) -> Result<SetupDownloads, SetupProgressError> {
        read_setup_downloads(journal_root)
    }

    fn thinking_download_bytes(&self, journal: &Path) -> Option<u64> {
        let rows = solstone_core_local::install::local_thinking_catalog_plan(journal, None)?;
        let members: Vec<DownloadMember> = rows
            .into_iter()
            .map(|row| {
                if row.packaged {
                    DownloadMember::Packaged
                } else {
                    DownloadMember::Catalog {
                        unit: row.unit,
                        platform: row.platform,
                        backend: None,
                        filename: row.filename,
                    }
                }
            })
            .collect();
        sum_download_bytes(&members)
    }

    fn catalog_facts(&self) -> SetupCatalogFacts {
        current_setup_facts()
    }

    fn refresh_brain(&self, journal_root: &Path) {
        if !crate::thinking::send_brain_refresh_request(journal_root) {
            log::warn!("setup: thinking check after the setup choice was not queued");
        }
    }
}

#[allow(dead_code)]
pub(crate) enum SpeechWork {
    WindowsPackage(Result<(), String>),
    Coreml,
    Linux {
        os: &'static str,
        arch: &'static str,
    },
    #[cfg(all(test, feature = "full-tests"))]
    Hold {
        before_admission: Option<std::sync::mpsc::Receiver<()>>,
        release: std::sync::mpsc::Receiver<()>,
        done: std::sync::mpsc::Sender<()>,
        fail: bool,
    },
}

struct AdmitDropGuard {
    tx: Option<std::sync::mpsc::SyncSender<Result<(), SetupRetryError>>>,
}

impl Drop for AdmitDropGuard {
    fn drop(&mut self) {
        if let Some(tx) = self.tx.take() {
            let _ = tx.send(Err(SetupRetryError {
                code: "speech_admission_pending",
                message: "speech install is starting",
                detail: "check speech progress in a moment.".into(),
            }));
        }
    }
}

pub(crate) fn retry_speech_with(journal: &Path, work: SpeechWork) -> Result<(), SetupRetryError> {
    retry_speech_with_timeout(journal, work, std::time::Duration::from_secs(5))
}

fn retry_speech_with_timeout(
    journal: &Path,
    work: SpeechWork,
    admission_timeout: std::time::Duration,
) -> Result<(), SetupRetryError> {
    if let SpeechWork::WindowsPackage(verified) = work {
        return packaged_speech_retry(verified);
    }

    let observed = solstone_core_local::install::status::read_observed_status(journal, "parakeet")
        .map_err(|error| SetupRetryError {
            code: "status_unreadable",
            message: "speech install could not start",
            detail: error.to_string(),
        })?;
    let lease = match solstone_core_local::install::lease::acquire(journal, "parakeet") {
        Ok(Some(lease)) => lease,
        Ok(None) => {
            // An exclusive lease alone cannot identify the attempt. In
            // particular, it can precede replacement of abandoned history.
            return Err(SetupRetryError {
                code: "speech_admission_pending",
                message: "speech install is starting",
                detail: "check speech progress in a moment.".into(),
            });
        }
        Err(error) => {
            return Err(SetupRetryError {
                code: "lease_unavailable",
                message: "speech install could not start",
                detail: error.to_string(),
            });
        }
    };
    let config = if matches!(work, SpeechWork::Coreml) {
        match solstone_core_journal_config::read_journal_config(journal) {
            Ok(config) => Some(config),
            Err(error) => {
                drop(lease);
                return Err(persist_speech_refusal(
                    journal,
                    &observed,
                    "config_unreadable",
                    error.to_string(),
                ));
            }
        }
    } else {
        None
    };

    let (tx, rx) = sync_channel::<Result<(), SetupRetryError>>(1);
    let journal_buf = journal.to_path_buf();
    let worker_observed = observed.clone();

    let spawn_res = thread::Builder::new()
        .name("speech-install".into())
        .spawn(move || {
            let mut worker_lease = Some(lease);
            let mut guard = AdmitDropGuard { tx: Some(tx) };
            let on_admitted = |_status: &solstone_core_local::install::status::InstallStatus| {
                if let Some(tx) = guard.tx.take() {
                    let _ = tx.send(Ok(()));
                }
            };
            let result = match work {
                SpeechWork::WindowsPackage(_) => Ok(()),
                SpeechWork::Coreml => {
                    let lease = worker_lease.take().expect("speech worker owns the lease");
                    let home = std::env::home_dir().unwrap_or_default();
                    solstone_core_local::install::coreml_install::run_coreml_parakeet_attempt(
                        &journal_buf,
                        &lease,
                        &home,
                        config.as_ref().expect("CoreML config was read"),
                        false,
                        on_admitted,
                    )
                    .map(|_| ())
                    .map_err(|error| (error.reason_code.clone(), error.to_string()))
                }
                SpeechWork::Linux { os, arch } => {
                    let lease = worker_lease.take().expect("speech worker owns the lease");
                    solstone_core_local::install::install_parakeet_with_lease_admitted(
                        &journal_buf,
                        os,
                        arch,
                        lease,
                        on_admitted,
                    )
                    .map(|_| ())
                    .map_err(|error| {
                        let code = error
                            .envelope
                            .error
                            .as_ref()
                            .map_or("install_failed", |error| error.reason_code.as_str())
                            .to_owned();
                        (code, format!("{error:?}"))
                    })
                }
                #[cfg(all(test, feature = "full-tests"))]
                SpeechWork::Hold {
                    before_admission,
                    release,
                    done,
                    fail,
                } => {
                    let lease = worker_lease.take().expect("speech worker owns the lease");
                    if let Some(before_admission) = before_admission {
                        let _ = before_admission.recv();
                    }
                    if let Ok(written) = solstone_core_local::install::status::begin_or_replace(
                        &journal_buf,
                        "parakeet",
                        r#"{"catalog_version":"test","provider":"parakeet","variant":"coreml"}"#
                            .to_string(),
                        "test-fingerprint".to_string(),
                        None,
                        "downloading",
                        None,
                    ) {
                        let mut on_admitted = on_admitted;
                        on_admitted(&written);
                        if release.recv().is_ok() {
                            if fail {
                                if let Ok(next) = solstone_core_local::install::status::transition(
                                    written,
                                    "failed",
                                    Some("failed".into()),
                                    Some("publication_failed".into()),
                                ) {
                                    let _ = solstone_core_local::install::status::write_status(
                                        &journal_buf,
                                        next,
                                    );
                                }
                            } else {
                                if let Ok(next) = solstone_core_local::install::status::transition(
                                    written,
                                    "installed",
                                    None,
                                    None,
                                ) {
                                    let _ = solstone_core_local::install::status::write_status(
                                        &journal_buf,
                                        next,
                                    );
                                }
                            }
                            drop(lease);
                            let _ = done.send(());
                        }
                    }
                    Ok(())
                }
            };
            drop(worker_lease);
            if let Err((code, detail)) = result
                && let Some(tx) = guard.tx.take()
            {
                let error = persist_speech_refusal(
                    &journal_buf,
                    &worker_observed,
                    "speech_admission_failed",
                    format!("{code}: {detail}"),
                );
                let _ = tx.send(Err(error));
            }
        });

    if let Err(e) = spawn_res {
        return Err(persist_speech_refusal(
            journal,
            &observed,
            "install_start_failed",
            e.to_string(),
        ));
    }

    match rx.recv_timeout(admission_timeout) {
        Ok(res) => res,
        Err(_) => Err(SetupRetryError {
            code: "speech_admission_pending",
            message: "speech install is starting",
            detail: "check speech progress in a moment.".into(),
        }),
    }
}

fn persist_speech_refusal(
    journal: &Path,
    observed: &solstone_core_local::install::status::InstallStatus,
    code: &'static str,
    detail: String,
) -> SetupRetryError {
    match solstone_core_local::install::status::record_terminal_refusal_if_unchanged(
        journal,
        "parakeet",
        observed.attempt_id.as_deref(),
        observed.revision,
        code,
        &detail,
    ) {
        Ok(_) => SetupRetryError {
            code,
            message: "speech install could not start",
            detail,
        },
        Err(error) => SetupRetryError {
            code: "status_write_failed",
            message: "speech install could not start",
            detail: error.to_string(),
        },
    }
}

#[cfg(all(test, feature = "full-tests"))]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn held_fetch_retry_returns_after_admit_and_second_retry_observes_same_attempt() {
        let temp = TempDir::new().expect("tempdir");
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        let (done_tx, done_rx) = std::sync::mpsc::channel();

        let work = SpeechWork::Hold {
            before_admission: None,
            release: release_rx,
            done: done_tx,
            fail: false,
        };

        let res1 = retry_speech_with(temp.path(), work);
        assert!(
            res1.is_ok(),
            "first retry_speech_with must return Ok after admit"
        );

        let status = solstone_core_local::install::status::read_status(temp.path(), "parakeet")
            .expect("read status");
        assert_eq!(status.install_state, "downloading");
        assert!(
            solstone_core_local::install::lease::is_held(temp.path(), "parakeet").unwrap_or(false)
        );
        let attempt_id = status.attempt_id.clone();
        assert!(attempt_id.is_some());

        let res2 = retry_speech_with(
            temp.path(),
            SpeechWork::Linux {
                os: "linux",
                arch: "x86_64",
            },
        );
        assert_eq!(res2.unwrap_err().code, "speech_admission_pending");

        let status2 = solstone_core_local::install::status::read_status(temp.path(), "parakeet")
            .expect("read status 2");
        assert_eq!(
            status2.attempt_id, attempt_id,
            "attempt id must not change on join"
        );

        release_tx.send(()).expect("send release");
        done_rx.recv().expect("recv done");

        let released =
            solstone_core_local::install::lease::acquire(temp.path(), "parakeet").unwrap();
        assert!(
            released.is_some(),
            "finished speech must release the worker lease"
        );
        drop(released);
        let final_status =
            solstone_core_local::install::status::read_status(temp.path(), "parakeet")
                .expect("final read status");
        assert_eq!(final_status.install_state, "installed");
    }

    #[test]
    fn retry_during_pre_admit_returns_speech_admission_pending() {
        let temp = TempDir::new().expect("tempdir");
        let downloading = solstone_core_local::install::status::begin_or_replace(
            temp.path(),
            "parakeet",
            r#"{"catalog_version":"test"}"#.to_string(),
            "test-hash".to_string(),
            None,
            "downloading",
            None,
        )
        .expect("begin downloading");
        let status = downloading;
        let expected_attempt_id = status.attempt_id.clone();
        let expected_revision = status.revision;

        let held = solstone_core_local::install::lease::acquire(temp.path(), "parakeet")
            .expect("acquire lease")
            .expect("got lease");

        let err = retry_speech_with(
            temp.path(),
            SpeechWork::Linux {
                os: "linux",
                arch: "x86_64",
            },
        )
        .expect_err("must error during pre-admit");
        assert_eq!(err.code, "speech_admission_pending");

        let status_after =
            solstone_core_local::install::status::read_status(temp.path(), "parakeet")
                .expect("re-read status");
        assert_eq!(status_after.attempt_id, expected_attempt_id);
        assert_eq!(status_after.install_state, "downloading");
        assert_eq!(status_after.revision, expected_revision);

        drop(held);
    }

    #[test]
    fn admission_timeout_leaves_the_single_worker_alive() {
        let temp = TempDir::new().unwrap();
        let (before_tx, before_rx) = std::sync::mpsc::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        let (done_tx, done_rx) = std::sync::mpsc::channel();
        let error = retry_speech_with_timeout(
            temp.path(),
            SpeechWork::Hold {
                before_admission: Some(before_rx),
                release: release_rx,
                done: done_tx,
                fail: false,
            },
            std::time::Duration::from_millis(20),
        )
        .unwrap_err();
        assert_eq!(error.code, "speech_admission_pending");
        assert_eq!(
            solstone_core_local::install::status::read_status(temp.path(), "parakeet")
                .unwrap()
                .install_state,
            "idle"
        );
        assert!(solstone_core_local::install::lease::is_held(temp.path(), "parakeet").unwrap());
        before_tx.send(()).unwrap();
        release_tx.send(()).unwrap();
        done_rx
            .recv_timeout(std::time::Duration::from_secs(2))
            .unwrap();
        assert_eq!(
            solstone_core_local::install::status::read_status(temp.path(), "parakeet")
                .unwrap()
                .install_state,
            "installed"
        );
    }

    #[test]
    fn unsupported_speech_target_refusal_survives_the_retry_response() {
        let temp = TempDir::new().unwrap();
        let result = retry_speech_with(
            temp.path(),
            SpeechWork::Linux {
                os: "linux",
                arch: "unsupported",
            },
        );
        assert!(result.is_err());
        let status =
            solstone_core_local::install::status::read_observed_status(temp.path(), "parakeet")
                .unwrap();
        assert_eq!(status.install_state, "failed");
        assert_eq!(status.error_code.as_deref(), Some("unsupported_platform"));
        assert!(!solstone_core_local::install::lease::is_held(temp.path(), "parakeet").unwrap());
    }

    #[test]
    fn windows_package_ok_projects_complete_without_fetch() {
        // Windows native qualification belongs to the caller. This injection is not signed-package proof.
        let temp = TempDir::new().expect("tempdir");
        let res = retry_speech_with(temp.path(), SpeechWork::WindowsPackage(Ok(())));
        assert!(res.is_ok());

        let parakeet_json = temp
            .path()
            .join("health")
            .join("providers")
            .join("parakeet.json");
        assert!(!parakeet_json.exists());

        let res2 = retry_speech_with(temp.path(), SpeechWork::WindowsPackage(Ok(())));
        assert!(res2.is_ok());

        let proj = solstone_core_sol_link::setup_progress::project_speech(None, false, true, None);
        assert_eq!(proj.state, "complete");
    }

    #[test]
    fn windows_package_err_is_stable() {
        // Windows native qualification belongs to the caller. This injection is not signed-package proof.
        let temp = TempDir::new().expect("tempdir");
        for _ in 0..2 {
            let err = retry_speech_with(
                temp.path(),
                SpeechWork::WindowsPackage(Err("different".into())),
            )
            .expect_err("must fail");
            assert_eq!(err.code, "speech_package_unavailable");
            assert_eq!(
                err.detail,
                "repair or reinstall the journal app, then restart your journal."
            );
        }
        let parakeet_json = temp
            .path()
            .join("health")
            .join("providers")
            .join("parakeet.json");
        assert!(!parakeet_json.exists());
    }

    #[tokio::test]
    async fn save_thinking_and_finalize_preserve_an_existing_local_refusal() {
        use axum::Extension;
        use axum::body::{Body, to_bytes};
        use axum::http::{Method, Request, StatusCode};
        use solstone_core_convey_http::identity::AccessBasis;
        use std::sync::Arc;
        use tower::ServiceExt;

        let temp = TempDir::new().expect("tempdir");
        let make_request = |method: Method, uri: &'static str, body: Option<serde_json::Value>| {
            let journal = temp.path();
            let app = solstone_core_sol_link::http::init_router_with_host(
                journal,
                Arc::new(ConveySetupHost),
            )
            .layer(Extension(AccessBasis::Localhost));
            async move {
                let mut builder = Request::builder().method(method).uri(uri);
                let body_payload = if let Some(val) = body {
                    builder = builder.header("content-type", "application/json");
                    Body::from(val.to_string())
                } else {
                    Body::empty()
                };
                let resp = app
                    .oneshot(builder.body(body_payload).unwrap())
                    .await
                    .unwrap();
                let status = resp.status();
                let bytes = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
                let json: serde_json::Value =
                    serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null);
                (status, json)
            }
        };

        let _ = ConveySetupHost.start_local(temp.path());
        let (status, _) = make_request(
            Method::POST,
            "/init/api/action",
            Some(serde_json::json!({"action": "save_thinking", "choice": "later"})),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        let (status, state) = make_request(Method::GET, "/init/api/state", None).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(state["downloads"]["thinking"]["state"], "error");
        let err_code = state["downloads"]["thinking"]["error"]["code"]
            .as_str()
            .unwrap();
        assert!(err_code == "installer_missing" || err_code == "installer_launch_failed");
        assert_eq!(state["thinking"]["choice"], "later");

        // 4. Before finalize, establish::current_candidate then establish::lock_in
        let _ = solstone_core_sol_link::establish::current_candidate(temp.path());
        let _ = solstone_core_sol_link::establish::lock_in(temp.path());

        // 5. POST /init/finalize with body {"lane":"local"}. Response JSON success is true.
        let (status, fin_json) = make_request(
            Method::POST,
            "/init/finalize",
            Some(serde_json::json!({"lane": "later"})),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(fin_json["success"], true);

        // 6. GET again: same error code, choice still "local".
        let (status, state2) = make_request(Method::GET, "/init/api/state", None).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(state2["downloads"]["thinking"]["error"]["code"], err_code);
        assert_eq!(state2["thinking"]["choice"], "later");
    }
}
