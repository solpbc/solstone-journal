// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Concrete adapters for the interactive command. The reducer and loop remain
//! fully injected; this is the only module that touches clocks, terminal I/O,
//! the filesystem, or a live Callosum socket.

use std::collections::VecDeque;
use std::io::{IsTerminal, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use chrono::Utc;
use crossterm::cursor::{Hide, MoveTo, Show};
use crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers};
use crossterm::style::{Attribute, Color, Print, ResetColor, SetAttribute, SetForegroundColor};
use crossterm::terminal::{
    Clear, ClearType, EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode,
    enable_raw_mode, size,
};
use crossterm::{execute, queue};
use serde_json::{Map, Value, json};
use solstone_core_brain::{inspect_brain_state, present_brain_inspection, read_journal_config};
use solstone_core_callosum::{CallosumReceiveEvent, CallosumSocketConnection};
use solstone_core_journal::{discover_home, read_config_journal, resolve_journal_path};
use solstone_core_system_health::sanitize_os_bytes_for_terminal;

use crate::{
    RestartEnqueueResult, RestartIdSource, SessionRestartIds, TopBrainSource, TopClock, TopInput,
    TopReceiveTransport, TopRenderOp, TopRestartTransport, TopState, TopTerminal, TrustedToken,
    frame_ops, platform_observer, run_top_with,
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
pub struct ProductionCallosum {
    inner: Mutex<ProductionCallosumInner>,
}
impl ProductionCallosum {
    pub fn new(path: impl AsRef<Path>) -> Result<Arc<Self>, String> {
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
pub struct ProductionReceive {
    shared: Arc<ProductionCallosum>,
}
impl ProductionReceive {
    pub fn new(shared: Arc<ProductionCallosum>) -> Self {
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
    fn stdin_is_tty(&mut self) -> bool;
    fn stdout_is_tty(&mut self) -> bool;
    fn enable_raw_mode(&mut self) -> Result<(), String>;
    fn disable_raw_mode(&mut self) -> Result<(), String>;
    fn enter_alt_screen(&mut self) -> Result<(), String>;
    fn leave_alt_screen(&mut self) -> Result<(), String>;
    fn hide_cursor(&mut self) -> Result<(), String>;
    fn show_cursor(&mut self) -> Result<(), String>;
    fn reset_style(&mut self) -> Result<(), String>;
    fn stdout_width(&mut self) -> Result<usize, String>;
    fn write_ops(&mut self, ops: &[TopRenderOp]) -> Result<(), String>;
}

#[derive(Clone, Debug, Eq, PartialEq, thiserror::Error)]
pub enum TerminalOwnerError {
    #[error("stdin is not a terminal")]
    StdinNotTty,
    #[error("stdout is not a terminal")]
    StdoutNotTty,
    #[error("terminal apply failed: {0}")]
    Apply(String),
    #[error("terminal width failed: {0}")]
    Width(String),
    #[error("terminal screen entry failed: {0}")]
    Screen(String),
}

/// Owns native terminal mutation after acquiring raw mode, the alternate screen,
/// and a hidden cursor. Cleanup is idempotent and is also attempted by Drop.
pub struct TerminalOwner<S: TerminalSyscalls> {
    syscalls: S,
    raw_mode: bool,
    alt_screen: bool,
    cursor_hidden: bool,
    cleanup_diagnostics: Vec<String>,
}

impl<S: TerminalSyscalls> TerminalOwner<S> {
    #[must_use]
    pub fn new(syscalls: S) -> Self {
        Self {
            syscalls,
            raw_mode: false,
            alt_screen: false,
            cursor_hidden: false,
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
        if let Err(error) = self.syscalls.enable_raw_mode() {
            let cleanup = self.restore_once().err();
            return Err(TerminalOwnerError::Apply(with_cleanup(error, cleanup)));
        }
        self.raw_mode = true;
        if let Err(error) = self.syscalls.enter_alt_screen() {
            let cleanup = self.restore_once().err();
            return Err(TerminalOwnerError::Screen(with_cleanup(error, cleanup)));
        }
        self.alt_screen = true;
        if let Err(error) = self.syscalls.hide_cursor() {
            let cleanup = self.restore_once().err();
            return Err(TerminalOwnerError::Screen(with_cleanup(error, cleanup)));
        }
        self.cursor_hidden = true;
        if let Err(error) = self.syscalls.stdout_width() {
            let cleanup = self.restore_once().err();
            return Err(TerminalOwnerError::Width(with_cleanup(error, cleanup)));
        }
        Ok(())
    }

    pub fn width(&mut self) -> Result<usize, String> {
        self.syscalls.stdout_width()
    }

    pub fn write_frame(&mut self, frame: &str) -> Result<(), String> {
        self.syscalls.write_ops(&frame_ops(frame))
    }

    pub fn restore_once(&mut self) -> Result<(), String> {
        let mut diagnostics = Vec::new();
        if (self.alt_screen || self.cursor_hidden)
            && let Err(error) = self.syscalls.reset_style()
        {
            diagnostics.push(format!("restore style: {error}"));
        }
        if self.cursor_hidden {
            self.cursor_hidden = false;
            if let Err(error) = self.syscalls.show_cursor() {
                diagnostics.push(format!("restore cursor: {error}"));
            }
        }
        if self.alt_screen {
            self.alt_screen = false;
            if let Err(error) = self.syscalls.leave_alt_screen() {
                diagnostics.push(format!("restore screen: {error}"));
            }
        }
        if self.raw_mode {
            self.raw_mode = false;
            if let Err(error) = self.syscalls.disable_raw_mode() {
                diagnostics.push(format!("restore raw mode: {error}"));
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

fn execute_cmd(command: impl crossterm::Command) -> Result<(), String> {
    let mut out = std::io::stdout().lock();
    execute!(out, command).map_err(|error| error.to_string())
}

fn queue_op(out: &mut impl Write, op: &TopRenderOp) -> std::io::Result<()> {
    match op {
        TopRenderOp::Style(TrustedToken::Home) => queue!(out, MoveTo(0, 0)),
        TopRenderOp::Style(TrustedToken::Clear) => queue!(out, Clear(ClearType::All)),
        TopRenderOp::Style(TrustedToken::Bold) => queue!(out, SetAttribute(Attribute::Bold)),
        TopRenderOp::Style(TrustedToken::Dim) => queue!(out, SetAttribute(Attribute::Dim)),
        TopRenderOp::Style(TrustedToken::Red) => {
            queue!(out, SetForegroundColor(Color::DarkRed))
        }
        TopRenderOp::Style(TrustedToken::Green) => {
            queue!(out, SetForegroundColor(Color::DarkGreen))
        }
        TopRenderOp::Style(TrustedToken::Yellow) => {
            queue!(out, SetForegroundColor(Color::DarkYellow))
        }
        TopRenderOp::Style(TrustedToken::Cyan) => {
            queue!(out, SetForegroundColor(Color::DarkCyan))
        }
        TopRenderOp::Style(TrustedToken::Magenta) => {
            queue!(out, SetForegroundColor(Color::DarkMagenta))
        }
        TopRenderOp::Style(TrustedToken::Select) => queue!(out, SetAttribute(Attribute::Reverse)),
        TopRenderOp::Style(TrustedToken::EndSelect) => {
            queue!(out, SetAttribute(Attribute::NoReverse))
        }
        TopRenderOp::Style(TrustedToken::Normal) => queue!(out, SetAttribute(Attribute::Reset)),
        TopRenderOp::Print(text) => queue!(out, Print(text.as_str())),
    }
}

impl TerminalSyscalls for SystemTerminalSyscalls {
    fn stdin_is_tty(&mut self) -> bool {
        std::io::stdin().is_terminal()
    }

    fn stdout_is_tty(&mut self) -> bool {
        std::io::stdout().is_terminal()
    }

    fn enable_raw_mode(&mut self) -> Result<(), String> {
        enable_raw_mode().map_err(|error| error.to_string())
    }

    fn disable_raw_mode(&mut self) -> Result<(), String> {
        disable_raw_mode().map_err(|error| error.to_string())
    }

    fn enter_alt_screen(&mut self) -> Result<(), String> {
        execute_cmd(EnterAlternateScreen)
    }

    fn leave_alt_screen(&mut self) -> Result<(), String> {
        execute_cmd(LeaveAlternateScreen)
    }

    fn hide_cursor(&mut self) -> Result<(), String> {
        execute_cmd(Hide)
    }

    fn show_cursor(&mut self) -> Result<(), String> {
        execute_cmd(Show)
    }

    fn reset_style(&mut self) -> Result<(), String> {
        execute_cmd(ResetColor)
    }

    fn stdout_width(&mut self) -> Result<usize, String> {
        let (columns, _) = size().map_err(|error| error.to_string())?;
        let width = usize::from(columns);
        (width > 0)
            .then_some(width)
            .ok_or_else(|| "terminal width unavailable".to_owned())
    }

    fn write_ops(&mut self, ops: &[TopRenderOp]) -> Result<(), String> {
        let mut out = std::io::stdout().lock();
        for op in ops {
            queue_op(&mut out, op).map_err(|error| error.to_string())?;
        }
        out.flush().map_err(|error| error.to_string())
    }
}

enum InputSource {
    Live,
    Scripted(VecDeque<Event>),
}

pub struct ProductionTerminal {
    owner: TerminalOwner<SystemTerminalSyscalls>,
    events: InputSource,
}
impl ProductionTerminal {
    pub fn from_events(events: VecDeque<Event>) -> Self {
        Self {
            owner: TerminalOwner::new(SystemTerminalSyscalls),
            events: InputSource::Scripted(events),
        }
    }

    #[allow(clippy::new_without_default)]
    pub fn new() -> Self {
        Self {
            owner: TerminalOwner::new(SystemTerminalSyscalls),
            events: InputSource::Live,
        }
    }
}

pub(crate) fn map_event(event: Event) -> TopInput {
    match event {
        Event::Key(key) => match (key.code, key.kind) {
            (KeyCode::Up, KeyEventKind::Press | KeyEventKind::Repeat) => TopInput::Up,
            (KeyCode::Down, KeyEventKind::Press | KeyEventKind::Repeat) => TopInput::Down,
            (KeyCode::Char('q'), KeyEventKind::Press)
                if !key.modifiers.contains(KeyModifiers::CONTROL) =>
            {
                TopInput::Quit
            }
            (KeyCode::Char('r'), KeyEventKind::Press)
                if !key.modifiers.contains(KeyModifiers::CONTROL) =>
            {
                TopInput::Restart
            }
            (KeyCode::Char('c' | 'C'), KeyEventKind::Press)
                if key.modifiers.contains(KeyModifiers::CONTROL) =>
            {
                TopInput::Interrupt
            }
            (KeyCode::Char('d' | 'D'), KeyEventKind::Press)
                if key.modifiers.contains(KeyModifiers::CONTROL) =>
            {
                TopInput::EndOfFile
            }
            _ => TopInput::None,
        },
        Event::Resize(_, _) | Event::FocusGained | Event::FocusLost | Event::Mouse(_) => {
            TopInput::None
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
        match &mut self.events {
            InputSource::Scripted(events) => {
                Ok(events.pop_front().map_or(TopInput::None, map_event))
            }
            InputSource::Live => match event::poll(Duration::from_secs_f64(timeout_seconds)) {
                Ok(false) => Ok(TopInput::None),
                Ok(true) => match event::read() {
                    Ok(event) => Ok(map_event(event)),
                    Err(_) => Ok(TopInput::EndOfFile),
                },
                Err(_) => Ok(TopInput::EndOfFile),
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{TopReceiveTransport, TopRestartTransport};

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

    fn assert_fragments_in_order(output: &str, fragments: &[&str]) {
        let mut cursor = 0usize;
        for fragment in fragments {
            let rest = &output[cursor..];
            let at = rest
                .find(fragment)
                .unwrap_or_else(|| panic!("missing {fragment:?} after {cursor} in {output:?}"));
            cursor += at + fragment.len();
        }
    }

    #[test]
    fn queue_op_emits_crossterm_sequences_in_token_order_with_dark_256_color_sgr() {
        crossterm::style::Colored::set_ansi_color_disabled(false);
        let ops = [
            TopRenderOp::Style(TrustedToken::Clear),
            TopRenderOp::Style(TrustedToken::Home),
            TopRenderOp::Style(TrustedToken::Bold),
            TopRenderOp::Style(TrustedToken::Dim),
            TopRenderOp::Style(TrustedToken::Select),
            TopRenderOp::Style(TrustedToken::EndSelect),
            TopRenderOp::Style(TrustedToken::Normal),
            TopRenderOp::Style(TrustedToken::Red),
            TopRenderOp::Style(TrustedToken::Green),
            TopRenderOp::Style(TrustedToken::Yellow),
            TopRenderOp::Style(TrustedToken::Cyan),
            TopRenderOp::Style(TrustedToken::Magenta),
            TopRenderOp::Print("payload-text".to_owned()),
            TopRenderOp::Style(TrustedToken::Normal),
        ];
        let mut bytes = Vec::new();
        for op in &ops {
            queue_op(&mut bytes, op).unwrap();
        }
        let output = String::from_utf8(bytes).unwrap();
        assert_fragments_in_order(
            &output,
            &[
                "\x1b[2J",
                "\x1b[1;1H",
                "\x1b[1m",
                "\x1b[2m",
                "\x1b[7m",
                "\x1b[27m",
                "\x1b[0m",
                "\x1b[38;5;1m",
                "\x1b[38;5;2m",
                "\x1b[38;5;3m",
                "\x1b[38;5;6m",
                "\x1b[38;5;5m",
                "payload-text",
                "\x1b[0m",
            ],
        );
        for forbidden in [
            "\x1b[31m",
            "\x1b[32m",
            "\x1b[33m",
            "\x1b[36m",
            "\x1b[35m",
            "\x1b[38;5;9m",
            "\x1b[38;5;10m",
            "\x1b[38;5;11m",
            "\x1b[38;5;14m",
            "\x1b[38;5;13m",
        ] {
            assert!(
                !output.contains(forbidden),
                "bright or 3/4-bit color {forbidden:?} in {output:?}"
            );
        }
    }
}
