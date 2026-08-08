// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::cell::RefCell;
use std::error::Error;
use std::fmt;
use std::fs::File;
use std::io::{self, Seek, SeekFrom, Write};

use nix::fcntl::{FcntlArg, OFlag, fcntl};
use nix::sys::stat::{SFlag, fstat};
use nix::unistd::{Whence, lseek};
use zip::ZipWriter;

use crate::manifest::{self, ManifestFields};
use crate::writer::{ArchiveEncodingError, WriteFailure};
use crate::{ArchiveError, ArchiveMemberName, ArchiveSource};

/// The frozen source and metadata to encode into a portable archive.
pub struct EncodeArchiveRequest<'a> {
    pub source: &'a ArchiveSource,
    pub solstone_version: &'a str,
    pub exported_at: &'a str,
}

/// The stage in which an output-file fault was observed.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EncodingPhase {
    Body,
    Finalize,
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
    },
    ArchiveFinish {
        phase: EncodingPhase,
        source: io::Error,
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
            } => write!(formatter, "archive write {}: {source}", member.as_str()),
            Self::ArchiveWrite {
                member: None,
                source,
            } => write!(formatter, "archive write: {source}"),
            Self::ArchiveFinish { phase, source } => {
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
    Abandoned,
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

    fn abandon(&mut self) {
        if matches!(&self.poison, Poison::Live) {
            self.poison = Poison::Abandoned;
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
        let result = self.file.write(buffer);
        self.state
            .borrow_mut()
            .handle_live_write(buffer.len(), result)
    }

    fn flush(&mut self) -> io::Result<()> {
        if !self.is_live() {
            return Ok(());
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
    let manifest = manifest::build(ManifestFields {
        solstone_version: request.solstone_version,
        exported_at: request.exported_at,
        source_journal,
        day_count: inventory.day_count(),
        entity_count: inventory.entity_count(),
        facet_count: inventory.facet_count(),
    })
    .map_err(from_encoding_error)?;
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
    if let Err(error) = crate::writer::write_archive(&mut zip, request.source, &manifest) {
        state.borrow_mut().abandon();
        drop(zip);
        return resolve(state, Err(from_encoding_error(error)));
    }
    if let Err(error) = request.source.revalidate() {
        state.borrow_mut().abandon();
        drop(zip);
        return resolve(state, Err(EncodeArchiveError::Source(error)));
    }

    state.borrow_mut().phase = EncodingPhase::Finalize;
    let finished = zip.finish();
    match finished {
        Ok(_) => resolve(state, Ok(())),
        Err(error) => resolve(
            state,
            Err(EncodeArchiveError::ArchiveWrite {
                member: None,
                source: io::Error::other(error),
            }),
        ),
    }
}

fn resolve(
    state: RefCell<SinkState>,
    fallback: Result<(), EncodeArchiveError>,
) -> Result<(), EncodeArchiveError> {
    let state = state.into_inner();
    match state.poison {
        // A real output fault always wins because the file's contents are no longer trustworthy.
        Poison::Faulted { error, phase } => Err(EncodeArchiveError::ArchiveFinish {
            phase,
            source: error,
        }),
        Poison::Live | Poison::Abandoned => fallback,
    }
}

fn preflight_writer(file: &File) -> Result<(), EncodeArchiveError> {
    let stat = fstat(file).map_err(|_| EncodeArchiveError::InvalidWriter {
        reason: "could not stat writer",
    })?;
    let mode = SFlag::from_bits_truncate(stat.st_mode);
    if mode & SFlag::S_IFMT != SFlag::S_IFREG {
        return Err(EncodeArchiveError::InvalidWriter {
            reason: "must be a regular file",
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

fn validate_member_name_length(name: &ArchiveMemberName) -> Result<(), EncodeArchiveError> {
    if name.as_str().len() > usize::from(u16::MAX) {
        return Err(EncodeArchiveError::InvalidMetadata {
            field: "member_name",
            value: name.as_str().to_owned(),
            reason: "must be at most 65535 bytes",
        });
    }
    Ok(())
}

fn from_encoding_error(error: ArchiveEncodingError) -> EncodeArchiveError {
    match error {
        ArchiveEncodingError::Source { source, .. } => EncodeArchiveError::Source(source),
        ArchiveEncodingError::Write { member, source } => match source {
            WriteFailure::Io(error) => EncodeArchiveError::ArchiveWrite {
                member,
                source: error,
            },
            WriteFailure::Zip(error) => EncodeArchiveError::ArchiveWrite {
                member,
                source: io::Error::other(error),
            },
        },
        ArchiveEncodingError::InvalidMetadata {
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

#[cfg(test)]
#[allow(clippy::disallowed_methods, clippy::disallowed_types)]
mod tests {
    use std::fs::{self, File, OpenOptions};
    use std::io::{Seek, SeekFrom};
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
    fn first_fault_wins_over_later_faults_and_abandonment() {
        let mut state = SinkState::live();
        state.record_fault(io::Error::other("first"));
        state.record_fault(io::Error::other("second"));
        state.abandon();

        assert!(matches!(
            state.poison,
            Poison::Faulted { error, phase: EncodingPhase::Body } if error.to_string() == "first"
        ));
    }

    #[test]
    fn abandonment_only_changes_live_state() {
        let mut state = SinkState::live();
        state.abandon();
        assert!(matches!(state.poison, Poison::Abandoned));
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
                reason: "must be at most 65535 bytes",
            }) if value.len() == usize::from(u16::MAX) + 1
        ));
    }
}
