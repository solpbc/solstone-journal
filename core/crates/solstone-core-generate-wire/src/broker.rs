// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! One confidential connection per journal.
//!
//! The supervisor holds the journal's attested confidential channel and makes
//! every confidential call through it. A Generate process elsewhere, which has
//! already taken its turn in the journal's call line, hands the call to the
//! supervisor over a private local socket instead of attesting a channel of its
//! own. Without the supervisor's socket a call connects directly, as before.
//!
//! On Mac and Linux the channel is a socket in the journal's `health/`
//! directory, readable and writable by the owner's account alone. On Windows it
//! is a named pipe named for that same path, which admits the owner's account
//! alone and a caller that answers the journal's pipe secret. Each connection
//! carries one call: a length-framed request and its priority one way, the
//! framed response the other. A caller that goes away, or cancels, cancels
//! its call.

use std::path::{Path, PathBuf};

/// The socket a journal's supervisor serves confidential calls on.
pub fn broker_socket_path(journal: &Path) -> PathBuf {
    journal.join("health/generate.sock")
}

/// The largest frame either side accepts: a Generate request's own ceiling
/// plus room for its envelope.
#[cfg(any(unix, windows))]
const MAX_FRAME_BYTES: u32 = 65 * 1024 * 1024;

#[cfg(any(unix, windows))]
static SERVING: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

#[cfg(all(test, any(unix, windows)))]
static SERVED: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

/// Whether this process serves confidential calls itself, so its own calls
/// use its channel directly instead of reaching it through the socket.
#[cfg(any(unix, windows))]
pub(crate) fn serving_here() -> bool {
    SERVING.load(std::sync::atomic::Ordering::Acquire)
}

/// The platform's private channel: how the supervisor listens and how a caller
/// reaches it. Everything above it is the same on every platform.
#[cfg(unix)]
mod channel {
    use std::io;
    use std::path::{Path, PathBuf};

    pub(super) type Reader = std::os::unix::net::UnixStream;
    pub(super) type Writer = std::os::unix::net::UnixStream;

    pub(super) struct Listener {
        listener: std::os::unix::net::UnixListener,
        path: PathBuf,
        /// The bound socket's identity, so a stop never removes another's.
        identity: (u64, u64),
    }

    pub(super) struct Pending(std::os::unix::net::UnixStream);

    impl Listener {
        pub(super) fn bind(path: &Path) -> io::Result<Self> {
            use std::os::unix::fs::{MetadataExt, PermissionsExt};
            let health = path.parent().expect("socket has a parent");
            std::fs::create_dir_all(health)?;
            // Bind in a private directory and move the socket into place only
            // once its mode admits the owner alone, so it is never reachable
            // more widely.
            static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
            let staging = health.join(format!(
                ".generate-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
            ));
            // One left by a supervisor that crashed with this process id is ours.
            let _ = std::fs::remove_dir_all(&staging);
            std::fs::create_dir(&staging)?;
            let staged = staging.join("s");
            let bound = (|| {
                std::fs::set_permissions(&staging, std::fs::Permissions::from_mode(0o700))?;
                let listener = std::os::unix::net::UnixListener::bind(&staged)?;
                std::fs::set_permissions(&staged, std::fs::Permissions::from_mode(0o600))?;
                // A socket left by a supervisor that exited is replaced.
                std::fs::rename(&staged, path)?;
                Ok::<_, io::Error>(listener)
            })();
            let _ = std::fs::remove_file(&staged);
            let _ = std::fs::remove_dir(&staging);
            let listener = bound?;
            let metadata = std::fs::symlink_metadata(path)?;
            // Polled, so a stop needs no connection to wake it.
            listener.set_nonblocking(true)?;
            Ok(Self {
                listener,
                path: path.to_path_buf(),
                identity: (metadata.dev(), metadata.ino()),
            })
        }

        pub(super) fn accept(&self) -> io::Result<Option<Pending>> {
            match self.listener.accept() {
                Ok((stream, _)) => Ok(Some(Pending(stream))),
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => Ok(None),
                Err(error) => Err(error),
            }
        }
    }

