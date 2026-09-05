// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::convert::Infallible;
use std::future::Future;
use std::path::PathBuf;
use std::pin::Pin;
use std::task::{Context, Poll};
use std::time::Duration;

use axum::response::IntoResponse;
use axum::response::sse::{Event, KeepAlive, Sse};
use futures_core::Stream;
use serde_json::{Map, json};
use solstone_core_callosum::{
    CallosumConnectionPhase, CallosumReceiveEvent, CallosumSocketConnection,
};

type NextEvent =
    Pin<Box<dyn Future<Output = (CallosumSocketConnection, Option<Event>, bool)> + Send>>;

// The response body owns the connection, including while waiting for a message.
// Dropping the body drops the connection and signals its transport task to stop.
struct LiveEvents {
    next: Option<NextEvent>,
}

fn next_event(mut connection: CallosumSocketConnection) -> NextEvent {
    Box::pin(async move {
        let mut terminal = false;
        let event = match connection.next_event().await {
            Some(CallosumReceiveEvent::Envelope { envelope, .. }) => {
                Some(Event::default().json_data(envelope).expect("JSON envelope"))
            }
            Some(CallosumReceiveEvent::Continuity {
                generation,
                epoch,
                phase,
            }) => {
                let state = match phase {
                    CallosumConnectionPhase::Connecting { .. } => "connecting",
                    CallosumConnectionPhase::Unavailable { .. } => "unavailable",
                    CallosumConnectionPhase::Connected => "connected",
                    CallosumConnectionPhase::Gapped { .. } => "gapped",
                    CallosumConnectionPhase::Stopped { .. } => {
                        terminal = true;
                        "stopped"
                    }
                };
                Some(
                    Event::default()
                        .event("continuity")
                        .json_data(json!({
                            "state": state, "generation": generation, "epoch": epoch,
                        }))
                        .expect("JSON continuity"),
                )
            }
            None => None,
        };
        (connection, event, terminal)
    })
}

impl LiveEvents {
    fn new(mut connection: CallosumSocketConnection) -> Self {
        connection.start();
        Self {
            next: Some(next_event(connection)),
        }
    }
}

impl Stream for LiveEvents {
    type Item = Result<Event, Infallible>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let Some(next) = self.next.as_mut() else {
            return Poll::Ready(None);
        };
        let Poll::Ready((connection, event, terminal)) = next.as_mut().poll(cx) else {
            return Poll::Pending;
        };
        self.next = if terminal {
            None
        } else {
            event.as_ref().map(|_| next_event(connection))
        };
        Poll::Ready(event.map(Ok))
    }
}

pub async fn events(journal_root: PathBuf) -> axum::response::Response {
    let connection =
        CallosumSocketConnection::new(journal_root.join("health/callosum.sock"), Map::new());
    let mut response = Sse::new(LiveEvents::new(connection))
        .keep_alive(
            KeepAlive::new()
                .interval(Duration::from_secs(15))
                .text("heartbeat"),
        )
        .into_response();
    response.headers_mut().insert(
        axum::http::header::CACHE_CONTROL,
        axum::http::HeaderValue::from_static("no-cache"),
    );
    response.headers_mut().insert(
        "x-accel-buffering",
        axum::http::HeaderValue::from_static("no"),
    );
    response
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures_util::StreamExt;
    use solstone_core_callosum::CallosumRetrySource;

    struct ClosedRetries;

    impl CallosumRetrySource for ClosedRetries {
        fn next_attempt(&mut self) -> Pin<Box<dyn Future<Output = bool> + Send + '_>> {
            Box::pin(async { false })
        }
    }

    #[tokio::test]
    async fn stopped_reader_closes_sse_so_the_browser_can_retry() {
        let connection = CallosumSocketConnection::with_retry_source(
            "unused.sock",
            Map::new(),
            1,
            Box::new(ClosedRetries),
        );
        let response = Sse::new(LiveEvents::new(connection)).into_response();
        let mut body = response.into_body().into_data_stream();
        let wire = tokio::time::timeout(Duration::from_secs(1), async {
            let mut wire = String::new();
            while let Some(chunk) = body.next().await {
                wire.push_str(std::str::from_utf8(&chunk.unwrap()).unwrap());
            }
            wire
        })
        .await
        .expect("terminal reader must close the SSE response");
        assert!(wire.contains("event: continuity\n"));
        assert!(wire.contains("\"state\":\"stopped\""));
    }
}
