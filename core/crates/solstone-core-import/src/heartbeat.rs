// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::io;
use std::path::PathBuf;
use std::sync::{Arc, Condvar, Mutex, PoisonError};
use std::thread::{Builder, JoinHandle};
use std::time::{Duration, Instant};

use crate::events::{EventEmitter, ImporterStatus, emit_importer_status};

pub type HeartbeatSpawner = fn(Builder, Box<dyn FnOnce() + Send>) -> io::Result<JoinHandle<()>>;

#[derive(Debug)]
pub struct HeartbeatExit<T> {
    value: T,
}

impl<T> HeartbeatExit<T> {
    /// Wraps a value without stopping the heartbeat.
    #[must_use]
    pub fn residual(value: T) -> Self {
        Self { value }
    }

    #[must_use]
    pub fn into_inner(self) -> T {
        self.value
    }
}

struct State {
    stop: bool,
    sending: bool,
}

struct SendingGuard {
    state: Arc<(Mutex<State>, Condvar)>,
}

impl Drop for SendingGuard {
    fn drop(&mut self) {
        let (lock, cvar) = &*self.state;
        let mut state = lock.lock().unwrap_or_else(PoisonError::into_inner);
        state.sending = false;
        cvar.notify_all();
    }
}

#[allow(clippy::too_many_arguments)]
fn worker_loop(
    interval: Duration,
    state: Arc<(Mutex<State>, Condvar)>,
    sink: Arc<dyn Fn(&ImporterStatus) + Send + Sync>,
    import_id: String,
    generation: u64,
    attempt_id: String,
    stage: String,
    started: Instant,
    on_worker_exit: Arc<dyn Fn() + Send + Sync>,
) {
    let (lock, cvar) = &*state;
    loop {
        let mut st = lock.lock().unwrap_or_else(PoisonError::into_inner);
        if st.stop {
            break;
        }
        let (st_after_wait, timeout_result) = cvar
            .wait_timeout(st, interval)
            .unwrap_or_else(PoisonError::into_inner);
        st = st_after_wait;
        if st.stop {
            break;
        }
        if timeout_result.timed_out() {
            st.sending = true;
            drop(st);

            {
                let _guard = SendingGuard {
                    state: Arc::clone(&state),
                };
                let elapsed_ms = started.elapsed().as_millis().min(u64::MAX as u128) as u64;
                let status = ImporterStatus {
                    import_id: import_id.clone(),
                    stage: stage.clone(),
                    elapsed_ms,
                    stage_elapsed_ms: elapsed_ms,
                    items_processed: 0,
                    items_total: 0,
                    earliest_date: None,
                    latest_date: None,
                    entities_found: 0,
                    source_type: None,
                    source_display: None,
                    generation: Some(generation),
                    attempt_id: Some(attempt_id.clone()),
                };
                sink(&status);
            }

            let st2 = lock.lock().unwrap_or_else(PoisonError::into_inner);
            if st2.stop {
                break;
            }
        }
    }
    on_worker_exit();
}

pub struct ImportHeartbeat {
    state: Option<Arc<(Mutex<State>, Condvar)>>,
    handle: Option<JoinHandle<()>>,
    on_close: Option<Arc<dyn Fn() + Send + Sync>>,
}

impl ImportHeartbeat {
    #[must_use]
    pub fn start(
        interval: Option<Duration>,
        journal: PathBuf,
        import_id: String,
        generation: u64,
        attempt_id: String,
        stage: String,
        started: Instant,
    ) -> Self {
        let sink = Arc::new(move |status: &ImporterStatus| {
            let emitter = EventEmitter::new(&journal, None);
            emit_importer_status(&emitter, status);
        });
        let on_close: Arc<dyn Fn() + Send + Sync> = Arc::new(|| {});
        let on_worker_exit: Arc<dyn Fn() + Send + Sync> = Arc::new(|| {});
        Self::start_injected(
            interval,
            sink,
            default_spawner,
            on_close,
            on_worker_exit,
            import_id,
            generation,
            attempt_id,
            stage,
            started,
        )
    }

    #[allow(clippy::too_many_arguments)]
    pub fn start_injected(
        interval: Option<Duration>,
        sink: Arc<dyn Fn(&ImporterStatus) + Send + Sync>,
        spawn: HeartbeatSpawner,
        on_close: Arc<dyn Fn() + Send + Sync>,
        on_worker_exit: Arc<dyn Fn() + Send + Sync>,
        import_id: String,
        generation: u64,
        attempt_id: String,
        stage: String,
        started: Instant,
    ) -> Self {
        let Some(interval) = interval else {
            return Self {
                state: None,
                handle: None,
                on_close: Some(on_close),
            };
        };

        let state = Arc::new((
            Mutex::new(State {
                stop: false,
                sending: false,
            }),
            Condvar::new(),
        ));
        let worker_state = Arc::clone(&state);

        let builder = Builder::new().name("import-heartbeat".to_owned());
        let spawn_res = spawn(
            builder,
            Box::new(move || {
                worker_loop(
                    interval,
                    worker_state,
                    sink,
                    import_id,
                    generation,
                    attempt_id,
                    stage,
                    started,
                    on_worker_exit,
                );
            }),
        );

        match spawn_res {
            Ok(handle) => Self {
                state: Some(state),
                handle: Some(handle),
                on_close: Some(on_close),
            },
            Err(_) => Self {
                state: None,
                handle: None,
                on_close: Some(on_close),
            },
        }
    }

