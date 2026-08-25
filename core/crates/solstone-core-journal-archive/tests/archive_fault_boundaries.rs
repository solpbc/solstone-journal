// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

mod common;

use std::fs::{self, File};
use std::io::{self, Cursor, Seek, SeekFrom, Write};
use std::os::unix::fs::{MetadataExt, symlink};
use std::os::unix::net::UnixListener;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::sync::Barrier;

use common::TempDir;
use nix::errno::Errno;
use nix::sys::stat::Mode;
use nix::unistd::mkfifo;
use solstone_core_journal_archive::{
    AcquisitionPrimitive, ArchiveError, ArchiveSource, DescendantPrimitive, EncodeArchiveError,
    EncodeArchiveRequest, EncodeTruncateBeforeRead, ExplicitArchiveOutputRequest,
    ExplicitTargetError, TestBoundary, TestFaultKind, TestSinkOperation,
    acquire_explicit_output_target, encode_archive, run_with_acquisition_fault,
    run_with_descendant_barrier, run_with_encode_control,
};
use zip::ZipWriter;

const DESCENDANT_BARRIER_ROOT: &str = "SOLSTONE_ARCHIVE_DESCENDANT_BARRIER_ROOT";
const DESCENDANT_BARRIER_MODE: &str = "SOLSTONE_ARCHIVE_DESCENDANT_BARRIER_MODE";
const DESCENDANT_BARRIER_KIND: &str = "SOLSTONE_ARCHIVE_DESCENDANT_BARRIER_KIND";
const DESCENDANT_MEMBER: &str = "imports/import-1/source.bin";
const STDERR_CHILD: &str = "SOLSTONE_ARCHIVE_STDERR_CHILD";

fn inode_identity(path: &Path) -> (u64, u64, u32, u64) {
    let metadata = fs::symlink_metadata(path).expect("stat final object without following it");
    (
        metadata.dev(),
        metadata.ino(),
        metadata.mode(),
        metadata.len(),
    )
}

fn assert_identity_unchanged(path: &Path, before: (u64, u64, u32, u64)) {
    assert_eq!(inode_identity(path), before, "{} changed", path.display());
}

#[test]
#[allow(clippy::disallowed_methods)]
fn final_object_kinds_are_classified_without_open_read_or_replacement() {
    let temporary = TempDir::new("final-object-kinds");
    let root = fs::canonicalize(temporary.path()).expect("canonicalize temporary root");
    let parent = root.join("output");
    fs::create_dir(&parent).expect("create output parent");

    let regular = parent.join("regular.zip");
    fs::write(&regular, b"keep").expect("create regular final object");
    let regular_witness = parent.join("regular-witness");
    fs::hard_link(&regular, &regular_witness).expect("create regular content witness");
    let regular_before = inode_identity(&regular);
    assert!(matches!(
        acquire_explicit_output_target(&ExplicitArchiveOutputRequest::new(
            regular.clone(),
            root.clone(),
        )),
        Err(ExplicitTargetError::Collision { .. })
    ));
    assert_identity_unchanged(&regular, regular_before);
    assert_eq!(
        fs::read(&regular_witness).expect("read hard-link witness, not final path"),
        b"keep",
        "classification must not modify the existing regular-file inode"
    );

    let directory = parent.join("directory.zip");
    fs::create_dir(&directory).expect("create directory final object");
    let directory_before = inode_identity(&directory);
    assert!(matches!(
        acquire_explicit_output_target(&ExplicitArchiveOutputRequest::new(
            directory.clone(),
            root.clone(),
        )),
        Err(ExplicitTargetError::UnsafeTarget {
            kind: "directory",
            ..
        })
    ));
    assert_identity_unchanged(&directory, directory_before);

    let link = parent.join("link.zip");
    symlink(&regular, &link).expect("create symlink final object");
    let link_before = inode_identity(&link);
    assert!(matches!(
        acquire_explicit_output_target(&ExplicitArchiveOutputRequest::new(
            link.clone(),
            root.clone(),
        )),
        Err(ExplicitTargetError::UnsafeTarget {
            kind: "symlink",
            ..
        })
    ));
    assert_identity_unchanged(&link, link_before);

    let socket = parent.join("socket.zip");
    let listener = UnixListener::bind(&socket).expect("create socket final object");
    let socket_before = inode_identity(&socket);
    assert!(matches!(
        acquire_explicit_output_target(&ExplicitArchiveOutputRequest::new(socket.clone(), root,)),
        Err(ExplicitTargetError::UnsafeTarget { kind: "socket", .. })
    ));
    assert_identity_unchanged(&socket, socket_before);
    drop(listener);
}

