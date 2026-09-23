// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc
use crate::context::CheckContext;
use serde_json::Value;
use solstone_core_callosum::{
    CallosumConnectionPhase, CallosumReceiveEvent, CallosumSocketConnection,
};
use std::time::Duration;

const STOP_CLEANUP_BOUND: Duration = Duration::from_millis(50);

fn is_fresh_status_timestamp(timestamp_ms: i64, watermark_ms: i64, now_ms: i64) -> bool {
    timestamp_ms > watermark_ms && timestamp_ms <= now_ms
}

/// Why a status probe has no status.
///
/// ⛔ These are not interchangeable.  Only [`Self::NoSocket`] establishes that
/// nothing is running; the rest mean the probe could not reach a conclusion,
/// and a probe that cannot reach a subsystem must not report that subsystem
/// dead.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Unavailable {
    /// No callosum socket exists, so there is nothing listening to answer.
    NoSocket,
    /// The probe could not construct its own async runtime.
    ProbeRuntime,
    /// A connection was made and no status frame arrived within the budget.
    Timeout,
    /// The status connection closed before a status frame arrived.
    Transport,
}

impl Unavailable {
    /// Short cause, for a diagnostic line.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::NoSocket => "nothing is listening",
            Self::ProbeRuntime => "nothing answered",
            Self::Timeout => "took too long to answer",
            Self::Transport => "the connection dropped",
        }
    }
}

pub fn fetch(context: &CheckContext) -> Result<Value, Unavailable> {
    if !context.callosum_socket_path.exists() {
        return Err(Unavailable::NoSocket);
    }
    let Ok(runtime) = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    else {
        return Err(Unavailable::ProbeRuntime);
    };
    runtime.block_on(async {
        let mut connection =
            CallosumSocketConnection::new(&context.callosum_socket_path, serde_json::Map::new());
        connection.start();
        let status = match tokio::time::timeout(context.service_status_timeout, async {
            loop {
                let Some(message) = connection.next_message().await else {
                    return Err(Unavailable::Transport);
                };
                if message.tract == "supervisor" && message.event == "status" {
                    return Ok(Value::Object(message.extra));
                }
            }
        })
        .await
        {
            Ok(result) => result,
            Err(_elapsed) => Err(Unavailable::Timeout),
        };
        // `stop` signals shutdown before it awaits the connection task. A doctor
        // probe has no outbound work to drain, so an unresponsive peer must not
        // extend the caller's status budget by the wire client's longer join bound.
        let _ = tokio::time::timeout(STOP_CLEANUP_BOUND, connection.stop()).await;
        status
    })
}

/// Fetch a fresh native Sense status beacon from the current Callosum stream.
///
/// Status events are periodic and Callosum can deliver a frame that was queued
/// just before this client joined. Requiring a server timestamp strictly after
/// the connection's `Connected` watermark prevents such a queued old beacon
/// from making the live check look healthy.
pub fn fetch_observe_status(context: &CheckContext) -> Result<Value, Unavailable> {
    if !context.callosum_socket_path.exists() {
        return Err(Unavailable::NoSocket);
    }
    let Ok(runtime) = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    else {
        return Err(Unavailable::ProbeRuntime);
    };
    runtime.block_on(async {
        let mut connection =
            CallosumSocketConnection::new(&context.callosum_socket_path, serde_json::Map::new());
        connection.start();
        let status = match tokio::time::timeout(context.service_status_timeout, async {
            let mut connected_watermark_ms = None;
            loop {
                match connection.next_event().await {
                    Some(CallosumReceiveEvent::Continuity {
                        phase: CallosumConnectionPhase::Connected,
                        ..
                    }) => {
                        connected_watermark_ms = Some(chrono::Utc::now().timestamp_millis());
                    }
                    Some(CallosumReceiveEvent::Envelope { envelope, .. }) => {
                        let Some(watermark_ms) = connected_watermark_ms else {
                            continue;
                        };
                        if envelope.tract != "observe" || envelope.event != "status" {
                            continue;
                        }
                        let now_ms = chrono::Utc::now().timestamp_millis();
                        let Some(timestamp_ms) = envelope.ts else {
                            continue;
                        };
                        if !is_fresh_status_timestamp(timestamp_ms, watermark_ms, now_ms) {
                            continue;
                        }
                        if envelope.extra.get("name").and_then(Value::as_str)
                            == Some("native.observe")
                        {
                            return Ok(Value::Object(envelope.extra));
                        }
                    }
                    Some(CallosumReceiveEvent::Continuity { .. }) => {}
                    None => return Err(Unavailable::Transport),
                }
            }
        })
        .await
        {
            Ok(result) => result,
            Err(_elapsed) => Err(Unavailable::Timeout),
        };
        let _ = tokio::time::timeout(STOP_CLEANUP_BOUND, connection.stop()).await;
        status
    })
}

#[cfg(test)]
mod tests {
    use super::is_fresh_status_timestamp;

    #[test]
    fn observe_status_must_be_strictly_newer_than_connection_and_not_from_the_future() {
        assert!(!is_fresh_status_timestamp(9, 10, 12));
        assert!(!is_fresh_status_timestamp(10, 10, 12));
        assert!(is_fresh_status_timestamp(11, 10, 12));
        assert!(!is_fresh_status_timestamp(13, 10, 12));
    }
}
