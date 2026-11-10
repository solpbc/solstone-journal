// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Setup evidence for the audio worker's actual immutable app model source.
//! Cached metadata is setup status only; inference keeps its full verification.

use solstone_core_convey_shell::PackagedSpeechSetup;
use solstone_core_journal_io::JournalRoot;
use std::{
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::SystemTime,
};

#[derive(Default)]
pub(crate) struct Slot(Mutex<Option<Arc<dyn PackagedSpeechSetup>>>);

impl Slot {
    pub(crate) fn bind(&self, proof: Arc<dyn PackagedSpeechSetup>) -> Result<(), String> {
        let mut current = self
            .0
            .lock()
            .map_err(|_| "speech setup owner unavailable")?;
        if let Some(previous) = current.as_ref() {
            if !Arc::ptr_eq(previous, &proof) {
                return Err("speech setup owner changed".into());
            }
        } else {
            *current = Some(proof);
        }
        Ok(())
    }

    fn proof(&self) -> Result<Arc<dyn PackagedSpeechSetup>, String> {
        self.0
            .lock()
            .map_err(|_| "speech setup owner unavailable")?
            .clone()
            .ok_or_else(|| "packaged speech has not been configured".into())
    }
}

impl PackagedSpeechSetup for Slot {
    fn status(&self) -> Result<(), String> {
        self.proof()?.status()
    }
    fn retry(&self) -> Result<(), String> {
        self.proof()?.retry()
    }
}

#[derive(Clone, PartialEq, Eq)]
struct Stamp {
    bytes: u64,
    modified: SystemTime,
    #[cfg(unix)]
    dev: u64,
    #[cfg(unix)]
    ino: u64,
    #[cfg(unix)]
    ctime: (i64, i64),
}

fn stamp(path: &Path) -> Result<Stamp, String> {
    let metadata = std::fs::symlink_metadata(path).map_err(|error| error.to_string())?;
    if !metadata.is_file() || metadata.file_type().is_symlink() {
        return Err("packaged speech model is not a regular file".into());
    }
    #[cfg(unix)]
    use std::os::unix::fs::MetadataExt;
    Ok(Stamp {
        bytes: metadata.len(),
        modified: metadata.modified().map_err(|error| error.to_string())?,
        #[cfg(unix)]
        dev: metadata.dev(),
        #[cfg(unix)]
        ino: metadata.ino(),
        #[cfg(unix)]
        ctime: (metadata.ctime(), metadata.ctime_nsec()),
    })
}

#[derive(Default)]
struct Checked {
    attempted: bool,
    witness: Option<Stamp>,
    error: Option<String>,
}

pub(crate) struct Proof {
    root: Arc<JournalRoot>,
    model: PathBuf,
    cancelled: Arc<dyn Fn() -> bool + Send + Sync>,
    verify: fn(&Path) -> Result<(), String>,
    checked: Mutex<Checked>,
}

fn verify_native(model: &Path) -> Result<(), String> {
    if !cfg!(feature = "audio-native") {
        return Err("native speech support was not compiled".into());
    }
    journal_mobile_audio::Parakeet::verify_model(model)
}

impl Proof {
    pub(crate) fn new(
        root: Arc<JournalRoot>,
        model: PathBuf,
        cancelled: Arc<dyn Fn() -> bool + Send + Sync>,
    ) -> Self {
        Self {
            root,
            model,
            cancelled,
            verify: verify_native,
            checked: Mutex::new(Checked::default()),
        }
    }

    fn checkpoint(&self) -> Result<(), String> {
        if (self.cancelled)() {
            return Err("speech processing is stopped".into());
        }
        self.root
            .revalidate_canonical_binding()
            .map_err(|error| error.to_string())
    }

