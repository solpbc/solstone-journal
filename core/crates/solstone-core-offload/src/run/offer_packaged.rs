// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Copied-package qualification only; production orchestration, cooperation,
//! settlement, carrier finish and provider durability remain caller obligations.
#![cfg(all(test, feature = "full-tests"))]
#![cfg(unix)]

use super::*;
use chrono::{DateTime, NaiveDate, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use solstone_core_backup::archive_activity::PreparedArchiveOperation;
use solstone_core_backup::archive_admission::{
    AdmissionError, ArchiveBudget, ArchiveCancellation, Clock,
};
use solstone_core_backup::archive_readback::{ReadAdmission, ReadOperation, ReadbackError};
use solstone_core_journal_io::media::{Cleanup, RootActivityCarrier};
use solstone_core_retention::archive_catalog::read_complete_catalog;
use solstone_core_retention::archive_offer::{
    OffloadOfferOutcome, OffloadOfferRefusal, OffloadOfferSelection, VerifiedOffloadOffers,
    capture_offload_offers, publish_offload_offers, verify_offload_offers,
};
use solstone_core_retention::archive_proof::ArchiveFileProof;
use solstone_core_retention::mark_mutation::{BoundMutationOutcome, MutationStatus};
use solstone_core_retention::marks;
use solstone_core_retention::remove_marked::ArchiveReadState;
use std::cell::Cell;
use std::fs;
use std::io::{self, Read, Write};
use std::os::fd::AsFd;
use std::os::unix::fs::MetadataExt;
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

type TestResult<T = ()> = Result<T, Box<dyn std::error::Error>>;
#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum Case {
    #[serde(rename = "creation_positive")]
    Positive,
    #[serde(rename = "creation_floor_null")]
    FloorNull,
    #[serde(rename = "creation_stale_processing")]
    StaleProcessing,
    #[serde(rename = "creation_writer_uncertain")]
    WriterUncertain,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Input {
    case: Case,
    journal: PathBuf,
    repository_id: String,
    snapshot_id: String,
    archive_paths: Vec<String>,
    expected_restic_path: PathBuf,
    control: PathBuf,
}
struct ProbeClock(Cell<i64>);
impl Clock for ProbeClock {
    fn now_unix(&self) -> i64 {
        self.0.get()
    }
    fn iso_week(&self) -> u8 {
        1
    }
}
fn invalid(message: impl Into<String>) -> io::Error {
    io::Error::other(message.into())
}
fn cut(stream: &mut UnixStream, label: &[u8; 8]) -> io::Result<()> {
    stream.write_all(label)?;
    let mut reply = [0];
    stream.read_exact(&mut reply)?;
    if reply != *b"a" {
        return Err(invalid("owning acknowledgement refused"));
    }
    Ok(())
}
fn stages(root: &Path) -> io::Result<Vec<PathBuf>> {
    let mut found = Vec::new();
    for entry in fs::read_dir(root)? {
        let entry = entry?;
        if entry
            .file_name()
            .to_string_lossy()
            .starts_with(".tmp-offload-")
        {
            found.push(entry.path());
        }
    }
    found.sort();
    Ok(found)
}
fn check_carrier(carrier: &RootActivityCarrier) -> TestResult {
    carrier.revalidate()?;
    assert_eq!(
        stages(carrier.journal_root().canonical_path())?,
        vec![carrier.diagnostic_path()]
    );
    assert_eq!(fs::read_dir(carrier.diagnostic_path())?.count(), 0);
    Ok(())
}
fn file_fact(path: &Path) -> TestResult<Value> {
    let metadata = fs::metadata(path)?;
    let bytes = fs::read(path)?;
    Ok(
        json!({"device":metadata.dev(),"inode":metadata.ino(),"bytes":metadata.len(),"sha256":format!("{:x}",Sha256::digest(&bytes))}),
    )
}
fn read_facts(verified: &VerifiedOffloadOffers<'_>) -> TestResult<Vec<Value>> {
    let mut facts = Vec::new();
    for resource in verified.archive_reads() {
        let ArchiveReadState::Prepared {
            file,
            target_identity,
            media,
            cleanup,
        } = resource
        else {
            return Err(invalid("successful verification lost original preparation").into());
        };
        assert_eq!(cleanup, Some(Cleanup::Complete));
        assert_eq!(*target_identity, media.identity());
        assert_eq!(file.bytes, media.digest().size);
        assert_eq!(file.sha256, media.digest().sha256);
        facts.push(json!({"name":file.name,"repository":file.repository_id,"snapshot":file.snapshot_id,
            "path":file.archive_path,"identity":target_identity,"cleanup":cleanup,"digest":media.digest()}));
    }
    assert_eq!(facts.len(), 2);
    let proof = &verified.marks()[0]
        .offload
        .as_ref()
        .ok_or_else(|| invalid("genuine offer missing"))?
        .proof;
    for (actual, expected) in facts.iter().zip(&proof.files) {
        assert_eq!(actual["name"], expected.name);
        assert_eq!(actual["repository"], proof.repository_id);
        assert_eq!(actual["snapshot"], proof.snapshot_id);
        assert_eq!(actual["path"], expected.archive_path);
    }
    Ok(facts)
}
fn receipt_facts(receipts: &[Result<LedgerWriteReceipt, LedgerWriteRefusal>]) -> Vec<Value> {
    receipts.iter().map(|result|match result {
        Ok(r)=>json!({"effect":format!("{:?}",r.effect()),"durability":r.durability().map(|fact|format!("{fact:?}")),
            "confirmation":format!("{:?}",r.confirmation()),"failures":r.failures()}),
        Err(e)=>json!({"refusal":format!("{e:?}")}),
    }).collect()
}
enum Dispatch<'clock> {
    Held {
        verified: VerifiedOffloadOffers<'clock>,
        receipts: Vec<Result<LedgerWriteReceipt, LedgerWriteRefusal>>,
    },
    Published {
        outcome: OffloadOfferOutcome<'clock>,
        receipts: Vec<Result<LedgerWriteReceipt, LedgerWriteRefusal>>,
    },
}
// SAME owning gate for every successful verification. No selector or expected
// result reaches this decision; readable history never replaces sync receipts.
fn dispatch<'clock>(
    verified: VerifiedOffloadOffers<'clock>,
    receipts: Vec<Result<LedgerWriteReceipt, LedgerWriteRefusal>>,
    today: NaiveDate,
    now: DateTime<Utc>,
    publication_calls: &mut usize,
) -> Dispatch<'clock> {
    let confirmed = !receipts.is_empty()
        && receipts.len() == verified.marks().len()
        && receipts.iter().all(|result| match result {
            Ok(r) => {
                matches!(
                    r.effect(),
                    LedgerWriteEffect::Appended { .. } | LedgerWriteEffect::AlreadyRecorded
                ) && r.durability() == [LedgerSync::Confirmed; 4]
                    && r.confirmation() == LedgerConfirmation::Confirmed
                    && r.failures().is_empty()
            }
            Err(_) => false,
        });
    if confirmed {
        *publication_calls += 1;
        Dispatch::Published {
            outcome: publish_offload_offers(verified, today, now),
            receipts,
        }
    } else {
        Dispatch::Held { verified, receipts }
    }
}
struct FileSyncFault {
    consumed: bool,
}
impl unix::Hooks for FileSyncFault {
    fn sync(&mut self, file: &impl AsFd, point: unix::SyncPoint) -> io::Result<()> {
        if point == unix::SyncPoint::File {
            assert!(!self.consumed);
            self.consumed = true;
            Err(invalid("injected file-sync uncertainty"))
        } else {
            nix::unistd::fsync(file).map_err(io::Error::from)
        }
    }
}
#[test]
#[ignore = "requires copied package, existing synthetic snapshot and owning process/HTTP fixture"]
fn packaged_offer_creation_entry() -> TestResult {
    let input: Input = serde_json::from_slice(&fs::read(
        std::env::var_os("OFFLOAD_OFFER_CREATION_PROBE_JSON")
            .ok_or_else(|| invalid("explicit input required"))?,
    )?)?;
    assert_eq!(input.archive_paths.len(), 2);
    let root = Arc::new(JournalRoot::open(&input.journal)?);
    let register_path = input.journal.join("health/retention-marks.json");
    let ledger_path = input.journal.join("health/offload/20261009.jsonl");
    assert!(!register_path.try_exists()?);
    assert!(!ledger_path.try_exists()?);
    let selection = OffloadOfferSelection {
        target: Target {
            day: "20261009".into(),
            stream: "_default".into(),
            dir: "120000_60_fixture".into(),
        },
        layout: SegmentLayout::Direct,
        names: vec!["file1.flac".into(), "file2.flac".into()],
    };
    let raw_dir = input
        .journal
        .join(solstone_core_retention::layout::segment_rel(
            &selection.target.day,
            &selection.target.stream,
            &selection.target.dir,
        ));
    let before_raw = selection
        .names
        .iter()
        .map(|name| file_fact(&raw_dir.join(name)))
        .collect::<TestResult<Vec<_>>>()?;
    let before_headers = selection
        .names
        .iter()
        .map(|name| fs::read(raw_dir.join(name).with_extension("jsonl")))
        .collect::<io::Result<Vec<_>>>()?;
    let started = i64::try_from(SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs())?;
    let clock = ProbeClock(Cell::new(started));
    let operation = PreparedArchiveOperation::prepare(
        Arc::clone(&root),
        ArchiveBudget::new_cancellable(&clock, ArchiveCancellation::new())?,
        &clock,
    )
    .map_err(|e| invalid(format!("actual original preparation: {e:?}")))?;
    let carrier = Arc::clone(operation.carrier());
    let activity = Arc::clone(operation.activity());
    let today = NaiveDate::from_ymd_opt(2026, 10, 10).ok_or_else(|| invalid("date"))?;
    let now = DateTime::from_timestamp(started, 0).ok_or_else(|| invalid("time"))?;
    let observed = capture_offload_offers(&operation, &[selection], today, now)
        .map_err(|e| invalid(format!("actual capture: {e:?}")))?;
    assert!(Arc::ptr_eq(observed.journal_root(), &root));
    assert!(Arc::ptr_eq(observed.activity(), &activity));
    assert_eq!(observed.groups().len(), 1);
    let group = &observed.groups()[0];
    assert_eq!(group.expected_mark(), None);
    assert_eq!(group.files().len(), 2);
    let proof = OffloadProof {
        journal_id: root.identity().observation_id(),
        manifest_id: "creation-fixture-manifest".into(),
        target: group.selection().target.clone(),
        layout: group.selection().layout,
        repository_id: input.repository_id.clone(),
        snapshot_id: input.snapshot_id.clone(),
        files: group
            .files()
            .iter()
            .zip(&input.archive_paths)
            .map(|(file, path)| ArchiveFileProof {
                witness_id: format!("creation-fixture-{}", file.name()),
                name: file.name().into(),
                bytes: file.observation().digest.size,
                sha256: file.observation().digest.sha256.clone(),
                archive_path: path.clone(),
                processing: file.processing().clone(),
            })
            .collect(),
    };
    let destination = operation
        .configuration()
        .destination()
        .ok_or_else(|| invalid("destination"))?;
    let password = operation
        .configuration()
        .keys()?
        .ok_or_else(|| invalid("key"))?
        .daily_key;
    let read = ReadOperation::admit(ReadAdmission::bind(
        operation,
        destination,
        password,
        input.repository_id,
        input.expected_restic_path,
    ))
    .map_err(|e| invalid(format!("actual read admission: {e:?}")))?;
    assert!(Arc::ptr_eq(read.carrier(), &carrier));
    let mut control = UnixStream::connect(&input.control)?;
    control.set_read_timeout(Some(Duration::from_secs(30)))?;
    control.set_write_timeout(Some(Duration::from_secs(30)))?;
    cut(&mut control, b"admitted")?;
    let result = verify_offload_offers(read, observed, vec![proof], today, now);
    if input.case == Case::FloorNull {
        let failure = match result {
            Err(f) => f,
            Ok(_) => return Err(invalid("null floor unexpectedly verified").into()),
        };
        assert!(matches!(
            failure.reason(),
            OffloadOfferRefusal::Readback(ReadbackError::Admission(
                AdmissionError::ConfigurationUnavailable
            ))
        ));
        assert!(Arc::ptr_eq(failure.journal_root(), &root));
        assert!(Arc::ptr_eq(failure.activity(), &activity));
        assert_eq!(failure.archive_reads().count(), 0);
        check_carrier(&carrier)?;
        assert!(!register_path.try_exists()?);
        assert!(!ledger_path.try_exists()?);
        for (i, name) in ["file1.flac", "file2.flac"].iter().enumerate() {
            assert_eq!(file_fact(&raw_dir.join(name))?, before_raw[i]);
            assert_eq!(
                fs::read(raw_dir.join(name).with_extension("jsonl"))?,
                before_headers[i]
            );
        }
        println!(
            "OFFER_CREATION_EVIDENCE {}",
            json!({"case":input.case,"refusal":format!("{:?}",failure.reason()),
            "carrier":carrier.diagnostic_path(),"reads":[],"publication_calls":0,"writer_calls":0,"raw":before_raw,
            "root_id":root.identity().observation_id(),"owners_retained":true})
        );
        io::stdout().flush()?;
        cut(&mut control, b"retained")?;
        assert!(Arc::ptr_eq(failure.observed().activity(), &activity));
        return Ok(());
    }
    let verified =
        result.map_err(|f| invalid(format!("actual verification refused: {:?}", f.reason())))?;
    assert!(Arc::ptr_eq(verified.journal_root(), &root));
    assert!(Arc::ptr_eq(verified.activity(), &activity));
    assert_eq!(verified.marks().len(), 1);
    let genuine_marks = verified.marks().to_vec();
    let reads = read_facts(&verified)?;
    check_carrier(&carrier)?;
    let mut fault = FileSyncFault { consumed: false };
    let receipts = verified
        .marks()
        .iter()
        .map(|mark| {
            let proof = &mark
                .offload
                .as_ref()
                .ok_or_else(|| invalid("genuine proof missing"))?
                .proof;
            Ok(if input.case == Case::WriterUncertain {
                unix::append_archive(&root, proof, 1, &mut fault)
            } else {
                append_archive_witness(&root, proof, 1)
            })
        })
        .collect::<TestResult<Vec<_>>>()?;
    assert_eq!(receipts.len(), 1);
    let receipt = receipts[0]
        .as_ref()
        .map_err(|e| invalid(format!("actual writer refused: {e:?}")))?;
    assert!(matches!(receipt.effect(),LedgerWriteEffect::Appended { bytes } if bytes>0));
    assert_eq!(receipt.confirmation(), LedgerConfirmation::Confirmed);
    if input.case == Case::WriterUncertain {
        assert!(fault.consumed);
        assert_eq!(
            receipt.durability(),
            [
                LedgerSync::Uncertain,
                LedgerSync::Confirmed,
                LedgerSync::Confirmed,
                LedgerSync::Confirmed
            ]
        );
        assert!(!receipt.failures().is_empty());
    } else {
        assert!(!fault.consumed);
        assert_eq!(receipt.durability(), [LedgerSync::Confirmed; 4]);
        assert!(receipt.failures().is_empty());
    }
    let ledger_bytes = fs::read(&ledger_path)?;
    let catalog = read_complete_catalog(&root).map_err(invalid)?;
    assert!(!catalog.degraded());
    let proof = &genuine_marks[0]
        .offload
        .as_ref()
        .ok_or_else(|| invalid("genuine proof missing"))?
        .proof;
    for file in &proof.files {
        assert!(catalog.active_witness(proof, file).is_some());
    }
    assert!(!register_path.try_exists()?);
    cut(&mut control, b"written!")?;
    let mut publication_calls = 0;
    let dispatched = dispatch(verified, receipts, today, now, &mut publication_calls);
    let (retained, receipts, publication) = match &dispatched {
        Dispatch::Held { verified, receipts } => {
            assert_eq!(input.case, Case::WriterUncertain);
            assert_eq!(publication_calls, 0);
            assert!(!register_path.try_exists()?);
            (verified, receipts, "held_uncertain_receipt")
        }
        Dispatch::Published { outcome, receipts } => {
            assert_eq!(publication_calls, 1);
            if input.case == Case::StaleProcessing {
                assert!(matches!(
                    outcome.publication(),
                    Err(OffloadOfferRefusal::Changed)
                ));
                assert!(!register_path.try_exists()?);
            } else {
                let Ok(BoundMutationOutcome::Published(result)) = outcome.publication() else {
                    return Err(invalid(format!(
                        "actual first publication: {:?}",
                        outcome.publication()
                    ))
                    .into());
                };
                assert_eq!(result.status, MutationStatus::Applied);
                let current = marks::load(root.canonical_path())?;
                assert_eq!(current.version, 2);
                assert_eq!(current, result.register);
                assert_eq!(
                    current.marks.values().cloned().collect::<Vec<_>>(),
                    genuine_marks
                );
            }
            (
                outcome.verified(),
                receipts,
                if input.case == Case::StaleProcessing {
                    "changed"
                } else {
                    "published_applied"
                },
            )
        }
    };
    assert_eq!(read_facts(retained)?, reads);
    assert!(Arc::ptr_eq(retained.journal_root(), &root));
    assert!(Arc::ptr_eq(retained.activity(), &activity));
    assert_eq!(retained.marks(), genuine_marks);
    assert_eq!(fs::read(&ledger_path)?, ledger_bytes);
    assert_eq!(read_complete_catalog(&root).map_err(invalid)?, catalog);
    let after_headers = ["file1.flac", "file2.flac"]
        .iter()
        .map(|name| fs::read(raw_dir.join(name).with_extension("jsonl")))
        .collect::<io::Result<Vec<_>>>()?;
    if input.case == Case::StaleProcessing {
        assert_eq!(after_headers[0], before_headers[0]);
        let end = before_headers[1]
            .iter()
            .position(|b| *b == b'\n')
            .ok_or_else(|| invalid("processing header missing"))?
            + 1;
        assert_eq!(after_headers[1], before_headers[1][..end]);
    } else {
        assert_eq!(after_headers, before_headers);
    }
    for (i, name) in ["file1.flac", "file2.flac"].iter().enumerate() {
        assert_eq!(file_fact(&raw_dir.join(name))?, before_raw[i]);
    }
    check_carrier(&carrier)?;
    println!(
        "OFFER_CREATION_EVIDENCE {}",
        json!({"case":input.case,"carrier":carrier.diagnostic_path(),"reads":reads,
        "publication":publication,"publication_calls":publication_calls,"writer_calls":receipts.len(),"receipts":receipt_facts(receipts),
        "marks":genuine_marks,"raw":before_raw,"ledger_sha256":format!("{:x}",Sha256::digest(&ledger_bytes)),
        "root_id":root.identity().observation_id(),"owners_retained":true})
    );
    io::stdout().flush()?;
    cut(&mut control, b"retained")?;
    assert_eq!(read_facts(retained)?, reads);
    Ok(())
}

