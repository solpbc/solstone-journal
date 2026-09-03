// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::cell::RefCell;
use std::error::Error;
use std::fmt;
use std::fs::File;
use std::io::{self, Seek, SeekFrom, Write};

#[cfg(unix)]
use nix::fcntl::{FcntlArg, OFlag, fcntl};
#[cfg(unix)]
use nix::sys::stat::{SFlag, fstat};
#[cfg(unix)]
use nix::unistd::{Whence, lseek};
use zip::ZipWriter;

use crate::manifest::{self, ManifestError, ManifestFields};
use crate::writer::{ArchiveEncodingError, WriteFailure};
use crate::{ArchiveError, ArchiveMemberName, ArchiveSource};

/// Inclusive chronicle-day window for a sliced portable export.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DayWindow {
    /// Lower bound, inclusive. `None` means unbounded below.
    pub from: Option<String>,
    /// Upper bound, inclusive. `None` means unbounded above.
    pub to: Option<String>,
}

impl DayWindow {
    /// Return whether an eight-digit chronicle day falls in this window.
    pub fn contains_day(&self, day: &str) -> bool {
        self.from.as_deref().is_none_or(|from| day >= from)
            && self.to.as_deref().is_none_or(|to| day <= to)
    }

    /// Return whether a file member belongs in a sliced archive.
    pub fn contains_member(&self, member: &str) -> bool {
        chronicle_day(member).is_some_and(|day| self.contains_day(day))
    }
}

fn chronicle_day(member: &str) -> Option<&str> {
    let rest = member.strip_prefix("chronicle/")?;
    let day = rest.split_once('/').map_or(rest, |(day, _)| day);
    (day.len() == 8 && day.bytes().all(|byte| byte.is_ascii_digit())).then_some(day)
}

/// The frozen source and metadata to encode into a portable archive.
pub struct EncodeArchiveRequest<'a> {
    pub source: &'a ArchiveSource,
    pub solstone_version: &'a str,
    pub exported_at: &'a str,
    /// When set, the zip contains only matching `chronicle/<day>/` trees and `_export.json`.
    pub day_window: Option<DayWindow>,
}

/// The stage in which an output-file fault was observed.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EncodingPhase {
    Body,
    Finalize,
}

/// A later body failure retained behind the first output-file fault.
#[derive(Debug)]
pub enum EncodeArchiveFollowOn {
    Source(ArchiveError),
    ArchiveWrite {
        member: Option<ArchiveMemberName>,
        source: io::Error,
    },
}

impl fmt::Display for EncodingPhase {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Body => formatter.write_str("body"),
            Self::Finalize => formatter.write_str("finalize"),
        }
    }
}

/// Failure while encoding a portable archive into a checked output file.
#[derive(Debug)]
pub enum EncodeArchiveError {
    InvalidWriter {
        reason: &'static str,
    },
    InvalidMetadata {
        field: &'static str,
        value: String,
        reason: &'static str,
    },
    Source(ArchiveError),
    ArchiveWrite {
        member: Option<ArchiveMemberName>,
        source: io::Error,
        followed: Option<EncodeArchiveFollowOn>,
    },
    ArchiveFinish {
        phase: EncodingPhase,
        source: io::Error,
        followed: Option<EncodeArchiveFollowOn>,
    },
}

impl fmt::Display for EncodeArchiveError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidWriter { reason } => write!(formatter, "invalid archive writer: {reason}"),
            Self::InvalidMetadata {
                field,
                value,
                reason,
            } => write!(
                formatter,
                "invalid archive metadata {field} {value:?}: {reason}"
            ),
            Self::Source(source) => write!(formatter, "archive source: {source}"),
            Self::ArchiveWrite {
                member: Some(member),
                source,
                ..
            } => write!(formatter, "archive write {}: {source}", member.as_str()),
            Self::ArchiveWrite {
                member: None,
                source,
                ..
            } => write!(formatter, "archive write: {source}"),
            Self::ArchiveFinish { phase, source, .. } => {
                write!(formatter, "archive finish {phase}: {source}")
            }
        }
    }
}

impl Error for EncodeArchiveError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Source(source) => Some(source),
            Self::ArchiveWrite { source, .. } | Self::ArchiveFinish { source, .. } => Some(source),
            Self::InvalidWriter { .. } | Self::InvalidMetadata { .. } => None,
        }
    }
}

struct FileSink<'a> {
    file: &'a mut File,
    state: &'a RefCell<SinkState>,
}

struct SinkState {
    poison: Poison,
    phase: EncodingPhase,
    position: u64,
    length: u64,
}

enum Poison {
    Live,
    Faulted {
        error: io::Error,
        phase: EncodingPhase,
    },
}

#[cfg(any(test, feature = "test-hooks"))]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TestBoundary {
    Body,
    RootDirectory,
    SourceStart,
    MemberTransition,
    SourcePayload,
    ManifestStart,
    ManifestPayload,
    FinalRevalidate,
    Abort,
    Finish,
    CentralDirectory,
    Footer,
    TerminalSeek,
}

#[cfg(any(test, feature = "test-hooks"))]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TestSinkOperation {
    Write,
    Seek,
    Flush,
}

#[cfg(any(test, feature = "test-hooks"))]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TestFaultKind {
    Error,
    WriteZero,
}

#[cfg(any(test, feature = "test-hooks"))]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct TestFault {
    pub(crate) boundary: TestBoundary,
    pub(crate) operation: TestSinkOperation,
    pub(crate) ordinal: usize,
    pub(crate) kind: TestFaultKind,
}

#[cfg(any(test, feature = "test-hooks"))]
#[derive(Clone, Debug, Eq, PartialEq)]
struct TestTraceEvent {
    boundary: TestBoundary,
    operation: TestSinkOperation,
    ordinal: usize,
    faulted: bool,
}

