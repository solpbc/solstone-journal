// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::ffi::{OsStr, OsString};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use crate::error::{EnumerationError, HealthDirectoryProbeError, OrdinaryTailError};

const REVERSE_TAIL_CHUNK_SIZE: usize = 65_536;

/// A proven metadata state of a probed path.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HealthDirectoryState {
    Absent,
    NotADirectory,
    Directory,
}

/// The portion of filesystem metadata needed by [`probe_health_directory`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProbeKind {
    Directory,
    Other,
}

/// Metadata seam for [`probe_health_directory`].
pub trait ProbeOps {
    fn metadata_kind(&self, path: &Path) -> io::Result<ProbeKind>;
}

/// Production [`ProbeOps`] implementation.
#[derive(Debug, Default)]
pub struct StdProbeOps;

impl ProbeOps for StdProbeOps {
    fn metadata_kind(&self, path: &Path) -> io::Result<ProbeKind> {
        // `metadata`, rather than `symlink_metadata`, intentionally follows a
        // symlink so a symlink to a directory is a proven directory.
        let metadata = fs::metadata(path)?;
        Ok(if metadata.is_dir() {
            ProbeKind::Directory
        } else {
            ProbeKind::Other
        })
    }
}

/// Probe a path's metadata without creating it.
///
/// The Python `today-day-path-permission` and `today-day-path-oserror`
/// fixture cases belong to journal-root resolution, which occurs in the
/// binary crate before this library receives a path. AC4 covers those
/// `resolve_process_journal_path()` failures at the CLI boundary.
pub fn probe_health_directory(
    path: &Path,
    ops: &dyn ProbeOps,
) -> Result<HealthDirectoryState, HealthDirectoryProbeError> {
    match ops.metadata_kind(path) {
        Ok(ProbeKind::Directory) => Ok(HealthDirectoryState::Directory),
        Ok(ProbeKind::Other) => Ok(HealthDirectoryState::NotADirectory),
        Err(source) if source.kind() == io::ErrorKind::NotFound => Ok(HealthDirectoryState::Absent),
        Err(source) if source.kind() == io::ErrorKind::NotADirectory => {
            Ok(HealthDirectoryState::NotADirectory)
        }
        Err(source) => Err(HealthDirectoryProbeError {
            path: path.to_path_buf(),
            source,
        }),
    }
}

/// Open file handle operations shared by both tail readers.
pub trait TailFile {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize>;
    fn seek(&mut self, position: io::SeekFrom) -> io::Result<u64>;
    fn stream_position(&mut self) -> io::Result<u64>;
}

impl TailFile for fs::File {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        std::io::Read::read(self, buffer)
    }

    fn seek(&mut self, position: io::SeekFrom) -> io::Result<u64> {
        std::io::Seek::seek(self, position)
    }

    fn stream_position(&mut self) -> io::Result<u64> {
        std::io::Seek::stream_position(self)
    }
}

/// Open-by-path seam for the tail readers.
pub trait TailFileOpener {
    fn open(&self, path: &Path) -> io::Result<Box<dyn TailFile>>;
}

/// Production [`TailFileOpener`] implementation.
#[derive(Debug, Default)]
pub struct StdTailFileOpener;

impl TailFileOpener for StdTailFileOpener {
    fn open(&self, path: &Path) -> io::Result<Box<dyn TailFile>> {
        Ok(Box::new(fs::File::open(path)?))
    }
}

/// Read a complete UTF-8 text file and retain Python `lines[-count:]`.
///
/// All `OSError` equivalents become an empty result, matching the retained
/// Python reader. Strict UTF-8 decode errors remain fatal.
pub fn tail_ordinary_text(
    path: &Path,
    count: i64,
    opener: &dyn TailFileOpener,
) -> Result<Vec<String>, OrdinaryTailError> {
    let mut file = match opener.open(path) {
        Ok(file) => file,
        Err(_) => return Ok(Vec::new()),
    };
    let bytes = match read_all(&mut *file) {
        Ok(bytes) => bytes,
        Err(_) => return Ok(Vec::new()),
    };
    let text = String::from_utf8(bytes).map_err(|source| OrdinaryTailError::InvalidUtf8 {
        path: path.to_path_buf(),
        source,
    })?;
    Ok(tail_slice(splitlines(&text), count))
}

