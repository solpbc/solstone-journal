// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::{
    fs,
    path::{Path, PathBuf},
    sync::LazyLock,
};

use axum::{
    Router,
    body::{Body, to_bytes},
    http::{Request, StatusCode},
};
use chrono::{Days, Local};
use serde_json::{Value, json};
use tempfile::TempDir;
use tokio::sync::{Mutex, MutexGuard};
use tower::ServiceExt;

use solstone_core_retention::policy::{policy_from_retention, policy_would_release};

const STUB_EXECUTOR: &str = r#"#!/bin/sh
for _a in "$@"; do printf '%s\n' "$_a" >> "$ORACLE_STUB_LOG"; done
printf '%s\n' '--END--' >> "$ORACLE_STUB_LOG"
REG='{"marks":{"m-0001":{"class":"policy_raw_release","target":{"day":"20260810","stream":"tmux","dir":"090000_300","name":"audio.flac"}},"m-0002":{"class":"policy_raw_release","target":{"day":"20260811","stream":"screen","dir":"090000_120","name":"video.webm"}},"m-0003":{"class":"operator_request","target":{"day":"20260811","stream":"screen","dir":"090000_120","name":"other.webm"}}}}'
PLAN='{"skipped_segments":[{"stream":"tmux","day":"20260810","dir":"090000_300","reason":"held_by_operator"},{"stream":"screen","day":"20260811","dir":"090000_120","reason":"no_media"}],"unreadable_days":[]}'
if [ -n "$ORACLE_STUB_EXIT" ] && [ "$ORACLE_STUB_EXIT" != "0" ]; then
  printf '%s' "{\"marks\":$REG}"
  exit "$ORACLE_STUB_EXIT"
fi
if [ "$1" = "marks" ]; then
  printf '%s' "{\"marks\":$REG}"
else
  printf '%s' "{\"marks\":$REG,\"plan\":$PLAN}"
fi
"#;

static EXECUTOR_ENV_LOCK: LazyLock<Mutex<()>> = LazyLock::new(|| Mutex::new(()));

pub(crate) fn executor_env_guard() -> MutexGuard<'static, ()> {
    EXECUTOR_ENV_LOCK.blocking_lock()
}

/// Pin the environment so `solstone-retention` is provably unresolvable.
///
/// AC 15 generalized: any test that replays a recorded case whose expected
/// value depends on executor availability must establish that availability
/// itself, rather than inheriting whatever the host happens to have installed.
///
/// The caller must already hold [`executor_env_guard`]. This deliberately does
/// not take the lock itself so it can nest inside a test that has staged a
/// hostile ambient environment.
pub(crate) fn without_executor<T>(work: impl FnOnce() -> T) -> T {
    let empty = tempfile::Builder::new()
        .prefix("no-executor-")
        .tempdir()
        .expect("executor-free PATH directory");
    let binary = crate::retention_executor::BINARY;
    assert!(
        !empty.path().join(binary).exists(),
        "the pinned PATH directory must not contain {binary}"
    );
    let path = empty.path().display().to_string();
    temp_env::with_vars(
        [
            ("SOLSTONE_RETENTION_BIN", None),
            ("PATH", Some(path.as_str())),
            ("ORACLE_STUB_LOG", None),
            ("ORACLE_STUB_EXIT", None),
        ],
        work,
    )
}

// These variables are process-global. This lock keeps focused runs honest even
// without the Makefile's serialized test setting; temp-env restores all four
// variables on scope exit, including unwinding from a panic.
struct ExecutorEnvironment {
    _serialized: MutexGuard<'static, ()>,
}

impl ExecutorEnvironment {
    fn install<T>(
        binary: Option<&Path>,
        path: &Path,
        log: &Path,
        exit: Option<&str>,
        work: impl FnOnce() -> T,
    ) -> T {
        let guard = Self {
            _serialized: EXECUTOR_ENV_LOCK.blocking_lock(),
        };
        let binary = binary.map(|path| path.display().to_string());
        let path = path.display().to_string();
        let log = log.display().to_string();
        let result = temp_env::with_vars(
            [
                ("SOLSTONE_RETENTION_BIN", binary.as_deref()),
                ("PATH", Some(path.as_str())),
                ("ORACLE_STUB_LOG", Some(log.as_str())),
                ("ORACLE_STUB_EXIT", exit),
            ],
            work,
        );
        drop(guard);
        result
    }
}

