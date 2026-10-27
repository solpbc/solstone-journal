// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use sha2::{Digest, Sha256};
use std::{
    fs::{self, File, Metadata},
    io::Read,
    path::Path,
};

pub(crate) const CUSTODY_CANCELLED: &str = "custody verification cancelled";

/// Publication before receipt persistence is not a completed network input.
/// Match the accepted custody receipt before giving these bytes to a processor.
#[cfg(feature = "audio")]
pub(crate) fn accepted_input(root: &Path, input: &Path) -> Result<bool, String> {
    Ok(AcceptedInput::capture(root, input, &|| false)?.is_some())
}

enum ReceiptFiles {
    Absent,
    Present(Vec<(u64, String)>),
}

fn accepted_receipt_files(root: &Path, input: &Path) -> Result<ReceiptFiles, String> {
    let segment = input.parent().ok_or("input segment missing")?;
    let relative = segment
        .strip_prefix(root.join("chronicle"))
        .map_err(|e| e.to_string())?;
    let parts = relative
        .iter()
        .map(|p| p.to_string_lossy())
        .collect::<Vec<_>>();
    if parts.len() != 3 {
        return Ok(ReceiptFiles::Absent);
    }
    let report =
        solstone_core_callosum::read_device_ingest_events(segment).map_err(|e| e.to_string())?;
    if report.unparseable != 0 {
        return Err(format!("unreadable custody receipt: {}", segment.display()));
    }
    let name = input
        .file_name()
        .ok_or("input filename missing")?
        .to_string_lossy();
    let files = report
        .records
        .iter()
        .filter(|r| {
            r.outcome == "accepted"
                && r.day == parts[0]
                && r.stream == parts[1]
                && r.segment == parts[2]
        })
        .flat_map(|r| &r.files)
        .filter(|f| f.written == name)
        .map(|f| (f.size, f.sha256.clone()))
        .collect::<Vec<_>>();
    if files.is_empty() {
        Ok(ReceiptFiles::Absent)
    } else {
        Ok(ReceiptFiles::Present(files))
    }
}

/// Retain the admitted inode and compare cheap file facts between work units.
/// Exact bytes are checked once here and again by the processor at publication.
pub(crate) struct AcceptedInput {
    _file: File,
    stamp: FileStamp,
    /// Host speech checkpoints compare this to the accepted receipt. The
    /// processing-only build has no host checkpoint and does not retain it.
    #[cfg(feature = "audio")]
    digest: String,
    #[cfg(all(unix, feature = "audio", feature = "processing"))]
    import_root: Option<std::sync::Arc<solstone_core_journal_io::JournalRoot>>,
}

#[derive(PartialEq, Eq)]
struct FileStamp {
    size: u64,
    modified: std::time::SystemTime,
    #[cfg(unix)]
    identity: (u64, u64, i64, i64),
}

fn stamp(metadata: &Metadata) -> Result<FileStamp, String> {
    if !metadata.is_file() {
        return Err("input is not a regular file".into());
    }
    Ok(FileStamp {
        size: metadata.len(),
        modified: metadata.modified().map_err(|e| e.to_string())?,
        #[cfg(unix)]
        identity: {
            use std::os::unix::fs::MetadataExt;
            (
                metadata.dev(),
                metadata.ino(),
                metadata.ctime(),
                metadata.ctime_nsec(),
            )
        },
    })
}

impl AcceptedInput {
    /// The retained admission size and digest still match an accepted receipt.
    /// This does not read or hash the recording.
    #[cfg(feature = "audio")]
    pub(crate) fn receipt_matches(&self, root: &Path, input: &Path) -> Result<bool, String> {
        #[cfg(all(unix, feature = "processing"))]
        if let Some(root) = &self.import_root {
            return Ok(solstone_core_import_host::audio_custody::read(root, input)?
                .is_some_and(|(size, digest)| size == self.stamp.size && digest == self.digest));
        }
        let ReceiptFiles::Present(files) = accepted_receipt_files(root, input)? else {
            return Ok(false);
        };
        Ok(files
            .iter()
            .any(|(size, digest)| *size == self.stamp.size && digest == &self.digest))
    }