#[cfg(any(test, feature = "test-hooks"))]
pub(crate) enum TestSourceAction {
    #[cfg(test)]
    RemoveBeforeOpen {
        member: String,
        path: std::path::PathBuf,
    },
    TruncateBeforeRead {
        member: String,
        copied: u64,
        path: std::path::PathBuf,
        length: u64,
    },
    #[cfg(test)]
    AppendBeforeFinalRevalidate {
        path: std::path::PathBuf,
        bytes: Vec<u8>,
    },
}

#[cfg(any(test, feature = "test-hooks"))]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum TestPreflightOperation {
    Stat,
    Flags,
    Position,
}

#[cfg(any(test, feature = "test-hooks"))]
struct TestControl {
    boundary: TestBoundary,
    fault: Option<TestFault>,
    trace: Vec<TestTraceEvent>,
    action: Option<TestSourceAction>,
    preflight_fault: Option<TestPreflightOperation>,
    body_write_failure: Option<String>,
    #[cfg(test)]
    finish_calls: usize,
    #[cfg(test)]
    abort_calls: usize,
}

#[cfg(any(test, feature = "test-hooks"))]
impl TestControl {
    const fn new() -> Self {
        Self {
            boundary: TestBoundary::Body,
            fault: None,
            trace: Vec::new(),
            action: None,
            preflight_fault: None,
            body_write_failure: None,
            #[cfg(test)]
            finish_calls: 0,
            #[cfg(test)]
            abort_calls: 0,
        }
    }
}

#[cfg(any(test, feature = "test-hooks"))]
thread_local! {
    static TEST_CONTROL: RefCell<TestControl> = const { RefCell::new(TestControl::new()) };
}

#[cfg(any(test, feature = "test-hooks"))]
pub(crate) fn install_encode_control(
    fault: Option<TestFault>,
    action: Option<TestSourceAction>,
    preflight_fault: Option<TestPreflightOperation>,
) {
    TEST_CONTROL.with(|control| {
        let mut replacement = TestControl::new();
        replacement.fault = fault;
        replacement.action = action;
        replacement.preflight_fault = preflight_fault;
        *control.borrow_mut() = replacement;
    });
}

#[cfg(feature = "test-hooks")]
pub(crate) fn encode_injected_operation_fired() -> bool {
    TEST_CONTROL.with(|control| control.borrow().trace.iter().any(|event| event.faulted))
}

#[cfg(feature = "test-hooks")]
pub(crate) fn reset_encode_control() {
    TEST_CONTROL.with(|control| *control.borrow_mut() = TestControl::new());
}

#[cfg(any(test, feature = "test-hooks"))]
pub(crate) fn test_set_boundary(boundary: TestBoundary) {
    TEST_CONTROL.with(|control| control.borrow_mut().boundary = boundary);
}

#[cfg(any(test, feature = "test-hooks"))]
fn test_sink_result(
    operation: TestSinkOperation,
    requested: usize,
    buffer: Option<&[u8]>,
) -> Option<io::Result<usize>> {
    TEST_CONTROL.with(|control| {
        let mut control = control.borrow_mut();
        if operation == TestSinkOperation::Write {
            if buffer.is_some_and(|bytes| bytes.starts_with(b"PK\x01\x02")) {
                control.boundary = TestBoundary::CentralDirectory;
            } else if buffer.is_some_and(|bytes| bytes.starts_with(b"PK\x05\x06")) {
                control.boundary = TestBoundary::Footer;
            }
        } else if operation == TestSinkOperation::Seek && control.boundary == TestBoundary::Footer {
            control.boundary = TestBoundary::TerminalSeek;
        }
        let boundary = control.boundary;
        let ordinal = control
            .trace
            .iter()
            .filter(|event| event.boundary == boundary && event.operation == operation)
            .count()
            + 1;
        let kind = control.fault.and_then(|fault| {
            (fault.boundary == boundary && fault.operation == operation && fault.ordinal == ordinal)
                .then_some(fault.kind)
        });
        control.trace.push(TestTraceEvent {
            boundary,
            operation,
            ordinal,
            faulted: kind.is_some(),
        });
        if kind.is_some() {
            control.fault = None;
        }
        match kind {
            Some(TestFaultKind::Error) => Some(Err(io::Error::other("injected sink fault"))),
            Some(TestFaultKind::WriteZero) => Some(Ok(0)),
            None => {
                let _ = requested;
                None
            }
        }
    })
}

#[cfg(any(test, feature = "test-hooks"))]
pub(crate) fn test_before_source_open(member: &ArchiveMemberName) {
    test_run_source_action(member, None);
}

#[cfg(any(test, feature = "test-hooks"))]
pub(crate) fn test_before_source_read(member: &ArchiveMemberName, copied: u64) {
    test_run_source_action(member, Some(copied));
}

#[cfg(any(test, feature = "test-hooks"))]
pub(crate) fn test_take_body_write_failure(member: &ArchiveMemberName) -> bool {
    TEST_CONTROL.with(|control| {
        let mut control = control.borrow_mut();
        if control.body_write_failure.as_deref() == Some(member.as_str()) {
            control.body_write_failure = None;
            true
        } else {
            false
        }
    })
}

#[cfg(any(test, feature = "test-hooks"))]
#[allow(clippy::disallowed_methods, clippy::disallowed_types)]
fn test_run_source_action(member: &ArchiveMemberName, copied: Option<u64>) {
    TEST_CONTROL.with(|control| {
        let mut control = control.borrow_mut();
        let should_run = match control.action.as_ref() {
            #[cfg(test)]
            Some(TestSourceAction::RemoveBeforeOpen { member: target, .. }) => {
                copied.is_none() && target == member.as_str()
            }
            Some(TestSourceAction::TruncateBeforeRead {
                member: target,
                copied: target_copied,
                ..
            }) => copied == Some(*target_copied) && target == member.as_str(),
            #[cfg(test)]
            Some(TestSourceAction::AppendBeforeFinalRevalidate { .. }) | None => false,
            #[cfg(not(test))]
            None => false,
        };
        if !should_run {
            return;
        }
        match control.action.take().expect("matched source action") {
            #[cfg(test)]
            TestSourceAction::RemoveBeforeOpen { path, .. } => {
                std::fs::remove_file(path).expect("remove source before open");
            }
            TestSourceAction::TruncateBeforeRead { path, length, .. } => {
                std::fs::OpenOptions::new()
                    .write(true)
                    .open(path)
                    .expect("open source for truncation")
                    .set_len(length)
                    .expect("truncate source before read");
            }
            #[cfg(test)]
            TestSourceAction::AppendBeforeFinalRevalidate { .. } => unreachable!(),
        }
    });
}