struct Harness {
    root: TempDir,
    stub: PathBuf,
    log: PathBuf,
}

impl Harness {
    fn new(retention: &Value) -> Self {
        let root = tempfile::Builder::new()
            .prefix("retention-")
            .tempdir()
            .expect("space-free temp journal");
        assert!(!root.path().display().to_string().contains(' '));
        let config = json!({
            "setup": {"completed_at": 1_700_000_000_000_i64},
            "retention": retention,
        });
        write_json(root.path(), "config/journal.json", &config);
        seed_chronicle(root.path());
        let stub = root.path().join("stub-retention");
        fs::write(&stub, STUB_EXECUTOR.as_bytes()).expect("stub bytes");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&stub, fs::Permissions::from_mode(0o755)).expect("stub mode");
        }
        let log = root.path().join("invocations.log");
        Self { root, stub, log }
    }

    fn router(&self) -> Router {
        crate::test_support::shell_router(self.root.path())
    }

    fn media(&self) -> Value {
        let mut output = serde_json::Map::new();
        for relative in seeded_media_paths() {
            let bytes = fs::read(self.root.path().join(relative)).expect("seeded media");
            output.insert(relative.to_owned(), json!(short_digest(&bytes)));
        }
        Value::Object(output)
    }

    fn invocations(&self) -> Vec<Vec<String>> {
        let Ok(text) = fs::read_to_string(&self.log) else {
            return Vec::new();
        };
        let mut invocations = Vec::new();
        let mut current = Vec::new();
        for line in text.lines() {
            if line == "--END--" {
                if !current.is_empty() {
                    invocations.push(normalize_argv(std::mem::take(&mut current)));
                }
            } else {
                current.push(line.to_owned());
            }
        }
        if !current.is_empty() {
            invocations.push(normalize_argv(current));
        }
        invocations
    }
}

fn write_json(root: &Path, relative: &str, value: &Value) {
    let path = root.join(relative);
    fs::create_dir_all(path.parent().expect("parent")).expect("directory");
    fs::write(
        path,
        format!("{}\n", serde_json::to_string_pretty(value).expect("JSON")),
    )
    .expect("JSON write");
}

fn seed_chronicle(root: &Path) {
    let with_raw = root.join("chronicle/20260810/tmux/090000_300");
    let purged = root.join("chronicle/20260810/tmux/100000_300");
    let bare = root.join("chronicle/20260811/screen/090000_120");
    for path in [&with_raw, &purged, &bare] {
        fs::create_dir_all(path).expect("segment directory");
    }
    fs::write(with_raw.join("audio.flac"), vec![0_u8; 4096]).expect("audio");
    fs::write(with_raw.join("monitor_1_diff.png"), vec![0_u8; 2048]).expect("monitor");
    fs::write(with_raw.join("audio.jsonl"), b"{\"seeded\": true}\n").expect("sidecar");
    fs::write(purged.join("audio.jsonl"), b"{\"seeded\": true}\n").expect("purged sidecar");
    fs::write(bare.join("notes.md"), b"seeded\n").expect("notes");
    for name in ["tmux", "screen"] {
        write_json(
            root,
            &format!("streams/{name}.json"),
            &json!({"name":name,"seq":1}),
        );
    }
}

fn seeded_media_paths() -> [&'static str; 5] {
    [
        "chronicle/20260810/tmux/090000_300/audio.flac",
        "chronicle/20260810/tmux/090000_300/audio.jsonl",
        "chronicle/20260810/tmux/090000_300/monitor_1_diff.png",
        "chronicle/20260810/tmux/100000_300/audio.jsonl",
        "chronicle/20260811/screen/090000_120/notes.md",
    ]
}