    pub(crate) fn still_current(&self, input: &Path) -> bool {
        if !fs::symlink_metadata(input)
            .ok()
            .filter(|m| m.file_type().is_file())
            .and_then(|m| stamp(&m).ok())
            .is_some_and(|current| current == self.stamp)
        {
            return false;
        }
        #[cfg(windows)]
        {
            let Ok(current) = File::open(input) else {
                return false;
            };
            let (Ok(current), Ok(admitted)) = (
                solstone_core_journal_io::windows_file_identity(&current),
                solstone_core_journal_io::windows_file_identity(&self._file),
            ) else {
                return false;
            };
            if current != admitted {
                return false;
            }
        }
        true
    }

    pub(crate) fn capture(
        root: &Path,
        input: &Path,
        cancelled: &dyn Fn() -> bool,
    ) -> Result<Option<Self>, String> {
        let ReceiptFiles::Present(files) = accepted_receipt_files(root, input)? else {
            return Ok(None);
        };
        Self::capture_receipted(input, cancelled, files)
    }

    #[cfg(all(unix, feature = "audio", feature = "processing"))]
    pub(crate) fn capture_bound(
        root: &std::sync::Arc<solstone_core_journal_io::JournalRoot>,
        input: &Path,
        cancelled: &dyn Fn() -> bool,
    ) -> Result<Option<Self>, String> {
        if let ReceiptFiles::Present(files) = accepted_receipt_files(root.canonical_path(), input)?
        {
            return Self::capture_receipted(input, cancelled, files);
        }
        if let Some(receipt) =
            solstone_core_import_host::audio_custody::read_checked(root, input, cancelled)?
        {
            let mut accepted = Self::capture_receipted(input, cancelled, vec![receipt])?;
            if let Some(accepted) = &mut accepted {
                accepted.import_root = Some(std::sync::Arc::clone(root));
            }
            return Ok(accepted);
        }
        Ok(None)
    }

    fn capture_receipted(
        input: &Path,
        cancelled: &dyn Fn() -> bool,
        files: Vec<(u64, String)>,
    ) -> Result<Option<Self>, String> {
        let named = fs::symlink_metadata(input).map_err(|e| e.to_string())?;
        if !named.file_type().is_file() {
            return Err("input is not a regular file".into());
        }
        #[cfg(unix)]
        let mut file = File::open(input).map_err(|e| e.to_string())?;
        #[cfg(windows)]
        let mut file = {
            use std::os::windows::fs::OpenOptionsExt;
            // FILE_SHARE_READ | FILE_SHARE_DELETE; accepted immutable bytes
            // cannot be changed by a writer with a deferred last-write time.
            fs::OpenOptions::new()
                .read(true)
                .share_mode(0x00000001 | 0x00000004)
                .open(input)
                .map_err(|e| e.to_string())?
        };
        let admitted = stamp(&file.metadata().map_err(|e| e.to_string())?)?;
        if stamp(&named)? != admitted {
            return Err("input changed while opening".into());
        }
        let size = admitted.size;
        // The processing owner records execution-budget refusals durably. Custody
        // still verifies the entire accepted receipt with a constant-size buffer.
        if !file.metadata().map_err(|e| e.to_string())?.is_file() {
            return Err("input is not a regular file".into());
        }
        let mut hash = Sha256::new();
        let mut bytes = [0u8; 65536];
        loop {
            if cancelled() {
                return Err(CUSTODY_CANCELLED.into());
            }
            let n = file.read(&mut bytes).map_err(|e| e.to_string())?;
            if n == 0 {
                break;
            }
            hash.update(&bytes[..n]);
        }
        let digest = format!("{:x}", hash.finalize());
        let accepted = Self {
            _file: file,
            stamp: admitted,
            #[cfg(feature = "audio")]
            digest: digest.clone(),
            #[cfg(all(unix, feature = "audio", feature = "processing"))]
            import_root: None,
        };
        if cancelled() || !accepted.still_current(input) {
            return Err("input changed during custody verification".into());
        }
        Ok(files
            .iter()
            .any(|file| file.0 == size && file.1 == digest)
            .then_some(accepted))
    }
}

