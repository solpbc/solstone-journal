// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use super::owner_effects::{observe_native_result, owner_json};
use super::*;
use serde_json::{Value, json};

#[test]
fn ordinary_archive_folder_previews_saves_and_replays_through_owner_routes() {
    if in_isolated_child_exact(
        "import_route::tests::archive_effects::ordinary_archive_folder_previews_saves_and_replays_through_owner_routes",
    ) {
        return;
    }
    let _serial = crate::foreground::TEST_LOCK
        .lock()
        .unwrap_or_else(|p| p.into_inner());
    let (root, server, host) = fixture(Arc::new(|context| {
        Ok(PreparedImport::new(
            context.arguments().to_vec(),
            context.seams_with_pdf(Box::new(solstone_core_import_sources::NullPdfWorker)),
        ))
    }));
    let native = observe_native_result(&server, &host);
    let source = tempfile::tempdir().unwrap();
    let filename = "limitless_pendant_2026-10-14T09-00-00_to_2026-10-14T09-01-00.opus";
    let bytes = include_bytes!("../../../core/fixtures/audio_decode_corpus/render.opus");
    fs::write(source.path().join(filename), bytes).unwrap();
    let stamp = solstone_core_import::validate_timestamp("20261014_090000").unwrap();
    let folder = solstone_core_import::admit_folder_bound(
        &server.root,
        source.path(),
        "ordinary-archive",
        &stamp,
    )
    .unwrap();
    let saved = solstone_core_import_web::register_admitted_folder(
        &server.root,
        &folder,
        "audio_archive",
        "picked archive",
        "ordinary-archive",
        &stamp,
        chrono_tz::UTC,
    )
    .unwrap();
    let id = folder.import_id.as_str();
    let mut request = json!({"path":saved["path"],"timestamp":saved["source_timestamp"],"import_id":id,"source_zone":"UTC"});
    let preview = owner_json(&server, "POST", "/app/import/api/preview", request.clone());
    assert_eq!(
        preview["size"]["schema"], "solstone.import.audio-archive.size.v1",
        "{preview}"
    );
    assert_eq!(preview["size"]["members"], 1);
    assert_eq!(preview["size"]["source_bytes"], bytes.len());
    assert_eq!(preview["size"]["days"], 1);
    assert!(
        !root.path().join("chronicle").exists(),
        "preview must not publish"
    );
    request["preview_token"] = preview["preview_token"].clone();
    owner_json(&server, "POST", "/app/import/api/start", request.clone());
    let (code, stdout, stderr) = native
        .recv_timeout(Duration::from_secs(10))
        .unwrap()
        .unwrap();
    assert_eq!(code, 0, "{stdout}{stderr}");
    let completed: Value = serde_json::from_str(&stdout).unwrap();
    let run_id = completed["run_id"].as_str().unwrap();
    assert_eq!(completed["parts_saved"], 1);
    let route_status = owner_json(
        &server,
        "GET",
        &format!("/app/import/api/{id}"),
        Value::Null,
    );
    assert_eq!(
        route_status["archive_run_id"], run_id,
        "the selected source must resolve to its actual indexed run: {route_status}"
    );
    let actual = owner_json(
        &server,
        "GET",
        &format!("/app/import/api/{run_id}"),
        Value::Null,
    );
    assert_eq!(actual["save_complete"], true);
    assert_eq!(
        actual["processed_parts"], 0,
        "saved audio is not processed audio"
    );
    assert_eq!(actual["processing"], 1);
    let record = solstone_core_import::audio_archive::read_audio_archive(root.path(), run_id)
        .unwrap()
        .unwrap();
    assert_eq!(record.parts.len(), 1);
    let part = &record.parts[0];
    let published = root
        .path()
        .join("chronicle")
        .join(&part.day)
        .join(&record.settings.stream)
        .join(&part.segment_key)
        .join(&part.stored_name);
    assert_eq!(fs::read(&published).unwrap(), bytes);
    #[cfg(feature = "audio")]
    {
        let admitted =
            crate::custody::AcceptedInput::capture_bound(&server.root, &published, &|| false)
                .unwrap()
                .expect("archive bytes need their producer size/SHA before shared audio admission");
        assert!(admitted.still_current(&published));
        assert!(admitted.receipt_matches(root.path(), &published).unwrap());
        let intent_path = root
            .path()
            .join("imports")
            .join(run_id)
            .join("tiles")
            .join(format!("{}_{}.intent.json", part.day, part.segment_key));
        let original_intent = fs::read(&intent_path).unwrap();
        fs::remove_file(&intent_path).unwrap();
        assert!(
            crate::custody::AcceptedInput::capture_bound(&server.root, &published, &|| false)
                .unwrap()
                .is_none(),
            "current bytes cannot replace a missing producer witness"
        );
        assert!(!admitted.receipt_matches(root.path(), &published).unwrap());
        let mut changed: Value = serde_json::from_slice(&original_intent).unwrap();
        changed["parts"][0]["sha256"] = json!("0".repeat(64));
        fs::write(&intent_path, serde_json::to_vec(&changed).unwrap()).unwrap();
        assert!(
            crate::custody::AcceptedInput::capture_bound(&server.root, &published, &|| false)
                .unwrap()
                .is_none(),
            "only the accepted SHA may authorize input"
        );
        assert!(!admitted.receipt_matches(root.path(), &published).unwrap());
        fs::write(&intent_path, &original_intent).unwrap();
        assert!(
            solstone_core_import::audio_archive::read_published_part_receipt_bound(
                &server.root,
                &published,
                &|| true
            )
            .is_err()
        );
        let parent = published.parent().unwrap();
        let displaced = parent.with_file_name("displaced-original-tile");
        let checkpoints = std::cell::Cell::new(0);
        let refused = solstone_core_import::audio_archive::read_published_part_receipt_bound(
            &server.root,
            &published,
            &|| {
                let count = checkpoints.get() + 1;
                checkpoints.set(count);
                if count == 2 {
                    fs::rename(parent, &displaced).unwrap();
                    fs::create_dir(parent).unwrap();
                    fs::write(parent.join("sentinel"), b"replacement stays untouched").unwrap();
                }
                false
            },
        );
        assert!(
            checkpoints.get() >= 2,
            "replacement control must run after original segment acquisition"
        );
        assert!(
            refused.is_err(),
            "the receipt reader adopted a displaced parent"
        );
        assert_eq!(
            fs::read(parent.join("sentinel")).unwrap(),
            b"replacement stays untouched"
        );
        assert_eq!(fs::read(displaced.join(&part.stored_name)).unwrap(), bytes);
        fs::remove_file(parent.join("sentinel")).unwrap();
        fs::remove_dir(parent).unwrap();
        fs::rename(&displaced, parent).unwrap();
        assert!(admitted.still_current(&published));
        assert!(admitted.receipt_matches(root.path(), &published).unwrap());
    }
    assert_eq!(fs::read(source.path().join(filename)).unwrap(), bytes);
    assert_eq!(
        fs::read(
            root.path()
                .join("imports")
                .join(id)
                .join("tree")
                .join(filename)
        )
        .unwrap(),
        bytes
    );
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    loop {
        let state = owner_json(
            &server,
            "GET",
            &format!("/app/import/api/{id}"),
            Value::Null,
        );
        match state["archive_active"].as_bool().unwrap() {
            false => break,
            true => {
                assert!(std::time::Instant::now() < deadline);
                std::thread::park_timeout(Duration::from_millis(10));
            }
        }
    }
    let replay = owner_json(&server, "POST", "/app/import/api/start", request);
    assert_eq!(replay["replay"], true);
    assert_eq!(replay["import_id"], run_id);
    assert!(
        matches!(native.try_recv(), Err(mpsc::TryRecvError::Empty)),
        "a saved archive replay must not invoke another native import"
    );
    assert_eq!(fs::read(&published).unwrap(), bytes);
    host.shutdown().unwrap();
    server.shutdown_for_retirement().unwrap();
}

