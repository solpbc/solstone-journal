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
use serde_json::{Map, Value, json};
use solstone_core_brain::{inspect_brain_state, present_brain_inspection, read_journal_config};
use solstone_core_callosum::{CallosumReceiveEvent, CallosumSocketConnection};
use solstone_core_journal::{discover_home, read_config_journal, resolve_journal_path};
use solstone_core_system_health::sanitize_os_bytes_for_terminal;

use crate::{
    TopBrainSource, TopClock, TopInput, TopReceiveTransport, TopRestartError, TopRestartTransport,
    TopState, TopTerminal, platform_observer, run_top_with,
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
    let mut terminal = ProductionTerminal::new()?;
    let mut clock = ProductionClock::new();
    let mut observer = platform_observer();
    let mut brain = ProductionBrain::new(journal);
    run_top_with(
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
        Ok(
            json!({"headline":presentation.headline,"reason":presentation.reason_text,"failing_component":presentation.failing_component,"evidence":presentation.evidence.age_text}),
        )
    }
}

struct ProductionCallosumInner {
    runtime: tokio::runtime::Runtime,
    connection: CallosumSocketConnection,
    generation: u64,
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
            CallosumReceiveEvent::Envelope { generation, .. }
            | CallosumReceiveEvent::Discontinuity { generation, .. },
        ) = &event
        {
            inner.generation = *generation;
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
    fn emit_restart(&self, service: &str, restart_id: &str) -> Result<(), TopRestartError> {
        let inner = self.inner.lock().map_err(|_| TopRestartError::Transport)?;
        let mut values = Map::new();
        values.insert("service".to_owned(), Value::String(service.to_owned()));
        values.insert(
            "restart_id".to_owned(),
            Value::String(restart_id.to_owned()),
        );
        inner
            .connection
            .emit("supervisor", "restart", values)
            .then_some(())
            .ok_or(TopRestartError::Transport)
    }
    fn generation(&self) -> u64 {
        self.inner.lock().map_or(0, |inner| inner.generation)
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
}
impl ProductionRestart {
    pub(crate) fn new(shared: Arc<ProductionCallosum>) -> Self {
        Self { shared }
    }
}
impl TopRestartTransport for ProductionRestart {
    fn emit_restart(&mut self, service: &str, restart_id: &str) -> Result<(), TopRestartError> {
        self.shared.emit_restart(service, restart_id)
    }
    fn current_generation(&self) -> u64 {
        self.shared.generation()
    }
}

pub(crate) struct ProductionTerminal {
    saved: Option<String>,
    keys: Receiver<u8>,
}
impl ProductionTerminal {
    pub(crate) fn new() -> Result<Self, String> {
        if !std::io::stdin().is_terminal() || !std::io::stdout().is_terminal() {
            return Err("top requires an interactive terminal".to_owned());
        }
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
        Ok(Self { saved: None, keys })
    }
    fn stty(args: &[&str]) -> Result<String, String> {
        let output = std::process::Command::new("stty")
            .args(args)
            .output()
            .map_err(|error| error.to_string())?;
        output
            .status
            .success()
            .then(|| String::from_utf8_lossy(&output.stdout).trim().to_owned())
            .ok_or_else(|| String::from_utf8_lossy(&output.stderr).trim().to_owned())
    }
}
impl TopTerminal for ProductionTerminal {
    fn enter(&mut self) -> Result<(), String> {
        self.saved = Some(Self::stty(&["-g"])?);
        Self::stty(&["raw", "-echo"])?;
        Ok(())
    }
    fn restore(&mut self) -> Result<(), String> {
        if let Some(saved) = self.saved.take() {
            Self::stty(&[&saved])?;
        }
        Ok(())
    }
    fn width(&mut self) -> Result<usize, String> {
        let size = Self::stty(&["size"])?;
        size.split_whitespace()
            .nth(1)
            .and_then(|value| value.parse().ok())
            .ok_or_else(|| "terminal width unavailable".to_owned())
    }
    fn render(&mut self, frame: &str) -> Result<(), String> {
        let mut output = std::io::stdout().lock();
        output
            .write_all(frame.as_bytes())
            .and_then(|_| output.flush())
            .map_err(|error| error.to_string())
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
        assert!(receive.next().unwrap().is_none());
        assert_eq!(restart.current_generation(), 0);
        assert!(restart.emit_restart("convey", "id").is_ok());
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
        let mut terminal = ProductionTerminal { saved: None, keys };
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