    impl Drop for Listener {
        fn drop(&mut self) {
            use std::os::unix::fs::MetadataExt;
            if std::fs::symlink_metadata(&self.path)
                .is_ok_and(|metadata| (metadata.dev(), metadata.ino()) == self.identity)
            {
                let _ = std::fs::remove_file(&self.path);
            }
        }
    }

    impl Pending {
        /// The caller's two directions. Its first frame must come within a
        /// minute; `call_read` then lifts the limit for the watch that follows.
        pub(super) fn open(self) -> io::Result<(Reader, Writer)> {
            self.0.set_nonblocking(false)?;
            self.0
                .set_read_timeout(Some(std::time::Duration::from_secs(60)))?;
            Ok((self.0.try_clone()?, self.0))
        }
    }

    pub(super) fn call_read(reader: &Reader) -> io::Result<()> {
        reader.set_read_timeout(None)
    }

    /// A caller's two directions; `None` when no supervisor serves this journal.
    pub(super) fn connect(path: &Path) -> Option<(Reader, Writer)> {
        let stream = std::os::unix::net::UnixStream::connect(path).ok()?;
        Some((stream.try_clone().ok()?, stream))
    }

    /// Leave a call from another thread: the supervisor sees the caller go,
    /// and the caller's own read returns at once.
    pub(super) fn abandon(writer: &mut Writer) {
        let _ = writer.shutdown(std::net::Shutdown::Both);
    }
}

#[cfg(windows)]
mod channel {
    use std::io::{self, Write};
    use std::path::Path;
    use std::time::Duration;

    pub(super) type Reader = Box<dyn io::Read + Send>;
    pub(super) type Writer = Box<dyn io::Write + Send>;

    pub(super) struct Listener(solstone_core_callosum::private_pipe::PrivatePipeListener);

    pub(super) struct Pending(solstone_core_callosum::private_pipe::PendingCaller);

    impl Listener {
        pub(super) fn bind(path: &Path) -> io::Result<Self> {
            std::fs::create_dir_all(path.parent().expect("pipe path has a parent"))?;
            solstone_core_callosum::private_pipe::PrivatePipeListener::bind(path).map(Self)
        }

        pub(super) fn accept(&self) -> io::Result<Option<Pending>> {
            Ok(self.0.accept()?.map(Pending))
        }
    }

    impl Pending {
        /// Challenge the caller, then its two directions.
        pub(super) fn open(self) -> io::Result<(Reader, Writer)> {
            let (reader, writer) =
                solstone_core_callosum::private_pipe::split_private_pipe(self.0.admit()?);
            Ok((Box::new(reader), Box::new(writer)))
        }
    }

    pub(super) fn call_read(_reader: &Reader) -> io::Result<()> {
        Ok(())
    }

    /// A caller's two directions; `None` when no supervisor serves this
    /// journal or none answered in time.
    pub(super) fn connect(path: &Path) -> Option<(Reader, Writer)> {
        let stream = solstone_core_callosum::private_pipe::connect_private_pipe(
            path,
            Duration::from_secs(2),
        )
        .ok()??;
        let (reader, writer) = solstone_core_callosum::private_pipe::split_private_pipe(stream);
        Some((Box::new(reader), Box::new(writer)))
    }

    /// Leave a call from another thread. A pipe cannot be shut down across
    /// threads, so the caller sends one byte, which the supervisor reads as the
    /// caller gone; the caller then reads the supervisor's cancelled answer.
    pub(super) fn abandon(writer: &mut Writer) {
        let _ = writer.write_all(&[0]).and_then(|()| writer.flush());
    }
}

/// A running confidential-call server. Dropping it stops accepting calls and
/// removes its socket; calls already accepted finish.
#[cfg(any(unix, windows))]
pub struct ConfidentialBroker {
    stop: std::sync::Arc<std::sync::atomic::AtomicBool>,
    accept: Option<std::thread::JoinHandle<()>>,
}

