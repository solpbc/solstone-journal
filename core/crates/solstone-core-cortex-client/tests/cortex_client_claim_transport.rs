// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

#![cfg(unix)]

use std::fs;
use std::io::{self, Read};
use std::os::unix::net::UnixListener;
use std::path::Path;
use std::time::{Duration, Instant};

use solstone_core_cortex_client::{
    CortexRequest, CortexRequestClient, CortexRequestPolicy, DispatchError, UseEndState,
    UseIdAllocator, get_use_end_state,
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
        .join("talents/steward")
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
fn successful_write_claims_active_and_completed_existing_use_files() {
    for (name, active, body) in [
        (
            "active",
            true,
            br#"{"event":"request","use_id":"active","name":"steward"}"#.as_slice(),
        ),
        (
            "completed",
            false,
            b"{\"event\":\"request\",\"use_id\":\"completed\",\"name\":\"steward\"}\n{\"event\":\"finish\",\"use_id\":\"completed\"}\n"
                .as_slice(),
        ),
    ] {
        let journal = tempfile::tempdir().expect("claim journal");
        let listener = bind(journal.path());
        write_use(journal.path(), name, active, body);
        assert_eq!(dispatch(journal.path(), name), Ok(name.to_owned()));
        accept_lines(&listener, 1, name);
    }
}

#[test]
fn malformed_existing_use_file_is_not_claimed() {
    let journal = tempfile::tempdir().expect("malformed journal");
    let listener = bind(journal.path());
    write_use(journal.path(), "malformed", false, b"not-json\n");
    assert_eq!(
        dispatch(journal.path(), "malformed"),
        Err(DispatchError::NotClaimed {
            use_id: "malformed".to_owned()
        })
    );
    accept_lines(&listener, 3, "malformed");
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

#[test]
fn another_talents_matching_id_does_not_acknowledge_the_request() {
    let journal = tempfile::tempdir().unwrap();
    let listener = bind(journal.path());
    fs::create_dir_all(journal.path().join("talents/other")).unwrap();
    fs::write(
        journal.path().join("talents/other/shared_active.jsonl"),
        br#"{"event":"request","use_id":"shared","name":"other"}"#,
    )
    .unwrap();
    assert_eq!(
        dispatch(journal.path(), "shared"),
        Err(DispatchError::NotClaimed {
            use_id: "shared".into()
        })
    );
    accept_lines(&listener, 3, "shared");
}

#[test]
fn mismatched_request_identity_in_the_expected_file_is_not_claimed() {
    for body in [
        br#"{"event":"request","use_id":"shared","name":"other"}"#.as_slice(),
        br#"{"event":"request","use_id":"other","name":"steward"}"#.as_slice(),
        br#"{"event":"request","use_id":"shared"}"#.as_slice(),
    ] {
        let journal = tempfile::tempdir().unwrap();
        let listener = bind(journal.path());
        write_use(journal.path(), "shared", true, body);
        assert_eq!(
            dispatch(journal.path(), "shared"),
            Err(DispatchError::NotClaimed {
                use_id: "shared".into()
            })
        );
        accept_lines(&listener, 3, "shared");
    }
}

#[test]
fn independent_clients_at_the_same_clock_keep_durable_outcomes_separate() {
    let journal = tempfile::tempdir().unwrap();
    let listener = bind(journal.path());
    listener.set_nonblocking(true).unwrap();
    let root = journal.path().to_path_buf();
    let server = std::thread::spawn(move || {
        for _ in 0..2 {
            let deadline = Instant::now() + Duration::from_secs(10);
            let mut stream = loop {
                match listener.accept() {
                    Ok((stream, _)) => break stream,
                    Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                        assert!(Instant::now() < deadline, "client did not send");
                        std::thread::sleep(Duration::from_millis(5));
                    }
                    Err(error) => panic!("accept: {error}"),
                }
            };
            stream.set_read_timeout(Some(IO_DEADLINE)).unwrap();
            let mut body = String::new();
            stream.read_to_string(&mut body).unwrap();
            let request: serde_json::Value = serde_json::from_str(&body).unwrap();
            let name = request["name"].as_str().unwrap();
            let id = request["use_id"].as_str().unwrap();
            let directory = root.join("talents").join(name);
            fs::create_dir_all(&directory).unwrap();
            let event = if name == "first" { "finish" } else { "error" };
            body.push_str(&format!("{{\"event\":\"{event}\",\"use_id\":\"{id}\"}}\n"));
            fs::write(directory.join(format!("{id}.jsonl")), body).unwrap();
        }
    });
    let make_client = || {
        CortexRequestClient::with_allocator(
            journal.path(),
            CortexRequestPolicy::interactive(),
            UseIdAllocator::new(|| Some(1_700_000_000_000)),
        )
    };
    let first = make_client();
    let second = make_client();
    let (first_id, second_id) = runtime().block_on(async {
        let first_request = CortexRequest::new("first prompt", "first");
        let second_request = CortexRequest::new("second prompt", "second");
        tokio::join!(
            first.dispatch(&first_request),
            second.dispatch(&second_request)
        )
    });
    server.join().unwrap();
    let first_id = first_id.unwrap();
    let second_id = second_id.unwrap();
    assert_ne!(first_id, second_id);
    assert_eq!(first_id, "1700000000000");
    assert_eq!(second_id, "1700000000001");
    assert_eq!(
        get_use_end_state(journal.path(), &first_id).unwrap(),
        UseEndState::Finish
    );
    assert_eq!(
        get_use_end_state(journal.path(), &second_id).unwrap(),
        UseEndState::Error
    );
}

#[test]
fn damaged_counter_refuses_before_request_publication() {
    let journal = tempfile::tempdir().unwrap();
    let listener = bind(journal.path());
    listener.set_nonblocking(true).unwrap();
    fs::write(journal.path().join("health/cortex-use-id.json"), b"damaged").unwrap();
    let result = runtime().block_on(
        CortexRequestClient::new(journal.path(), CortexRequestPolicy::interactive())
            .dispatch(&CortexRequest::new("prompt", "steward")),
    );
    assert_eq!(result, Err(DispatchError::Unavailable));
    assert!(matches!(listener.accept(), Err(error) if error.kind() == io::ErrorKind::WouldBlock));
    assert_eq!(
        fs::read(journal.path().join("health/cortex-use-id.json")).unwrap(),
        b"damaged"
    );
}
