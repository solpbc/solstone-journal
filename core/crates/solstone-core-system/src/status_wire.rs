// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use serde::Serialize;
use serde_json::{Map, Value};

use crate::lifecycle::SyncPeerIdentity;
use crate::provider_runtime::{ProviderName, ReasonCode, RuntimePhase};
use crate::queue::TaskQueueStatusSnapshot;
use crate::schedule::ScheduleStatus;

pub struct SupervisorStatusWireInput {
    pub services: Vec<ServiceCandidate>,
    pub crashed: Vec<CrashedServiceCandidate>,
    pub queue: TaskQueueStatusSnapshot,
    pub stale_heartbeats: Vec<StaleHeartbeatWireInput>,
    pub schedules: Vec<ScheduleStatus>,
    pub callosum_clients: usize,
}
pub enum ServiceCandidate {
    SupervisorSelf {
        reference: String,
        pid: u32,
        uptime_seconds: u64,
    },
    App {
        name: String,
        observation: ProcessObservation,
    },
    Provider {
        provider: ProviderName,
        observation: ProcessObservation,
        phase: RuntimePhase,
        reason_code: Option<ReasonCode>,
    },
}
pub enum ProcessObservation {
    Live {
        reference: String,
        pid: u32,
        uptime_seconds: u64,
    },
    ConfirmedAbsent,
}
pub struct CrashedServiceCandidate {
    pub name: String,
    pub restart_attempts: u32,
    pub phase: RuntimePhase,
    pub reason_code: Option<ReasonCode>,
}
pub struct StaleHeartbeatWireInput {
    pub source_filename: Vec<u8>,
    pub hostname: String,
    pub identity: SyncPeerIdentity,
    pub journal_path: String,
    pub pid: Option<u32>,
    pub wall_time: Option<String>,
    pub malformed: bool,
}

#[derive(Serialize)]
struct ServiceWireRow {
    name: String,
    pid: u32,
    uptime_seconds: u64,
    #[serde(rename = "ref")]
    reference: String,
    phase: String,
    reason_code: Option<String>,
}
#[derive(Serialize)]
struct CrashedWireRow {
    name: String,
    restart_attempts: u32,
    phase: String,
    reason_code: Option<String>,
}
#[derive(Serialize)]
struct TaskWireRow {
    #[serde(rename = "ref")]
    reference: String,
    name: String,
    max_runtime_seconds: u64,
    duration_seconds: u64,
    slow: bool,
    stuck: bool,
}
#[derive(Serialize)]
struct RecentTaskWireRow {
    #[serde(rename = "ref")]
    reference: String,
    exit_status: String,
    scheduler_name: Option<String>,
}
#[derive(Serialize)]
struct ScheduleWireRow {
    name: String,
    every: String,
    last_run: Option<f64>,
    due: bool,
    next_run: i64,
    daily_time: Option<String>,
    weekly_day: Option<String>,
    weekly_time: Option<String>,
}
#[derive(Serialize)]
struct StaleHeartbeatDetailWireRow {
    hostname: String,
    heartbeat_schema: String,
    legacy_machine_id_prefix: Option<String>,
    writer_id_prefix: Option<String>,
    run_id: Option<String>,
    journal_path: String,
    pid: Option<u32>,
    wall_time: Option<String>,
    malformed: bool,
    reason_code: String,
}

fn row(value: impl Serialize) -> Value {
    serde_json::to_value(value).expect("status wire rows are serializable")
}
fn stale_display(stale: &StaleHeartbeatWireInput) -> String {
    let identity = if stale.hostname.is_empty() {
        stale.identity.display_prefix().to_owned()
    } else {
        stale.hostname.clone()
    };
    format!("{identity} ({})", stale.journal_path)
}
fn live_service(
    name: String,
    observation: ProcessObservation,
    phase: String,
    reason_code: Option<String>,
) -> Option<Value> {
    let ProcessObservation::Live {
        reference,
        pid,
        uptime_seconds,
    } = observation
    else {
        return None;
    };
    Some(row(ServiceWireRow {
        name,
        pid,
        uptime_seconds,
        reference,
        phase,
        reason_code,
    }))
}

