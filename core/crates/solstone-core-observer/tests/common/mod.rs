#![allow(dead_code)]

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{Value, json};
use solstone_core_observer::store::record::ObserverRecord;
use solstone_core_observer::store::write::save_observer;

pub fn with_utc_tz<R>(operation: impl FnOnce() -> R) -> R {
    solstone_core_observer::store::format::use_utc_for_differential_tests();
    operation()
}

pub fn root(name: &str) -> PathBuf {
    std::env::temp_dir().join(format!(
        "observer-differential-{name}-{}",
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("time")
            .as_nanos()
    ))
}

pub fn seed(
    root: &Path,
    key: &str,
    name: &str,
    created_at: i64,
    last_seen: Option<i64>,
) -> ObserverRecord {
    let record = ObserverRecord::from_value(json!({"key":key,"name":name,"device_binding":{"device":format!("sha256:{}", "a".repeat(64)),"kind":"cert"},"created_at":created_at,"last_seen":last_seen,"last_segment":null,"last_segment_received_at":null,"last_segment_day":null,"enabled":true,"revoked":false,"revoked_at":null,"stats":{"segments_received":2,"bytes_received":1024}})).expect("record");
    save_observer(root, &record).expect("save record");
    record
}

pub fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("time")
        .as_millis() as i64
}

pub struct FullFixture {
    pub bound_name: &'static str,
    pub unbound_name: &'static str,
    pub excluded_names: [&'static str; 3],
}

pub fn seed_full_fixture(root: &Path, now: i64) -> FullFixture {
    write_record(
        root,
        json!({"key":"aaaaaaaa111", "name":"bound-live", "device_binding":{"device":format!("sha256:{}", "a".repeat(64)),"kind":"cert"}, "created_at":now - 1_800_000, "last_seen":now - 30_000, "last_segment":null, "last_segment_received_at":null, "last_segment_day":null, "enabled":true, "revoked":false, "revoked_at":null, "stats":{"segments_received":2,"bytes_received":1024}}),
    );
    write_record(
        root,
        json!({"key":"bbbbbbbb222", "name":"unbound-stale", "created_at":now - 1_200_000, "last_seen":now - 300_000, "last_segment":null, "last_segment_received_at":null, "last_segment_day":null, "enabled":true, "revoked":false, "revoked_at":null, "stats":{"segments_received":3,"bytes_received":2048}}),
    );
    write_record(
        root,
        json!({"key":"cccccccc333", "name":"revoked-never", "created_at":now - 600_000, "last_seen":null, "last_segment":null, "last_segment_received_at":null, "last_segment_day":null, "enabled":true, "revoked":true, "revoked_at":now - 60_000, "stats":{"segments_received":4,"bytes_received":4096}}),
    );
    write_raw(
        root,
        "dddddddd.json",
        json!({"key":"dddddddd444", "name":"fingerprint-rejected", "fingerprint":"legacy", "created_at":now}),
    );
    write_raw(
        root,
        "eeeeeeee.json",
        json!({"name":"missing-key-rejected", "created_at":now}),
    );
    write_raw(
        root,
        "wrongname.json",
        json!({"key":"ffffffff666", "name":"filename-rejected", "created_at":now}),
    );
    FullFixture {
        bound_name: "bound-live",
        unbound_name: "unbound-stale",
        excluded_names: [
            "fingerprint-rejected",
            "missing-key-rejected",
            "filename-rejected",
        ],
    }
}

pub fn write_record(root: &Path, value: Value) -> ObserverRecord {
    let record = ObserverRecord::from_value(value).expect("record");
    save_observer(root, &record).expect("save record");
    record
}

pub fn write_raw(root: &Path, filename: &str, value: Value) {
    let path = root.join("apps/observer/observers").join(filename);
    fs::create_dir_all(path.parent().expect("parent")).expect("directory");
    fs::write(path, value.to_string()).expect("raw record");
}

pub fn write_history(root: &Path, prefix: &str, day: &str, rows: &[Value]) {
    let path = root
        .join("apps/observer/observers")
        .join(prefix)
        .join("hist")
        .join(format!("{day}.jsonl"));
    fs::create_dir_all(path.parent().expect("parent")).expect("directory");
    let contents = rows
        .iter()
        .map(|row| row.to_string())
        .collect::<Vec<_>>()
        .join("\n")
        + "\n";
    fs::write(path, contents).expect("history");
}

pub fn segment_dir(root: &Path, day: &str, stream: &str, segment: &str) -> PathBuf {
    root.join("chronicle").join(day).join(stream).join(segment)
}

fn sha256_hex(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    format!("{:x}", Sha256::digest(bytes))
}

/// Write a segment with a valid `ingest.json` manifest, its declared media
/// content, and a `stream.json` chain marker -- the shape prune's identity
/// and chain-repair logic reads on both the Rust and Python sides.
pub fn write_segment(
    root: &Path,
    day: &str,
    stream: &str,
    segment: &str,
    seq: u64,
    prev_segment: Option<&str>,
    audio: &[u8],
) -> PathBuf {
    let dir = segment_dir(root, day, stream, segment);
    fs::create_dir_all(&dir).expect("segment dir");
    fs::write(dir.join("audio.flac"), audio).expect("audio");
    let manifest = json!({
        "schema_version": 1,
        "files": {"audio.flac": {"sha256": sha256_hex(audio), "size": audio.len()}},
    });
    fs::write(dir.join("ingest.json"), manifest.to_string()).expect("manifest");
    let marker = json!({
        "stream": stream,
        "prev_day": prev_segment.map(|_| day),
        "prev_segment": prev_segment,
        "seq": seq,
    });
    fs::write(dir.join("stream.json"), marker.to_string()).expect("marker");
    dir
}

pub fn seed_observer_owning_stream(root: &Path, prefix: &str, stream: &str) -> ObserverRecord {
    write_record(
        root,
        json!({"key": format!("{prefix}12345678"), "name": stream, "stream": stream}),
    )
}

pub fn python() -> PathBuf {
    let root = repository_root();
    let venv = root.join(".venv/bin/python3");
    if venv.is_file() {
        venv
    } else {
        PathBuf::from("python3")
    }
}
pub fn repository_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(3)
        .expect("repo root")
        .to_path_buf()
}

