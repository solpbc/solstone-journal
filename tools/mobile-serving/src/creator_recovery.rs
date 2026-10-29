// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Stopped-owner creator recovery is separate from ordinary mobile startup.

use std::sync::Arc;

use solstone_core_journal_io::JournalRoot;

pub(crate) const REQUIRED: &str = "creator_recovery_required";

/// Observe before ordinary startup can publish config or construct workers.
/// This read never retires a reservation or interprets a saved record as IO authority.
pub(crate) fn require_ordinary_start(root: &Arc<JournalRoot>) -> Result<(), String> {
    root.revalidate_canonical_binding()
        .map_err(|_| REQUIRED.to_owned())?;
    if solstone_core_journal_io::whole_journal_restore::whole_journal_restore_blocks(
        root.canonical_path(),
    ) {
        // Keep the existing durable restore attempt's recovery router reachable;
        // its ordinary admission remains fenced by that original attempt.
        return root
            .revalidate_canonical_binding()
            .map_err(|_| REQUIRED.to_owned());
    }
    let census = solstone_core_journal_io::activity_obligations::observe_obligations(root)
        .map_err(|_| REQUIRED.to_owned())?;
    root.revalidate_canonical_binding()
        .map_err(|_| REQUIRED.to_owned())?;
    if census.entries().is_empty() {
        Ok(())
    } else {
        Err(REQUIRED.to_owned())
    }
}

#[cfg(feature = "processing")]
pub fn run_stopped(
    root: Arc<JournalRoot>,
    command: &serde_json::Value,
    checkpoint: &dyn Fn() -> Result<(), String>,
) -> Result<serde_json::Value, String> {
    solstone_core_backup_web::creator_recovery::run_stopped(root, command, checkpoint)
}

