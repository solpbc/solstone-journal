// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Request-provenance guard for the loopback listener.
//!
//! Every request that reaches the loopback listener is treated as the owner
//! (`AccessBasis::Localhost`), and the bind address is what keeps other
//! machines out. It does not keep out the owner's own browser: a page on another
//! site can send a state-changing request to `127.0.0.1`, and a name the page
//! controls can be re-pointed at loopback so the page reads the responses. This
//! layer refuses both, in two halves:
//!
//! 1. **Host allowlist, every method.** The request's `Host` (and the target
//!    authority of an absolute-form request) must be exactly `localhost`,
//!    `127.0.0.1` or `[::1]`, with an optional numeric port.
//! 2. **Cross-site guard, every method except GET, HEAD, OPTIONS and TRACE.**
//!    A `Sec-Fetch-Site` other than `same-origin`, `same-site` or `none` is
//!    refused, and so is an `Origin` whose host is not one of those three names.
//!    A request with neither header, such as one from the `solstone` CLI, passes.
//!
//! The guard acts only on `Localhost` connections. The paired-device door
//! serves the same routes under `Host: spl.local` and authenticates with a
//! device certificate, so it is neither wrapped in this layer nor subject to it
//! if the layer were ever moved onto the shared router.

use axum::Router;
use axum::body::Body;
use axum::http::{HeaderMap, Method, Request, StatusCode, Uri};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use solstone_core_convey_http::envelope::error_envelope;
use solstone_core_convey_http::identity::AccessBasis;

const HOST_MESSAGE: &str = "your journal's web app can only be opened at localhost, 127.0.0.1 or [::1] on the computer it runs on.";
const SITE_MESSAGE: &str =
    "your journal's web app turned this request away because it came from another website.";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Refusal {
    HostNotAllowed,
    CrossSite,
    CrossOrigin,
}

impl Refusal {
    fn into_response(self) -> Response {
        let (reason_code, message, detail) = match self {
            Self::HostNotAllowed => ("host_not_allowed", HOST_MESSAGE, "host not allowed"),
            Self::CrossSite => ("cross_origin_blocked", SITE_MESSAGE, "cross-site request"),
            Self::CrossOrigin => ("cross_origin_blocked", SITE_MESSAGE, "cross-origin request"),
        };
        error_envelope(reason_code, message, detail, StatusCode::FORBIDDEN).into_response()
    }
}

/// Guard the loopback listener's router. Applied last, so it runs before
/// every other layer and before any route.
pub(crate) fn apply_layer(router: Router) -> Router {
    router.layer(middleware::from_fn(guard_loopback_origin))
}

async fn guard_loopback_origin(request: Request<Body>, next: Next) -> Response {
    // Only device-door bases are exempt. A missing basis is not one of them.
    let device_door = matches!(
        request.extensions().get::<AccessBasis>(),
        Some(AccessBasis::LinkedDevice { .. } | AccessBasis::PairingPeer { .. })
    );
    if !device_door
        && let Some(refusal) = refuse(request.method(), request.uri(), request.headers())
    {
        return refusal.into_response();
    }
    next.run(request).await
}

fn refuse(method: &Method, uri: &Uri, headers: &HeaderMap) -> Option<Refusal> {
    match solstone_core_convey_http::loopback_guard::evaluate_loopback_request(method, uri, headers)
    {
        Some(solstone_core_convey_http::loopback_guard::LoopbackRefusal::HostNotAllowed) => {
            Some(Refusal::HostNotAllowed)
        }
        Some(solstone_core_convey_http::loopback_guard::LoopbackRefusal::CrossSite) => {
            Some(Refusal::CrossSite)
        }
        Some(solstone_core_convey_http::loopback_guard::LoopbackRefusal::CrossOrigin) => {
            Some(Refusal::CrossOrigin)
        }
        None => None,
    }
}

#[cfg(test)]
mod tests {

    use axum::Extension;
    use axum::body::to_bytes;
    use axum::http::HeaderName;
    use axum::routing::get;
    use serde_json::Value;
    use solstone_core_convey_http::identity::{Carrier, LinkedDeviceCid};
    use tower::ServiceExt;

    use super::*;

    const VALID_CID: &str =
        "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";

    fn headers(pairs: &[(&str, &str)]) -> HeaderMap {
        let mut map = HeaderMap::new();
        for (name, value) in pairs {
            map.append(
                HeaderName::from_bytes(name.as_bytes()).expect("header name"),
                value.parse().expect("header value"),
            );
        }
        map
    }