#[allow(clippy::disallowed_methods)]
fn nested_journal(temporary: &TempDir, bytes: &[u8]) -> PathBuf {
    let root = temporary.path().join("outer/inner/journal");
    let source = root.join(DESCENDANT_MEMBER);
    fs::create_dir_all(source.parent().expect("source has parent"))
        .expect("create journal parents");
    fs::write(source, bytes).expect("write journal source");
    root
}

fn noisy_bytes(length: usize) -> Vec<u8> {
    let mut state = 0x1234_5678_u32;
    (0..length)
        .map(|_| {
            state ^= state << 13;
            state ^= state >> 17;
            state ^= state << 5;
            state as u8
        })
        .collect()
}

#[allow(clippy::disallowed_methods)]
fn encode_source_fixture(name: &str) -> (TempDir, ArchiveSource, PathBuf) {
    let temporary = TempDir::new(name);
    let root = temporary.path().join("journal");
    fs::create_dir(&root).expect("create journal root");
    let first = root.join("imports/first/source.bin");
    fs::create_dir_all(first.parent().expect("first parent")).expect("create first parent");
    fs::write(&first, noisy_bytes(192 * 1024)).expect("write first");
    let second = root.join("imports/second/source.bin");
    fs::create_dir_all(second.parent().expect("second parent")).expect("create second parent");
    fs::write(&second, noisy_bytes(96 * 1024)).expect("write second");
    let source = ArchiveSource::open(&root).expect("open source fixture");
    (temporary, source, first)
}

fn encode_request(source: &ArchiveSource) -> EncodeArchiveRequest<'_> {
    EncodeArchiveRequest {
        source,
        solstone_version: "1.2.3",
        exported_at: "2040-01-02T03:04:59Z",
        day_window: None,
    }
}

struct DropFailWriter(Cursor<Vec<u8>>);

