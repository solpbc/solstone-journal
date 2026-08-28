// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Injected state machine for following top-level operational service logs.

use std::collections::HashMap;
use std::ffi::OsString;
use std::fs;
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::path::{Component, Path, PathBuf};
use std::time::Duration;

use solstone_core_system::operational_log_parse::parse_health_log_row;
use solstone_core_system_health::sanitize_os_bytes_for_terminal;

use crate::read::sort_os_strings_like_python;
use crate::render::render_stream_row;

const READ_CHUNK_SIZE: usize = 4096;
const ROTATION_INTERVAL: Duration = Duration::from_secs(2);

/// A strict UTF-8 universal-newline reader failure.
#[derive(Debug)]
pub enum FollowReadError {
    InvalidUtf8,
    Io(io::Error),
}

/// Continuous text reader used by the follower.
pub trait FollowReader {
    /// Return one normalized line without its terminator, an unterminated
    /// final fragment, or `None` at true EOF.
    fn read_line(&mut self) -> Result<Option<String>, FollowReadError>;
    /// Seek an initial/discovered source to EOF before it is tracked.
    fn seek_to_end(&mut self) -> io::Result<()>;
}

/// Filesystem boundary required by the pure follower state machine.
pub trait FollowFs {
    /// List direct `*.log` names. The implementation returns CPython's
    /// filesystem-decoded ordering and deliberately includes non-symlinks.
    fn list_top_level_logs(&self, health_dir: &Path) -> io::Result<Vec<OsString>>;
    /// Observe the current final-path type. `Ok(false)` covers a proven
    /// missing path and a proven non-symlink.
    fn is_symlink(&self, path: &Path) -> io::Result<bool>;
    /// Resolve a source path with `Path.resolve(strict=False)` semantics.
    fn resolve(&self, path: &Path) -> io::Result<PathBuf>;
    /// Open a source at offset zero. Initial discovery separately seeks to
    /// EOF; rotation deliberately does not.
    fn open(&self, resolved: &Path) -> io::Result<Box<dyn FollowReader>>;
}

/// Production filesystem implementation of [`FollowFs`].
#[derive(Debug, Default)]
pub struct StdFollowFs;

impl FollowFs for StdFollowFs {
    fn list_top_level_logs(&self, health_dir: &Path) -> io::Result<Vec<OsString>> {
        let mut names = Vec::new();
        for entry in fs::read_dir(health_dir)? {
            let entry = entry?;
            let name = entry.file_name();
            if name.as_encoded_bytes().ends_with(b".log") {
                names.push(name);
            }
        }
        sort_os_strings_like_python(&mut names);
        Ok(names)
    }

    fn is_symlink(&self, path: &Path) -> io::Result<bool> {
        match fs::symlink_metadata(path) {
            Ok(metadata) => Ok(metadata.file_type().is_symlink()),
            Err(error)
                if matches!(
                    error.kind(),
                    io::ErrorKind::NotFound | io::ErrorKind::NotADirectory
                ) =>
            {
                Ok(false)
            }
            Err(error) => Err(error),
        }
    }

    fn resolve(&self, path: &Path) -> io::Result<PathBuf> {
        resolve_non_strict(&StdResolveFs, path)
    }

    fn open(&self, resolved: &Path) -> io::Result<Box<dyn FollowReader>> {
        Ok(Box::new(FileFollowReader::open(resolved)?))
    }
}

trait ResolveFs {
    fn current_dir(&self) -> io::Result<PathBuf>;
    fn is_symlink(&self, path: &Path) -> io::Result<bool>;
    fn read_link(&self, path: &Path) -> io::Result<PathBuf>;
}

struct StdResolveFs;

impl ResolveFs for StdResolveFs {
    fn current_dir(&self) -> io::Result<PathBuf> {
        std::env::current_dir()
    }

    fn is_symlink(&self, path: &Path) -> io::Result<bool> {
        fs::symlink_metadata(path).map(|metadata| metadata.file_type().is_symlink())
    }

    fn read_link(&self, path: &Path) -> io::Result<PathBuf> {
        fs::read_link(path)
    }
}

fn resolve_non_strict(fs: &dyn ResolveFs, path: &Path) -> io::Result<PathBuf> {
    let base = if path.is_absolute() {
        PathBuf::new()
    } else {
        fs.current_dir()?
    };
    resolve_components(fs, base, path, &mut HashMap::new())
}

fn resolve_components(
    fs: &dyn ResolveFs,
    mut resolved: PathBuf,
    path: &Path,
    seen: &mut HashMap<PathBuf, Option<PathBuf>>,
) -> io::Result<PathBuf> {
    for component in path.components() {
        match component {
            Component::Prefix(prefix) => resolved = PathBuf::from(prefix.as_os_str()),
            Component::RootDir => resolved = PathBuf::from(component.as_os_str()),
            Component::CurDir => {}
            Component::ParentDir => {
                resolved.pop();
            }
            Component::Normal(name) => {
                let candidate = resolved.join(name);
                let is_symlink = fs.is_symlink(&candidate).unwrap_or(false);
                if !is_symlink {
                    resolved = candidate;
                    continue;
                }
                if let Some(previous) = seen.get(&candidate) {
                    resolved = previous.clone().unwrap_or(candidate);
                    continue;
                }
                let target = match fs.read_link(&candidate) {
                    Ok(target) => target,
                    Err(_) => {
                        resolved = candidate;
                        continue;
                    }
                };
                seen.insert(candidate.clone(), None);
                let target_base = if target.is_absolute() {
                    PathBuf::new()
                } else {
                    resolved.clone()
                };
                let target = resolve_components(fs, target_base, &target, seen)?;
                seen.insert(candidate, Some(target.clone()));
                resolved = target;
            }
        }
    }
    Ok(resolved)
}

struct FileFollowReader {
    file: fs::File,
    pending: Vec<u8>,
    eof: bool,
    chunk_size: usize,
}

impl FileFollowReader {
    fn open(path: &Path) -> io::Result<Self> {
        Self::open_with_chunk(path, READ_CHUNK_SIZE)
    }

    fn open_with_chunk(path: &Path, chunk_size: usize) -> io::Result<Self> {
        Ok(Self {
            file: fs::File::open(path)?,
            pending: Vec::new(),
            eof: false,
            chunk_size,
        })
    }