    fn verdict(method: Method, pairs: &[(&str, &str)]) -> Option<Refusal> {
        refuse(&method, &Uri::from_static("/x"), &headers(pairs))
    }

    #[test]
    fn host_allowlist_accepts_exactly_the_three_loopback_names() {
        for host in [
            "localhost",
            "LocalHost",
            "localhost:5015",
            "127.0.0.1",
            "127.0.0.1:5015",
            "127.0.0.1:1",
            "[::1]",
            "[::1]:5015",
        ] {
            assert_eq!(verdict(Method::GET, &[("host", host)]), None, "{host}");
            assert_eq!(verdict(Method::POST, &[("host", host)]), None, "{host}");
        }
    }

    #[test]
    fn host_allowlist_refuses_every_other_name() {
        for host in [
            "evil.example",
            "evil.example:5015",
            "localhost.evil.example",
            "127.0.0.1.evil.example",
            "evil-localhost",
            "localhost.",
            "127.0.0.2",
            "127.1",
            "0.0.0.0",
            "[::]",
            "[::2]",
            "[0:0:0:0:0:0:0:1]",
            "[localhost]",
            "[127.0.0.1]",
            "::1",
            "::1:5015",
            "localhost:",
            "localhost:99999",
            "localhost:80a",
            "localhost:+80",
            "127.0.0.1:5015:1",
            "evil.example@127.0.0.1",
            "127.0.0.1@evil.example",
            "localhost:5015/evil",
            "spl.local",
            "",
        ] {
            assert_eq!(
                verdict(Method::GET, &[("host", host)]),
                Some(Refusal::HostNotAllowed),
                "{host:?}"
            );
        }
    }

    #[test]
    fn a_request_that_names_no_host_is_refused() {
        assert_eq!(verdict(Method::GET, &[]), Some(Refusal::HostNotAllowed));
    }

    #[test]
    fn every_host_value_and_the_request_target_must_be_loopback() {
        assert_eq!(
            verdict(
                Method::GET,
                &[("host", "localhost"), ("host", "evil.example")]
            ),
            Some(Refusal::HostNotAllowed)
        );
        let absolute = Uri::from_static("http://evil.example/x");
        assert_eq!(
            refuse(&Method::GET, &absolute, &headers(&[("host", "localhost")])),
            Some(Refusal::HostNotAllowed)
        );
        let absolute = Uri::from_static("http://127.0.0.1:5015/x");
        assert_eq!(
            refuse(
                &Method::GET,
                &absolute,
                &headers(&[("host", "evil.example")])
            ),
            Some(Refusal::HostNotAllowed)
        );
        assert_eq!(
            refuse(&Method::GET, &absolute, &headers(&[("host", "localhost")])),
            None
        );
        assert_eq!(refuse(&Method::GET, &absolute, &headers(&[])), None);
    }

    #[test]
    fn cross_site_provenance_is_refused_on_every_state_changing_method() {
        for method in [
            Method::POST,
            Method::PUT,
            Method::PATCH,
            Method::DELETE,
            Method::from_bytes(b"PROPFIND").expect("method"),
        ] {
            assert_eq!(
                verdict(
                    method.clone(),
                    &[("host", "localhost"), ("sec-fetch-site", "cross-site")]
                ),
                Some(Refusal::CrossSite),
                "{method}"
            );
            assert_eq!(
                verdict(
                    method.clone(),
                    &[("host", "localhost"), ("origin", "https://evil.example")]
                ),
                Some(Refusal::CrossOrigin),
                "{method}"
            );
        }
    }

    #[test]
    fn unknown_or_malformed_provenance_is_refused() {
        for value in [
            "",
            "Cross-Site",
            "SAME-ORIGIN",
            "same-origin, cross-site",
            "other",
        ] {
            assert_eq!(
                verdict(
                    Method::POST,
                    &[("host", "localhost"), ("sec-fetch-site", value)]
                ),
                Some(Refusal::CrossSite),
                "{value:?}"
            );
        }
        for origin in [
            "null",
            "",
            "https://evil.example",
            "http://localhost.evil.example",
            "http://127.0.0.1.evil.example:5015",
            "http://evil.example@localhost",
            "http://localhost:5015/path",
            "chrome-extension://fgfnkcefedeheoeamppkiiloncfekakf",
            "ftp://localhost",
            "localhost:5015",
        ] {
            assert_eq!(
                verdict(Method::POST, &[("host", "localhost"), ("origin", origin)]),
                Some(Refusal::CrossOrigin),
                "{origin:?}"
            );
        }
        assert_eq!(
            verdict(
                Method::POST,
                &[
                    ("host", "localhost"),
                    ("origin", "http://localhost"),
                    ("origin", "null")
                ]
            ),
            Some(Refusal::CrossOrigin)
        );
    }