#[cfg(any(test, feature = "test-hooks"))]
fn test_before_final_revalidate() {
    test_set_boundary(TestBoundary::FinalRevalidate);
    #[cfg(test)]
    #[allow(clippy::disallowed_methods, clippy::disallowed_types)]
    TEST_CONTROL.with(|control| {
        let mut control = control.borrow_mut();
        if !matches!(
            control.action,
            Some(TestSourceAction::AppendBeforeFinalRevalidate { .. })
        ) {
            return;
        }
        if let Some(TestSourceAction::AppendBeforeFinalRevalidate { path, bytes }) =
            control.action.take()
        {
            let mut file = std::fs::OpenOptions::new()
                .append(true)
                .open(path)
                .expect("open source for final-revalidation mutation");
            file.write_all(&bytes)
                .expect("append before final revalidation");
        }
    });
}

#[cfg(not(any(test, feature = "test-hooks")))]
fn test_before_final_revalidate() {}

#[cfg(any(test, feature = "test-hooks"))]
fn test_before_abort() {
    test_set_boundary(TestBoundary::Abort);
    #[cfg(test)]
    TEST_CONTROL.with(|control| control.borrow_mut().abort_calls += 1);
}

#[cfg(not(any(test, feature = "test-hooks")))]
fn test_before_abort() {}

#[cfg(any(test, feature = "test-hooks"))]
fn test_before_finish() {
    test_set_boundary(TestBoundary::Finish);
    #[cfg(test)]
    TEST_CONTROL.with(|control| control.borrow_mut().finish_calls += 1);
}

#[cfg(not(any(test, feature = "test-hooks")))]
fn test_before_finish() {}

#[cfg(any(test, feature = "test-hooks"))]
fn test_take_preflight_fault(operation: TestPreflightOperation) -> bool {
    TEST_CONTROL.with(|control| {
        let mut control = control.borrow_mut();
        if control.preflight_fault == Some(operation) {
            control.preflight_fault = None;
            true
        } else {
            false
        }
    })
}

impl SinkState {
    const fn live() -> Self {
        Self {
            poison: Poison::Live,
            phase: EncodingPhase::Body,
            position: 0,
            length: 0,
        }
    }

    fn is_live(&self) -> bool {
        matches!(&self.poison, Poison::Live)
    }

    fn record_fault(&mut self, error: io::Error) {
        if matches!(&self.poison, Poison::Live) {
            self.poison = Poison::Faulted {
                error,
                phase: self.phase,
            };
        }
    }

    fn handle_live_write(
        &mut self,
        requested: usize,
        result: io::Result<usize>,
    ) -> io::Result<usize> {
        match result {
            Ok(0) if requested != 0 => {
                self.record_fault(io::Error::new(
                    io::ErrorKind::WriteZero,
                    "archive writer returned zero bytes for a nonempty write",
                ));
                Ok(self.virtual_write(requested))
            }
            Ok(count) => {
                self.record_live_write(count);
                Ok(count)
            }
            Err(error) => {
                self.record_fault(error);
                Ok(self.virtual_write(requested))
            }
        }
    }

    fn record_live_write(&mut self, count: usize) {
        self.position = self.position.saturating_add(count as u64);
        self.length = self.length.max(self.position);
    }

    fn record_live_seek(&mut self, position: u64) {
        self.position = position;
        self.length = self.length.max(position);
    }

    fn virtual_write(&mut self, count: usize) -> usize {
        self.position = self.position.saturating_add(count as u64);
        self.length = self.length.max(self.position);
        count
    }

    fn virtual_seek(&mut self, from: SeekFrom) -> u64 {
        self.position = match from {
            SeekFrom::Start(position) => position,
            SeekFrom::Current(offset) => self.position.saturating_add_signed(offset),
            SeekFrom::End(offset) => self.length.saturating_add_signed(offset),
        };
        self.length = self.length.max(self.position);
        self.position
    }
}

impl FileSink<'_> {
    fn is_live(&self) -> bool {
        self.state.borrow().is_live()
    }
}

impl Write for FileSink<'_> {
    fn write(&mut self, buffer: &[u8]) -> io::Result<usize> {
        if !self.is_live() {
            return Ok(self.state.borrow_mut().virtual_write(buffer.len()));
        }
        #[cfg(any(test, feature = "test-hooks"))]
        if let Some(result) = test_sink_result(TestSinkOperation::Write, buffer.len(), Some(buffer))
        {
            return self
                .state
                .borrow_mut()
                .handle_live_write(buffer.len(), result);
        }
        let result = self.file.write(buffer);
        self.state
            .borrow_mut()
            .handle_live_write(buffer.len(), result)
    }

    fn flush(&mut self) -> io::Result<()> {
        if !self.is_live() {
            return Ok(());
        }
        #[cfg(any(test, feature = "test-hooks"))]
        if let Some(result) = test_sink_result(TestSinkOperation::Flush, 0, None) {
            return match result {
                Ok(_) => Ok(()),
                Err(error) => {
                    self.state.borrow_mut().record_fault(error);
                    Ok(())
                }
            };
        }
        match self.file.flush() {
            Ok(()) => Ok(()),
            Err(error) => {
                self.state.borrow_mut().record_fault(error);
                Ok(())
            }
        }
    }
}

