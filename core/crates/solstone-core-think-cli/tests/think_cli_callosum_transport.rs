// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::fs;
use std::io::{self, BufRead, BufReader, Read, Seek, SeekFrom};
use std::os::unix::net::UnixListener;

use chrono::NaiveDate;
use serde_json::{Map, Value};
use solstone_core_journal_io::{JournalRoot, operational_log::catalog_oplogs};

const STATUS_LINE: &[u8] =
    b"{\"tract\":\"think\",\"event\":\"status\",\"ts\":1785000000000,\"mode\":\"daily\"}\n";

#[test]
fn think_emission_is_exact_write_only_newline_envelope_with_eof() {
    let journal = tempfile::tempdir().expect("think journal");
    let health = journal.path().join("health");
    fs::create_dir_all(&health).expect("create health directory");
    let socket = health.join("callosum.sock");
    assert!(
        socket.as_os_str().len() < 100,
        "private Callosum fixture path exceeds the Unix socket path budget: {}",
        socket.display()
    );
    let listener = UnixListener::bind(&socket).expect("bind private Callosum listener");

    assert!(solstone_core_think_cli::test_support::emit(
        journal.path(),
        1_785_000_000_000,
        "status",
        Map::from_iter([("mode".to_owned(), Value::String("daily".to_owned()))]),
    ));

    listener
        .set_nonblocking(true)
        .expect("make the already-ready accept bounded");
    let (mut stream, _) = listener.accept().expect("accept queued think request");
    stream
        .set_nonblocking(false)
        .expect("make accepted request blocking for the bounded read");
    let mut received = Vec::new();
    stream
        .read_to_end(&mut received)
        .expect("read request through sender EOF");
    assert_eq!(received, STATUS_LINE);
    let mut after_eof = [0_u8; 1];
    assert_eq!(stream.read(&mut after_eof).expect("re-read EOF"), 0);
    assert!(matches!(listener.accept(), Err(error) if error.kind() == io::ErrorKind::WouldBlock));

    drop(stream);
    drop(listener);
    assert!(!solstone_core_think_cli::test_support::emit(
        journal.path(),
        1,
        "status",
        Map::new(),
    ));
}

#[test]
fn think_runtime_can_connect_a_unix_socket() {
    let directory = tempfile::tempdir().expect("socket directory");
    let path = directory.path().join("callosum.sock");
    let listener = UnixListener::bind(&path).expect("bind");
    let runtime = solstone_core_think_cli::test_support::runtime().expect("think runtime");
    runtime.block_on(async {
        tokio::net::UnixStream::connect(&path)
            .await
            .expect("think runtime must enable IO");
    });
    drop(listener);
}

#[test]
fn segment_dispatch_uses_one_timestamp_for_callosum_and_the_run_log() {
    let journal = tempfile::tempdir().expect("think journal");
    let health = journal.path().join("health");
    fs::create_dir_all(&health).expect("create health directory");
    let listener = UnixListener::bind(health.join("callosum.sock")).expect("bind listener");

    assert!(
        solstone_core_think_cli::test_support::emit_segment_dispatch(
            journal.path(),
            "20260813",
            1_785_000_200_000,
        )
        .expect("write segment dispatch")
    );

    let (mut stream, _) = listener.accept().expect("accept segment dispatch");
    let mut received = String::new();
    stream
        .read_to_string(&mut received)
        .expect("read segment dispatch");
    let callosum: Value = serde_json::from_str(received.trim_end()).expect("Callosum JSON");
    let day = NaiveDate::parse_from_str("20260813", "%Y%m%d").unwrap();
    let dispatch = catalog_oplogs(JournalRoot::open(journal.path()).unwrap(), &[day])
        .unwrap()
        .into_catalogued_entries()
        .into_iter()
        .flat_map(|(entry, mut file)| {
            file.seek(SeekFrom::Start(entry.payload_offset() as u64))
                .unwrap();
            BufReader::new(file)
                .lines()
                .map_while(Result::ok)
                .filter_map(|line| serde_json::from_str::<Value>(&line).ok())
                .collect::<Vec<_>>()
        })
        .find(|record| record["event"] == "talent.dispatch")
        .expect("talent.dispatch record");
    assert_eq!(callosum["event"], "talent_started");
    assert_eq!(callosum["ts"], dispatch["ts"]);
    assert_eq!(dispatch["ts"], 1_785_000_200_000_i64);
}