    fn verify_locked(&self, checked: &mut Checked) -> Result<(), String> {
        self.checkpoint()?;
        // Setup already runs on the host's blocking pool. Keep the actual
        // admitted root use through verification even if its HTTP observer
        // disappears; native Stop/restore drains this before root release.
        let _use = solstone_core_journal_io::whole_journal_restore::begin_target_use_bound(
            self.root.clone(),
        )
        .map_err(|error| error.to_string())?;
        let result: Result<Stamp, String> = (|| {
            let before = stamp(&self.model)?;
            (self.verify)(&self.model)?;
            self.checkpoint()?;
            let after = stamp(&self.model)?;
            if before != after {
                return Err("packaged speech model changed during verification".into());
            }
            Ok(after)
        })();
        checked.attempted = true;
        match result {
            Ok(witness) => {
                checked.witness = Some(witness);
                checked.error = None;
                Ok(())
            }
            Err(error) => {
                checked.witness = None;
                checked.error = Some(error.clone());
                Err(error)
            }
        }
    }
}

impl PackagedSpeechSetup for Proof {
    fn status(&self) -> Result<(), String> {
        self.checkpoint()?;
        let mut checked = self
            .checked
            .lock()
            .map_err(|_| "speech setup verification unavailable")?;
        if !checked.attempted {
            return self.verify_locked(&mut checked);
        }
        if let Some(error) = &checked.error {
            return Err(error.clone());
        }
        let current = match stamp(&self.model) {
            Ok(current) => current,
            Err(error) => {
                checked.error = Some(error.clone());
                return Err(error);
            }
        };
        if Some(&current) != checked.witness.as_ref() {
            let error = "packaged speech model changed; retry verification".to_string();
            checked.error = Some(error.clone());
            return Err(error);
        }
        Ok(())
    }

    fn retry(&self) -> Result<(), String> {
        let mut checked = self
            .checked
            .lock()
            .map_err(|_| "speech setup verification unavailable")?;
        self.verify_locked(&mut checked)
    }
}

#[cfg(all(test, feature = "full-tests"))]
mod tests {
    use super::*;

    fn verify_fixture(path: &Path) -> Result<(), String> {
        let counter = path.with_extension("checks");
        let checks: u32 = std::fs::read_to_string(&counter)
            .ok()
            .and_then(|n| n.parse().ok())
            .unwrap_or(0);
        std::fs::write(counter, (checks + 1).to_string()).unwrap();
        if std::fs::read(path).unwrap() != b"pinned speech" {
            return Err("fixture digest differs".into());
        }
        Ok(())
    }

    fn fixture() -> (tempfile::TempDir, Proof) {
        let directory = tempfile::tempdir().unwrap();
        let journal = directory.path().join("journal");
        std::fs::create_dir(&journal).unwrap();
        let model = directory.path().join("model");
        std::fs::write(&model, b"pinned speech").unwrap();
        let root = Arc::new(JournalRoot::open(&journal).unwrap());
        let mut proof = Proof::new(root, model, Arc::new(|| false));
        // This tests witness/cache mechanics with an explicitly local fixture.
        // Production always uses verify_native and the complete maintained pin.
        proof.verify = verify_fixture;
        (directory, proof)
    }

    fn checks(proof: &Proof) -> u32 {
        std::fs::read_to_string(proof.model.with_extension("checks"))
            .unwrap()
            .parse()
            .unwrap()
    }

