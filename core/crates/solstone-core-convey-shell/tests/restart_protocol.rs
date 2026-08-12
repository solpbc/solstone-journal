// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

#![cfg(unix)]

use std::collections::VecDeque;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::Deserialize;
use serde_json::{Map, json};
use solstone_core_callosum::CallosumEnvelope;
use solstone_core_convey_shell::{
    RestartConveyOptions, RestartTransport, restart_convey, restart_convey_with_transport,
};

struct Journal(PathBuf);
impl Journal {
    fn new() -> Self {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "solstone-restart-protocol-{}-{stamp}",
            std::process::id()
        ));
        fs::create_dir_all(path.join("health")).expect("health directory");
        Self(path)
    }
    fn path(&self) -> &Path {
        &self.0
    }
    fn port(&self, text: &str) {
        fs::write(self.path().join("health/convey.port"), text).expect("port writes");
    }
}

#[derive(Deserialize)]
struct Corpus {
    commands: Commands,
}

#[derive(Deserialize)]
struct Commands {
    restart_convey: Grammar,
}

#[derive(Deserialize)]
struct Grammar {
    cases: Vec<Case>,
}

#[derive(Deserialize)]
struct Case {
    label: String,
    #[serde(default)]
    stdout: String,
    #[serde(default)]
    stderr: String,
}

fn template(label: &str) -> Case {
    serde_json::from_str::<Corpus>(include_str!(
        "../../../fixtures/convey_restart_reference_grammar.json"
    ))
    .expect("frozen grammar corpus parses")
    .commands
    .restart_convey
    .cases
    .into_iter()
    .find(|case| case.label == label)
    .expect("restart template exists")
}

fn render_template(value: &str, journal: &Journal) -> String {
    value.replace("{journal}", &journal.path().display().to_string())
}

