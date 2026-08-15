// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::fs;
use std::io::{self, ErrorKind, Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::thread::sleep;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde_json::Value;
use sha2::{Digest, Sha256};

struct TempJournal(PathBuf);

impl TempJournal {
    fn established(name: &str) -> Self {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock is after epoch")
            .as_nanos();
        let path = std::env::temp_dir().join(format!("solstone-core-convey-{name}-{nanos}"));
        fs::create_dir_all(path.join("config")).expect("journal config directory creates");
        fs::write(
            path.join("config/journal.json"),
            br#"{"setup":{"completed_at":1767225600}}"#,
        )
        .expect("established journal config writes");
        Self(path)
    }
}

impl Drop for TempJournal {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

struct ChildGuard(Child);

impl ChildGuard {
    fn terminate(&mut self) {
        if self.0.try_wait().expect("child status reads").is_none() {
            self.0.kill().expect("convey child terminates");
        }
        self.0.wait().expect("convey child reaps");
    }
}

impl Drop for ChildGuard {
    fn drop(&mut self) {
        self.terminate();
    }
}

struct HttpResponse {
    status: u16,
    content_type: String,
    body: Vec<u8>,
}

fn free_port() -> u16 {
    let listener = TcpListener::bind(("127.0.0.1", 0)).expect("free port reserves");
    let port = listener.local_addr().expect("local address reads").port();
    drop(listener);
    port
}

fn spawn_convey(port: u16, journal: &TempJournal) -> ChildGuard {
    for _ in 0..100 {
        match Command::new(env!("CARGO_BIN_EXE_solstone-core"))
            .args(["convey", "--port", &port.to_string(), "--journal"])
            .arg(&journal.0)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
        {
            Ok(child) => return ChildGuard(child),
            Err(error) if error.kind() == ErrorKind::ExecutableFileBusy => {
                sleep(Duration::from_millis(20));
            }
            Err(error) => panic!("convey binary should spawn: {error}"),
        }
    }
    panic!("convey binary stayed busy after retries")
}

fn request(address: SocketAddr, path: &str) -> io::Result<HttpResponse> {
    let mut stream = TcpStream::connect_timeout(&address, Duration::from_millis(100))?;
    stream.set_read_timeout(Some(Duration::from_secs(1)))?;
    stream.write_all(
        format!("GET {path} HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n").as_bytes(),
    )?;
    let mut bytes = Vec::new();
    let mut chunk = [0_u8; 8192];
    let split = loop {
        let read = stream.read(&mut chunk)?;
        if read == 0 {
            return Err(io::Error::new(
                ErrorKind::UnexpectedEof,
                "HTTP response ended before its headers",
            ));
        }
        bytes.extend_from_slice(&chunk[..read]);
        if let Some(split) = bytes.windows(4).position(|window| window == b"\r\n\r\n") {
            break split;
        }
    };
    let headers = std::str::from_utf8(&bytes[..split])
        .map_err(|_| io::Error::new(ErrorKind::InvalidData, "HTTP headers are not UTF-8"))?;
    let status = headers
        .split_whitespace()
        .nth(1)
        .ok_or_else(|| io::Error::new(ErrorKind::InvalidData, "HTTP response has no status"))?
        .parse()
        .map_err(|_| io::Error::new(ErrorKind::InvalidData, "HTTP status is invalid"))?;
    let content_type = headers
        .lines()
        .find_map(|line| {
            line.strip_prefix("content-type: ")
                .or_else(|| line.strip_prefix("Content-Type: "))
        })
        .unwrap_or_default()
        .to_owned();
    let content_length = headers
        .lines()
        .find_map(|line| {
            line.strip_prefix("content-length: ")
                .or_else(|| line.strip_prefix("Content-Length: "))
        })
        .ok_or_else(|| io::Error::new(ErrorKind::InvalidData, "HTTP response has no length"))?
        .parse::<usize>()
        .map_err(|_| io::Error::new(ErrorKind::InvalidData, "HTTP content length is invalid"))?;
    while bytes.len() < split + 4 + content_length {
        let read = stream.read(&mut chunk)?;
        if read == 0 {
            return Err(io::Error::new(
                ErrorKind::UnexpectedEof,
                "HTTP response body ended early",
            ));
        }
        bytes.extend_from_slice(&chunk[..read]);
    }
    Ok(HttpResponse {
        status,
        content_type,
        body: bytes[split + 4..split + 4 + content_length].to_vec(),
    })
}

fn wait_until_ready(port: u16, child: &mut ChildGuard) {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        if let Ok(response) = request(SocketAddr::from(([127, 0, 0, 1], port)), "/api/shell")
            && response.status == 200
        {
            return;
        }
        if Instant::now() >= deadline {
            child.terminate();
            let status = child.0.try_wait().expect("child status reads");
            let stderr = child
                .0
                .stderr
                .as_mut()
                .map(|stderr| {
                    let mut output = String::new();
                    let _ = stderr.read_to_string(&mut output);
                    output
                })
                .unwrap_or_default();
            panic!("convey did not become ready; status={status:?}; stderr={stderr}");
        }
        sleep(Duration::from_millis(20));
    }
}

#[test]
fn convey_process_serves_shell_on_both_loopbacks_and_writes_its_port_file() {
    let journal = TempJournal::established("process");
    let port = free_port();
    let mut child = spawn_convey(port, &journal);
    wait_until_ready(port, &mut child);

    let speakers = request(SocketAddr::from(([127, 0, 0, 1], port)), "/app/speakers/")
        .expect("speakers shell responds");
    assert_eq!(speakers.status, 200);
    assert_eq!(
        format!("{:x}", Sha256::digest(&speakers.body)),
        "508e101adc759313ddce94c3263f4124f7575e7dd0d07dfc69cf02817746e7fe"
    );

    let shell_v4 = request(SocketAddr::from(([127, 0, 0, 1], port)), "/api/shell")
        .expect("IPv4 shell responds");
    assert_eq!(shell_v4.status, 200);
    assert_eq!(shell_v4.content_type, "application/json");
    assert_eq!(
        serde_json::from_slice::<Value>(&shell_v4.body).expect("shell JSON")["apps"]
            .as_array()
            .expect("apps array")
            .len(),
        21
    );
    assert_eq!(
        request(
            SocketAddr::from(([0, 0, 0, 0, 0, 0, 0, 1], port)),
            "/api/shell"
        )
        .expect("IPv6 shell responds")
        .status,
        200
    );
    assert_eq!(
        request(
            SocketAddr::from(([127, 0, 0, 1], port)),
            "/app/speakers/api/state"
        )
        .expect("speaker state responds")
        .status,
        200
    );
    assert_eq!(
        fs::read(journal.0.join("health/convey.port")).expect("port file reads"),
        port.to_string().into_bytes()
    );

    child.terminate();
}

#[test]
fn convey_reports_an_occupied_port() {
    let journal = TempJournal::established("occupied");
    let listener = TcpListener::bind(("127.0.0.1", 0)).expect("port reserves");
    let port = listener.local_addr().expect("local address reads").port();
    let output = Command::new(env!("CARGO_BIN_EXE_solstone-core"))
        .args(["convey", "--port", &port.to_string(), "--journal"])
        .arg(&journal.0)
        .output()
        .expect("convey command runs");
    assert!(!output.status.success());
    let stderr = String::from_utf8(output.stderr).expect("stderr is UTF-8");
    assert!(stderr.contains(&port.to_string()));
    assert!(stderr.contains("convey may already be running"));
}