    #[test]
    fn verification_retains_admission_until_real_completion_after_observer_drop() {
        use solstone_core_journal_io::whole_journal_restore::{
            hold_target_for_restore, release_target_for_restore, wait_for_target_use_to_end,
        };
        use std::{
            sync::mpsc,
            thread,
            time::{Duration, Instant},
        };
        let (_directory, mut proof) = fixture();
        proof.verify = |model| {
            verify_fixture(model)?;
            std::fs::write(model.with_extension("entered"), b"entered").unwrap();
            let deadline = Instant::now() + Duration::from_secs(5);
            while !model.with_extension("release").exists() {
                if Instant::now() >= deadline {
                    return Err("fixture verifier timed out".into());
                }
                thread::sleep(Duration::from_millis(10));
            }
            Ok(())
        };
        let proof = Arc::new(proof);
        let retained = proof.clone();
        let (reply, observer) = mpsc::channel();
        let verifier = thread::spawn(move || {
            let _ = reply.send(retained.status());
        });
        let deadline = Instant::now() + Duration::from_secs(2);
        while !proof.model.with_extension("entered").exists() {
            assert!(Instant::now() < deadline);
            thread::sleep(Duration::from_millis(10));
        }
        drop(observer);
        let path = proof.root.canonical_path().to_owned();
        hold_target_for_restore(&path);
        let target = path.clone();
        let (finished, waiting) = mpsc::channel();
        let drain = thread::spawn(move || {
            wait_for_target_use_to_end(&target);
            finished.send(()).unwrap();
        });
        assert!(waiting.recv_timeout(Duration::from_millis(100)).is_err());
        std::fs::write(proof.model.with_extension("release"), b"release").unwrap();
        verifier.join().unwrap();
        waiting.recv_timeout(Duration::from_secs(2)).unwrap();
        drain.join().unwrap();
        release_target_for_restore(&path);
    }

    #[test]
    fn verified_status_avoids_rehash_and_changes_require_explicit_retry() {
        let (_directory, proof) = fixture();
        proof.status().unwrap();
        for _ in 0..5 {
            proof.status().unwrap();
        }
        assert_eq!(checks(&proof), 1);
        let replacement = proof.model.with_extension("next");
        std::fs::write(&replacement, b"broken speech").unwrap();
        std::fs::rename(&replacement, &proof.model).unwrap();
        assert!(proof.status().is_err());
        assert_eq!(checks(&proof), 1);
        assert_eq!(proof.retry().unwrap_err(), "fixture digest differs");
        assert_eq!(checks(&proof), 2);
        std::fs::write(&replacement, b"pinned speech").unwrap();
        std::fs::rename(&replacement, &proof.model).unwrap();
        assert!(
            proof.status().is_err(),
            "changed sources never regain readiness from metadata alone"
        );
        proof.retry().unwrap();
        proof.status().unwrap();
        assert_eq!(checks(&proof), 3);
    }

    #[test]
    fn cancellation_and_root_replacement_refuse_even_an_unchanged_verified_model() {
        let (directory, mut proof) = fixture();
        proof.status().unwrap();
        proof.cancelled = Arc::new(|| true);
        assert!(proof.status().is_err());
        assert!(proof.retry().is_err());
        assert_eq!(checks(&proof), 1);
        proof.cancelled = Arc::new(|| false);
        std::fs::rename(
            proof.root.canonical_path(),
            directory.path().join("original"),
        )
        .unwrap();
        std::fs::create_dir(directory.path().join("journal")).unwrap();
        assert!(proof.status().is_err());
        assert!(proof.retry().is_err());
        assert_eq!(checks(&proof), 1);
    }

    #[test]
    fn stop_during_verification_never_publishes_readiness() {
        let (_directory, mut proof) = fixture();
        let stop = proof.model.with_extension("stopped");
        proof.cancelled = Arc::new(move || stop.exists());
        proof.verify = |model| {
            verify_fixture(model)?;
            std::fs::write(model.with_extension("stopped"), b"stop").unwrap();
            Ok(())
        };
        assert!(proof.status().is_err());
        assert_eq!(checks(&proof), 1);
        assert!(proof.checked.lock().unwrap().witness.is_none());
    }

    #[test]
    fn unbound_slot_cannot_report_ready_or_replace_its_actual_audio_owner() {
        let (_first, first) = fixture();
        let (_second, second) = fixture();
        let slot = Slot::default();
        assert!(slot.status().is_err());
        assert!(slot.retry().is_err());
        let first: Arc<dyn PackagedSpeechSetup> = Arc::new(first);
        slot.bind(first.clone()).unwrap();
        slot.bind(first).unwrap();
        assert!(slot.bind(Arc::new(second)).is_err());
        slot.status().unwrap();
    }
}
