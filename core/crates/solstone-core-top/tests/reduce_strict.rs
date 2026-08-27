// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use serde_json::{Map, Value, json};
use solstone_core_callosum::{CallosumConnectionPhase, CallosumEnvelope, CallosumReceiveEvent};
use solstone_core_top::{
    FrameSample, PlainTopStyle, ProcessObserver, ProcessSample, ReductionDisposition,
    ReductionSample, TopMalformed, TopMalformedKind, TopRoute, TopState, apply_receive_event,
    reduce_envelope, render_frame,
};

const FIXTURE: &str = include_str!("../../../fixtures/top_reference.json");

#[derive(Default)]
struct RecordingObserver {
    calls: usize,
}

impl ProcessObserver for RecordingObserver {
    fn sample(&mut self, _: u32, _: f64) -> ProcessSample {
        self.calls += 1;
        ProcessSample::Missing
    }
}

fn sample() -> ReductionSample {
    ReductionSample::fixture(100.0, "2027-01-15T08:00:00")
}

fn event(tract: &str, event: &str, extra: Map<String, Value>) -> CallosumEnvelope {
    CallosumEnvelope {
        tract: tract.to_owned(),
        event: event.to_owned(),
        ts: None,
        extra,
    }
}

fn assert_malformed(envelope: CallosumEnvelope) {
    let mut state = TopState::default();
    let before = state.fixture_value();
    let mut observer = RecordingObserver::default();
    assert!(matches!(
        reduce_envelope(&mut state, &envelope, &sample(), &mut observer),
        ReductionDisposition::Malformed(_)
    ));
    assert_eq!(state.fixture_value(), before);
    assert_eq!(state.malformed_events, 1);
    assert_eq!(observer.calls, 0);
}

fn assert_malformed_kind(envelope: CallosumEnvelope, route: TopRoute, kind: TopMalformedKind) {
    let mut state = TopState::default();
    let before = state.fixture_value();
    let mut observer = RecordingObserver::default();
    let expected = TopMalformed { route, kind };
    assert_eq!(
        reduce_envelope(&mut state, &envelope, &sample(), &mut observer),
        ReductionDisposition::Malformed(expected.clone())
    );
    assert_eq!(state.last_malformed, Some(expected));
    assert_eq!(state.fixture_value(), before);
    assert_eq!(state.malformed_events, 1);
    assert_eq!(observer.calls, 0);
}

#[test]
fn recognized_routes_reject_atomically_with_typed_evidence() {
    let fixture: Value = serde_json::from_str(FIXTURE).unwrap();
    for case in fixture["malformed_events"].as_array().unwrap() {
        let name = case["name"].as_str().unwrap();
        if name == "unknown-tract" {
            continue;
        }
        let mut state = TopState::default();
        let before = state.fixture_value();
        let mut observer = RecordingObserver::default();
        let envelope: CallosumEnvelope = serde_json::from_value(case["event"].clone()).unwrap();
        assert!(
            matches!(
                reduce_envelope(&mut state, &envelope, &sample(), &mut observer),
                ReductionDisposition::Malformed(_)
            ),
            "{name}"
        );
        assert_eq!(state.fixture_value(), before, "{name}");
        assert_eq!(state.malformed_events, 1, "{name}");
        assert_eq!(observer.calls, 0, "{name}");
    }

    for envelope in [
        event(
            "supervisor",
            "status",
            Map::from_iter([
                (
                    "services".into(),
                    json!([{ "name":"n", "ref": 7, "pid":1, "uptime_seconds":0 }]),
                ),
                ("crashed".into(), json!([])),
                ("queues".into(), json!({})),
            ]),
        ),
        event(
            "supervisor",
            "status",
            Map::from_iter([
                ("services".into(), json!([])),
                (
                    "crashed".into(),
                    json!([{ "name":"n", "restart_attempts":"one" }]),
                ),
                ("queues".into(), json!({"q": -1})),
            ]),
        ),
        event(
            "supervisor",
            "queue",
            Map::from_iter([
                ("command".into(), json!("q")),
                ("queued".into(), json!(1.5)),
            ]),
        ),
        event(
            "logs",
            "exec",
            Map::from_iter([
                ("ref".into(), json!("r")),
                ("name".into(), json!("n")),
                ("pid".into(), json!(1)),
                ("cmd".into(), json!(["x", 7])),
            ]),
        ),
        event(
            "logs",
            "line",
            Map::from_iter([
                ("ref".into(), json!("r")),
                ("name".into(), json!("n")),
                ("pid".into(), json!(1)),
                ("line".into(), json!("x")),
                ("stream".into(), json!("merged")),
            ]),
        ),
        event(
            "logs",
            "exit",
            Map::from_iter([
                ("ref".into(), json!("r")),
                ("exit_code".into(), json!(2_147_483_648i64)),
            ]),
        ),
        event(
            "observe",
            "observed",
            Map::from_iter([
                ("day".into(), json!("d")),
                ("segment".into(), json!("s")),
                ("duration".into(), json!(1.5)),
            ]),
        ),
        event(
            "think",
            "status",
            Map::from_iter([("agents_total".into(), json!("one"))]),
        ),
        event(
            "think",
            "completed",
            Map::from_iter([
                ("success".into(), json!(1)),
                ("failed".into(), json!(0)),
                ("duration_ms".into(), json!(1)),
                ("failed_names".into(), json!([7])),
            ]),
        ),
        event(
            "supervisor",
            "restarting",
            Map::from_iter([("service".into(), json!(""))]),
        ),
        event(
            "logs",
            "exit",
            Map::from_iter([
                ("ref".into(), json!("r")),
                ("exit_code".into(), json!(null)),
                ("name".into(), json!(false)),
            ]),
        ),
    ] {
        assert_malformed(envelope);
    }
}