impl Drop for Journal {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[derive(Clone, Copy)]
enum Event {
    Restarting,
    Stopped,
    Started,
    Log,
    StdoutLog,
    PreRequestLookalike,
    OtherRestart,
    Uncorrelated,
    OtherService,
    OtherMessage,
}

struct Peer {
    events: VecDeque<Event>,
    restart_id: Option<String>,
    write_port_on_send: Option<String>,
    fail_send: bool,
    elapsed: VecDeque<Duration>,
}

impl Peer {
    fn one(event: Event) -> Self {
        Self {
            events: VecDeque::from([event]),
            restart_id: None,
            write_port_on_send: None,
            fail_send: false,
            elapsed: VecDeque::new(),
        }
    }
    fn envelope(&self, event: Event) -> CallosumEnvelope {
        let mut extra = Map::new();
        match event {
            Event::Restarting => {
                extra.insert("service".into(), json!("convey"));
                extra.insert(
                    "restart_id".into(),
                    json!(self.restart_id.as_deref().unwrap_or_default()),
                );
                CallosumEnvelope {
                    tract: "supervisor".into(),
                    event: "restarting".into(),
                    ts: None,
                    extra,
                }
            }
            Event::Stopped => {
                extra.insert("service".into(), json!("convey"));
                extra.insert("exit_code".into(), json!(0));
                extra.insert(
                    "restart_id".into(),
                    json!(self.restart_id.as_deref().unwrap_or_default()),
                );
                CallosumEnvelope {
                    tract: "supervisor".into(),
                    event: "stopped".into(),
                    ts: None,
                    extra,
                }
            }
            Event::Started => {
                extra.insert("service".into(), json!("convey"));
                extra.insert("pid".into(), json!(321));
                extra.insert("ref".into(), json!("restart-ref"));
                extra.insert(
                    "restart_id".into(),
                    json!(self.restart_id.as_deref().unwrap_or_default()),
                );
                CallosumEnvelope {
                    tract: "supervisor".into(),
                    event: "started".into(),
                    ts: None,
                    extra,
                }
            }
            Event::Log | Event::StdoutLog => {
                extra.insert(
                    "restart_id".into(),
                    json!(self.restart_id.as_deref().unwrap_or_default()),
                );
                extra.insert("line".into(), json!("fixture log"));
                extra.insert(
                    "stream".into(),
                    json!(if matches!(event, Event::Log) {
                        "stderr"
                    } else {
                        "stdout"
                    }),
                );
                CallosumEnvelope {
                    tract: "logs".into(),
                    event: "line".into(),
                    ts: Some(1_700_000_000_000),
                    extra,
                }
            }
            Event::PreRequestLookalike => {
                extra.insert("service".into(), json!("convey"));
                extra.insert("ref".into(), json!("supervisor-app-convey"));
                CallosumEnvelope {
                    tract: "supervisor".into(),
                    event: "started".into(),
                    ts: None,
                    extra,
                }
            }
            Event::OtherRestart => {
                extra.insert("service".into(), json!("convey"));
                extra.insert("restart_id".into(), json!("other"));
                CallosumEnvelope {
                    tract: "supervisor".into(),
                    event: "started".into(),
                    ts: None,
                    extra,
                }
            }
            Event::Uncorrelated => {
                extra.insert("service".into(), json!("convey"));
                CallosumEnvelope {
                    tract: "supervisor".into(),
                    event: "started".into(),
                    ts: None,
                    extra,
                }
            }
            Event::OtherService => {
                extra.insert("service".into(), json!("sense"));
                extra.insert(
                    "restart_id".into(),
                    json!(self.restart_id.as_deref().unwrap_or_default()),
                );
                CallosumEnvelope {
                    tract: "supervisor".into(),
                    event: "started".into(),
                    ts: None,
                    extra,
                }
            }
            Event::OtherMessage => {
                extra.insert(
                    "restart_id".into(),
                    json!(self.restart_id.as_deref().unwrap_or_default()),
                );
                CallosumEnvelope {
                    tract: "observe".into(),
                    event: "observed".into(),
                    ts: None,
                    extra,
                }
            }
        }
    }
}

struct IdlePeer(Peer);

impl RestartTransport for IdlePeer {
    fn send_restart(&mut self, restart_id: &str) -> Result<(), String> {
        self.0.send_restart(restart_id)
    }
    fn start_timer(&mut self) {
        self.0.start_timer();
    }
    fn elapsed(&mut self) -> Duration {
        self.0.elapsed()
    }
    fn next_event(&mut self, _: Duration) -> Result<Option<CallosumEnvelope>, String> {
        Ok(self
            .0
            .events
            .pop_front()
            .map(|event| self.0.envelope(event)))
    }
}

impl RestartTransport for Peer {
    fn send_restart(&mut self, restart_id: &str) -> Result<(), String> {
        if self.fail_send {
            return Err("delivery failure".into());
        }
        self.restart_id = Some(restart_id.into());
        Ok(())
    }
    fn start_timer(&mut self) {}
    fn elapsed(&mut self) -> Duration {
        self.elapsed.pop_front().unwrap_or(Duration::ZERO)
    }
    fn next_event(&mut self, _: Duration) -> Result<Option<CallosumEnvelope>, String> {
        self.events
            .pop_front()
            .map(|event| Ok(Some(self.envelope(event))))
            .unwrap_or_else(|| Err("controlled peer exhausted".into()))
    }
}

fn options() -> RestartConveyOptions {
    RestartConveyOptions {
        timeout: 1.0,
        verbose: false,
    }
}

#[test]
fn correlated_start_requires_a_fresh_port_inode_even_when_the_port_is_reused() {
    let journal = Journal::new();
    journal.port("5015");
    let peer = Peer::one(Event::Started);
    let error = restart_convey_with_transport(journal.path(), options(), peer)
        .expect_err("stale inode must not succeed");
    assert!(error.stderr().contains("Failed to send restart request"));
}

#[test]
fn correlated_start_with_absent_or_invalid_readiness_cannot_succeed() {
    let absent = Journal::new();
    let error = restart_convey_with_transport(absent.path(), options(), Peer::one(Event::Started))
        .expect_err("absent readiness must fail");
    assert!(error.stderr().contains("Failed to send restart request"));

    let invalid = Journal::new();
    let mut peer = Peer::one(Event::Started);
    peer.write_port_on_send = Some("0".into());
    let error = restart_convey_with_transport(
        invalid.path(),
        options(),
        WritingPeer {
            peer,
            journal: invalid.path().to_path_buf(),
        },
    )
    .expect_err("zero readiness port must fail");
    assert!(error.stderr().contains("Failed to send restart request"));
}

#[test]
fn missing_callasum_socket_fails_fast_with_its_path() {
    let journal = Journal::new();
    let error = restart_convey(journal.path(), options()).expect_err("missing socket");
    let expected = template("template-connect-failure");
    assert_eq!(error.stdout(), render_template(&expected.stdout, &journal));
    assert!(
        error.stderr().contains(
            &journal
                .path()
                .join("health/callosum.sock")
                .display()
                .to_string()
        )
    );
    assert!(
        error
            .stderr()
            .ends_with(expected.stderr.split_once("\n\n").expect("separator").1)
    );
}

#[test]
fn port_unknown_reference_success_is_precluded_by_af005_readiness() {
    let journal = Journal::new();
    let error = restart_convey_with_transport(journal.path(), options(), Peer::one(Event::Started))
        .expect_err("AF-005 requires a fresh valid port instead of an unknown-port success");
    let expected = template("template-success-port-unknown");
    let unknown_line = expected.stdout.lines().last().expect("success line exists");
    assert!(!error.stdout().contains(unknown_line));
}

#[test]
fn correlated_start_with_a_fresh_valid_port_succeeds() {
    let journal = Journal::new();
    let mut peer = Peer::one(Event::Started);
    peer.write_port_on_send = Some("5015".into());
    // A real replacement happens after send, matching Convey's atomic writer.
    let result = restart_convey_with_transport(
        journal.path(),
        options(),
        WritingPeer {
            peer,
            journal: journal.path().to_path_buf(),
        },
    )
    .expect("fresh ready port succeeds");
    let expected = template("template-success-port-known");
    assert_eq!(result.stdout(), render_template(&expected.stdout, &journal));
    assert_eq!(result.stderr(), expected.stderr);
}

#[test]
fn infinite_timeout_waits_without_duration_conversion() {
    let journal = Journal::new();
    let mut peer = Peer::one(Event::Started);
    peer.write_port_on_send = Some("5015".into());
    let result = restart_convey_with_transport(
        journal.path(),
        RestartConveyOptions {
            timeout: f64::INFINITY,
            verbose: false,
        },
        WritingPeer {
            peer,
            journal: journal.path().to_path_buf(),
        },
    )
    .expect("infinite timeout can complete");
    let expected = template("template-success-port-known");
    assert_eq!(
        result.stdout(),
        render_template(&expected.stdout.replace("1.0", "inf"), &journal)
    );
}

#[test]
fn nan_timeout_expires_immediately() {
    let journal = Journal::new();
    let error = restart_convey_with_transport(
        journal.path(),
        RestartConveyOptions {
            timeout: f64::NAN,
            verbose: false,
        },
        Peer::one(Event::Started),
    )
    .expect_err("nan matches Python's immediately-false wait condition");
    let expected = template("template-timeout");
    assert_eq!(
        error.stdout(),
        render_template(&expected.stdout.replace("0.0", "nan"), &journal)
    );
    assert_eq!(error.stderr(), expected.stderr.replace("0.0", "nan"));
}

#[test]
fn verbose_restart_keeps_progress_and_live_logs_on_stderr() {
    let journal = Journal::new();
    let peer = Peer {
        events: VecDeque::from([
            Event::Restarting,
            Event::Stopped,
            Event::Started,
            Event::StdoutLog,
        ]),
        restart_id: None,
        write_port_on_send: None,
        fail_send: false,
        elapsed: VecDeque::from([
            Duration::ZERO,
            Duration::ZERO,
            Duration::ZERO,
            Duration::ZERO,
            Duration::ZERO,
            Duration::from_secs(1),
        ]),
    };
    let result = restart_convey_with_transport(
        journal.path(),
        RestartConveyOptions {
            timeout: 1.0,
            verbose: true,
        },
        DeferredWritingPeer {
            peer,
            journal: journal.path().to_path_buf(),
            port: "5015".into(),
            wrote_port: false,
        },
    )
    .expect("controlled fresh port succeeds");
    let expected = template("template-verbose-live-stream");
    assert_eq!(result.stdout(), render_template(&expected.stdout, &journal));
    let expected_stderr = expected.stderr.replace("{time}", &log_time());
    assert_eq!(result.stderr(), expected_stderr);
}

fn log_time() -> String {
    chrono::DateTime::<chrono::Utc>::from_timestamp_millis(1_700_000_000_000)
        .expect("fixture timestamp is valid")
        .with_timezone(&chrono::Local)
        .format("%H:%M:%S")
        .to_string()
}

struct WritingPeer {
    peer: Peer,
    journal: PathBuf,
}

struct DeferredWritingPeer {
    peer: Peer,
    journal: PathBuf,
    port: String,
    wrote_port: bool,
}

impl RestartTransport for DeferredWritingPeer {
    fn send_restart(&mut self, id: &str) -> Result<(), String> {
        self.peer.send_restart(id)
    }
    fn start_timer(&mut self) {
        self.peer.start_timer();
    }
    fn elapsed(&mut self) -> Duration {
        self.peer.elapsed()
    }
    fn next_event(&mut self, _: Duration) -> Result<Option<CallosumEnvelope>, String> {
        if let Some(event) = self.peer.events.pop_front() {
            return Ok(Some(self.peer.envelope(event)));
        }
        if !self.wrote_port {
            let target = self.journal.join("health/convey.port");
            let temp = self.journal.join("health/.replacement");
            fs::write(&temp, &self.port).expect("replacement writes");
            fs::rename(temp, target).expect("replacement renames");
            self.wrote_port = true;
        }
        Ok(None)
    }
}
impl RestartTransport for WritingPeer {
    fn send_restart(&mut self, id: &str) -> Result<(), String> {
        self.peer.send_restart(id)?;
        if let Some(port) = &self.peer.write_port_on_send {
            let target = self.journal.join("health/convey.port");
            let temp = self.journal.join("health/.replacement");
            fs::write(&temp, port).expect("replacement writes");
            fs::rename(temp, target).expect("replacement renames");
        }
        Ok(())
    }
    fn start_timer(&mut self) {
        self.peer.start_timer();
    }
    fn elapsed(&mut self) -> Duration {
        self.peer.elapsed()
    }
    fn next_event(&mut self, timeout: Duration) -> Result<Option<CallosumEnvelope>, String> {
        self.peer.next_event(timeout)
    }
}

fn assert_event_is_excluded(event: Event, description: &str) {
    let journal = Journal::new();
    let error = restart_convey_with_transport(journal.path(), options(), Peer::one(event))
        .expect_err(description);
    assert!(error.stderr().contains("Failed to send restart request"));
}

#[test]
fn pre_request_matching_lookalike_is_excluded() {
    assert_event_is_excluded(
        Event::PreRequestLookalike,
        "pre-request matching-looking event cannot succeed",
    );
}

#[test]
fn events_without_a_restart_id_are_excluded() {
    assert_event_is_excluded(Event::Uncorrelated, "uncorrelated event cannot succeed");
}

#[test]
fn concurrent_restart_and_unrelated_events_are_excluded() {
    assert_event_is_excluded(Event::OtherRestart, "concurrent restart cannot succeed");
    assert_event_is_excluded(Event::OtherService, "other service cannot succeed");
    assert_event_is_excluded(Event::OtherMessage, "unrelated message cannot succeed");
}

#[test]
fn delivery_failure_is_reported() {
    let journal = Journal::new();
    let mut peer = Peer::one(Event::Started);
    peer.fail_send = true;
    let error = restart_convey_with_transport(journal.path(), options(), peer)
        .expect_err("delivery failure");
    let expected = template("template-emit-failure");
    assert_eq!(error.stdout(), render_template(&expected.stdout, &journal));
    assert_eq!(error.stderr(), expected.stderr);
}

#[test]
fn no_terminal_event_times_out_when_the_wait_window_is_expired() {
    let journal = Journal::new();
    let error = restart_convey_with_transport(
        journal.path(),
        RestartConveyOptions {
            timeout: 0.0,
            verbose: false,
        },
        Peer {
            events: VecDeque::new(),
            restart_id: None,
            write_port_on_send: None,
            fail_send: false,
            elapsed: VecDeque::new(),
        },
    )
    .expect_err("zero timeout");
    let expected = template("template-timeout");
    assert_eq!(error.stdout(), render_template(&expected.stdout, &journal));
    assert_eq!(error.stderr(), expected.stderr);
}

#[test]
fn ignored_events_then_an_idle_peer_take_the_real_timeout_path() {
    let journal = Journal::new();
    let peer = Peer {
        events: VecDeque::from([Event::OtherService]),
        restart_id: None,
        write_port_on_send: None,
        fail_send: false,
        elapsed: VecDeque::from([Duration::ZERO, Duration::from_secs(1)]),
    };
    let error = restart_convey_with_transport(journal.path(), options(), IdlePeer(peer))
        .expect_err("ignored event cannot become a terminal start");
    let expected = template("template-timeout");
    assert_eq!(
        error.stdout(),
        render_template(&expected.stdout.replace("0.0", "1.0"), &journal)
    );
    assert_eq!(error.stderr(), expected.stderr.replace("0.0", "1.0"));
}

#[test]
fn second_correlated_start_before_fresh_readiness_is_a_crash() {
    let journal = Journal::new();
    let peer = Peer {
        events: VecDeque::from([Event::Started, Event::Started]),
        restart_id: None,
        write_port_on_send: None,
        fail_send: false,
        elapsed: VecDeque::new(),
    };
    let error = restart_convey_with_transport(journal.path(), options(), peer)
        .expect_err("second start crashes");
    assert!(
        error
            .stderr()
            .contains("Convey crashed and restarted (attempt 2)")
    );
}

#[test]
fn matching_logs_are_dumped_only_for_non_verbose_failures() {
    let journal = Journal::new();
    let peer = Peer {
        events: VecDeque::from([Event::Log, Event::Started, Event::Started]),
        restart_id: None,
        write_port_on_send: None,
        fail_send: false,
        elapsed: VecDeque::new(),
    };
    let error =
        restart_convey_with_transport(journal.path(), options(), peer).expect_err("second start");
    assert!(error.stderr().contains("Collected logs:"));
    let expected = template("template-second-start-log-dump");
    let suffix = expected
        .stderr
        .split("{time}")
        .nth(1)
        .expect("log template includes time");
    assert!(error.stderr().contains(suffix));
}
