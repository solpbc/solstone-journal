// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::time::{Duration, Instant};

use axum::{
    body::Body,
    http::{Request, header},
    middleware::Next,
    response::Response,
};

pub(crate) fn is_dashboard_response(response: &Response) -> bool {
    response
        .headers()
        .get(header::CONTENT_TYPE)
        .is_some_and(|value| {
            let kind = value.to_str().unwrap_or("").split(';').next().unwrap_or("");
            matches!(kind, "text/html" | "application/json")
        })
}

pub(crate) fn append(response: &mut Response, name: &'static str, duration: Duration) {
    let value = format!("{name};dur={:.3}", duration.as_secs_f64() * 1000.0);
    response.headers_mut().append(
        header::HeaderName::from_static("server-timing"),
        value
            .parse()
            .expect("fixed timing vocabulary and numeric duration"),
    );
}

// This router layer also covers the embedded owner path, which bypasses the
// transport layers. It measures admission through response headers, not bodies.
pub(crate) async fn measure(request: Request<Body>, next: Next) -> Response {
    let started = Instant::now();
    let mut response = next.run(request).await;
    if is_dashboard_response(&response) {
        append(&mut response, "app", started.elapsed());
    }
    response
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{Router, routing::get};
    use tower::ServiceExt;

    #[tokio::test]
    async fn app_timing_appends_to_every_existing_header_and_skips_streams_and_media() {
        for content_type in [
            "text/html; charset=utf-8",
            "application/json",
            "text/event-stream",
            "audio/mpeg",
            "text/javascript",
        ] {
            let app = Router::new()
                .route(
                    "/read",
                    get(move || async move {
                        Response::builder()
                            .header(header::CONTENT_TYPE, content_type)
                            .header("server-timing", "coalesced;desc=\"immediate\"")
                            .header("server-timing", "month-fold;dur=1.25")
                            .body(Body::empty())
                            .unwrap()
                    }),
                )
                .layer(axum::middleware::from_fn(measure));
            let response = app
                .oneshot(Request::get("/read").body(Body::empty()).unwrap())
                .await
                .unwrap();
            let values: Vec<_> = response
                .headers()
                .get_all("server-timing")
                .iter()
                .map(|v| v.to_str().unwrap())
                .collect();
            assert_eq!(
                &values[..2],
                ["coalesced;desc=\"immediate\"", "month-fold;dur=1.25"]
            );
            if is_dashboard_response(&response) {
                assert_eq!(values.len(), 3);
                assert!(
                    values[2]
                        .strip_prefix("app;dur=")
                        .unwrap()
                        .parse::<f64>()
                        .unwrap()
                        >= 0.0
                );
            } else {
                assert_eq!(values.len(), 2);
            }
        }
    }

    #[tokio::test]
    async fn dashboard_routes_report_app_and_gate_timings() {
        let journal = tempfile::tempdir().unwrap();
        std::fs::create_dir(journal.path().join("config")).unwrap();
        std::fs::write(
            journal.path().join("config/journal.json"),
            br#"{"setup":{"completed_at":1}}"#,
        )
        .unwrap();
        let app = crate::router(journal.path().to_owned());
        for path in [
            "/app/home/",
            "/api/shell",
            "/app/home/workspace",
            "/app/transcripts/api/stats/202611",
        ] {
            let response = app
                .clone()
                .oneshot(Request::get(path).body(Body::empty()).unwrap())
                .await
                .unwrap();
            assert_eq!(response.status(), 200, "{path}");
            let timings = response
                .headers()
                .get_all("server-timing")
                .iter()
                .map(|v| v.to_str().unwrap())
                .collect::<Vec<_>>();
            for metric in ["app;dur=", "gate;dur="] {
                assert!(
                    timings.iter().any(|value| value.starts_with(metric)),
                    "{path}: {timings:?}"
                );
            }
            if path.contains("/stats/") {
                for metric in ["coalesced;desc=", "month-fold;dur="] {
                    assert!(
                        timings.iter().any(|value| value.starts_with(metric)),
                        "{path}: {timings:?}"
                    );
                }
            }
        }
    }
}
