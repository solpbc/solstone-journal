// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! AC16: prove the standalone native transcribe binary composes its helper seams.

use std::fs;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::PathBuf;
use std::process::Command;
use std::thread;

use serde_json::Value;

const TRANSCRIBE: &str = env!("CARGO_BIN_EXE_solstone-transcribe");
const VAD_STUB: &str = env!("CARGO_BIN_EXE_solstone-transcribe-vad-stub");
const SPEAKERS_STUB: &str = env!("CARGO_BIN_EXE_solstone-transcribe-speakers-stub");

#[test]
fn standalone_binary_reaches_transcript_publication_with_stubbed_onnx_helpers() {
    if std::env::consts::OS != "linux" {
        return;
    }
    let temporary = tempfile::tempdir().expect("temporary journal");
    let journal = temporary.path();
    let segment = journal.join("chronicle/20260810/audio/120000_1");
    fs::create_dir_all(&segment).expect("segment directory");
    let audio = segment.join("audio.wav");
    fs::copy(fixture_audio(), &audio).expect("copy parakeet sample");

    let assets = journal.join("model-assets");
    fs::create_dir_all(&assets).expect("model assets directory");
    for name in [
        "silero_vad_v6.onnx",
        "wespeaker-resnet34-256.onnx",
        "pyannote-segmentation-3.0.onnx",
    ] {
        fs::copy(model_asset(name), assets.join(name)).expect("copy model asset");
    }

    let server = ParakeetStub::start();
    let health = journal.join("health");
    fs::create_dir_all(&health).expect("health directory");
    fs::write(health.join("parakeet-cpp.port"), server.port().to_string()).expect("port file");

    let output = Command::new(TRANSCRIBE)
        .arg("--backend")
        .arg("parakeet-cpp")
        .arg(&audio)
        .env("SOLSTONE_JOURNAL", journal)
        .env("SOL_SKIP_SUPERVISOR_CHECK", "1")
        .env("SOLSTONE_VAD_BINARY", VAD_STUB)
        .env("SOLSTONE_SPEAKERS_ANALYZE_BINARY", SPEAKERS_STUB)
        .env("SOLSTONE_TRANSCRIBE_MODEL_ASSETS_DIR", &assets)
        .output()
        .expect("start standalone transcribe binary");
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    server.join();

    let jsonl = audio.with_extension("jsonl");
    let npz = audio.with_extension("npz");
    assert!(jsonl.is_file(), "transcript must be published");
    assert!(npz.is_file(), "embedding sidecar must be published");
    assert!(
        audio.is_file(),
        "successful transcription must retain owner media"
    );

    let mut lines = BufReader::new(fs::File::open(jsonl).expect("open JSONL")).lines();
    let header: Value = serde_json::from_str(&lines.next().expect("header line").expect("header"))
        .expect("header JSON");
    assert_eq!(header["_solstone_processing"]["state"], "analyzed");
    let statement: Value =
        serde_json::from_str(&lines.next().expect("statement line").expect("statement"))
            .expect("statement JSON");
    assert_eq!(statement["text"], "Hello world.");
}

fn fixture_audio() -> PathBuf {
    repository_root().join("solstone/observe/transcribe/_fixtures/parakeet_sample.wav")
}

fn model_asset(name: &str) -> PathBuf {
    repository_root().join("core/models/assets").join(name)
}

fn repository_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(3)
        .expect("repository root")
        .to_path_buf()
}

struct ParakeetStub {
    port: u16,
    thread: thread::JoinHandle<()>,
}

impl ParakeetStub {
    fn start() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind parakeet stub");
        let port = listener.local_addr().expect("stub address").port();
        let thread = thread::spawn(move || {
            for _ in 0..2 {
                let (stream, _) = listener.accept().expect("accept parakeet request");
                respond(stream);
            }
        });
        Self { port, thread }
    }

    fn port(&self) -> u16 {
        self.port
    }

    fn join(self) {
        self.thread.join().expect("parakeet stub thread");
    }
}

fn respond(mut stream: TcpStream) {
    let mut request = Vec::new();
    let mut chunk = [0_u8; 1024];
    while !request.windows(4).any(|window| window == b"\r\n\r\n") {
        let read = stream.read(&mut chunk).expect("read request headers");
        assert!(read > 0, "request ended before headers");
        request.extend_from_slice(&chunk[..read]);
    }
    let header_end = request
        .windows(4)
        .position(|window| window == b"\r\n\r\n")
        .expect("header terminator")
        + 4;
    let headers = String::from_utf8_lossy(&request[..header_end]).into_owned();
    let path = headers.split_whitespace().nth(1).expect("request path");
    if path == "/v1/audio/transcriptions" {
        let content_length = headers
            .lines()
            .find_map(|line| {
                let (name, value) = line.split_once(':')?;
                name.eq_ignore_ascii_case("content-length")
                    .then(|| value.trim().parse::<usize>().ok())
                    .flatten()
            })
            .expect("multipart request must declare its content length");
        while request.len() < header_end + content_length {
            let read = stream.read(&mut chunk).expect("read request body");
            assert!(read > 0, "request ended before body");
            request.extend_from_slice(&chunk[..read]);
        }
    }
    let body = match path {
        "/health" => "{}",
        "/v1/audio/transcriptions" => {
            r#"{"text":"Hello world.","words":[{"word":"Hello ","start":0.0,"end":0.3,"conf":0.99},{"word":"world.","start":0.3,"end":0.7,"conf":0.99}]}"#
        }
        other => panic!("unexpected request path: {other}"),
    };
    write!(
        stream,
        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
        body.len(),
        body
    )
    .expect("write stub response");
}