#[test]
fn fixture_malformed_rows_keep_their_typed_route_and_kind() {
    let fixture: Value = serde_json::from_str(FIXTURE).unwrap();
    let expected = [
        (
            "supervisor-status-defaults",
            TopRoute::SupervisorStatus,
            TopMalformedKind::MissingField("services"),
        ),
        (
            "supervisor-service-missing-pid",
            TopRoute::SupervisorStatus,
            TopMalformedKind::MissingField("pid"),
        ),
        (
            "supervisor-services-wrong-type",
            TopRoute::SupervisorStatus,
            TopMalformedKind::WrongType("services"),
        ),
        (
            "supervisor-queues-wrong-type",
            TopRoute::SupervisorStatus,
            TopMalformedKind::MissingField("services"),
        ),
        (
            "queue-missing-command",
            TopRoute::SupervisorQueue,
            TopMalformedKind::MissingField("command"),
        ),
        (
            "queue-wrong-count",
            TopRoute::SupervisorQueue,
            TopMalformedKind::WrongType("queued"),
        ),
        (
            "logs-exec-missing-pid",
            TopRoute::LogsExec,
            TopMalformedKind::MissingField("pid"),
        ),
        (
            "logs-line-missing-ref",
            TopRoute::LogsLine,
            TopMalformedKind::MissingField("ref"),
        ),
        (
            "logs-line-wrong-stream",
            TopRoute::LogsLine,
            TopMalformedKind::WrongType("line"),
        ),
        (
            "observe-missing-day",
            TopRoute::ObserveObserved,
            TopMalformedKind::MissingField("day"),
        ),
        (
            "observe-duration-wrong-type",
            TopRoute::ObserveObserved,
            TopMalformedKind::WrongType("duration"),
        ),
        (
            "think-completed-defaults",
            TopRoute::ThinkCompleted,
            TopMalformedKind::MissingField("success"),
        ),
    ];
    for (name, route, kind) in expected {
        let event = fixture["malformed_events"]
            .as_array()
            .unwrap()
            .iter()
            .find(|case| case["name"] == name)
            .unwrap()["event"]
            .clone();
        assert_malformed_kind(serde_json::from_value(event).unwrap(), route, kind);
    }
}

