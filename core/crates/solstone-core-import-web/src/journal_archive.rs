// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Owner-facing portable journal archive export and preview routes.

use std::path::{Path, PathBuf};

use axum::{
    body::Body,
    extract::{Json, State},
    http::{StatusCode, header},
    response::Response,
};
use chrono::{SecondsFormat, Utc};
use serde_json::Value;
use solstone_core_import_sources::archive::plan_journal_archive;
use solstone_core_journal_archive::{
    ArchiveSource, EncodeArchiveRequest, ExplicitArchiveOutputRequest,
    acquire_explicit_output_target, publish_archive,
};
use tokio_util::io::ReaderStream;

use crate::{
    AppState,
    http::{error, import_not_found, json as json_response},
    lifecycle::contained_import_file,
};

const EXPORT_FILENAME: &str = "journal-export.zip";

/// Stream a portable archive without leaving an export artifact in the journal.
pub(crate) async fn export(State(state): State<AppState>) -> Response {
    match export_file(&state.root).await {
        Ok(file) => Response::builder()
            .header(header::CONTENT_TYPE, "application/zip")
            .header(
                header::CONTENT_DISPOSITION,
                format!("attachment; filename=\"{EXPORT_FILENAME}\""),
            )
            .body(Body::from_stream(ReaderStream::new(file)))
            .expect("archive export response"),
        Err(detail) => error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "I couldn't export this journal.",
            "journal_archive_export_failed",
            detail,
        ),
    }
}

/// Return a read-only plan for a staged portable journal archive.
pub(crate) async fn preview(State(state): State<AppState>, Json(data): Json<Value>) -> Response {
    let archive_path = match staged_archive_path(&state.root, &data) {
        Ok(path) => path,
        Err(response) => return *response,
    };
    match plan_journal_archive(&archive_path) {
        Ok(plan) => json_response(
            StatusCode::OK,
            serde_json::to_value(plan).expect("plan JSON"),
        ),
        Err(archive_error) => error(
            StatusCode::BAD_REQUEST,
            "I couldn't preview that archive.",
            "journal_archive_preview_failed",
            archive_error.to_string(),
        ),
    }
}

async fn export_file(root: &Path) -> Result<tokio::fs::File, String> {
    let source = ArchiveSource::open(root).map_err(|error| error.to_string())?;
    let scratch = tempfile::tempdir().map_err(|error| error.to_string())?;
    let output = scratch.path().join(EXPORT_FILENAME);
    let target = acquire_explicit_output_target(&ExplicitArchiveOutputRequest::new(
        output,
        scratch.path().to_owned(),
    ))
    .map_err(|error| error.to_string())?;
    let exported_at = Utc::now().to_rfc3339_opts(SecondsFormat::Secs, true);
    let request = EncodeArchiveRequest {
        source: &source,
        solstone_version: env!("CARGO_PKG_VERSION"),
        exported_at: &exported_at,
        day_window: None,
    };
    publish_archive(&target, &request).map_err(|error| error.to_string())?;
    let file = tokio::fs::File::open(target.final_path())
        .await
        .map_err(|error| error.to_string())?;
    drop(scratch);
    Ok(file)
}

fn staged_archive_path(root: &Path, data: &Value) -> Result<PathBuf, Box<Response>> {
    let Some(path) = data
        .get("path")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
    else {
        return Err(Box::new(invalid_preview_request("missing path")));
    };
    let Some(timestamp) = data
        .get("timestamp")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
    else {
        return Err(Box::new(invalid_preview_request("missing timestamp")));
    };
    let file_path = Path::new(path);
    let original_timestamp = if !file_path.starts_with(root.join("imports")) {
        timestamp
    } else {
        file_path
            .parent()
            .and_then(|value| value.file_name())
            .and_then(|value| value.to_str())
            .unwrap_or(timestamp)
    };
    if contained_import_file(root, original_timestamp, "import.json").is_err() {
        return Err(Box::new(import_not_found(&format!(
            "Import metadata not found for {original_timestamp}"
        ))));
    }
    let Some(filename) = file_path.file_name().and_then(|value| value.to_str()) else {
        return Err(Box::new(invalid_preview_request(
            "archive path has no file name",
        )));
    };
    contained_import_file(root, original_timestamp, filename)
        .map_err(|error| Box::new(invalid_preview_request(&error.to_string())))
}

