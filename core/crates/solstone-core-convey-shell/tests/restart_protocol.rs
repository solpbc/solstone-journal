// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

#![cfg(unix)]

use std::collections::VecDeque;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;
use std::time::{SystemTime, UNIX_EPOCH};

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

impl Drop for Journal {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

enum Event {
    Started,
    Log,
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
}

impl Peer {
    fn one(event: Event) -> Self {
        Self {
            events: VecDeque::from([event]),
            restart_id: None,
            write_port_on_send: None,
            fail_send: false,
        }
    }
    fn envelope(&self, event: Event) -> CallosumEnvelope {
        let mut extra = Map::new();
        match event {
            Event::Started => {
                extra.insert("service".into(), json!("convey"));
                extra.insert("pid".into(), json!(321));
                extra.insert("ref".into(), json!("supervisor-app-convey"));
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
            Event::Log => {
                extra.insert(
                    "restart_id".into(),
                    json!(self.restart_id.as_deref().unwrap_or_default()),
                );
                extra.insert("line".into(), json!("fixture log"));
                extra.insert("stream".into(), json!("stderr"));
                CallosumEnvelope {
                    tract: "logs".into(),
                    event: "line".into(),
                    ts: None,
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

impl RestartTransport for Peer {
    fn send_restart(&mut self, restart_id: &str) -> Result<(), String> {
        if self.fail_send {
            return Err("delivery failure".into());
        }
        self.restart_id = Some(restart_id.into());
        Ok(())
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
        timeout: Duration::from_secs(1),
        verbose: false,
        debug: false,
    }
}

#[test]
fn correlated_start_requires_a_fresh_port_inode_even_when_the_port_is_reused() {
    let journal = Journal::new();
    journal.port("5015");
    let peer = Peer::one(Event::Started);
    let error = restart_convey_with_transport(journal.path(), options(), peer)
        .expect_err("stale inode must not succeed");
    assert!(error.output().contains("Failed to send restart request"));
}

#[test]
fn correlated_start_with_absent_or_invalid_readiness_cannot_succeed() {
    let absent = Journal::new();
    let error = restart_convey_with_transport(absent.path(), options(), Peer::one(Event::Started))
        .expect_err("absent readiness must fail");
    assert!(error.output().contains("Failed to send restart request"));

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
    assert!(error.output().contains("Failed to send restart request"));
}

#[test]
fn missing_callasum_socket_fails_fast_with_its_path() {
    let journal = Journal::new();
    let error = restart_convey(journal.path(), options()).expect_err("missing socket");
    assert!(
        error.output().contains(
            &journal
                .path()
                .join("health/callosum.sock")
                .display()
                .to_string()
        )
    );
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
    assert_eq!(result.output, "Convey running at http://localhost:5015\n");
}

struct WritingPeer {
    peer: Peer,
    journal: PathBuf,
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
    fn next_event(&mut self, timeout: Duration) -> Result<Option<CallosumEnvelope>, String> {
        self.peer.next_event(timeout)
    }
}

fn assert_event_is_excluded(event: Event, description: &str) {
    let journal = Journal::new();
    let error = restart_convey_with_transport(journal.path(), options(), Peer::one(event))
        .expect_err(description);
    assert!(error.output().contains("Failed to send restart request"));
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
    assert!(error.output().contains("delivery failure"));
}

#[test]
fn no_terminal_event_times_out_when_the_wait_window_is_expired() {
    let journal = Journal::new();
    let error = restart_convey_with_transport(
        journal.path(),
        RestartConveyOptions {
            timeout: Duration::ZERO,
            verbose: false,
            debug: false,
        },
        Peer {
            events: VecDeque::new(),
            restart_id: None,
            write_port_on_send: None,
            fail_send: false,
        },
    )
    .expect_err("zero timeout");
    assert!(
        error
            .output()
            .contains("Timeout waiting for convey to restart (0.0s)")
    );
}

#[test]
fn second_correlated_start_before_fresh_readiness_is_a_crash() {
    let journal = Journal::new();
    let peer = Peer {
        events: VecDeque::from([Event::Started, Event::Started]),
        restart_id: None,
        write_port_on_send: None,
        fail_send: false,
    };
    let error = restart_convey_with_transport(journal.path(), options(), peer)
        .expect_err("second start crashes");
    assert!(
        error
            .output()
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
    };
    let error =
        restart_convey_with_transport(journal.path(), options(), peer).expect_err("second start");
    assert!(error.output().contains("Collected logs:"));
    assert!(error.output().contains("[00:00:00] [ERR] fixture log"));
}