#[test]
fn undercovered_consumed_fields_reject_wrong_types() {
    assert_malformed_kind(
        event(
            "supervisor",
            "restarting",
            Map::from_iter([("service".into(), json!(1))]),
        ),
        TopRoute::SupervisorLifecycle,
        TopMalformedKind::WrongType("service"),
    );
    assert_malformed_kind(
        event(
            "supervisor",
            "queue",
            Map::from_iter([
                ("command".into(), json!(false)),
                ("queued".into(), json!(1)),
            ]),
        ),
        TopRoute::SupervisorQueue,
        TopMalformedKind::WrongType("command"),
    );
    for (field, value) in [
        ("name", json!(1)),
        ("ref", json!(1)),
        ("pid", json!("1")),
        ("uptime_seconds", json!("0")),
    ] {
        let mut service = json!({"name":"svc","ref":"ref","pid":1,"uptime_seconds":0});
        service[field] = value;
        assert_malformed_kind(
            event(
                "supervisor",
                "status",
                Map::from_iter([
                    ("services".into(), json!([service])),
                    ("crashed".into(), json!([])),
                    ("queues".into(), json!({})),
                ]),
            ),
            TopRoute::SupervisorStatus,
            TopMalformedKind::WrongType(field),
        );
    }
    assert_malformed_kind(
        event(
            "supervisor",
            "status",
            Map::from_iter([
                ("services".into(), json!([])),
                (
                    "crashed".into(),
                    json!([{"name":"svc","restart_attempts":"one"}]),
                ),
                ("queues".into(), json!({})),
            ]),
        ),
        TopRoute::SupervisorStatus,
        TopMalformedKind::WrongType("restart_attempts"),
    );
    assert_malformed_kind(
        event(
            "supervisor",
            "status",
            Map::from_iter([
                ("services".into(), json!([])),
                ("crashed".into(), json!([])),
                ("queues".into(), json!({"queue":"one"})),
            ]),
        ),
        TopRoute::SupervisorStatus,
        TopMalformedKind::WrongType("queues[]"),
    );
    assert_malformed_kind(
        event(
            "logs",
            "line",
            Map::from_iter([
                ("ref".into(), json!("r")),
                ("line".into(), json!(false)),
                ("stream".into(), json!("stdout")),
            ]),
        ),
        TopRoute::LogsLine,
        TopMalformedKind::WrongType("line"),
    );
    for (field, value) in [("name", json!(false)), ("pid", json!("3"))] {
        let mut extra = Map::from_iter([
            ("ref".into(), json!("r")),
            ("line".into(), json!("line")),
            ("stream".into(), json!("stdout")),
            ("name".into(), json!("task")),
            ("pid".into(), json!(3)),
        ]);
        extra.insert(field.to_owned(), value);
        assert_malformed_kind(
            event("logs", "line", extra),
            TopRoute::LogsLine,
            TopMalformedKind::WrongType(field),
        );
    }
    for (field, value) in [("mode", json!(1)), ("stream", json!(false))] {
        assert_malformed_kind(
            event("observe", "status", Map::from_iter([(field.into(), value)])),
            TopRoute::ObserveStatus,
            TopMalformedKind::WrongType(field),
        );
    }
    for field in [
        "mode",
        "day",
        "segment",
        "agents_total",
        "agents_completed",
        "segments_total",
        "segments_completed",
    ] {
        assert_malformed_kind(
            event(
                "think",
                "status",
                Map::from_iter([(field.into(), json!(false))]),
            ),
            TopRoute::ThinkStatus,
            TopMalformedKind::WrongType(field),
        );
    }
}