    fn fill(&mut self) -> Result<(), FollowReadError> {
        let mut chunk = [0; READ_CHUNK_SIZE];
        match self.file.read(&mut chunk[..self.chunk_size]) {
            Ok(0) => {
                self.eof = true;
                Ok(())
            }
            Ok(read) => {
                self.pending.extend_from_slice(&chunk[..read]);
                Ok(())
            }
            Err(error) => Err(FollowReadError::Io(error)),
        }
    }
}

impl FollowReader for FileFollowReader {
    fn read_line(&mut self) -> Result<Option<String>, FollowReadError> {
        loop {
            let boundary = self
                .pending
                .iter()
                .position(|byte| matches!(byte, b'\n' | b'\r'));
            if let Some(boundary) = boundary {
                if self.pending[boundary] == b'\r'
                    && boundary + 1 == self.pending.len()
                    && !self.eof
                {
                    self.fill()?;
                    continue;
                }
                let line = std::str::from_utf8(&self.pending[..boundary])
                    .map_err(|_| FollowReadError::InvalidUtf8)?
                    .to_owned();
                let consumed = if self.pending[boundary] == b'\r'
                    && self.pending.get(boundary + 1) == Some(&b'\n')
                {
                    boundary + 2
                } else {
                    boundary + 1
                };
                self.pending.drain(..consumed);
                return Ok(Some(line));
            }
            if self.eof {
                if self.pending.is_empty() {
                    self.eof = false;
                    return Ok(None);
                }
                let line = std::str::from_utf8(&self.pending)
                    .map_err(|_| FollowReadError::InvalidUtf8)?
                    .to_owned();
                self.pending.clear();
                self.eof = false;
                return Ok(Some(line));
            }
            if let Err(error) = std::str::from_utf8(&self.pending)
                && error.error_len().is_some()
            {
                return Err(FollowReadError::InvalidUtf8);
            }
            self.fill()?;
        }
    }

    fn seek_to_end(&mut self) -> io::Result<()> {
        self.file.seek(SeekFrom::End(0))?;
        self.pending.clear();
        self.eof = false;
        Ok(())
    }
}

struct TrackedSource {
    source_path: PathBuf,
    resolved: PathBuf,
    reader: Box<dyn FollowReader>,
}

/// Persistent insertion-ordered follower state.
pub struct FollowState {
    tracked: Vec<TrackedSource>,
    last_rotation_check: Duration,
}

impl FollowState {
    fn close_all(&mut self) {
        self.tracked.clear();
    }
}

/// Result of initial discovery over an already-proven health directory.
pub struct InitialDiscovery {
    pub state: FollowState,
    pub has_tracked_sources: bool,
}

/// A typed fatal operation. Source paths are rendered escaped by the driver.
#[derive(Debug)]
pub struct FollowFatalError {
    pub path: PathBuf,
    pub operation: &'static str,
    pub source: Option<io::Error>,
}

/// Result of one follower polling cycle.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TickOutcome {
    Continued,
    Stopped,
}

/// Borrowed dependencies and sinks for one [`tick`] call.
pub struct FollowTickContext<'a> {
    pub fs: &'a dyn FollowFs,
    pub health_dir: &'a Path,
    pub stop: &'a dyn Fn() -> bool,
    pub output: &'a mut dyn Write,
    pub is_tty: bool,
    pub last_service: &'a mut Option<String>,
    pub warn: &'a mut dyn FnMut(String),
}

/// Discover sources before the first poll.
///
/// This receives an already-proven health directory. Journal-root resolution
/// and the absent/non-directory versus metadata-error probe remain AC4 CLI
/// boundary responsibilities, just as for the AC1 reader probe.
pub fn discover_initial(
    fs: &dyn FollowFs,
    health_dir: &Path,
    now: Duration,
    warn: &mut dyn FnMut(String),
) -> Result<InitialDiscovery, FollowFatalError> {
    let names = fs
        .list_top_level_logs(health_dir)
        .map_err(|source| fatal(health_dir, "directory-list", source))?;
    let mut state = FollowState {
        tracked: Vec::new(),
        last_rotation_check: now,
    };
    discover_names(&mut state, fs, health_dir, names, warn);
    Ok(InitialDiscovery {
        has_tracked_sources: !state.tracked.is_empty(),
        state,
    })
}

/// Run the production polling loop. The caller supplies stop observation;
/// AC4 owns the concrete interrupt mechanism and initial directory probe.
pub fn run_follow(
    fs: &dyn FollowFs,
    health_dir: &Path,
    now: &dyn Fn() -> Duration,
    stop: &dyn Fn() -> bool,
    output: &mut dyn Write,
    is_tty: bool,
    warn: &mut dyn FnMut(String),
) -> Result<(), FollowFatalError> {
    let mut initial = discover_initial(fs, health_dir, now(), warn)?;
    if !initial.has_tracked_sources {
        warn("No log files found.".to_owned());
        return Ok(());
    }
    let mut last_service = None;
    loop {
        let mut context = FollowTickContext {
            fs,
            health_dir,
            stop,
            output,
            is_tty,
            last_service: &mut last_service,
            warn,
        };
        match tick(&mut initial.state, now(), &mut context) {
            Ok(TickOutcome::Continued) => std::thread::sleep(Duration::from_millis(200)),
            Ok(TickOutcome::Stopped) => return Ok(()),
            Err(error) => return Err(error),
        }
    }
}

/// Execute one ordered follower cycle: stop, drain, rotate, then discover.
pub fn tick(
    state: &mut FollowState,
    now: Duration,
    context: &mut FollowTickContext<'_>,
) -> Result<TickOutcome, FollowFatalError> {
    if (context.stop)() {
        state.close_all();
        return Ok(TickOutcome::Stopped);
    }
    if let Err(error) = drain(
        state,
        context.stop,
        context.output,
        context.is_tty,
        context.last_service,
    ) {
        state.close_all();
        return Err(error);
    }
    if (context.stop)() {
        state.close_all();
        return Ok(TickOutcome::Stopped);
    }
    if now.saturating_sub(state.last_rotation_check) >= ROTATION_INTERVAL {
        state.last_rotation_check = now;
        if let Err(error) = rotate_and_discover(state, context.fs, context.health_dir, context.warn)
        {
            state.close_all();
            return Err(error);
        }
    }
    if (context.stop)() {
        state.close_all();
        return Ok(TickOutcome::Stopped);
    }
    Ok(TickOutcome::Continued)
}