impl Seek for FileSink<'_> {
    fn seek(&mut self, from: SeekFrom) -> io::Result<u64> {
        if !self.is_live() {
            return Ok(self.state.borrow_mut().virtual_seek(from));
        }
        #[cfg(any(test, feature = "test-hooks"))]
        if let Some(result) = test_sink_result(TestSinkOperation::Seek, 0, None) {
            return match result {
                Ok(_) => unreachable!("seek injection cannot return success"),
                Err(error) => {
                    let mut state = self.state.borrow_mut();
                    state.record_fault(error);
                    Ok(state.virtual_seek(from))
                }
            };
        }
        match self.file.seek(from) {
            Ok(position) => {
                self.state.borrow_mut().record_live_seek(position);
                Ok(position)
            }
            Err(error) => {
                let mut state = self.state.borrow_mut();
                state.record_fault(error);
                Ok(state.virtual_seek(from))
            }
        }
    }
}

/// Encode one frozen archive source into an already-open checked output file.
///
/// The caller retains `file` on every result. `InvalidWriter`,
/// `InvalidMetadata`, and a source failure detected before ZIP construction
/// leave an initially empty file at offset zero. After ZIP construction starts,
/// bytes and cursor position on `Err` are unspecified: the caller must discard
/// the file based solely on the `Result`, even if those bytes parse as a ZIP.
/// This function deliberately does not truncate, rewind, unlink, or publish.
pub fn encode_archive(
    request: &EncodeArchiveRequest<'_>,
    file: &mut File,
) -> Result<(), EncodeArchiveError> {
    preflight_writer(file)?;
    for entry in request.source.inventory().entries() {
        validate_member_name_length(entry.member_name())?;
    }

    let source_journal = request.source.canonical_source().to_str().ok_or_else(|| {
        EncodeArchiveError::InvalidMetadata {
            field: "source_journal",
            value: request
                .source
                .canonical_source()
                .to_string_lossy()
                .into_owned(),
            reason: "must be valid UTF-8",
        }
    })?;
    let inventory = request.source.inventory();
    let (day_count, entity_count, facet_count) = match &request.day_window {
        None => (
            inventory.day_count(),
            inventory.entity_count(),
            inventory.facet_count(),
        ),
        Some(window) => {
            let mut days = std::collections::BTreeSet::new();
            for entry in inventory.entries() {
                if let Some(day) = chronicle_day(entry.member_name().as_str())
                    && window.contains_day(day)
                {
                    days.insert(day);
                }
            }
            (days.len(), 0, 0)
        }
    };
    let manifest = manifest::build(ManifestFields {
        solstone_version: request.solstone_version,
        exported_at: request.exported_at,
        source_journal,
        day_count,
        entity_count,
        facet_count,
    })
    .map_err(from_manifest_error)?;
    request
        .source
        .revalidate()
        .map_err(EncodeArchiveError::Source)?;

    let state = RefCell::new(SinkState::live());
    let sink = FileSink {
        file,
        state: &state,
    };
    let mut zip = ZipWriter::new(sink);
    #[cfg(any(test, feature = "test-hooks"))]
    test_set_boundary(TestBoundary::Body);
    let mut pending = crate::writer::write_archive(
        &mut zip,
        request.source,
        &manifest,
        request.day_window.as_ref(),
    )
    .err()
    .map(PendingFailure::from_encoding_error);
    if pending.is_none() {
        test_before_final_revalidate();
        pending = request
            .source
            .revalidate()
            .err()
            .map(PendingFailure::Source);
    }

    state.borrow_mut().phase = EncodingPhase::Finalize;
    let abort_error = if pending.is_some() {
        test_before_abort();
        zip.abort_file().err().map(io::Error::other)
    } else {
        None
    };
    test_before_finish();
    let finish_error = zip.finish().err().map(io::Error::other);

    resolve(state, pending, abort_error.or(finish_error))
}

fn resolve(
    state: RefCell<SinkState>,
    pending: Option<PendingFailure>,
    cleanup_error: Option<io::Error>,
) -> Result<(), EncodeArchiveError> {
    let state = state.into_inner();
    match state.poison {
        Poison::Faulted {
            error,
            phase: EncodingPhase::Body,
        } => Err(EncodeArchiveError::ArchiveWrite {
            member: None,
            source: error,
            followed: pending.map(PendingFailure::into_follow_on),
        }),
        Poison::Faulted {
            error,
            phase: EncodingPhase::Finalize,
        } => Err(EncodeArchiveError::ArchiveFinish {
            phase: EncodingPhase::Finalize,
            source: error,
            followed: pending.map(PendingFailure::into_follow_on),
        }),
        Poison::Live => {
            if let Some(source) = cleanup_error {
                Err(EncodeArchiveError::ArchiveFinish {
                    phase: EncodingPhase::Finalize,
                    source,
                    followed: pending.map(PendingFailure::into_follow_on),
                })
            } else if let Some(pending) = pending {
                Err(pending.into_error())
            } else {
                Ok(())
            }
        }
    }
}

enum PendingFailure {
    Source(ArchiveError),
    ArchiveWrite {
        member: Option<ArchiveMemberName>,
        source: io::Error,
    },
}

impl PendingFailure {
    fn from_encoding_error(error: ArchiveEncodingError) -> Self {
        match error {
            ArchiveEncodingError::Source { source, .. } => Self::Source(source),
            ArchiveEncodingError::Write { member, source } => Self::ArchiveWrite {
                member,
                source: match source {
                    WriteFailure::Io(error) => error,
                    WriteFailure::Zip(error) => io::Error::other(error),
                },
            },
        }
    }