#[test]
fn strict_route_validators_project_only_consumed_fields() {
    let valid = event(
        "observe",
        "status",
        Map::from_iter([
            ("mode".into(), json!("tmux")),
            (
                "screencast".into(),
                json!({"window_elapsed_seconds":1,"ignored":"x"}),
            ),
            ("tmux".into(), json!({"captures":2,"ignored":true})),
            (
                "audio".into(),
                json!({"threshold_hits":3,"will_save":true,"ignored":null}),
            ),
            (
                "activity".into(),
                json!({"screen_locked":false,"ignored":0}),
            ),
            (
                "describe".into(),
                json!({"running":[1],"queued":[],"ignored":"x"}),
            ),
            ("transcribe".into(), json!({"running":[],"queued":[{}]})),
            ("producer_health".into(), json!("opaque")),
        ]),
    );
    let mut state = TopState::default();
    assert!(matches!(
        reduce_envelope(
            &mut state,
            &valid,
            &sample(),
            &mut RecordingObserver::default()
        ),
        ReductionDisposition::Applied(_)
    ));
    assert!(!state.observe_status.contains_key("producer_health"));
    assert!(state.observe_status["audio"].get("ignored").is_none());
    assert!(state.observe_status["describe"].get("ignored").is_none());

    for (key, value) in [
        ("screencast", json!({"window_elapsed_seconds":"one"})),
        ("tmux", json!({"captures":-1})),
        ("audio", json!({"will_save":"yes"})),
        ("activity", json!({"screen_locked":1})),
        ("describe", json!({"running":"no"})),
        ("transcribe", json!([])),
    ] {
        assert_malformed(event(
            "observe",
            "status",
            Map::from_iter([(key.to_owned(), value)]),
        ));
    }

    let think = event(
        "think",
        "status",
        Map::from_iter([
            ("mode".into(), json!("batch")),
            ("agents_total".into(), json!(2)),
            ("current_agents".into(), json!(["a"])),
            ("extension".into(), json!(true)),
        ]),
    );
    let mut state = TopState::default();
    assert!(matches!(
        reduce_envelope(
            &mut state,
            &think,
            &sample(),
            &mut RecordingObserver::default()
        ),
        ReductionDisposition::Applied(_)
    ));
    assert!(!state.think_status.contains_key("extension"));
    assert_malformed(event(
        "think",
        "status",
        Map::from_iter([("current_agents".into(), json!(["a", 1]))]),
    ));
    let extension_only = event(
        "think",
        "status",
        Map::from_iter([("producer_health".into(), json!(true))]),
    );
    let before = state.clone();
    assert_eq!(
        reduce_envelope(
            &mut state,
            &extension_only,
            &sample(),
            &mut RecordingObserver::default(),
        ),
        ReductionDisposition::Ignored
    );
    assert_eq!(state, before);
}

#[test]
fn malformed_event_has_no_side_effects_and_loop_continues() {
    let mut state = TopState::default();
    let mut observer = RecordingObserver::default();
    let malformed = event(
        "logs",
        "exec",
        Map::from_iter([
            ("ref".into(), json!("r")),
            ("name".into(), json!("n")),
            ("pid".into(), json!(0)),
        ]),
    );
    assert!(matches!(
        reduce_envelope(&mut state, &malformed, &sample(), &mut observer),
        ReductionDisposition::Malformed(_)
    ));
    assert_eq!(observer.calls, 0);
    assert!(state.running_tasks.is_empty());
    assert!(
        render_frame(&state, FrameSample::default(), 120, &PlainTopStyle)
            .contains("malformed events: 1")
    );
    state.continuity.generation = 1;
    state.continuity.epoch = 1;
    state.continuity.connection = CallosumConnectionPhase::Connected;
    let effects = apply_receive_event(
        &mut state,
        &CallosumReceiveEvent::Envelope {
            generation: 1,
            epoch: 1,
            envelope: event(
                "supervisor",
                "started",
                Map::from_iter([("service".into(), json!(false))]),
            ),
        },
        &sample(),
        &mut observer,
    );
    assert!(!effects.refresh_brain);
    assert_eq!(observer.calls, 0);
    let valid = event(
        "logs",
        "exec",
        Map::from_iter([
            ("ref".into(), json!("r")),
            ("name".into(), json!("n")),
            ("pid".into(), json!(1)),
        ]),
    );
    assert!(matches!(
        reduce_envelope(&mut state, &valid, &sample(), &mut observer),
        ReductionDisposition::Applied(_)
    ));
    assert_eq!(observer.calls, 1);
    assert!(state.running_tasks.contains_key("r"));
}

#[test]
fn unknown_routes_are_ignored() {
    for envelope in [
        event("other", "status", Map::new()),
        event("logs", "unknown", Map::new()),
    ] {
        let mut state = TopState::default();
        let before = state.clone();
        let mut observer = RecordingObserver::default();
        assert_eq!(
            reduce_envelope(&mut state, &envelope, &sample(), &mut observer),
            ReductionDisposition::Ignored
        );
        assert_eq!(state, before);
        assert_eq!(observer.calls, 0);
    }
}
