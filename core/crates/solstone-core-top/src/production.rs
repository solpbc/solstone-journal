// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Concrete adapters for the interactive command. The reducer and loop remain
//! fully injected; this is the only module that touches clocks, terminal I/O,
//! the filesystem, or a live Callosum socket.

use std::io::{IsTerminal, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use chrono::Utc;
use nix::sys::termios::{self, SetArg, Termios};
use serde_json::{Map, Value, json};
use solstone_core_brain::{inspect_brain_state, present_brain_inspection, read_journal_config};
use solstone_core_callosum::{CallosumReceiveEvent, CallosumSocketConnection};
use solstone_core_journal::{discover_home, read_config_journal, resolve_journal_path};
use solstone_core_system_health::sanitize_os_bytes_for_terminal;

use crate::{
    RestartEnqueueResult, RestartIdSource, SessionRestartIds, TopBrainSource, TopClock, TopInput,
    TopReceiveTransport, TopRestartTransport, TopState, TopTerminal, platform_observer,
    run_top_with,
};

pub(super) fn run(verbose: bool, debug: bool) -> Result<(), String> {
    let journal = resolve_process_journal_path()?;
    if verbose || debug {
        eprintln!(
            "solstone-core top: journal={}",
            sanitize_os_bytes_for_terminal(journal.as_os_str().as_encoded_bytes())
        );
    }
    let shared = ProductionCallosum::new(journal.join("health/callosum.sock"))?;
    let mut receive = ProductionReceive::new(Arc::clone(&shared));
    let mut restart = ProductionRestart::new(shared);
    let mut terminal = ProductionTerminal::new();
    let mut clock = ProductionClock::new();
    let mut observer = platform_observer();
    let mut brain = ProductionBrain::new(journal);
    run_top_with_outer_panic_cleanup(
        &mut TopState::default(),
        &mut clock,
        &mut terminal,
        &mut receive,
        &mut observer,
        &mut restart,
        &mut brain,
    )
    .map_err(|error| error.to_string())
}

/// The concrete composition owns the panic boundary outside the pure runner.
/// On unwinding after acquisition it restores the terminal and stops receive
/// transport exactly once, then resumes the original panic unchanged.
pub fn run_top_with_outer_panic_cleanup(
    state: &mut TopState,
    clock: &mut dyn TopClock,
    terminal: &mut dyn TopTerminal,
    receive: &mut dyn TopReceiveTransport,
    observer: &mut dyn crate::ProcessObserver,
    restart: &mut dyn TopRestartTransport,
    brain: &mut dyn TopBrainSource,
) -> Result<(), crate::TopLoopError> {
    match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        run_top_with(state, clock, terminal, receive, observer, restart, brain)
    })) {
        Ok(result) => result,
        Err(payload) => {
            let restore = terminal.restore();
            let stop = receive.stop();
            if let Err(error) = restore {
                eprintln!("top panic cleanup terminal: {error}");
            }
            if let Err(error) = stop {
                eprintln!("top panic cleanup transport: {error}");
            }
            std::panic::resume_unwind(payload);
        }
    }
}

fn resolve_process_journal_path() -> Result<PathBuf, String> {
    let env_journal = std::env::var_os("SOLSTONE_JOURNAL");
    let home_env = std::env::var_os("HOME");
    let fallback = std::env::home_dir();
    let home = discover_home(home_env.as_deref(), fallback.as_deref())
        .map_err(|error| format!("{error:?}"))?;
    let config =
        read_config_journal(&home).map_err(|error| format!("journal config: {error:?}"))?;
    Ok(resolve_journal_path(env_journal.as_deref(), config.as_deref(), None, &home).path)
}

pub(crate) struct ProductionClock {
    started: Instant,
}
impl ProductionClock {
    pub(crate) fn new() -> Self {
        Self {
            started: Instant::now(),
        }
    }
}
impl TopClock for ProductionClock {
    fn wall_seconds(&self) -> f64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0.0, |duration| duration.as_secs_f64())
    }
    fn monotonic_seconds(&self) -> f64 {
        self.started.elapsed().as_secs_f64()
    }
    fn datetime(&self) -> Value {
        json!({"datetime": Utc::now().naive_utc().to_string()})
    }
    fn sleep(&mut self, seconds: f64) -> Result<(), String> {
        std::thread::sleep(Duration::from_secs_f64(seconds.max(0.0)));
        Ok(())
    }
}