/// Read tail chunks backward with retained per-chunk replacement decoding.
///
/// Every I/O error becomes an empty result, matching `tail_lines_large`.
pub fn tail_reverse_text(path: &Path, count: i64, opener: &dyn TailFileOpener) -> Vec<String> {
    let mut file = match opener.open(path) {
        Ok(file) => file,
        Err(_) => return Vec::new(),
    };
    if file.seek(io::SeekFrom::End(0)).is_err() {
        return Vec::new();
    }
    let size = match file.stream_position() {
        Ok(size) => size,
        Err(_) => return Vec::new(),
    };
    if size == 0 {
        return Vec::new();
    }

    let mut lines = Vec::new();
    let mut remaining = size;
    while remaining > 0 && count >= 0 && (lines.len() as u128) <= count as u128 {
        let read_size = REVERSE_TAIL_CHUNK_SIZE.min(remaining as usize);
        remaining -= read_size as u64;
        if file.seek(io::SeekFrom::Start(remaining)).is_err() {
            return Vec::new();
        }
        let mut chunk = vec![0; read_size];
        if read_exact(&mut *file, &mut chunk).is_err() {
            return Vec::new();
        }
        let decoded = String::from_utf8_lossy(&chunk);
        let mut chunk_lines = splitlines(&decoded);
        chunk_lines.append(&mut lines);
        lines = chunk_lines;
    }
    tail_slice(lines, count)
}

fn read_all(file: &mut dyn TailFile) -> io::Result<Vec<u8>> {
    let mut bytes = Vec::new();
    let mut buffer = [0; 8_192];
    loop {
        let read = file.read(&mut buffer)?;
        if read == 0 {
            return Ok(bytes);
        }
        bytes.extend_from_slice(&buffer[..read]);
    }
}

fn read_exact(file: &mut dyn TailFile, buffer: &mut [u8]) -> io::Result<()> {
    let mut filled = 0;
    while filled < buffer.len() {
        let read = file.read(&mut buffer[filled..])?;
        if read == 0 {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "tail file ended during chunk read",
            ));
        }
        filled += read;
    }
    Ok(())
}

pub(crate) fn tail_slice<T>(mut lines: Vec<T>, count: i64) -> Vec<T> {
    let start = if count == 0 {
        0
    } else if count > 0 {
        lines.len().saturating_sub(count as usize)
    } else {
        lines.len().min(count.unsigned_abs() as usize)
    };
    lines.drain(..start);
    lines
}

fn splitlines(text: &str) -> Vec<String> {
    let mut lines = Vec::new();
    let mut start = 0;
    let mut scalars = text.char_indices().peekable();
    while let Some((index, scalar)) = scalars.next() {
        let boundary_len = match scalar {
            '\n' | '\x0b' | '\x0c' | '\x1c'..='\x1e' | '\u{0085}' | '\u{2028}' | '\u{2029}' => {
                scalar.len_utf8()
            }
            '\r' => {
                if let Some(&(next, '\n')) = scalars.peek() {
                    scalars.next();
                    next + 1 - index
                } else {
                    1
                }
            }
            _ => continue,
        };
        lines.push(text[start..index].to_owned());
        start = index + boundary_len;
    }
    if start < text.len() {
        lines.push(text[start..].to_owned());
    }
    lines
}

/// One direct directory entry used by [`DayLogDirectoryOps`].
#[derive(Debug, Clone)]
pub struct DayLogEntry {
    pub name: OsString,
    pub path: PathBuf,
}

/// Directory enumeration seam, including failures during iteration or stat.
pub trait DayLogDirectoryOps {
    fn read_dir(
        &self,
        path: &Path,
    ) -> io::Result<Box<dyn Iterator<Item = io::Result<DayLogEntry>>>>;
    fn is_symlink(&self, entry: &DayLogEntry) -> io::Result<bool>;
}

/// Production [`DayLogDirectoryOps`] implementation.
#[derive(Debug, Default)]
pub struct StdDayLogDirectoryOps;

