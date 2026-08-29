// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc
use crate::context::CheckContext;
use serde_json::Value;
use solstone_core_callosum::CallosumSocketConnection;
use std::time::Duration;

const STOP_CLEANUP_BOUND: Duration = Duration::from_millis(50);

pub fn fetch(context: &CheckContext) -> Option<Value> {
    if !context.callosum_socket_path.exists() {
        return None;
    }
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .ok()?;
    runtime.block_on(async {
        let mut connection =
            CallosumSocketConnection::new(&context.callosum_socket_path, serde_json::Map::new());
        connection.start();
        let status = tokio::time::timeout(context.service_status_timeout, async {
            loop {
                let message = connection.next_message().await?;
                if message.tract == "supervisor" && message.event == "status" {
                    return Some(Value::Object(message.extra));
                }
            }
        })
        .await
        .ok()
        .flatten();
        // `stop` signals shutdown before it awaits the connection task. A doctor
        // probe has no outbound work to drain, so an unresponsive peer must not
        // extend the caller's status budget by the wire client's longer join bound.
        let _ = tokio::time::timeout(STOP_CLEANUP_BOUND, connection.stop()).await;
        status
    })
}