pub(crate) struct ProductionBrain {
    journal: PathBuf,
}
impl ProductionBrain {
    pub(crate) fn new(journal: PathBuf) -> Self {
        Self { journal }
    }
}
impl TopBrainSource for ProductionBrain {
    fn inspect(&mut self) -> Result<Value, String> {
        let config = read_journal_config(&self.journal)
            .map_err(|error| error.to_string())?
            .config
            .unwrap_or_default();
        let inspection = inspect_brain_state(&self.journal, &config, Utc::now());
        let presentation = present_brain_inspection(&inspection, Utc::now());
        let mut lines = vec![presentation.headline];
        if !presentation.reason_text.is_empty() {
            lines.push(format!("  {}", presentation.reason_text));
        }
        Ok(json!({"lines": lines}))
    }
}

struct ProductionCallosumInner {
    runtime: tokio::runtime::Runtime,
    connection: CallosumSocketConnection,
    generation: u64,
    epoch: u64,
}
pub(crate) struct ProductionCallosum {
    inner: Mutex<ProductionCallosumInner>,
}
impl ProductionCallosum {
    pub(crate) fn new(path: impl AsRef<Path>) -> Result<Arc<Self>, String> {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(1)
            .enable_all()
            .build()
            .map_err(|error| error.to_string())?;
        Ok(Arc::new(Self {
            inner: Mutex::new(ProductionCallosumInner {
                runtime,
                connection: CallosumSocketConnection::new(path, Map::new()),
                generation: 0,
                epoch: 0,
            }),
        }))
    }
    fn start(&self) -> Result<(), String> {
        let mut inner = self
            .inner
            .lock()
            .map_err(|_| "Callosum lock poisoned".to_owned())?;
        let ProductionCallosumInner {
            runtime,
            connection,
            ..
        } = &mut *inner;
        let guard = runtime.enter();
        connection.start();
        drop(guard);
        Ok(())
    }
    fn next(&self) -> Result<Option<CallosumReceiveEvent>, String> {
        let mut inner = self
            .inner
            .lock()
            .map_err(|_| "Callosum lock poisoned".to_owned())?;
        let event = inner.connection.try_next_event();
        if let Some(
            CallosumReceiveEvent::Envelope {
                generation, epoch, ..
            }
            | CallosumReceiveEvent::Continuity {
                generation, epoch, ..
            },
        ) = &event
        {
            inner.generation = *generation;
            inner.epoch = *epoch;
        }
        Ok(event)
    }
    fn stop(&self) -> Result<(), String> {
        let mut inner = self
            .inner
            .lock()
            .map_err(|_| "Callosum lock poisoned".to_owned())?;
        let ProductionCallosumInner {
            runtime,
            connection,
            ..
        } = &mut *inner;
        runtime.block_on(connection.stop());
        Ok(())
    }
    fn emit_restart(&self, service: &str, restart_id: &str) -> RestartEnqueueResult {
        let Ok(inner) = self.inner.lock() else {
            return RestartEnqueueResult::TransportError;
        };
        let mut values = Map::new();
        values.insert("service".to_owned(), Value::String(service.to_owned()));
        values.insert(
            "restart_id".to_owned(),
            Value::String(restart_id.to_owned()),
        );
        if inner.connection.emit("supervisor", "restart", values) {
            RestartEnqueueResult::Enqueued
        } else {
            RestartEnqueueResult::Closed
        }
    }
    fn generation(&self) -> u64 {
        self.inner.lock().map_or(0, |inner| inner.generation)
    }
    fn epoch(&self) -> u64 {
        self.inner.lock().map_or(0, |inner| inner.epoch)
    }
}
pub(crate) struct ProductionReceive {
    shared: Arc<ProductionCallosum>,
}
impl ProductionReceive {
    pub(crate) fn new(shared: Arc<ProductionCallosum>) -> Self {
        Self { shared }
    }
}
impl TopReceiveTransport for ProductionReceive {
    fn start(&mut self) -> Result<(), String> {
        self.shared.start()
    }
    fn next(&mut self) -> Result<Option<CallosumReceiveEvent>, String> {
        self.shared.next()
    }
    fn stop(&mut self) -> Result<(), String> {
        self.shared.stop()
    }
}
pub(crate) struct ProductionRestart {
    shared: Arc<ProductionCallosum>,
    ids: SessionRestartIds,
}
impl ProductionRestart {
    pub(crate) fn new(shared: Arc<ProductionCallosum>) -> Self {
        Self {
            shared,
            ids: SessionRestartIds::new(),
        }
    }
}
impl TopRestartTransport for ProductionRestart {
    fn emit_restart(&mut self, service: &str, restart_id: &str) -> RestartEnqueueResult {
        self.shared.emit_restart(service, restart_id)
    }
    fn current_generation(&self) -> u64 {
        self.shared.generation()
    }
    fn current_epoch(&self) -> u64 {
        self.shared.epoch()
    }
    fn restart_ids(&mut self) -> &mut dyn RestartIdSource {
        &mut self.ids
    }
}