fn short_digest(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    format!("{:x}", Sha256::digest(bytes))
        .chars()
        .take(16)
        .collect()
}

fn normalize_argv(argv: Vec<String>) -> Vec<String> {
    let mut output = Vec::new();
    let mut iter = argv.into_iter();
    while let Some(argument) = iter.next() {
        output.push(argument.clone());
        if let Some(value) = match argument.as_str() {
            "--journal" => Some("<JOURNAL_ROOT>"),
            "--today" => Some("<VOLATILE:today>"),
            "--now" => Some("<VOLATILE:now>"),
            _ => None,
        } {
            iter.next().expect("flag value");
            output.push(value.to_owned());
        }
    }
    output
}

pub(crate) fn run_async<T>(work: impl std::future::Future<Output = T>) -> T {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("runtime")
        .block_on(work)
}

fn request(method: &str, path: &str, sent: Option<&Value>) -> Request<Body> {
    let request = Request::builder().method(method).uri(path);
    match sent {
        Some(value) => request
            .header("content-type", "application/json")
            .body(Body::from(serde_json::to_vec(value).expect("request JSON")))
            .expect("request"),
        None => request.body(Body::empty()).expect("request"),
    }
}

async fn send(
    router: Router,
    method: &str,
    path: &str,
    sent: Option<&Value>,
) -> (StatusCode, Value) {
    let response = router
        .oneshot(request(method, path, sent))
        .await
        .expect("response");
    let status = response.status();
    let body = serde_json::from_slice(
        &to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("body"),
    )
    .expect("JSON body");
    (status, body)
}

fn assert_corpus_response(
    case: &str,
    record: &Value,
    root: &Path,
    status: StatusCode,
    body: Value,
) {
    assert_eq!(
        status.as_u16(),
        record["status"].as_u64().expect("status") as u16,
        "{case}"
    );
    let (normalized, _) = crate::corpus::normalize(body, "", &root.display().to_string());
    assert_eq!(
        crate::corpus::digest(&normalized),
        record["digest"],
        "{case}"
    );
}

fn stub_case<'a>(corpus: &'a Value, name: &str) -> &'a Value {
    &corpus["purge_stubbed"][format!("POST {name}")]
}

fn root_from_config(config: &Value) -> TempDir {
    let root = tempfile::Builder::new()
        .prefix("config-")
        .tempdir()
        .expect("temporary config root");
    write_json(root.path(), "config/journal.json", config);
    root
}

#[test]
fn ac4_stubbed_purge_never_marks_on_a_non_releasing_policy() {
    let corpus = crate::test_support::corpus();
    let keep = [
        "stub.keep-policy",
        "stub.keep-policy.stream-filter",
        "stub.days-policy.unparseable",
        "stub.days-policy.zero",
        "stub.executor-refused",
        "stub.executor-halted",
    ];
    let release = [
        "stub.days-policy",
        "stub.days-policy.bool-true",
        "stub.processed-policy",
        "stub.keep-default.releasing-per-stream",
    ];
    assert_eq!(keep.len() + release.len(), 10);
    for name in keep.into_iter().chain(release) {
        let record = stub_case(&corpus, name);
        let harness = Harness::new(&record["retention_config"]);
        let exit = record["stub_exit_code"].as_str();
        ExecutorEnvironment::install(
            Some(&harness.stub),
            harness.root.path(),
            &harness.log,
            exit,
            || {
                run_async(send(
                    harness.router(),
                    "POST",
                    "/app/settings/api/storage/list",
                    Some(&record["sent"]),
                ))
            },
        );
        let invocations = harness.invocations();
        let verbs = invocations
            .iter()
            .map(|argv| argv.first().expect("verb").as_str())
            .collect::<Vec<_>>();
        let expected = record["executor_verbs"]
            .as_array()
            .expect("verbs")
            .iter()
            .map(Value::as_str)
            .collect::<Option<Vec<_>>>()
            .expect("string verbs");
        assert_eq!(verbs, expected, "{name} verb sequence");
        if keep.contains(&name) {
            assert_eq!(verbs, ["marks"], "{name} must never mark");
        } else {
            assert_eq!(verbs, ["marks", "mark"], "{name} must mark");
        }
    }
}