fn drain(
    state: &mut FollowState,
    stop: &dyn Fn() -> bool,
    output: &mut dyn Write,
    is_tty: bool,
    last_service: &mut Option<String>,
) -> Result<(), FollowFatalError> {
    for source in &mut state.tracked {
        loop {
            if stop() {
                return Ok(());
            }
            let line = match source.reader.read_line() {
                Ok(line) => line,
                Err(FollowReadError::InvalidUtf8) => {
                    return Err(FollowFatalError {
                        path: source.source_path.clone(),
                        operation: "utf8-read",
                        source: None,
                    });
                }
                Err(FollowReadError::Io(source_error)) => {
                    return Err(FollowFatalError {
                        path: source.source_path.clone(),
                        operation: "read",
                        source: Some(source_error),
                    });
                }
            };
            if stop() {
                return Ok(());
            }
            let Some(line) = line else { break };
            if line.is_empty() {
                continue;
            }
            let service = parse_health_log_row(&line).map(|row| row.service);
            render_stream_row(output, &line, service.as_deref(), is_tty, last_service)
                .map_err(output_error)?;
            output.flush().map_err(output_error)?;
        }
    }
    Ok(())
}

fn rotate_and_discover(
    state: &mut FollowState,
    fs: &dyn FollowFs,
    health_dir: &Path,
    warn: &mut dyn FnMut(String),
) -> Result<(), FollowFatalError> {
    let mut index = 0;
    while index < state.tracked.len() {
        let source_path = state.tracked[index].source_path.clone();
        let is_symlink = fs
            .is_symlink(&source_path)
            .map_err(|source| fatal(&source_path, "symlink-metadata", source))?;
        if !is_symlink {
            index += 1;
            continue;
        }
        let resolved = fs
            .resolve(&source_path)
            .map_err(|source| fatal(&source_path, "resolve", source))?;
        if resolved == state.tracked[index].resolved {
            index += 1;
            continue;
        }
        let mut old = state.tracked.remove(index);
        let reader = match fs.open(&resolved) {
            Ok(reader) => reader,
            Err(_) => {
                warn(warning("rotation-open", &source_path));
                drop(old);
                continue;
            }
        };
        old.reader = reader;
        old.resolved = resolved;
        state.tracked.insert(index, old);
        index += 1;
    }

    let names = match fs.list_top_level_logs(health_dir) {
        Ok(names) => names,
        Err(source)
            if matches!(
                source.kind(),
                io::ErrorKind::NotFound | io::ErrorKind::NotADirectory
            ) =>
        {
            return Ok(());
        }
        Err(source) => return Err(fatal(health_dir, "directory-list", source)),
    };
    discover_names(state, fs, health_dir, names, warn);
    Ok(())
}

fn discover_names(
    state: &mut FollowState,
    fs: &dyn FollowFs,
    health_dir: &Path,
    names: Vec<OsString>,
    warn: &mut dyn FnMut(String),
) {
    for name in names {
        let source_path = health_dir.join(name);
        if state
            .tracked
            .iter()
            .any(|tracked| tracked.source_path == source_path)
        {
            continue;
        }
        let resolved = match fs.resolve(&source_path) {
            Ok(resolved) => resolved,
            Err(_) => {
                warn(warning("resolve", &source_path));
                continue;
            }
        };
        let mut reader = match fs.open(&resolved) {
            Ok(reader) => reader,
            Err(_) => {
                warn(warning("initial-open", &source_path));
                continue;
            }
        };
        if reader.seek_to_end().is_err() {
            warn(warning("initial-seek", &source_path));
            continue;
        }
        state.tracked.push(TrackedSource {
            source_path,
            resolved,
            reader,
        });
    }
}

fn fatal(path: &Path, operation: &'static str, source: io::Error) -> FollowFatalError {
    FollowFatalError {
        path: path.to_path_buf(),
        operation,
        source: Some(source),
    }
}

fn warning(operation: &'static str, path: &Path) -> String {
    format!(
        "health logs: {operation} failed for {}",
        sanitize_os_bytes_for_terminal(path.as_os_str().as_encoded_bytes())
    )
}

fn source_path_for_output() -> PathBuf {
    PathBuf::from("<stdout>")
}

fn output_error(source: io::Error) -> FollowFatalError {
    FollowFatalError {
        path: source_path_for_output(),
        operation: "output",
        source: Some(source),
    }
}

#[cfg(all(test, unix))]
mod tests {
    use std::cell::{Cell, RefCell};
    use std::collections::{HashMap, VecDeque};
    #[cfg(target_os = "linux")]
    use std::os::unix::ffi::OsStringExt;
    use std::os::unix::fs::symlink;
    use std::rc::Rc;

    use tempfile::{NamedTempFile, TempDir};

    use super::*;

    #[derive(Clone)]
    enum ReadEvent {
        Line(&'static str),
        End,
        Invalid,
        Io,
    }

    #[derive(Clone)]
    enum OpenEvent {
        Reader {
            events: Vec<ReadEvent>,
            seek_error: bool,
        },
        Error,
    }

    struct FakeReader {
        events: VecDeque<ReadEvent>,
        seek_error: bool,
        closes: Rc<Cell<usize>>,
    }

    #[derive(Default)]
    struct FlushTrackingWriter {
        bytes: Vec<u8>,
        flushes: usize,
    }

    impl Write for FlushTrackingWriter {
        fn write(&mut self, buffer: &[u8]) -> io::Result<usize> {
            self.bytes.extend_from_slice(buffer);
            Ok(buffer.len())
        }

        fn flush(&mut self) -> io::Result<()> {
            self.flushes += 1;
            Ok(())
        }
    }

    impl Drop for FakeReader {
        fn drop(&mut self) {
            self.closes.set(self.closes.get() + 1);
        }
    }

    impl FollowReader for FakeReader {
        fn read_line(&mut self) -> Result<Option<String>, FollowReadError> {
            match self.events.pop_front().unwrap_or(ReadEvent::End) {
                ReadEvent::Line(line) => Ok(Some(line.to_owned())),
                ReadEvent::End => Ok(None),
                ReadEvent::Invalid => Err(FollowReadError::InvalidUtf8),
                ReadEvent::Io => Err(FollowReadError::Io(io::Error::other("read failed"))),
            }
        }

        fn seek_to_end(&mut self) -> io::Result<()> {
            if self.seek_error {
                Err(io::Error::other("seek failed"))
            } else {
                Ok(())
            }
        }
    }