// Separate caller qualification; the preceding four-control leaf remains frozen.
#[cfg(all(test, feature = "full-tests"))]
mod caller_qualification {
    use super::*;
    use crate::run::{
        OffloadCreationOwner, OffloadCreationRefusal, OffloadLedgerEffect, OffloadLedgerSync,
        OffloadLedgerWrite, create_offload_offers,
    };
    use chrono::Datelike;
    use solstone_core_backup_runtime::{
        BackupServices, NativeJournalMaintenance, SystemToolRunner, ToolOutput, ToolRequest,
        ToolRunner, UreqHttpTransport,
    };
    use solstone_core_retention::archive_proof::OffloadProof;
    use std::cell::RefCell;

    #[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
    #[serde(rename_all = "snake_case")]
    enum CallerCase {
        #[serde(rename = "caller_first_absent")]
        FirstAbsent,
        #[serde(rename = "caller_orphan_reuse")]
        OrphanReuse,
        #[serde(rename = "caller_writer_uncertain")]
        WriterUncertain,
    }
    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct CallerInput {
        case: CallerCase,
        journal: PathBuf,
        repository_id: String,
        snapshot_id: String,
        archive_paths: Vec<String>,
        expected_restic_path: PathBuf,
        control: PathBuf,
    }
    struct NativeClock;
    impl Clock for NativeClock {
        fn now_unix(&self) -> i64 {
            Utc::now().timestamp()
        }
        fn iso_week(&self) -> u8 {
            Utc::now().iso_week().week() as u8
        }
    }

    // Observation delegates the same actual request and cancellation, retaining
    // actual output even on failure. It grants no admission or settlement.
    struct ObservedRunner(RefCell<Vec<Value>>);
    impl ObservedRunner {
        fn observe(
            &self,
            request: &ToolRequest<'_>,
            cancellation: Option<&ArchiveCancellation>,
        ) -> io::Result<ToolOutput> {
            use std::os::fd::AsRawFd;
            let argv = request
                .argv
                .iter()
                .map(|arg| arg.to_string_lossy().into_owned())
                .collect::<Vec<_>>();
            let sources = argv
                .iter()
                .filter(|arg| arg.starts_with("/proc/self/fd/"))
                .map(|path| Ok(json!({"path":path,"fact":file_fact(Path::new(path))?})))
                .collect::<TestResult<Vec<_>>>()
                .map_err(|error| invalid(error.to_string()))?;
            let descriptors = request
                .pass_fds
                .iter()
                .map(|fd| {
                    let path = format!("/proc/self/fd/{}", fd.as_raw_fd());
                    let metadata = fs::metadata(&path)?;
                    Ok(json!({"fd":fd.as_raw_fd(),"device":metadata.dev(),"inode":metadata.ino()}))
                })
                .collect::<io::Result<Vec<_>>>()?;
            let start = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map_err(io::Error::other)?
                .as_nanos();
            let result = match cancellation {
                Some(cancellation) => SystemToolRunner.run_cancellable(request, cancellation),
                None => SystemToolRunner.run(request),
            };
            let end = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map_err(io::Error::other)?
                .as_nanos();
            let outcome = match &result {
                Ok(output) => {
                    json!({"returncode":output.returncode,"stdout_bytes":output.stdout,"stderr_bytes":output.stderr})
                }
                Err(error) => {
                    json!({"error":error.to_string(),"kind":format!("{:?}",error.kind())})
                }
            };
            self.0.borrow_mut().push(json!({"program":request.program.to_string_lossy(),"argv":argv,
                "environment_keys":request.env.keys().map(|key|key.to_string_lossy()).collect::<Vec<_>>(),
                "timeout_ms":request.timeout.map(|timeout|timeout.as_millis()),"stdin_present":request.stdin.is_some(),
                "cancellable":cancellation.is_some(),"sources":sources,"descriptors":descriptors,
                "started_unix_ns":start,"ended_unix_ns":end,"outcome":outcome}));
            result
        }
    }
    impl ToolRunner for ObservedRunner {
        fn run(&self, request: &ToolRequest<'_>) -> io::Result<ToolOutput> {
            self.observe(request, None)
        }
        fn run_cancellable(
            &self,
            request: &ToolRequest<'_>,
            cancellation: &ArchiveCancellation,
        ) -> io::Result<ToolOutput> {
            self.observe(request, Some(cancellation))
        }
    }

