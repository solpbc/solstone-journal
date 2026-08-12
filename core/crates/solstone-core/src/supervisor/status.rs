// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use serde::Serialize;
use serde_json::{Map, Value, json};
use solstone_core_system::lifecycle::ForeignWriter;
use solstone_core_system::queue::TaskQueueStatusSnapshot;

#[derive(Clone, Serialize)]
pub(crate) struct StaleHeartbeatStatus {
    pub hostname: String,
    pub journal_path: String,
    pub pid: Option<u32>,
    pub machine_id_prefix: String,
    pub wall_time: String,
    pub malformed: bool,
}
impl From<&ForeignWriter> for StaleHeartbeatStatus {
    fn from(writer: &ForeignWriter) -> Self {
        Self {
            hostname: writer.hostname.clone(),
            journal_path: writer.journal_path.clone(),
            pid: writer.pid,
            machine_id_prefix: writer.machine_id.chars().take(8).collect(),
            wall_time: writer.wall_time.clone(),
            malformed: writer.malformed,
        }
    }
}

pub(crate) struct StatusFields {
    pub services: Value,
    pub crashed: Value,
    pub queue: TaskQueueStatusSnapshot,
    pub stale: Vec<StaleHeartbeatStatus>,
    pub schedules: Value,
    pub clients: usize,
}

pub(crate) fn values(fields: StatusFields) -> Map<String, Value> {
    let TaskQueueStatusSnapshot {
        tasks,
        recent_tasks,
        queues,
    } = fields.queue;
    Map::from_iter([
        ("services".into(), fields.services),
        ("crashed".into(), fields.crashed),
        (
            "tasks".into(),
            json!(
                tasks
                    .into_iter()
                    .map(|task| task.reference)
                    .collect::<Vec<_>>()
            ),
        ),
        (
            "recent_tasks".into(),
            json!(
                recent_tasks
                    .into_iter()
                    .map(|task| json!({
                        "ref": task.reference,
                        "exit_status": task.exit_status,
                        "scheduler_name": task.scheduler_name,
                    }))
                    .collect::<Vec<_>>()
            ),
        ),
        ("queues".into(), json!(queues)),
        ("stale_heartbeats".into(), json!(fields.stale)),
        ("schedules".into(), fields.schedules),
        ("callosum_clients".into(), json!(fields.clients)),
    ])
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::time::{Duration, SystemTime};

    use serde_json::json;
    use solstone_core_system::partition::Partition;
    use solstone_core_system::queue::{TaskHistoryRecord, TaskStatus};

    use super::*;

    fn fields(queue: TaskQueueStatusSnapshot) -> StatusFields {
        StatusFields {
            services: json!([]),
            crashed: json!([]),
            queue,
            stale: Vec::new(),
            schedules: json!([]),
            clients: 0,
        }
    }

    #[test]
    fn queue_projection_preserves_complete_snapshot_order_and_depths() {
        let partition = Partition::new("svc");
        let snapshot = TaskQueueStatusSnapshot {
            tasks: vec![
                TaskStatus {
                    partition: partition.clone(),
                    reference: "z-task".to_owned(),
                    command: vec!["z".to_owned()],
                    duration_seconds: 1,
                    cap_seconds: 10,
                    slow: false,
                    stuck: false,
                },
                TaskStatus {
                    partition: partition.clone(),
                    reference: "a-task".to_owned(),
                    command: vec!["a".to_owned()],
                    duration_seconds: 2,
                    cap_seconds: 10,
                    slow: false,
                    stuck: false,
                },
            ],
            recent_tasks: vec![
                TaskHistoryRecord {
                    partition: partition.clone(),
                    command: vec!["first".to_owned()],
                    reference: "recent-z".to_owned(),
                    ended_at: SystemTime::UNIX_EPOCH + Duration::from_secs(1),
                    exit_status: "ok".to_owned(),
                    scheduler_name: Some("daily".to_owned()),
                },
                TaskHistoryRecord {
                    partition,
                    command: vec!["second".to_owned()],
                    reference: "recent-a".to_owned(),
                    ended_at: SystemTime::UNIX_EPOCH + Duration::from_secs(2),
                    exit_status: "error".to_owned(),
                    scheduler_name: None,
                },
            ],
            queues: BTreeMap::from([("b".to_owned(), 2), ("a".to_owned(), 1)]),
        };

        let output = values(fields(snapshot));
        assert_eq!(output["tasks"], json!(["z-task", "a-task"]));
        assert_eq!(
            output["recent_tasks"],
            json!([
                {"ref": "recent-z", "exit_status": "ok", "scheduler_name": "daily"},
                {"ref": "recent-a", "exit_status": "error", "scheduler_name": null},
            ])
        );
        assert_eq!(output["queues"], json!({"a": 1, "b": 2}));
    }

    #[test]
    fn queue_projection_replaces_all_three_collections_with_empty_values() {
        let output = values(fields(TaskQueueStatusSnapshot {
            tasks: Vec::new(),
            recent_tasks: Vec::new(),
            queues: BTreeMap::new(),
        }));

        assert_eq!(output["tasks"], json!([]));
        assert_eq!(output["recent_tasks"], json!([]));
        assert_eq!(output["queues"], json!({}));
    }
}
