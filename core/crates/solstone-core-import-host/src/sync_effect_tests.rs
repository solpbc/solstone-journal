// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Auxiliary saves use the production audio pipeline; only Plaud's remote I/O is synthetic.

use super::*;
use sha2::{Digest, Sha256};
use std::os::unix::fs::MetadataExt;
use std::sync::{Arc, mpsc};

struct Bus {
    stop: Option<tokio::sync::oneshot::Sender<()>>,
    worker: Option<std::thread::JoinHandle<()>>,
}

impl Bus {
    fn start(root: &Path) -> Self {
        fs::create_dir_all(root.join("health")).unwrap();
        let socket = root.join("health/callosum.sock");
        let (ready, wait_ready) = mpsc::sync_channel(1);
        let (stop, stopped) = tokio::sync::oneshot::channel();
        let worker = std::thread::spawn(move || {
            audio_import_runtime().unwrap().block_on(async {
                let server = solstone_core_callosum::CallosumSocketServer::bind(socket)
                    .await
                    .unwrap();
                ready.send(()).unwrap();
                let _ = stopped.await;
                server.stop().await;
            });
        });
        wait_ready.recv_timeout(Duration::from_secs(10)).unwrap();
        Self {
            stop: Some(stop),
            worker: Some(worker),
        }
    }
}

impl Drop for Bus {
    fn drop(&mut self) {
        let _ = self.stop.take().unwrap().send(());
        self.worker.take().unwrap().join().unwrap();
    }
}

fn wave() -> Vec<u8> {
    let samples = 45 * 16_000u32;
    let size = samples * 2;
    let mut bytes = Vec::new();
    bytes.extend_from_slice(b"RIFF");
    bytes.extend_from_slice(&(36 + size).to_le_bytes());
    bytes.extend_from_slice(b"WAVEfmt ");
    bytes.extend_from_slice(&16u32.to_le_bytes());
    bytes.extend_from_slice(&1u16.to_le_bytes());
    bytes.extend_from_slice(&1u16.to_le_bytes());
    bytes.extend_from_slice(&16_000u32.to_le_bytes());
    bytes.extend_from_slice(&32_000u32.to_le_bytes());
    bytes.extend_from_slice(&2u16.to_le_bytes());
    bytes.extend_from_slice(&16u16.to_le_bytes());
    bytes.extend_from_slice(b"data");
    bytes.extend_from_slice(&size.to_le_bytes());
    for sample in 0..samples {
        let value =
            ((f64::from(sample) * 440.0 * std::f64::consts::TAU / 16_000.0).sin() * 8000.0) as i16;
        bytes.extend_from_slice(&value.to_le_bytes());
    }
    bytes
}