    #[test]
    #[ignore = "requires copied package and owning actual caller/S3/child fixture"]
    fn packaged_offer_caller_entry() -> TestResult {
        let input: CallerInput = serde_json::from_slice(&fs::read(
            std::env::var_os("OFFLOAD_OFFER_CALLER_PROBE_JSON")
                .ok_or_else(|| invalid("explicit caller input required"))?,
        )?)?;
        assert_eq!(input.archive_paths.len(), 2);
        let root = Arc::new(JournalRoot::open(&input.journal)?);
        let register_path = input.journal.join("health/retention-marks.json");
        let ledger_path = input.journal.join("health/offload/20261009.jsonl");
        assert!(!register_path.try_exists()?);
        assert!(!ledger_path.try_exists()?);
        assert!(stages(&input.journal)?.is_empty());
        let selection = OffloadOfferSelection {
            target: Target {
                day: "20261009".into(),
                stream: "_default".into(),
                dir: "120000_60_fixture".into(),
            },
            layout: SegmentLayout::Direct,
            names: vec!["file1.flac".into(), "file2.flac".into()],
        };
        let raw_dir = input
            .journal
            .join(solstone_core_retention::layout::segment_rel(
                &selection.target.day,
                &selection.target.stream,
                &selection.target.dir,
            ));
        let raw = selection
            .names
            .iter()
            .map(|name| file_fact(&raw_dir.join(name)))
            .collect::<TestResult<Vec<_>>>()?;
        let headers = selection
            .names
            .iter()
            .map(|name| fs::read(raw_dir.join(name).with_extension("jsonl")))
            .collect::<io::Result<Vec<_>>>()?;
        let configuration = fs::read(input.journal.join("config/journal.json"))?;
        let clock = NativeClock;
        let now = Utc::now();
        let today = NaiveDate::from_ymd_opt(2026, 10, 10).ok_or_else(|| invalid("date"))?;
        let budget = ArchiveBudget::new_cancellable(&clock, ArchiveCancellation::new())?;
        let budget_seconds = budget.remaining(&clock)?.as_secs();
        let operation = PreparedArchiveOperation::prepare(Arc::clone(&root), budget, &clock)
            .map_err(|error| invalid(format!("actual caller preparation: {error:?}")))?;
        let activity = Arc::clone(operation.activity());
        let carrier = Arc::clone(operation.carrier());
        let mut seed = None;
        if input.case == CallerCase::OrphanReuse {
            let observed =
                capture_offload_offers(&operation, std::slice::from_ref(&selection), today, now)
                    .map_err(|error| invalid(format!("actual orphan capture: {error:?}")))?;
            assert!(Arc::ptr_eq(observed.journal_root(), &root));
            assert!(Arc::ptr_eq(observed.activity(), &activity));
            assert_eq!(observed.groups().len(), 1);
            let group = &observed.groups()[0];
            assert!(group.expected_mark().is_none());
            let proof = OffloadProof {
                target: selection.target.clone(),
                layout: selection.layout,
                journal_id: root.identity().observation_id(),
                repository_id: input.repository_id.clone(),
                snapshot_id: input.snapshot_id.clone(),
                manifest_id: "4".repeat(64),
                files: group
                    .files()
                    .iter()
                    .zip(&input.archive_paths)
                    .enumerate()
                    .map(|(index, (file, path))| ArchiveFileProof {
                        witness_id: if index == 0 {
                            "5".repeat(64)
                        } else {
                            "6".repeat(64)
                        },
                        name: file.name().into(),
                        bytes: file.observation().digest.size,
                        sha256: file.observation().digest.sha256.clone(),
                        archive_path: path.clone(),
                        processing: file.processing().clone(),
                    })
                    .collect(),
            };
            proof.validate()?;
            let receipt = append_archive_witness(&root, &proof, u64::try_from(now.timestamp())?)
                .map_err(|error| invalid(format!("actual orphan seed writer: {error:?}")))?;
            assert!(matches!(receipt.effect(),LedgerWriteEffect::Appended {bytes} if bytes>0));
            assert_eq!(receipt.durability(), [LedgerSync::Confirmed; 4]);
            assert_eq!(receipt.confirmation(), LedgerConfirmation::Confirmed);
            assert!(receipt.failures().is_empty());
            let bytes = fs::read(&ledger_path)?;
            let catalog = read_complete_catalog(&root).map_err(invalid)?;
            for file in &proof.files {
                assert!(catalog.active_witness(&proof, file).is_some());
            }
            assert!(!register_path.try_exists()?);
            seed = Some((proof, receipt, bytes, catalog));
        }
        let runner = ObservedRunner(RefCell::new(Vec::new()));
        let http = UreqHttpTransport;
        let maintenance = NativeJournalMaintenance;
        let services = BackupServices {
            runner: &runner,
            http: &http,
            clock: &clock,
            restic: Some((&input.expected_restic_path).into()),
            rclone_path: None,
            version: "actual-caller-fixture",
            journal_maintenance: &maintenance,
        };
        let mut control = UnixStream::connect(&input.control)?;
        cut(&mut control, b"prepared")?;
        let fault = if input.case == CallerCase::WriterUncertain {
            Some(unix::FileSyncUncertainty::arm()?)
        } else {
            None
        };
        let creation = create_offload_offers(
            operation,
            &services,
            std::slice::from_ref(&selection),
            today,
            now,
        );
        println!(
            "CALLER_RETURN_FACTS {}",
            json!({"case":input.case,"reason":format!("{:?}",creation.reason()),
            "archives":creation.archives().iter().map(|result|format!("{result:?}")).collect::<Vec<_>>(),
            "requests":runner.0.borrow().clone(),"fault":fault.as_ref().map(unix::FileSyncUncertainty::observations)})
        );
        let (verified, publication) = match (input.case, creation.owner()) {
            (CallerCase::WriterUncertain, OffloadCreationOwner::Verified(verified)) => {
                assert!(matches!(
                    creation.reason(),
                    Some(OffloadCreationRefusal::Ledger)
                ));
                assert_eq!(fault.as_ref().expect("armed").observations(), (1, true));
                assert!(!register_path.try_exists()?);
                (verified, "held_ledger_uncertain")
            }
            (
                CallerCase::FirstAbsent | CallerCase::OrphanReuse,
                OffloadCreationOwner::Published(outcome),
            ) => {
                assert!(creation.reason().is_none());
                let Ok(BoundMutationOutcome::Published(published)) = outcome.publication() else {
                    return Err(invalid(format!(
                        "actual caller publication: {:?}",
                        outcome.publication()
                    ))
                    .into());
                };
                assert!(matches!(published.status, MutationStatus::Applied));
                let current = marks::load(&input.journal)?;
                assert_eq!(current.version, 2);
                assert_eq!(current, published.register);
                assert_eq!(
                    current.marks.values().cloned().collect::<Vec<_>>(),
                    outcome.verified().marks()
                );
                (outcome.verified(), "published_applied")
            }
            _ => {
                return Err(invalid(format!(
                    "actual caller unexpected owning phase: {:?}",
                    creation.reason()
                ))
                .into());
            }
        };
        assert!(Arc::ptr_eq(verified.journal_root(), &root));
        assert!(Arc::ptr_eq(verified.activity(), &activity));
        let observed = creation
            .observed()
            .ok_or_else(|| invalid("retained observed input missing"))?;
        assert!(Arc::ptr_eq(observed.journal_root(), &root));
        assert!(Arc::ptr_eq(observed.activity(), &activity));
        assert!(observed.groups()[0].expected_mark().is_none());
        assert!(creation.partial_copy().is_none());
        assert!(creation.native_error().is_none());
        let copies = creation.source_copies().collect::<Vec<_>>();
        assert_eq!(copies.len(), 2);
        let source_facts = copies
            .iter()
            .zip(&selection.names)
            .enumerate()
            .map(|(index, (copy, name))| {
                assert_eq!(copy.digest().size, raw[index]["bytes"].as_u64().unwrap());
                assert_eq!(copy.digest().sha256, raw[index]["sha256"].as_str().unwrap());
                json!({"name":name,"identity":copy.identity(),"digest":copy.digest()})
            })
            .collect::<Vec<_>>();
        assert_ne!(copies[0].identity(), copies[1].identity());
        let reads = read_facts(verified)?;
        let proof = &verified.marks()[0]
            .offload
            .as_ref()
            .ok_or_else(|| invalid("genuine caller offer missing"))?
            .proof;
        let offer = verified.marks()[0].offload.as_ref().expect("checked offer");
        offer.validate()?;
        assert!(!offer.nonce.as_str().is_empty());
        assert!(!offer.revision.as_str().is_empty());
        let requests = runner.0.borrow();
        let backups = requests
            .iter()
            .filter(|request| request["argv"][0] == "backup")
            .collect::<Vec<_>>();
        if input.case == CallerCase::OrphanReuse {
            assert!(backups.is_empty());
            assert!(creation.archives().is_empty());
            let (seed_proof, _, bytes, catalog) = seed.as_ref().expect("real retained orphan seed");
            assert_eq!(proof, seed_proof);
            assert_eq!(fs::read(&ledger_path)?, *bytes);
            assert_eq!(read_complete_catalog(&root).map_err(invalid)?, *catalog);
        } else {
            assert_eq!(backups.len(), 1);
            assert_eq!(creation.archives().len(), 1);
            let archive = creation.archives()[0]
                .as_ref()
                .map_err(|error| invalid(format!("actual retained archive: {error:?}")))?;
            assert_eq!(archive.repository_id, input.repository_id);
            assert_ne!(archive.snapshot_id, input.snapshot_id);
            assert_eq!(proof.snapshot_id, archive.snapshot_id);
            let args = backups[0]["argv"].as_array().expect("actual argv");
            assert!(args.contains(&json!("--force")));
            assert!(args.contains(&json!("--no-cache")));
            assert!(args.contains(&json!("solstone-archive")));
            assert_eq!(
                backups[0]["sources"]
                    .as_array()
                    .expect("actual sources")
                    .len(),
                2
            );
            assert_eq!(
                backups[0]["descriptors"]
                    .as_array()
                    .expect("actual descriptors")
                    .len(),
                2
            );
            assert_eq!(backups[0]["outcome"]["returncode"], 0);
        }
        let writes = creation.ledger_writes().collect::<Vec<_>>();
        assert_eq!(writes.len(), 1);
        let receipt = match writes[0] {
            OffloadLedgerWrite::Recorded {
                effect,
                durability,
                confirmation,
                failures,
            } => {
                if input.case == CallerCase::OrphanReuse {
                    assert_eq!(effect, OffloadLedgerEffect::AlreadyRecorded);
                } else {
                    assert!(matches!(effect,OffloadLedgerEffect::Appended {bytes} if bytes>0));
                }
                assert_eq!(confirmation, OffloadLedgerSync::Confirmed);
                if input.case == CallerCase::WriterUncertain {
                    assert_eq!(
                        durability,
                        [
                            OffloadLedgerSync::Uncertain,
                            OffloadLedgerSync::Confirmed,
                            OffloadLedgerSync::Confirmed,
                            OffloadLedgerSync::Confirmed
                        ]
                    );
                    assert!(!failures.is_empty());
                    assert!(failures.iter().any(|failure| {
                        failure.contains("sync File: injected owning caller FileSync uncertainty")
                    }));
                } else {
                    assert_eq!(durability, [OffloadLedgerSync::Confirmed; 4]);
                    assert!(failures.is_empty());
                }
                json!({"effect":format!("{effect:?}"),"durability":durability.map(|fact|format!("{fact:?}")),"confirmation":format!("{confirmation:?}"),"failures":failures})
            }
            OffloadLedgerWrite::Refused { reason, detail } => {
                return Err(invalid(format!("actual writer refusal {reason}: {detail:?}")).into());
            }
        };
        let catalog = read_complete_catalog(&root).map_err(invalid)?;
        assert_eq!(catalog.days.len(), 1);
        assert_eq!(catalog.days[0].segments.len(), 1);
        assert_eq!(catalog.days[0].segments[0].history.len(), 1);
        for file in &proof.files {
            assert!(catalog.active_witness(proof, file).is_some());
        }
        let ledger = fs::read(&ledger_path)?;
        carrier.revalidate()?;
        assert_eq!(fs::read_dir(carrier.diagnostic_path())?.count(), 0);
        let namespace = stages(&input.journal)?;
        assert_eq!(namespace.len(), 3);
        for stage in namespace
            .iter()
            .filter(|path| **path != carrier.diagnostic_path())
        {
            let entries = fs::read_dir(stage)?.collect::<io::Result<Vec<_>>>()?;
            assert_eq!(entries.len(), 1);
            let leaf = entries[0].path();
            let name = leaf
                .file_name()
                .ok_or_else(|| invalid("retained copy name missing"))?
                .to_string_lossy();
            let index = selection
                .names
                .iter()
                .position(|selected| *selected == name)
                .ok_or_else(|| invalid("unexpected retained copy"))?;
            let metadata = fs::metadata(&leaf)?;
            assert_eq!(
                json!(copies[index].identity()),
                json!({"platform":"unix","device":metadata.dev(),"inode":metadata.ino()})
            );
            assert_ne!(
                (metadata.dev(), metadata.ino()),
                (
                    raw[index]["device"].as_u64().unwrap(),
                    raw[index]["inode"].as_u64().unwrap()
                )
            );
            assert_eq!(file_fact(&leaf)?["sha256"], raw[index]["sha256"]);
        }
        for (index, name) in selection.names.iter().enumerate() {
            assert_eq!(file_fact(&raw_dir.join(name))?, raw[index]);
            assert_eq!(
                fs::read(raw_dir.join(name).with_extension("jsonl"))?,
                headers[index]
            );
        }
        assert_eq!(
            fs::read(input.journal.join("config/journal.json"))?,
            configuration
        );
        println!(
            "OFFER_CALLER_EVIDENCE {}",
            json!({"case":input.case,"publication":publication,
            "carrier":carrier.diagnostic_path(),"budget_seconds_before_prepare":budget_seconds,"now":now,
            "source_copies":source_facts,"reads":reads,"marks":verified.marks(),"raw":raw,
            "ledger_sha256":format!("{:x}",Sha256::digest(&ledger)),"receipt":receipt,"requests":*requests,
            "seed":seed.as_ref().map(|(proof,receipt,bytes,_)|json!({"label":"SYNTHETIC RECORDED ORPHAN; real pinned archived bytes/live capture; no prior caller completion", "proof":proof,
                "receipt":{"effect":format!("{:?}",receipt.effect()),"durability":receipt.durability().map(|fact|format!("{fact:?}")),
                    "confirmation":format!("{:?}",receipt.confirmation()),"failures":receipt.failures()},
                "ledger_sha256":format!("{:x}",Sha256::digest(bytes))})),"owners_retained":true})
        );
        io::stdout().flush()?;
        cut(&mut control, b"returned")?;
        assert_eq!(read_facts(verified)?, reads);
        assert_eq!(fs::read(&ledger_path)?, ledger);
        assert_eq!(read_complete_catalog(&root).map_err(invalid)?, catalog);
        carrier.revalidate()?;
        Ok(())
    }

    #[cfg(all(test, unix, feature = "full-tests"))]
    mod planner_qualification {
        use super::*;
        use crate::run::{
            CurrentOffloadAvailability, OffloadPlanningPhase, create_planned_offload_offers,
            with_payload_capacity_fault,
        };

        #[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
        #[serde(rename_all = "snake_case")]
        enum PlannerCase {
            PlannerPositive,
            PlannerPayloadCapacityRefused,
        }

        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct PlannerInput {
            case: PlannerCase,
            journal: PathBuf,
            repository_id: String,
            snapshot_id: String,
            archive_paths: Vec<String>,
            expected_restic_path: PathBuf,
            control: PathBuf,
        }