#[cfg(any(unix, windows))]
impl ConfidentialBroker {
    /// Serve the journal's confidential calls from this process.
    ///
    /// # Errors
    /// Returns the bind error when the channel cannot be created; callers then
    /// keep connecting directly.
    pub fn start(journal: &Path) -> std::io::Result<Self> {
        let listener = channel::Listener::bind(&broker_socket_path(journal))?;
        let journal = journal.to_path_buf();
        let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let accept = {
            let stop = std::sync::Arc::clone(&stop);
            std::thread::Builder::new()
                .name("confidential-broker".into())
                .spawn(move || accept_calls(&listener, &journal, &stop))?
        };
        SERVING.store(true, std::sync::atomic::Ordering::Release);
        log::info!("confidential calls: one connection, served by this process");
        Ok(Self {
            stop,
            accept: Some(accept),
        })
    }
}

#[cfg(any(unix, windows))]
impl Drop for ConfidentialBroker {
    fn drop(&mut self) {
        SERVING.store(false, std::sync::atomic::Ordering::Release);
        self.stop.store(true, std::sync::atomic::Ordering::Release);
        // The accept loop owns the listener; joining it closes the channel.
        if let Some(accept) = self.accept.take() {
            let _ = accept.join();
        }
    }
}

/// Calls one broker serves at once.
#[cfg(any(unix, windows))]
const MAX_SERVED_CALLS: usize = 4;

#[cfg(any(unix, windows))]
static SERVING_CALLS: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

#[cfg(any(unix, windows))]
fn accept_calls(
    listener: &channel::Listener,
    journal: &Path,
    stop: &std::sync::atomic::AtomicBool,
) {
    while !stop.load(std::sync::atomic::Ordering::Acquire) {
        let pending = match listener.accept() {
            Ok(Some(pending)) => pending,
            Ok(None) | Err(_) => {
                std::thread::sleep(std::time::Duration::from_millis(20));
                continue;
            }
        };
        // The handshake and the call's first frame wait on the caller, so they
        // run off this loop; a caller that stalls there holds only its thread.
        let journal = journal.to_path_buf();
        let spawned = std::thread::Builder::new()
            .name("confidential-call".into())
            .spawn(move || {
                if let Ok((reader, writer)) = pending.open() {
                    serve_call(reader, writer, &journal);
                }
            });
        if let Err(error) = spawned {
            log::warn!("confidential calls: could not serve a call: {error}");
        }
    }
}

#[cfg(any(unix, windows))]
fn serve_call(mut reader: channel::Reader, mut writer: channel::Writer, journal: &Path) {
    let (priority, request) = match read_frame(&mut reader).and_then(|frame| decode_call(&frame)) {
        Ok(call) => call,
        Err(error) => {
            log::warn!("confidential calls: refused an unreadable call: {error}");
            let _ = write_frame(
                &mut writer,
                format!("ecall was unreadable: {error}").as_bytes(),
            );
            return;
        }
    };
    if channel::call_read(&reader).is_err() {
        return;
    }
    // Callers take turns in the call line, so only a caller that left mid-call
    // overlaps the next; more than a few at once is a fault. The call has been
    // read, so its caller is waiting for this answer.
    if SERVING_CALLS.fetch_add(1, std::sync::atomic::Ordering::AcqRel) >= MAX_SERVED_CALLS {
        SERVING_CALLS.fetch_sub(1, std::sync::atomic::Ordering::AcqRel);
        let _ = write_frame(&mut writer, b"econfidential calls are busy");
        return;
    }
    struct Served;
    impl Drop for Served {
        fn drop(&mut self) {
            SERVING_CALLS.fetch_sub(1, std::sync::atomic::Ordering::AcqRel);
        }
    }
    let _served = Served;
    // The caller writes nothing more, so a read returns only when it leaves.
    let gone = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    {
        let gone = std::sync::Arc::clone(&gone);
        let _ = std::thread::Builder::new()
            .name("confidential-call-watch".into())
            .spawn(move || {
                let mut byte = [0u8; 1];
                let _ = std::io::Read::read(&mut reader, &mut byte);
                gone.store(true, std::sync::atomic::Ordering::Release);
            });
    }
    #[cfg(test)]
    SERVED.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let cancelled = || gone.load(std::sync::atomic::Ordering::Acquire);
    let cancellation = solstone_core_generate::Cancellation::from_fn(&cancelled);
    let response = crate::dispatch::respond_in_held_turn(
        &request,
        &crate::EndpointRuntime::default(),
        journal,
        &crate::GenerateCallContext {
            priority,
            cancellation: &cancellation,
        },
    );
    let encoded = match response {
        Ok(response) => solstone_core_generate::encode_one_shot_response(&response),
        Err(detail) => Err(detail),
    };
    match encoded {
        Ok(encoded) => {
            let _ = write_frame(&mut writer, format!("r{encoded}").as_bytes());
        }
        Err(detail) => {
            let _ = write_frame(&mut writer, format!("e{detail}").as_bytes());
        }
    }
}

