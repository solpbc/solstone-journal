// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::convert::Infallible;
use std::future::Future;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};
use std::time::Duration;

use axum::response::IntoResponse;
use axum::response::sse::{Event, KeepAlive, Sse};
use futures_core::Stream;
use serde_json::{Map, json};
use solstone_core_callosum::{
    BrainStartupCapture, BrainStartupCursor, CallosumConnectionPhase, CallosumReceiveEvent,
    CallosumSocketConnection,
};

type NextEvent = Pin<
    Box<
        dyn Future<
                Output = (
                    CallosumSocketConnection,
                    Option<StartupReader>,
                    Option<Event>,
                    bool,
                ),
            > + Send,
    >,
>;

struct StartupReader {
    capture: Arc<BrainStartupCapture>,
    cursor: BrainStartupCursor,
}
enum Received {
    Bus(Option<CallosumReceiveEvent>),
    Startup(Option<solstone_core_callosum::BrainStartupEvent>),
}

// The response body owns the connection, including while waiting for a message.
// Dropping the body drops the connection and signals its transport task to stop.
struct LiveEvents {
    next: Option<NextEvent>,
}

fn next_event(
    mut connection: CallosumSocketConnection,
    mut startup: Option<StartupReader>,
) -> NextEvent {
    Box::pin(async move {
        let mut terminal = false;
        let received = if let Some(reader) = &mut startup {
            tokio::select! {
                biased;
                event = reader.capture.next(&mut reader.cursor) => Received::Startup(event),
                event = connection.next_event() => Received::Bus(event),
            }
        } else {
            Received::Bus(connection.next_event().await)
        };
        let event = match received {
            Received::Startup(Some(event)) => {
                terminal = event.terminal;
                Some(Event::default().event(event.name).data(event.data))
            }
            Received::Startup(None) => {
                terminal = true;
                None
            }
            Received::Bus(event) => match event {
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
            },
        };
        (connection, startup, event, terminal)
    })
}

impl LiveEvents {
    fn new(
        mut connection: CallosumSocketConnection,
        capture: Option<Arc<BrainStartupCapture>>,
    ) -> Self {
        connection.start();
        let startup = capture.map(|capture| StartupReader {
            capture,
            cursor: BrainStartupCursor::default(),
        });
        Self {
            next: Some(next_event(connection, startup)),
        }
    }
}

impl Stream for LiveEvents {
    type Item = Result<Event, Infallible>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let Some(next) = self.next.as_mut() else {
            return Poll::Ready(None);
        };
        let Poll::Ready((connection, startup, event, terminal)) = next.as_mut().poll(cx) else {
            return Poll::Pending;
        };
        self.next = if terminal {
            None
        } else {
            event.as_ref().map(|_| next_event(connection, startup))
        };
        Poll::Ready(event.map(Ok))
    }
}

pub async fn events(
    journal_root: PathBuf,
    capture: Option<Arc<BrainStartupCapture>>,
) -> axum::response::Response {
    let connection =
        CallosumSocketConnection::new(journal_root.join("health/callosum.sock"), Map::new());
    let mut response = Sse::new(LiveEvents::new(connection, capture))
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
        let response = Sse::new(LiveEvents::new(connection, None)).into_response();
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
    #[cfg(feature = "full-tests")]
    #[tokio::test]
    async fn stop_during_prefix_handoff_is_terminal_and_cannot_certify_replacement() {
        let capture = BrainStartupCapture::new();
        let (emitter, _receiver) =
            solstone_core_callosum::test_support::diagnostic_fixture_with_startup_capture(
                64,
                Arc::clone(&capture),
            );
        assert!(emitter.emit("supervisor","brain_observation",serde_json::from_value(serde_json::json!({
            "observation_id":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa","caller":"mobile_refresh","phase":"terminal","outcome":"ready"
        })).unwrap()));
        let connection = CallosumSocketConnection::with_retry_source(
            "unused",
            Map::new(),
            1,
            Box::new(ClosedRetries),
        );
        let response =
            Sse::new(LiveEvents::new(connection, Some(Arc::clone(&capture)))).into_response();
        let mut body = response.into_body().into_data_stream();
        let first = String::from_utf8(body.next().await.unwrap().unwrap().to_vec()).unwrap();
        assert!(first.contains("event: brain_startup\n"));
        capture.close();
        let closed = String::from_utf8(body.next().await.unwrap().unwrap().to_vec()).unwrap();
        assert!(closed.contains("\"coverage\":\"closed\""));
        assert!(body.next().await.is_none());
        let replacement = BrainStartupCapture::new();
        let replacement = replacement
            .next(&mut BrainStartupCursor::default())
            .await
            .unwrap();
        let replacement: serde_json::Value = serde_json::from_str(&replacement.data).unwrap();
        assert_eq!(replacement["through"], 0);
        assert!(!first.contains(replacement["generation"].as_str().unwrap()));
    }
}
