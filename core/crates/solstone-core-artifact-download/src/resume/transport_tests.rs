// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use super::*;
use sha2::{Digest, Sha256};
use std::net::TcpListener;
use std::sync::{Arc, Barrier, Mutex};
use std::thread;
use std::time::Instant;

struct Server {
    url: String,
    requests: Arc<Mutex<Vec<String>>>,
    worker: thread::JoinHandle<()>,
}

impl Server {
    fn new(responses: Vec<Vec<u8>>) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let url = format!("http://{}/file", listener.local_addr().unwrap());
        let requests = Arc::new(Mutex::new(Vec::new()));
        let observed = requests.clone();
        let worker = thread::spawn(move || {
            for response in responses {
                let deadline = Instant::now() + Duration::from_secs(5);
                let (mut connection, _) = loop {
                    match listener.accept() {
                        Ok(connection) => break connection,
                        Err(error)
                            if error.kind() == io::ErrorKind::WouldBlock
                                && Instant::now() < deadline =>
                        {
                            thread::sleep(Duration::from_millis(10))
                        }
                        result => panic!("expected artifact request: {result:?}"),
                    }
                };
                connection
                    .set_read_timeout(Some(Duration::from_secs(2)))
                    .unwrap();
                let mut request = Vec::new();
                while !request.ends_with(b"\r\n\r\n") {
                    let mut byte = [0];
                    connection.read_exact(&mut byte).unwrap();
                    request.push(byte[0]);
                    assert!(request.len() < 16 * 1024);
                }
                observed
                    .lock()
                    .unwrap()
                    .push(String::from_utf8(request).unwrap().to_ascii_lowercase());
                connection.write_all(&response).unwrap();
            }
        });
        Self {
            url,
            requests,
            worker,
        }
    }

    fn finish(self) -> Vec<String> {
        self.worker.join().unwrap();
        Arc::try_unwrap(self.requests)
            .unwrap()
            .into_inner()
            .unwrap()
    }
}

fn response(status: &str, headers: &str, body: &[u8]) -> Vec<u8> {
    let mut bytes = format!("HTTP/1.1 {status}\r\nConnection: close\r\n{headers}\r\n").into_bytes();
    bytes.extend_from_slice(body);
    bytes
}

