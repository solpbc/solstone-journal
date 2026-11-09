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
    journal: Option<PathBuf>,
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
        journal: Option<PathBuf>,
    ) -> Self {
        connection.start();
        if let Some(ref j) = journal {
            solstone_core_transcripts_web::note_dashboard_stream(j, true);
        }
        let startup = capture.map(|capture| StartupReader {
            capture,
            cursor: BrainStartupCursor::default(),
        });
        Self {
            next: Some(next_event(connection, startup)),
            journal,
        }
    }
}

impl Drop for LiveEvents {
    fn drop(&mut self) {
        if let Some(ref j) = self.journal {
            solstone_core_transcripts_web::note_dashboard_stream(j, false);
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

/// How often the stream says it is alive with a named event a page can see.
/// A comment line keeps intermediaries open but never reaches the page.
pub const HEARTBEAT_INTERVAL: Duration = Duration::from_secs(5);
/// How long the browser waits before reconnecting a dropped stream itself.
pub const RECONNECT_DELAY: Duration = Duration::from_secs(2);

/// The live events, opened by one `hello` naming this build and carrying the
/// browser's reconnect delay, with a named heartbeat at a steady interval. The heartbeat frame also carries the plain comment line older
/// readers and intermediaries rely on. Ends when the live events end.
struct Heartbeating<S> {
    hello: Option<Event>,
    inner: S,
    tick: tokio::time::Interval,
    done: bool,
}

impl<S> Heartbeating<S> {
    fn new(inner: S, build: &str, period: Duration) -> Self {
        let mut tick = tokio::time::interval_at(tokio::time::Instant::now() + period, period);
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        let hello = Event::default()
            .event("hello")
            .retry(RECONNECT_DELAY)
            .json_data(json!({ "server_build": build }))
            .expect("JSON hello");
        Self {
            hello: Some(hello),
            inner,
            tick,
            done: false,
        }
    }
}

fn heartbeat_event() -> Event {
    let at = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_millis() as u64)
        .unwrap_or(0);
    Event::default()
        .comment("heartbeat")
        .event("heartbeat")
        .json_data(json!({ "at": at }))
        .expect("JSON heartbeat")
}

impl<S> Stream for Heartbeating<S>
where
    S: Stream<Item = Result<Event, Infallible>> + Unpin,
{
    type Item = Result<Event, Infallible>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        if self.done {
            return Poll::Ready(None);
        }
        if let Some(hello) = self.hello.take() {
            return Poll::Ready(Some(Ok(hello)));
        }
        // The tick first: it is ready at most once a period, so a busy
        // stream still beats, and the live events are never starved.
        if self.tick.poll_tick(cx).is_ready() {
            return Poll::Ready(Some(Ok(heartbeat_event())));
        }
        match Pin::new(&mut self.inner).poll_next(cx) {
            Poll::Ready(None) => {
                self.done = true;
                Poll::Ready(None)
            }
            other => other,
        }
    }
}

pub async fn events(
    journal_root: PathBuf,
    capture: Option<Arc<BrainStartupCapture>>,
) -> axum::response::Response {
    let connection =
        CallosumSocketConnection::new(journal_root.join("health/callosum.sock"), Map::new());
    let live = LiveEvents::new(connection, capture, Some(journal_root));
    let mut response = Sse::new(Heartbeating::new(
        live,
        crate::assets::server_build(),
        HEARTBEAT_INTERVAL,
    ))
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

    fn quiet_live_events() -> impl Stream<Item = Result<Event, Infallible>> + Unpin {
        futures_util::stream::pending()
    }

    async fn read_wire(
        body: &mut (impl futures_core::Stream<Item = Result<axum::body::Bytes, axum::Error>> + Unpin),
        until: &str,
    ) -> String {
        let mut wire = String::new();
        tokio::time::timeout(Duration::from_secs(2), async {
            while !wire.contains(until) {
                let chunk = body.next().await.expect("stream open").unwrap();
                wire.push_str(std::str::from_utf8(&chunk).unwrap());
            }
        })
        .await
        .expect("frame arrives");
        wire
    }

    #[tokio::test]
    async fn stream_opens_with_its_build_and_reconnect_delay() {
        let stream = Heartbeating::new(quiet_live_events(), "0123456789abcdef", HEARTBEAT_INTERVAL);
        let response = Sse::new(stream).into_response();
        let mut body = response.into_body().into_data_stream();
        let wire = read_wire(&mut body, "\n\n").await;
        assert!(wire.starts_with("event: hello\n"), "{wire}");
        assert!(wire.contains("retry: 2000\n"), "{wire}");
        assert!(
            wire.contains("data: {\"server_build\":\"0123456789abcdef\"}\n"),
            "{wire}"
        );
        assert!(!wire.contains("id:"), "{wire}");
    }

    #[tokio::test]
    async fn quiet_stream_sends_a_named_heartbeat_beside_the_comment() {
        let stream = Heartbeating::new(quiet_live_events(), "build", Duration::from_millis(30));
        let response = Sse::new(stream).into_response();
        let mut body = response.into_body().into_data_stream();
        let wire = read_wire(&mut body, "\"at\":").await;
        assert!(wire.contains(": heartbeat\n"), "{wire}");
        assert!(wire.contains("event: heartbeat\n"), "{wire}");
        assert!(wire.contains("data: {\"at\":"), "{wire}");
    }

    #[tokio::test]
    async fn heartbeats_keep_coming_while_events_flow_and_stop_with_the_stream() {
        let (sender, receiver) =
            tokio::sync::mpsc::unbounded_channel::<Result<Event, Infallible>>();
        let live = tokio_stream_from(receiver);
        let mut stream = Heartbeating::new(live, "build", Duration::from_millis(20));
        let hello = stream.next().await.unwrap().unwrap();
        let _ = hello;
        let mut beats = 0;
        let mut envelopes = 0;
        let deadline = tokio::time::Instant::now() + Duration::from_millis(200);
        while tokio::time::Instant::now() < deadline {
            sender.send(Ok(Event::default().data("{}"))).unwrap();
            match tokio::time::timeout(Duration::from_millis(50), stream.next()).await {
                Ok(Some(Ok(event))) => {
                    if format!("{event:?}").contains("heartbeat") {
                        beats += 1;
                    } else {
                        envelopes += 1;
                    }
                }
                _ => break,
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        assert!(envelopes > 0);
        assert!(beats > 0, "a busy stream still sends heartbeats");
        drop(sender);
        let rest = tokio::time::timeout(Duration::from_secs(1), async {
            while let Some(_event) = stream.next().await {}
        })
        .await;
        assert!(rest.is_ok(), "the stream ends when the live events end");
    }

    fn tokio_stream_from(
        mut receiver: tokio::sync::mpsc::UnboundedReceiver<Result<Event, Infallible>>,
    ) -> impl Stream<Item = Result<Event, Infallible>> + Unpin {
        Box::pin(futures_util::stream::poll_fn(move |cx| {
            receiver.poll_recv(cx)
        }))
    }

    #[tokio::test]
    async fn stopped_reader_closes_sse_so_the_browser_can_retry() {
        let connection = CallosumSocketConnection::with_retry_source(
            "unused.sock",
            Map::new(),
            1,
            Box::new(ClosedRetries),
        );
        let response = Sse::new(LiveEvents::new(connection, None, None)).into_response();
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
        let response = Sse::new(LiveEvents::new(
            connection,
            Some(Arc::clone(&capture)),
            None,
        ))
        .into_response();
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