/// Hand a confidential call to the journal's supervisor. `None` means no
/// supervisor serves this journal here, and the caller connects directly.
///
/// # Errors
/// Returns an error when the supervisor took the call but no response came
/// back; the call is not repeated, since it may already have reached the engine.
#[cfg(any(unix, windows))]
pub(crate) fn forward(
    journal: &Path,
    request: &solstone_core_generate::GenerateRequest,
    context: &crate::GenerateCallContext<'_>,
) -> Option<Result<solstone_core_generate::GenerateResponse, String>> {
    if serving_here() {
        return None;
    }
    forward_to_supervisor(journal, request, context)
}

#[cfg(any(unix, windows))]
fn forward_to_supervisor(
    journal: &Path,
    request: &solstone_core_generate::GenerateRequest,
    context: &crate::GenerateCallContext<'_>,
) -> Option<Result<solstone_core_generate::GenerateResponse, String>> {
    let (mut reader, mut writer) = channel::connect(&broker_socket_path(journal))?;
    let call = match encode_call(context.priority, request) {
        Ok(call) => call,
        Err(detail) => return Some(Err(detail)),
    };
    if write_frame(&mut writer, &call).is_err() {
        // Nothing reached the supervisor, so the caller may connect directly.
        return None;
    }
    let done = std::sync::atomic::AtomicBool::new(false);
    let reply = std::thread::scope(|scope| {
        let done = &done;
        let writer = &mut writer;
        scope.spawn(move || {
            while !done.load(std::sync::atomic::Ordering::Acquire) {
                if context.cancellation.is_cancelled() {
                    channel::abandon(writer);
                    return;
                }
                std::thread::sleep(std::time::Duration::from_millis(50));
            }
        });
        let reply = read_frame(&mut reader);
        done.store(true, std::sync::atomic::Ordering::Release);
        reply
    });
    drop(writer);
    if context.cancellation.is_cancelled() {
        return Some(Ok(solstone_core_generate::GenerateResponse::Cancelled));
    }
    Some(match reply {
        Ok(frame) => decode_reply(&frame),
        Err(error) => Err(format!("confidential call was not answered: {error}")),
    })
}

#[cfg(not(any(unix, windows)))]
pub(crate) fn forward(
    _journal: &Path,
    _request: &solstone_core_generate::GenerateRequest,
    _context: &crate::GenerateCallContext<'_>,
) -> Option<Result<solstone_core_generate::GenerateResponse, String>> {
    None
}

#[cfg(any(unix, windows))]
fn encode_call(
    priority: solstone_core_journal_config::work_priority::WorkPriority,
    request: &solstone_core_generate::GenerateRequest,
) -> Result<Vec<u8>, String> {
    let request = solstone_core_generate::encode_one_shot_request(request)?;
    Ok(format!("{priority}\n{request}").into_bytes())
}

#[cfg(any(unix, windows))]
fn decode_call(
    frame: &[u8],
) -> std::io::Result<(
    solstone_core_journal_config::work_priority::WorkPriority,
    solstone_core_generate::GenerateRequest,
)> {
    let invalid = |detail: String| std::io::Error::new(std::io::ErrorKind::InvalidData, detail);
    let text = std::str::from_utf8(frame).map_err(|error| invalid(error.to_string()))?;
    let (priority, request) = text
        .split_once('\n')
        .ok_or_else(|| invalid("call has no priority".into()))?;
    let priority = priority
        .parse()
        .map_err(|_| invalid("call priority is invalid".into()))?;
    let request = solstone_core_generate::decode_one_shot_request(request).map_err(invalid)?;
    Ok((priority, request))
}