    pub fn finish(&mut self) {
        if let Some(state) = self.state.take() {
            let (lock, cvar) = &*state;
            {
                let mut st = lock.lock().unwrap_or_else(PoisonError::into_inner);
                st.stop = true;
                cvar.notify_all();
                while st.sending {
                    st = cvar.wait(st).unwrap_or_else(PoisonError::into_inner);
                }
            }
        }
        if let Some(on_close) = self.on_close.take() {
            on_close();
        }
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }

    #[allow(clippy::double_must_use)]
    #[must_use]
    pub fn finish_then<T, E>(
        mut self,
        emit: impl FnOnce() -> Result<T, E>,
    ) -> Result<HeartbeatExit<T>, HeartbeatExit<E>> {
        self.finish();
        match emit() {
            Ok(value) => Ok(HeartbeatExit { value }),
            Err(error) => Err(HeartbeatExit { value: error }),
        }
    }
}

impl Drop for ImportHeartbeat {
    fn drop(&mut self) {
        self.finish();
    }
}

fn default_spawner(builder: Builder, task: Box<dyn FnOnce() + Send>) -> io::Result<JoinHandle<()>> {
    builder.spawn(task)
}

#[cfg(all(test, feature = "full-tests"))]
mod tests {
    use super::*;
    use std::sync::mpsc;