        #[test]
        #[ignore = "requires copied package and owning actual planner/S3/child fixture"]
        fn packaged_offer_planner_entry() -> TestResult {
            let input: PlannerInput = serde_json::from_slice(&fs::read(
                std::env::var_os("OFFLOAD_OFFER_PLANNER_PROBE_JSON")
                    .ok_or_else(|| invalid("explicit planner input required"))?,
            )?)?;
            assert_eq!(input.archive_paths.len(), 2);
            let root = Arc::new(JournalRoot::open(&input.journal)?);
            let register_path = input.journal.join("health/retention-marks.json");
            let ledger_path = input.journal.join("health/offload/20261009.jsonl");
            assert!(!register_path.try_exists()?);
            assert!(!ledger_path.try_exists()?);
            assert!(stages(&input.journal)?.is_empty());
            let selection = OffloadOfferSelection {
                target: Target {
                    day: "20261009".into(),
                    stream: "_default".into(),
                    dir: "120000_60_fixture".into(),
                },
                layout: SegmentLayout::Direct,
                names: vec!["file1.flac".into(), "file2.flac".into()],
            };
            let raw_dir = input
                .journal
                .join(solstone_core_retention::layout::segment_rel(
                    &selection.target.day,
                    &selection.target.stream,
                    &selection.target.dir,
                ));
            let raw = selection
                .names
                .iter()
                .map(|name| file_fact(&raw_dir.join(name)))
                .collect::<TestResult<Vec<_>>>()?;
            let headers = selection
                .names
                .iter()
                .map(|name| fs::read(raw_dir.join(name).with_extension("jsonl")))
                .collect::<io::Result<Vec<_>>>()?;
            let configuration = fs::read(input.journal.join("config/journal.json"))?;
            let clock = NativeClock;
            let at = Utc::now().fixed_offset();
            let budget = ArchiveBudget::new_cancellable(&clock, ArchiveCancellation::new())?;
            let budget_seconds = budget.remaining(&clock)?.as_secs();
            let operation = PreparedArchiveOperation::prepare(Arc::clone(&root), budget, &clock)
                .map_err(|error| invalid(format!("actual planner preparation: {error:?}")))?;
            assert!(operation.configuration().offload_enabled());
            assert_eq!(operation.configuration().offload_budget_bytes()?, Some(0));
            let activity = Arc::clone(operation.activity());
            let carrier = Arc::clone(operation.carrier());
            let runner = ObservedRunner(RefCell::new(Vec::new()));
            let http = UreqHttpTransport;
            let maintenance = NativeJournalMaintenance;
            let services = BackupServices {
                runner: &runner,
                http: &http,
                clock: &clock,
                restic: Some((&input.expected_restic_path).into()),
                rclone_path: None,
                version: "actual-planner-fixture",
                journal_maintenance: &maintenance,
            };
            let mut control = UnixStream::connect(&input.control)?;
            cut(&mut control, b"prepared")?;
            let (creation, consumed) = if input.case == PlannerCase::PlannerPayloadCapacityRefused {
                with_payload_capacity_fault(|| {
                    create_planned_offload_offers(operation, &services, at)
                })
            } else {
                (
                    create_planned_offload_offers(operation, &services, at),
                    false,
                )
            };
            println!(
                "PLANNER_RETURN_FACTS {}",
                json!({"case":input.case,
                "reason":format!("{:?}",creation.reason()),"fault_consumed":consumed,
                "requests":runner.0.borrow().clone()})
            );
            let planning = creation
                .planning()
                .ok_or_else(|| invalid("actual planning report missing"))?;
            let reconciliation = planning
                .reconciliation()
                .ok_or_else(|| invalid("reconciliation missing"))?
                .as_ref()
                .map_err(|failure| {
                    invalid(format!("actual reconciliation: {:?}", failure.reason()))
                })?;
            assert!(Arc::ptr_eq(reconciliation.journal_root(), &root));
            assert!(reconciliation.register().marks.is_empty());
            assert!(reconciliation.groups().is_empty());
            assert_eq!(planning.budget_bytes(), Some(0));
            let local = planning
                .local()
                .ok_or_else(|| invalid("actual local population missing"))?;
            let usage = local.usage().map_err(invalid)?;
            assert_eq!((usage.total_bytes, usage.total_files), (4_194_324, 2));
            assert_eq!(local.files.len(), 2);
            for file in &local.files {
                assert_eq!(file.key.day, selection.target.day);
                assert_eq!(file.key.stream, selection.target.stream);
                assert_eq!(file.key.segment_dir, selection.target.dir);
                assert_eq!(file.key.layout, selection.layout);
                assert!(selection.names.contains(&file.key.name));
                assert_eq!(file.bytes, 2_097_162);
            }
            let decision = planning
                .decision()
                .ok_or_else(|| invalid("actual current inspection missing"))?;
            assert_eq!(
                decision.availability(),
                CurrentOffloadAvailability::Available
            );
            assert!(decision.refusals().is_empty());
            assert_eq!(decision.eligible().count(), 0);
            assert_eq!(
                decision.root_observation(),
                root.identity().observation_id()
            );
            let projection = planning
                .initial_projection()
                .ok_or_else(|| invalid("actual projection unavailable"))?;
            assert_eq!(
                (
                    projection.local.total_bytes,
                    projection.local.total_files,
                    projection.eligible_bytes,
                    projection.eligible_files,
                    projection.projected_local_bytes
                ),
                (4_194_324, 2, 0, 0, 4_194_324)
            );
            let observed = creation
                .observed()
                .ok_or_else(|| invalid("real planner capture missing"))?;
            assert!(Arc::ptr_eq(observed.journal_root(), &root));
            assert!(Arc::ptr_eq(observed.activity(), &activity));
            assert_eq!(observed.groups().len(), 1);
            let group = &observed.groups()[0];
            assert_eq!(group.selection(), &selection);
            assert!(group.expected_mark().is_none());
            assert_eq!(group.files().len(), 2);
            for (file, fact) in group.files().iter().zip(&raw) {
                assert_eq!(
                    file.observation().digest.size,
                    fact["bytes"].as_u64().unwrap()
                );
                assert_eq!(
                    file.observation().digest.sha256,
                    fact["sha256"].as_str().unwrap()
                );
            }
            assert!(creation.partial_copy().is_none());
            assert!(creation.native_error().is_none());
            let requests = runner.0.borrow();
            assert!(!requests.is_empty());
            let mut source_facts = Vec::new();
            let mut reads = Vec::new();
            let mut marks = Vec::new();
            let mut receipt = None;
            let mut capacity_error_os = None;
            let publication;
            match input.case {
                PlannerCase::PlannerPayloadCapacityRefused => {
                    assert!(consumed);
                    assert_eq!(planning.phase(), OffloadPlanningPhase::Held);
                    let OffloadCreationOwner::Admitted(original) = creation.owner() else {
                        return Err(invalid("post-P refusal lost the actual admitted owner").into());
                    };
                    assert!(Arc::ptr_eq(original.journal_root(), &root));
                    assert!(Arc::ptr_eq(original.activity(), &activity));
                    assert!(Arc::ptr_eq(original.carrier(), &carrier));
                    assert_eq!(original.repository_id(), input.repository_id);
                    original.prepared_operation().revalidate()?;
                    let Some(OffloadCreationRefusal::Inputs(OffloadOfferRefusal::Io(error))) =
                        creation.reason()
                    else {
                        return Err(invalid("retained native capacity error missing").into());
                    };
                    assert_eq!(error.raw_os_error(), Some(13));
                    capacity_error_os = error.raw_os_error();
                    assert_eq!(creation.source_copies().count(), 0);
                    assert!(creation.archives().is_empty());
                    assert_eq!(creation.ledger_writes().count(), 0);
                    assert!(!register_path.try_exists()?);
                    assert!(!ledger_path.try_exists()?);
                    assert!(
                        requests.iter().all(|request| request["argv"][0] != "backup"
                            && request["argv"][0] != "dump")
                    );
                    check_carrier(&carrier)?;
                    publication = "held_capacity_os13";
                }
                PlannerCase::PlannerPositive => {
                    assert!(!consumed);
                    assert_eq!(planning.phase(), OffloadPlanningPhase::OffersPublished);
                    assert!(creation.reason().is_none());
                    let OffloadCreationOwner::Published(outcome) = creation.owner() else {
                        return Err(
                            invalid("actual planner did not retain genuine publication").into()
                        );
                    };
                    let Ok(BoundMutationOutcome::Published(published)) = outcome.publication()
                    else {
                        return Err(invalid(format!(
                            "actual planner publication: {:?}",
                            outcome.publication()
                        ))
                        .into());
                    };
                    assert!(matches!(published.status, MutationStatus::Applied));
                    let verified = outcome.verified();
                    assert!(Arc::ptr_eq(verified.journal_root(), &root));
                    assert!(Arc::ptr_eq(verified.activity(), &activity));
                    let current = marks::load(&input.journal)?;
                    assert_eq!(current.version, 2);
                    assert_eq!(current, published.register);
                    assert_eq!(
                        current.marks.values().cloned().collect::<Vec<_>>(),
                        verified.marks()
                    );
                    marks = verified.marks().to_vec();
                    reads = read_facts(verified)?;
                    let copies = creation.source_copies().collect::<Vec<_>>();
                    assert_eq!(copies.len(), 2);
                    for ((copy, name), fact) in copies.iter().zip(&selection.names).zip(&raw) {
                        assert_eq!(copy.digest().size, fact["bytes"].as_u64().unwrap());
                        assert_eq!(copy.digest().sha256, fact["sha256"].as_str().unwrap());
                        source_facts.push(
                            json!({"name":name,"identity":copy.identity(),"digest":copy.digest()}),
                        );
                    }
                    assert_ne!(copies[0].identity(), copies[1].identity());
                    let backups = requests
                        .iter()
                        .filter(|request| request["argv"][0] == "backup")
                        .collect::<Vec<_>>();
                    assert_eq!(backups.len(), 1);
                    for option in ["--force", "--no-cache", "solstone-archive"] {
                        assert!(
                            backups[0]["argv"]
                                .as_array()
                                .unwrap()
                                .contains(&json!(option))
                        );
                    }
                    assert_eq!(backups[0]["sources"].as_array().unwrap().len(), 2);
                    assert_eq!(backups[0]["descriptors"].as_array().unwrap().len(), 2);
                    assert_eq!(backups[0]["outcome"]["returncode"], 0);
                    assert_eq!(creation.archives().len(), 1);
                    let archive = creation.archives()[0]
                        .as_ref()
                        .map_err(|error| invalid(format!("actual archive: {error:?}")))?;
                    let proof = &marks[0]
                        .offload
                        .as_ref()
                        .ok_or_else(|| invalid("real offer missing"))?
                        .proof;
                    assert_eq!(archive.repository_id, input.repository_id);
                    assert_ne!(archive.snapshot_id, input.snapshot_id);
                    assert_eq!(archive.snapshot_id, proof.snapshot_id);
                    let writes = creation.ledger_writes().collect::<Vec<_>>();
                    assert_eq!(writes.len(), 1);
                    let OffloadLedgerWrite::Recorded {
                        effect,
                        durability,
                        confirmation,
                        failures,
                    } = writes[0]
                    else {
                        return Err(invalid("actual owning ledger receipt missing").into());
                    };
                    assert!(matches!(effect,OffloadLedgerEffect::Appended {bytes} if bytes>0));
                    assert_eq!(durability, [OffloadLedgerSync::Confirmed; 4]);
                    assert_eq!(confirmation, OffloadLedgerSync::Confirmed);
                    assert!(failures.is_empty());
                    receipt = Some(
                        json!({"effect":format!("{effect:?}"),"durability":durability.map(|fact|format!("{fact:?}")),
                        "confirmation":format!("{confirmation:?}"),"failures":failures}),
                    );
                    let catalog = read_complete_catalog(&root).map_err(invalid)?;
                    assert_eq!(catalog.days.len(), 1);
                    assert_eq!(catalog.days[0].segments.len(), 1);
                    assert_eq!(catalog.days[0].segments[0].history.len(), 1);
                    for file in &proof.files {
                        assert!(catalog.active_witness(proof, file).is_some());
                    }
                    assert_eq!(stages(&input.journal)?.len(), 3);
                    publication = "published_applied";
                }
            }
            for (index, name) in selection.names.iter().enumerate() {
                assert_eq!(file_fact(&raw_dir.join(name))?, raw[index]);
                assert_eq!(
                    fs::read(raw_dir.join(name).with_extension("jsonl"))?,
                    headers[index]
                );
            }
            assert_eq!(
                fs::read(input.journal.join("config/journal.json"))?,
                configuration
            );
            carrier.revalidate()?;
            assert_eq!(fs::read_dir(carrier.diagnostic_path())?.count(), 0);
            let catalog = read_complete_catalog(&root).map_err(invalid)?;
            let ledger = if ledger_path.try_exists()? {
                Some(fs::read(&ledger_path)?)
            } else {
                None
            };
            println!(
                "OFFER_PLANNER_EVIDENCE {}",
                json!({"case":input.case,"publication":publication,
                "carrier":carrier.diagnostic_path(),"budget_seconds_before_prepare":budget_seconds,"at":at,
                "phase":format!("{:?}",planning.phase()),"projection":{"local_bytes":projection.local.total_bytes,
                    "local_files":projection.local.total_files,"eligible_bytes":projection.eligible_bytes,
                    "eligible_files":projection.eligible_files,"projected_local_bytes":projection.projected_local_bytes},
                "source_copies":source_facts,"reads":reads,"marks":marks,"raw":raw,"receipt":receipt,
                "ledger_sha256":ledger.as_ref().map(|bytes|format!("{:x}",Sha256::digest(bytes))),
                "requests":*requests,"fault_consumed":consumed,"capacity_error_os":capacity_error_os,
                "owners_retained":true})
            );
            io::stdout().flush()?;
            cut(&mut control, b"returned")?;
            assert_eq!(read_complete_catalog(&root).map_err(invalid)?, catalog);
            assert_eq!(
                if ledger_path.try_exists()? {
                    Some(fs::read(&ledger_path)?)
                } else {
                    None
                },
                ledger
            );
            carrier.revalidate()?;
            if input.case == PlannerCase::PlannerPayloadCapacityRefused {
                let OffloadCreationOwner::Admitted(original) = creation.owner() else {
                    unreachable!()
                };
                assert!(Arc::ptr_eq(original.carrier(), &carrier));
                original.prepared_operation().revalidate()?;
                let Some(OffloadCreationRefusal::Inputs(OffloadOfferRefusal::Io(error))) =
                    creation.reason()
                else {
                    unreachable!()
                };
                assert_eq!(error.raw_os_error(), Some(13));
                check_carrier(&carrier)?;
            } else {
                let OffloadCreationOwner::Published(outcome) = creation.owner() else {
                    unreachable!()
                };
                assert_eq!(read_facts(outcome.verified())?, reads);
            }
            Ok(())
        }
    }
}