pub trait TerminalSyscalls {
    type Saved: Clone;

    fn stdin_is_tty(&mut self) -> bool;
    fn stdout_is_tty(&mut self) -> bool;
    fn capture_stdin(&mut self) -> Result<Self::Saved, String>;
    fn raw_mode(&mut self, saved: &Self::Saved) -> Self::Saved;
    fn apply_stdin(&mut self, settings: &Self::Saved) -> Result<(), String>;
    fn stdout_width(&mut self) -> Result<usize, String>;
    fn write_stdout(&mut self, bytes: &str) -> Result<(), String>;
}

#[derive(Clone, Debug, Eq, PartialEq, thiserror::Error)]
pub enum TerminalOwnerError {
    #[error("stdin is not a terminal")]
    StdinNotTty,
    #[error("stdout is not a terminal")]
    StdoutNotTty,
    #[error("terminal capture failed: {0}")]
    Capture(String),
    #[error("terminal apply failed: {0}")]
    Apply(String),
    #[error("terminal width failed: {0}")]
    Width(String),
    #[error("terminal screen entry failed: {0}")]
    Screen(String),
}

/// Owns native terminal mutation after capturing stdin attributes. Cleanup is
/// idempotent and is also attempted by Drop during unwinding.
pub struct TerminalOwner<S: TerminalSyscalls> {
    syscalls: S,
    saved: Option<S::Saved>,
    screen_entered: bool,
    cleanup_diagnostics: Vec<String>,
}

impl<S: TerminalSyscalls> TerminalOwner<S> {
    #[must_use]
    pub fn new(syscalls: S) -> Self {
        Self {
            syscalls,
            saved: None,
            screen_entered: false,
            cleanup_diagnostics: Vec::new(),
        }
    }

    pub fn enter(&mut self) -> Result<(), TerminalOwnerError> {
        if !self.syscalls.stdin_is_tty() {
            return Err(TerminalOwnerError::StdinNotTty);
        }
        if !self.syscalls.stdout_is_tty() {
            return Err(TerminalOwnerError::StdoutNotTty);
        }
        let saved = self
            .syscalls
            .capture_stdin()
            .map_err(TerminalOwnerError::Capture)?;
        self.saved = Some(saved.clone());
        let raw = self.syscalls.raw_mode(&saved);
        if let Err(error) = self.syscalls.apply_stdin(&raw) {
            let cleanup = self.restore_once().err();
            return Err(TerminalOwnerError::Apply(with_cleanup(error, cleanup)));
        }
        if let Err(error) = self.syscalls.stdout_width() {
            let cleanup = self.restore_once().err();
            return Err(TerminalOwnerError::Width(with_cleanup(error, cleanup)));
        }
        self.screen_entered = true;
        if let Err(error) = self.syscalls.write_stdout("\x1b[?1049h\x1b[?25l") {
            let cleanup = self.restore_once().err();
            return Err(TerminalOwnerError::Screen(with_cleanup(error, cleanup)));
        }
        Ok(())
    }

    pub fn width(&mut self) -> Result<usize, String> {
        self.syscalls.stdout_width()
    }

    pub fn write_frame(&mut self, frame: &str) -> Result<(), String> {
        self.syscalls.write_stdout(frame)
    }

