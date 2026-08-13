// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::fs;
use std::io::{BufRead, BufReader, Read};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdout, Command, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use serde_json::{Map, Value, json};
use solstone_core_callosum::{CallosumSocketConnection, CallosumSocketServer};
use tokio::time::{sleep, timeout};

static NEXT_ROOT: AtomicUsize = AtomicUsize::new(0);

const PYTHON_CLIENT: &str = concat!(
    "import json, os, sys, time\n",
    "from pathlib import Path\n",
    "sys.path.insert(0, os.environ['SOLSTONE_REPO_ROOT'])\n",
    "from solstone.think.callosum import CallosumConnection\n",
    "socket_path = Path(os.environ['CALLOSUM_SOCKET'])\n",
    "messages = json.loads(os.environ['CALLOSUM_MESSAGES'])\n",
    "expected = int(os.environ['CALLOSUM_EXPECTED'])\n",
    "received = []\n",
    "receiver = CallosumConnection(socket_path)\n",
    "receiver.start(lambda message: received.append(message))\n",
    "time.sleep(0.3)\n",
    "print('ready', flush=True)\n",
    "sender = None\n",
    "if messages:\n",
    "    sender = CallosumConnection(socket_path)\n",
    "    sender.start()\n",
    "    time.sleep(0.1)\n",
    "    for message in messages:\n",
    "        fields = dict(message)\n",
    "        tract = fields.pop('tract')\n",
    "        event = fields.pop('event')\n",
    "        if not sender.emit(tract, event, **fields):\n",
    "            raise RuntimeError('Python Callosum emit was rejected')\n",
    "deadline = time.monotonic() + 5.0\n",
    "while len(received) < expected and time.monotonic() < deadline:\n",
    "    time.sleep(0.02)\n",
    "if len(received) < expected:\n",
    "    raise RuntimeError('Python Callosum receive timed out')\n",
    "print(json.dumps(received), flush=True)\n",
    "if sender:\n",
    "    sender.stop()\n",
    "receiver.stop()\n"
);

const PYTHON_SERVER: &str = concat!(
    "import os, sys, threading, time\n",
    "from pathlib import Path\n",
    "sys.path.insert(0, os.environ['SOLSTONE_REPO_ROOT'])\n",
    "from solstone.think.callosum import CallosumServer\n",
    "server = CallosumServer(Path(os.environ['CALLOSUM_SOCKET']))\n",
    "thread = threading.Thread(target=server.start, daemon=True)\n",
    "thread.start()\n",
    "deadline = time.monotonic() + 5.0\n",
    "while not Path(os.environ['CALLOSUM_SOCKET']).exists() and time.monotonic() < deadline:\n",
    "    time.sleep(0.01)\n",
    "if not Path(os.environ['CALLOSUM_SOCKET']).exists():\n",
    "    raise RuntimeError('Python Callosum server did not bind')\n",
    "print('ready', flush=True)\n",
    "time.sleep(30.0)\n"
);

struct TempSocket {
    root: PathBuf,
    path: PathBuf,
}

impl TempSocket {
    fn new(name: &str) -> Self {
        let suffix = NEXT_ROOT.fetch_add(1, Ordering::Relaxed);
        let root = std::env::temp_dir().join(format!(
            "solstone-callosum-cross-{name}-{}-{suffix}",
            std::process::id()
        ));
        fs::create_dir_all(&root).expect("create temporary socket directory");
        Self {
            path: root.join("callosum.sock"),
            root,
        }
    }
}

impl Drop for TempSocket {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

fn repository_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(3)
        .expect("repository root")
        .to_path_buf()
}

fn python() -> PathBuf {
    let repository = repository_root();
    let venv = repository.join(".venv/bin/python3");
    if venv.is_file() {
        venv
    } else {
        PathBuf::from("python3")
    }
}

struct PythonClient {
    child: Option<Child>,
    stdout: Option<BufReader<ChildStdout>>,
}

impl PythonClient {
    fn start(socket_path: &Path, messages: &[Value], expected: usize) -> Self {
        let mut child = Command::new(python())
            .args(["-c", PYTHON_CLIENT])
            .env("SOLSTONE_REPO_ROOT", repository_root())
            .env("CALLOSUM_SOCKET", socket_path)
            .env(
                "CALLOSUM_MESSAGES",
                serde_json::to_string(messages).expect("serialize Python messages"),
            )
            .env("CALLOSUM_EXPECTED", expected.to_string())
            .stdout(Stdio::piped())
            .spawn()
            .expect("start Python Callosum client");
        let stdout = child.stdout.take().expect("Python client stdout");
        Self {
            child: Some(child),
            stdout: Some(BufReader::new(stdout)),
        }
    }

