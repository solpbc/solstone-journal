// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::path::Path;

#[cfg(windows)]
#[doc(hidden)]
pub struct WindowsPipeNamespace {
    _guard: crate::windows::namespace_fixture::Guard,
}

#[cfg(windows)]
impl WindowsPipeNamespace {
    pub fn register(socket: impl AsRef<Path>) -> Self {
        Self {
            _guard: crate::windows::namespace_fixture::Guard::register(socket.as_ref()),
        }
    }
}

/// Disposable one-shot receiver using the production Windows pipe admission.
/// Only the installation namespace is replaced, for this exact socket path.
/// A separate runtime thread lets synchronous senders complete their handshake.
#[doc(hidden)]
pub struct OneShotListener {
    socket: std::path::PathBuf,
    messages: std::sync::mpsc::Receiver<Result<serde_json::Value, String>>,
    stop: Option<tokio::sync::oneshot::Sender<()>>,
    done: std::sync::mpsc::Receiver<Result<(), String>>,
    worker: Option<std::thread::JoinHandle<()>>,
    #[cfg(unix)]
    bound: std::sync::Arc<std::sync::atomic::AtomicBool>,
    #[cfg(windows)]
    _namespace: crate::windows::namespace_fixture::Guard,
}

impl OneShotListener {
    pub fn bind(socket: impl AsRef<Path>) -> Self {
        let socket = socket.as_ref().to_path_buf();
        std::fs::create_dir_all(socket.parent().expect("socket parent"))
            .expect("create fixture socket parent");
        #[cfg(windows)]
        let namespace = crate::windows::namespace_fixture::Guard::register(&socket);
        #[cfg(windows)]
        assert!(
            !crate::windows::secret_path(&socket)
                .expect("fixture secret path")
                .exists(),
            "fixture requires an unused secret path"
        );
        let (ready_tx, ready) = std::sync::mpsc::sync_channel(1);
        let (message_tx, messages) = std::sync::mpsc::sync_channel(16);
        let (done_tx, done) = std::sync::mpsc::sync_channel(1);
        let (stop_tx, mut stop_rx) = tokio::sync::oneshot::channel();
        let thread_socket = socket.clone();
        #[cfg(unix)]
        let bound = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        #[cfg(unix)]
        let thread_bound = bound.clone();
        let worker = std::thread::spawn(move || {
            let result = (|| -> Result<(), String> {
                let runtime = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .map_err(|error| error.to_string())?;
                runtime.block_on(async {
                    #[cfg(unix)]
                    let listener = tokio::net::UnixListener::bind(&thread_socket)
                        .map_err(|error| error.to_string())?;
                    #[cfg(windows)]
                    let (listener, secret) =
                        super::server::bind_windows_listener(&thread_socket)
                            .map_err(|error| error.to_string())?;
                    #[cfg(windows)]
                    use interprocess::local_socket::traits::tokio::Listener as _;
                    #[cfg(unix)]
                    thread_bound.store(true, std::sync::atomic::Ordering::Release);
                    ready_tx.send(Ok(())).map_err(|error| error.to_string())?;
                    loop {
                        let accepted = tokio::select! {
                            biased;
                            accepted = listener.accept() => accepted,
                            _ = &mut stop_rx => return Ok(()),
                        };
                        #[cfg(unix)]
                        let (stream, _) = accepted.map_err(|error| error.to_string())?;
                        #[cfg(windows)]
                        let stream = accepted.map_err(|error| error.to_string())?;
                        let read =
                            tokio::time::timeout(std::time::Duration::from_secs(5), async {
                                #[cfg(windows)]
                                let stream = {
                                    use interprocess::local_socket::traits::StreamCommon as _;
                                    let mut stream = stream;
                                    let pid =
                                        stream.peer_creds().ok().and_then(|creds| creds.pid());
                                    if !super::server::authenticate_windows_peer(
                                        &mut stream,
                                        &secret,
                                        pid,
                                    )
                                    .await
                                    {
                                        return Err(
                                            "fixture rejected unauthenticated peer".to_owned()
                                        );
                                    }
                                    stream
                                };
                                use tokio::io::AsyncReadExt as _;
                                let mut bytes = Vec::new();
                                stream
                                    .take(65_537)
                                    .read_to_end(&mut bytes)
                                    .await
                                    .map_err(|error| error.to_string())?;
                                if bytes.len() > 65_536 {
                                    return Err("fixture frame exceeds limit".to_owned());
                                }
                                serde_json::from_slice(&bytes)
                                    .map_err(|error| error.to_string())
                            });
                        let received = tokio::select! {
                            biased;
                            received = read => received.map_err(|_| "fixture read timed out".to_owned())?,
                            _ = &mut stop_rx => return Err("fixture stopped with an in-flight peer".to_owned()),
                        };
                        message_tx
                            .try_send(received)
                            .map_err(|error| error.to_string())?;
                    }
                })
            })();
            if let Err(error) = &result {
                let _ = ready_tx.try_send(Err(error.clone()));
                let _ = message_tx.try_send(Err(error.clone()));
            }
            let _ = done_tx.send(result);
        });
        let fixture = Self {
            socket,
            messages,
            stop: Some(stop_tx),
            done,
            worker: Some(worker),
            #[cfg(unix)]
            bound,
            #[cfg(windows)]
            _namespace: namespace,
        };
        ready
            .recv_timeout(std::time::Duration::from_secs(10))
            .expect("fixture bind completes")
            .expect("fixture listener binds");
        fixture
    }