// This appendix qualifies only the actual owning creation/finish entrance.
// Historical package adaptation and synthetic S3 are explicit driver evidence.
mod owned_creation_qualification {
    use super::*;
    use crate::run::{
        OffloadCreationRefusal, OffloadLedgerEffect, OffloadLedgerSync, OffloadLedgerWrite,
        OwnedOffloadCreationFinish, OwnedOffloadCreationFinishRefusal, OwnedOffloadCreationOwner,
        create_owned_offload_offers, prepare_owned_offload_inputs, with_source_cleanup_fault,
    };
    use chrono::Datelike;
    use solstone_core_backup::archive_activity::OwnedPreparedArchiveOperation;
    use solstone_core_backup_runtime::{
        DirectReadFinishState, OwnedDirectReadFinish,
        admit_prepared_archive_owned_creator_installed,
    };
    use solstone_core_journal_io::activity::{ActivityLease, TeardownLease};
    use solstone_core_journal_io::activity_obligations::NativeObligation;
    use solstone_core_journal_io::atomic::{
        BoundPublicationPrimitive, run_with_bound_publication_fault,
    };
    use solstone_core_journal_io::media::{
        MediaDirectory, MediaIdentity, MediaStage, PreparedMedia,
    };
    use solstone_core_retention::archive_offer::SettledOffloadOfferReport;
    use std::sync::Arc;

