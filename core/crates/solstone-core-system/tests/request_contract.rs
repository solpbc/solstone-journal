// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::time::Duration;

use solstone_core_system::cap::DefaultCapResolver;
use solstone_core_system::partition::partition_for;
use solstone_core_system::request::{
    ActiveTaskSnapshot, BusTaskRequest, ExecutionRequest, RefusalReason, RequestDisposition,
    ScheduledArgv, ScheduledRequest, TaskArgv, WireTaskRequest, classify_wire_request,
};

fn words(values: &[&str]) -> Vec<String> {
    values.iter().map(|value| (*value).to_owned()).collect()
}

#[test]
fn ac5_every_production_argv_round_trips_byte_for_byte() {
    let corpus = [
        words(&[
            "journal",
            "brain",
            "refresh",
            "--expected-fingerprint",
            "fp",
        ]),
        words(&["journal", "think", "-v", "--day", "20260807"]),
        words(&[
            "journal",
            "think",
            "-v",
            "--day",
            "20260807",
            "--segment",
            "a",
            "--stream",
            "field",
            "--live",
        ]),
        words(&[
            "journal",
            "think",
            "-v",
            "--day",
            "20260807",
            "--segment",
            "a",
            "--flush",
        ]),
        words(&[
            "journal",
            "think",
            "--activity",
            "id",
            "--facet",
            "work",
            "--day",
            "20260807",
        ]),
        words(&["journal", "heartbeat"]),
        words(&[
            "journal",
            "brain",
            "renew-prerequisites",
            "--json",
            "--expected-fingerprint",
            "fp",
        ]),
        words(&[
            "journal",
            "brain",
            "refresh",
            "--json",
            "--expect-active-fingerprint-absent",
        ]),
        words(&[
            "journal",
            "brain",
            "refresh",
            "--json",
            "--expected-fingerprint",
            "fp",
            "--expected-active-fingerprint",
        ]),
        words(&["journal", "importer", "--sync", "plaud", "--save"]),
        words(&["journal", "importer", "--sync", "obsidian", "--save"]),
        words(&[
            "journal",
            "think",
            "-v",
            "--day",
            "20260801",
            "--from-scratch",
        ]),
        words(&["journal", "maintenance", "run", "backup:run"]),
        words(&["journal", "maintenance", "run", "backup:verify"]),
        words(&["journal", "maintenance", "run", "app:routine"]),
        words(&["journal", "indexer", "--rescan-file", "/tmp/output.jsonl"]),
        words(&["journal", "indexer", "--rescan", "--verbose"]),
        words(&["journal", "indexer", "--rescan-full"]),
        words(&[
            "journal",
            "importer",
            "/tmp/in",
            "123",
            "--facet",
            "work",
            "--setting",
            "home",
            "--source",
            "camera",
            "--force",
        ]),
        words(&["journal", "think", "--weekly", "-v"]),
        words(&["journal", "think", "--cadence"]),
        words(&["journal", "facet-candidates"]),
        words(&["journal", "indexer", "--rebuild-edges"]),
    ];
    for cmd in corpus {
        assert_eq!(TaskArgv::from_wire(cmd.clone()).unwrap().as_wire(), cmd);
    }
    let scheduled = words(&["sol", "call", "timeline", "rollup-day"]);
    assert_eq!(
        ScheduledArgv::from_wire(scheduled.clone())
            .unwrap()
            .as_wire(),
        scheduled
    );
}

#[test]
fn ac6_unknown_ordinary_bus_command_is_lossless_named_variant() {
    let raw = words(&["sol", "call", "timeline", "rollup-day"]);
    let argv = TaskArgv::from_wire(raw.clone()).unwrap();
    assert!(matches!(argv, TaskArgv::Unknown { .. }));
    assert_eq!(argv.as_wire(), raw);
}

#[test]
fn ac8_partition_resolution_matches_ordered_python_contract() {
    assert_eq!(
        partition_for(&words(&["journal", "think"])).as_str(),
        "daily"
    );
    assert_eq!(
        partition_for(&words(&["journal", "think", "--segment", "x", "--flush"])).as_str(),
        "flush"
    );
    assert_eq!(
        partition_for(&words(&["journal", "think", "--activity", "id", "--flush"])).as_str(),
        "activity"
    );
    assert_eq!(
        partition_for(&words(&["journal", "maintenance", "run", "backup:run"])).as_str(),
        "maintenance:backup:run"
    );
    assert_eq!(
        partition_for(&words(&["journal", "maintenance", "status"])).as_str(),
        "maintenance"
    );
    assert_eq!(
        partition_for(&words(&["sol", "think", "--segment", "x", "--flush"])).as_str(),
        partition_for(&words(&["journal", "think", "--segment", "x", "--flush"])).as_str()
    );
    assert_eq!(
        partition_for(&words(&["/usr/local/bin/tool"])).as_str(),
        "tool"
    );
    assert_eq!(partition_for(&[]).as_str(), "unknown");
}

#[test]
fn ac9_refusal_carries_all_supervisor_skipped_fields() {
    let resolver = DefaultCapResolver::new(Duration::from_secs(10));
    let disposition = classify_wire_request(
        WireTaskRequest {
            cmd: Some(words(&["journal", "think", "--day", "20260807"])),
            reference: Some("request-ref".to_owned()),
            day: Some("20260807".to_owned()),
            scheduler_name: Some("scheduled-name".to_owned()),
            queue_if_active_cmd_differs: false,
        },
        "fallback",
        true,
        Some(ActiveTaskSnapshot {
            reference: "active-ref".to_owned(),
            cmd: Some(words(&["journal", "think", "--day", "20260807"])),
            started_at: Some(99),
        }),
        &resolver,
        100,
    );
    let RequestDisposition::Refused(refusal) = disposition else {
        panic!("expected refusal")
    };
    assert_eq!(refusal.reason, RefusalReason::StillRunning);
    assert_eq!(refusal.reference, "request-ref");
    assert_eq!(refusal.active_reference, "active-ref");
    assert_eq!(
        refusal.cmd,
        words(&["journal", "think", "--day", "20260807"])
    );
    assert_eq!(refusal.scheduler_name.as_deref(), Some("scheduled-name"));
}

