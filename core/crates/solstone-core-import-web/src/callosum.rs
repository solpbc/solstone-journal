// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::{path::Path, time::Duration};

use serde_json::{Map, Value, json};
use solstone_core_callosum::{
    CallosumConnectionPhase, CallosumOneShotError, CallosumOneShotSender, CallosumReceiveEvent,
    CallosumSocketConnection,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum BusError {
    Unavailable,
}

fn socket(root: &Path) -> std::path::PathBuf {
    root.join("health/callosum.sock")
}

fn send(root: &Path, value: Value) -> Result<(), BusError> {
    let line = format!(
        "{}\n",
        serde_json::to_string(&value).map_err(|_| BusError::Unavailable)?
    );
    CallosumOneShotSender::new(socket(root), Duration::from_secs(2))
        .send_line(&line)
        .map_err(|error| match error {
            CallosumOneShotError::Unavailable => BusError::Unavailable,
        })
}

fn request(task_id: &str, cmd: &[String]) -> Value {
    json!({
        "tract": "supervisor",
        "event": "request",
        "ref": task_id,
        "cmd": cmd,
        "queue_if_active_cmd_differs": true,
    })
}

pub(crate) fn request_required(root: &Path, task_id: &str, cmd: &[String]) -> Result<(), BusError> {
    send(root, request(task_id, cmd))
}

/// A bus subscription that requests a task and then hears how it ended. The supervisor
/// reports every requested task's end as `supervisor/stopped` with its `ref` and
/// `exit_code`, a failed spawn included.
pub(crate) struct TaskWatch {
    connection: CallosumSocketConnection,
}

impl TaskWatch {
    /// Subscribe, or nothing when the bus does not connect promptly.
    pub(crate) async fn subscribe(root: &Path) -> Option<Self> {
        let mut connection = CallosumSocketConnection::new(socket(root), Map::new());
        connection.start();
        let connected = tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                match connection.next_event().await {
                    Some(CallosumReceiveEvent::Continuity {
                        phase: CallosumConnectionPhase::Connected,
                        ..
                    }) => return true,
                    Some(CallosumReceiveEvent::Continuity {
                        phase:
                            CallosumConnectionPhase::Unavailable { .. }
                            | CallosumConnectionPhase::Stopped { .. },
                        ..
                    })
                    | None => return false,
                    Some(_) => {}
                }
            }
        })
        .await;
        matches!(connected, Ok(true)).then_some(Self { connection })
    }

    /// Request the task on this connection. The server registers a subscriber before it
    /// reads that subscriber's frames, so the task's end cannot race past this watch the way
    /// it could past a separate sending socket.
    pub(crate) fn request(&self, task_id: &str, cmd: &[String]) -> Result<(), BusError> {
        let Value::Object(mut fields) = request(task_id, cmd) else {
            return Err(BusError::Unavailable);
        };
        fields.remove("tract");
        fields.remove("event");
        if self.connection.emit("supervisor", "request", fields) {
            Ok(())
        } else {
            Err(BusError::Unavailable)
        }
    }

    /// The exit code the supervisor reports for `task_id`, or nothing if the stream ends
    /// first. A missed event leaves the import to the running bound, as before.
    pub(crate) async fn exit_code(mut self, task_id: &str) -> Option<i32> {
        loop {
            match self.connection.next_event().await? {
                CallosumReceiveEvent::Envelope { envelope, .. }
                    if envelope.tract == "supervisor"
                        && envelope.event == "stopped"
                        && envelope.extra.get("ref").and_then(Value::as_str) == Some(task_id) =>
                {
                    return envelope
                        .extra
                        .get("exit_code")
                        .and_then(Value::as_i64)
                        .and_then(|code| i32::try_from(code).ok())
                        .or(Some(-1));
                }
                CallosumReceiveEvent::Continuity {
                    phase: CallosumConnectionPhase::Stopped { .. },
                    ..
                } => return None,
                _ => {}
            }
        }
    }
}

#[cfg(all(test, feature = "full-tests"))]
mod tests {
    use super::*;
    use solstone_core_callosum::CallosumSocketServer;

    async fn connected(connection: &mut CallosumSocketConnection) {
        while !matches!(
            connection.next_event().await,
            Some(CallosumReceiveEvent::Continuity {
                phase: CallosumConnectionPhase::Connected,
                ..
            })
        ) {}
    }

    #[tokio::test]
    async fn a_watch_hears_the_end_of_the_task_it_requested_even_an_immediate_one() {
        let root = tempfile::TempDir::new().unwrap();
        let server = CallosumSocketServer::bind(socket(root.path()))
            .await
            .unwrap();
        // Stands in for the supervisor, which reads requests off its own bus connection.
        let mut supervisor = CallosumSocketConnection::new(socket(root.path()), Map::new());
        supervisor.start();
        connected(&mut supervisor).await;

        let watch = TaskWatch::subscribe(root.path()).await.unwrap();
        watch
            .request("task-1", &["journal".to_owned(), "importer".to_owned()])
            .unwrap();
        let request = loop {
            let message = supervisor.next_message().await.unwrap();
            if message.tract == "supervisor" && message.event == "request" {
                break message;
            }
        };
        assert_eq!(request.extra["ref"], "task-1");
        assert_eq!(request.extra["cmd"], json!(["journal", "importer"]));
        assert_eq!(request.extra["queue_if_active_cmd_differs"], true);
        for (reference, exit_code) in [("another-task", 0), ("task-1", 2)] {
            let fields = Map::from_iter([
                ("ref".to_owned(), json!(reference)),
                ("exit_code".to_owned(), json!(exit_code)),
            ]);
            assert!(supervisor.emit("supervisor", "stopped", fields));
        }

        let exit_code = tokio::time::timeout(Duration::from_secs(5), watch.exit_code("task-1"))
            .await
            .unwrap();
        assert_eq!(exit_code, Some(2));
        server.stop().await;
    }

    #[tokio::test]
    async fn no_bus_means_no_watch() {
        let root = tempfile::TempDir::new().unwrap();
        assert!(TaskWatch::subscribe(root.path()).await.is_none());
    }
}