#[test]
fn ordinary_archive_manifest_previews_and_saves_nested_members() {
    if in_isolated_child_exact(
        "import_route::tests::archive_effects::ordinary_archive_manifest_previews_and_saves_nested_members",
    ) {
        return;
    }
    let _serial = crate::foreground::TEST_LOCK
        .lock()
        .unwrap_or_else(|p| p.into_inner());
    let (root, server, host) = fixture(Arc::new(|context| {
        Ok(PreparedImport::new(
            context.arguments().to_vec(),
            context.seams_with_pdf(Box::new(solstone_core_import_sources::NullPdfWorker)),
        ))
    }));
    let native = observe_native_result(&server, &host);
    let source = tempfile::tempdir().unwrap();
    fs::create_dir(source.path().join("audio")).unwrap();
    let bytes = include_bytes!("../../../core/fixtures/audio_decode_corpus/render.opus");
    fs::write(source.path().join("audio/clip.opus"), bytes).unwrap();
    fs::write(
        source.path().join("members.json"),
        br#"{"members":[{"path":"audio/clip.opus","instant":"2026-10-14T09:00:00Z"}]}"#,
    )
    .unwrap();
    let stamp = solstone_core_import::validate_timestamp("20261014_090000").unwrap();
    let folder = solstone_core_import::admit_folder_bound(
        &server.root,
        source.path(),
        "manifest-archive",
        &stamp,
    )
    .unwrap();
    let saved = solstone_core_import_web::register_admitted_folder(
        &server.root,
        &folder,
        "audio_archive",
        "picked manifest archive",
        "manifest-archive",
        &stamp,
        chrono_tz::UTC,
    )
    .unwrap();
    let mut request = json!({"path":saved["path"],"timestamp":saved["source_timestamp"],"import_id":folder.import_id.as_str(),"archive_manifest":"members.json"});
    let preview = owner_json(&server, "POST", "/app/import/api/preview", request.clone());
    assert_eq!(preview["size"]["members"], 1, "{preview}");
    assert_eq!(preview["size"]["source_bytes"], bytes.len());
    assert!(!root.path().join("chronicle").exists());
    request["preview_token"] = preview["preview_token"].clone();
    owner_json(&server, "POST", "/app/import/api/start", request);
    let (code, stdout, stderr) = native
        .recv_timeout(Duration::from_secs(10))
        .unwrap()
        .unwrap();
    assert_eq!(code, 0, "{stdout}{stderr}");
    let completed: Value = serde_json::from_str(&stdout).unwrap();
    let record = solstone_core_import::audio_archive::read_audio_archive(
        root.path(),
        completed["run_id"].as_str().unwrap(),
    )
    .unwrap()
    .unwrap();
    assert_eq!(record.parts.len(), 1);
    let part = &record.parts[0];
    let published = root
        .path()
        .join("chronicle")
        .join(&part.day)
        .join(&record.settings.stream)
        .join(&part.segment_key)
        .join(&part.stored_name);
    assert_eq!(fs::read(published).unwrap(), bytes);
    assert_eq!(
        fs::read(source.path().join("audio/clip.opus")).unwrap(),
        bytes
    );
    host.shutdown().unwrap();
    server.shutdown_for_retirement().unwrap();
}

