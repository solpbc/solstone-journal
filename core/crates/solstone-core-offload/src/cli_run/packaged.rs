// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Ignored installed coordinator controls; barriers observe, never authorize.

use super::*;
use serde::Deserialize;
use serde_json::{Value, json};
use solstone_core_journal_io::activity::ActivityLease;
use solstone_core_journal_io::media::Cleanup;
use std::cell::RefCell;
use std::fs;
use std::io::{Read, Write};
use std::os::unix::fs::MetadataExt;
use std::os::unix::net::UnixStream;
use std::path::PathBuf;

type TestResult<T = ()> = Result<T, Box<dyn std::error::Error>>;
thread_local! { static CONTROL: RefCell<Option<UnixStream>> = const { RefCell::new(None) }; }

fn cut(stream: &mut UnixStream, label: &[u8; 8], first: bool) -> io::Result<()> {
    stream.write_all(label)?;
    if first {
        stream.write_all(&std::process::id().to_be_bytes())?;
    }
    let mut ack = [0];
    stream.read_exact(&mut ack)?;
    assert_eq!(ack, [b'a']);
    Ok(())
}

pub(super) fn observe(phase: CliUnwindPhase) {
    let (label, first): (&[u8; 8], bool) = match phase {
        CliUnwindPhase::Preparation => return,
        CliUnwindPhase::Preflight => (b"prepared", true),
        CliUnwindPhase::Admission => (b"captured", false),
        CliUnwindPhase::Actor => (b"admitted", false),
        CliUnwindPhase::PlanFinish => (b"created_", false),
        CliUnwindPhase::RuntimeFinish => (b"finished", false),
    };
    CONTROL.with(|slot| {
        if let Some(stream) = slot.borrow_mut().as_mut() {
            cut(stream, label, first).expect("actual CLI barrier acknowledgement");
        }
    });
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Input {
    case: String,
    journal: PathBuf,
    repository_id: String,
    expected_restic_path: PathBuf,
    control: PathBuf,
}
struct ActualClock;
impl Clock for ActualClock {
    fn now_unix(&self) -> i64 {
        jiff::Timestamp::now().as_second()
    }
    fn iso_week(&self) -> u8 {
        jiff::tz::Offset::UTC
            .to_datetime(jiff::Timestamp::now())
            .date()
            .iso_week_date()
            .week()
            .unsigned_abs()
    }
}
fn stages(root: &Path) -> io::Result<Vec<PathBuf>> {
    let mut stages = fs::read_dir(root)?
        .map(|e| e.map(|e| e.path()))
        .collect::<io::Result<Vec<_>>>()?;
    stages.retain(|p| {
        p.file_name()
            .unwrap()
            .as_encoded_bytes()
            .starts_with(b".tmp-offload-")
    });
    stages.sort();
    Ok(stages)
}
fn file_fact(path: &Path) -> TestResult<Value> {
    use sha2::{Digest, Sha256};
    let m = fs::symlink_metadata(path)?;
    assert!(m.is_file() && !m.file_type().is_symlink());
    Ok(
        json!({"device":m.dev(),"inode":m.ino(),"bytes":m.len(),"sha256":format!("{:x}",Sha256::digest(fs::read(path)?))}),
    )
}

#[cfg(all(test, feature = "full-tests"))]
#[test]
#[ignore = "requires pinned copied installed package and observed synthetic S3 fixture"]
fn packaged_owned_cli_finish_entry() -> TestResult {
    let input: Input = serde_json::from_slice(&fs::read(
        std::env::var_os("OFFLOAD_OWNED_CLI_PROBE_JSON")
            .ok_or_else(|| io::Error::other("explicit CLI probe input required"))?,
    )?)?;
    let positive = match input.case.as_str() {
        "cli_complete" => true,
        "cli_finish_unknown" => false,
        _ => return Err(io::Error::other("unknown CLI probe case").into()),
    };
    let exe = std::env::current_exe()?;
    assert_eq!(
        resolve_restic_member(&exe).unwrap(),
        input.expected_restic_path
    );
    let root = Arc::new(JournalRoot::open(&input.journal)?);
    assert!(stages(&input.journal)?.is_empty());
    let raw_dir = input.journal.join("chronicle/20261009/120000_60_fixture");
    let raw = ["file1.flac", "file2.flac"]
        .iter()
        .map(|name| file_fact(&raw_dir.join(name)))
        .collect::<TestResult<Vec<_>>>()?;
    let headers = ["file1.jsonl", "file2.jsonl"]
        .iter()
        .map(|name| fs::read(raw_dir.join(name)))
        .collect::<io::Result<Vec<_>>>()?;
    CONTROL.with(|slot| *slot.borrow_mut() = Some(UnixStream::connect(&input.control).unwrap()));
    if !positive {
        PANIC_AT.with(|slot| slot.set(Some(CliUnwindPhase::RuntimeFinish)));
    }
    let clock: Arc<dyn Clock + Send + Sync> = Arc::new(ActualClock);
    let run = run_cli_offload(&input.journal, Arc::clone(&clock), false);
    // The whole concrete aggregate crosses this boundary before any report is borrowed.
    let run = std::thread::spawn(move || run)
        .join()
        .expect("closed CLI result thread joined");
    let custody = run
        .preparation_custody()
        .expect("actual original preparation custody");
    custody.revalidate()?;
    assert!(Arc::ptr_eq(custody.owned_clock(), &clock));
    assert_eq!(custody.journal_root().identity(), root.identity());
    assert!(ActivityLease::try_acquire(Arc::clone(&root)).is_err());
    let report = run
        .settled_report()
        .expect("actual settled report survives receiving boundary");
    assert_eq!(
        report.planning().phase(),
        crate::run::OffloadPlanningPhase::OffersPublished
    );
    let creation = report.creation().expect("genuine creation report");
    let marks = creation.offers().marks().to_vec();
    assert_eq!(marks.len(), 1);
    let proof = &marks[0].offload.as_ref().unwrap().proof;
    assert_eq!(proof.repository_id, input.repository_id);
    assert_eq!(proof.files.len(), 2);
    let copies = creation.source_copies().iter().zip(&proof.files).map(|(copy,file)| {
        assert_eq!(copy.cleanup, Cleanup::Complete);
        assert_eq!(copy.digest.size, file.bytes);
        assert_eq!(copy.digest.sha256, file.sha256);
        json!({"name":file.name,"identity":copy.identity,"digest":copy.digest,"cleanup":copy.cleanup})
    }).collect::<Vec<_>>();
    assert_eq!(copies.len(), 2);
    let ledger = creation.ledger_writes().map(|write| match write {
        crate::run::OffloadLedgerWrite::Recorded{effect,durability,confirmation,failures} => {
            assert!(failures.is_empty()); json!({"effect":format!("{effect:?}"),"durability":durability.map(|s|format!("{s:?}")),"confirmation":format!("{confirmation:?}")})
        }
        crate::run::OffloadLedgerWrite::Refused{..} => panic!("genuine ledger publication refused"),
    }).collect::<Vec<_>>();
    assert_eq!(ledger.len(), 1);
    let reads = creation.offers().archive_reads().iter().map(|read| {
        assert_eq!(read.cleanup(),Some(Cleanup::Complete));
        json!({"name":read.file().name,"snapshot":read.file().snapshot_id,"path":read.file().archive_path,
            "identity":read.target_identity(),"digest":read.digest(),"cleanup":read.cleanup()})
    }).collect::<Vec<_>>();
    assert_eq!(reads.len(), 2);
    let before_register = fs::read(input.journal.join("health/retention-marks.json"))?;
    let before_ledger = fs::read(input.journal.join("health/offload/20261009.jsonl"))?;
    let stdout = run.output().0.to_owned();
    let stderr = run.output().1.to_owned();
    let exit = run.output().2;
    let phase = format!("{:?}", run.phase());
    let mut next = None;
    let completion;
    if positive {
        assert_eq!(run.phase(), CliRunPhase::Finished);
        assert_eq!(exit, 0);
        let runtime = run.runtime().unwrap();
        assert_eq!(runtime.state(), DirectReadFinishState::Completed);
        assert!(runtime.retirement().is_some());
        completion = Some(json!({"state":"Completed","retirement_receipt":true}));
        assert!(stages(&input.journal)?.is_empty());
    } else {
        assert_eq!(
            run.phase(),
            CliRunPhase::Unknown(CliUnwindPhase::RuntimeFinish)
        );
        assert_ne!(exit, 0);
        assert!(run.runtime().is_none());
        assert_eq!(stages(&input.journal)?.len(), 1);
        completion = None;
    }
    CONTROL.with(|slot| {
        if positive {
            cut(slot.borrow_mut().as_mut().unwrap(), b"retired_", false).unwrap();
        }
    });
    if positive {
        // Release only the completed whole result; admission still used the same coordinator.
        drop(run);
        let mut stream = CONTROL.with(|slot| slot.borrow_mut().take().unwrap());
        let second = run_cli_offload(&input.journal, Arc::new(ActualClock), false);
        assert_eq!(second.output().2, 0, "{}", second.output().1);
        assert_eq!(
            second.runtime().unwrap().state(),
            DirectReadFinishState::Completed
        );
        next = Some(
            json!({"state":"Completed","phase":format!("{:?}",second.observed().planning_phase)}),
        );
        assert!(stages(&input.journal)?.is_empty());
        drop(second);
        cut(&mut stream, b"nextdone", false)?;
        CONTROL.with(|slot| *slot.borrow_mut() = Some(stream));
    } else {
        std::hint::black_box(&run);
        assert!(
            run.observed()
                .creation
                .as_ref()
                .unwrap()
                .publication
                .as_ref()
                .unwrap()
                .published
        );
        assert_eq!(
            run.observed()
                .creation
                .as_ref()
                .unwrap()
                .source_copies
                .len(),
            2
        );
    }
    for (index, name) in ["file1.flac", "file2.flac"].iter().enumerate() {
        assert_eq!(file_fact(&raw_dir.join(name))?, raw[index]);
        assert_eq!(
            fs::read(raw_dir.join(name).with_extension("jsonl"))?,
            headers[index]
        );
    }
    assert_eq!(
        fs::read(input.journal.join("health/retention-marks.json"))?,
        before_register
    );
    assert_eq!(
        fs::read(input.journal.join("health/offload/20261009.jsonl"))?,
        before_ledger
    );
    CONTROL.with(|slot| cut(slot.borrow_mut().as_mut().unwrap(), b"returned", false).unwrap());
    println!(
        "OWNED_CLI_PROBE_JSON={}",
        json!({"case":input.case,"harness_pid":std::process::id(),
        "raw":raw,"marks":marks,"reads":reads,"source_copies":copies,"ledger":ledger,"phase":phase,"stdout":stdout,"stderr":stderr,
        "exit":exit,"completion":completion,"next":next,"stages":stages(&input.journal)?,"original_custody":true,
        "closed_thread_joined":true,"limits":"synthetic copied Linux package; injected panic at consuming finish boundary, not actual runtime retirement failure or recovery authority"})
    );
    Ok(())
}