impl DayLogDirectoryOps for StdDayLogDirectoryOps {
    fn read_dir(
        &self,
        path: &Path,
    ) -> io::Result<Box<dyn Iterator<Item = io::Result<DayLogEntry>>>> {
        let entries = fs::read_dir(path)?.map(|entry| {
            entry.map(|entry| DayLogEntry {
                name: entry.file_name(),
                path: entry.path(),
            })
        });
        Ok(Box::new(entries))
    }

    fn is_symlink(&self, entry: &DayLogEntry) -> io::Result<bool> {
        Ok(fs::symlink_metadata(&entry.path)?.file_type().is_symlink())
    }
}

/// Return `*.log` entries that are themselves symlinks, in CPython's Linux
/// filesystem-decoded order.
pub fn list_day_log_symlinks(
    health_dir: &Path,
    ops: &dyn DayLogDirectoryOps,
) -> Result<Vec<OsString>, EnumerationError> {
    let entries = match ops.read_dir(health_dir) {
        Ok(entries) => entries,
        Err(source)
            if matches!(
                source.kind(),
                io::ErrorKind::NotFound | io::ErrorKind::NotADirectory
            ) =>
        {
            return Ok(Vec::new());
        }
        Err(source) => return Err(enumeration_error(health_dir, source)),
    };

    let mut matched = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|source| enumeration_error(health_dir, source))?;
        if !entry.name.as_encoded_bytes().ends_with(b".log") {
            continue;
        }
        if ops
            .is_symlink(&entry)
            .map_err(|source| enumeration_error(&entry.path, source))?
        {
            matched.push((python_surrogateescape_sort_key(&entry.name), entry.name));
        }
    }
    matched.sort_by(|left, right| left.0.cmp(&right.0));
    Ok(matched.into_iter().map(|(_, name)| name).collect())
}

fn enumeration_error(path: &Path, source: io::Error) -> EnumerationError {
    EnumerationError::Enumerate {
        path: path.to_path_buf(),
        source,
    }
}

fn python_surrogateescape_sort_key(name: &OsStr) -> Vec<u32> {
    let mut key = Vec::new();
    let mut remaining = name.as_encoded_bytes();
    while !remaining.is_empty() {
        match std::str::from_utf8(remaining) {
            Ok(valid) => {
                key.extend(valid.chars().map(u32::from));
                break;
            }
            Err(error) => {
                let prefix = std::str::from_utf8(&remaining[..error.valid_up_to()])
                    .expect("valid UTF-8 prefix");
                key.extend(prefix.chars().map(u32::from));
                key.push(0xdc00 + u32::from(remaining[error.valid_up_to()]));
                remaining = &remaining[error.valid_up_to() + 1..];
            }
        }
    }
    key
}

pub(crate) fn sort_os_strings_like_python(names: &mut [OsString]) {
    names.sort_by(|left, right| {
        let ordering =
            python_surrogateescape_sort_key(left).cmp(&python_surrogateescape_sort_key(right));
        if ordering == std::cmp::Ordering::Equal {
            std::cmp::Ordering::Equal
        } else {
            ordering
        }
    });
}

#[cfg(all(test, unix))]
mod tests {
    use std::cell::RefCell;
    use std::ffi::OsString;
    use std::io;
    use std::os::unix::ffi::{OsStrExt, OsStringExt};
    use std::os::unix::fs::symlink;
    use std::path::{Path, PathBuf};
    use std::rc::Rc;

    use base64::Engine as _;
    use serde_json::Value;
    use sha2::{Digest, Sha256};
    use tempfile::TempDir;

    use super::*;
    use crate::fixture;

    #[derive(Clone, Copy)]
    enum Boundary {
        None,
        Open,
        Read,
        InitialSeek,
        Tell,
        LoopSeek,
    }

    impl Boundary {
        fn parse(value: &str) -> Self {
            match value {
                "open" => Self::Open,
                "read" => Self::Read,
                "initial_seek" => Self::InitialSeek,
                "tell" => Self::Tell,
                "loop_seek" => Self::LoopSeek,
                _ => panic!("unknown fixture boundary {value}"),
            }
        }
    }