    pub fn restore_once(&mut self) -> Result<(), String> {
        let mut diagnostics = Vec::new();
        if let Some(saved) = self.saved.take()
            && let Err(error) = self.syscalls.apply_stdin(&saved)
        {
            diagnostics.push(format!("restore stdin termios: {error}"));
        }
        if self.screen_entered {
            self.screen_entered = false;
            if let Err(error) = self.syscalls.write_stdout("\x1b[?25h\x1b[?1049l") {
                diagnostics.push(format!("restore screen: {error}"));
            }
        }
        self.cleanup_diagnostics.extend(diagnostics.clone());
        (!diagnostics.is_empty())
            .then(|| diagnostics.join("; "))
            .map_or(Ok(()), Err)
    }

    #[must_use]
    pub fn cleanup_diagnostics(&self) -> &[String] {
        &self.cleanup_diagnostics
    }

    #[must_use]
    pub fn syscalls(&self) -> &S {
        &self.syscalls
    }
}

impl<S: TerminalSyscalls> Drop for TerminalOwner<S> {
    fn drop(&mut self) {
        let _ = self.restore_once();
    }
}

fn with_cleanup(primary: String, cleanup: Option<String>) -> String {
    cleanup.map_or(primary.clone(), |cleanup| {
        format!("{primary}; cleanup: {cleanup}")
    })
}

pub(crate) struct SystemTerminalSyscalls;

impl TerminalSyscalls for SystemTerminalSyscalls {
    type Saved = Termios;

    fn stdin_is_tty(&mut self) -> bool {
        std::io::stdin().is_terminal()
    }

    fn stdout_is_tty(&mut self) -> bool {
        std::io::stdout().is_terminal()
    }

    fn capture_stdin(&mut self) -> Result<Self::Saved, String> {
        termios::tcgetattr(std::io::stdin()).map_err(|error| error.to_string())
    }

    fn raw_mode(&mut self, saved: &Self::Saved) -> Self::Saved {
        let mut raw = saved.clone();
        termios::cfmakeraw(&mut raw);
        raw
    }

    fn apply_stdin(&mut self, settings: &Self::Saved) -> Result<(), String> {
        termios::tcsetattr(std::io::stdin(), SetArg::TCSANOW, settings)
            .map_err(|error| error.to_string())
    }

    fn stdout_width(&mut self) -> Result<usize, String> {
        let width = rustix::termios::tcgetwinsize(std::io::stdout())
            .map_err(|error| error.to_string())?
            .ws_col;
        let width = usize::from(width);
        (width > 0)
            .then_some(width)
            .ok_or_else(|| "terminal width unavailable".to_owned())
    }

    fn write_stdout(&mut self, bytes: &str) -> Result<(), String> {
        let mut output = std::io::stdout().lock();
        output
            .write_all(bytes.as_bytes())
            .and_then(|_| output.flush())
            .map_err(|error| error.to_string())
    }
}

pub(crate) struct ProductionTerminal {
    owner: TerminalOwner<SystemTerminalSyscalls>,
    keys: Receiver<u8>,
}
impl ProductionTerminal {
    pub(crate) fn new() -> Self {
        let (sender, keys) = mpsc::channel();
        std::thread::spawn(move || {
            let mut input = std::io::stdin();
            let mut byte = [0_u8; 1];
            while input.read_exact(&mut byte).is_ok() {
                if sender.send(byte[0]).is_err() {
                    break;
                }
            }
        });
        Self {
            owner: TerminalOwner::new(SystemTerminalSyscalls),
            keys,
        }
    }
}
impl TopTerminal for ProductionTerminal {
    fn enter(&mut self) -> Result<(), String> {
        self.owner.enter().map_err(|error| error.to_string())
    }
    fn restore(&mut self) -> Result<(), String> {
        self.owner.restore_once()
    }
    fn width(&mut self) -> Result<usize, String> {
        self.owner.width()
    }
    fn render(&mut self, frame: &str) -> Result<(), String> {
        self.owner.write_frame(frame)
    }
    fn input(&mut self, timeout_seconds: f64) -> Result<TopInput, String> {
        match self
            .keys
            .recv_timeout(Duration::from_secs_f64(timeout_seconds))
        {
            Ok(b'q') => Ok(TopInput::Quit),
            Ok(3) => Ok(TopInput::Interrupt),
            Ok(4) => Ok(TopInput::EndOfFile),
            Ok(b'r') => Ok(TopInput::Restart),
            Ok(b'\x1b') => self.decode_escape(),
            Ok(_) => Ok(TopInput::None),
            Err(mpsc::RecvTimeoutError::Timeout) => Ok(TopInput::None),
            Err(mpsc::RecvTimeoutError::Disconnected) => Ok(TopInput::EndOfFile),
        }
    }
}