    fn into_error(self) -> EncodeArchiveError {
        match self {
            Self::Source(source) => EncodeArchiveError::Source(source),
            Self::ArchiveWrite { member, source } => EncodeArchiveError::ArchiveWrite {
                member,
                source,
                followed: None,
            },
        }
    }

    fn into_follow_on(self) -> EncodeArchiveFollowOn {
        match self {
            Self::Source(source) => EncodeArchiveFollowOn::Source(source),
            Self::ArchiveWrite { member, source } => {
                EncodeArchiveFollowOn::ArchiveWrite { member, source }
            }
        }
    }
}

fn preflight_writer(file: &File) -> Result<(), EncodeArchiveError> {
    #[cfg(unix)]
    {
        preflight_writer_unix(file)
    }
    #[cfg(windows)]
    {
        preflight_writer_windows(file)
    }
}

#[cfg(unix)]
fn preflight_writer_unix(file: &File) -> Result<(), EncodeArchiveError> {
    #[cfg(any(test, feature = "test-hooks"))]
    if test_take_preflight_fault(TestPreflightOperation::Stat) {
        return Err(EncodeArchiveError::InvalidWriter {
            reason: "could not stat writer",
        });
    }
    let stat = fstat(file).map_err(|_| EncodeArchiveError::InvalidWriter {
        reason: "could not stat writer",
    })?;
    let mode = SFlag::from_bits_truncate(stat.st_mode);
    if mode & SFlag::S_IFMT != SFlag::S_IFREG {
        return Err(EncodeArchiveError::InvalidWriter {
            reason: "must be a regular file",
        });
    }

    #[cfg(any(test, feature = "test-hooks"))]
    if test_take_preflight_fault(TestPreflightOperation::Flags) {
        return Err(EncodeArchiveError::InvalidWriter {
            reason: "could not read writer flags",
        });
    }
    let raw_flags =
        fcntl(file, FcntlArg::F_GETFL).map_err(|_| EncodeArchiveError::InvalidWriter {
            reason: "could not read writer flags",
        })?;
    let flags = OFlag::from_bits_truncate(raw_flags);
    let access_mode = flags & OFlag::O_ACCMODE;
    if access_mode != OFlag::O_WRONLY && access_mode != OFlag::O_RDWR {
        return Err(EncodeArchiveError::InvalidWriter {
            reason: "must be opened for writing",
        });
    }
    if flags.contains(OFlag::O_APPEND) {
        return Err(EncodeArchiveError::InvalidWriter {
            reason: "must not be opened in append mode",
        });
    }
    if stat.st_size != 0 {
        return Err(EncodeArchiveError::InvalidWriter {
            reason: "must be empty",
        });
    }
    #[cfg(any(test, feature = "test-hooks"))]
    if test_take_preflight_fault(TestPreflightOperation::Position) {
        return Err(EncodeArchiveError::InvalidWriter {
            reason: "could not determine writer position",
        });
    }
    let position =
        lseek(file, 0, Whence::SeekCur).map_err(|_| EncodeArchiveError::InvalidWriter {
            reason: "could not determine writer position",
        })?;
    if position != 0 {
        return Err(EncodeArchiveError::InvalidWriter {
            reason: "must be positioned at the start",
        });
    }
    Ok(())
}

#[cfg(windows)]
fn preflight_writer_windows(file: &File) -> Result<(), EncodeArchiveError> {
    #[cfg(any(test, feature = "test-hooks"))]
    if test_take_preflight_fault(TestPreflightOperation::Stat) {
        return Err(EncodeArchiveError::InvalidWriter {
            reason: "could not stat writer",
        });
    }
    let metadata = file
        .metadata()
        .map_err(|_| EncodeArchiveError::InvalidWriter {
            reason: "could not stat writer",
        })?;
    if !metadata.file_type().is_file() {
        return Err(EncodeArchiveError::InvalidWriter {
            reason: "must be a regular file",
        });
    }

    // Windows does not expose a stable std-only query for the desired-access
    // bits held by an existing `File`. The Windows publisher opens this file
    // with read/write access itself; for another caller, the ZIP write is the
    // authoritative access check. Length and cursor are still checked before
    // ZIP construction so no existing output is accepted.
    #[cfg(any(test, feature = "test-hooks"))]
    if test_take_preflight_fault(TestPreflightOperation::Flags) {
        return Err(EncodeArchiveError::InvalidWriter {
            reason: "could not read writer flags",
        });
    }
    if metadata.len() != 0 {
        return Err(EncodeArchiveError::InvalidWriter {
            reason: "must be empty",
        });
    }
    #[cfg(any(test, feature = "test-hooks"))]
    if test_take_preflight_fault(TestPreflightOperation::Position) {
        return Err(EncodeArchiveError::InvalidWriter {
            reason: "could not determine writer position",
        });
    }
    let mut reader = file;
    let position = reader
        .stream_position()
        .map_err(|_| EncodeArchiveError::InvalidWriter {
            reason: "could not determine writer position",
        })?;
    if position != 0 {
        return Err(EncodeArchiveError::InvalidWriter {
            reason: "must be positioned at the start",
        });
    }
    Ok(())
}

fn validate_member_name_length(name: &ArchiveMemberName) -> Result<(), EncodeArchiveError> {
    if name.as_str().len() > usize::from(u16::MAX) {
        return Err(EncodeArchiveError::InvalidMetadata {
            field: "member_name",
            value: name.as_str().to_owned(),
            reason: "UTF-8 member path exceeds ZIP u16 byte limit",
        });
    }
    Ok(())
}

fn from_manifest_error(error: ManifestError) -> EncodeArchiveError {
    match error {
        ManifestError::InvalidMetadata {
            field,
            value,
            reason,
        } => EncodeArchiveError::InvalidMetadata {
            field,
            value,
            reason,
        },
    }
}