    #[derive(Default)]
    struct TailState {
        closed: bool,
    }

    struct FakeTailOpener {
        bytes: Vec<u8>,
        boundary: Boundary,
        state: Rc<RefCell<TailState>>,
    }

    impl FakeTailOpener {
        fn new(bytes: Vec<u8>, boundary: Boundary) -> (Self, Rc<RefCell<TailState>>) {
            let state = Rc::new(RefCell::new(TailState::default()));
            (
                Self {
                    bytes,
                    boundary,
                    state: state.clone(),
                },
                state,
            )
        }
    }

    impl TailFileOpener for FakeTailOpener {
        fn open(&self, _path: &Path) -> io::Result<Box<dyn TailFile>> {
            if matches!(self.boundary, Boundary::Open) {
                return Err(injected_error());
            }
            Ok(Box::new(FakeTailFile {
                bytes: self.bytes.clone(),
                position: 0,
                boundary: self.boundary,
                state: self.state.clone(),
            }))
        }
    }

    struct FakeTailFile {
        bytes: Vec<u8>,
        position: usize,
        boundary: Boundary,
        state: Rc<RefCell<TailState>>,
    }

    impl Drop for FakeTailFile {
        fn drop(&mut self) {
            self.state.borrow_mut().closed = true;
        }
    }

    impl TailFile for FakeTailFile {
        fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
            if matches!(self.boundary, Boundary::Read) {
                return Err(injected_error());
            }
            let available = &self.bytes[self.position..];
            let count = available.len().min(buffer.len());
            buffer[..count].copy_from_slice(&available[..count]);
            self.position += count;
            Ok(count)
        }

        fn seek(&mut self, position: io::SeekFrom) -> io::Result<u64> {
            let is_initial = matches!(position, io::SeekFrom::End(0));
            if (is_initial && matches!(self.boundary, Boundary::InitialSeek))
                || (!is_initial && matches!(self.boundary, Boundary::LoopSeek))
            {
                return Err(injected_error());
            }
            self.position = match position {
                io::SeekFrom::End(0) => self.bytes.len(),
                io::SeekFrom::Start(position) => position as usize,
                _ => panic!("unexpected fake seek"),
            };
            Ok(self.position as u64)
        }