fn digest(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

fn fetch(server: &Server, target: &Path, pin: &[u8]) -> Result<bool, ArchiveError> {
    download_verified_url_internal(
        &server.url,
        &digest(pin),
        Some(pin.len() as u64),
        target,
        &DownloadHostPolicy {
            allowed_hosts: &["127.0.0.1"],
            allow_http: true,
        },
        &mut |_, _| {},
    )
}

fn seed(target: &Path, pin: &[u8], prefix: &[u8]) -> (PathBuf, PathBuf) {
    let (partial, metadata, mut binding) = partial_paths(target, &digest(pin)).unwrap();
    binding.size_bytes = Some(pin.len() as u64);
    fs::write(&partial, prefix).unwrap();
    fs::write(&metadata, serde_json::to_vec(&binding).unwrap()).unwrap();
    (partial, metadata)
}

#[test]
fn truncated_transfer_resumes_exact_prefix_and_reuses_completed_cache() {
    for first in [
        response("200 OK", "Content-Length: 4\r\n", b"ab"),
        response("200 OK", "", b"ab"),
    ] {
        let root = tempfile::tempdir().unwrap();
        let target = root.path().join("model");
        let server = Server::new(vec![
            first,
            response(
                "206 Partial Content",
                "Content-Range: bytes 2-3/4\r\nContent-Length: 2\r\n",
                b"cd",
            ),
        ]);
        assert!(fetch(&server, &target, b"abcd").is_err());
        assert_eq!(retained_prefix_bytes(&target, &digest(b"abcd"), 4), 2);
        assert!(fetch(&server, &target, b"abcd").unwrap());
        assert!(!fetch(&server, &target, b"abcd").unwrap());
        assert_eq!(fs::read(&target).unwrap(), b"abcd");
        let requests = server.finish();
        assert_eq!(requests.len(), 2);
        assert!(!requests[0].contains("range:"));
        assert!(requests[1].contains("range: bytes=2-\r\n"));
        assert!(
            requests
                .iter()
                .all(|request| request.contains("accept-encoding: identity\r\n"))
        );
    }
}

#[test]
fn ignored_range_restarts_and_redirected_range_keeps_its_binding() {
    for redirected in [false, true] {
        let root = tempfile::tempdir().unwrap();
        let target = root.path().join("model");
        seed(&target, b"abcd", b"ab");
        let responses = if redirected {
            vec![
                response("302 Found", "Location: /next\r\nContent-Length: 0\r\n", b""),
                response(
                    "206 Partial Content",
                    "Content-Range: bytes 2-3/4\r\nContent-Length: 2\r\n",
                    b"cd",
                ),
            ]
        } else {
            vec![response("200 OK", "Content-Length: 4\r\n", b"abcd")]
        };
        let server = Server::new(responses);
        assert!(fetch(&server, &target, b"abcd").unwrap());
        assert_eq!(fs::read(target).unwrap(), b"abcd");
        assert!(
            server
                .finish()
                .iter()
                .all(|request| request.contains("range: bytes=2-\r\n"))
        );
    }
}

#[test]
fn invalid_resume_framing_and_redirects_preserve_the_prefix_and_incumbent() {
    for (status, headers, body) in [
        (
            "206 Partial Content",
            "Content-Range: bytes 1-2/4\r\nContent-Length: 2\r\n",
            &b"cd"[..],
        ),
        (
            "206 Partial Content",
            "Content-Range: bytes 2-3/*\r\nContent-Length: 2\r\n",
            &b"cd"[..],
        ),
        (
            "206 Partial Content",
            "Content-Range: bytes 2-3/4\r\nContent-Length: 3\r\n",
            &b"cde"[..],
        ),
        (
            "206 Partial Content",
            "Content-Range: bytes 2-3/4\r\nContent-Encoding: gzip\r\nContent-Length: 2\r\n",
            &b"cd"[..],
        ),
        (
            "416 Range Not Satisfiable",
            "Content-Range: bytes */4\r\nContent-Length: 0\r\n",
            &b""[..],
        ),
        (
            "302 Found",
            "Location: http://localhost:1/file\r\nContent-Length: 0\r\n",
            &b""[..],
        ),
    ] {
        let root = tempfile::tempdir().unwrap();
        let target = root.path().join("model");
        fs::write(&target, b"old!").unwrap();
        let (partial, _) = seed(&target, b"abcd", b"ab");
        let server = Server::new(vec![response(status, headers, body)]);
        assert!(fetch(&server, &target, b"abcd").is_err(), "{headers}");
        assert_eq!(fs::read(partial).unwrap(), b"ab");
        assert_eq!(fs::read(target).unwrap(), b"old!");
        assert_eq!(server.finish().len(), 1);
    }
}

#[test]
fn bad_full_digest_resets_without_replacing_the_incumbent() {
    let root = tempfile::tempdir().unwrap();
    let target = root.path().join("model");
    fs::write(&target, b"old!").unwrap();
    let server = Server::new(vec![response("200 OK", "Content-Length: 4\r\n", b"bad!")]);
    assert!(matches!(
        fetch(&server, &target, b"abcd"),
        Err(ArchiveError::DigestMismatch { .. })
    ));
    assert_eq!(retained_prefix_bytes(&target, &digest(b"abcd"), 4), 0);
    assert_eq!(fs::read(target).unwrap(), b"old!");
    assert_eq!(server.finish().len(), 1);
}

#[test]
fn changed_pins_and_malformed_metadata_do_not_mix_or_block_retry() {
    let root = tempfile::tempdir().unwrap();
    let target = root.path().join("model");
    let (old_partial, _) = seed(&target, b"old!", b"ol");
    let (partial, meta) = seed(&target, b"abcd", b"untrusted");
    fs::write(&meta, b"{").unwrap();
    let server = Server::new(vec![response("200 OK", "Content-Length: 4\r\n", b"abcd")]);
    assert!(fetch(&server, &target, b"abcd").unwrap());
    assert_eq!(fs::read(old_partial).unwrap(), b"ol");
    assert!(!partial.exists());
    let preserved = fs::read_dir(root.path())
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .find(|path| path.is_dir())
        .unwrap();
    assert_eq!(fs::read(preserved.join("payload")).unwrap(), b"untrusted");
    assert_eq!(fs::read(preserved.join("binding.json")).unwrap(), b"{");
    assert!(!server.finish()[0].contains("range:"));
}

#[test]
fn simultaneous_writers_share_one_verified_publication() {
    let root = tempfile::tempdir().unwrap();
    let target = root.path().join("model");
    let server = Server::new(vec![response("200 OK", "Content-Length: 4\r\n", b"abcd")]);
    let barrier = Arc::new(Barrier::new(3));
    let workers = (0..2)
        .map(|_| {
            let target = target.clone();
            let url = server.url.clone();
            let barrier = barrier.clone();
            thread::spawn(move || {
                barrier.wait();
                download_verified_url_internal(
                    &url,
                    &digest(b"abcd"),
                    Some(4),
                    &target,
                    &DownloadHostPolicy {
                        allowed_hosts: &["127.0.0.1"],
                        allow_http: true,
                    },
                    &mut |_, _| {},
                )
                .unwrap()
            })
        })
        .collect::<Vec<_>>();
    barrier.wait();
    let mut fetched = workers
        .into_iter()
        .map(|worker| worker.join().unwrap())
        .collect::<Vec<_>>();
    fetched.sort();
    assert_eq!(fetched, [false, true]);
    assert_eq!(server.finish().len(), 1);
}