#[cfg(all(test, unix))]
#[allow(clippy::disallowed_methods, clippy::disallowed_types)]
mod tests {
    use std::fs::{self, File, OpenOptions};
    use std::io::{Seek, SeekFrom, Write};
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicU64, Ordering};

    use super::*;

    struct TempDir {
        path: PathBuf,
    }

    impl TempDir {
        fn new(name: &str) -> Self {
            static NEXT: AtomicU64 = AtomicU64::new(0);
            let path = std::env::temp_dir().join(format!(
                "solstone-core-journal-archive-encode-{name}-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir(&path).expect("create temporary directory");
            Self { path }
        }

        fn path(&self) -> &Path {
            &self.path
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.path);
        }
    }

    fn write_member(root: &Path, member: &str, bytes: &[u8]) -> PathBuf {
        let path = root.join(member);
        fs::create_dir_all(path.parent().expect("member parent")).expect("create member parents");
        fs::write(&path, bytes).expect("write member");
        path
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

    fn source_fixture(name: &str) -> (TempDir, ArchiveSource, PathBuf, PathBuf) {
        let temporary = TempDir::new(name);
        let root = temporary.path().join("journal");
        fs::create_dir(&root).expect("create journal root");
        let first = write_member(&root, "imports/first/source.bin", &noisy_bytes(192 * 1024));
        let second = write_member(&root, "imports/second/source.bin", &noisy_bytes(96 * 1024));
        let source = ArchiveSource::open(&root).expect("open source fixture");
        (temporary, source, first, second)
    }

    fn request(source: &ArchiveSource) -> EncodeArchiveRequest<'_> {
        EncodeArchiveRequest {
            source,
            solstone_version: "1.2.3",
            exported_at: "2040-01-02T03:04:59Z",
            day_window: None,
        }
    }

    fn install_control(
        fault: Option<TestFault>,
        action: Option<TestSourceAction>,
        preflight_fault: Option<TestPreflightOperation>,
    ) {
        super::install_encode_control(fault, action, preflight_fault);
    }

    fn take_control() -> TestControl {
        TEST_CONTROL
            .with(|control| std::mem::replace(&mut *control.borrow_mut(), TestControl::new()))
    }

    fn install_body_write_failure(member: &str) {
        TEST_CONTROL.with(|control| {
            control.borrow_mut().body_write_failure = Some(member.to_owned());
        });
    }

    fn fault(
        boundary: TestBoundary,
        operation: TestSinkOperation,
        ordinal: usize,
        kind: TestFaultKind,
    ) -> TestFault {
        TestFault {
            boundary,
            operation,
            ordinal,
            kind,
        }
    }

    fn assert_invalid_writer(file: &File, reason: &'static str) {
        assert!(matches!(
            preflight_writer(file),
            Err(EncodeArchiveError::InvalidWriter { reason: actual }) if actual == reason
        ));
    }

    #[test]
    fn preflight_accepts_an_empty_writable_file_at_the_start() {
        let temporary = TempDir::new("preflight-valid");
        let path = temporary.path().join("archive.zip");
        let file = File::create(&path).expect("create output file");

        assert!(preflight_writer(&file).is_ok());
        assert_eq!(file.metadata().expect("output metadata").len(), 0);
    }

    #[test]
    fn preflight_rejects_invalid_writer_forms_without_writing() {
        let temporary = TempDir::new("preflight-invalid");

        let directory = temporary.path().join("directory");
        fs::create_dir(&directory).expect("create directory");
        let directory_file = File::open(&directory).expect("open directory");
        assert_invalid_writer(&directory_file, "must be a regular file");

        let read_only_path = temporary.path().join("read-only.zip");
        File::create(&read_only_path).expect("create read-only fixture");
        let read_only = OpenOptions::new()
            .read(true)
            .open(&read_only_path)
            .expect("open read-only fixture");
        assert_invalid_writer(&read_only, "must be opened for writing");
        assert_eq!(
            fs::metadata(&read_only_path)
                .expect("read-only metadata")
                .len(),
            0
        );

        let append_path = temporary.path().join("append.zip");
        File::create(&append_path).expect("create append fixture");
        let append = OpenOptions::new()
            .append(true)
            .open(&append_path)
            .expect("open append fixture");
        assert_invalid_writer(&append, "must not be opened in append mode");
        assert_eq!(
            fs::metadata(&append_path).expect("append metadata").len(),
            0
        );

        let nonempty_path = temporary.path().join("nonempty.zip");
        fs::write(&nonempty_path, b"old").expect("write nonempty fixture");
        let nonempty = OpenOptions::new()
            .write(true)
            .open(&nonempty_path)
            .expect("open nonempty fixture");
        assert_invalid_writer(&nonempty, "must be empty");
        assert_eq!(
            fs::read(&nonempty_path).expect("read nonempty fixture"),
            b"old"
        );

        let offset_path = temporary.path().join("offset.zip");
        let mut offset = File::create(&offset_path).expect("create offset fixture");
        offset
            .seek(SeekFrom::Start(1))
            .expect("seek offset fixture");
        assert_invalid_writer(&offset, "must be positioned at the start");
        assert_eq!(
            fs::metadata(&offset_path).expect("offset metadata").len(),
            0
        );
    }

    #[test]
    fn first_fault_wins_over_later_faults() {
        let mut state = SinkState::live();
        state.record_fault(io::Error::other("first"));
        state.record_fault(io::Error::other("second"));

        assert!(matches!(
            state.poison,
            Poison::Faulted { error, phase: EncodingPhase::Body } if error.to_string() == "first"
        ));
    }

    #[test]
    fn nonempty_zero_write_latches_and_fabricates_full_success() {
        let mut state = SinkState::live();
        assert_eq!(
            state
                .handle_live_write(3, Ok(0))
                .expect("zero write is virtualized"),
            3
        );
        assert_eq!(state.virtual_write(2), 2);
        assert!(matches!(
            state.poison,
            Poison::Faulted { error, phase: EncodingPhase::Body }
                if error.kind() == io::ErrorKind::WriteZero
        ));
    }

    #[test]
    fn member_name_length_is_checked_in_utf8_bytes() {
        let maximum = ArchiveMemberName::new("a".repeat(usize::from(u16::MAX)));
        assert!(validate_member_name_length(&maximum).is_ok());

        let too_long = ArchiveMemberName::new("a".repeat(usize::from(u16::MAX) + 1));
        assert!(matches!(
            validate_member_name_length(&too_long),
            Err(EncodeArchiveError::InvalidMetadata {
                field: "member_name",
                value,
                reason: "UTF-8 member path exceeds ZIP u16 byte limit",
            }) if value.len() == usize::from(u16::MAX) + 1
        ));
    }

    #[test]
    fn injected_preflight_failures_are_typed_and_leave_zero_bytes() {
        for (operation, reason) in [
            (TestPreflightOperation::Stat, "could not stat writer"),
            (TestPreflightOperation::Flags, "could not read writer flags"),
            (
                TestPreflightOperation::Position,
                "could not determine writer position",
            ),
        ] {
            let (_temporary, source, _, _) = source_fixture("preflight-fault");
            let output = TempDir::new("preflight-fault-output");
            let path = output.path().join("archive.zip");
            let mut file = File::create(&path).expect("create output");
            install_control(None, None, Some(operation));

            assert!(matches!(
                encode_archive(&request(&source), &mut file),
                Err(EncodeArchiveError::InvalidWriter { reason: actual }) if actual == reason
            ));
            assert_eq!(file.metadata().expect("output metadata").len(), 0);
            assert_eq!(file.stream_position().expect("output position"), 0);
            assert!(take_control().preflight_fault.is_none());
        }
    }

    #[test]
    fn sink_fault_matrix_is_latched_before_zip_can_lose_the_writer() {
        let rows = [
            (
                TestBoundary::RootDirectory,
                TestSinkOperation::Write,
                TestFaultKind::Error,
                EncodingPhase::Body,
            ),
            (
                TestBoundary::RootDirectory,
                TestSinkOperation::Write,
                TestFaultKind::WriteZero,
                EncodingPhase::Body,
            ),
            (
                TestBoundary::SourcePayload,
                TestSinkOperation::Write,
                TestFaultKind::Error,
                EncodingPhase::Body,
            ),
            (
                TestBoundary::SourcePayload,
                TestSinkOperation::Write,
                TestFaultKind::WriteZero,
                EncodingPhase::Body,
            ),
            (
                TestBoundary::MemberTransition,
                TestSinkOperation::Write,
                TestFaultKind::Error,
                EncodingPhase::Body,
            ),
            (
                TestBoundary::MemberTransition,
                TestSinkOperation::Write,
                TestFaultKind::WriteZero,
                EncodingPhase::Body,
            ),
            (
                TestBoundary::MemberTransition,
                TestSinkOperation::Seek,
                TestFaultKind::Error,
                EncodingPhase::Body,
            ),
            (
                TestBoundary::ManifestStart,
                TestSinkOperation::Seek,
                TestFaultKind::Error,
                EncodingPhase::Body,
            ),
            (
                TestBoundary::Finish,
                TestSinkOperation::Write,
                TestFaultKind::Error,
                EncodingPhase::Finalize,
            ),
            (
                TestBoundary::CentralDirectory,
                TestSinkOperation::Write,
                TestFaultKind::Error,
                EncodingPhase::Finalize,
            ),
            (
                TestBoundary::Footer,
                TestSinkOperation::Write,
                TestFaultKind::Error,
                EncodingPhase::Finalize,
            ),
            (
                TestBoundary::TerminalSeek,
                TestSinkOperation::Seek,
                TestFaultKind::Error,
                EncodingPhase::Finalize,
            ),
        ];

        for (boundary, operation, kind, phase) in rows {
            let (_temporary, source, _, _) = source_fixture("sink-fault-matrix");
            let output = TempDir::new("sink-fault-output");
            let path = output.path().join("archive.zip");
            let mut file = File::create(&path).expect("create output");
            install_control(Some(fault(boundary, operation, 1, kind)), None, None);

            let result = encode_archive(&request(&source), &mut file);
            match (phase, result) {
                (
                    EncodingPhase::Body,
                    Err(EncodeArchiveError::ArchiveWrite {
                        followed: None,
                        source,
                        ..
                    }),
                ) => {
                    if kind == TestFaultKind::WriteZero {
                        assert_eq!(source.kind(), io::ErrorKind::WriteZero);
                    } else {
                        assert_eq!(source.to_string(), "injected sink fault");
                    }
                }
                (
                    EncodingPhase::Finalize,
                    Err(EncodeArchiveError::ArchiveFinish {
                        phase: EncodingPhase::Finalize,
                        followed: None,
                        source,
                    }),
                ) => assert_eq!(source.to_string(), "injected sink fault"),
                (_, other) => panic!("wrong result for {boundary:?}/{operation:?}: {other:?}"),
            }

            let control = take_control();
            assert!(control.fault.is_none(), "fault was not consumed");
            assert_eq!(control.finish_calls, 1, "finish count for {boundary:?}");
            assert_eq!(control.abort_calls, 0, "abort count for {boundary:?}");
            assert!(control.trace.iter().any(|event| event.faulted));
            assert!(
                !control
                    .trace
                    .iter()
                    .any(|event| event.operation == TestSinkOperation::Flush),
                "locked zip path unexpectedly flushed"
            );
            if boundary != TestBoundary::RootDirectory {
                assert_ne!(file.metadata().expect("output metadata").len(), 0);
                assert_ne!(file.stream_position().expect("output position"), 0);
            }
        }
    }

    #[test]
    fn source_failures_abort_then_finish_once_with_exact_provenance() {
        let cases = [
            ("open", "imports/first/source.bin"),
            ("early-eof", "imports/first/source.bin"),
            ("mid-read", "imports/first/source.bin"),
            ("final-revalidate", "imports/second/source.bin"),
        ];
        for (case, expected_member) in cases {
            let (_temporary, source, first, second) = source_fixture("source-fault");
            let action = match case {
                "open" => TestSourceAction::RemoveBeforeOpen {
                    member: "imports/first/source.bin".to_owned(),
                    path: first,
                },
                "early-eof" => TestSourceAction::TruncateBeforeRead {
                    member: "imports/first/source.bin".to_owned(),
                    copied: 0,
                    path: first,
                    length: 0,
                },
                "mid-read" => TestSourceAction::TruncateBeforeRead {
                    member: "imports/first/source.bin".to_owned(),
                    copied: 64 * 1024,
                    path: first,
                    length: 64 * 1024,
                },
                "final-revalidate" => TestSourceAction::AppendBeforeFinalRevalidate {
                    path: second,
                    bytes: vec![0x42],
                },
                _ => unreachable!(),
            };
            let output = TempDir::new("source-fault-output");
            let path = output.path().join("archive.zip");
            let mut file = File::create(&path).expect("create output");
            install_control(None, Some(action), None);

            assert!(matches!(
                encode_archive(&request(&source), &mut file),
                Err(EncodeArchiveError::Source(ArchiveError::SourceChanged {
                    member: Some(actual)
                })) if actual.as_str() == expected_member
            ));
            let control = take_control();
            assert!(control.action.is_none(), "source action was not consumed");
            assert_eq!(control.abort_calls, 1, "abort count for {case}");
            assert_eq!(control.finish_calls, 1, "finish count for {case}");
        }
    }

    #[test]
    fn output_and_source_failure_order_is_preserved() {
        let (_temporary, source, _, second) = source_fixture("ordered-faults");
        let output = TempDir::new("ordered-fault-output");
        let path = output.path().join("archive.zip");
        let mut file = File::create(&path).expect("create output");
        install_control(
            Some(fault(
                TestBoundary::SourcePayload,
                TestSinkOperation::Write,
                1,
                TestFaultKind::Error,
            )),
            Some(TestSourceAction::AppendBeforeFinalRevalidate {
                path: second,
                bytes: vec![0x42],
            }),
            None,
        );

        assert!(matches!(
            encode_archive(&request(&source), &mut file),
            Err(EncodeArchiveError::ArchiveWrite {
                followed: Some(EncodeArchiveFollowOn::Source(ArchiveError::SourceChanged {
                    member: Some(actual)
                })),
                ..
            }) if actual.as_str() == "imports/second/source.bin"
        ));
        let control = take_control();
        assert_eq!(control.abort_calls, 1);
        assert_eq!(control.finish_calls, 1);
    }

    #[test]
    fn abort_fault_retains_the_body_source_failure() {
        let (_temporary, source, first, _) = source_fixture("abort-fault");
        let output = TempDir::new("abort-fault-output");
        let path = output.path().join("archive.zip");
        let mut file = File::create(&path).expect("create output");
        install_control(
            Some(fault(
                TestBoundary::Abort,
                TestSinkOperation::Write,
                1,
                TestFaultKind::Error,
            )),
            Some(TestSourceAction::TruncateBeforeRead {
                member: "imports/first/source.bin".to_owned(),
                copied: 0,
                path: first,
                length: 0,
            }),
            None,
        );

        assert!(matches!(
            encode_archive(&request(&source), &mut file),
            Err(EncodeArchiveError::ArchiveFinish {
                phase: EncodingPhase::Finalize,
                followed: Some(EncodeArchiveFollowOn::Source(ArchiveError::SourceChanged {
                    member: Some(actual)
                })),
                ..
            }) if actual.as_str() == "imports/first/source.bin"
        ));
        let control = take_control();
        assert!(control.fault.is_none());
        assert_eq!(control.abort_calls, 1);
        assert_eq!(control.finish_calls, 1);
    }

    #[test]
    fn abort_fault_retains_the_body_write_failure() {
        let (_temporary, source, _, _) = source_fixture("abort-write-follow-on");
        let output = TempDir::new("abort-write-follow-on-output");
        let path = output.path().join("archive.zip");
        let mut file = File::create(&path).expect("create output");
        install_control(
            Some(fault(
                TestBoundary::Abort,
                TestSinkOperation::Write,
                1,
                TestFaultKind::Error,
            )),
            None,
            None,
        );
        install_body_write_failure("imports/first/source.bin");

        assert!(matches!(
            encode_archive(&request(&source), &mut file),
            Err(EncodeArchiveError::ArchiveFinish {
                phase: EncodingPhase::Finalize,
                followed: Some(EncodeArchiveFollowOn::ArchiveWrite {
                    member: Some(actual),
                    source,
                }),
                ..
            }) if actual.as_str() == "imports/first/source.bin"
                && source.to_string() == "injected body write failure"
        ));
        let control = take_control();
        assert!(control.fault.is_none());
        assert!(control.body_write_failure.is_none());
        assert_eq!(control.abort_calls, 1);
        assert_eq!(control.finish_calls, 1);
    }

    #[test]
    fn flush_fault_is_virtualized_at_the_adapter_boundary() {
        let temporary = TempDir::new("flush-fault");
        let path = temporary.path().join("output");
        let mut file = File::create(path).expect("create output");
        let state = RefCell::new(SinkState::live());
        install_control(
            Some(fault(
                TestBoundary::Body,
                TestSinkOperation::Flush,
                1,
                TestFaultKind::Error,
            )),
            None,
            None,
        );
        let mut sink = FileSink {
            file: &mut file,
            state: &state,
        };

        sink.flush().expect("flush fault is virtualized");
        assert!(matches!(
            &state.borrow().poison,
            Poison::Faulted {
                phase: EncodingPhase::Body,
                error,
            } if error.to_string() == "injected sink fault"
        ));
        assert!(take_control().fault.is_none());
    }
}