impl ProductionTerminal {
    fn decode_escape(&self) -> Result<TopInput, String> {
        let wait = Duration::from_millis(10);
        match self.keys.recv_timeout(wait) {
            Ok(b'[') => match self.keys.recv_timeout(wait) {
                Ok(b'A') => Ok(TopInput::Up),
                Ok(b'B') => Ok(TopInput::Down),
                Ok(_) | Err(mpsc::RecvTimeoutError::Timeout) => Ok(TopInput::None),
                Err(mpsc::RecvTimeoutError::Disconnected) => Ok(TopInput::EndOfFile),
            },
            Ok(_) | Err(mpsc::RecvTimeoutError::Timeout) => Ok(TopInput::None),
            Err(mpsc::RecvTimeoutError::Disconnected) => Ok(TopInput::EndOfFile),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{TopReceiveTransport, TopRestartTransport};
    use solstone_core_callosum::{CallosumEnvelope, CallosumSocketServer};
    use std::sync::atomic::{AtomicUsize, Ordering};

    static NEXT_SOCKET: AtomicUsize = AtomicUsize::new(0);
    #[test]
    fn production_callosum_start_stop_and_restart_adapter_are_safe_without_server() {
        let path = std::env::temp_dir().join("solstone-top-no-server.sock");
        let shared = ProductionCallosum::new(path).unwrap();
        let mut receive = ProductionReceive::new(Arc::clone(&shared));
        let mut restart = ProductionRestart::new(shared);
        receive.start().unwrap();
        assert!(matches!(
            receive.next().unwrap(),
            Some(CallosumReceiveEvent::Continuity {
                generation: 0,
                epoch: 0,
                phase: solstone_core_callosum::CallosumConnectionPhase::Connecting { attempt: 1 },
            })
        ));
        assert_eq!(restart.current_generation(), 0);
        assert_eq!(
            restart.emit_restart("convey", "id"),
            RestartEnqueueResult::Enqueued
        );
        receive.stop().unwrap();
    }

    #[tokio::test]
    async fn production_receive_is_driven_while_the_sync_consumer_polls() {
        let ordinal = NEXT_SOCKET.fetch_add(1, Ordering::Relaxed);
        let root = std::env::temp_dir().join(format!("solstone-top-production-{ordinal}"));
        std::fs::create_dir_all(&root).unwrap();
        let socket = root.join("callosum.sock");
        let server = CallosumSocketServer::bind(&socket).await.unwrap();
        let shared = ProductionCallosum::new(&socket).unwrap();
        let mut receive = ProductionReceive::new(shared);
        receive.start().unwrap();
        tokio::time::timeout(Duration::from_secs(2), async {
            while server.client_count() != 1 {
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .unwrap();
        assert!(server.broadcast(CallosumEnvelope {
            tract: "supervisor".to_owned(),
            event: "status".to_owned(),
            ts: None,
            extra: Map::new(),
        }));
        let event = tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                if let Some(event @ CallosumReceiveEvent::Envelope { .. }) = receive.next().unwrap()
                {
                    return event;
                }
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .unwrap();
        assert!(matches!(event, CallosumReceiveEvent::Envelope { .. }));
        std::thread::spawn(move || receive.stop().unwrap())
            .join()
            .unwrap();
        server.stop().await;
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn terminal_decodes_only_complete_up_and_down_escape_sequences() {
        let (sender, keys) = mpsc::channel();
        let mut terminal = ProductionTerminal {
            owner: TerminalOwner::new(SystemTerminalSyscalls),
            keys,
        };
        sender.send(b'\x1b').unwrap();
        sender.send(b'[').unwrap();
        sender.send(b'A').unwrap();
        assert_eq!(terminal.input(0.0).unwrap(), TopInput::Up);
        sender.send(b'\x1b').unwrap();
        sender.send(b'[').unwrap();
        sender.send(b'B').unwrap();
        assert_eq!(terminal.input(0.0).unwrap(), TopInput::Down);
        sender.send(b'\x1b').unwrap();
        assert_eq!(terminal.input(0.0).unwrap(), TopInput::None);
    }
}
