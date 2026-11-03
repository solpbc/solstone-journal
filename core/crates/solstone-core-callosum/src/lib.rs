// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Callosum wire envelopes and their durable per-segment event log.

#![deny(clippy::disallowed_methods, clippy::disallowed_types)]

mod local_inference;
mod model;
mod oneshot;
mod reader;
mod registry;
#[cfg(unix)]
mod unix_endpoint;
#[cfg(any(test, windows))]
pub mod windows;
#[cfg(feature = "wire")]
mod wire;
mod writer;
#[cfg(unix)]
pub use unix_endpoint::{UnixEndpoint, local_unix_listener_status};

/// Resolve an app-owned Callosum endpoint before opening a Unix socket.
/// Mobile app paths can exceed sockaddr_un even when their final symlink
/// points to a short app-private endpoint. Desktop journal addresses resolve
/// beside the owner HTTP socket when that journal has a runtime namespace.
///
/// # Errors
/// Returns an error when a mobile alias is missing, relative or too long.
#[cfg(unix)]
pub fn endpoint_path(
    path: &std::path::Path,
) -> std::io::Result<std::borrow::Cow<'_, std::path::Path>> {
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    if path.file_name() == Some(std::ffi::OsStr::new("callosum.sock"))
        && let Some(health) = path.parent()
        && health.file_name() == Some(std::ffi::OsStr::new("health"))
        && let Some(journal) = health.parent()
        && !journal.as_os_str().is_empty()
        && let Some(http) = solstone_core_local_endpoint::socket_for_journal(journal)?
    {
        return Ok(std::borrow::Cow::Owned(
            http.with_file_name("callosum.sock"),
        ));
    }
    #[cfg(any(target_os = "ios", target_os = "android"))]
    if path.as_os_str().len() >= 104
        || std::fs::symlink_metadata(path).is_ok_and(|metadata| metadata.file_type().is_symlink())
    {
        return short_socket_alias(path).map(std::borrow::Cow::Owned);
    }
    Ok(std::borrow::Cow::Borrowed(path))
}

#[cfg(any(
    target_os = "ios",
    target_os = "android",
    all(unix, test, feature = "full-tests")
))]
fn short_socket_alias(path: &std::path::Path) -> std::io::Result<std::path::PathBuf> {
    let target = std::fs::read_link(path)?;
    if !target.is_absolute() || target.as_os_str().len() >= 104 {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "socket alias requires a short absolute target",
        ));
    }
    Ok(target)
}