    #[derive(Default)]
    struct FakeFs {
        lists: RefCell<VecDeque<Result<Vec<OsString>, io::ErrorKind>>>,
        links: RefCell<HashMap<PathBuf, VecDeque<Result<bool, io::ErrorKind>>>>,
        resolves: RefCell<HashMap<PathBuf, VecDeque<Result<PathBuf, io::ErrorKind>>>>,
        opens: RefCell<HashMap<PathBuf, VecDeque<OpenEvent>>>,
        closes: Rc<Cell<usize>>,
    }

    impl FakeFs {
        fn list(&self, names: &[&str]) {
            self.lists
                .borrow_mut()
                .push_back(Ok(names.iter().map(|name| OsString::from(*name)).collect()));
        }

        fn list_error(&self) {
            self.lists.borrow_mut().push_back(Err(io::ErrorKind::Other));
        }

        fn list_not_found(&self) {
            self.lists
                .borrow_mut()
                .push_back(Err(io::ErrorKind::NotFound));
        }

        fn resolve(&self, source: &Path, target: &Path) {
            self.resolves
                .borrow_mut()
                .entry(source.to_path_buf())
                .or_default()
                .push_back(Ok(target.to_path_buf()));
        }

        fn resolve_error(&self, source: &Path) {
            self.resolves
                .borrow_mut()
                .entry(source.to_path_buf())
                .or_default()
                .push_back(Err(io::ErrorKind::Other));
        }

        fn link(&self, source: &Path, value: bool) {
            self.links
                .borrow_mut()
                .entry(source.to_path_buf())
                .or_default()
                .push_back(Ok(value));
        }

        fn link_error(&self, source: &Path) {
            self.links
                .borrow_mut()
                .entry(source.to_path_buf())
                .or_default()
                .push_back(Err(io::ErrorKind::PermissionDenied));
        }

        fn end(&self, target: &Path, events: Vec<ReadEvent>) {
            self.opens
                .borrow_mut()
                .entry(target.to_path_buf())
                .or_default()
                .push_back(OpenEvent::Reader {
                    events,
                    seek_error: false,
                });
        }

        fn end_seek_error(&self, target: &Path) {
            self.opens
                .borrow_mut()
                .entry(target.to_path_buf())
                .or_default()
                .push_back(OpenEvent::Reader {
                    events: Vec::new(),
                    seek_error: true,
                });
        }

        fn start(&self, target: &Path, events: Vec<ReadEvent>) {
            self.opens
                .borrow_mut()
                .entry(target.to_path_buf())
                .or_default()
                .push_back(OpenEvent::Reader {
                    events,
                    seek_error: false,
                });
        }

        fn start_error(&self, target: &Path) {
            self.opens
                .borrow_mut()
                .entry(target.to_path_buf())
                .or_default()
                .push_back(OpenEvent::Error);
        }

        fn open(
            queue: &RefCell<HashMap<PathBuf, VecDeque<OpenEvent>>>,
            target: &Path,
            closes: &Rc<Cell<usize>>,
        ) -> io::Result<Box<dyn FollowReader>> {
            match queue
                .borrow_mut()
                .get_mut(target)
                .and_then(VecDeque::pop_front)
                .unwrap_or(OpenEvent::Error)
            {
                OpenEvent::Reader { events, seek_error } => Ok(Box::new(FakeReader {
                    events: events.into(),
                    seek_error,
                    closes: closes.clone(),
                })),
                OpenEvent::Error => Err(io::Error::other("open failed")),
            }
        }
    }

    impl FollowFs for FakeFs {
        fn list_top_level_logs(&self, _health_dir: &Path) -> io::Result<Vec<OsString>> {
            match self
                .lists
                .borrow_mut()
                .pop_front()
                .unwrap_or(Ok(Vec::new()))
            {
                Ok(names) => Ok(names),
                Err(kind) => Err(io::Error::from(kind)),
            }
        }

        fn is_symlink(&self, path: &Path) -> io::Result<bool> {
            match self
                .links
                .borrow_mut()
                .get_mut(path)
                .and_then(VecDeque::pop_front)
                .unwrap_or(Ok(false))
            {
                Ok(value) => Ok(value),
                Err(kind) => Err(io::Error::from(kind)),
            }
        }

        fn resolve(&self, path: &Path) -> io::Result<PathBuf> {
            match self
                .resolves
                .borrow_mut()
                .get_mut(path)
                .and_then(VecDeque::pop_front)
                .unwrap_or_else(|| Ok(path.to_path_buf()))
            {
                Ok(path) => Ok(path),
                Err(kind) => Err(io::Error::from(kind)),
            }
        }

        fn open(&self, resolved: &Path) -> io::Result<Box<dyn FollowReader>> {
            Self::open(&self.opens, resolved, &self.closes)
        }
    }

    fn health() -> PathBuf {
        PathBuf::from("/journal/health")
    }

    fn source(name: &str) -> PathBuf {
        health().join(name)
    }

    fn target(name: &str) -> PathBuf {
        PathBuf::from("/targets").join(name)
    }

    fn initial(fs: &FakeFs, names: &[&str]) -> FollowState {
        fs.list(names);
        for name in names {
            let source = source(name);
            let target = target(name);
            fs.resolve(&source, &target);
            fs.end(&target, vec![ReadEvent::End]);
        }
        discover_initial(fs, &health(), Duration::ZERO, &mut |_| {})
            .unwrap()
            .state
    }

    fn tick_at(
        state: &mut FollowState,
        fs: &FakeFs,
        at: u64,
        output: &mut Vec<u8>,
    ) -> Result<TickOutcome, FollowFatalError> {
        let health = health();
        let mut last_service = None;
        let mut warn = |_| {};
        let mut context = FollowTickContext {
            fs,
            health_dir: &health,
            stop: &|| false,
            output,
            is_tty: false,
            last_service: &mut last_service,
            warn: &mut warn,
        };
        tick(state, Duration::from_secs(at), &mut context)
    }

