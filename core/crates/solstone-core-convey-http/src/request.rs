// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use axum::body::Body;
use axum::body::HttpBody;
use axum::http::{Request, StatusCode};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use std::pin::Pin;
use std::task::{Context, Poll};

struct AdmittedBody<G> {
    body: Body,
    admission: Option<Box<G>>,
}

impl<G: Send + 'static> HttpBody for AdmittedBody<G> {
    type Data = axum::body::Bytes;
    type Error = axum::Error;

    fn poll_frame(
        mut self: Pin<&mut Self>,
        context: &mut Context<'_>,
    ) -> Poll<Option<Result<hyper::body::Frame<Self::Data>, Self::Error>>> {
        let result = Pin::new(&mut self.body).poll_frame(context);
        if matches!(&result, Poll::Ready(None)) {
            self.admission.take();
        }
        result
    }

    fn is_end_stream(&self) -> bool {
        self.body.is_end_stream()
    }

    fn size_hint(&self) -> hyper::body::SizeHint {
        self.body.size_hint()
    }
}

/// Retain admission through the handler, its awaited blocking work, and its
/// response body. Losing the client stops waiting for the response, not the
/// admitted handler. The original guard moves into the completed response.
pub async fn run_admitted(
    admission: impl Send + 'static,
    request: Request<Body>,
    next: Next,
) -> Response {
    let work = tokio::spawn(async move {
        let response = next.run(request).await;
        let (parts, body) = response.into_parts();
        Response::from_parts(
            parts,
            Body::new(AdmittedBody {
                body,
                admission: Some(Box::new(admission)),
            }),
        )
    });
    match work.await {
        Ok(response) => response,
        Err(_) => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    }
}