#[test]
fn ac10_stubbed_purge_replays_recorded_status_and_digest() {
    let corpus = crate::test_support::corpus();
    for name in [
        "stub.keep-policy",
        "stub.keep-policy.stream-filter",
        "stub.days-policy",
        "stub.days-policy.bool-true",
        "stub.processed-policy",
        "stub.days-policy.unparseable",
        "stub.days-policy.zero",
        "stub.keep-default.releasing-per-stream",
        "stub.executor-refused",
        "stub.executor-halted",
    ] {
        let record = stub_case(&corpus, name);
        let harness = Harness::new(&record["retention_config"]);
        let exit = record["stub_exit_code"].as_str();
        let (status, body) = ExecutorEnvironment::install(
            Some(&harness.stub),
            harness.root.path(),
            &harness.log,
            exit,
            || {
                run_async(send(
                    harness.router(),
                    "POST",
                    "/app/settings/api/storage/list",
                    Some(&record["sent"]),
                ))
            },
        );
        assert_corpus_response(name, record, harness.root.path(), status, body.clone());
        match name {
            "stub.keep-policy" => assert_eq!(body["standing_total"], 2),
            "stub.keep-policy.stream-filter" => assert_eq!(body["standing_total"], 1),
            _ => {}
        }
    }
}

#[test]
fn ac5_releasing_policy_argv_is_semantically_equal_to_the_corpus() {
    let corpus = crate::test_support::corpus();
    for name in [
        "stub.keep-default.releasing-per-stream",
        "stub.days-policy",
        "stub.days-policy.bool-true",
        "stub.processed-policy",
    ] {
        let record = stub_case(&corpus, name);
        let harness = Harness::new(&record["retention_config"]);
        ExecutorEnvironment::install(
            Some(&harness.stub),
            harness.root.path(),
            &harness.log,
            None,
            || {
                run_async(send(
                    harness.router(),
                    "POST",
                    "/app/settings/api/storage/list",
                    Some(&record["sent"]),
                ));
            },
        );
        let observed = harness.invocations();
        let expected = record["executor_invocations"]
            .as_array()
            .expect("invocations");
        let observed_policy = observed[1]
            .iter()
            .position(|argument| argument == "--policy")
            .map(|index| &observed[1][index + 1])
            .expect("observed policy");
        let expected_policy = expected[1]
            .as_array()
            .expect("expected argv")
            .iter()
            .position(|argument| argument == "--policy")
            .and_then(|index| expected[1][index + 1].as_str())
            .expect("expected policy");
        let observed_policy: Value = serde_json::from_str(observed_policy).expect("observed JSON");
        let expected_policy: Value = serde_json::from_str(expected_policy).expect("expected JSON");
        assert_eq!(observed_policy, expected_policy, "{name}");
        assert!(
            observed_policy["per_stream"].is_array(),
            "{name} pair array"
        );
        assert_eq!(
            observed_policy["default_rule"],
            expected_policy["default_rule"]
        );
        assert_eq!(
            observed_policy["minimum_age"],
            expected_policy["minimum_age"]
        );
        assert_eq!(observed_policy["enabled"], expected_policy["enabled"]);
    }
}

#[test]
fn ac7_and_ac11_successful_purge_keeps_media_and_drops_unknown_skip_reasons() {
    let corpus = crate::test_support::corpus();
    for (name, record) in corpus["purge_stubbed"].as_object().expect("stubbed cases") {
        if record["status"] != 200 {
            continue;
        }
        let harness = Harness::new(&record["retention_config"]);
        let before = harness.media();
        let (status, body) = ExecutorEnvironment::install(
            Some(&harness.stub),
            harness.root.path(),
            &harness.log,
            None,
            || {
                run_async(send(
                    harness.router(),
                    "POST",
                    "/app/settings/api/storage/list",
                    Some(&record["sent"]),
                ))
            },
        );
        assert_eq!(status, StatusCode::OK, "{name}");
        assert_eq!(harness.media(), before, "{name} media bytes");
        assert_eq!(
            harness.media(),
            record["media_digests_after"],
            "{name} media digest"
        );
        if name == "POST stub.days-policy" {
            assert_eq!(body["held"], json!([]));
            assert_eq!(body["no_media"].as_array().map(Vec::len), Some(1));
        }
    }
}