    #[test]
    fn initial_discovery_orders_and_warns_per_source_without_symlink_filtering() {
        let fs = FakeFs::default();
        fs.list(&["a-.log", "a-\u{80}.log", "a-\u{80}-raw.log", "seek.log"]);
        for name in ["a-.log", "a-\u{80}.log"] {
            fs.resolve(&source(name), &target(name));
            fs.end(&target(name), vec![ReadEvent::End]);
        }
        fs.resolve_error(&source("a-\u{80}-raw.log"));
        fs.resolve(&source("seek.log"), &target("seek.log"));
        fs.end_seek_error(&target("seek.log"));
        let mut warnings = Vec::new();
        let initial = discover_initial(&fs, &health(), Duration::ZERO, &mut |warning| {
            warnings.push(warning)
        })
        .unwrap();
        assert!(initial.has_tracked_sources);
        assert_eq!(initial.state.tracked.len(), 2);
        assert_eq!(warnings.len(), 2);
        assert!(warnings.iter().any(|warning| warning.contains("resolve")));
        assert!(
            warnings
                .iter()
                .any(|warning| warning.contains("initial-seek"))
        );
    }

    #[test]
    fn initial_directory_failure_is_fatal_and_zero_sources_is_distinct_success() {
        let failing = FakeFs::default();
        failing.list_error();
        assert!(matches!(
            discover_initial(&failing, &health(), Duration::ZERO, &mut |_| {}),
            Err(FollowFatalError {
                operation: "directory-list",
                ..
            })
        ));
        let empty = FakeFs::default();
        empty.list(&[]);
        assert!(
            !discover_initial(&empty, &health(), Duration::ZERO, &mut |_| {})
                .unwrap()
                .has_tracked_sources
        );
    }

    #[test]
    fn run_follow_reports_no_files_after_empty_or_failed_initial_discovery() {
        let empty = FakeFs::default();
        empty.list(&[]);
        let mut warnings = Vec::new();
        run_follow(
            &empty,
            &health(),
            &|| Duration::ZERO,
            &|| false,
            &mut Vec::new(),
            false,
            &mut |warning| warnings.push(warning),
        )
        .unwrap();
        assert_eq!(warnings, ["No log files found."]);

        let failed = FakeFs::default();
        failed.list(&["resolve.log", "open.log", "seek.log"]);
        failed.resolve_error(&source("resolve.log"));
        failed.resolve(&source("open.log"), &target("open.log"));
        failed.start_error(&target("open.log"));
        failed.resolve(&source("seek.log"), &target("seek.log"));
        failed.end_seek_error(&target("seek.log"));
        let mut warnings = Vec::new();
        run_follow(
            &failed,
            &health(),
            &|| Duration::ZERO,
            &|| false,
            &mut Vec::new(),
            false,
            &mut |warning| warnings.push(warning),
        )
        .unwrap();
        assert_eq!(warnings.len(), 4);
        assert!(
            warnings[..3]
                .iter()
                .all(|warning| warning.starts_with("health logs:"))
        );
        assert_eq!(warnings[3], "No log files found.");
    }

    #[test]
    fn std_follow_listing_includes_regular_and_symlink_logs_in_python_order() {
        let directory = TempDir::new().unwrap();
        std::fs::write(directory.path().join("regular.log"), b"").unwrap();
        std::fs::write(directory.path().join("target"), b"").unwrap();
        symlink(
            directory.path().join("target"),
            directory.path().join("linked.log"),
        )
        .unwrap();
        std::fs::write(directory.path().join("ignored.txt"), b"").unwrap();
        let valid = OsString::from("a-\u{80}.log");
        #[cfg(target_os = "linux")]
        let invalid = OsString::from_vec(b"a-\x80.log".to_vec());
        std::fs::write(directory.path().join(&valid), b"").unwrap();
        #[cfg(target_os = "linux")]
        std::fs::write(directory.path().join(&invalid), b"").unwrap();

        let names = StdFollowFs.list_top_level_logs(directory.path()).unwrap();
        assert!(names.contains(&OsString::from("regular.log")));
        assert!(names.contains(&OsString::from("linked.log")));
        assert!(!names.contains(&OsString::from("ignored.txt")));
        #[cfg(target_os = "linux")]
        assert!(
            names.iter().position(|name| name == &valid).unwrap()
                < names.iter().position(|name| name == &invalid).unwrap()
        );
    }

    #[derive(Default)]
    struct RecordingResolveFs {
        cwd: PathBuf,
        cwd_error: bool,
        links: HashMap<PathBuf, Result<PathBuf, io::ErrorKind>>,
        metadata_errors: Vec<PathBuf>,
        events: RefCell<Vec<String>>,
    }

    impl ResolveFs for RecordingResolveFs {
        fn current_dir(&self) -> io::Result<PathBuf> {
            self.events.borrow_mut().push("cwd".into());
            if self.cwd_error {
                Err(io::Error::from(io::ErrorKind::PermissionDenied))
            } else {
                Ok(self.cwd.clone())
            }
        }

        fn is_symlink(&self, path: &Path) -> io::Result<bool> {
            self.events
                .borrow_mut()
                .push(format!("lstat:{}", path.display()));
            if self.metadata_errors.iter().any(|error| error == path) {
                return Err(io::Error::from(io::ErrorKind::PermissionDenied));
            }
            Ok(self.links.contains_key(path))
        }

        fn read_link(&self, path: &Path) -> io::Result<PathBuf> {
            self.events
                .borrow_mut()
                .push(format!("readlink:{}", path.display()));
            match self.links.get(path).expect("recorded symlink") {
                Ok(target) => Ok(target.clone()),
                Err(kind) => Err(io::Error::from(*kind)),
            }
        }
    }

    #[test]
    fn non_strict_resolver_suppresses_lstat_and_readlink_errors() {
        let mut fs = RecordingResolveFs {
            cwd: PathBuf::from("/work"),
            ..RecordingResolveFs::default()
        };
        fs.metadata_errors.push(PathBuf::from("/work/missing"));
        fs.links.insert(
            PathBuf::from("/work/link"),
            Ok(PathBuf::from("missing/../leaf")),
        );
        assert_eq!(
            resolve_non_strict(&fs, Path::new("link/tail")).unwrap(),
            PathBuf::from("/work/leaf/tail")
        );
        assert!(
            fs.events
                .borrow()
                .iter()
                .any(|event| event == "lstat:/work/missing")
        );

        let mut failing = RecordingResolveFs {
            cwd: PathBuf::from("/work"),
            ..RecordingResolveFs::default()
        };
        failing.links.insert(
            PathBuf::from("/work/link"),
            Err(io::ErrorKind::PermissionDenied),
        );
        assert_eq!(
            resolve_non_strict(&failing, Path::new("link")).unwrap(),
            PathBuf::from("/work/link")
        );
        assert_eq!(
            failing.events.borrow().as_slice(),
            ["cwd", "lstat:/work/link", "readlink:/work/link"]
        );

        let cwd_failure = RecordingResolveFs {
            cwd_error: true,
            ..RecordingResolveFs::default()
        };
        assert_eq!(
            resolve_non_strict(&cwd_failure, Path::new("relative"))
                .unwrap_err()
                .kind(),
            io::ErrorKind::PermissionDenied
        );
    }