    /// Read the exact expected count, then stop and join before checking extras.
    pub fn finish(mut self, expected: usize) -> Vec<serde_json::Value> {
        let messages = (0..expected)
            .map(|_| {
                self.messages
                    .recv_timeout(std::time::Duration::from_secs(10))
                    .expect("fixture notification arrives")
                    .expect("fixture receives valid JSON")
            })
            .collect();
        self.shutdown().expect("fixture shuts down cleanly");
        assert!(
            self.messages.try_recv().is_err(),
            "unexpected fixture notification"
        );
        messages
    }

    fn shutdown(&mut self) -> Result<(), String> {
        let Some(worker) = self.worker.take() else {
            return Ok(());
        };
        if let Some(stop) = self.stop.take() {
            let _ = stop.send(());
        }
        // Every native handshake and read is bounded; never join a live worker blind.
        let completion = self
            .done
            .recv_timeout(std::time::Duration::from_secs(10))
            .map_err(|error| format!("fixture worker did not stop: {error}"));
        let joined = if completion.is_ok() {
            worker
                .join()
                .map_err(|_| "fixture worker panicked".to_owned())
        } else {
            Err("fixture join refused without completion".to_owned())
        };
        #[cfg(unix)]
        if self.bound.load(std::sync::atomic::Ordering::Acquire) {
            std::fs::remove_file(&self.socket).map_err(|error| error.to_string())?;
        }
        #[cfg(windows)]
        if let Ok(secret) = crate::windows::secret_path(&self.socket) {
            match std::fs::remove_file(secret) {
                Ok(()) => {}
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => return Err(format!("remove fixture secret: {error}")),
            }
        }
        completion.and_then(|result| result).and(joined)
    }
}

impl Drop for OneShotListener {
    fn drop(&mut self) {
        if let Err(error) = self.shutdown()
            && !std::thread::panicking()
        {
            panic!("{error}");
        }
    }
}

#[cfg(all(test, feature = "full-tests"))]
mod windows_native_tests {
    use super::OneShotListener;
    use crate::CallosumOneShotSender;
    use std::time::Duration;

    #[test]
    fn one_shot_fixture_duplicate_bind_preserves_the_first_listener() {
        let root = tempfile::tempdir().unwrap();
        let socket = root.path().join("health/callosum.sock");
        let listener = OneShotListener::bind(&socket);
        assert!(std::panic::catch_unwind(|| OneShotListener::bind(&socket)).is_err());
        CallosumOneShotSender::new(&socket, Duration::from_secs(2))
            .send_line("{\"event\":\"original\"}\n")
            .unwrap();
        assert_eq!(
            listener.finish(1),
            [serde_json::json!({"event": "original"})]
        );
    }

    #[test]
    fn one_shot_fixture_rejects_extra_events_and_releases_its_path() {
        let root = tempfile::tempdir().unwrap();
        let socket = root.path().join("health/callosum.sock");
        let listener = OneShotListener::bind(&socket);
        let sender = CallosumOneShotSender::new(&socket, Duration::from_secs(2));
        sender.send_line("{\"event\":\"one\"}\n").unwrap();
        sender.send_line("{\"event\":\"extra\"}\n").unwrap();
        assert!(
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| listener.finish(1))).is_err()
        );
        OneShotListener::bind(&socket).finish(0);
    }

    #[test]
    fn one_shot_fixture_releases_its_path_on_unwind_and_isolates_neighbors() {
        let root = tempfile::tempdir().unwrap();
        let socket = root.path().join("health/callosum.sock");
        let neighbor = root.path().join("other/health/callosum.sock");
        assert!(
            std::panic::catch_unwind(|| {
                let _listener = OneShotListener::bind(&socket);
                assert!(
                    CallosumOneShotSender::new(&neighbor, Duration::from_millis(50))
                        .send_line("{}\n")
                        .is_err()
                );
                panic!("fixture assertion fails");
            })
            .is_err()
        );
        OneShotListener::bind(&socket).finish(0);
    }
}
