// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc
use crate::context::CheckContext;
use serde_json::Value;
use solstone_core_callosum::CallosumSocketConnection;
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
        connection.stop().await;
        status
    })
}