    struct TransientReadlinkFs {
        calls: Cell<usize>,
    }

    impl ResolveFs for TransientReadlinkFs {
        fn current_dir(&self) -> io::Result<PathBuf> {
            Ok(PathBuf::from("/work"))
        }

        fn is_symlink(&self, path: &Path) -> io::Result<bool> {
            Ok(path == Path::new("/work/link"))
        }

        fn read_link(&self, path: &Path) -> io::Result<PathBuf> {
            assert_eq!(path, Path::new("/work/link"));
            let call = self.calls.get();
            self.calls.set(call + 1);
            if call == 0 {
                Err(io::Error::from(io::ErrorKind::PermissionDenied))
            } else {
                Ok(PathBuf::from("target"))
            }
        }
    }

    #[test]
    fn failed_readlink_is_not_cached_and_retries_same_candidate() {
        let fs = TransientReadlinkFs {
            calls: Cell::new(0),
        };
        assert_eq!(
            resolve_non_strict(&fs, Path::new("link/../link")).unwrap(),
            PathBuf::from("/work/target")
        );
        assert_eq!(fs.calls.get(), 2);
    }

    #[test]
    fn non_strict_resolver_preserves_missing_and_loop_paths_for_open() {
        let directory = TempDir::new().unwrap();
        std::fs::create_dir(directory.path().join("target-dir")).unwrap();
        std::fs::write(directory.path().join("target-dir/file"), b"").unwrap();
        symlink(
            directory.path().join("target-dir"),
            directory.path().join("absolute"),
        )
        .unwrap();
        assert_eq!(
            StdFollowFs
                .resolve(&directory.path().join("absolute/./file"))
                .unwrap(),
            directory.path().join("target-dir/file")
        );

        symlink("missing/leaf", directory.path().join("broken")).unwrap();
        let broken = StdFollowFs
            .resolve(&directory.path().join("broken"))
            .unwrap();
        assert_eq!(broken, directory.path().join("missing/leaf"));

        symlink("b", directory.path().join("a")).unwrap();
        symlink("a", directory.path().join("b")).unwrap();
        let loop_path = StdFollowFs.resolve(&directory.path().join("a")).unwrap();
        assert_eq!(loop_path, directory.path().join("a"));
        assert!(
            std::fs::File::open(loop_path)
                .unwrap_err()
                .raw_os_error()
                .is_some()
        );
    }

    #[test]
    fn loop_open_failures_warn_without_stopping_other_sources() {
        let directory = TempDir::new().unwrap();
        std::fs::write(directory.path().join("target"), b"").unwrap();
        std::fs::write(directory.path().join("survivor.log"), b"").unwrap();
        symlink("target", directory.path().join("rot.log")).unwrap();
        symlink("loop-b", directory.path().join("loop-a")).unwrap();
        symlink("loop-a", directory.path().join("loop-b")).unwrap();
        symlink("loop-a", directory.path().join("initial-loop.log")).unwrap();

        let mut warnings = Vec::new();
        let mut initial = discover_initial(
            &StdFollowFs,
            directory.path(),
            Duration::ZERO,
            &mut |warning| warnings.push(warning),
        )
        .unwrap();
        assert!(initial.has_tracked_sources);
        assert!(
            warnings
                .iter()
                .any(|warning| warning.contains("initial-open failed"))
        );

        std::fs::remove_file(directory.path().join("rot.log")).unwrap();
        symlink("loop-a", directory.path().join("rot.log")).unwrap();
        std::fs::write(
            directory.path().join("survivor.log"),
            b"2026-01-01 12:00:00 [survivor:out] alive\n",
        )
        .unwrap();
        let mut output = Vec::new();
        let mut last_service = None;
        let mut context = FollowTickContext {
            fs: &StdFollowFs,
            health_dir: directory.path(),
            stop: &|| false,
            output: &mut output,
            is_tty: false,
            last_service: &mut last_service,
            warn: &mut |warning| warnings.push(warning),
        };
        tick(&mut initial.state, Duration::from_secs(2), &mut context).unwrap();
        assert_eq!(output, b"2026-01-01 12:00:00 [survivor:out] alive\n");
        assert!(
            warnings
                .iter()
                .any(|warning| warning.contains("rotation-open failed"))
        );
        assert!(
            initial
                .state
                .tracked
                .iter()
                .all(|source| source.source_path != directory.path().join("rot.log"))
        );
    }

    #[test]
    fn drains_universal_results_in_insertion_order_and_keeps_malformed_rows() {
        let fs = FakeFs::default();
        let mut state = initial(&fs, &["b.log", "a.log"]);
        state.tracked[0].reader = Box::new(FakeReader {
            events: vec![
                ReadEvent::Line("malformed"),
                ReadEvent::Line(""),
                ReadEvent::End,
            ]
            .into(),
            seek_error: false,
            closes: fs.closes.clone(),
        });
        state.tracked[1].reader = Box::new(FakeReader {
            events: vec![
                ReadEvent::Line("2026-01-01 12:00:00 [svc:out] cr-normalized"),
                ReadEvent::Line("unterminated"),
                ReadEvent::End,
            ]
            .into(),
            seek_error: false,
            closes: fs.closes.clone(),
        });
        let mut output = Vec::new();
        tick_at(&mut state, &fs, 1, &mut output).unwrap();
        assert_eq!(
            output,
            b"malformed\n2026-01-01 12:00:00 [svc:out] cr-normalized\nunterminated\n"
        );
    }

