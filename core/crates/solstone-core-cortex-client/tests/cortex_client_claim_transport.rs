// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::fs;
use std::io::{self, Read};
use std::os::unix::net::UnixListener;
use std::path::Path;
use std::time::{Duration, Instant};

use solstone_core_cortex_client::{
    CortexRequest, CortexRequestClient, CortexRequestPolicy, DispatchError,
};

const IO_DEADLINE: Duration = Duration::from_secs(2);

fn bind(journal: &Path) -> UnixListener {
    let health = journal.join("health");
    fs::create_dir_all(&health).expect("create health directory");
    let socket = health.join("callosum.sock");
    assert!(
        socket.as_os_str().len() < 100,
        "private Callosum fixture path exceeds the Unix socket path budget: {}",
        socket.display()
    );
    UnixListener::bind(socket).expect("bind private Callosum listener")
}

fn write_use(journal: &Path, use_id: &str, active: bool, body: &[u8]) {
    let suffix = if active { "_active.jsonl" } else { ".jsonl" };
    let path = journal
        .join("talents/test")
        .join(format!("{use_id}{suffix}"));
    fs::create_dir_all(path.parent().expect("use file parent")).expect("create talent directory");
    fs::write(path, body).expect("write durable use file");
}

fn accept_lines(listener: &UnixListener, expected_count: usize, use_id: &str) {
    listener
        .set_nonblocking(true)
        .expect("make queued accepts bounded");
    for _ in 0..expected_count {
        let (mut stream, _) = listener.accept().expect("accept queued claim request");
        let mut bytes = Vec::new();
        let deadline = Instant::now() + IO_DEADLINE;
        let mut buffer = [0_u8; 256];
        loop {
            match stream.read(&mut buffer) {
                Ok(0) => break,
                Ok(count) => bytes.extend_from_slice(&buffer[..count]),
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                    assert!(
                        Instant::now() < deadline,
                        "request sender did not close before the I/O deadline"
                    );
                    std::thread::sleep(Duration::from_millis(10));
                }
                Err(error) => panic!("read request through sender EOF: {error}"),
            }
        }
        assert_eq!(
            bytes,
            format!(
                "{{\"tract\":\"cortex\",\"event\":\"request\",\"ts\":42,\"use_id\":\"{use_id}\",\"prompt\":\"prompt\",\"name\":\"steward\"}}\n"
            )
            .as_bytes()
        );
        let mut after_eof = [0_u8; 1];
        assert_eq!(stream.read(&mut after_eof).expect("re-read EOF"), 0);
    }
    assert!(matches!(listener.accept(), Err(error) if error.kind() == io::ErrorKind::WouldBlock));
}

fn runtime() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_current_thread()
        .enable_time()
        .build()
        .expect("Cortex client test runtime")
}

fn dispatch(journal: &Path, use_id: &str) -> Result<String, DispatchError> {
    runtime().block_on(async {
        tokio::time::pause();
        CortexRequestClient::new(journal, CortexRequestPolicy::interactive())
            .dispatch_with_use_id(
                &CortexRequest::new("prompt", "steward"),
                42,
                use_id.to_owned(),
            )
            .await
    })
}

#[test]
fn successful_write_claims_active_completed_and_malformed_existing_use_files() {
    for (name, active, body) in [
        ("active", true, b"{\"event\":\"request\"}\n".as_slice()),
        ("completed", false, b"{\"event\":\"finish\"}\n".as_slice()),
        ("malformed", false, b"not-json\n".as_slice()),
    ] {
        let journal = tempfile::tempdir().expect("claim journal");
        let listener = bind(journal.path());
        write_use(journal.path(), name, active, body);
        assert_eq!(dispatch(journal.path(), name), Ok(name.to_owned()));
        accept_lines(&listener, 1, name);
    }
}

#[test]
fn successful_write_without_a_use_file_is_not_claimed() {
    let journal = tempfile::tempdir().expect("unclaimed journal");
    let listener = bind(journal.path());
    assert_eq!(
        dispatch(journal.path(), "missing"),
        Err(DispatchError::NotClaimed {
            use_id: "missing".to_owned()
        })
    );
    accept_lines(&listener, 3, "missing");
}

#[test]
fn unavailable_transport_and_unreadable_claim_storage_are_unavailable() {
    let unavailable = tempfile::tempdir().expect("unavailable journal");
    assert_eq!(
        dispatch(unavailable.path(), "transport"),
        Err(DispatchError::Unavailable)
    );

    let unreadable = tempfile::tempdir().expect("unreadable journal");
    fs::write(unreadable.path().join("talents"), b"not a directory")
        .expect("create unreadable claim-storage fixture");
    assert_eq!(
        dispatch(unreadable.path(), "storage"),
        Err(DispatchError::Unavailable)
    );
}