        fn stream_position(&mut self) -> io::Result<u64> {
            if matches!(self.boundary, Boundary::Tell) {
                return Err(injected_error());
            }
            Ok(self.position as u64)
        }
    }

    fn injected_error() -> io::Error {
        io::Error::other("injected OSError")
    }

    struct FakeProbeOps {
        result: RefCell<Option<io::Result<ProbeKind>>>,
    }

    impl ProbeOps for FakeProbeOps {
        fn metadata_kind(&self, _path: &Path) -> io::Result<ProbeKind> {
            self.result
                .borrow_mut()
                .take()
                .expect("one fake metadata call")
        }
    }

    struct FakeDayLogDirectoryOps {
        entries: RefCell<Option<io::Result<Vec<io::Result<DayLogEntry>>>>>,
        symlink: io::Result<bool>,
    }

    impl DayLogDirectoryOps for FakeDayLogDirectoryOps {
        fn read_dir(
            &self,
            _path: &Path,
        ) -> io::Result<Box<dyn Iterator<Item = io::Result<DayLogEntry>>>> {
            let entries = self
                .entries
                .borrow_mut()
                .take()
                .expect("one fake directory read")?;
            Ok(Box::new(entries.into_iter()))
        }

        fn is_symlink(&self, _entry: &DayLogEntry) -> io::Result<bool> {
            match &self.symlink {
                Ok(value) => Ok(*value),
                Err(_) => Err(injected_error()),
            }
        }
    }

    #[test]
    fn health_log_io_fixture_is_pinned_and_complete() {
        assert_eq!(fixture::raw_sha256(), fixture::FIXTURE_SHA256);
        let fixture = fixture::fixture();
        assert_eq!(fixture.metadata.chunk_size, REVERSE_TAIL_CHUNK_SIZE);
        assert_eq!(fixture.metadata.platform, "linux");
        assert_eq!(
            fixture.metadata.reference_path,
            "solstone/think/logs_cli.py"
        );
        assert_eq!(
            fixture.metadata.reference_git_blob_oid,
            "ed8583156c11c27a9986c2790ee410e4f8e362bc"
        );
        assert_eq!(
            fixture.metadata.reference_sha256,
            "f2ce46d928dc7c1a2922b8060e95c26b610cfe4eae250370571dc532ceed7a7f"
        );
    }

    #[test]
    fn consumes_every_in_scope_health_directory_fixture_case() {
        let root = TempDir::new().unwrap();
        let health = root.path().join("health");
        std::fs::create_dir(&health).unwrap();
        let link = root.path().join("health-link");
        symlink(&health, &link).unwrap();
        let missing = root.path().join("missing");
        let standard = StdProbeOps;

        for case in fixture::fixture()
            .cases
            .iter()
            .filter(|case| case.family == "today_health_directory")
        {
            match case.id.as_str() {
                "today-health-directory-existing" => {
                    assert_eq!(
                        probe_health_directory(&health, &standard).unwrap(),
                        HealthDirectoryState::Directory
                    )
                }
                "today-health-directory-missing" => {
                    assert_eq!(
                        probe_health_directory(&missing, &standard).unwrap(),
                        HealthDirectoryState::Absent
                    )
                }
                "today-health-directory-symlink" => {
                    assert_eq!(
                        probe_health_directory(&link, &standard).unwrap(),
                        HealthDirectoryState::Directory
                    )
                }
                "today-is-dir-permission" | "today-is-dir-oserror" => {
                    let fake = FakeProbeOps {
                        result: RefCell::new(Some(Err(injected_error()))),
                    };
                    assert!(probe_health_directory(&health, &fake).is_err());
                }
                "today-day-path-permission" | "today-day-path-oserror" => {}
                unexpected => panic!("unexpected fixture case {unexpected}"),
            }
        }

        let ordinary_file = root.path().join("file");
        std::fs::write(&ordinary_file, b"not a directory").unwrap();
        assert_eq!(
            probe_health_directory(&ordinary_file, &standard).unwrap(),
            HealthDirectoryState::NotADirectory
        );
    }

    #[test]
    fn consumes_all_ordinary_tail_fixture_cases() {
        for case in fixture::fixture()
            .cases
            .iter()
            .filter(|case| case.family == "ordinary_tail")
        {
            let temporary = TempDir::new().unwrap();
            let path = materialize_fixture_input(temporary.path(), case.input.as_ref());
            for call in calls(case) {
                let count = call["n"].as_i64().unwrap();
                let result = if let Some(boundary) = &case.injected_boundary {
                    let (fake, _) = FakeTailOpener::new(Vec::new(), Boundary::parse(boundary));
                    tail_ordinary_text(&path, count, &fake)
                } else {
                    tail_ordinary_text(&path, count, &StdTailFileOpener)
                };
                let expected = &call["outcome"];
                if expected["kind"] == "raise" {
                    assert!(
                        matches!(result, Err(OrdinaryTailError::InvalidUtf8 { .. })),
                        "{}",
                        case.id
                    );
                } else {
                    assert_lines_match(&result.unwrap(), &expected["value"], &case.id);
                }
            }
        }
    }

    #[test]
    fn consumes_all_reverse_tail_fixture_cases_and_closes_handles() {
        for case in fixture::fixture()
            .cases
            .iter()
            .filter(|case| case.family == "reverse_tail")
        {
            let temporary = TempDir::new().unwrap();
            let path = materialize_fixture_input(temporary.path(), case.input.as_ref());
            let input_bytes = case
                .input
                .as_ref()
                .and_then(|input| input.get("recipe"))
                .map(recipe_bytes)
                .unwrap_or_default();
            for call in calls(case) {
                let count = call["n"].as_i64().unwrap();
                let (result, state) = if let Some(boundary) = &case.injected_boundary {
                    let (fake, state) =
                        FakeTailOpener::new(input_bytes.clone(), Boundary::parse(boundary));
                    (tail_reverse_text(&path, count, &fake), Some(state))
                } else if case.id == "reverse-success-closes-descriptor" {
                    let (fake, state) = FakeTailOpener::new(b"a\nb\n".to_vec(), Boundary::None);
                    (tail_reverse_text(&path, count, &fake), Some(state))
                } else {
                    (tail_reverse_text(&path, count, &StdTailFileOpener), None)
                };
                assert_lines_match(&result, &call["outcome"]["value"], &case.id);
                if call["descriptor_closed"] == Value::Bool(true) {
                    assert!(
                        state.expect("injected handle").borrow().closed,
                        "{}",
                        case.id
                    );
                }
            }
        }
    }

    #[test]
    fn consumes_all_day_log_enumeration_fixture_cases() {
        for case in fixture::fixture()
            .cases
            .iter()
            .filter(|case| case.family == "day_log_enumeration")
        {
            match case.id.as_str() {
                "enumeration-membership-and-order" => {
                    if cfg!(target_os = "macos") {
                        continue;
                    }
                    let temporary = TempDir::new().unwrap();
                    let directory = temporary.path().join("health");
                    std::fs::create_dir(&directory).unwrap();
                    let target = temporary.path().join("target");
                    std::fs::write(&target, b"target").unwrap();
                    let target_directory = temporary.path().join("target-directory");
                    std::fs::create_dir(&target_directory).unwrap();
                    let expected = names_from_value(&case.forward.as_ref().unwrap()["value"]);
                    for name in &expected {
                        let target = if name.as_bytes().starts_with(b"b-") {
                            temporary.path().join("missing-target")
                        } else if name.as_bytes().starts_with(b"c-") {
                            target_directory.clone()
                        } else {
                            target.clone()
                        };
                        symlink(target, directory.join(name)).unwrap();
                    }
                    assert_eq!(
                        raw_names(
                            list_day_log_symlinks(&directory, &StdDayLogDirectoryOps).unwrap()
                        ),
                        raw_names(expected)
                    );
                }
                "enumeration-missing-directory" => {
                    let temporary = TempDir::new().unwrap();
                    assert!(
                        list_day_log_symlinks(
                            &temporary.path().join("missing"),
                            &StdDayLogDirectoryOps
                        )
                        .unwrap()
                        .is_empty()
                    );
                }
                "enumeration-injected-permission" | "enumeration-injected-oserror" => {
                    let fake = FakeDayLogDirectoryOps {
                        entries: RefCell::new(Some(Err(injected_error()))),
                        symlink: Ok(true),
                    };
                    assert!(list_day_log_symlinks(Path::new("health"), &fake).is_err());
                }
                "enumeration-disappears-after-one" => {
                    let fake = FakeDayLogDirectoryOps {
                        entries: RefCell::new(Some(Ok(vec![
                            Ok(DayLogEntry {
                                name: OsString::from("first.log"),
                                path: PathBuf::from("health/first.log"),
                            }),
                            Err(io::Error::new(io::ErrorKind::NotFound, "disappeared")),
                        ]))),
                        symlink: Ok(true),
                    };
                    assert!(list_day_log_symlinks(Path::new("health"), &fake).is_err());
                }
                unexpected => panic!("unexpected fixture case {unexpected}"),
            }
        }
    }

    #[test]
    #[cfg(target_os = "linux")]
    fn surrogateescape_sorting_beats_raw_directory_order() {
        let temporary = TempDir::new().unwrap();
        let directory = temporary.path().join("health");
        std::fs::create_dir(&directory).unwrap();
        let target = temporary.path().join("target");
        std::fs::write(&target, b"target").unwrap();
        let invalid = OsString::from_vec(b"a-\x80.log".to_vec());
        let valid = OsString::from_vec(b"a-\xc2\x80.log".to_vec());
        symlink(&target, directory.join(&invalid)).unwrap();
        symlink(&target, directory.join(&valid)).unwrap();
        assert_eq!(
            raw_names(list_day_log_symlinks(&directory, &StdDayLogDirectoryOps).unwrap()),
            vec![valid.as_bytes().to_vec(), invalid.as_bytes().to_vec()]
        );
    }

    #[test]
    fn sortable_key_keeps_valid_u0080_before_invalid_byte() {
        let mut names = vec![
            OsString::from_vec(b"a-\x80.log".to_vec()),
            OsString::from_vec(b"a-\xc2\x80.log".to_vec()),
        ];
        sort_os_strings_like_python(&mut names);
        assert_eq!(names[0].as_bytes(), b"a-\xc2\x80.log");
        assert_eq!(names[1].as_bytes(), b"a-\x80.log");
    }

    fn calls(case: &crate::fixture::Case) -> &[Value] {
        case.calls
            .as_ref()
            .and_then(Value::as_array)
            .expect("tail fixture calls")
    }

    fn materialize_fixture_input(root: &Path, input: Option<&Value>) -> PathBuf {
        let Some(input) = input else {
            return root.join("injected.log");
        };
        if let Some(disposition) = input.get("disposition").and_then(Value::as_str) {
            return match disposition {
                "missing" => root.join("missing.log"),
                "directory" => root.to_path_buf(),
                _ => panic!("unknown fixture disposition {disposition}"),
            };
        }
        let bytes = recipe_bytes(&input["recipe"]);
        assert_eq!(
            format!("{:x}", Sha256::digest(&bytes)),
            input["sha256"].as_str().unwrap()
        );
        assert_eq!(bytes.len(), input["size"].as_u64().unwrap() as usize);
        let path = root.join("fixture.log");
        std::fs::write(&path, bytes).unwrap();
        path
    }

    fn recipe_bytes(recipe: &Value) -> Vec<u8> {
        if let Some(base64) = recipe.get("base64").and_then(Value::as_str) {
            return base64::engine::general_purpose::STANDARD
                .decode(base64)
                .unwrap();
        }
        let mut output = Vec::new();
        for segment in recipe["segments"].as_array().unwrap() {
            let bytes = if let Some(ascii) = segment.get("ascii").and_then(Value::as_str) {
                ascii.as_bytes().to_vec()
            } else {
                decode_hex(segment["hex"].as_str().unwrap())
            };
            for _ in 0..segment["repeat"].as_u64().unwrap() {
                output.extend_from_slice(&bytes);
            }
        }
        output
    }

    fn decode_hex(input: &str) -> Vec<u8> {
        input
            .as_bytes()
            .chunks_exact(2)
            .map(|digits| u8::from_str_radix(std::str::from_utf8(digits).unwrap(), 16).unwrap())
            .collect()
    }

    fn assert_lines_match(actual: &[String], expected: &Value, label: &str) {
        let expected = expected.as_array().unwrap();
        assert_eq!(actual.len(), expected.len(), "{label}");
        for (actual, expected) in actual.iter().zip(expected) {
            if let Some(expected) = expected.as_str() {
                assert_eq!(actual, expected, "{label}");
                continue;
            }
            let large = &expected["large_string"];
            assert!(
                actual.starts_with(large["prefix"].as_str().unwrap()),
                "{label}"
            );
            assert!(
                actual.ends_with(large["suffix"].as_str().unwrap()),
                "{label}"
            );
            assert_eq!(
                actual.chars().count(),
                large["scalars"].as_u64().unwrap() as usize,
                "{label}"
            );
            assert_eq!(
                actual.len(),
                large["utf8_bytes"].as_u64().unwrap() as usize,
                "{label}"
            );
            assert_eq!(
                format!("{:x}", Sha256::digest(actual.as_bytes())),
                large["sha256"].as_str().unwrap(),
                "{label}"
            );
            assert_eq!(
                actual.matches('\u{fffd}').count(),
                large["replacement_count"].as_u64().unwrap() as usize,
                "{label}"
            );
        }
    }

    fn names_from_value(value: &Value) -> Vec<OsString> {
        value
            .as_array()
            .unwrap()
            .iter()
            .map(|entry| {
                OsString::from_vec(
                    base64::engine::general_purpose::STANDARD
                        .decode(entry["name_base64"].as_str().unwrap())
                        .unwrap(),
                )
            })
            .collect()
    }

    fn raw_names(names: Vec<OsString>) -> Vec<Vec<u8>> {
        names
            .into_iter()
            .map(|name| name.as_bytes().to_vec())
            .collect()
    }
}