fn invalid_preview_request(detail: &str) -> Response {
    error(
        StatusCode::BAD_REQUEST,
        "I couldn't use one of those values.",
        "invalid_request_value",
        detail.to_owned(),
    )
}

#[cfg(test)]
mod tests {
    use std::{
        collections::{BTreeMap, BTreeSet},
        fs,
        io::{Cursor, Write},
        path::Path,
    };

    use axum::http::StatusCode;
    use serde_json::json;
    use solstone_core_import_sources::archive::{ArchiveMergeOptions, merge_journal_archive};
    use solstone_core_journal_archive::{
        ArchiveSource, EncodeArchiveRequest, ExplicitArchiveOutputRequest,
        acquire_explicit_output_target, publish_archive,
    };
    use zip::{ZipArchive, ZipWriter, write::SimpleFileOptions};

    use super::*;
    use crate::{
        corpus::tests::{request, response_header},
        test_support::{phase_root, seed_import},
    };

    #[tokio::test]
    async fn ac18_export_streams_the_public_archive_contract_without_journal_residue() {
        let workspace = tempfile::tempdir().expect("workspace");
        let root = workspace.path().join("journal");
        fs::create_dir(&root).expect("journal root");
        establish(&root);
        fs::create_dir_all(root.join("chronicle/20260811/audio/120000_30")).expect("chronicle");
        fs::write(
            root.join("chronicle/20260811/audio/120000_30/stream.json"),
            b"stream",
        )
        .expect("stream");
        fs::create_dir_all(root.join("entities/owner")).expect("entity");
        fs::write(
            root.join("entities/owner/entity.json"),
            b"{\"id\":\"owner\"}",
        )
        .expect("entity file");
        fs::create_dir_all(root.join("facets/work")).expect("facet");
        fs::write(root.join("facets/work/facet.json"), b"{\"name\":\"work\"}").expect("facet file");
        fs::create_dir_all(root.join("notes")).expect("extra root");
        fs::write(root.join("notes/owner-note.txt"), b"keep me").expect("extra root file");

        let expected = workspace.path().join("expected.zip");
        publish_public_archive(&root, &expected);
        let expected_members = zip_members(&fs::read(&expected).expect("expected archive"));

        let (status, content_type, _, body) =
            request(&root, "GET", "/app/import/api/journal-archive/export", None).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(content_type, "application/zip");
        assert_eq!(
            response_header(
                &root,
                "GET",
                "/app/import/api/journal-archive/export",
                None,
                "content-disposition",
            )
            .await
            .as_deref(),
            Some("attachment; filename=\"journal-export.zip\"")
        );
        let members = zip_members(&body);
        assert_eq!(members, expected_members);
        assert!(!members.iter().any(|member| member.starts_with("config/")));
        assert!(members.contains("notes/owner-note.txt"));
        assert_no_zip_residue(&root);
    }