    #[test]
    fn drains_flush_each_row_in_both_output_modes() {
        for is_tty in [false, true] {
            let fs = FakeFs::default();
            let mut state = initial(&fs, &["a.log"]);
            state.tracked[0].reader = Box::new(FakeReader {
                events: vec![
                    ReadEvent::Line("2026-01-01 12:00:00 [svc:out] first"),
                    ReadEvent::Line("2026-01-01 12:00:01 [svc:out] second"),
                    ReadEvent::End,
                ]
                .into(),
                seek_error: false,
                closes: fs.closes.clone(),
            });
            let health = health();
            let mut output = FlushTrackingWriter::default();
            let mut last_service = None;
            let mut warn = |_| {};
            let mut context = FollowTickContext {
                fs: &fs,
                health_dir: &health,
                stop: &|| false,
                output: &mut output,
                is_tty,
                last_service: &mut last_service,
                warn: &mut warn,
            };

            tick(&mut state, Duration::from_secs(1), &mut context).unwrap();
            assert_eq!(output.flushes, 2, "is_tty={is_tty}");
        }
    }

    #[test]
    fn tty_follow_headers_use_only_parsed_services_and_escape_rows() {
        let fs = FakeFs::default();
        let mut state = initial(&fs, &["a.log"]);
        state.tracked[0].reader = Box::new(FakeReader {
            events: vec![
                ReadEvent::Line("malformed\x1b"),
                ReadEvent::Line("2026-01-01 12:00:00 [svc\x1b:out] message\x1b"),
                ReadEvent::End,
            ]
            .into(),
            seek_error: false,
            closes: fs.closes.clone(),
        });
        let health = health();
        let mut output = Vec::new();
        let mut last_service = None;
        let mut warn = |_| {};
        let mut context = FollowTickContext {
            fs: &fs,
            health_dir: &health,
            stop: &|| false,
            output: &mut output,
            is_tty: true,
            last_service: &mut last_service,
            warn: &mut warn,
        };
        tick(&mut state, Duration::from_secs(1), &mut context).unwrap();
        assert_eq!(
            String::from_utf8(output).unwrap(),
            "malformed\\x1b\n\x1b[2m── svc\\x1b ──\x1b[0m\n2026-01-01 12:00:00 [svc\\x1b:out] message\\x1b\n"
        );
    }

    #[test]
    fn invalid_utf8_and_io_keep_prior_rows_then_close_every_handle() {
        let fs = FakeFs::default();
        let mut state = initial(&fs, &["a.log", "b.log"]);
        state.tracked[0].reader = Box::new(FakeReader {
            events: vec![ReadEvent::Line("good"), ReadEvent::Invalid].into(),
            seek_error: false,
            closes: fs.closes.clone(),
        });
        let mut output = Vec::new();
        assert!(matches!(
            tick_at(&mut state, &fs, 1, &mut output),
            Err(FollowFatalError {
                operation: "utf8-read",
                ..
            })
        ));
        assert_eq!(output, b"good\n");
        assert!(state.tracked.is_empty());
        assert_eq!(fs.closes.get(), 3);

        let fs = FakeFs::default();
        let mut state = initial(&fs, &["a.log"]);
        state.tracked[0].reader = Box::new(FakeReader {
            events: vec![ReadEvent::Line("good"), ReadEvent::Io].into(),
            seek_error: false,
            closes: fs.closes.clone(),
        });
        let mut output = Vec::new();
        assert!(matches!(
            tick_at(&mut state, &fs, 1, &mut output),
            Err(FollowFatalError {
                operation: "read",
                ..
            })
        ));
        assert_eq!(output, b"good\n");
    }

    #[test]
    fn production_reader_normalizes_newlines_and_invalid_utf8_independent_of_row_size() {
        let file = NamedTempFile::new().unwrap();
        std::fs::write(file.path(), b"a\rb\r\nc\nunterminated").unwrap();
        let mut reader = FileFollowReader::open(file.path()).unwrap();
        assert_eq!(reader.read_line().unwrap(), Some("a".to_owned()));
        assert_eq!(reader.read_line().unwrap(), Some("b".to_owned()));
        assert_eq!(reader.read_line().unwrap(), Some("c".to_owned()));
        assert_eq!(reader.read_line().unwrap(), Some("unterminated".to_owned()));

        let file = NamedTempFile::new().unwrap();
        let mut bytes = b"good\nbad".to_vec();
        bytes.extend(std::iter::repeat_n(b'x', READ_CHUNK_SIZE * 3));
        bytes.extend_from_slice(b"\xffmore\n");
        std::fs::write(file.path(), bytes).unwrap();
        for chunk_size in [1, READ_CHUNK_SIZE] {
            let mut reader = FileFollowReader::open_with_chunk(file.path(), chunk_size).unwrap();
            assert_eq!(reader.read_line().unwrap(), Some("good".to_owned()));
            assert!(matches!(
                reader.read_line(),
                Err(FollowReadError::InvalidUtf8)
            ));
        }

        for bytes in [b"\xffbad\n".as_slice(), b"unterminated\xff".as_slice()] {
            let file = NamedTempFile::new().unwrap();
            std::fs::write(file.path(), bytes).unwrap();
            assert!(matches!(
                FileFollowReader::open_with_chunk(file.path(), 1)
                    .unwrap()
                    .read_line(),
                Err(FollowReadError::InvalidUtf8)
            ));
        }
    }

    #[test]
    fn rotation_observes_live_types_and_opened_handles_wait_until_next_tick() {
        let fs = FakeFs::default();
        let mut state = initial(&fs, &["a.log"]);
        let source = source("a.log");
        let old = target("a.log");
        let new = target("new.log");
        fs.link(&source, true);
        fs.resolve(&source, &new);
        fs.start(&new, vec![ReadEvent::Line("rotated"), ReadEvent::End]);
        fs.list(&["a.log"]);
        let mut output = Vec::new();
        tick_at(&mut state, &fs, 2, &mut output).unwrap();
        assert!(output.is_empty());
        assert_eq!(state.tracked[0].resolved, new);
        tick_at(&mut state, &fs, 3, &mut output).unwrap();
        assert_eq!(output, b"rotated\n");
        assert_ne!(state.tracked[0].resolved, old);
    }

    #[test]
    fn non_symlink_missing_and_same_target_replacements_keep_old_descriptor() {
        let fs = FakeFs::default();
        let mut state = initial(&fs, &["a.log"]);
        let source = source("a.log");
        let old_reader = std::mem::replace(
            &mut state.tracked[0].reader,
            Box::new(FakeReader {
                events: vec![ReadEvent::Line("old"), ReadEvent::End].into(),
                seek_error: false,
                closes: fs.closes.clone(),
            }),
        );
        drop(old_reader);
        fs.link(&source, false);
        fs.list(&[]);
        let mut output = Vec::new();
        tick_at(&mut state, &fs, 2, &mut output).unwrap();
        assert_eq!(output, b"old\n");
        tick_at(&mut state, &fs, 3, &mut output).unwrap();
        assert_eq!(output, b"old\n");

        fs.link(&source, true);
        fs.resolve(&source, &target("a.log"));
        fs.list(&["a.log"]);
        tick_at(&mut state, &fs, 4, &mut output).unwrap();
        assert_eq!(state.tracked.len(), 1);
    }