#[cfg(unix)]
#[cfg(all(test, feature = "full-tests"))]
mod endpoint_tests {
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    #[test]
    fn runtime_namespace_serves_both_clients_and_preserves_http_record() {
        use std::{path::Path, time::Duration};
        if std::env::var_os("CALLOSUM_NAMESPACE_TEST_CHILD").is_none() {
            let root = tempfile::tempdir_in("/var/tmp").unwrap();
            let output = std::process::Command::new(std::env::current_exe().unwrap())
                .args(["--exact", "endpoint_tests::runtime_namespace_serves_both_clients_and_preserves_http_record", "--nocapture"])
                .env("CALLOSUM_NAMESPACE_TEST_CHILD", root.path())
                .env(solstone_core_local_endpoint::SOCKET_ENV, root.path().join("runtime/journal.sock"))
                .output().unwrap();
            assert!(
                output.status.success(),
                "{}{}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
            return;
        }
        let root =
            std::path::PathBuf::from(std::env::var_os("CALLOSUM_NAMESPACE_TEST_CHILD").unwrap());
        let journal = root.join("journal");
        std::fs::create_dir(&journal).unwrap();
        let logical = journal.join("health/callosum.sock");
        let physical = root.join("runtime/callosum.sock");
        assert_eq!(super::endpoint_path(&logical).unwrap(), physical);
        assert_eq!(
            super::endpoint_path(Path::new("/other.sock")).unwrap(),
            Path::new("/other.sock")
        );
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        runtime.block_on(async {
            let server = super::CallosumSocketServer::bind(&logical).await.unwrap();
            assert!(physical.exists());
            assert!(!logical.exists());
            assert!(super::CallosumSocketServer::bind(&logical).await.is_err());
            // HTTP must bind beside Callosum, and stopping the companion must
            // leave the HTTP runtime record and listener intact.
            let http =
                solstone_core_local_endpoint::SocketLease::bind(&root.join("runtime/journal.sock"))
                    .unwrap();
            let record = root.join("runtime/journal.json");
            std::fs::write(&record, b"http-owner").unwrap();
            let mut client = super::CallosumSocketConnection::new(&logical, serde_json::Map::new());
            client.start();
            tokio::time::timeout(Duration::from_secs(5), async {
                while server.client_count() != 1 {
                    tokio::task::yield_now().await;
                }
            })
            .await
            .unwrap();
            let sender = super::CallosumOneShotSender::new(&logical, Duration::from_secs(2));
            tokio::task::spawn_blocking(move || {
                sender.send_line_confirmed("{\"tract\":\"control\",\"event\":\"runtime\"}\n")
            })
            .await
            .unwrap()
            .unwrap();
            let message = tokio::time::timeout(Duration::from_secs(5), client.next_message())
                .await
                .unwrap()
                .unwrap();
            assert_eq!(message.event, "runtime");
            client.stop().await;
            server.stop().await;
            assert!(!physical.exists());
            assert_eq!(std::fs::read(&record).unwrap(), b"http-owner");
            assert!(root.join("runtime/journal.sock").exists());
            drop(server);
            // HTTP already holds its lifetime directory lease when Cortex
            // restarts. The companion's creation lock must be independent.
            let restarted = super::CallosumSocketServer::bind(&logical).await.unwrap();
            assert!(physical.exists());
            restarted.stop().await;
            drop(restarted);
            assert_eq!(std::fs::read(&record).unwrap(), b"http-owner");
            assert!(root.join("runtime/journal.sock").exists());
            drop(http);
        });
    }

    #[test]
    fn long_socket_alias_reaches_the_same_unix_endpoint() {
        let root = tempfile::tempdir_in("/var/tmp").unwrap();
        let physical = root.path().join("s");
        let directory = root.path().join("long".repeat(32));
        std::fs::create_dir(&directory).unwrap();
        let logical = directory.join("callosum.sock");
        std::os::unix::fs::symlink(&physical, &logical).unwrap();
        let listener =
            std::os::unix::net::UnixListener::bind(super::short_socket_alias(&logical).unwrap())
                .unwrap();
        let _client =
            std::os::unix::net::UnixStream::connect(super::short_socket_alias(&logical).unwrap())
                .unwrap();
        let _peer = listener.accept().unwrap();
        std::fs::remove_file(&logical).unwrap();
        std::os::unix::fs::symlink("relative.sock", &logical).unwrap();
        assert!(super::short_socket_alias(&logical).is_err());
    }
}

pub use local_inference::LOCAL_INFERENCE_TOKEN_MAX;
#[cfg(feature = "wire")]
pub use local_inference::{
    LocalInferenceReadError, LocalInferenceSnapshot, LocalInferenceSnapshotOffer,
};
pub use model::{CallosumEnvelope, DeviceIngestEvent, DurableEvent, FileDescriptor, ReportedZone};
pub use oneshot::{CallosumOneShotError, CallosumOneShotSender};
pub use reader::{
    CallosumReadError, DeviceIngestReport, DurableEventsReport, read_device_ingest_events,
    read_durable_events, read_reported_zone,
};
pub use registry::callosum_registry;
#[cfg(feature = "full-tests")]
#[doc(hidden)]
pub use wire::one_shot_fixture as test_fixture;
#[cfg(all(feature = "wire", any(test, feature = "test-hooks")))]
#[doc(hidden)]
pub use wire::test_support;
#[cfg(feature = "wire")]
pub use wire::{
    BrainStartupCapture, BrainStartupCursor, BrainStartupEvent, CallosumConnectionPhase,
    CallosumDiagnosticEmitter, CallosumGapReason, CallosumReceiveEvent, CallosumRetrySource,
    CallosumSocketConnection, CallosumSocketServer, CallosumSocketServerError,
    CallosumStoppedReason, PrivateProofReceive, TokioRetrySource, request_local_inference_snapshot,
    request_local_inference_snapshot_sync,
};
pub use writer::{CallosumWriteError, append_durable_event, append_durable_event_checked};
