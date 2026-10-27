// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use super::*;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

#[test]
#[ignore = "requires retained Opus speech fixture and pinned native model/assets; explicit qualification only"]
fn ordinary_archive_uses_shared_speech_and_has_owner_readable_output() {
    if in_isolated_child_exact(
        "import_route::tests::model_effects::archive_model_effects::ordinary_archive_uses_shared_speech_and_has_owner_readable_output",
    ) {
        return;
    }
    let _serial = crate::foreground::TEST_LOCK
        .lock()
        .unwrap_or_else(|p| p.into_inner());
    let (root, server, host, generator) = model_fixture();
    fs::write(root.path().join("config/journal.json"),serde_json::to_vec(&json!({"setup":{"completed_at":1},"identity":{"timezone":"UTC"},"processing":{"history_confirmed":true},"transcribe":{"backend":"parakeet","confidential_audio":false,"preserve_all":true}})).unwrap()).unwrap();
    let bytes = fs::read(
        std::env::var("FC2_MEDIA_ARCHIVE_SPEECH_FIXTURE").expect("retained Opus fixture required"),
    )
    .unwrap();
    assert_eq!(
        format!("{:x}", Sha256::digest(&bytes)),
        "1069a86b98d375ce8154be9698733cd115480456b38c98247b60632333d16131"
    );
    host.enable_audio(
        std::path::PathBuf::from(
            std::env::var("FC2_MEDIA_MODELS").expect("retained assets required"),
        ),
        std::path::PathBuf::from(
            std::env::var("FC2_MEDIA_PARAKEET_MODEL").expect("retained pinned model required"),
        ),
        root.path().join("native-scratch"),
    )
    .unwrap();
    let source = tempfile::tempdir().unwrap();
    let name = "limitless_pendant_2026-10-14T09-00-00_to_2026-10-14T09-01-00.opus";
    fs::write(source.path().join(name), &bytes).unwrap();
    let stamp = solstone_core_import::validate_timestamp("20261014_090000").unwrap();
    let folder = solstone_core_import::admit_folder_bound(
        &server.root,
        source.path(),
        "ordinary-archive-speech",
        &stamp,
    )
    .unwrap();
    let saved = solstone_core_import_web::register_admitted_folder(
        &server.root,
        &folder,
        "audio_archive",
        "picked speech archive",
        "ordinary-archive-speech",
        &stamp,
        chrono_tz::UTC,
    )
    .unwrap();
    let mut request = json!({"path":saved["path"],"timestamp":saved["source_timestamp"],"import_id":folder.import_id.as_str(),"source_zone":"UTC"});
    let preview = owner_json(&server, "POST", "/app/import/api/preview", request.clone());
    request["preview_token"] = preview["preview_token"].clone();
    let native = observe_native_result(&server, &host);
    owner_json(&server, "POST", "/app/import/api/start", request.clone());
    let (code, stdout, stderr) = native
        .recv_timeout(Duration::from_secs(10))
        .unwrap()
        .unwrap();
    assert_eq!(code, 0, "{stdout}{stderr}");
    let completed: Value = serde_json::from_str(&stdout).unwrap();
    let id = completed["run_id"].as_str().unwrap();
    let archive = solstone_core_import::audio_archive::read_audio_archive(root.path(), id)
        .unwrap()
        .unwrap();
    assert_eq!(archive.parts.len(), 1);
    let part = &archive.parts[0];
    let input = root
        .path()
        .join("chronicle")
        .join(&part.day)
        .join(&archive.settings.stream)
        .join(&part.segment_key)
        .join(&part.stored_name);
    let before = owner_json(
        &server,
        "GET",
        &format!("/app/import/api/{id}"),
        Value::Null,
    );
    assert_eq!(before["saved_parts"], 1);
    assert_eq!(before["processed_parts"], 0);
    assert!(!input.with_extension("jsonl").exists());
    host.run_sense_pass().unwrap();
    let transcript = fs::read_to_string(input.with_extension("jsonl"))
        .unwrap_or_else(|error| panic!("{error}; audio status {}", host.audio_status()));
    assert!(
        transcript.contains("quick brown fox jumps over the lazy dog"),
        "{transcript}"
    );
    assert!(input.with_extension("npz").is_file());
    assert_eq!(fs::read(&input).unwrap(), bytes);
    assert_eq!(fs::read(source.path().join(name)).unwrap(), bytes);
    let read = owner_json(
        &server,
        "GET",
        &format!("/app/transcripts/api/read/{}?transcripts=1", part.day),
        Value::Null,
    );
    assert!(
        read["markdown"]
            .as_str()
            .unwrap()
            .contains("quick brown fox"),
        "{read}"
    );
    use solstone_core_format::content::{
        ContentResolution, RawPerceptFamily, resolve_content_shape,
    };
    let transcript_path = input.with_extension("jsonl");
    assert_eq!(
        resolve_content_shape(
            &transcript_path,
            transcript_path
                .strip_prefix(root.path().join("chronicle"))
                .unwrap()
                .to_str()
                .unwrap()
        ),
        ContentResolution::Unindexed(RawPerceptFamily::Audio)
    );
    assert!(
        generator.calls.lock().unwrap().is_empty(),
        "local speech must reuse its existing executor"
    );
    let replay = owner_json(&server, "POST", "/app/import/api/start", request);
    assert_eq!(replay["replay"], true);
    assert_eq!(replay["import_id"], id);
    eprintln!(
        "ordinary archive speech transcript: {transcript}; actual archive status: {}",
        owner_json(
            &server,
            "GET",
            &format!("/app/import/api/{id}"),
            Value::Null
        )
    );
    host.shutdown().unwrap();
    server.shutdown_for_retirement().unwrap();
}