    fn wait_ready(&mut self) {
        let mut line = String::new();
        self.stdout
            .as_mut()
            .expect("Python client stdout")
            .read_line(&mut line)
            .expect("read Python client readiness");
        assert_eq!(line.trim(), "ready");
    }

    async fn finish(mut self) -> String {
        let mut child = self.child.take().expect("Python client child");
        let mut stdout = self.stdout.take().expect("Python client stdout");
        let tail = tokio::task::spawn_blocking(move || {
            let mut tail = String::new();
            stdout
                .read_to_string(&mut tail)
                .expect("read Python client output");
            assert!(child.wait().expect("reap Python client").success());
            tail
        })
        .await
        .expect("join Python client output reader");
        tail.lines()
            .last()
            .expect("Python client observation")
            .to_owned()
    }
}

impl Drop for PythonClient {
    fn drop(&mut self) {
        if let Some(child) = &mut self.child
            && child.try_wait().expect("poll Python client").is_none()
        {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

struct PythonServer {
    child: Option<Child>,
}

impl PythonServer {
    fn start(socket_path: &Path) -> Self {
        let mut child = Command::new(python())
            .args(["-c", PYTHON_SERVER])
            .env("SOLSTONE_REPO_ROOT", repository_root())
            .env("CALLOSUM_SOCKET", socket_path)
            .stdout(Stdio::piped())
            .spawn()
            .expect("start Python Callosum server");
        let stdout = child.stdout.take().expect("Python server stdout");
        let mut lines = BufReader::new(stdout).lines();
        assert_eq!(
            lines
                .next()
                .expect("server ready line")
                .expect("server ready text"),
            "ready"
        );
        Self { child: Some(child) }
    }
}

impl Drop for PythonServer {
    fn drop(&mut self) {
        if let Some(child) = &mut self.child
            && child.try_wait().expect("poll Python server").is_none()
        {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

fn connection(socket_path: &Path) -> CallosumSocketConnection {
    let mut connection = CallosumSocketConnection::new(socket_path, Map::new());
    connection.start();
    connection
}

async fn wait_for_clients(server: &CallosumSocketServer, count: usize) {
    timeout(Duration::from_secs(3), async {
        while server.client_count() != count {
            sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("expected connected clients");
}

async fn next_value(connection: &mut CallosumSocketConnection) -> Value {
    let envelope = timeout(Duration::from_secs(3), connection.next_message())
        .await
        .expect("reflected message should arrive")
        .expect("connection receiver should stay open");
    serde_json::to_value(envelope).expect("serialize reflected envelope")
}

async fn next_values(connection: &mut CallosumSocketConnection, count: usize) -> Vec<Value> {
    let mut values = Vec::with_capacity(count);
    for _ in 0..count {
        values.push(next_value(connection).await);
    }
    values
}

fn emit_value(connection: &CallosumSocketConnection, value: &Value) -> bool {
    let mut fields = value.as_object().expect("envelope object").clone();
    let tract = fields
        .remove("tract")
        .expect("tract")
        .as_str()
        .expect("string tract")
        .to_owned();
    let event = fields
        .remove("event")
        .expect("event")
        .as_str()
        .expect("string event")
        .to_owned();
    connection.emit(&tract, &event, fields)
}

fn assert_reflection(original: &Value, reflected: &Value) {
    let timestamp = reflected["ts"].as_i64().expect("integer server timestamp");
    assert!(timestamp > 0);
    let mut expected = original.clone();
    expected
        .as_object_mut()
        .expect("envelope object")
        .insert("ts".to_owned(), json!(timestamp));
    assert_eq!(reflected, &expected);
}

fn parse_observations(output: &str) -> Vec<Value> {
    serde_json::from_str(output).expect("parse Python observations")
}

fn fixtures() -> Vec<Value> {
    vec![
        json!({"tract": "future", "event": "unknown", "extension": true}),
        json!({
            "tract": "nested",
            "event": "object",
            "payload": {"items": [1, {"state": "kept"}], "enabled": true},
        }),
        json!({
            "tract": "unicode",
            "event": "integer",
            "text": "日本語🦀",
            "integer": 9_007_199_254_740_993_i64,
        }),
    ]
}

#[tokio::test(flavor = "current_thread")]
async fn ac19_python_client_rust_server_python_client_round_trip() {
    let socket = TempSocket::new("ac19");
    let server = CallosumSocketServer::bind(&socket.path)
        .await
        .expect("bind Rust Callosum server");
    let mut observer = connection(&socket.path);
    wait_for_clients(&server, 1).await;

    let message = json!({
        "tract": "python",
        "event": "to_rust",
        "extension": {"preserved": true},
    });
    let mut client = PythonClient::start(&socket.path, std::slice::from_ref(&message), 1);
    client.wait_ready();
    wait_for_clients(&server, 3).await;

    let rust_observation = next_value(&mut observer).await;
    let python_observation = parse_observations(&client.finish().await)
        .into_iter()
        .next()
        .expect("Python reflected observation");
    assert_reflection(&message, &rust_observation);
    assert_eq!(rust_observation, python_observation);

    observer.stop().await;
    server.stop().await;
}

#[tokio::test(flavor = "current_thread")]
async fn ac20_rust_connection_python_server_rust_connection_round_trip() {
    let socket = TempSocket::new("ac20");
    let _server = PythonServer::start(&socket.path);
    let mut receiver = connection(&socket.path);
    let mut sender = connection(&socket.path);
    sleep(Duration::from_millis(250)).await;

    let message = json!({
        "tract": "rust",
        "event": "to_python",
        "extension": {"preserved": true},
    });
    assert!(emit_value(&sender, &message));
    assert_reflection(&message, &next_value(&mut receiver).await);

    sender.stop().await;
    receiver.stop().await;
}

#[tokio::test(flavor = "current_thread")]
async fn ac21_escaped_large_unicode_frames_keep_python_peer_connected() {
    let socket = TempSocket::new("ac21");
    let _server = PythonServer::start(&socket.path);
    let mut receiver = connection(&socket.path);
    let mut sender = connection(&socket.path);
    sleep(Duration::from_millis(250)).await;

    let text = "日本語🦀".repeat(1_000);
    assert!(text.chars().count() * 6 > 4_096);
    let large = json!({"tract": "unicode", "event": "large", "text": text});
    assert!(emit_value(&sender, &large));
    assert_reflection(&large, &next_value(&mut receiver).await);

    let follow_up = json!({"tract": "unicode", "event": "after", "extension": true});
    assert!(emit_value(&sender, &follow_up));
    assert_reflection(&follow_up, &next_value(&mut receiver).await);

    sender.stop().await;
    receiver.stop().await;
}

#[tokio::test(flavor = "current_thread")]
async fn ac22_cross_language_fixture_parse_equality_and_separator_difference() {
    let messages = fixtures();

    let rust_to_python = TempSocket::new("ac22-rust-to-python");
    let _python_server = PythonServer::start(&rust_to_python.path);
    let mut rust_receiver = connection(&rust_to_python.path);
    let mut rust_sender = connection(&rust_to_python.path);
    let mut python_receiver = PythonClient::start(&rust_to_python.path, &[], messages.len());
    python_receiver.wait_ready();
    sleep(Duration::from_millis(250)).await;
    for message in &messages {
        assert!(emit_value(&rust_sender, message));
    }
    let rust_observations = next_values(&mut rust_receiver, messages.len()).await;
    let python_output = python_receiver.finish().await;
    let python_observations = parse_observations(&python_output);
    assert_eq!(python_observations.len(), messages.len());
    for ((message, rust_observation), python_observation) in messages
        .iter()
        .zip(&rust_observations)
        .zip(&python_observations)
    {
        assert_reflection(message, rust_observation);
        assert_eq!(rust_observation, python_observation);
    }
    let rust_json = String::from_utf8(
        serde_json::to_vec(&python_observations).expect("serialize compact Rust JSON"),
    )
    .expect("Rust JSON is UTF-8");
    assert!(
        !rust_json.contains(", ") && !rust_json.contains(": "),
        "serde_json keeps compact comma/colon separators"
    );
    assert!(
        python_output.contains(", ") && python_output.contains(": "),
        "Python json.dumps keeps spaces after comma and colon"
    );
    assert_eq!(
        serde_json::from_str::<Value>(&rust_json).expect("parse Rust JSON"),
        serde_json::from_str::<Value>(&python_output).expect("parse Python JSON")
    );
    rust_sender.stop().await;
    rust_receiver.stop().await;

    let python_to_rust = TempSocket::new("ac22-python-to-rust");
    let rust_server = CallosumSocketServer::bind(&python_to_rust.path)
        .await
        .expect("bind Rust Callosum server");
    let mut rust_observer = connection(&python_to_rust.path);
    wait_for_clients(&rust_server, 1).await;
    let mut python_sender = PythonClient::start(&python_to_rust.path, &messages, messages.len());
    python_sender.wait_ready();
    wait_for_clients(&rust_server, 3).await;
    let rust_observations = next_values(&mut rust_observer, messages.len()).await;
    let python_observations = parse_observations(&python_sender.finish().await);
    assert_eq!(python_observations.len(), messages.len());
    for ((message, rust_observation), python_observation) in messages
        .iter()
        .zip(&rust_observations)
        .zip(&python_observations)
    {
        assert_reflection(message, rust_observation);
        assert_eq!(rust_observation, python_observation);
    }
    rust_observer.stop().await;
    rust_server.stop().await;
}