#[cfg(any(unix, windows))]
fn decode_reply(frame: &[u8]) -> Result<solstone_core_generate::GenerateResponse, String> {
    let text = std::str::from_utf8(frame).map_err(|error| error.to_string())?;
    match text.split_at_checked(1) {
        Some(("r", response)) => solstone_core_generate::decode_one_shot_response(response),
        Some(("e", detail)) => Err(detail.to_owned()),
        _ => Err("confidential call reply is invalid".into()),
    }
}

#[cfg(any(unix, windows))]
fn write_frame(stream: &mut impl std::io::Write, body: &[u8]) -> std::io::Result<()> {
    let length = u32::try_from(body.len())
        .ok()
        .filter(|length| *length <= MAX_FRAME_BYTES)
        .ok_or_else(|| std::io::Error::new(std::io::ErrorKind::InvalidInput, "frame too large"))?;
    stream.write_all(&length.to_be_bytes())?;
    stream.write_all(body)?;
    stream.flush()
}

#[cfg(any(unix, windows))]
fn read_frame(stream: &mut impl std::io::Read) -> std::io::Result<Vec<u8>> {
    let mut length = [0u8; 4];
    stream.read_exact(&mut length)?;
    let length = u32::from_be_bytes(length);
    if length > MAX_FRAME_BYTES {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "frame too large",
        ));
    }
    let mut body = vec![0u8; length as usize];
    stream.read_exact(&mut body)?;
    Ok(body)
}

#[cfg(all(test, any(unix, windows)))]
mod tests {
    use super::*;

    /// Brokers in one test process share its serving flag; take turns.
    static SERIAL: std::sync::Mutex<()> = std::sync::Mutex::new(());

    #[test]
    fn frames_round_trip_and_refuse_oversize_lengths() {
        let mut wire = Vec::new();
        write_frame(&mut wire, b"hello").unwrap();
        assert_eq!(read_frame(&mut wire.as_slice()).unwrap(), b"hello");
        let mut oversize = (MAX_FRAME_BYTES + 1).to_be_bytes().to_vec();
        oversize.extend_from_slice(b"x");
        assert!(read_frame(&mut oversize.as_slice()).is_err());
    }

    #[test]
    fn a_handed_over_call_is_answered_by_the_supervisor_as_it_would_answer_directly() {
        let _serial = SERIAL
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let journal = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(journal.path().join("config")).unwrap();
        // Windows names the pipe for the journal's installation; a test names
        // it as Callosum's native tests do.
        #[cfg(windows)]
        let _namespace = {
            std::fs::create_dir_all(journal.path().join("health")).unwrap();
            solstone_core_callosum::private_pipe::test_pipe_namespace(&broker_socket_path(
                journal.path(),
            ))
        };
        // A confidential engine nothing serves: the call fails the same way
        // wherever it runs, so only who answered it differs.
        let config = serde_json::json!({
            "providers": {"active": {"provider": "local"}, "local": {
                "endpoint_url": "https://127.0.0.1:9", "served_model_id": "model", "credential": "test"
            }},
            "services": {"confidential": {
                "endpoint_url": "https://127.0.0.1:9", "served_model_id": "model",
                "credential_fingerprint_sha256":
                    "9f86d081884c7d659a2feaa0c55ad015a3bf4f1b2b0b822cd15d6c15b0f00a08"
            }}
        });
        std::fs::write(
            journal.path().join("config/journal.json"),
            config.to_string(),
        )
        .unwrap();
        let request = solstone_core_generate::GenerateRequest {
            id: Some("request".into()),
            context: "test.broker".into(),
            contents: vec![solstone_core_generate::ContentPart::Text {
                text: "hello".into(),
            }],
            system_instruction: None,
            temperature: 0.0,
            max_output_tokens: 8,
            timeout_s: Some(5.0),
            json_output: false,
            json_schema: None,
            attempt_index: 0,
            exclusive_admission: false,
            overflow_text: None,
            transport_retries: None,
        };
        let context = crate::GenerateCallContext {
            priority: "5:1".parse().unwrap(),
            cancellation: &solstone_core_generate::Cancellation::never(),
        };
        assert!(
            forward_to_supervisor(journal.path(), &request, &context).is_none(),
            "with no supervisor here a call connects directly"
        );
        let broker = ConfidentialBroker::start(journal.path()).unwrap();
        let served = SERVED.load(std::sync::atomic::Ordering::Relaxed);
        // The caller holds its turn, as dispatch does: the supervisor must
        // serve the call without waiting in the line again.
        let turn = solstone_core_local::admission::wait_for_model_call(
            journal.path(),
            context.priority,
            &|| false,
        )
        .unwrap();
        let (sent, answered) = std::sync::mpsc::channel();
        std::thread::scope(|scope| {
            scope.spawn(|| {
                let _ = sent.send(forward_to_supervisor(journal.path(), &request, &context));
            });
            assert!(
                answered
                    .recv_timeout(std::time::Duration::from_secs(30))
                    .is_ok(),
                "a handed-over call must not wait behind its own caller's turn"
            );
        });
        drop(turn);
        let forwarded = forward_to_supervisor(journal.path(), &request, &context)
            .expect("the supervisor takes the call")
            .expect("the supervisor answers");
        assert_eq!(
            SERVED.load(std::sync::atomic::Ordering::Relaxed),
            served + 2
        );
        let direct = crate::dispatch::respond_in_held_turn(
            &request,
            &crate::EndpointRuntime::default(),
            journal.path(),
            &context,
        )
        .unwrap();
        assert_eq!(
            solstone_core_generate::encode_one_shot_response(&forwarded).unwrap(),
            solstone_core_generate::encode_one_shot_response(&direct).unwrap()
        );
        drop(broker);
    }

