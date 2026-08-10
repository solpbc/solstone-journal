// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use serde::Serialize;
use serde_json::{Map, Value, json};
use solstone_core_system::lifecycle::ForeignWriter;

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
    pub tasks: Value,
    pub recent_tasks: Value,
    pub queues: Value,
    pub stale: Vec<StaleHeartbeatStatus>,
    pub schedules: Value,
    pub clients: usize,
}

pub(crate) fn values(fields: StatusFields) -> Map<String, Value> {
    Map::from_iter([
        ("services".into(), fields.services),
        ("crashed".into(), fields.crashed),
        ("tasks".into(), fields.tasks),
        ("recent_tasks".into(), fields.recent_tasks),
        ("queues".into(), fields.queues),
        ("stale_heartbeats".into(), json!(fields.stale)),
        ("schedules".into(), fields.schedules),
        ("callosum_clients".into(), json!(fields.clients)),
    ])
}