pub fn oracle(
    root: &Path,
    operation: &str,
    json_output: bool,
    now_ms: i64,
    identifier: Option<&str>,
) -> Value {
    let script = r#"import contextlib, importlib.util, io, json, os, sys, types, time
time.tzset()
spec = importlib.util.spec_from_file_location('observer_cli_oracle', os.path.join(os.environ['SOLSTONE_REPO_ROOT'], 'solstone/observe/observer_cli.py'))
module = importlib.util.module_from_spec(spec)
spec.loader.exec_module(module)
module.now_ms = lambda: int(os.environ['OBSERVER_NOW_MS'])
args = json.load(sys.stdin)
out, err = io.StringIO(), io.StringIO()
with contextlib.redirect_stdout(out), contextlib.redirect_stderr(err):
    if args['operation'] == 'list': code = module.cmd_list(types.SimpleNamespace(json_output=args['json']))
    elif args['operation'] == 'status_all': code = module._status_all(json_output=args['json'])
    elif args['operation'] == 'status_single': code = module._status_single(args['identifier'], json_output=args['json'])
    else: code = module.reconcile_observers(dry_run=True)
json.dump({'code': code, 'stdout': out.getvalue(), 'stderr': err.getvalue()}, sys.stdout)
"#;
    let input = json!({"operation":operation,"json":json_output,"identifier":identifier});
    let repository = repository_root();
    let mut child = Command::new(python())
        .args(["-c", script])
        .env("SOLSTONE_REPO_ROOT", &repository)
        .env("PYTHONPATH", &repository)
        .env("SOLSTONE_JOURNAL", root)
        .env("OBSERVER_NOW_MS", now_ms.to_string())
        .env("TZ", "UTC")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .expect("Python oracle");
    child
        .stdin
        .take()
        .expect("stdin")
        .write_all(input.to_string().as_bytes())
        .expect("input");
    let output = child.wait_with_output().expect("output");
    assert!(
        output.status.success(),
        "Python failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).expect("oracle JSON")
}

/// Run the Python `prune.py` reference directly (bypassing `observer_cli.py`'s
/// argparse layer, matching the Rust side's direct `run_prune`/`format_result`
/// entry points) and return its `{code, stdout, stderr}` triple.
pub fn oracle_prune(root: &Path, days: &[String], stream: Option<&str>, execute: bool) -> Value {
    let script = r#"import contextlib, importlib.util, io, json, os, sys, time
time.tzset()
spec = importlib.util.spec_from_file_location('prune_oracle', os.path.join(os.environ['SOLSTONE_REPO_ROOT'], 'solstone/apps/observer/prune.py'))
module = importlib.util.module_from_spec(spec)
# prune.py defines @dataclass(frozen=True) classes at module scope; dataclass's
# forward-ref resolution looks the module up via sys.modules[cls.__module__],
# which is None unless the module is registered before exec_module runs.
sys.modules['prune_oracle'] = module
spec.loader.exec_module(module)
args = json.load(sys.stdin)
out, err = io.StringIO(), io.StringIO()
with contextlib.redirect_stdout(out), contextlib.redirect_stderr(err):
    result = module.run_prune(days=args['days'], stream=args['stream'], execute=args['execute'], cross_start=False)
    text = module.format_result(result)
    code = module.result_exit_code(result)
json.dump({'code': code, 'stdout': text, 'stderr': err.getvalue()}, sys.stdout)
"#;
    let input = json!({"days": days, "stream": stream, "execute": execute});
    let repository = repository_root();
    let mut child = Command::new(python())
        .args(["-c", script])
        .env("SOLSTONE_REPO_ROOT", &repository)
        .env("PYTHONPATH", &repository)
        .env("SOLSTONE_JOURNAL", root)
        .env("TZ", "UTC")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .expect("Python prune oracle");
    child
        .stdin
        .take()
        .expect("stdin")
        .write_all(input.to_string().as_bytes())
        .expect("input");
    let output = child.wait_with_output().expect("output");
    assert!(
        output.status.success(),
        "Python prune oracle failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).expect("prune oracle JSON")
}

pub fn python_json(root: &Path, script: &str, input: Value) -> Value {
    let repository = repository_root();
    let mut child = Command::new(python())
        .args(["-c", script])
        .env("SOLSTONE_REPO_ROOT", &repository)
        .env("PYTHONPATH", &repository)
        .env("SOLSTONE_JOURNAL", root)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .expect("Python helper");
    child
        .stdin
        .take()
        .expect("stdin")
        .write_all(input.to_string().as_bytes())
        .expect("input");
    let output = child.wait_with_output().expect("output");
    assert!(
        output.status.success(),
        "Python failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).expect("Python JSON")
}

pub fn cleanup(root: PathBuf) {
    fs::remove_dir_all(root).expect("cleanup");
}