impl Write for DropFailWriter {
    fn write(&mut self, _buffer: &[u8]) -> io::Result<usize> {
        Err(io::Error::other("uncontrolled drop failure"))
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl Seek for DropFailWriter {
    fn seek(&mut self, from: SeekFrom) -> io::Result<u64> {
        self.0.seek(from)
    }
}

fn run_named_child(name: &str, envs: &[(&str, &str)]) -> std::process::Output {
    let mut command = Command::new(std::env::current_exe().expect("current test executable"));
    command
        .args(["--ignored", "--exact", name, "--nocapture"])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    for (key, value) in envs {
        command.env(key, value);
    }
    command.output().expect("wait for helper")
}

#[test]
fn acquisition_faults_are_thread_local() {
    let rendezvous = Arc::new(Barrier::new(3));
    std::thread::scope(|scope| {
        let first_rendezvous = Arc::clone(&rendezvous);
        scope.spawn(move || {
            let (result, consumed) = run_with_acquisition_fault(
                AcquisitionPrimitive::RequestedRootOpen,
                1,
                Errno::EACCES as i32,
                || {
                    first_rendezvous.wait();
                    ArchiveSource::open(Path::new("/"))
                },
            );
            assert!(consumed);
            let actual = match result {
                Ok(_) => panic!("thread-local requested-root fault must fail"),
                Err(actual) => actual,
            };
            match actual {
                ArchiveError::SourceIo {
                    operation,
                    member,
                    source,
                } => {
                    assert_eq!(operation, "open journal root");
                    assert!(member.is_none());
                    assert_eq!(source.raw_os_error(), Some(Errno::EACCES as i32));
                }
                other => panic!("unexpected acquisition error: {other:?}"),
            }
        });

        let second_rendezvous = Arc::clone(&rendezvous);
        scope.spawn(move || {
            let (result, consumed) = run_with_acquisition_fault(
                AcquisitionPrimitive::Canonicalize,
                1,
                Errno::EIO as i32,
                || {
                    second_rendezvous.wait();
                    ArchiveSource::open(Path::new("/"))
                },
            );
            assert!(consumed);
            let actual = match result {
                Ok(_) => panic!("thread-local canonicalize fault must fail"),
                Err(actual) => actual,
            };
            match actual {
                ArchiveError::SourceIo {
                    operation,
                    member,
                    source,
                } => {
                    assert_eq!(operation, "canonicalize journal root");
                    assert!(member.is_none());
                    assert_eq!(source.raw_os_error(), Some(Errno::EIO as i32));
                }
                other => panic!("unexpected acquisition error: {other:?}"),
            }
        });

        rendezvous.wait();
    });
}

#[test]
#[allow(clippy::disallowed_methods)]
#[ignore = "subprocess fixture for descendant_stat_to_open_swaps_are_bounded_and_changed"]
fn descendant_barrier_child() {
    use std::os::unix::net::UnixListener;

    let Some(root) = std::env::var_os(DESCENDANT_BARRIER_ROOT).map(PathBuf::from) else {
        return;
    };
    let mode = std::env::var(DESCENDANT_BARRIER_MODE).expect("barrier child mode");
    let kind = std::env::var(DESCENDANT_BARRIER_KIND).expect("barrier child kind");
    let target = root.join(DESCENDANT_MEMBER);
    let kind_owned = kind.clone();
    let callback = move || {
        fs::remove_file(&target).expect("remove inventoried file at stat/open barrier");
        match kind_owned.as_str() {
            "fifo" => mkfifo(&target, Mode::S_IRUSR | Mode::S_IWUSR).expect("create barrier fifo"),
            "socket" => {
                let listener = UnixListener::bind(&target).expect("create barrier socket");
                drop(listener);
            }
            _ => panic!("unknown barrier replacement kind"),
        }
    };

    let (result, fired) = if mode == "initial" {
        run_with_descendant_barrier(
            DescendantPrimitive::Metadata,
            Some(DESCENDANT_MEMBER),
            1,
            callback,
            || ArchiveSource::open(&root).map(|_| ()),
        )
    } else {
        let source = ArchiveSource::open(&root).expect("open source before barrier");
        let inventory_entry = source
            .inventory()
            .entries()
            .iter()
            .find(|entry| entry.member_name().as_str() == DESCENDANT_MEMBER)
            .expect("barrier inventory entry");
        run_with_descendant_barrier(
            DescendantPrimitive::Metadata,
            Some(DESCENDANT_MEMBER),
            1,
            callback,
            || match mode.as_str() {
                "open-file" => source.open_file(inventory_entry).map(|_| ()),
                "revalidate" => source.revalidate(),
                _ => panic!("unknown barrier child mode"),
            },
        )
    };

    assert!(fired, "stat/open barrier did not fire");
    assert!(
        matches!(
            result,
            Err(ArchiveError::SourceChanged { member: Some(ref member) })
                if member.as_str() == DESCENDANT_MEMBER
        ),
        "unexpected stat/open replacement result: {result:?}"
    );
}

#[test]
fn descendant_stat_to_open_swaps_are_bounded_and_changed() {
    for mode in ["initial", "open-file", "revalidate"] {
        for kind in ["fifo", "socket"] {
            let temporary = TempDir::new("barrier");
            let root = nested_journal(&temporary, b"source");
            // Boundedness comes from production O_NONBLOCK FILE_FLAGS; the suite timeout is the hang net.
            let output = run_named_child(
                "descendant_barrier_child",
                &[
                    (DESCENDANT_BARRIER_ROOT, root.to_str().expect("root utf8")),
                    (DESCENDANT_BARRIER_MODE, mode),
                    (DESCENDANT_BARRIER_KIND, kind),
                ],
            );
            assert!(
                output.status.success(),
                "{mode}/{kind} barrier child failed\nstdout:\n{}\nstderr:\n{}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
        }
    }
}

#[test]
#[allow(clippy::disallowed_methods)]
#[ignore = "subprocess fixture for controlled_failures_never_reach_zip_drop_stderr"]
fn stderr_oracle_child() {
    match std::env::var(STDERR_CHILD).as_deref() {
        Ok("uncontrolled") => {
            drop(ZipWriter::new(DropFailWriter(Cursor::new(Vec::new()))));
        }
        Ok(scenario) if scenario.starts_with("controlled-") => {
            let (_temporary, source, first) = encode_source_fixture("stderr-child");
            let output = TempDir::new("stderr-child-output");
            let path = output.path().join("archive.zip");
            let mut file = File::create(&path).expect("create controlled output");
            let (boundary, operation, kind, truncate, body_phase) = match scenario {
                "controlled-body" => (
                    TestBoundary::RootDirectory,
                    TestSinkOperation::Write,
                    TestFaultKind::Error,
                    None,
                    true,
                ),
                "controlled-zero" => (
                    TestBoundary::RootDirectory,
                    TestSinkOperation::Write,
                    TestFaultKind::WriteZero,
                    None,
                    true,
                ),
                "controlled-transition" => (
                    TestBoundary::MemberTransition,
                    TestSinkOperation::Write,
                    TestFaultKind::Error,
                    None,
                    true,
                ),
                "controlled-abort" => (
                    TestBoundary::Abort,
                    TestSinkOperation::Write,
                    TestFaultKind::Error,
                    Some(EncodeTruncateBeforeRead {
                        member: "imports/first/source.bin".to_owned(),
                        copied: 0,
                        path: first,
                        length: 0,
                    }),
                    false,
                ),
                "controlled-central" => (
                    TestBoundary::CentralDirectory,
                    TestSinkOperation::Write,
                    TestFaultKind::Error,
                    None,
                    false,
                ),
                "controlled-footer" => (
                    TestBoundary::Footer,
                    TestSinkOperation::Write,
                    TestFaultKind::Error,
                    None,
                    false,
                ),
                "controlled-terminal" => (
                    TestBoundary::TerminalSeek,
                    TestSinkOperation::Seek,
                    TestFaultKind::Error,
                    None,
                    false,
                ),
                _ => panic!("unknown stderr child scenario {scenario}"),
            };
            let (result, fired) =
                run_with_encode_control(boundary, operation, 1, kind, truncate, || {
                    encode_archive(&encode_request(&source), &mut file)
                });
            assert!(
                fired,
                "injected encode operation did not fire for {scenario}"
            );
            if body_phase {
                assert!(matches!(
                    result,
                    Err(EncodeArchiveError::ArchiveWrite { .. })
                ));
            } else {
                assert!(matches!(
                    result,
                    Err(EncodeArchiveError::ArchiveFinish { .. })
                ));
            }
            drop(file);
            let _ = fs::remove_file(&path);
        }
        _ => {}
    }
}

#[test]
fn controlled_failures_never_reach_zip_drop_stderr() {
    let uncontrolled = run_named_child("stderr_oracle_child", &[(STDERR_CHILD, "uncontrolled")]);
    assert!(uncontrolled.status.success());
    assert!(
        String::from_utf8_lossy(&uncontrolled.stderr).contains("ZipWriter drop failed"),
        "negative control did not observe zip Drop stderr: {uncontrolled:?}"
    );

    for scenario in [
        "controlled-body",
        "controlled-zero",
        "controlled-transition",
        "controlled-abort",
        "controlled-central",
        "controlled-footer",
        "controlled-terminal",
    ] {
        let controlled = run_named_child("stderr_oracle_child", &[(STDERR_CHILD, scenario)]);
        assert!(
            controlled.status.success(),
            "controlled stderr child failed for {scenario}: {controlled:?}"
        );
        assert_eq!(controlled.stderr, b"", "stderr for {scenario}");
    }
}