#[test]
fn ac9_wedged_threshold_is_strictly_more_than_twice_the_cap() {
    let resolver = DefaultCapResolver::new(Duration::from_secs(10));
    let make = |started_at| {
        classify_wire_request(
            WireTaskRequest {
                cmd: Some(words(&["journal", "heartbeat"])),
                ..WireTaskRequest::default()
            },
            "ref",
            true,
            Some(ActiveTaskSnapshot {
                reference: "active".to_owned(),
                cmd: None,
                started_at: Some(started_at),
            }),
            &resolver,
            100,
        )
    };
    assert!(
        matches!(make(80), RequestDisposition::Refused(refusal) if refusal.reason == RefusalReason::StillRunning)
    );
    assert!(
        matches!(make(79), RequestDisposition::Refused(refusal) if refusal.reason == RefusalReason::Wedged)
    );
}

#[test]
fn ac10_busy_differing_command_queues_when_bypass_enabled() {
    let resolver = DefaultCapResolver::default();
    let active = ActiveTaskSnapshot {
        reference: "active".to_owned(),
        cmd: Some(words(&["journal", "importer", "a", "1"])),
        started_at: Some(1),
    };
    let bypass = classify_wire_request(
        WireTaskRequest {
            cmd: Some(words(&["journal", "importer", "b", "2"])),
            queue_if_active_cmd_differs: true,
            ..WireTaskRequest::default()
        },
        "ref",
        true,
        Some(active),
        &resolver,
        2,
    );
    assert_eq!(bypass, RequestDisposition::QueueDespiteActive);
}

#[test]
fn ac10_busy_differing_command_refuses_without_bypass() {
    let resolver = DefaultCapResolver::default();
    let active = ActiveTaskSnapshot {
        reference: "active".to_owned(),
        cmd: Some(words(&["journal", "importer", "a", "1"])),
        started_at: Some(1),
    };
    assert!(matches!(
        classify_wire_request(
            WireTaskRequest {
                cmd: Some(words(&["journal", "importer", "b", "2"])),
                queue_if_active_cmd_differs: false,
                ..WireTaskRequest::default()
            },
            "ref",
            true,
            Some(active),
            &resolver,
            2,
        ),
        RequestDisposition::Refused(_)
    ));
}

#[test]
fn ac11_missing_command_and_queue_unavailable_are_distinct_silent_outcomes() {
    let resolver = DefaultCapResolver::default();
    assert_eq!(
        classify_wire_request(WireTaskRequest::default(), "ref", true, None, &resolver, 2),
        RequestDisposition::IgnoredMissingCommand
    );
    assert_eq!(
        classify_wire_request(
            WireTaskRequest {
                cmd: Some(words(&["journal", "heartbeat"])),
                ..WireTaskRequest::default()
            },
            "ref",
            false,
            None,
            &resolver,
            2
        ),
        RequestDisposition::IgnoredQueueUnavailable
    );
}

#[test]
fn ac9_race_unreadable_active_process_has_no_active_command() {
    let resolver = DefaultCapResolver::default();
    let disposition = classify_wire_request(
        WireTaskRequest {
            cmd: Some(words(&["journal", "heartbeat"])),
            ..WireTaskRequest::default()
        },
        "ref",
        true,
        Some(ActiveTaskSnapshot {
            reference: "active".to_owned(),
            cmd: None,
            started_at: None,
        }),
        &resolver,
        99,
    );
    assert!(
        matches!(disposition, RequestDisposition::Refused(refusal) if refusal.reason == RefusalReason::StillRunning && refusal.active_reference == "active")
    );
}

#[test]
fn ac9_cap_resolution_uses_override_or_default() {
    let mut resolver = DefaultCapResolver::new(Duration::from_secs(42));
    let think = partition_for(&words(&["journal", "think"]));
    resolver.set_override(think.clone(), Duration::from_secs(7));
    assert_eq!(
        solstone_core_system::cap::CapResolver::cap_for(&resolver, &think),
        Duration::from_secs(7)
    );
    assert_eq!(
        solstone_core_system::cap::CapResolver::cap_for(
            &resolver,
            &partition_for(&words(&["journal", "heartbeat"]))
        ),
        Duration::from_secs(42)
    );
    resolver.set_override(partition_for(&words(&["journal", "think"])), Duration::ZERO);
    assert_eq!(
        solstone_core_system::cap::CapResolver::cap_for(
            &resolver,
            &partition_for(&words(&["journal", "think"]))
        ),
        Duration::from_secs(42)
    );
}

#[test]
fn ac7_scheduler_construction_is_explicitly_separate_from_bus_decode() {
    let scheduled = ScheduledRequest::new(
        ScheduledArgv::from_wire(words(&["sol", "call", "timeline", "rollup-day"])).unwrap(),
        "sched:timeline:1",
        "timeline",
    );
    let request = ExecutionRequest::Scheduled(scheduled);
    assert!(matches!(request, ExecutionRequest::Scheduled(_)));
    assert!(
        BusTaskRequest::decode(
            WireTaskRequest {
                cmd: Some(words(&["sol", "call", "timeline", "rollup-day"])),
                ..WireTaskRequest::default()
            },
            "ref"
        )
        .is_ok()
    );
}