#[test]
fn ordinary_archive_resume_keeps_an_already_published_tile_and_its_original_identity() {
    if in_isolated_child_exact(
        "import_route::tests::archive_effects::ordinary_archive_resume_keeps_an_already_published_tile_and_its_original_identity",
    ) {
        return;
    }
    let _serial = crate::foreground::TEST_LOCK
        .lock()
        .unwrap_or_else(|p| p.into_inner());
    let (root, server, host) = fixture(Arc::new(|context| {
        Ok(PreparedImport::new(
            context.arguments().to_vec(),
            context.seams_with_pdf(Box::new(solstone_core_import_sources::NullPdfWorker)),
        ))
    }));
    let native = observe_native_result(&server, &host);
    let source = tempfile::tempdir().unwrap();
    let filename = "limitless_pendant_2026-10-14T09-00-00_to_2026-10-14T09-01-00.opus";
    let bytes = include_bytes!("../../../core/fixtures/audio_decode_corpus/render.opus");
    fs::write(source.path().join(filename), bytes).unwrap();
    let stamp = solstone_core_import::validate_timestamp("20261014_090000").unwrap();
    let folder = solstone_core_import::admit_folder_bound(
        &server.root,
        source.path(),
        "archive-resume",
        &stamp,
    )
    .unwrap();
    let saved = solstone_core_import_web::register_admitted_folder(
        &server.root,
        &folder,
        "audio_archive",
        "picked archive",
        "archive-resume",
        &stamp,
        chrono_tz::UTC,
    )
    .unwrap();
    let id = folder.import_id.as_str();
    let mut request = json!({"path":saved["path"],"timestamp":saved["source_timestamp"],"import_id":id,"source_zone":"UTC"});
    let preview = owner_json(&server, "POST", "/app/import/api/preview", request.clone());
    let metadata = solstone_core_import::read_import_metadata(root.path(), id).unwrap();
    let snapshot = root
        .path()
        .join("imports")
        .join(id)
        .join(metadata["preview"]["relative"].as_str().unwrap());
    // Seed the maintained interrupted state using its real producer/fault seam:
    // the tile was atomically published, but its saved mark was not committed.
    let interrupted = solstone_core_import_host::audio_archive::run_audio_archive_with(
        solstone_core_import_host::audio_archive::AudioArchiveDispatchOptions {
            path: snapshot,
            source_zone: Some("UTC".into()),
            expected_identity: Some(preview["size"]["archive_identity"].as_str().unwrap().into()),
            expected_owner_zone: Some("UTC".into()),
            json: true,
            ..Default::default()
        },
        root.path(),
        None,
        Some(solstone_core_import::audio_archive::ArchiveFault::AfterPublishBeforeSavedMark),
    );
    assert_ne!(interrupted.exit_code, 0);
    let run_id = solstone_core_import::audio_archive::read_matching_archive_run(
        root.path(),
        preview["size"]["archive_identity"].as_str().unwrap(),
    )
    .unwrap()
    .unwrap();
    let record = solstone_core_import::audio_archive::read_audio_archive(root.path(), &run_id)
        .unwrap()
        .unwrap();
    let part = &record.parts[0];
    let published = root
        .path()
        .join("chronicle")
        .join(&part.day)
        .join(&record.settings.stream)
        .join(&part.segment_key)
        .join(&part.stored_name);
    use std::os::unix::fs::MetadataExt;
    let original_inode = fs::metadata(&published).unwrap().ino();
    assert_eq!(fs::read(&published).unwrap(), bytes);
    let preview = owner_json(&server, "POST", "/app/import/api/preview", request.clone());
    assert_eq!(preview["size"]["save_complete"], false);
    request["preview_token"] = preview["preview_token"].clone();
    owner_json(&server, "POST", "/app/import/api/start", request);
    let (code, stdout, stderr) = native
        .recv_timeout(Duration::from_secs(10))
        .unwrap()
        .unwrap();
    assert_eq!(code, 0, "{stdout}{stderr}");
    let resumed: Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(resumed["run_id"], run_id);
    assert_eq!(resumed["save_complete"], true);
    assert_eq!(fs::metadata(&published).unwrap().ino(), original_inode);
    assert_eq!(fs::read(&published).unwrap(), bytes);
    assert_eq!(fs::read(source.path().join(filename)).unwrap(), bytes);
    let status = owner_json(
        &server,
        "GET",
        &format!("/app/import/api/{run_id}"),
        Value::Null,
    );
    assert_eq!(status["saved_parts"], 1);
    assert_eq!(status["processed_parts"], 0);
    host.shutdown().unwrap();
    server.shutdown_for_retirement().unwrap();
}
