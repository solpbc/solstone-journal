// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Refuses state-changing requests a web page could send without script.
//!
//! An HTML form can send only three body types: `text/plain`,
//! `application/x-www-form-urlencoded` and `multipart/form-data`. A form
//! placed in a page the journal shows submits from the journal's own origin,
//! so the loopback guard admits it, and a JSON route that parses its body
//! without checking the type would act on a `text/plain` body shaped as JSON.
//! Every convey JSON caller labels its body `application/json`, and a form
//! cannot, so this layer turns the three form types away from every
//! state-changing method on every route and every carrier. The two upload
//! routes take only `multipart/form-data`, and their own extractor already
//! refuses anything else, so this layer leaves them exactly as they are. A
//! request with no body type, such as a body-less POST, is not a form
//! submission and passes.

use axum::Router;
use axum::body::Body;
use axum::http::{HeaderMap, Method, Request, StatusCode, header};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use solstone_core_convey_http::envelope::error_envelope;

/// Routes whose handlers take only a `multipart/form-data` upload.
const UPLOAD_ROUTES: [&str; 2] = ["/app/devices/ingest", "/app/import/api/save"];

const MESSAGE: &str =
    "your journal turned this request away because it wasn't sent by your journal's own pages.";

/// Guard the shared convey router, so it applies on every carrier.
pub(crate) fn apply_layer(router: Router) -> Router {
    router.layer(middleware::from_fn(refuse_form_bodies))
}

async fn refuse_form_bodies(request: Request<Body>, next: Next) -> Response {
    if refused(request.method(), request.uri().path(), request.headers()) {
        return error_envelope(
            "unsupported_media_type",
            MESSAGE,
            "state-changing requests take application/json",
            StatusCode::UNSUPPORTED_MEDIA_TYPE,
        )
        .into_response();
    }
    next.run(request).await
}

fn refused(method: &Method, path: &str, headers: &HeaderMap) -> bool {
    if matches!(
        *method,
        Method::GET | Method::HEAD | Method::OPTIONS | Method::TRACE
    ) || UPLOAD_ROUTES.contains(&path)
    {
        return false;
    }
    headers
        .get_all(header::CONTENT_TYPE)
        .iter()
        .any(|value| match value.to_str().map(essence) {
            Ok(essence) => matches!(
                essence.as_str(),
                "text/plain" | "application/x-www-form-urlencoded" | "multipart/form-data"
            ),
            // A form always sends a readable type; an unreadable one is refused all the same.
            Err(_) => true,
        })
}

/// The media type without parameters, lowercased: `Text/Plain; charset=UTF-8` → `text/plain`.
fn essence(value: &str) -> String {
    value
        .split(';')
        .next()
        .unwrap_or_default()
        .trim()
        .to_ascii_lowercase()
}

#[cfg(test)]
mod tests {
    use axum::body::to_bytes;
    use axum::http::HeaderValue;
    use axum::routing::post;
    use serde_json::Value;
    use tower::ServiceExt;

    use super::*;

    fn with_type(value: &str) -> HeaderMap {
        let mut headers = HeaderMap::new();
        headers.insert(header::CONTENT_TYPE, value.parse().expect("header value"));
        headers
    }

    const FORM_TYPES: [&str; 5] = [
        "text/plain",
        "text/plain;charset=UTF-8",
        "Application/X-WWW-Form-Urlencoded",
        " multipart/form-data; boundary=----x",
        "TEXT/PLAIN ; charset=utf-8",
    ];

    // Falsified by dropping the guard: a form planted in rendered text posts a
    // text/plain body shaped as JSON, and a JSON route acts on it.
    #[test]
    fn every_form_body_type_is_refused_on_every_state_changing_method() {
        for method in [Method::POST, Method::PUT, Method::PATCH, Method::DELETE] {
            for value in FORM_TYPES {
                assert!(
                    refused(
                        &method,
                        "/app/thinking/api/local/endpoint",
                        &with_type(value)
                    ),
                    "{method} {value}"
                );
            }
        }
    }

    #[test]
    fn json_untyped_and_safe_requests_pass() {
        let path = "/app/thinking/api/local/endpoint";
        for value in [
            "application/json",
            "application/json; charset=utf-8",
            "application/octet-stream",
        ] {
            assert!(!refused(&Method::POST, path, &with_type(value)), "{value}");
        }
        assert!(!refused(&Method::POST, path, &HeaderMap::new()));
        for method in [Method::GET, Method::HEAD, Method::OPTIONS] {
            assert!(
                !refused(&method, path, &with_type("text/plain")),
                "{method}"
            );
        }
    }

    #[test]
    fn only_the_upload_routes_keep_multipart() {
        let multipart = with_type("multipart/form-data; boundary=x");
        for path in UPLOAD_ROUTES {
            assert!(!refused(&Method::POST, path, &multipart), "{path}");
        }
        for path in [
            "/app/import/api/start",
            "/app/devices/ingest/x",
            "/app/import/api/save/",
        ] {
            assert!(refused(&Method::POST, path, &multipart), "{path}");
        }
    }

    #[test]
    fn any_form_type_among_several_or_an_unreadable_type_is_refused() {
        let mut headers = with_type("application/json");
        headers.append(header::CONTENT_TYPE, HeaderValue::from_static("text/plain"));
        assert!(refused(&Method::POST, "/x", &headers));
        let mut unreadable = HeaderMap::new();
        unreadable.insert(
            header::CONTENT_TYPE,
            HeaderValue::from_bytes(b"text/plain\xff").expect("opaque header value"),
        );
        assert!(refused(&Method::POST, "/x", &unreadable));
    }

    #[tokio::test]
    async fn a_refusal_never_reaches_the_route() {
        let reached = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let flag = reached.clone();
        let app = apply_layer(Router::new().route(
            "/x",
            post(move || {
                flag.store(true, std::sync::atomic::Ordering::SeqCst);
                async { StatusCode::OK }
            }),
        ));
        let response = app
            .clone()
            .oneshot(
                Request::post("/x")
                    .header(header::CONTENT_TYPE, "text/plain")
                    .body(Body::from(r#"{"a":"b"}"#))
                    .expect("request"),
            )
            .await
            .expect("response");
        assert_eq!(response.status(), StatusCode::UNSUPPORTED_MEDIA_TYPE);
        let body: Value = serde_json::from_slice(
            &to_bytes(response.into_body(), usize::MAX)
                .await
                .expect("body"),
        )
        .expect("envelope");
        assert_eq!(body["reason_code"], "unsupported_media_type");
        assert!(!reached.load(std::sync::atomic::Ordering::SeqCst));

        let response = app
            .oneshot(
                Request::post("/x")
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(r#"{"a":"b"}"#))
                    .expect("request"),
            )
            .await
            .expect("response");
        assert_eq!(response.status(), StatusCode::OK);
        assert!(reached.load(std::sync::atomic::Ordering::SeqCst));
    }
}