fn journal() -> tempfile::TempDir {
    let root = tempfile::tempdir().unwrap();
    fs::create_dir(root.path().join("config")).unwrap();
    fs::write(root.path().join("config/journal.json"), br#"{"identity":{"timezone":"UTC"},"env":{"PLAUD_ACCESS_TOKEN":"synthetic-token-not-for-state"}}"#).unwrap();
    root
}

fn audio_records(root: &Path) -> Vec<crate::audio::AudioImportRecord> {
    fs::read_dir(root.join("imports"))
        .unwrap()
        .filter_map(|entry| {
            let path = entry.unwrap().path().join("audio-import.json");
            path.is_file()
                .then(|| serde_json::from_slice(&fs::read(path).unwrap()).unwrap())
        })
        .collect()
}

fn assert_saved_audio(root: &Path, original: &[u8]) -> (PathBuf, u64) {
    let records = audio_records(root);
    assert_eq!(records.len(), 1, "one actual ordinary audio import");
    let record = &records[0];
    assert_eq!(fs::read(&record.source_media).unwrap(), original);
    assert_eq!(record.created_segments.len(), 1);
    assert!(record.dropped_chunks.is_empty());
    assert!(record.abort.is_none());
    assert_eq!(
        record.wait,
        crate::audio::AudioWaitRecord::NotRequested,
        "sync saves then leaves processing to the shared journal"
    );
    let part = &record.created_segments[0];
    assert_eq!(part.stream, "import.audio");
    assert!((part.duration_seconds - 45.0).abs() < 0.01);
    assert_eq!(part.start_offset_seconds, 0.0);
    let path = &part.file_path;
    let bytes = fs::read(path).unwrap();
    assert!(!bytes.is_empty());
    let bound = solstone_core_journal_io::JournalRoot::open(root).unwrap();
    let (size, digest) = crate::audio_custody::read(&bound, path)
        .unwrap()
        .expect("real producer custody");
    assert_eq!(size, bytes.len() as u64);
    assert_eq!(digest, format!("{:x}", Sha256::digest(bytes)));
    (path.clone(), fs::metadata(path).unwrap().ino())
}

struct UnusedText;
impl solstone_core_import::text::WireClient for UnusedText {
    fn execute(
        &self,
        _: &solstone_core_generate::GenerateRequest,
    ) -> Result<solstone_core_generate::GenerateResponse, solstone_core_generate::ClientError> {
        panic!("saving auxiliary audio must not introduce another Generate executor")
    }
}

#[test]
fn audio_sync_save_uses_real_pipeline_and_reuses_its_import_checkpoint() {
    let root = journal();
    let _bus = Bus::start(root.path());
    let source = tempfile::tempdir().unwrap();
    let original = wave();
    let path = source.path().join("meeting.wav");
    fs::write(&path, &original).unwrap();
    let args = vec![
        "--sync".into(),
        "audio".into(),
        "--path".into(),
        source.path().display().to_string(),
        "--save".into(),
    ];
    let mut first = None;
    for _ in 0..2 {
        let seams = crate::call::ImportSeams {
            text: Box::new(UnusedText),
            image: Box::new(solstone_core_import_sources::NullWireClient),
            model: Box::new(solstone_core_import_sources::NullDocumentModelClient),
            pdf: Box::new(solstone_core_import_sources::NullPdfWorker),
            reindex: Box::new(crate::registry::IndexScan {
                journal: root.path().to_path_buf(),
            }),
        };
        let handle = crate::call::start(
            args.clone(),
            root.path().to_path_buf(),
            seams,
            solstone_core_import::WorkStop::new(),
        )
        .unwrap();
        let (run, _) = handle.join();
        assert_eq!(run.exit_code, 0, "{}{}", run.stdout, run.stderr);
        let saved = assert_saved_audio(root.path(), &original);
        if let Some(first) = &first {
            assert_eq!(&saved, first, "repeat keeps the same published file");
        }
        first = Some(saved);
        assert_eq!(fs::read(&path).unwrap(), original);
    }
    let state = fs::read_to_string(root.path().join("imports/audio.json")).unwrap();
    assert!(state.contains("\"imported\""), "{state}");
}

struct Remote {
    bytes: Arc<Vec<u8>>,
    downloads: usize,
}
impl PlaudCatalogue for Remote {
    fn list_files(
        &mut self,
        _: &str,
    ) -> Result<Vec<solstone_core_import::sync_plaud::PlaudFile>, PlaudFailureKind> {
        Ok(vec![solstone_core_import::sync_plaud::PlaudFile {
            id: "synthetic-1".into(),
            filename: "meeting".into(),
            fullname: "meeting.wav".into(),
            filesize: self.bytes.len() as u64,
            start_time: 1_760_432_400.0,
            duration: 45_000.0,
            is_trash: false,
        }])
    }
}
impl PlaudDownload for Remote {
    fn temporary_url(&mut self, _: &str, _: &str) -> Result<String, PlaudFailureKind> {
        Ok("synthetic".into())
    }
    fn download(&mut self, _: &str, destination: &Path) -> Result<(), PlaudFailureKind> {
        self.downloads += 1;
        fs::write(destination, self.bytes.as_slice()).map_err(|_| PlaudFailureKind::Download)
    }
}

#[test]
fn plaud_sync_save_uses_real_pipeline_and_keeps_one_stored_audio_import() {
    let root = journal();
    let _bus = Bus::start(root.path());
    let original = Arc::new(wave());
    let mut catalogue = Remote {
        bytes: original.clone(),
        downloads: 0,
    };
    let mut download = Remote {
        bytes: original.clone(),
        downloads: 0,
    };
    let mut pipeline = GenericAudioPipeline {
        journal_path: root.path(),
    };
    let mut first = None;
    for _ in 0..2 {
        let run = run_plaud_with(
            root.path(),
            true,
            &mut catalogue,
            &mut download,
            &mut pipeline,
        );
        assert_eq!(run.exit_code, 0, "{}{}", run.stdout, run.stderr);
        let saved = assert_saved_audio(root.path(), &original);
        if let Some(first) = &first {
            assert_eq!(&saved, first, "repeat keeps the same published file");
        }
        first = Some(saved);
    }
    assert_eq!(download.downloads, 1);
    let state = fs::read_to_string(root.path().join("imports/plaud.json")).unwrap();
    assert!(state.contains("\"imported\""), "{state}");
    assert!(!state.contains("synthetic-token-not-for-state"));
}
