// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::convert::Infallible;
use std::pin::Pin;
use std::task::{Context, Poll};

use axum::response::IntoResponse;
use axum::response::sse::{Event, Sse};
use futures_core::Stream;

struct IdleEvents {
    sent_heartbeat: bool,
}

impl Stream for IdleEvents {
    type Item = Result<Event, Infallible>;

    fn poll_next(mut self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        if !self.sent_heartbeat {
            self.sent_heartbeat = true;
            return Poll::Ready(Some(Ok(Event::default().comment("heartbeat"))));
        }
        Poll::Pending
    }
}

pub async fn events() -> axum::response::Response {
    let mut response = Sse::new(IdleEvents {
        sent_heartbeat: false,
    })
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