pub fn project_supervisor_status(mut input: SupervisorStatusWireInput) -> Map<String, Value> {
    let services = input
        .services
        .into_iter()
        .filter_map(|candidate| match candidate {
            ServiceCandidate::SupervisorSelf {
                reference,
                pid,
                uptime_seconds,
            } => Some(row(ServiceWireRow {
                name: "supervisor".to_owned(),
                pid,
                uptime_seconds,
                reference,
                phase: "running".to_owned(),
                reason_code: None,
            })),
            ServiceCandidate::App { name, observation } => {
                live_service(name, observation, "running".to_owned(), None)
            }
            ServiceCandidate::Provider {
                provider,
                observation,
                phase,
                reason_code,
            } => live_service(
                provider.as_str().to_owned(),
                observation,
                phase.as_str().to_owned(),
                reason_code.map(|code| code.as_str().to_owned()),
            ),
        })
        .collect();
    let crashed = input
        .crashed
        .into_iter()
        .map(|candidate| {
            row(CrashedWireRow {
                name: candidate.name,
                restart_attempts: candidate.restart_attempts,
                phase: candidate.phase.as_str().to_owned(),
                reason_code: candidate.reason_code.map(|code| code.as_str().to_owned()),
            })
        })
        .collect();
    let tasks = input
        .queue
        .tasks
        .into_iter()
        .map(|task| {
            row(TaskWireRow {
                reference: task.reference,
                name: task.partition.as_str().to_owned(),
                max_runtime_seconds: task.cap_seconds,
                duration_seconds: task.duration_seconds,
                slow: task.slow,
                stuck: task.stuck,
            })
        })
        .collect();
    let recent_tasks = input
        .queue
        .recent_tasks
        .into_iter()
        .map(|task| {
            row(RecentTaskWireRow {
                reference: task.reference,
                exit_status: task.exit_status,
                scheduler_name: task.scheduler_name,
            })
        })
        .collect();
    let schedules = input
        .schedules
        .into_iter()
        .map(|schedule| {
            row(ScheduleWireRow {
                name: schedule.name,
                every: schedule.every,
                last_run: schedule.last_run,
                due: schedule.due,
                next_run: schedule.next_run,
                daily_time: schedule.daily_time,
                weekly_day: schedule.weekly_day,
                weekly_time: schedule.weekly_time,
            })
        })
        .collect();
    input
        .stale_heartbeats
        .sort_by(|left, right| left.source_filename.cmp(&right.source_filename));
    let stale_heartbeats = input
        .stale_heartbeats
        .iter()
        .map(|stale| row(stale_display(stale)))
        .collect();
    let stale_heartbeat_details = input
        .stale_heartbeats
        .into_iter()
        .map(|stale| {
            row(StaleHeartbeatDetailWireRow {
                hostname: stale.hostname,
                heartbeat_schema: stale.identity.schema_name().to_owned(),
                legacy_machine_id_prefix: stale
                    .identity
                    .legacy_machine_id_prefix()
                    .map(str::to_owned),
                writer_id_prefix: stale.identity.writer_id_prefix().map(str::to_owned),
                run_id: stale.identity.run_id().map(str::to_owned),
                journal_path: stale.journal_path,
                pid: (!stale.malformed).then_some(stale.pid).flatten(),
                wall_time: (!stale.malformed).then_some(stale.wall_time).flatten(),
                malformed: stale.malformed,
                reason_code: if stale.malformed {
                    "malformed-heartbeat".to_owned()
                } else {
                    "stale-heartbeat".to_owned()
                },
            })
        })
        .collect();
    Map::from_iter([
        ("services".into(), Value::Array(services)),
        ("crashed".into(), Value::Array(crashed)),
        ("tasks".into(), Value::Array(tasks)),
        ("recent_tasks".into(), Value::Array(recent_tasks)),
        ("queues".into(), row(input.queue.queues)),
        ("stale_heartbeats".into(), Value::Array(stale_heartbeats)),
        (
            "stale_heartbeat_details".into(),
            Value::Array(stale_heartbeat_details),
        ),
        ("schedules".into(), Value::Array(schedules)),
        ("callosum_clients".into(), row(input.callosum_clients)),
    ])
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::partition::Partition;
    use crate::queue::{TaskHistoryRecord, TaskStatus};
    use serde_json::Value;
    use std::collections::BTreeMap;
    use std::time::SystemTime;

    fn expected(text: &str) -> Value {
        serde_json::from_str(text).unwrap()
    }

    fn input(services: Vec<ServiceCandidate>) -> SupervisorStatusWireInput {
        SupervisorStatusWireInput {
            services,
            crashed: vec![],
            queue: TaskQueueStatusSnapshot {
                tasks: vec![],
                recent_tasks: vec![],
                queues: BTreeMap::new(),
            },
            stale_heartbeats: vec![],
            schedules: vec![],
            callosum_clients: 0,
        }
    }

    fn live(reference: &str, pid: u32, uptime_seconds: u64) -> ProcessObservation {
        ProcessObservation::Live {
            reference: reference.into(),
            pid,
            uptime_seconds,
        }
    }

    fn crashed(
        name: &str,
        restart_attempts: u32,
        phase: RuntimePhase,
        reason_code: Option<ReasonCode>,
    ) -> CrashedServiceCandidate {
        CrashedServiceCandidate {
            name: name.into(),
            restart_attempts,
            phase,
            reason_code,
        }
    }

    fn task(
        partition: &str,
        reference: &str,
        duration_seconds: u64,
        cap_seconds: u64,
        slow: bool,
        stuck: bool,
    ) -> TaskStatus {
        TaskStatus {
            partition: Partition::new(partition),
            reference: reference.into(),
            command: vec![],
            duration_seconds,
            cap_seconds,
            slow,
            stuck,
        }
    }

    fn recent(
        partition: &str,
        reference: &str,
        exit_status: &str,
        scheduler_name: Option<&str>,
    ) -> TaskHistoryRecord {
        TaskHistoryRecord {
            partition: Partition::new(partition),
            command: vec![],
            reference: reference.into(),
            ended_at: SystemTime::UNIX_EPOCH,
            exit_status: exit_status.into(),
            scheduler_name: scheduler_name.map(str::to_owned),
        }
    }

    fn schedule(
        name: &str,
        every: &str,
        last_run: Option<f64>,
        due: bool,
        next_run: i64,
        daily_time: Option<&str>,
        weekly: Option<(&str, &str)>,
    ) -> ScheduleStatus {
        ScheduleStatus {
            name: name.into(),
            every: every.into(),
            last_run,
            due,
            next_run,
            daily_time: daily_time.map(str::to_owned),
            weekly_day: weekly.map(|(day, _)| day.to_owned()),
            weekly_time: weekly.map(|(_, time)| time.to_owned()),
        }
    }

    fn stale(
        source_filename: Vec<u8>,
        hostname: &str,
        identity: SyncPeerIdentity,
        journal_path: &str,
        pid: Option<u32>,
        wall_time: Option<&str>,
        malformed: bool,
    ) -> StaleHeartbeatWireInput {
        StaleHeartbeatWireInput {
            source_filename,
            hostname: hostname.into(),
            identity,
            journal_path: journal_path.into(),
            pid,
            wall_time: wall_time.map(str::to_owned),
            malformed,
        }
    }

    fn base_input() -> SupervisorStatusWireInput {
        SupervisorStatusWireInput {
            services: vec![
                ServiceCandidate::SupervisorSelf {
                    reference: "sup-ref".into(),
                    pid: 1,
                    uptime_seconds: 2,
                },
                ServiceCandidate::App {
                    name: "convey".into(),
                    observation: live("app-ref", 3, 4),
                },
                ServiceCandidate::Provider {
                    provider: ProviderName::Local,
                    observation: live("provider-ref", 5, 6),
                    phase: RuntimePhase::CleanupFailed,
                    reason_code: Some(ReasonCode::known("cleanup-attempt-failed")),
                },
            ],
            crashed: vec![crashed(
                "local",
                7,
                RuntimePhase::CleanupFailed,
                Some(ReasonCode::known("cleanup-attempt-failed")),
            )],
            queue: TaskQueueStatusSnapshot {
                tasks: vec![task("daily", "task-ref", 8, 9, true, false)],
                recent_tasks: vec![recent("daily", "recent-ref", "ok", None)],
                queues: BTreeMap::from([("z".into(), 1), ("a".into(), 2)]),
            },
            stale_heartbeats: vec![
                stale(
                    vec![0xff],
                    "",
                    SyncPeerIdentity::LegacyV1 {
                        legacy_machine_id_prefix: "01234567".to_owned(),
                    },
                    "/one",
                    Some(10),
                    Some(""),
                    false,
                ),
                stale(
                    b"a".to_vec(),
                    "",
                    SyncPeerIdentity::Unidentified,
                    "/two",
                    Some(99),
                    Some("raw"),
                    true,
                ),
            ],
            schedules: vec![schedule(
                "weekly",
                "weekly",
                None,
                false,
                11,
                None,
                Some(("mon", "09:00")),
            )],
            callosum_clients: 12,
        }
    }

    fn stale_rows() -> Vec<StaleHeartbeatWireInput> {
        vec![
            stale(
                vec![0xff],
                "z-host",
                SyncPeerIdentity::Unidentified,
                "/z",
                Some(3),
                Some("z"),
                false,
            ),
            stale(
                b"a".to_vec(),
                "",
                SyncPeerIdentity::LegacyV1 {
                    legacy_machine_id_prefix: "abcdefgh".to_owned(),
                },
                "/a",
                None,
                Some(""),
                false,
            ),
            stale(
                vec![0, 0xff],
                "",
                SyncPeerIdentity::Unidentified,
                "/zero",
                None,
                None,
                true,
            ),
        ]
    }

    #[test]
    fn projects_complete_hand_authored_payload() {
        let output = Value::Object(project_supervisor_status(base_input()));
        assert_eq!(
            output,
            expected(
                r#"{"services":[{"name":"supervisor","pid":1,"uptime_seconds":2,"ref":"sup-ref","phase":"running","reason_code":null},{"name":"convey","pid":3,"uptime_seconds":4,"ref":"app-ref","phase":"running","reason_code":null},{"name":"local","pid":5,"uptime_seconds":6,"ref":"provider-ref","phase":"cleanup-failed","reason_code":"cleanup-attempt-failed"}],"crashed":[{"name":"local","restart_attempts":7,"phase":"cleanup-failed","reason_code":"cleanup-attempt-failed"}],"tasks":[{"ref":"task-ref","name":"daily","max_runtime_seconds":9,"duration_seconds":8,"slow":true,"stuck":false}],"recent_tasks":[{"ref":"recent-ref","exit_status":"ok","scheduler_name":null}],"queues":{"a":2,"z":1},"stale_heartbeats":["(unknown) (/two)","01234567 (/one)"],"stale_heartbeat_details":[{"hostname":"","heartbeat_schema":"unidentified","legacy_machine_id_prefix":null,"writer_id_prefix":null,"run_id":null,"journal_path":"/two","pid":null,"wall_time":null,"malformed":true,"reason_code":"malformed-heartbeat"},{"hostname":"","heartbeat_schema":"v1","legacy_machine_id_prefix":"01234567","writer_id_prefix":null,"run_id":null,"journal_path":"/one","pid":10,"wall_time":"","malformed":false,"reason_code":"stale-heartbeat"}],"schedules":[{"name":"weekly","every":"weekly","last_run":null,"due":false,"next_run":11,"daily_time":null,"weekly_day":"mon","weekly_time":"09:00"}],"callosum_clients":12}"#
            )
        );
    }

    #[test]
    fn projects_explicit_empty_payload() {
        let empty = input(vec![]);
        assert_eq!(
            Value::Object(project_supervisor_status(empty)),
            expected(
                r#"{"services":[],"crashed":[],"tasks":[],"recent_tasks":[],"queues":{},"stale_heartbeats":[],"stale_heartbeat_details":[],"schedules":[],"callosum_clients":0}"#
            )
        );
    }

    #[test]
    fn permutation_twins_preserve_input_array_order() {
        let mut input = base_input();
        input.services.reverse();
        input
            .crashed
            .push(crashed("second", 0, RuntimePhase::Failed, None));
        input
            .queue
            .tasks
            .push(task("other", "second-task", 0, 1, false, false));
        input
            .queue
            .recent_tasks
            .push(recent("other", "second-recent", "error", Some("nightly")));
        input.schedules.push(schedule(
            "daily",
            "daily",
            Some(1.0),
            true,
            2,
            Some("10:00"),
            None,
        ));
        let output = Value::Object(project_supervisor_status(input));
        assert_eq!(
            [
                &output["services"][0]["name"],
                &output["crashed"][1]["name"],
                &output["tasks"][1]["ref"],
                &output["recent_tasks"][1]["ref"],
                &output["schedules"][1]["name"]
            ],
            [
                &Value::String("local".into()),
                &Value::String("second".into()),
                &Value::String("second-task".into()),
                &Value::String("second-recent".into()),
                &Value::String("daily".into())
            ]
        );
    }

    #[test]
    fn phase_census_is_complete_and_absent_candidates_do_not_emit() {
        let expected_phases = [
            "not-desired",
            "observing",
            "artifact-not-ready",
            "host-blocked",
            "starting",
            "warming",
            "backoff",
            "retry-requested",
            "ready",
            "ready-proof-unavailable",
            "stop-deferred",
            "stopping",
            "stopped",
            "failed",
            "cleanup-failed",
            "state-corrupt",
            "state-unavailable",
        ];
        assert_eq!(RuntimePhase::ALL.map(RuntimePhase::as_str), expected_phases);
        for phase in RuntimePhase::ALL {
            for is_live in [true, false] {
                let input = input(vec![ServiceCandidate::Provider {
                    provider: ProviderName::Parakeet,
                    observation: if is_live {
                        live("ref", 1, 2)
                    } else {
                        ProcessObservation::ConfirmedAbsent
                    },
                    phase,
                    reason_code: Some(ReasonCode::from_wire("future-code")),
                }]);
                let output = project_supervisor_status(input);
                assert_eq!(
                    output["services"].as_array().unwrap().len(),
                    usize::from(is_live)
                );
                if is_live {
                    assert_eq!(output["services"][0]["phase"], phase.as_str());
                }
            }
        }
    }

    #[test]
    fn stale_projection_is_byte_identical_and_index_aligned() {
        let mut forward = base_input();
        forward.stale_heartbeats = stale_rows();
        let mut reverse = base_input();
        reverse.stale_heartbeats = stale_rows().into_iter().rev().collect();
        let mut shuffled = base_input();
        let rows = stale_rows();
        shuffled.stale_heartbeats = vec![
            rows.into_iter().nth(1).unwrap(),
            stale_rows().into_iter().nth(2).unwrap(),
            stale_rows().into_iter().next().unwrap(),
        ];
        let bytes = serde_json::to_vec(&Value::Object(project_supervisor_status(forward))).unwrap();
        assert_eq!(
            bytes,
            serde_json::to_vec(&Value::Object(project_supervisor_status(reverse))).unwrap()
        );
        assert_eq!(
            bytes,
            serde_json::to_vec(&Value::Object(project_supervisor_status(shuffled))).unwrap()
        );
        let output = Value::Object(project_supervisor_status(base_input()));
        assert!(!output["stale_heartbeats"][0].is_object());
        assert!(
            output["stale_heartbeats"]
                .as_array()
                .unwrap()
                .iter()
                .all(Value::is_string)
        );
        assert_eq!(
            output["stale_heartbeats"].as_array().unwrap().len(),
            output["stale_heartbeat_details"].as_array().unwrap().len()
        );
        assert_eq!(output["stale_heartbeat_details"][0]["journal_path"], "/two");
        assert_eq!(output["stale_heartbeat_details"][1]["wall_time"], "");
        assert_eq!(output["stale_heartbeat_details"][1]["pid"], 10);
        assert_eq!(output["stale_heartbeat_details"][0]["pid"], Value::Null);
        assert_eq!(
            output["stale_heartbeat_details"][0]["reason_code"],
            "malformed-heartbeat"
        );
        assert_eq!(
            output["stale_heartbeat_details"][1]["reason_code"],
            "stale-heartbeat"
        );
    }

    #[test]
    fn v2_stale_heartbeat_details_keep_writer_and_run_identity() {
        let mut input = input(vec![]);
        input.stale_heartbeats = vec![stale(
            b"solstone-v2.check".to_vec(),
            "foreign-host",
            SyncPeerIdentity::V2 {
                writer_id_prefix: "01234567".to_owned(),
                run_id: "fedcba9876543210fedcba9876543210".to_owned(),
            },
            "/foreign-journal",
            Some(42),
            Some("1234.5"),
            false,
        )];

        let output = Value::Object(project_supervisor_status(input));
        let detail = &output["stale_heartbeat_details"][0];
        assert_eq!(detail["heartbeat_schema"], "v2");
        assert_eq!(detail["writer_id_prefix"], "01234567");
        assert_eq!(detail["run_id"], "fedcba9876543210fedcba9876543210");
        assert_eq!(detail["legacy_machine_id_prefix"], Value::Null);
        assert_eq!(detail["pid"], 42);
        assert_eq!(detail["wall_time"], "1234.5");
        assert_eq!(detail["malformed"], false);
        assert_eq!(detail["reason_code"], "stale-heartbeat");
    }

    #[test]
    fn wire_contract_has_canonical_order_and_no_legacy_shapes() {
        let output = Value::Object(project_supervisor_status(base_input()));
        assert_eq!(
            output
                .as_object()
                .unwrap()
                .keys()
                .cloned()
                .collect::<Vec<_>>(),
            [
                "services",
                "crashed",
                "tasks",
                "recent_tasks",
                "queues",
                "stale_heartbeats",
                "stale_heartbeat_details",
                "schedules",
                "callosum_clients"
            ]
        );
        assert_eq!(output["services"][2]["name"], "local");
        assert_eq!(output["crashed"][0]["name"], "local");
        assert_ne!(output["tasks"][0], Value::String("task-ref".into()));
        assert_eq!(output["tasks"][0]["ref"], "task-ref");
        assert_eq!(output["tasks"][0]["name"], "daily");
        assert_eq!(output["tasks"][0]["max_runtime_seconds"], 9);
        for key in ["reference", "partition", "cap_seconds", "command"] {
            assert!(output["tasks"][0].get(key).is_none());
        }
        assert!(!output["schedules"][0].is_string());
        assert_eq!(output["schedules"][0]["weekly_time"], "09:00");
    }
}