    #[tokio::test]
    async fn ac19_preview_reports_only_the_staged_days_and_never_merges() {
        let root = phase_root("empty");
        fs::create_dir_all(root.path().join("chronicle/20260812/audio/120000_30"))
            .expect("existing day");
        fs::write(
            root.path()
                .join("chronicle/20260812/audio/120000_30/stream.json"),
            b"existing",
        )
        .expect("existing stream");
        let before_upload = snapshot(root.path());

        let archive = root.path().join("day-d.zip");
        write_zip(
            &archive,
            &[("chronicle/20260810/audio/120000_30/stream.json", b"day d")],
        );
        let timestamp = "20260813_120000";
        seed_import(
            root.path(),
            timestamp,
            "day-d.zip",
            "application/zip",
            "journal-archive-preview",
            None,
            &fs::read(&archive).expect("archive bytes"),
        );
        fs::remove_file(&archive).expect("outside staging archive");
        let after_upload = snapshot(root.path());
        let additions = after_upload
            .keys()
            .filter(|path| !before_upload.contains_key(*path))
            .collect::<BTreeSet<_>>();
        assert!(
            additions
                .iter()
                .all(|path| path.starts_with(&format!("imports/{timestamp}/"))),
            "unexpected upload additions: {additions:?}"
        );

        let staged = root
            .path()
            .join("imports")
            .join(timestamp)
            .join("day-d.zip");
        let (status, _, _, body) = request(
            root.path(),
            "POST",
            "/app/import/api/journal-archive/preview",
            Some(&json!({"path": staged, "timestamp": timestamp})),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        let plan: Value = serde_json::from_slice(&body).expect("preview JSON");
        assert_eq!(plan["days"], json!(["20260810"]));
        assert_ne!(plan["days"], json!(["20260812"]));
        assert_eq!(snapshot(root.path()), after_upload);

        let merge_target = root.path().join("merge-target");
        fs::create_dir(&merge_target).expect("merge target");
        let options = ArchiveMergeOptions {
            working_root: root.path().join("merge-work"),
            ..ArchiveMergeOptions::default()
        };
        let result = merge_journal_archive(&staged, &merge_target, &options, None)
            .expect("same archive merges");
        assert!(result.merge_summary.segments_copied > 0);
        assert!(
            merge_target
                .join("chronicle/20260810/audio/120000_30/stream.json")
                .exists()
        );
    }

    #[tokio::test]
    async fn export_is_session_gated_in_unestablished_and_corrupt_phases() {
        for phase in ["unestablished", "corrupt"] {
            let root = phase_root(phase);
            let (status, content_type, location, _) = request(
                root.path(),
                "GET",
                "/app/import/api/journal-archive/export",
                None,
            )
            .await;
            if phase == "unestablished" {
                assert_eq!(
                    (status, location.as_deref()),
                    (StatusCode::FOUND, Some("/init"))
                );
            } else {
                assert_eq!(
                    (status, content_type),
                    (
                        StatusCode::INTERNAL_SERVER_ERROR,
                        "application/json".to_owned()
                    )
                );
            }
        }
    }

    #[test]
    fn journal_archive_guided_flow_keeps_the_apply_control() {
        let workspace = include_str!("../assets/workspace.html");
        let branch = workspace
            .split("${source.name === 'journal_archive' ? `")
            .nth(1)
            .and_then(|tail| tail.split("` : `").next())
            .expect("journal archive guided branch");

        assert!(branch.contains("id=\"guidedStartBtn\""));
        assert!(branch.contains("start import"));
        assert!(branch.contains("id=\"guidedPreviewBtn\""));
        assert!(workspace.contains(
            "guidedStartBtn.addEventListener('click', () => startGuidedImport(source));"
        ));
        assert!(workspace.contains("fetch('/app/import/api/start'"));
    }

    fn assert_no_facet_control(markup: &str) {
        assert!(!markup.contains("<select"));
        assert!(!markup.contains("for=\"facet"));
        assert!(!markup.contains("for='facet"));

        for attribute in ["id=\"", "id='", "name=\"", "name='"] {
            let mut remaining = markup;
            while let Some((_, tail)) = remaining.split_once(attribute) {
                let quote = attribute.as_bytes()[attribute.len() - 1] as char;
                let (value, tail) = tail.split_once(quote).expect("attribute value closes");
                assert!(
                    !value.to_ascii_lowercase().contains("facet"),
                    "{attribute}{value} must not name a facet control"
                );
                remaining = tail;
            }
        }
    }

    #[test]
    fn import_metadata_forms_keep_setting_without_facet_controls() {
        let workspace = include_str!("../assets/workspace.html");
        let guided = workspace
            .split("${source.name === 'journal_archive' ? `")
            .nth(1)
            .and_then(|tail| tail.split_once("` : `").map(|(_, branch)| branch))
            .and_then(|branch| branch.split_once("\n  `;").map(|(markup, _)| markup))
            .expect("guided import metadata branch");
        let quick = workspace
            .split_once("function renderQuickImportFlow() {")
            .and_then(|(_, tail)| {
                tail.split_once("function setupGuidedUploadArea() {")
                    .map(|(body, _)| body)
            })
            .expect("quick import flow");
        let quick_submit = workspace
            .split_once("function setupQuickImportForm() {")
            .and_then(|(_, tail)| {
                tail.split_once("async function uploadGuidedSourceFile(source) {")
                    .map(|(body, _)| body)
            })
            .expect("quick import submit flow");
        let confirm = workspace
            .split_once("<div id=\"detectModal\"")
            .and_then(|(_, tail)| tail.split_once("<script>").map(|(markup, _)| markup))
            .expect("detect modal");

        for markup in [guided, quick, confirm] {
            assert_no_facet_control(markup);
        }

        assert!(quick.contains("id=\"quickSettingInput\""));
        assert!(quick_submit.contains("fd.append('setting', settingValue)"));
        assert!(guided.contains("id=\"guidedSettingInput\""));
        assert!(workspace.contains("fd.append('setting', guidedSettingInput.value.trim())"));
        assert!(confirm.contains("id=\"settingInput\""));
        assert_eq!(
            workspace
                .matches("body: JSON.stringify({ path, setting })")
                .count(),
            2
        );
        assert!(workspace.contains("settingInput.value = res.setting || '';"));
    }

    #[test]
    fn import_history_has_only_source_filtering_and_detail_hides_legacy_facets() {
        let workspace = include_str!("../assets/workspace.html");
        let detail = include_str!("../assets/import_detail.js");

        assert!(!workspace.contains("data-facet="));
        assert!(workspace.contains("function filterImportsBySource()"));
        assert!(workspace.contains("row.dataset.sourceType === currentSourceFilter"));
        assert!(workspace.contains("no-imports-filtered"));
        assert!(!detail.contains("importJson.facet"));
        assert!(!detail.contains("kvRow(strings.facet,"));
        assert!(detail.contains("facets_created"));
        assert!(detail.contains("facets_merged"));
    }

    fn establish(root: &Path) {
        fs::create_dir_all(root.join("config")).expect("config directory");
        fs::write(
            root.join("config/journal.json"),
            b"{\"setup\":{\"completed_at\":1767225600}}\n",
        )
        .expect("config");
    }

    fn publish_public_archive(root: &Path, output: &Path) {
        let source = ArchiveSource::open(root).expect("archive source");
        let target = acquire_explicit_output_target(&ExplicitArchiveOutputRequest::new(
            output.to_path_buf(),
            output.parent().expect("output parent").to_path_buf(),
        ))
        .expect("archive target");
        let exported_at = "2026-08-20T00:00:00Z";
        publish_archive(
            &target,
            &EncodeArchiveRequest {
                source: &source,
                solstone_version: env!("CARGO_PKG_VERSION"),
                exported_at,
                day_window: None,
            },
        )
        .expect("public archive");
    }

    fn zip_members(bytes: &[u8]) -> BTreeSet<String> {
        let mut archive = ZipArchive::new(Cursor::new(bytes)).expect("zip archive");
        (0..archive.len())
            .map(|index| {
                archive
                    .by_index(index)
                    .expect("zip member")
                    .name()
                    .to_owned()
            })
            .collect()
    }

    fn assert_no_zip_residue(root: &Path) {
        for path in [
            root.to_path_buf(),
            root.join("chronicle"),
            root.join("entities"),
            root.join("facets"),
            root.join("imports"),
        ] {
            if !path.exists() {
                continue;
            }
            assert!(
                !walk_files(&path)
                    .iter()
                    .any(|file| file.extension().is_some_and(|extension| extension == "zip")),
                "zip residue below {}",
                path.display()
            );
        }
    }

    fn walk_files(path: &Path) -> Vec<PathBuf> {
        let mut files = Vec::new();
        for entry in fs::read_dir(path).expect("read directory") {
            let entry = entry.expect("directory entry");
            let file_type = entry.file_type().expect("file type");
            if file_type.is_dir() {
                files.extend(walk_files(&entry.path()));
            } else if file_type.is_file() {
                files.push(entry.path());
            }
        }
        files
    }

    fn write_zip(path: &Path, members: &[(&str, &[u8])]) {
        let mut writer = ZipWriter::new(fs::File::create(path).expect("zip output"));
        for (name, bytes) in members {
            writer
                .start_file(name, SimpleFileOptions::default())
                .expect("zip member");
            writer.write_all(bytes).expect("zip bytes");
        }
        writer.finish().expect("finish zip");
    }

    fn snapshot(root: &Path) -> BTreeMap<String, Vec<u8>> {
        let mut snapshot = BTreeMap::new();
        fn visit(root: &Path, path: &Path, snapshot: &mut BTreeMap<String, Vec<u8>>) {
            for entry in fs::read_dir(path).expect("read directory") {
                let entry = entry.expect("directory entry");
                let path = entry.path();
                if entry.file_type().expect("file type").is_dir() {
                    visit(root, &path, snapshot);
                } else {
                    snapshot.insert(
                        path.strip_prefix(root)
                            .expect("under root")
                            .to_string_lossy()
                            .to_string(),
                        fs::read(path).expect("file bytes"),
                    );
                }
            }
        }
        visit(root, root, &mut snapshot);
        snapshot
    }
}