    #[test]
    fn test_heartbeat_sends_while_running_and_stops_on_finish() {
        let sent = Arc::new(Mutex::new(Vec::new()));
        let sent_clone = Arc::clone(&sent);
        let sink = Arc::new(move |status: &ImporterStatus| {
            sent_clone
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .push(status.clone());
        });
        let on_close = Arc::new(|| {});
        let on_worker_exit = Arc::new(|| {});

        let started = Instant::now();
        let mut heartbeat = ImportHeartbeat::start_injected(
            Some(Duration::from_millis(20)),
            sink,
            default_spawner,
            on_close,
            on_worker_exit,
            "imp_1".to_owned(),
            3,
            "att_1".to_owned(),
            "execution".to_owned(),
            started,
        );

        let deadline = Instant::now() + Duration::from_secs(2);
        while Instant::now() < deadline {
            let count = sent.lock().unwrap_or_else(PoisonError::into_inner).len();
            if count >= 2 {
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }

        heartbeat.finish();

        let recorded = sent.lock().unwrap_or_else(PoisonError::into_inner).clone();
        assert!(
            recorded.len() >= 2,
            "expected at least 2 sends, got {}",
            recorded.len()
        );

        let mut prev_elapsed = 0;
        for status in &recorded {
            assert_eq!(status.import_id, "imp_1");
            assert_eq!(status.generation, Some(3));
            assert_eq!(status.attempt_id.as_deref(), Some("att_1"));
            assert_eq!(status.stage, "execution");
            assert_eq!(status.items_processed, 0);
            assert_eq!(status.items_total, 0);
            assert_eq!(status.entities_found, 0);
            assert_eq!(status.stage_elapsed_ms, status.elapsed_ms);
            assert!(
                status.elapsed_ms >= prev_elapsed,
                "elapsed should be non-decreasing: {} vs {}",
                status.elapsed_ms,
                prev_elapsed
            );
            assert!(status.elapsed_ms > 0);
            prev_elapsed = status.elapsed_ms;
        }

        let count_at_finish = recorded.len();
        std::thread::sleep(Duration::from_millis(50));
        let count_after_wait = sent.lock().unwrap_or_else(PoisonError::into_inner).len();
        assert_eq!(
            count_at_finish, count_after_wait,
            "no sends should occur after finish"
        );
    }

    #[test]
    fn test_heartbeat_close_waits_for_send() {
        let log = Arc::new(Mutex::new(Vec::new()));
        let (enter_tx, enter_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let release_rx = Mutex::new(release_rx);

        let log_sink = Arc::clone(&log);
        // Only the first send blocks: a later one, if the worker ticks again
        // before finish closes, must not wait for a release that never comes.
        let first = std::sync::atomic::AtomicBool::new(true);
        let sink = Arc::new(move |_status: &ImporterStatus| {
            if first.swap(false, std::sync::atomic::Ordering::SeqCst) {
                let _ = enter_tx.send(());
                let _ = release_rx
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .recv();
            }
            log_sink
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .push("sent");
        });

        let log_close = Arc::clone(&log);
        let on_close = Arc::new(move || {
            log_close
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .push("closed");
        });
        let on_worker_exit = Arc::new(|| {});

        let heartbeat = ImportHeartbeat::start_injected(
            Some(Duration::from_millis(10)),
            sink,
            default_spawner,
            on_close,
            on_worker_exit,
            "imp_2".to_owned(),
            1,
            "att_2".to_owned(),
            "execution".to_owned(),
            Instant::now(),
        );

        enter_rx
            .recv_timeout(Duration::from_secs(1))
            .expect("worker enters sink");

        let finish_handle = Builder::new()
            .name("finish-thread".to_owned())
            .spawn(move || {
                let mut hb = heartbeat;
                hb.finish();
            })
            .expect("spawn finish thread");

        std::thread::sleep(Duration::from_millis(30));
        {
            let current_log = log.lock().unwrap_or_else(PoisonError::into_inner).clone();
            assert!(
                !current_log.contains(&"closed"),
                "finish must not run on_close while sink is in-flight"
            );
        }

        let _ = release_tx.send(());
        finish_handle.join().expect("finish thread joins");

        let final_log = log.lock().unwrap_or_else(PoisonError::into_inner).clone();
        assert_eq!(
            final_log.last(),
            Some(&"closed"),
            "nothing is sent after close: {final_log:?}"
        );
        assert!(
            final_log.len() >= 2
                && final_log[..final_log.len() - 1]
                    .iter()
                    .all(|entry| *entry == "sent"),
            "the in-flight send finishes before close: {final_log:?}"
        );
    }

    #[test]
    fn test_heartbeat_one_hour_interval_returns_fast() {
        let sink = Arc::new(|_status: &ImporterStatus| {});
        let on_close = Arc::new(|| {});
        let on_worker_exit = Arc::new(|| {});

        let start = Instant::now();
        let mut hb = ImportHeartbeat::start_injected(
            Some(Duration::from_secs(3600)),
            sink.clone(),
            default_spawner,
            on_close.clone(),
            on_worker_exit.clone(),
            "imp_3".to_owned(),
            1,
            "att_3".to_owned(),
            "execution".to_owned(),
            Instant::now(),
        );
        hb.finish();

        let hb2 = ImportHeartbeat::start_injected(
            Some(Duration::from_secs(3600)),
            sink,
            default_spawner,
            on_close,
            on_worker_exit,
            "imp_3".to_owned(),
            1,
            "att_3".to_owned(),
            "execution".to_owned(),
            Instant::now(),
        );
        drop(hb2);

        assert!(
            start.elapsed() < Duration::from_secs(1),
            "finish and drop should return well within 1 second"
        );
    }

    #[test]
    fn test_heartbeat_drop_stops_and_joins() {
        let (tx, rx) = mpsc::channel();
        let sink = Arc::new(|_status: &ImporterStatus| {});
        let on_close = Arc::new(|| {});
        let on_worker_exit = Arc::new(move || {
            let _ = tx.send(());
        });

        let hb = ImportHeartbeat::start_injected(
            Some(Duration::from_millis(20)),
            sink,
            default_spawner,
            on_close,
            on_worker_exit,
            "imp_4".to_owned(),
            1,
            "att_4".to_owned(),
            "execution".to_owned(),
            Instant::now(),
        );
        drop(hb);

        rx.recv_timeout(Duration::from_secs(1))
            .expect("worker exit signal received within 1 second");
    }

    #[test]
    fn test_heartbeat_panicking_sink_does_not_hang_finish() {
        let sink = Arc::new(|_status: &ImporterStatus| {
            panic!("forced sink panic");
        });
        let on_close = Arc::new(|| {});
        let on_worker_exit = Arc::new(|| {});

        let mut hb = ImportHeartbeat::start_injected(
            Some(Duration::from_millis(10)),
            sink,
            default_spawner,
            on_close,
            on_worker_exit,
            "imp_5".to_owned(),
            1,
            "att_5".to_owned(),
            "execution".to_owned(),
            Instant::now(),
        );

        std::thread::sleep(Duration::from_millis(30));
        let start = Instant::now();
        hb.finish();
        assert!(
            start.elapsed() < Duration::from_secs(1),
            "finish should return quickly despite panicking sink"
        );
    }

    #[test]
    fn test_heartbeat_injected_spawn_failure() {
        let sent = Arc::new(Mutex::new(false));
        let sent_clone = Arc::clone(&sent);
        let sink = Arc::new(move |_status: &ImporterStatus| {
            *sent_clone.lock().unwrap_or_else(PoisonError::into_inner) = true;
        });
        let on_close = Arc::new(|| {});
        let on_worker_exit = Arc::new(|| {});

        fn failing_spawner(
            _builder: Builder,
            _task: Box<dyn FnOnce() + Send>,
        ) -> io::Result<JoinHandle<()>> {
            Err(io::Error::other("spawn failed"))
        }

        let mut hb = ImportHeartbeat::start_injected(
            Some(Duration::from_millis(10)),
            sink,
            failing_spawner,
            on_close,
            on_worker_exit,
            "imp_6".to_owned(),
            1,
            "att_6".to_owned(),
            "execution".to_owned(),
            Instant::now(),
        );

        hb.finish();
        assert!(
            !*sent.lock().unwrap_or_else(PoisonError::into_inner),
            "sink must not be called when spawn fails"
        );
    }
}
