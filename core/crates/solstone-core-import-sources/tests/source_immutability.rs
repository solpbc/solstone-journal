// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, UNIX_EPOCH};

use chrono::{DateTime, Local};
use image::{ImageBuffer, Rgb};
use solstone_core_generate::{
    ClientError, GenerateRequest, GenerateResponse, RefusalReason, RefusedResponse,
};
use solstone_core_import::observe_source_immutability;
use solstone_core_import_sources::MODULE_STUBS;
use solstone_core_import_sources::image::{
    DescriptionOutcome, ProgressUpdate, WireClient, import_image,
};

static NEXT_TREE: AtomicUsize = AtomicUsize::new(0);

struct SuccessWire;

impl WireClient for SuccessWire {
    fn execute(&self, _: &GenerateRequest) -> Result<GenerateResponse, ClientError> {
        Ok(GenerateResponse::Generated(Box::new(
            solstone_core_generate::GeneratedResponse {
                id: None,
                text: "A red square.".to_owned(),
                model: "test".to_owned(),
                usage: serde_json::json!({}),
                finish_reason: "stop".to_owned(),
                thinking: None,
                schema_validation: None,
                input_budget: None,
                request_budget: None,
                inference: None,
                hints_applied: Vec::new(),
            },
        )))
    }
}

struct NoEngineWire;

impl WireClient for NoEngineWire {
    fn execute(&self, _: &GenerateRequest) -> Result<GenerateResponse, ClientError> {
        Ok(GenerateResponse::Refused(RefusedResponse {
            id: None,
            reason: RefusalReason::NoEngineConfigured,
            reason_code: None,
            retryable: false,
            blocking: true,
            reset_at_ms: None,
            provider: None,
            detail: "no engine".to_owned(),
        }))
    }
}

struct FailingWire;

impl WireClient for FailingWire {
    fn execute(&self, _: &GenerateRequest) -> Result<GenerateResponse, ClientError> {
        Err(ClientError::Io("wire unavailable".to_owned()))
    }
}

#[test]
fn source_stubs_leave_the_owner_source_unchanged() {
    let tree = TempTree::new();
    fs::write(tree.path().join("source.txt"), b"source").unwrap();

    let report = observe_source_immutability(tree.path(), |_| {
        for (_, stub) in MODULE_STUBS {
            assert!(stub().is_err());
        }
    })
    .unwrap();

    assert!(!report.violated());
}

#[test]
fn image_import_preserves_source_and_records_its_segment_contract() {
    let tree = TempTree::new();
    let source_root = tree.path().join("source");
    let journal = tree.path().join("journal");
    fs::create_dir(&source_root).unwrap();
    let source = source_root.join("picture.PNG");
    ImageBuffer::<Rgb<u8>, _>::from_pixel(4, 4, Rgb([255, 0, 0]))
        .save(&source)
        .unwrap();
    fs::set_permissions(&source, fs::Permissions::from_mode(0o640)).unwrap();
    let source_mtime = UNIX_EPOCH + Duration::from_secs(1_768_977_600);
    let source_time: DateTime<Local> = source_mtime.into();
    let day = source_time.format("%Y%m%d").to_string();
    let segment_key = format!("{}_0", source_time.format("%H%M%S"));
    fs::File::open(&source)
        .unwrap()
        .set_times(fs::FileTimes::new().set_modified(source_mtime))
        .unwrap();
    let source_bytes = fs::read(&source).unwrap();
    let source_mode = fs::metadata(&source).unwrap().permissions().mode() & 0o777;
    let mut progress = Vec::new();

    let report = observe_source_immutability(&source_root, |_| {
        let result = import_image(
            &source,
            &journal,
            "import-1",
            Some(&mut |update: &ProgressUpdate| progress.push(update.clone())),
            &SuccessWire,
        )
        .unwrap();
        assert_eq!(result.created_segment.stream, "import.image");
        assert_eq!(result.created_segment.segment, segment_key);
        assert_eq!(result.days_affected, vec![day.clone()]);
        assert!(matches!(
            result.description,
            DescriptionOutcome::Generated(_)
        ));
        assert_eq!(result.files_created.len(), 1);
        let segment = journal
            .join("chronicle")
            .join(&day)
            .join("import.image")
            .join(&segment_key);
        let installed = segment.join("original.png");
        assert_eq!(fs::read(&installed).unwrap(), source_bytes);
        assert_eq!(
            fs::metadata(&installed).unwrap().permissions().mode() & 0o777,
            0o600
        );
        assert_eq!(
            fs::metadata(&installed).unwrap().modified().unwrap(),
            source_mtime
        );
        assert!(segment.join("image_transcript.md").is_file());
        let manifest: serde_json::Value = serde_json::from_slice(
            &fs::read(journal.join("imports/import-1/manifest.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(manifest["days_affected"], serde_json::json!([day.as_str()]));
    })
    .unwrap();

    assert!(!report.violated());
    assert_eq!(fs::read(&source).unwrap(), source_bytes);
    assert_eq!(
        fs::metadata(&source).unwrap().permissions().mode() & 0o777,
        source_mode
    );
    assert_eq!(
        fs::metadata(&source).unwrap().modified().unwrap(),
        source_mtime
    );
    assert_eq!(
        progress,
        [ProgressUpdate {
            current: 1,
            total: 1,
            earliest_date: day.clone(),
            latest_date: day,
            entities_found: 0,
        }]
    );
}

#[test]
fn unavailable_description_doors_preserve_the_installed_original() {
    for (label, wire) in [
        ("no-engine", &NoEngineWire as &dyn WireClient),
        ("wire-failure", &FailingWire as &dyn WireClient),
    ] {
        let tree = TempTree::new();
        let source = tree.path().join("picture.png");
        let journal = tree.path().join("journal");
        ImageBuffer::<Rgb<u8>, _>::from_pixel(4, 4, Rgb([255, 0, 0]))
            .save(&source)
            .unwrap();

        let result = import_image(&source, &journal, label, None, wire).unwrap();
        assert!(matches!(
            result.description,
            DescriptionOutcome::Unavailable { .. }
        ));
        let segment = journal.join(format!(
            "chronicle/{}/import.image/{}",
            result.created_segment.day, result.created_segment.segment
        ));
        assert_eq!(
            fs::read(segment.join("original.png")).unwrap(),
            fs::read(&source).unwrap()
        );
        assert!(
            fs::read_to_string(segment.join("image_transcript.md"))
                .unwrap()
                .contains("unavailable —")
        );
    }
}

struct TempTree {
    path: PathBuf,
}

impl TempTree {
    fn new() -> Self {
        let index = NEXT_TREE.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "import-sources-image-test-{}-{index}",
            std::process::id()
        ));
        fs::create_dir(&path).unwrap();
        Self { path }
    }

    fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for TempTree {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.path).unwrap();
    }
}