    #[test]
    fn same_origin_browsers_and_headerless_clients_pass() {
        for (host, origin) in [
            ("localhost:5015", "http://localhost:5015"),
            ("127.0.0.1:5015", "http://127.0.0.1:5015"),
            ("[::1]:5015", "http://[::1]:5015"),
            ("localhost:5015", "https://localhost:5015"),
        ] {
            for site in ["same-origin", "same-site", "none"] {
                assert_eq!(
                    verdict(
                        Method::POST,
                        &[("host", host), ("origin", origin), ("sec-fetch-site", site)]
                    ),
                    None,
                    "{host} {origin} {site}"
                );
            }
        }
        // The `solstone` and `journal` CLIs and the desktop clients send neither.
        assert_eq!(verdict(Method::POST, &[("host", "localhost:5015")]), None);
        assert_eq!(verdict(Method::DELETE, &[("host", "127.0.0.1:5015")]), None);
    }

    #[test]
    fn read_only_methods_are_not_subject_to_the_cross_site_half() {
        for method in [Method::GET, Method::HEAD, Method::OPTIONS, Method::TRACE] {
            assert_eq!(
                verdict(
                    method,
                    &[
                        ("host", "localhost"),
                        ("sec-fetch-site", "cross-site"),
                        ("origin", "https://evil.example")
                    ]
                ),
                None
            );
        }
    }

    async fn status_and_reason(
        router: Router,
        basis: Option<AccessBasis>,
        host: &str,
    ) -> (StatusCode, Option<String>) {
        let router = match basis {
            Some(basis) => router.layer(Extension(basis)),
            None => router,
        };
        let response = router
            .oneshot(
                Request::builder()
                    .uri("/probe")
                    .header("host", host)
                    .body(Body::empty())
                    .expect("request"),
            )
            .await
            .expect("response");
        let status = response.status();
        let body = to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("body");
        let reason = serde_json::from_slice::<Value>(&body)
            .ok()
            .and_then(|json| json["reason_code"].as_str().map(str::to_owned));
        (status, reason)
    }

    #[tokio::test]
    async fn only_localhost_connections_are_guarded_and_a_missing_basis_is_not_exempt() {
        let guarded = || apply_layer(Router::new().route("/probe", get(|| async { "ok" })));
        let cid = LinkedDeviceCid::try_from(VALID_CID).expect("cid");

        let (status, reason) =
            status_and_reason(guarded(), Some(AccessBasis::Localhost), "evil.example").await;
        assert_eq!(
            (status, reason.as_deref()),
            (StatusCode::FORBIDDEN, Some("host_not_allowed"))
        );

        let (status, reason) = status_and_reason(guarded(), None, "evil.example").await;
        assert_eq!(
            (status, reason.as_deref()),
            (StatusCode::FORBIDDEN, Some("host_not_allowed"))
        );

        for carrier in [Carrier::Direct, Carrier::ViaSpl] {
            let basis = AccessBasis::LinkedDevice {
                carrier,
                cid: cid.clone(),
            };
            let (status, _) = status_and_reason(guarded(), Some(basis), "spl.local").await;
            assert_eq!(status, StatusCode::OK);
        }
        let (status, _) = status_and_reason(
            guarded(),
            Some(AccessBasis::PairingPeer {
                carrier: Carrier::Direct,
            }),
            "spl.local",
        )
        .await;
        assert_eq!(status, StatusCode::OK);

        let (status, _) =
            status_and_reason(guarded(), Some(AccessBasis::Localhost), "localhost:5015").await;
        assert_eq!(status, StatusCode::OK);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn loopback_guard_refuses_cross_origin_agents_capability_mutation() {
        let temp = tempfile::TempDir::new_in("/var/tmp").unwrap();
        let journal = temp.path().to_path_buf();
        let app = apply_layer(solstone_core_mcp_endpoint::owner_routes(journal))
            .layer(Extension(AccessBasis::Localhost));

        let request = Request::builder()
            .method(Method::PUT)
            .uri("/app/agents/api/capability")
            .header("host", "localhost")
            .header("origin", "https://evil.example")
            .header("content-type", "application/json")
            .body(Body::from(r#"{"enabled":true}"#))
            .unwrap();
        let response = app.oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
        let body = to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("body");
        let reason = serde_json::from_slice::<Value>(&body)
            .ok()
            .and_then(|json| json["reason_code"].as_str().map(str::to_owned));
        assert_eq!(reason.as_deref(), Some("cross_origin_blocked"));
    }
}