    #[test]
    fn a_full_broker_reads_the_call_then_answers_busy() {
        let _serial = SERIAL
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let journal = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(journal.path().join("health")).unwrap();
        #[cfg(windows)]
        let _namespace = solstone_core_callosum::private_pipe::test_pipe_namespace(
            &broker_socket_path(journal.path()),
        );
        let broker = ConfidentialBroker::start(journal.path()).unwrap();
        SERVING_CALLS.fetch_add(MAX_SERVED_CALLS, std::sync::atomic::Ordering::AcqRel);
        let request = solstone_core_generate::GenerateRequest {
            id: Some("busy".into()),
            context: "test.broker".into(),
            contents: vec![solstone_core_generate::ContentPart::Text {
                text: "hello".into(),
            }],
            system_instruction: None,
            temperature: 0.0,
            max_output_tokens: 8,
            timeout_s: Some(5.0),
            json_output: false,
            json_schema: None,
            attempt_index: 0,
            exclusive_admission: false,
            overflow_text: None,
            transport_retries: None,
        };
        let context = crate::GenerateCallContext {
            priority: "5:1".parse().unwrap(),
            cancellation: &solstone_core_generate::Cancellation::never(),
        };
        // The answer comes back at once, neither end waiting on the other.
        let (sent, answered) = std::sync::mpsc::channel();
        std::thread::scope(|scope| {
            scope.spawn(|| {
                let _ = sent.send(forward_to_supervisor(journal.path(), &request, &context));
            });
            let answer = answered
                .recv_timeout(std::time::Duration::from_secs(30))
                .expect("a busy broker answers");
            assert!(matches!(answer, Some(Err(detail)) if detail.contains("busy")));
        });
        SERVING_CALLS.fetch_sub(MAX_SERVED_CALLS, std::sync::atomic::Ordering::AcqRel);
        drop(broker);
    }

    #[cfg(unix)]
    #[test]
    fn the_socket_admits_the_owner_alone_and_leaves_with_the_server() {
        let _serial = SERIAL
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        use std::os::unix::fs::PermissionsExt;
        let journal = tempfile::tempdir().unwrap();
        let broker = ConfidentialBroker::start(journal.path()).unwrap();
        let path = broker_socket_path(journal.path());
        let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
        assert!(serving_here());
        drop(broker);
        assert!(!path.exists());
        assert!(!serving_here());
        // Nothing of the private staging directory remains.
        let leftovers = std::fs::read_dir(journal.path().join("health"))
            .unwrap()
            .filter_map(Result::ok)
            .filter(|entry| {
                entry
                    .file_name()
                    .to_string_lossy()
                    .starts_with(".generate-")
            })
            .count();
        assert_eq!(leftovers, 0);
    }
}
