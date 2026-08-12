// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::error::Error;
use std::io::{self, ErrorKind, Write};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::thread;
use std::time::{Duration, Instant};

use serde_json::{Map, Value, json};
use socket2::{Domain, SockAddr, Socket, Type};
use solstone_core_callosum::CallosumEnvelope;
use solstone_core_ingest_resolve::{IngestNotice, IngestNotifier};

const NOTIFY_BUDGET: Duration = Duration::from_secs(1);
const RETRY_DELAY: Duration = Duration::from_millis(10);

/// Sends accepted observer-ingest facts to the local Callosum socket.
pub struct CallosumIngestNotifier {
    socket_path: PathBuf,
}

impl CallosumIngestNotifier {
    pub fn new(journal_root: impl AsRef<Path>) -> Self {
        Self {
            socket_path: journal_root.as_ref().join("health/callosum.sock"),
        }
    }

    fn send(&self, notice: &IngestNotice<'_>) -> Result<(), Box<dyn Error + Send + Sync>> {
        let mut extra = Map::new();
        extra.insert("day".to_owned(), json!(notice.day));
        extra.insert("segment".to_owned(), json!(notice.segment));
        extra.insert("stream".to_owned(), json!(notice.stream));
        extra.insert("observer".to_owned(), json!(notice.did));
        extra.insert(
            "files".to_owned(),
            Value::Array(
                notice
                    .files
                    .iter()
                    .map(|file| Value::String(file.name.as_str().to_owned()))
                    .collect(),
            ),
        );
        let envelope = CallosumEnvelope {
            tract: "observe".to_owned(),
            event: "observing".to_owned(),
            ts: None,
            extra,
        };
        let mut line = serde_json::to_string(&envelope)?;
        line.push('\n');
        send_line_with_budget(&self.socket_path, &line).map_err(Into::into)
    }
}

impl IngestNotifier for CallosumIngestNotifier {
    fn notify(&self, notice: &IngestNotice<'_>) -> Result<(), Box<dyn Error + Send + Sync>> {
        match self.send(notice) {
            Ok(()) => Ok(()),
            Err(error) => {
                eprintln!("convey: observer ingest Callosum notify failed: {error}");
                Err(error)
            }
        }
    }
}

fn send_line_with_budget(socket_path: &Path, line: &str) -> io::Result<()> {
    let deadline = Instant::now() + NOTIFY_BUDGET;
    let address = SockAddr::unix(socket_path)?;
    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return Err(io::Error::new(
                ErrorKind::TimedOut,
                "Callosum connect timed out",
            ));
        }
        let socket = Socket::new(Domain::UNIX, Type::STREAM, None)?;
        socket.set_nonblocking(true)?;
        match socket.connect(&address) {
            Ok(()) => {
                let stream: UnixStream = socket.into();
                stream.set_nonblocking(false)?;
                let remaining = deadline.saturating_duration_since(Instant::now());
                if remaining.is_zero() {
                    return Err(io::Error::new(
                        ErrorKind::TimedOut,
                        "Callosum write timed out",
                    ));
                }
                stream.set_write_timeout(Some(remaining))?;
                stream.set_read_timeout(Some(remaining))?;
                let mut stream = stream;
                return stream.write_all(line.as_bytes());
            }
            Err(error) if error.kind() == ErrorKind::WouldBlock => {
                let delay = RETRY_DELAY.min(deadline.saturating_duration_since(Instant::now()));
                if delay.is_zero() {
                    return Err(io::Error::new(
                        ErrorKind::TimedOut,
                        "Callosum connect timed out",
                    ));
                }
                thread::sleep(delay);
            }
            Err(error) => return Err(error),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::time::Instant;

    use super::CallosumIngestNotifier;
    use solstone_core_ingest_resolve::{IngestNotice, IngestNotifier};

    #[test]
    fn nonexistent_socket_fails_without_spending_the_retry_budget() {
        let notifier = CallosumIngestNotifier {
            socket_path: std::env::temp_dir()
                .join(format!("solstone-missing-callosum-{}", std::process::id())),
        };
        let meta = serde_json::Map::new();
        let notice = IngestNotice {
            did: "sha256:test",
            source: "source",
            day: "20260811",
            stream: "device",
            segment: "120000_1",
            files: &[],
            meta: &meta,
        };
        let started = Instant::now();

        assert!(notifier.notify(&notice).is_err());
        assert!(
            started.elapsed() < std::time::Duration::from_millis(100),
            "a missing socket is terminal, not retried for the one-second budget"
        );
    }
}