#[test]
fn ac9_refused_stub_receipts_preserve_media_and_carry_the_refusal_summary() {
    let corpus = crate::test_support::corpus();
    for name in ["stub.executor-refused", "stub.executor-halted"] {
        let record = stub_case(&corpus, name);
        let harness = Harness::new(&record["retention_config"]);
        let before = harness.media();
        let (status, body) = ExecutorEnvironment::install(
            Some(&harness.stub),
            harness.root.path(),
            &harness.log,
            record["stub_exit_code"].as_str(),
            || {
                run_async(send(
                    harness.router(),
                    "POST",
                    "/app/settings/api/storage/list",
                    Some(&record["sent"]),
                ))
            },
        );
        assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR, "{name}");
        assert_eq!(
            body["detail"],
            "could not build the list: the retention tool refused without naming an entry."
        );
        assert_eq!(harness.media(), before, "{name} media bytes");
    }
}

#[test]
fn ac3_purge_without_an_executor_matches_refusal_records() {
    let corpus = crate::test_support::corpus();
    for (name, record) in corpus["purge"].as_object().expect("purge cases") {
        let harness = Harness::new(&record["retention_config"]);
        assert!(!harness.root.path().join("solstone-retention").exists());
        let before = harness.media();
        let sent = (!record["sent"].is_null()).then_some(&record["sent"]);
        let (status, body) =
            ExecutorEnvironment::install(None, harness.root.path(), &harness.log, None, || {
                run_async(send(
                    harness.router(),
                    "POST",
                    "/app/settings/api/storage/list",
                    sent,
                ))
            });
        assert_corpus_response(name, record, harness.root.path(), status, body.clone());
        assert!(
            harness.invocations().is_empty(),
            "{name} executor invocations"
        );
        assert_eq!(harness.media(), before, "{name} media bytes");
        assert_eq!(
            harness.media(),
            record["media_digests_after"],
            "{name} media digest"
        );
        if record["sent"].is_array() {
            assert_eq!(status, StatusCode::BAD_REQUEST, "{name}");
        } else {
            assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR, "{name}");
            assert_eq!(body["detail"], record["normalized"]["detail"], "{name}");
        }
    }
}

#[test]
fn ac14_true_days_releases_with_period_one() {
    let corpus = crate::test_support::corpus();
    let record = stub_case(&corpus, "stub.days-policy.bool-true");
    let harness = Harness::new(&record["retention_config"]);
    ExecutorEnvironment::install(
        Some(&harness.stub),
        harness.root.path(),
        &harness.log,
        None,
        || {
            run_async(send(
                harness.router(),
                "POST",
                "/app/settings/api/storage/list",
                Some(&record["sent"]),
            ));
        },
    );
    let invocations = harness.invocations();
    assert_eq!(
        invocations
            .iter()
            .map(|argv| argv[0].as_str())
            .collect::<Vec<_>>(),
        ["marks", "mark"]
    );
    let index = invocations[1]
        .iter()
        .position(|argument| argument == "--policy")
        .expect("policy flag");
    let policy: Value = serde_json::from_str(&invocations[1][index + 1]).expect("policy JSON");
    assert_eq!(policy["default_rule"]["period"], 1);
}