#[cfg(all(
    test,
    unix,
    feature = "audio",
    feature = "processing",
    feature = "full-tests"
))]
mod import_tests {
    use super::*;
    use solstone_core_import_host::audio_custody;
    use solstone_core_journal_io::JournalRoot;
    use std::sync::Arc;

    #[test]
    fn imported_audio_requires_exact_published_witness_and_keeps_it_at_checkpoints() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp
            .path()
            .join("chronicle/20261026/imported_audio/120000_3");
        fs::create_dir_all(&path).unwrap();
        let input = path.join("imported_audio.wav");
        fs::write(&input, b"accepted original").unwrap();
        let root = Arc::new(JournalRoot::open(temp.path()).unwrap());
        for (stream, key) in [("import.audio", "120000_300"), ("import.text", "120000_3")] {
            let other = temp
                .path()
                .join("chronicle/20261026")
                .join(stream)
                .join(key);
            fs::create_dir_all(&other).unwrap();
            let unwitnessed = other.join("original.wav");
            fs::write(&unwitnessed, b"not accepted by current bytes").unwrap();
            assert!(
                AcceptedInput::capture_bound(&root, &unwitnessed, &|| false)
                    .unwrap()
                    .is_none()
            );
        }
        assert!(
            AcceptedInput::capture_bound(&root, &input, &|| false)
                .unwrap()
                .is_none()
        );
        let segment = JournalRoot::open(&path).unwrap();
        audio_custody::publish(&segment, "20261026_120000", input.file_name().unwrap()).unwrap();
        let admitted = AcceptedInput::capture_bound(&root, &input, &|| false)
            .unwrap()
            .unwrap();
        assert!(admitted.receipt_matches(temp.path(), &input).unwrap());
        assert!(admitted.still_current(&input));
        fs::write(&input, b"changed original!").unwrap();
        assert!(!admitted.still_current(&input));
        assert!(
            AcceptedInput::capture_bound(&root, &input, &|| false)
                .unwrap()
                .is_none()
        );
        fs::write(path.join("imported-audio-custody.json"), b"invalid receipt").unwrap();
        assert!(AcceptedInput::capture_bound(&root, &input, &|| false).is_err());
        assert!(admitted.receipt_matches(temp.path(), &input).is_err());
    }

    #[test]
    fn imported_audio_witness_refuses_stop_and_original_root_replacement() {
        use solstone_core_import::{WorkStop, WorkStopGuard};
        let temp = tempfile::tempdir().unwrap();
        let journal = temp.path().join("journal");
        let path = journal.join("chronicle/20261026/imported_audio/120000_3");
        fs::create_dir_all(&path).unwrap();
        let input = path.join("imported_audio.wav");
        fs::write(&input, b"accepted original").unwrap();
        let root = Arc::new(JournalRoot::open(&journal).unwrap());
        let segment = JournalRoot::open(&path).unwrap();
        let stop = WorkStop::new();
        stop.request();
        {
            let _installed = WorkStopGuard::install(&stop);
            assert!(
                audio_custody::publish(&segment, "20261026_120000", input.file_name().unwrap())
                    .is_err()
            );
        }
        assert!(!path.join("imported-audio-custody.json").exists());
        audio_custody::publish(&segment, "20261026_120000", input.file_name().unwrap()).unwrap();
        assert!(AcceptedInput::capture_bound(&root, &input, &|| true).is_err());
        fs::rename(&journal, temp.path().join("original")).unwrap();
        fs::create_dir_all(&path).unwrap();
        fs::write(&input, b"foreign sentinel").unwrap();
        assert!(
            audio_custody::publish(&segment, "20261026_120001", input.file_name().unwrap())
                .is_err()
        );
        assert!(AcceptedInput::capture_bound(&root, &input, &|| false).is_err());
        assert_eq!(fs::read(&input).unwrap(), b"foreign sentinel");
        assert!(!path.join("imported-audio-custody.json").exists());
    }
}