    #[derive(Clone, Copy, Debug, Deserialize, Serialize, Eq, PartialEq)]
    #[serde(rename_all = "snake_case")]
    enum OwnedCase {
        #[serde(rename = "owned_creation_complete")]
        Complete,
        #[serde(rename = "owned_creation_publication_uncertain")]
        PublicationUncertain,
        #[serde(rename = "owned_creation_second_source_cleanup_pending")]
        SecondCleanup,
        #[serde(rename = "owned_creation_distinct_final_obligation")]
        Obligation,
    }
    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct OwnedInput {
        case: OwnedCase,
        journal: PathBuf,
        repository_id: String,
        expected_restic_path: PathBuf,
        control: PathBuf,
    }
    struct ActualClock;
    impl Clock for ActualClock {
        fn now_unix(&self) -> i64 {
            Utc::now().timestamp()
        }
        fn iso_week(&self) -> u8 {
            Utc::now().iso_week().week() as u8
        }
    }
    fn prepared_cut(control: &mut UnixStream) -> TestResult {
        control.write_all(b"prepared")?;
        control.write_all(&std::process::id().to_be_bytes())?;
        let mut ack = [0];
        control.read_exact(&mut ack)?;
        assert_eq!(ack, [b'a']);
        Ok(())
    }
    fn directory_fact(path: &Path) -> TestResult<Value> {
        let m = fs::symlink_metadata(path)?;
        assert!(m.is_dir() && !m.file_type().is_symlink());
        Ok(json!({"device":m.dev(),"inode":m.ino()}))
    }
    fn identity_matches(identity: MediaIdentity, path: &Path) -> io::Result<bool> {
        let m = fs::symlink_metadata(path)?;
        Ok(identity
            == MediaIdentity::Unix {
                device: m.dev(),
                inode: m.ino(),
            })
    }
    fn copies<'a>(
        journal: &Path,
        owned: impl Iterator<Item = &'a PreparedMedia>,
    ) -> TestResult<Vec<Value>> {
        let mut facts = Vec::new();
        for media in owned {
            let mut matches = Vec::new();
            for stage in stages(journal)? {
                for leaf in fs::read_dir(&stage)? {
                    let leaf = leaf?.path();
                    if identity_matches(media.identity(), &leaf)? {
                        matches.push((stage.clone(), leaf));
                    }
                }
            }
            assert_eq!(matches.len(), 1);
            let (stage, leaf) = &matches[0];
            let fact = file_fact(leaf)?;
            assert_eq!(fact["bytes"], media.digest().size);
            assert_eq!(fact["sha256"], media.digest().sha256);
            facts.push(json!({"name":leaf.file_name().unwrap().to_string_lossy(),"identity":media.identity(),
                "digest":media.digest(),"stage":stage,"stage_identity":directory_fact(stage)?,"path":leaf,"fact":fact}));
        }
        Ok(facts)
    }
    fn ledger_facts<'a>(writes: impl Iterator<Item = OffloadLedgerWrite<'a>>) -> Vec<Value> {
        writes.map(|write| match write {
            OffloadLedgerWrite::Recorded { effect, durability, confirmation, failures } => {
                assert!(matches!(effect, OffloadLedgerEffect::Appended { bytes } if bytes>0));
                assert_eq!(durability, [OffloadLedgerSync::Confirmed;4]);
                assert_eq!(confirmation, OffloadLedgerSync::Confirmed);
                assert!(failures.is_empty());
                json!({"effect":format!("{effect:?}"),"durability":durability.map(|v|format!("{v:?}")),
                    "confirmation":format!("{confirmation:?}"),"failures":failures})
            }
            OffloadLedgerWrite::Refused { reason, detail } => panic!("actual witness refused: {reason} {detail:?}"),
        }).collect()
    }
    fn settled_facts(report: &SettledOffloadOfferReport) -> Value {
        let reads = report.archive_reads().iter().map(|r| {
            assert_eq!(r.cleanup(), Some(Cleanup::Complete));
            assert_eq!(r.file().bytes, r.digest().size);
            assert_eq!(r.file().sha256, r.digest().sha256);
            json!({"name":r.file().name,"repository":r.file().repository_id,"snapshot":r.file().snapshot_id,
                "path":r.file().archive_path,"identity":r.target_identity(),"digest":r.digest(),"cleanup":r.cleanup()})
        }).collect::<Vec<_>>();
        json!({"publication":format!("{:?}",report.publication()),"marks":report.marks(),"reads":reads})
    }
    fn completed_facts(
        finish: &OwnedDirectReadFinish,
        clock: &Arc<dyn Clock + Send + Sync>,
    ) -> Value {
        assert_eq!(finish.state(), DirectReadFinishState::Completed);
        assert!(finish.reason().is_none());
        assert!(Arc::ptr_eq(finish.owned_clock(), clock));
        let retired = finish.retirement().expect("actual retirement");
        assert!(retired.receipt.failures().is_empty());
        json!({"state":format!("{:?}",finish.state()),"root":finish.journal_root().identity().observation_id(),
            "carrier":retired.owned.diagnostic_path(),"effect":format!("{:?}",retired.receipt.effect()),
            "root_durability":format!("{:?}",retired.receipt.root_durability()),
            "confirmation":format!("{:?}",retired.receipt.confirmation()),"failures":retired.receipt.failures().iter().map(|error|
                json!({"error":error.to_string(),"kind":format!("{:?}",error.kind()),"raw_os_error":error.raw_os_error()})).collect::<Vec<_>>()})
    }

    #[test]
    #[ignore = "requires exact copied package and owning creator/finish S3/process fixture"]
    fn packaged_owned_creation_finish_entry() -> TestResult {
        let input: OwnedInput = serde_json::from_slice(&fs::read(
            std::env::var_os("OFFLOAD_OWNED_CREATION_PROBE_JSON")
                .ok_or_else(|| invalid("explicit owned creation input required"))?,
        )?)?;
        let names = if input.case == OwnedCase::SecondCleanup {
            vec![
                "file1.flac".to_owned(),
                "file2.flac".to_owned(),
                "file3.flac".to_owned(),
            ]
        } else {
            vec!["file1.flac".to_owned(), "file2.flac".to_owned()]
        };
        let root = Arc::new(JournalRoot::open(&input.journal)?);
        assert!(stages(&input.journal)?.is_empty());
        let register_path = input.journal.join("health/retention-marks.json");
        let ledger_path = input.journal.join("health/offload/20261009.jsonl");
        assert!(!register_path.try_exists()? && !ledger_path.try_exists()?);
        let selection = OffloadOfferSelection {
            target: Target {
                day: "20261009".into(),
                stream: "_default".into(),
                dir: "120000_60_fixture".into(),
            },
            layout: SegmentLayout::Direct,
            names,
        };
        let raw_dir = input.journal.join("chronicle/20261009/120000_60_fixture");
        let raw = selection
            .names
            .iter()
            .map(|name| file_fact(&raw_dir.join(name)))
            .collect::<TestResult<Vec<_>>>()?;
        let headers = selection
            .names
            .iter()
            .map(|name| fs::read(raw_dir.join(name).with_extension("jsonl")))
            .collect::<io::Result<Vec<_>>>()?;
        let config = fs::read(input.journal.join("config/journal.json"))?;
        let clock: Arc<dyn Clock + Send + Sync> = Arc::new(ActualClock);
        let cancellation = ArchiveCancellation::new();
        let budget = ArchiveBudget::new_cancellable(clock.as_ref(), cancellation.clone())?;
        let original = match OwnedPreparedArchiveOperation::prepare(
            Arc::clone(&root),
            budget,
            Arc::clone(&clock),
        ) {
            Ok(original) => original,
            Err(failure) => {
                let reason = format!("{:?}", failure.reason());
                std::hint::black_box(&failure);
                panic!("original preparation refused: {reason}")
            }
        };
        let carrier = Arc::clone(original.carrier());
        let original_carrier = carrier.diagnostic_path();
        let original_carrier_identity = directory_fact(&original_carrier)?;
        let mut control = UnixStream::connect(&input.control)?;
        prepared_cut(&mut control)?;
        let today = NaiveDate::from_ymd_opt(2026, 10, 10).unwrap();
        let now = Utc::now();
        let inputs = match prepare_owned_offload_inputs(
            &original,
            std::slice::from_ref(&selection),
            today,
            now,
        ) {
            Ok(inputs) => inputs,
            Err(failure) => {
                let closed = original.into_owned_hold();
                assert!(Arc::ptr_eq(closed.carrier(), failure.carrier()));
                let reason = format!("{:?}", failure.reason());
                std::hint::black_box(&(closed, failure));
                panic!("pre-WAN capture refused before admission: {reason}")
            }
        };
        assert!(Arc::ptr_eq(inputs.carrier(), &carrier));
        assert_eq!(inputs.observed().groups().len(), 1);
        assert_eq!(
            inputs.observed().groups()[0].files().len(),
            selection.names.len()
        );
        assert!(inputs.observed().groups()[0].expected_mark().is_none());
        cut(&mut control, b"captured")?;
        let installed = match admit_prepared_archive_owned_creator_installed(
            original,
            &input.expected_restic_path,
            None,
            "test",
            &solstone_core_backup_runtime::UreqHttpTransport,
        ) {
            Ok(installed) => installed,
            Err(failure) => {
                let reason = format!("{:?}", failure.error());
                std::hint::black_box(&failure);
                panic!("actual creator admission refused: {reason}")
            }
        };
        let (admitted, outside) = installed.into_parts();
        assert!(Arc::ptr_eq(admitted.carrier(), &carrier));
        assert!(Arc::ptr_eq(outside.carrier(), &carrier));
        assert!(Arc::ptr_eq(outside.owned_clock(), &clock));
        assert_eq!(admitted.repository_id(), input.repository_id);
        cut(&mut control, b"admitted")?;
        let created = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            if input.case == OwnedCase::PublicationUncertain {
                run_with_bound_publication_fault(
                    BoundPublicationPrimitive::ParentSync,
                    1,
                    13,
                    || create_owned_offload_offers(admitted, inputs, today, now),
                )
            } else {
                (
                    create_owned_offload_offers(admitted, inputs, today, now),
                    false,
                )
            }
        }));
        let (creation, publication_fault) = match created {
            Ok(created) => created,
            Err(error) => {
                let unknown = outside.into_unknown();
                assert_eq!(unknown.state(), DirectReadFinishState::Unknown);
                std::hint::black_box(&unknown);
                std::panic::resume_unwind(error)
            }
        };
        assert_eq!(
            publication_fault,
            input.case == OwnedCase::PublicationUncertain
        );
        assert!(Arc::ptr_eq(creation.carrier(), &carrier));
        assert!(creation.partial_copy().is_none());
        let OwnedOffloadCreationOwner::Published(outcome) = creation.owner() else {
            panic!("actual creation not Published: {:?}", creation.reason())
        };
        let verified = outcome.verified();
        let marks = verified.marks().to_vec();
        assert_eq!(marks.len(), 1);
        assert_eq!(marks[0].proposal.names, selection.names);
        assert_eq!(
            marks[0].proposal.bytes,
            2097162 * selection.names.len() as u64
        );
        let proof = &marks[0].offload.as_ref().expect("actual bound offer").proof;
        assert_eq!(proof.repository_id, input.repository_id);
        assert_eq!(proof.snapshot_id.len(), 64);
        assert_eq!(proof.files.len(), selection.names.len());
        let catalog = read_complete_catalog(&root).map_err(invalid)?;
        for (index, file) in proof.files.iter().enumerate() {
            assert!(catalog.active_witness(proof, file).is_some());
            assert_eq!(file.name, selection.names[index]);
            assert_eq!(file.bytes, raw[index]["bytes"]);
            assert_eq!(file.sha256, raw[index]["sha256"]);
        }
        let reads=verified.archive_reads().map(|read| {
            let ArchiveReadState::Prepared { file,target_identity,media,cleanup }=read else { panic!("missing original prepared read") };
            assert_eq!(cleanup,Some(Cleanup::Complete)); assert_eq!(*target_identity,media.identity());
            json!({"name":file.name,"repository":file.repository_id,"snapshot":file.snapshot_id,"path":file.archive_path,
                "identity":target_identity,"digest":media.digest(),"cleanup":cleanup})
        }).collect::<Vec<_>>();
        assert_eq!(reads.len(), selection.names.len());
        let ledger = ledger_facts(creation.ledger_writes());
        assert_eq!(ledger.len(), 1);
        let archives = creation
            .archives()
            .iter()
            .map(|result| match result {
                Ok(snapshot) => {
                    json!({"repository":snapshot.repository_id,"snapshot":snapshot.snapshot_id})
                }
                Err(failure) => panic!("actual archive failed: {failure:?}"),
            })
            .collect::<Vec<_>>();
        assert_eq!(archives.len(), 1);
        let source_copies = copies(&input.journal, creation.source_copies())?;
        assert_eq!(source_copies.len(), selection.names.len());
        let publication = if input.case == OwnedCase::PublicationUncertain {
            assert!(matches!(
                creation.reason(),
                Some(OffloadCreationRefusal::Publication)
            ));
            let Ok(BoundMutationOutcome::PublishedUncertain {
                candidate,
                uncertainty,
            }) = outcome.publication()
            else {
                panic!("actual uncertainty absent")
            };
            assert_eq!(candidate.status, MutationStatus::Applied);
            assert_eq!(
                uncertainty
                    .durability
                    .as_ref()
                    .and_then(io::Error::raw_os_error),
                Some(13)
            );
            json!({"kind":"published_uncertain","candidate":candidate.register,"durability_os":13,"actual":format!("{uncertainty:?}")})
        } else {
            assert!(creation.reason().is_none());
            let Ok(BoundMutationOutcome::Published(result)) = outcome.publication() else {
                panic!("actual confirmed publication absent")
            };
            assert_eq!(result.status, MutationStatus::Applied);
            json!({"kind":"published_applied","register":result.register})
        };
        let register_before_finish = fs::read(&register_path)?;
        let ledger_before_finish = fs::read(&ledger_path)?;
        cut(&mut control, b"created_")?;
        let mut foreign_stage = None;
        let mut foreign_fact = None;
        if input.case == OwnedCase::Obligation {
            let parent = MediaDirectory::open(Arc::clone(&root), Path::new(""))?;
            let stage = match MediaStage::create(parent, Arc::clone(carrier.activity())) {
                Ok(stage) => stage,
                Err(failure) => {
                    std::hint::black_box(&failure);
                    panic!("actual different stage failed")
                }
            };
            foreign_fact = Some(
                json!({"path":stage.diagnostic_path(),"identity":directory_fact(&stage.diagnostic_path())?}),
            );
            foreign_stage = Some(stage);
        }
        let finished = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            if input.case == OwnedCase::SecondCleanup {
                with_source_cleanup_fault(1, || creation.finish())
            } else {
                (creation.finish(), false)
            }
        }));
        let (finished, cleanup_fault) = match finished {
            Ok(result) => result,
            Err(error) => {
                let unknown = outside.into_unknown();
                assert_eq!(unknown.state(), DirectReadFinishState::Unknown);
                std::hint::black_box(&unknown);
                std::panic::resume_unwind(error)
            }
        };
        assert_eq!(cleanup_fault, input.case == OwnedCase::SecondCleanup);
        let mut next = None;
        let mut completion = None;
        let mut held_custody = None;
        let finish_facts = match finished {
            OwnedOffloadCreationFinish::Settled(settled) => {
                assert_eq!(input.case, OwnedCase::Complete);
                let (read, report) = settled.into_parts();
                assert!(Arc::ptr_eq(read.carrier(), &carrier));
                assert!(Arc::ptr_eq(read.owned_clock(), &clock));
                assert_eq!(report.source_copies().len(), selection.names.len());
                for (actual, before) in report.source_copies().iter().zip(&source_copies) {
                    assert_eq!(actual.cleanup, Cleanup::Complete);
                    assert_eq!(serde_json::to_value(actual.identity)?, before["identity"]);
                    assert_eq!(serde_json::to_value(&actual.digest)?, before["digest"]);
                    assert!(!Path::new(before["stage"].as_str().unwrap()).try_exists()?);
                }
                let facts = json!({"kind":"settled","report":settled_facts(report.offers()),
                    "source_cleanups":report.source_copies().iter().enumerate().map(|(i,r)|json!({"index":i,"cleanup":r.cleanup,"identity":r.identity,"digest":r.digest})).collect::<Vec<_>>(),
                    "ledger":ledger_facts(report.ledger_writes())});
                check_carrier(&carrier)?;
                cut(&mut control, b"finished")?;
                drop(carrier);
                let result = outside.finish_owned(read);
                completion = Some(completed_facts(&result, &clock));
                assert!(stages(&input.journal)?.is_empty());
                drop(result); // Already explicitly retired; closes its activity handle.
                cut(&mut control, b"retired_")?;
                let budget =
                    ArchiveBudget::new_cancellable(clock.as_ref(), ArchiveCancellation::new())?;
                let original = match OwnedPreparedArchiveOperation::prepare(
                    Arc::clone(&root),
                    budget,
                    Arc::clone(&clock),
                ) {
                    Ok(original) => original,
                    Err(failure) => {
                        std::hint::black_box(&failure);
                        panic!("next actual original refused")
                    }
                };
                let installed = match admit_prepared_archive_owned_creator_installed(
                    original,
                    &input.expected_restic_path,
                    None,
                    "test",
                    &solstone_core_backup_runtime::UreqHttpTransport,
                ) {
                    Ok(installed) => installed,
                    Err(failure) => {
                        std::hint::black_box(&failure);
                        panic!("next actual installed admission refused")
                    }
                };
                let (admitted, outside) = installed.into_parts();
                let read = match admitted.into_read_operation() {
                    Ok(read) => read,
                    Err(failure) => {
                        std::hint::black_box(&failure);
                        panic!("next no-writer handoff refused")
                    }
                };
                let settled = match read.settle_direct() {
                    Ok(read) => read,
                    Err(failure) => {
                        std::hint::black_box(&failure);
                        panic!("next actual no-writer settlement refused")
                    }
                };
                let completed = outside.finish_owned(settled);
                next = Some(completed_facts(&completed, &clock));
                drop(completed); // Completed facts are retained; no reservation cleanup here.
                let teardown = TeardownLease::try_acquire(Arc::clone(&root))?;
                teardown.revalidate()?;
                drop(teardown);
                assert!(stages(&input.journal)?.is_empty());
                cut(&mut control, b"nextdone")?;
                std::hint::black_box(&report); // Resource-free actual report stays alive.
                facts
            }
            OwnedOffloadCreationFinish::Held(held) => {
                assert_ne!(input.case, OwnedCase::Complete);
                assert!(Arc::ptr_eq(held.carrier(), &carrier));
                assert_eq!(held.source_copies().count(), selection.names.len());
                for (actual, before) in held.source_copies().zip(&source_copies) {
                    assert_eq!(serde_json::to_value(actual.identity())?, before["identity"]);
                    assert_eq!(serde_json::to_value(actual.digest())?, before["digest"]);
                }
                let cleanups = held
                    .source_cleanups()
                    .iter()
                    .map(|r| json!({"index":r.index,"cleanup":r.cleanup}))
                    .collect::<Vec<_>>();
                match input.case {
                    OwnedCase::PublicationUncertain => {
                        assert!(matches!(
                            held.finish_refusal(),
                            Some(OwnedOffloadCreationFinishRefusal::IncompleteCreation)
                        ));
                        assert!(matches!(
                            held.reason(),
                            Some(OffloadCreationRefusal::Publication)
                        ));
                        assert!(held.source_cleanups().is_empty());
                        let offers = held.offers().expect("actual whole offer hold");
                        let Some(Ok(BoundMutationOutcome::PublishedUncertain {
                            uncertainty, ..
                        })) = offers.publication()
                        else {
                            panic!("actual uncertain candidate lost")
                        };
                        assert_eq!(
                            uncertainty
                                .durability
                                .as_ref()
                                .and_then(io::Error::raw_os_error),
                            Some(13)
                        );
                        assert_eq!(copies(&input.journal, held.source_copies())?, source_copies);
                    }
                    OwnedCase::SecondCleanup => {
                        assert!(matches!(
                            held.finish_refusal(),
                            Some(OwnedOffloadCreationFinishRefusal::Cleanup(Cleanup::Pending))
                        ));
                        assert!(held.read().is_some() && held.settled_offers().is_some());
                        assert_eq!(
                            held.source_cleanups()
                                .iter()
                                .map(|r| (r.index, r.cleanup))
                                .collect::<Vec<_>>(),
                            vec![(0, Cleanup::Complete), (1, Cleanup::Pending)]
                        );
                        assert!(
                            !Path::new(source_copies[0]["stage"].as_str().unwrap()).try_exists()?
                        );
                        assert!(
                            !Path::new(source_copies[1]["path"].as_str().unwrap()).try_exists()?
                        );
                        let second = Path::new(source_copies[1]["stage"].as_str().unwrap());
                        assert_eq!(directory_fact(second)?, source_copies[1]["stage_identity"]);
                        assert_eq!(fs::read_dir(second)?.count(), 0);
                        let third = Path::new(source_copies[2]["path"].as_str().unwrap());
                        assert_eq!(file_fact(third)?, source_copies[2]["fact"]);
                        assert_eq!(
                            directory_fact(third.parent().unwrap())?,
                            source_copies[2]["stage_identity"]
                        );
                    }
                    OwnedCase::Obligation => {
                        assert!(held.read().is_some() && held.settled_offers().is_some());
                        assert_eq!(
                            held.source_cleanups()
                                .iter()
                                .map(|r| r.cleanup)
                                .collect::<Vec<_>>(),
                            vec![Cleanup::Complete; 2]
                        );
                        for copy in &source_copies {
                            assert!(!Path::new(copy["stage"].as_str().unwrap()).try_exists()?);
                        }
                        let stage = foreign_stage.as_ref().expect("actual distinct stage");
                        let Some(OwnedOffloadCreationFinishRefusal::Obligations(obligations)) =
                            held.finish_refusal()
                        else {
                            panic!("final actual census gate absent")
                        };
                        let relative = stage
                            .diagnostic_path()
                            .strip_prefix(&input.journal)?
                            .to_path_buf();
                        assert!(obligations.iter().any(
                            |v| matches!(v,NativeObligation::Stage{relative:r,..} if r==&relative)
                        ));
                        assert_eq!(
                            directory_fact(&stage.diagnostic_path())?,
                            foreign_fact.as_ref().unwrap()["identity"]
                        );
                    }
                    OwnedCase::Complete => unreachable!(),
                }
                assert_eq!(
                    directory_fact(&original_carrier)?,
                    original_carrier_identity
                );
                assert!(ActivityLease::try_acquire(Arc::clone(&root)).is_err());
                assert!(TeardownLease::try_acquire(Arc::clone(&root)).is_err());
                let facts = json!({"kind":"held","refusal":format!("{:?}",held.finish_refusal()),"reason":format!("{:?}",held.reason()),
                    "source_cleanups":cleanups,"settled_report":held.settled_offers().map(settled_facts),"whole_offers":held.offers().map(|o|format!("{:?}",o.publication()))});
                cut(&mut control, b"finished")?;
                let (held, outside, stage) =
                    std::thread::spawn(move || (held, outside, foreign_stage))
                        .join()
                        .expect("actual closed custody thread joined");
                assert!(Arc::ptr_eq(held.carrier(), &carrier));
                assert!(Arc::ptr_eq(outside.carrier(), &carrier));
                assert!(Arc::ptr_eq(outside.owned_clock(), &clock));
                held_custody = Some((held, outside, stage));
                facts
            }
        };
        for (index, name) in selection.names.iter().enumerate() {
            assert_eq!(file_fact(&raw_dir.join(name))?, raw[index]);
            assert_eq!(
                fs::read(raw_dir.join(name).with_extension("jsonl"))?,
                headers[index]
            );
        }
        assert_eq!(fs::read(input.journal.join("config/journal.json"))?, config);
        assert_eq!(fs::read(&register_path)?, register_before_finish);
        assert_eq!(fs::read(&ledger_path)?, ledger_before_finish);
        cut(&mut control, b"returned")?;
        println!(
            "OWNED_CREATION_PROBE_JSON={}",
            serde_json::to_string(&json!({"case":input.case,"harness_pid":std::process::id(),
            "carrier":original_carrier,"carrier_identity":original_carrier_identity,"original_clock":true,"raw":raw,"marks":marks,
            "archives":archives,"ledger":ledger,"reads":reads,"source_copies":source_copies,"publication":publication,
            "publication_fault":publication_fault,"cleanup_fault":cleanup_fault,"finish":finish_facts,"completion":completion,"next":next,
            "foreign":foreign_fact,"closed_thread_joined":held_custody.is_some(),"stages":stages(&input.journal)?,
            "limits":"exact historical-package-adapted synthetic local Linux owned creator/finish only; no activation/provider/route/recovery claim"}))?
        );
        std::hint::black_box(&held_custody);
        std::hint::black_box(&cancellation);
        Ok(())
    }
}