    #[test]
    fn rotation_metadata_and_reresolve_errors_are_fatal_but_rotation_open_warns_and_retries() {
        let fs = FakeFs::default();
        let mut state = initial(&fs, &["a.log"]);
        fs.link_error(&source("a.log"));
        let mut output = Vec::new();
        assert!(matches!(
            tick_at(&mut state, &fs, 2, &mut output),
            Err(FollowFatalError {
                operation: "symlink-metadata",
                ..
            })
        ));

        let fs = FakeFs::default();
        let mut state = initial(&fs, &["a.log"]);
        fs.link(&source("a.log"), true);
        fs.resolve_error(&source("a.log"));
        assert!(matches!(
            tick_at(&mut state, &fs, 2, &mut Vec::new()),
            Err(FollowFatalError {
                operation: "resolve",
                ..
            })
        ));

        let fs = FakeFs::default();
        let mut state = initial(&fs, &["a.log"]);
        fs.link(&source("a.log"), true);
        fs.resolve(&source("a.log"), &target("new.log"));
        fs.start_error(&target("new.log"));
        fs.list(&["a.log"]);
        fs.resolve(&source("a.log"), &target("a.log"));
        fs.end(
            &target("a.log"),
            vec![ReadEvent::Line("retry"), ReadEvent::End],
        );
        let mut warnings = Vec::new();
        let health = health();
        let mut output = Vec::new();
        let mut last_service = None;
        let mut warn = |warning| warnings.push(warning);
        let mut context = FollowTickContext {
            fs: &fs,
            health_dir: &health,
            stop: &|| false,
            output: &mut output,
            is_tty: false,
            last_service: &mut last_service,
            warn: &mut warn,
        };
        tick(&mut state, Duration::from_secs(2), &mut context).unwrap();
        assert_eq!(warnings.len(), 1);
        assert_eq!(state.tracked.len(), 1);
    }

    #[test]
    fn discovery_appends_after_survivors_and_clock_jump_scans_once() {
        let fs = FakeFs::default();
        let mut state = initial(&fs, &["b.log"]);
        fs.link(&source("b.log"), false);
        fs.list(&["a.log", "b.log"]);
        fs.resolve(&source("a.log"), &target("a.log"));
        fs.end(
            &target("a.log"),
            vec![ReadEvent::Line("new"), ReadEvent::End],
        );
        let mut output = Vec::new();
        tick_at(&mut state, &fs, 10, &mut output).unwrap();
        assert_eq!(
            state
                .tracked
                .iter()
                .map(|tracked| tracked.source_path.file_name().unwrap().to_str().unwrap())
                .collect::<Vec<_>>(),
            ["b.log", "a.log"]
        );
        tick_at(&mut state, &fs, 11, &mut output).unwrap();
        assert_eq!(output, b"new\n");
    }

    #[test]
    fn periodic_directory_failure_closes_survivors_and_stop_wins_before_events() {
        let fs = FakeFs::default();
        let mut state = initial(&fs, &["a.log"]);
        fs.link(&source("a.log"), false);
        fs.list_error();
        let mut output = Vec::new();
        assert!(matches!(
            tick_at(&mut state, &fs, 2, &mut output),
            Err(FollowFatalError {
                operation: "directory-list",
                ..
            })
        ));
        assert!(state.tracked.is_empty());

        let fs = FakeFs::default();
        let mut state = initial(&fs, &["a.log"]);
        state.tracked[0].reader = Box::new(FakeReader {
            events: vec![ReadEvent::Line("queued")].into(),
            seek_error: false,
            closes: fs.closes.clone(),
        });
        let mut output = Vec::new();
        let health = health();
        let mut last_service = None;
        let mut warn = |_| {};
        let mut context = FollowTickContext {
            fs: &fs,
            health_dir: &health,
            stop: &|| true,
            output: &mut output,
            is_tty: false,
            last_service: &mut last_service,
            warn: &mut warn,
        };
        assert_eq!(
            tick(&mut state, Duration::from_secs(1), &mut context).unwrap(),
            TickOutcome::Stopped
        );
        assert!(output.is_empty());
        assert!(state.tracked.is_empty());
    }

    #[test]
    fn periodic_directory_disappearance_keeps_sources_and_later_discovers_recreation() {
        let fs = FakeFs::default();
        let mut state = initial(&fs, &["survivor.log"]);
        fs.link(&source("survivor.log"), false);
        fs.list_not_found();
        let mut output = Vec::new();
        assert_eq!(
            tick_at(&mut state, &fs, 2, &mut output).unwrap(),
            TickOutcome::Continued
        );
        assert_eq!(state.tracked.len(), 1);

        fs.link(&source("survivor.log"), false);
        fs.list(&["survivor.log", "recreated.log"]);
        fs.resolve(&source("recreated.log"), &target("recreated.log"));
        fs.end(&target("recreated.log"), vec![ReadEvent::End]);
        assert_eq!(
            tick_at(&mut state, &fs, 4, &mut output).unwrap(),
            TickOutcome::Continued
        );
        assert_eq!(
            state
                .tracked
                .iter()
                .map(|tracked| tracked.source_path.file_name().unwrap().to_str().unwrap())
                .collect::<Vec<_>>(),
            ["survivor.log", "recreated.log"]
        );
    }

    #[test]
    fn follow_api_has_no_query_inputs_so_filters_and_count_cannot_apply() {
        let fs = FakeFs::default();
        let mut state = initial(&fs, &["a.log"]);
        state.tracked[0].reader = Box::new(FakeReader {
            events: vec![
                ReadEvent::Line("2026-01-01 00:00:00 [other:out] unmatched"),
                ReadEvent::End,
            ]
            .into(),
            seek_error: false,
            closes: fs.closes.clone(),
        });
        let mut output = Vec::new();
        tick_at(&mut state, &fs, 1, &mut output).unwrap();
        assert!(String::from_utf8(output).unwrap().contains("unmatched"));
    }
}