#[test]
fn ac2_per_stream_replaces_the_prior_map_including_tmux() {
    let corpus = crate::test_support::corpus();
    let record = &corpus["mutations"]["PUT storage.per-stream"];
    let root = root_from_config(&record["config_before"]);
    let (status, body) = run_async(send(
        crate::test_support::shell_router(root.path()),
        "PUT",
        "/app/settings/api/storage",
        Some(&record["sent"]),
    ));
    assert_corpus_response("PUT storage.per-stream", record, root.path(), status, body);
    let config: Value = serde_json::from_slice(
        &fs::read(root.path().join("config/journal.json")).expect("config after"),
    )
    .expect("config JSON");
    // The full conformance assertion already implies this, but the deletion is
    // the behavior this route must preserve, so state the absence directly.
    assert!(config["retention"]["per_stream"].get("tmux").is_none());
}

#[test]
fn ac8_prune_unavailable_is_generic_while_purge_carries_the_tool_detail() {
    let corpus = crate::test_support::corpus();
    let purge_record = &corpus["purge"]["POST keep-policy.no-filter"];
    let prune_record = &corpus["mutations"]["POST prune-logs.dry-run"];
    let harness = Harness::new(&purge_record["retention_config"]);
    assert!(!harness.root.path().join("solstone-retention").exists());
    let (prune_status, prune_body, purge_status, purge_body) =
        ExecutorEnvironment::install(None, harness.root.path(), &harness.log, None, || {
            run_async(async {
                let prune = send(
                    harness.router(),
                    "POST",
                    "/app/settings/api/storage/prune-logs",
                    Some(&prune_record["sent"]),
                )
                .await;
                let purge = send(
                    harness.router(),
                    "POST",
                    "/app/settings/api/storage/list",
                    Some(&purge_record["sent"]),
                )
                .await;
                (prune.0, prune.1, purge.0, purge.1)
            })
        });
    assert_corpus_response(
        "POST prune-logs.dry-run",
        prune_record,
        harness.root.path(),
        prune_status,
        prune_body.clone(),
    );
    assert_eq!(crate::corpus::digest(&prune_body), "2bb002e7fa1ad610");
    assert_corpus_response(
        "POST keep-policy.no-filter",
        purge_record,
        harness.root.path(),
        purge_status,
        purge_body.clone(),
    );
    assert_ne!(prune_body, purge_body);
    assert!(
        prune_body["detail"]
            .as_str()
            .is_some_and(|detail| !detail.contains("solstone-retention"))
    );
    assert!(
        purge_body["detail"]
            .as_str()
            .is_some_and(|detail| detail.contains("solstone-retention"))
    );
}

#[test]
fn prune_logs_disabled_is_a_noop_without_an_executor() {
    let harness = Harness::new(&json!({
        "journal_logs": {"enabled": false, "days": 14},
    }));
    assert!(!harness.root.path().join("solstone-retention").exists());
    let (status, body) =
        ExecutorEnvironment::install(None, harness.root.path(), &harness.log, None, || {
            run_async(send(
                harness.router(),
                "POST",
                "/app/settings/api/storage/prune-logs",
                Some(&json!({"dry_run": false})),
            ))
        });
    let cutoff_day = Local::now()
        .date_naive()
        .checked_sub_days(Days::new(14))
        .expect("cutoff date")
        .format("%Y%m%d")
        .to_string();
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        body,
        json!({
            "enabled": false,
            "dry_run": false,
            "days": 14,
            "cutoff_day": cutoff_day,
            "files_deleted": 0,
            "dirs_deleted": 0,
            "bytes_freed": 0,
            "bytes_freed_human": "0 B",
            "by_class": {},
            "by_day": {},
            "retention_log": {},
            "errors": [],
            "audit_written": false,
            "partial_error": false,
        })
    );
    assert!(harness.invocations().is_empty());
}