mod owned_planner_qualification {
    use super::*;
    use crate::run::{
        CurrentOffloadAvailability, OffloadLedgerEffect, OffloadLedgerSync, OffloadLedgerWrite,
        OffloadPlanning, OffloadPlanningPhase, OffloadPlanningReport, OwnedOffloadCreationOwner,
        OwnedOffloadPlanFinish, OwnedOffloadPlanFinishRefusal, OwnedOffloadPlanOwner,
        create_owned_planned_offload_offers, prepare_owned_offload_plan,
    };
    use chrono::Datelike;
    use solstone_core_backup::archive_activity::OwnedPreparedArchiveOperation;
    use solstone_core_backup_runtime::{
        DirectReadFinishState, OwnedDirectReadFinish,
        admit_prepared_archive_owned_creator_installed,
    };
    use solstone_core_journal_io::activity::{ActivityLease, TeardownLease};
    use solstone_core_journal_io::activity_obligations::NativeObligation;
    use solstone_core_journal_io::media::{
        MediaDirectory, MediaIdentity, MediaStage, PreparedMedia,
    };
    use solstone_core_retention::archive_offer::OffloadReconciliationDisposition;
    use std::sync::Arc;

    #[derive(Clone, Copy, Debug, Deserialize, Serialize, Eq, PartialEq)]
    enum PlannerCase {
        #[serde(rename = "owned_planner_needed_complete")]
        Needed,
        #[serde(rename = "owned_planner_metadata_complete")]
        Metadata,
        #[serde(rename = "owned_planner_metadata_final_obligation")]
        Obligation,
    }
    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct PlannerInput {
        case: PlannerCase,
        journal: PathBuf,
        repository_id: String,
        expected_restic_path: PathBuf,
        control: PathBuf,
    }
    struct ActualClock;
    impl Clock for ActualClock {
        fn now_unix(&self) -> i64 {
            Utc::now().timestamp()
        }
        fn iso_week(&self) -> u8 {
            Utc::now().iso_week().week() as u8
        }
    }
    fn directory_fact(path: &Path) -> TestResult<Value> {
        let m = fs::symlink_metadata(path)?;
        assert!(m.is_dir() && !m.file_type().is_symlink());
        Ok(json!({"device":m.dev(),"inode":m.ino()}))
    }
    fn copy_facts<'a>(
        journal: &Path,
        copies: impl Iterator<Item = &'a PreparedMedia>,
    ) -> TestResult<Vec<Value>> {
        copies.map(|media| {
            let mut found = Vec::new();
            for stage in stages(journal)? {
                for leaf in fs::read_dir(&stage)? {
                    let leaf=leaf?.path(); let m=fs::symlink_metadata(&leaf)?;
                    if media.identity()==(MediaIdentity::Unix{device:m.dev(),inode:m.ino()}) {
                        found.push((stage.clone(),leaf));
                    }
                }
            }
            assert_eq!(found.len(),1);
            let (stage,leaf)=&found[0]; let fact=file_fact(leaf)?;
            assert_eq!(fact["bytes"],media.digest().size);
            assert_eq!(fact["sha256"],media.digest().sha256);
            Ok(json!({"name":leaf.file_name().unwrap().to_string_lossy(),"identity":media.identity(),
                "digest":media.digest(),"stage":stage,"stage_identity":directory_fact(stage)?,"path":leaf,"fact":fact}))
        }).collect()
    }
    fn ledger_facts<'a>(writes: impl Iterator<Item = OffloadLedgerWrite<'a>>) -> Vec<Value> {
        writes.map(|write| match write {
            OffloadLedgerWrite::Recorded{effect,durability,confirmation,failures} => {
                assert!(matches!(effect,OffloadLedgerEffect::Appended{bytes} if bytes>0));
                assert_eq!(durability,[OffloadLedgerSync::Confirmed;4]);
                assert_eq!(confirmation,OffloadLedgerSync::Confirmed); assert!(failures.is_empty());
                json!({"effect":format!("{effect:?}"),"durability":durability.map(|v|format!("{v:?}")),"confirmation":format!("{confirmation:?}"),"failures":failures})
            }
            OffloadLedgerWrite::Refused{reason,detail} => panic!("actual writer refused: {reason} {detail:?}"),
        }).collect()
    }
    fn projection_fact(
        local: &crate::measurement::RawMediaPopulation,
        projection: &crate::measurement::ProposedRelief,
    ) -> TestResult<Value> {
        assert!(local.complete());
        assert_eq!(local.usage().map_err(invalid)?, projection.local);
        Ok(
            json!({"local":projection.local,"eligible_bytes":projection.eligible_bytes,"eligible_files":projection.eligible_files,
            "projected_local_bytes":projection.projected_local_bytes}),
        )
    }
    fn planning_fact(
        planning: &OffloadPlanning,
        root: &JournalRoot,
        needed: bool,
    ) -> TestResult<Value> {
        let reconciliation = planning
            .reconciliation()
            .expect("actual reconciliation")
            .as_ref()
            .unwrap_or_else(|failure| {
                panic!("actual reconciliation failed: {:?}", failure.reason())
            });
        assert_eq!(reconciliation.journal_root().identity(), root.identity());
        assert!(
            reconciliation
                .groups()
                .iter()
                .all(|g| matches!(g.disposition(), OffloadReconciliationDisposition::Unchanged))
        );
        let decision = planning.decision().expect("actual current decision");
        assert_eq!(
            decision.availability(),
            CurrentOffloadAvailability::Available
        );
        assert_eq!(
            decision.root_observation(),
            root.identity().observation_id()
        );
        assert!(decision.refusals().is_empty());
        let eligible=decision.eligible().map(|e|json!({"name":e.name(),"bytes":e.bytes(),"witness":e.witness_id(),"mark":e.mark()})).collect::<Vec<_>>();
        assert_eq!(eligible.len(), if needed { 0 } else { 2 });
        if !needed {
            assert_eq!(
                eligible
                    .iter()
                    .map(|v| v["name"].as_str().unwrap())
                    .collect::<Vec<_>>(),
                vec!["file1.flac", "file2.flac"]
            );
        }
        Ok(
            json!({"phase":format!("{:?}",planning.phase()),"budget_bytes":planning.budget_bytes(),
            "projection":projection_fact(planning.local().expect("complete L"),planning.initial_projection().expect("actual P projection"))?,
            "eligible":eligible,"root":decision.root_observation(),"reconciliation_register":reconciliation.register(),
            "reconciliation_groups":reconciliation.groups().len()}),
        )
    }
    fn report_fact(report: &OffloadPlanningReport) -> TestResult<Value> {
        let decision = report.decision().expect("actual current report");
        assert_eq!(
            decision.availability(),
            CurrentOffloadAvailability::Available
        );
        assert!(decision.refusals().is_empty());
        assert_eq!(
            report.reconciliation().root_observation(),
            decision.root_observation()
        );
        assert!(
            report
                .reconciliation()
                .groups()
                .iter()
                .all(|g| matches!(g.disposition(), OffloadReconciliationDisposition::Unchanged))
        );
        Ok(
            json!({"phase":format!("{:?}",report.phase()),"budget_bytes":report.budget_bytes(),
            "projection":projection_fact(report.local().unwrap(),report.initial_projection().unwrap())?,"root":decision.root_observation(),
            "eligible":decision.eligible().map(|e|json!({"name":e.name(),"bytes":e.bytes(),"witness":e.witness_id(),"mark":e.mark()})).collect::<Vec<_>>(),
            "reconciliation_register":report.reconciliation().register(),"reconciliation_groups":report.reconciliation().groups().len()}),
        )
    }
    fn completed_fact(
        finish: &OwnedDirectReadFinish,
        clock: &Arc<dyn Clock + Send + Sync>,
    ) -> Value {
        assert_eq!(finish.state(), DirectReadFinishState::Completed);
        assert!(finish.reason().is_none());
        assert!(Arc::ptr_eq(finish.owned_clock(), clock));
        let retired = finish.retirement().expect("actual retirement");
        assert!(retired.receipt.failures().is_empty());
        json!({"state":format!("{:?}",finish.state()),"root":finish.journal_root().identity().observation_id(),
            "carrier":retired.owned.diagnostic_path(),"effect":format!("{:?}",retired.receipt.effect()),
            "root_durability":format!("{:?}",retired.receipt.root_durability()),"confirmation":format!("{:?}",retired.receipt.confirmation()),
            "failures":retired.receipt.failures().iter().map(|e|e.to_string()).collect::<Vec<_>>()})
    }

    #[test]
    #[ignore = "requires exact copied package and original owned planner/finish S3/process fixture"]
    fn packaged_owned_planner_finish_entry() -> TestResult {
        let input: PlannerInput = serde_json::from_slice(&fs::read(
            std::env::var_os("OFFLOAD_OWNED_PLANNER_PROBE_JSON")
                .ok_or_else(|| invalid("explicit owned planner input required"))?,
        )?)?;
        let needed = input.case == PlannerCase::Needed;
        let names = if needed {
            vec!["file1.flac", "file2.flac"]
        } else {
            vec!["file1.flac", "file2.flac", "file3.flac"]
        };
        let root = Arc::new(JournalRoot::open(&input.journal)?);
        assert!(stages(&input.journal)?.is_empty());
        let root_identity = root.identity().observation_id();
        let register_path = input.journal.join("health/retention-marks.json");
        let ledger_path = input.journal.join("health/offload/20261009.jsonl");
        let before_register = register_path
            .try_exists()?
            .then(|| fs::read(&register_path))
            .transpose()?;
        let before_ledger = ledger_path
            .try_exists()?
            .then(|| fs::read(&ledger_path))
            .transpose()?;
        assert_eq!(before_register.is_none(), needed);
        assert_eq!(before_ledger.is_none(), needed);
        let raw_dir = input.journal.join("chronicle/20261009/120000_60_fixture");
        let raw = names
            .iter()
            .map(|n| file_fact(&raw_dir.join(n)))
            .collect::<TestResult<Vec<_>>>()?;
        let headers = names
            .iter()
            .map(|n| fs::read(raw_dir.join(n).with_extension("jsonl")))
            .collect::<io::Result<Vec<_>>>()?;
        let config = fs::read(input.journal.join("config/journal.json"))?;
        let clock: Arc<dyn Clock + Send + Sync> = Arc::new(ActualClock);
        let cancellation = ArchiveCancellation::new();
        let budget = ArchiveBudget::new_cancellable(clock.as_ref(), cancellation.clone())?;
        let original = match OwnedPreparedArchiveOperation::prepare(
            Arc::clone(&root),
            budget,
            Arc::clone(&clock),
        ) {
            Ok(original) => original,
            Err(failure) => {
                std::hint::black_box(&failure);
                panic!("actual original prepare refused: {:?}", failure.reason())
            }
        };
        let carrier = Arc::clone(original.carrier());
        let carrier_path = carrier.diagnostic_path();
        let carrier_identity = directory_fact(&carrier_path)?;
        let mut control = UnixStream::connect(&input.control)?;
        control.write_all(b"prepared")?;
        control.write_all(&std::process::id().to_be_bytes())?;
        let mut ack = [0];
        control.read_exact(&mut ack)?;
        assert_eq!(ack, [b'a']);
        let inputs = match prepare_owned_offload_plan(&original, Utc::now().fixed_offset()) {
            Ok(inputs) => inputs,
            Err(failure) => {
                let closed = original.into_owned_hold();
                assert!(Arc::ptr_eq(closed.carrier(), failure.carrier()));
                let reason = format!("{:?}", failure.reason());
                std::hint::black_box(&(closed, failure));
                panic!("actual preflight refused before admission: {reason}")
            }
        };
        assert!(Arc::ptr_eq(inputs.carrier(), &carrier));
        assert_eq!(
            inputs.planning().budget_bytes(),
            Some(if needed { 0 } else { 2097162 })
        );
        let preflight_usage = inputs
            .planning()
            .local()
            .unwrap()
            .usage()
            .map_err(invalid)?;
        assert_eq!(preflight_usage.total_bytes, 2097162 * names.len() as u64);
        assert_eq!(preflight_usage.total_files, names.len() as u64);
        cut(&mut control, b"captured")?;
        let installed = match admit_prepared_archive_owned_creator_installed(
            original,
            &input.expected_restic_path,
            None,
            "test",
            &solstone_core_backup_runtime::UreqHttpTransport,
        ) {
            Ok(installed) => installed,
            Err(failure) => {
                std::hint::black_box(&failure);
                panic!("actual installed admission refused: {:?}", failure.error())
            }
        };
        let (admitted, outside) = installed.into_parts();
        assert!(Arc::ptr_eq(admitted.carrier(), &carrier));
        assert!(Arc::ptr_eq(outside.carrier(), &carrier));
        assert!(Arc::ptr_eq(outside.owned_clock(), &clock));
        assert_eq!(admitted.repository_id(), input.repository_id);
        cut(&mut control, b"admitted")?;
        let planned = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            create_owned_planned_offload_offers(admitted, inputs)
        }));
        let plan = match planned {
            Ok(plan) => plan,
            Err(error) => {
                let unknown = outside.into_unknown();
                std::hint::black_box(&unknown);
                std::panic::resume_unwind(error)
            }
        };
        assert!(Arc::ptr_eq(plan.carrier(), &carrier));
        assert!(
            plan.reason().is_none(),
            "actual planning refusal: {:?}",
            plan.reason()
        );
        assert!(plan.capture_failure().is_none());
        let planning = planning_fact(plan.planning(), &root, needed)?;
        let projection = plan.planning().initial_projection().unwrap();
        assert_eq!(projection.eligible_bytes, if needed { 0 } else { 4194324 });
        assert_eq!(projection.eligible_files, if needed { 0 } else { 2 });
        assert_eq!(
            projection.projected_local_bytes,
            if needed { 4194324 } else { 2097162 }
        );
        if !needed {
            assert!(projection.local.total_bytes > plan.planning().budget_bytes().unwrap());
            assert!(projection.projected_local_bytes <= plan.planning().budget_bytes().unwrap());
        }
        let mut source_copies = Vec::new();
        let mut reads = Vec::new();
        let mut archives = Vec::new();
        let mut ledger = Vec::new();
        let marks;
        match plan.owner() {
            OwnedOffloadPlanOwner::Creation(creation) => {
                assert!(needed);
                assert_eq!(plan.planning().phase(), OffloadPlanningPhase::Held);
                assert!(creation.reason().is_none());
                assert!(creation.partial_copy().is_none());
                let OwnedOffloadCreationOwner::Published(outcome) = creation.owner() else {
                    panic!("genuine creation not published")
                };
                let Ok(BoundMutationOutcome::Published(published)) = outcome.publication() else {
                    panic!("genuine publication not confirmed")
                };
                assert_eq!(published.status, MutationStatus::Applied);
                let verified = outcome.verified();
                marks = verified.marks().to_vec();
                assert_eq!(marks.len(), 1);
                assert_eq!(marks[0].proposal.names, vec!["file1.flac", "file2.flac"]);
                assert_eq!(marks[0].proposal.bytes, 4194324);
                let proof = &marks[0].offload.as_ref().unwrap().proof;
                assert_eq!(proof.repository_id, input.repository_id);
                assert_eq!(proof.files.len(), 2);
                let catalog = read_complete_catalog(&root).map_err(invalid)?;
                for (index, file) in proof.files.iter().enumerate() {
                    assert!(catalog.active_witness(proof, file).is_some());
                    assert_eq!(file.name, names[index]);
                    assert_eq!(file.bytes, raw[index]["bytes"]);
                    assert_eq!(file.sha256, raw[index]["sha256"]);
                }
                reads=verified.archive_reads().map(|read|{let ArchiveReadState::Prepared{file,target_identity,media,cleanup}=read else{panic!("original prepared read missing")};
                    assert_eq!(cleanup,Some(Cleanup::Complete));assert_eq!(*target_identity,media.identity());json!({"name":file.name,"repository":file.repository_id,"snapshot":file.snapshot_id,"path":file.archive_path,"identity":target_identity,"digest":media.digest(),"cleanup":cleanup})}).collect();
                assert_eq!(reads.len(), 2);
                ledger = ledger_facts(creation.ledger_writes());
                assert_eq!(ledger.len(), 1);
                archives = creation
                    .archives()
                    .iter()
                    .map(|r| match r {
                        Ok(s) => json!({"repository":s.repository_id,"snapshot":s.snapshot_id}),
                        Err(e) => panic!("actual archive failed: {e:?}"),
                    })
                    .collect();
                assert_eq!(archives.len(), 1);
                source_copies = copy_facts(&input.journal, creation.source_copies())?;
                assert_eq!(source_copies.len(), 2);
            }
            OwnedOffloadPlanOwner::Read(read) => {
                assert!(!needed);
                assert_eq!(
                    plan.planning().phase(),
                    OffloadPlanningPhase::AlreadySatisfied
                );
                assert!(Arc::ptr_eq(read.carrier(), &carrier));
                assert!(Arc::ptr_eq(read.owned_clock(), &clock));
                let register: marks::Register =
                    serde_json::from_slice(before_register.as_ref().unwrap())?;
                marks = register.marks.into_values().collect::<Vec<_>>();
                assert_eq!(marks.len(), 1);
                assert_eq!(marks[0].proposal.names, vec!["file1.flac", "file2.flac"]);
                assert_eq!(
                    fs::read(&register_path)?,
                    *before_register.as_ref().unwrap()
                );
                assert_eq!(fs::read(&ledger_path)?, *before_ledger.as_ref().unwrap());
            }
            _ => panic!("actual plan has wrong concrete owner"),
        }
        let register_before_finish = fs::read(&register_path)?;
        let ledger_before_finish = fs::read(&ledger_path)?;
        cut(&mut control, b"planned_")?;
        let mut foreign_stage = None;
        let mut foreign_fact = None;
        if input.case == PlannerCase::Obligation {
            let parent = MediaDirectory::open(Arc::clone(&root), Path::new(""))?;
            let stage = match MediaStage::create(parent, Arc::clone(carrier.activity())) {
                Ok(stage) => stage,
                Err(failure) => {
                    std::hint::black_box(&failure);
                    panic!("actual distinct stage creation failed")
                }
            };
            foreign_fact = Some(
                json!({"path":stage.diagnostic_path(),"identity":directory_fact(&stage.diagnostic_path())?}),
            );
            foreign_stage = Some(stage);
        }
        let finished = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| plan.finish()));
        let finished = match finished {
            Ok(finished) => finished,
            Err(error) => {
                let unknown = outside.into_unknown();
                std::hint::black_box(&unknown);
                std::panic::resume_unwind(error)
            }
        };
        let mut completion = None;
        let mut next = None;
        let mut held_custody = None;
        let finish_fact = match finished {
            OwnedOffloadPlanFinish::Settled(settled) => {
                assert_ne!(input.case, PlannerCase::Obligation);
                let (read, report) = settled.into_parts();
                assert!(Arc::ptr_eq(read.carrier(), &carrier));
                assert!(Arc::ptr_eq(read.owned_clock(), &clock));
                let final_planning = report_fact(report.planning())?;
                let mut expected_planning = planning.clone();
                if needed {
                    assert_eq!(
                        report.planning().phase(),
                        OffloadPlanningPhase::OffersPublished
                    );
                    expected_planning["phase"] = json!("OffersPublished");
                }
                assert_eq!(final_planning, expected_planning);
                let source_cleanup = if needed {
                    let creation = report.creation().expect("actual settled creation report");
                    assert_eq!(creation.source_copies().len(), 2);
                    creation.source_copies().iter().zip(&source_copies).map(|(actual,before)|{
                        assert_eq!(actual.cleanup,Cleanup::Complete);assert_eq!(serde_json::to_value(actual.identity).unwrap(),before["identity"]);assert_eq!(serde_json::to_value(&actual.digest).unwrap(),before["digest"]);
                        assert!(!Path::new(before["stage"].as_str().unwrap()).try_exists().unwrap());json!({"identity":actual.identity,"digest":actual.digest,"cleanup":actual.cleanup})}).collect::<Vec<_>>()
                } else {
                    assert!(report.creation().is_none());
                    Vec::new()
                };
                check_carrier(&carrier)?;
                assert_eq!(stages(&input.journal)?.len(), 1);
                cut(&mut control, b"finished")?;
                drop(carrier);
                let completed = outside.finish_owned(read);
                completion = Some(completed_fact(&completed, &clock));
                assert_eq!(completion.as_ref().unwrap()["root"], root_identity);
                assert!(stages(&input.journal)?.is_empty());
                drop(completed);
                cut(&mut control, b"retired_")?;
                if input.case == PlannerCase::Metadata {
                    let next_clock: Arc<dyn Clock + Send + Sync> = Arc::new(ActualClock);
                    let budget = ArchiveBudget::new_cancellable(
                        next_clock.as_ref(),
                        ArchiveCancellation::new(),
                    )?;
                    let original = match OwnedPreparedArchiveOperation::prepare(
                        Arc::clone(&root),
                        budget,
                        Arc::clone(&next_clock),
                    ) {
                        Ok(original) => original,
                        Err(failure) => {
                            std::hint::black_box(&failure);
                            panic!("next actual original refused")
                        }
                    };
                    let installed = match admit_prepared_archive_owned_creator_installed(
                        original,
                        &input.expected_restic_path,
                        None,
                        "test",
                        &solstone_core_backup_runtime::UreqHttpTransport,
                    ) {
                        Ok(installed) => installed,
                        Err(failure) => {
                            std::hint::black_box(&failure);
                            panic!("next actual installed admission refused")
                        }
                    };
                    let (admitted, outside) = installed.into_parts();
                    let read = match admitted.into_read_operation() {
                        Ok(read) => read,
                        Err(failure) => {
                            std::hint::black_box(&failure);
                            panic!("next genuine NoWriter handoff refused")
                        }
                    };
                    let settled = match read.settle_direct() {
                        Ok(settled) => settled,
                        Err(failure) => {
                            std::hint::black_box(&failure);
                            panic!("next real settlement refused")
                        }
                    };
                    let completed = outside.finish_owned(settled);
                    next = Some(completed_fact(&completed, &next_clock));
                    drop(completed);
                    let teardown = TeardownLease::try_acquire(Arc::clone(&root))?;
                    teardown.revalidate()?;
                    drop(teardown);
                    assert!(stages(&input.journal)?.is_empty());
                    cut(&mut control, b"nextdone")?;
                }
                std::hint::black_box(&report);
                json!({"kind":"settled","planning":final_planning,"creation_present":report.creation().is_some(),"source_cleanups":source_cleanup})
            }
            OwnedOffloadPlanFinish::Held(held) => {
                assert_eq!(input.case, PlannerCase::Obligation);
                assert!(Arc::ptr_eq(held.carrier(), &carrier));
                assert!(held.metadata_settled());
                assert!(held.settlement_failure().is_none());
                assert!(held.creation().is_none());
                assert!(held.settled_creation().is_none());
                assert!(held.reason().is_none());
                assert!(held.capture_failure().is_none());
                let read = held.read().expect("actual settled-phase read retained");
                assert!(Arc::ptr_eq(read.carrier(), &carrier));
                assert!(Arc::ptr_eq(read.owned_clock(), &clock));
                assert_eq!(read.journal_root().identity(), root.identity());
                assert_eq!(planning_fact(held.planning(), &root, false)?, planning);
                let stage = foreign_stage.as_ref().unwrap();
                let relative = stage
                    .diagnostic_path()
                    .strip_prefix(&input.journal)?
                    .to_path_buf();
                let Some(OwnedOffloadPlanFinishRefusal::Obligations(obligations)) =
                    held.finish_refusal()
                else {
                    panic!("actual late census refusal absent")
                };
                assert!(
                    obligations.iter().any(
                        |v| matches!(v,NativeObligation::Stage{relative:r,..} if r==&relative)
                    )
                );
                assert_eq!(
                    directory_fact(&stage.diagnostic_path())?,
                    foreign_fact.as_ref().unwrap()["identity"]
                );
                assert_eq!(directory_fact(&carrier_path)?, carrier_identity);
                let mut expected_stages = vec![carrier_path.clone(), stage.diagnostic_path()];
                expected_stages.sort();
                assert_eq!(stages(&input.journal)?, expected_stages);
                carrier.revalidate()?;
                assert_eq!(fs::read_dir(&carrier_path)?.count(), 0);
                assert_eq!(fs::read_dir(stage.diagnostic_path())?.count(), 0);
                assert!(ActivityLease::try_acquire(Arc::clone(&root)).is_err());
                assert!(TeardownLease::try_acquire(Arc::clone(&root)).is_err());
                let fact = json!({"kind":"held","planning":planning,"refusal":format!("{:?}",held.finish_refusal()),"metadata_settled":held.metadata_settled(),"settlement_failure_absent":true,"creation_present":false,"read_retained":true});
                cut(&mut control, b"finished")?;
                let (held, outside, stage) =
                    std::thread::spawn(move || (held, outside, foreign_stage))
                        .join()
                        .expect("actual metadata closed custody thread joined");
                assert!(Arc::ptr_eq(held.carrier(), &carrier));
                assert!(Arc::ptr_eq(outside.carrier(), &carrier));
                assert!(Arc::ptr_eq(outside.owned_clock(), &clock));
                held_custody = Some((held, outside, stage));
                fact
            }
        };
        for (index, name) in names.iter().enumerate() {
            assert_eq!(file_fact(&raw_dir.join(name))?, raw[index]);
            assert_eq!(
                fs::read(raw_dir.join(name).with_extension("jsonl"))?,
                headers[index]
            );
        }
        assert_eq!(fs::read(input.journal.join("config/journal.json"))?, config);
        assert_eq!(fs::read(&register_path)?, register_before_finish);
        assert_eq!(fs::read(&ledger_path)?, ledger_before_finish);
        cut(&mut control, b"returned")?;
        println!(
            "OWNED_PLANNER_PROBE_JSON={}",
            serde_json::to_string(
                &json!({"case":input.case,"harness_pid":std::process::id(),"root":root_identity,
            "carrier":carrier_path,"carrier_identity":carrier_identity,"original_clock":true,"raw":raw,"marks":marks,"planning":planning,
            "archives":archives,"ledger":ledger,"reads":reads,"source_copies":source_copies,"finish":finish_fact,"completion":completion,"next":next,
            "foreign":foreign_fact,"closed_thread_joined":held_custody.is_some(),"stages":stages(&input.journal)?,
            "limits":"historical-package-adapted synthetic local Linux owned planner only; no activation/provider/route/recovery claim"})
            )?
        );
        std::hint::black_box(&held_custody);
        std::hint::black_box(&cancellation);
        Ok(())
    }
}