#[cfg(all(test, feature = "full-tests"))]
#[cfg(target_os = "linux")]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn address() -> std::net::SocketAddrV4 {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let std::net::SocketAddr::V4(address) = listener.local_addr().unwrap() else {
            unreachable!()
        };
        address
    }

    fn start(root: &std::path::Path) -> Result<crate::MobileServing, String> {
        crate::MobileServing::start_with_payload(
            root.to_owned(),
            address(),
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../core/payload"),
        )
    }

    fn refused_before_config(root: &std::path::Path) {
        match start(root) {
            Ok(server) => {
                server.shutdown_for_retirement().unwrap();
                panic!("ordinary startup crossed unobserved creator frontier");
            }
            Err(error) => assert_eq!(error, REQUIRED),
        }
        assert!(!root.join("config/journal.json").exists());
        assert!(!root.join("backup-activity.lock").exists());
    }

    #[test]
    fn complete_clean_journal_still_starts_and_joins() {
        let directory = tempfile::tempdir().unwrap();
        let server = start(directory.path()).unwrap();
        assert!(directory.path().join("config/journal.json").is_file());
        server.shutdown_for_retirement().unwrap();
    }

    #[test]
    fn incomplete_census_refuses_instead_of_becoming_empty() {
        let directory = tempfile::tempdir().unwrap();
        let chronicle = directory.path().join("chronicle");
        std::fs::write(&chronicle, b"not a directory").unwrap();
        refused_before_config(directory.path());
        assert_eq!(std::fs::read(chronicle).unwrap(), b"not a directory");
        assert!(!directory.path().join("health").exists());
    }

    #[test]
    fn census_budget_refuses_without_retiring_any_reservation() {
        let directory = tempfile::tempdir().unwrap();
        for index in 0..4097 {
            std::fs::create_dir(directory.path().join(format!(".tmp-offload-{index:032x}")))
                .unwrap();
        }
        refused_before_config(directory.path());
        assert_eq!(std::fs::read_dir(directory.path()).unwrap().count(), 4097);
        assert!(!directory.path().join("health").exists());
    }

    #[test]
    fn unknown_restore_intent_stays_intact_before_startup() {
        let directory = tempfile::tempdir().unwrap();
        let intents = directory.path().join("health/offload/restore-intents");
        std::fs::create_dir_all(&intents).unwrap();
        let unknown = intents.join("unknown");
        std::fs::write(&unknown, b"unknown restore facts").unwrap();
        refused_before_config(directory.path());
        assert_eq!(std::fs::read(unknown).unwrap(), b"unknown restore facts");
    }

    #[test]
    fn original_root_loss_refuses_and_leaves_replacement_empty() {
        let parent = tempfile::tempdir().unwrap();
        let journal = parent.path().join("journal");
        std::fs::create_dir(&journal).unwrap();
        let root = Arc::new(JournalRoot::open(&journal).unwrap());
        std::fs::rename(&journal, parent.path().join("original")).unwrap();
        std::fs::create_dir(&journal).unwrap();
        assert_eq!(require_ordinary_start(&root).unwrap_err(), REQUIRED);
        assert_eq!(std::fs::read_dir(&journal).unwrap().count(), 0);
    }

    #[test]
    fn creator_stages_refuse_before_actual_mobile_startup_effects() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        let empty = root.join(".tmp-offload-00000000000000000000000000000001");
        let copied = root.join(".tmp-offload-00000000000000000000000000000002");
        std::fs::create_dir(&empty).unwrap();
        std::fs::create_dir(&copied).unwrap();
        std::fs::write(copied.join("imported_audio.wav"), b"original duplicate").unwrap();
        let before_empty = std::os::unix::fs::MetadataExt::ino(&empty.metadata().unwrap());
        let before_copied = std::os::unix::fs::MetadataExt::ino(&copied.metadata().unwrap());
        assert!(!root.join("config/journal.json").exists());
        let started = crate::MobileServing::start_with_payload(
            root.to_owned(),
            address(),
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../core/payload"),
        );
        let (admitted, error) = match started {
            Ok(server) => {
                server.shutdown_for_retirement().unwrap();
                drop(server);
                (true, None)
            }
            Err(error) => (false, Some(error)),
        };
        eprintln!(
            "creator_startup_frontier admitted={admitted} config_published={}",
            root.join("config/journal.json").exists()
        );
        assert!(
            !admitted,
            "ordinary startup crossed creator recovery frontier"
        );
        assert_eq!(error.as_deref(), Some(REQUIRED));
        assert!(!root.join("config/journal.json").exists());
        assert!(!root.join("health").exists());
        assert_eq!(
            std::os::unix::fs::MetadataExt::ino(&empty.metadata().unwrap()),
            before_empty
        );
        assert_eq!(
            std::os::unix::fs::MetadataExt::ino(&copied.metadata().unwrap()),
            before_copied
        );
        assert_eq!(
            std::fs::read(copied.join("imported_audio.wav")).unwrap(),
            b"original duplicate"
        );
    }
    #[test]
    fn pending_restore_with_creator_stage_keeps_recovery_reachable_and_work_closed() {
        use solstone_core_journal_io::whole_journal_restore::{
            WholeJournalRestoreSource, begin_target_use_bound, publish_whole_journal_restore,
        };
        let directory = tempfile::tempdir().unwrap();
        let record = publish_whole_journal_restore(
            directory.path(),
            &WholeJournalRestoreSource::Byo {
                repository: "fixture".into(),
                backend: "local".into(),
            },
            "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef",
            "/journal",
        )
        .unwrap();
        let stage = directory
            .path()
            .join(".tmp-offload-00000000000000000000000000000001");
        std::fs::create_dir(&stage).unwrap();
        let server = start(directory.path()).unwrap();
        assert_eq!(
            server
                .request_owner("GET", "/init/api/recovery", "")
                .unwrap()
                .status,
            200
        );
        assert!(server.index().is_err());
        assert!(begin_target_use_bound(server.root.clone()).is_err());
        assert!(!directory.path().join("config/journal.json").exists());
        assert!(!solstone_core_indexer_store::db::db_path(directory.path()).exists());
        assert!(
            matches!(solstone_core_journal_io::whole_journal_restore::read_whole_journal_restore(directory.path()),
            solstone_core_journal_io::whole_journal_restore::WholeJournalRestoreReadiness::Pending(ref current) if **current == record)
        );
        assert!(stage.is_dir());
        server.shutdown_for_retirement().unwrap();
    }
}