#[test]
fn ac12_malformed_retention_writes_preserve_the_reference_asymmetry() {
    let corpus = crate::test_support::corpus();
    for name in [
        "PUT storage.retention",
        "PUT storage.journal-logs",
        "PUT storage.per-stream",
        "PUT storage.raw-days-true",
    ] {
        let record = &corpus["mutations_malformed"][name];
        let root = root_from_config(&record["config_before"]);
        let (status, body) = run_async(send(
            crate::test_support::shell_router(root.path()),
            record["method"].as_str().expect("method"),
            record["path"].as_str().expect("path"),
            Some(&record["sent"]),
        ));
        assert_corpus_response(name, record, root.path(), status, body);
        let after: Value = serde_json::from_slice(
            &fs::read(root.path().join("config/journal.json")).expect("config after"),
        )
        .expect("config JSON");
        assert_eq!(after, record["config_after"], "{name}");
        match name {
            "PUT storage.journal-logs" | "PUT storage.per-stream" => {
                assert_eq!(after["retention"]["raw_media"], "sometimes");
                assert_eq!(after["retention"]["raw_media_days"], "ninety");
            }
            "PUT storage.raw-days-true" => {
                assert_eq!(after["retention"]["raw_media"], "sometimes");
                assert_eq!(after["retention"]["raw_media_days"], true);
            }
            "PUT storage.retention" => {
                // This write overwrites both malformed fields, so it cannot
                // carry the signal and its malformed digest matches clean config.
            }
            _ => unreachable!("enumerated case"),
        }
    }
}

#[test]
fn ac6_policy_would_release_has_python_zero_polarity() {
    for (retention, expected) in [
        (
            json!({"raw_media":"days","raw_media_days":30,"empty_audio":"keep"}),
            true,
        ),
        (json!({"raw_media":"processed","empty_audio":"keep"}), true),
        (json!({"raw_media":"keep","empty_audio":"keep"}), false),
        (
            json!({"raw_media":"days","raw_media_days":0,"empty_audio":"keep"}),
            false,
        ),
        (
            json!({"raw_media":"days","raw_media_days":"ninety","empty_audio":"keep"}),
            false,
        ),
        (
            json!({"raw_media":"days","raw_media_days":-1,"empty_audio":"keep"}),
            false,
        ),
        (
            json!({"raw_media":"days","raw_media_days":true,"empty_audio":"keep"}),
            true,
        ),
        (
            json!({"raw_media":"days","raw_media_days":7.5,"empty_audio":"keep"}),
            false,
        ),
        (
            json!({"raw_media":"days","raw_media_days":" 30 ","empty_audio":"keep"}),
            true,
        ),
        (json!({"raw_media":"keep"}), true),
        (json!({"raw_media":"keep","empty_audio":"processed"}), true),
        (
            json!({"raw_media":"keep","empty_audio":"days","empty_audio_days":7}),
            true,
        ),
        (
            json!({"raw_media":"keep","empty_audio":"days","empty_audio_days":0}),
            false,
        ),
    ] {
        assert_eq!(
            policy_would_release(&policy_from_retention(
                retention.as_object().expect("retention")
            )),
            expected
        );
    }
}

#[test]
fn ordinary_raw_media_save_leaves_empty_audio_keep() {
    let root = root_from_config(&json!({
        "setup": {"completed_at": 1_700_000_000_000_i64},
        "retention": {
            "raw_media": "keep",
            "raw_media_days": null,
            "empty_audio": "keep",
            "empty_audio_days": null
        }
    }));
    let (status, _body) = run_async(send(
        crate::test_support::shell_router(root.path()),
        "PUT",
        "/app/settings/api/storage",
        Some(&json!({"raw_media": "days", "raw_media_days": 30})),
    ));
    assert_eq!(status, StatusCode::OK);
    let config: Value = serde_json::from_slice(
        &fs::read(root.path().join("config/journal.json")).expect("config after"),
    )
    .expect("config JSON");
    assert_eq!(config["retention"]["raw_media"], "days");
    assert_eq!(config["retention"]["raw_media_days"], 30);
    assert_eq!(config["retention"]["empty_audio"], "keep");
    assert_eq!(config["retention"]["empty_audio_days"], Value::Null);
}

#[test]
fn ac6_fractional_minimum_age_rounds_up() {
    let policy = policy_from_retention(
        json!({"raw_media":"days","raw_media_days":30,"raw_media_minimum_days":7.5})
            .as_object()
            .expect("retention"),
    );
    assert_eq!(policy.minimum_age.0, 8);
}
