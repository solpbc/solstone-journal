// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! One rule decides every dashboard response's cache policy: what its bytes
//! depend on.
//!
//! - Build-only bytes at a hashed path are kept for good. A file's hashed path
//!   is `<tree>/<hash>/<path>`, and the hash covers the exact bytes served for
//!   every file in the tree, so new bytes always arrive under a new name.
//! - Build-only bytes at a stable path (the shell page, view fragments, the
//!   unhashed static paths older pages and apps still use) are revalidated on
//!   every use against a strong validator of the exact bytes, so an unchanged
//!   build answers `304` and a new one answers its new bytes.
//! - Everything else depends on the journal, the owner, the clock or the
//!   request, and is never stored. That is also the default for anything the
//!   rule doesn't class.
//!
//! A hashed path answers through the same router as its stable path, so it
//! passes the same gates and handler. Only this build's hashes are served: any
//! other hash is a `404` that is never stored, so a page from an older build
//! can't receive new code under an old name.

use std::convert::Infallible;

use axum::Router;
use axum::body::{Body, Bytes};
use axum::extract::Request;
use axum::http::header::{
    CACHE_CONTROL, CONTENT_LENGTH, CONTENT_TYPE, ETAG, HeaderValue, IF_NONE_MATCH,
};
use axum::http::{HeaderMap, Method, StatusCode, Uri};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use sha2::{Digest, Sha256};
use tower::ServiceExt;

use crate::assets::{self, AssetKind, AssetRecord};

mod refs {
    include!("../../../build-support/convey_asset_refs.rs");
}

pub(crate) use refs::{has_tree_hash_shape, reference_spans, resolve_reference};

/// Every class of dashboard response, by what its bytes depend on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum CacheClass {
    /// Build-only bytes under a hashed path of this build.
    Hashed,
    /// Build-only bytes at a stable path, with a validator of those bytes.
    Revalidated,
    /// Bytes that depend on the journal, the owner, the clock or the request,
    /// and anything not classed above.
    Personal,
}

/// The one policy table.
pub(crate) fn cache_control(class: CacheClass) -> &'static str {
    match class {
        CacheClass::Hashed => "public, max-age=31536000, immutable",
        CacheClass::Revalidated => "no-cache",
        CacheClass::Personal => "no-store",
    }
}

/// Routes for this build's hashed paths. Each one answers with the response of
/// the stable path it names, from `stable`, which must already hold every
/// dashboard route and their request extensions.
pub(crate) fn hashed_routes(stable: Router) -> Router {
    let mut routes = Router::new();
    for path in assets::hashed_paths() {
        let stable = stable.clone();
        routes = routes.route(
            path,
            get(move |request: Request| answer_hashed(stable.clone(), request)),
        );
    }
    routes
}

async fn answer_hashed(stable: Router, mut request: Request) -> Response {
    let Some(record) = assets::hashed_record(request.uri().path()) else {
        return unknown_hash_response();
    };
    let target = match request.uri().query() {
        Some(query) => format!("{}?{query}", record.path),
        None => record.path.to_owned(),
    };
    let Ok(uri) = target.parse::<Uri>() else {
        return unknown_hash_response();
    };
    *request.uri_mut() = uri;
    let result: Result<Response, Infallible> = stable.oneshot(request).await;
    match result {
        Ok(response) => response,
        Err(never) => match never {},
    }
}

fn unknown_hash_response() -> Response {
    let mut response = crate::not_found_response();
    response.headers_mut().insert(
        CACHE_CONTROL,
        HeaderValue::from_static(cache_control(CacheClass::Personal)),
    );
    response
}

/// Refuse a hashed path whose hash isn't this build's. It runs inside the
/// gates, so a caller the gates turn away learns nothing about the build.
pub(crate) async fn refuse_unknown_hash(request: Request, next: Next) -> Response {
    let path = request.uri().path();
    if matches!(*request.method(), Method::GET | Method::HEAD)
        && assets::hashed_record(path).is_none()
        && has_tree_hash_shape(path)
    {
        return unknown_hash_response();
    }
    next.run(request).await
}

/// The layer that applies the rule. It sits outside every dashboard route, so
/// its header is the only `Cache-Control` a dashboard response carries.
pub(crate) async fn apply(request: Request, next: Next) -> Response {
    let readable = matches!(*request.method(), Method::GET | Method::HEAD);
    let head = *request.method() == Method::HEAD;
    let path = request.uri().path().to_owned();
    let validators = request.headers().get(IF_NONE_MATCH).cloned();
    let response = next.run(request).await;
    if !readable || response.status() != StatusCode::OK {
        return classed(response, CacheClass::Personal);
    }
    if let Some(record) = assets::hashed_record(&path) {
        let response = served(record, head, response).await;
        return classed(response, CacheClass::Hashed);
    }
    let Some(record) = assets::record(&path) else {
        return match shell_page(response).await {
            Ok((response, tag)) => revalidated(response, tag, validators.as_ref()),
            Err(response) => classed(response, CacheClass::Personal),
        };
    };
    let (response, tag) = match record.kind {
        AssetKind::Dynamic => return classed(response, CacheClass::Personal),
        AssetKind::File => (served(record, head, response).await, record.etag.to_owned()),
        AssetKind::Template if head => return classed(response, CacheClass::Personal),
        AssetKind::Template => filled(record, response).await,
    };
    if response.status() != StatusCode::OK {
        return classed(response, CacheClass::Personal);
    }
    revalidated(response, &tag, validators.as_ref())
}

fn classed(mut response: Response, class: CacheClass) -> Response {
    response.headers_mut().insert(
        CACHE_CONTROL,
        HeaderValue::from_static(cache_control(class)),
    );
    response
}

/// The shell page, which every view crate embeds and serves from its own page
/// routes, is recognised by its exact bytes and served rewritten. Any other
/// response comes back untouched.
async fn shell_page(response: Response) -> Result<(Response, &'static str), Response> {
    let (Some(record), Some(source)) = (
        assets::record("/static/shell.html"),
        assets::lookup("/static/shell.html"),
    ) else {
        return Err(response);
    };
    if !is_html(response.headers()) || exact_length(&response) != Some(record.byte_len) {
        return Err(response);
    }
    let (parts, bytes) = body_bytes(response).await;
    let Some(bytes) = bytes else {
        return Err(failed(parts));
    };
    if bytes != source.bytes {
        return Err(with_body(parts, bytes));
    }
    Ok((
        with_body(parts, refs::apply_edits(&bytes, record.edits)),
        record.etag,
    ))
}

fn is_html(headers: &HeaderMap) -> bool {
    headers
        .get(CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| value.starts_with("text/html"))
}

fn exact_length(response: &Response) -> Option<usize> {
    axum::body::HttpBody::size_hint(response.body())
        .exact()
        .and_then(|length| usize::try_from(length).ok())
}

async fn body_bytes(response: Response) -> (axum::http::response::Parts, Option<Bytes>) {
    let (parts, body) = response.into_parts();
    let bytes = axum::body::to_bytes(body, usize::MAX).await.ok();
    (parts, bytes)
}

fn with_body(mut parts: axum::http::response::Parts, bytes: impl Into<Bytes>) -> Response {
    let bytes = bytes.into();
    parts
        .headers
        .insert(CONTENT_LENGTH, HeaderValue::from(bytes.len()));
    Response::from_parts(parts, Body::from(bytes))
}

fn failed(mut parts: axum::http::response::Parts) -> Response {
    parts.status = StatusCode::INTERNAL_SERVER_ERROR;
    parts.headers.remove(CONTENT_LENGTH);
    Response::from_parts(parts, Body::empty())
}

/// A file's served bytes: its source with references rewritten. The edits are
/// byte ranges of the source, so a body of any other length is refused rather
/// than rewritten in the wrong places.
async fn served(record: &AssetRecord, head: bool, response: Response) -> Response {
    if head || record.edits.is_empty() {
        return response;
    }
    if exact_length(&response) != Some(record.byte_len) {
        return failed(response.into_parts().0);
    }
    let (parts, bytes) = body_bytes(response).await;
    match bytes {
        Some(bytes) => with_body(parts, refs::apply_edits(&bytes, record.edits)),
        None => failed(parts),
    }
}

/// A template's bytes, rewritten after the crate filled it in, and their tag.
async fn filled(record: &AssetRecord, response: Response) -> (Response, String) {
    let (parts, bytes) = body_bytes(response).await;
    let Some(text) = bytes
        .as_ref()
        .and_then(|bytes| std::str::from_utf8(bytes).ok())
    else {
        return (failed(parts), String::new());
    };
    let rewritten = rewrite_references(record.path, text);
    let tag = strong_tag(rewritten.as_bytes());
    (with_body(parts, rewritten), tag)
}

/// Rewrite every reference to a tree file of this build to its hashed path.
pub(crate) fn rewrite_references(base: &str, text: &str) -> String {
    let mut edits = Vec::new();
    for (start, end) in reference_spans(text) {
        let reference = &text[start..end];
        let Some(target) = resolve_reference(base, reference) else {
            continue;
        };
        if let Some(hashed) = assets::record(&target).and_then(|record| record.hashed) {
            let path_end = start + reference.find(['?', '#']).unwrap_or(reference.len());
            edits.push((start, path_end, hashed));
        }
    }
    let rewritten = refs::apply_edits(text.as_bytes(), &edits);
    String::from_utf8(rewritten).expect("rewriting ASCII paths keeps UTF-8")
}

fn strong_tag(bytes: &[u8]) -> String {
    format!("\"{}\"", &format!("{:x}", Sha256::digest(bytes))[..32])
}

/// Attach the validator, and answer `304` when the client already holds it.
fn revalidated(
    mut response: Response,
    tag: impl AsRef<str>,
    validators: Option<&HeaderValue>,
) -> Response {
    let tag = tag.as_ref();
    let Ok(value) = HeaderValue::from_str(tag) else {
        return classed(response, CacheClass::Personal);
    };
    response.headers_mut().insert(ETAG, value);
    let response = classed(response, CacheClass::Revalidated);
    if !validators.is_some_and(|validators| matches_validator(validators, tag)) {
        return response;
    }
    let (mut parts, _) = response.into_parts();
    parts.status = StatusCode::NOT_MODIFIED;
    parts.headers.remove(CONTENT_LENGTH);
    parts.headers.remove(CONTENT_TYPE);
    Response::from_parts(parts, Body::empty()).into_response()
}

/// `If-None-Match` uses weak comparison, and the server marks a gzipped
/// answer's tag with its encoding, so both forms of our tag match.
pub(crate) fn matches_validator(validators: &HeaderValue, tag: &str) -> bool {
    let Ok(validators) = validators.to_str() else {
        return false;
    };
    let ours = tag.trim_matches('"');
    validators.split(',').map(str::trim).any(|candidate| {
        if candidate == "*" {
            return true;
        }
        let candidate = candidate.strip_prefix("W/").unwrap_or(candidate);
        let Some(opaque) = candidate
            .strip_prefix('"')
            .and_then(|c| c.strip_suffix('"'))
        else {
            return false;
        };
        opaque == ours || opaque.strip_suffix("-gzip") == Some(ours)
    })
}

/// Recompute a tree's hash from the bytes a router actually serves for it.
#[cfg(test)]
pub(crate) fn tree_hash_of_served<'a>(
    root: &str,
    files: impl IntoIterator<Item = (&'a str, &'a [u8])>,
) -> String {
    refs::tree_hash(root, files)
}

pub(crate) fn rewrite_source(bytes: &[u8], edits: &[(usize, usize, &str)]) -> Vec<u8> {
    refs::apply_edits(bytes, edits)
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use axum::body::Body;
    use axum::http::{HeaderMap, Request, StatusCode, header};
    use tower::ServiceExt;

    use super::*;

    fn established_journal() -> tempfile::TempDir {
        let journal = tempfile::tempdir().unwrap();
        std::fs::create_dir(journal.path().join("config")).unwrap();
        std::fs::write(
            journal.path().join("config/journal.json"),
            br#"{"setup":{"completed_at":1}}"#,
        )
        .unwrap();
        journal
    }

    async fn fetch(
        app: &Router,
        path: &str,
        validators: Option<&str>,
    ) -> (StatusCode, HeaderMap, Vec<u8>) {
        let mut request = Request::get(path)
            .extension(solstone_core_convey_http::identity::AccessBasis::Localhost);
        if let Some(validators) = validators {
            request = request.header(header::IF_NONE_MATCH, validators);
        }
        let response = app
            .clone()
            .oneshot(request.body(Body::empty()).unwrap())
            .await
            .unwrap();
        let status = response.status();
        let headers = response.headers().clone();
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap()
            .to_vec();
        (status, headers, body)
    }

    fn hashed(path: &str) -> &'static str {
        assets::record(path)
            .and_then(|record| record.hashed)
            .unwrap_or_else(|| panic!("{path} has a hashed path"))
    }

    fn policy(headers: &HeaderMap) -> &str {
        headers[header::CACHE_CONTROL].to_str().unwrap()
    }

    #[test]
    fn every_class_has_an_explicit_policy_and_the_default_stores_nothing() {
        for (class, expected) in [
            (CacheClass::Hashed, "public, max-age=31536000, immutable"),
            (CacheClass::Revalidated, "no-cache"),
            (CacheClass::Personal, "no-store"),
        ] {
            assert_eq!(cache_control(class), expected, "{class:?}");
        }
    }

    // Every family of dashboard response, by what its bytes depend on. A new
    // family belongs here with the policy it should get; anything the rule
    // doesn't class falls to no-store, which this also checks.
    #[tokio::test]
    async fn every_route_class_gets_its_policy_and_repeats_no_header() {
        let journal = established_journal();
        let app = crate::router(journal.path().to_owned());
        let cases: Vec<(&str, String, StatusCode, &str)> = vec![
            (
                "shell page",
                "/app/home/".into(),
                StatusCode::OK,
                "no-cache",
            ),
            (
                "shell page from a view crate",
                "/app/settings/".into(),
                StatusCode::OK,
                "no-cache",
            ),
            (
                "transcripts day page",
                "/app/transcripts/20260101".into(),
                StatusCode::OK,
                "no-cache",
            ),
            (
                "view fragment",
                "/app/home/workspace".into(),
                StatusCode::OK,
                "no-cache",
            ),
            (
                "fragment filled in per build",
                "/app/health/workspace".into(),
                StatusCode::OK,
                "no-cache",
            ),
            (
                "unhashed shell file",
                "/static/app.js".into(),
                StatusCode::OK,
                "no-cache",
            ),
            (
                "unhashed view file",
                "/app/home/static/home.js".into(),
                StatusCode::OK,
                "no-cache",
            ),
            (
                "unhashed runtime-built file",
                "/static/sol-status/mark.svg".into(),
                StatusCode::OK,
                "no-cache",
            ),
            ("favicon", "/favicon.ico".into(), StatusCode::OK, "no-cache"),
            (
                "import guide",
                "/app/import/api/guide/ics".into(),
                StatusCode::OK,
                "no-cache",
            ),
            (
                "hashed shell file",
                hashed("/static/app.js").into(),
                StatusCode::OK,
                "public, max-age=31536000, immutable",
            ),
            (
                "hashed vendor file",
                hashed("/static/vendor/marked/marked.min.js").into(),
                StatusCode::OK,
                "public, max-age=31536000, immutable",
            ),
            (
                "hashed view file",
                hashed("/app/home/static/home.js").into(),
                StatusCode::OK,
                "public, max-age=31536000, immutable",
            ),
            (
                "hashed view file in a directory-served crate",
                hashed("/app/health/static/health.js").into(),
                StatusCode::OK,
                "public, max-age=31536000, immutable",
            ),
            (
                "unknown shell hash",
                "/static/0123456789abcdef/app.js".into(),
                StatusCode::NOT_FOUND,
                "no-store",
            ),
            (
                "unknown vendor hash",
                "/static/vendor/marked/0123456789abcdef/marked.min.js".into(),
                StatusCode::NOT_FOUND,
                "no-store",
            ),
            (
                "unknown view hash",
                "/app/health/static/0123456789abcdef/health.js".into(),
                StatusCode::NOT_FOUND,
                "no-store",
            ),
            (
                "shell state read",
                "/api/shell".into(),
                StatusCode::OK,
                "no-store",
            ),
            (
                "journal read",
                "/app/health/api/state".into(),
                StatusCode::OK,
                "no-store",
            ),
            (
                "unknown path",
                "/static/no-such-file.js".into(),
                StatusCode::NOT_FOUND,
                "no-store",
            ),
            ("front door", "/".into(), StatusCode::OK, "no-cache"),
            (
                "redirect",
                "/app/home".into(),
                StatusCode::PERMANENT_REDIRECT,
                "no-store",
            ),
        ];
        for (class, path, status, expected) in cases {
            let (actual, headers, _) = fetch(&app, &path, None).await;
            assert_eq!(actual, status, "{class}: {path}");
            assert_eq!(policy(&headers), expected, "{class}: {path}");
            assert_eq!(
                headers.contains_key(header::ETAG),
                expected == "no-cache",
                "{class}: {path} validator"
            );
            for name in headers.keys() {
                assert_eq!(
                    headers.get_all(name).iter().count(),
                    1,
                    "{class}: {path} repeats {name}"
                );
            }
        }
    }

    #[tokio::test]
    async fn every_embedded_asset_is_classed_by_what_its_bytes_depend_on() {
        let journal = established_journal();
        // With the agents app, which serves the one workspace the default omits.
        let app =
            crate::router_with_hosted_parent(journal.path().to_owned(), None, Some(Router::new()));
        for record in assets::INVENTORY {
            let (status, headers, body) = fetch(&app, record.path, None).await;
            // The setup page redirects once a journal is set up; never stored either way.
            if record.kind == AssetKind::Dynamic {
                assert_eq!(policy(&headers), "no-store", "{}", record.path);
                continue;
            }
            assert_eq!(status, StatusCode::OK, "{}", record.path);
            match record.kind {
                AssetKind::Dynamic => unreachable!(),
                AssetKind::Template => assert_eq!(policy(&headers), "no-cache", "{}", record.path),
                AssetKind::File => {
                    assert_eq!(policy(&headers), "no-cache", "{}", record.path);
                    assert_eq!(headers[header::ETAG], record.etag, "{}", record.path);
                    assert_eq!(
                        strong_tag(&body),
                        record.etag,
                        "{}: tag of the bytes sent",
                        record.path
                    );
                    assert_eq!(body, assets::served_bytes(record.path), "{}", record.path);
                }
            }
        }
    }

    // What makes `immutable` safe: the bytes behind a hashed path are a pure
    // function of its hash. Recompute every tree's hash from what is served.
    #[tokio::test]
    async fn every_hashed_path_serves_the_bytes_its_hash_covers() {
        let journal = established_journal();
        let app = crate::router(journal.path().to_owned());
        let mut served: BTreeMap<&str, Vec<(String, Vec<u8>)>> = BTreeMap::new();
        for hashed in assets::hashed_paths() {
            let record = assets::hashed_record(hashed).unwrap();
            let tree = record.tree.unwrap();
            let (status, headers, body) = fetch(&app, hashed, None).await;
            assert_eq!(status, StatusCode::OK, "{hashed}");
            assert_eq!(
                policy(&headers),
                "public, max-age=31536000, immutable",
                "{hashed}"
            );
            served
                .entry(tree)
                .or_default()
                .push((record.path.to_owned(), body));
        }
        assert_eq!(served.len(), assets::trees().len());
        for (tree, hash) in assets::trees() {
            let files = &served[tree];
            let recomputed = tree_hash_of_served(
                tree,
                files
                    .iter()
                    .map(|(path, bytes)| (path.as_str(), bytes.as_slice())),
            );
            assert_eq!(recomputed, *hash, "{tree}");
            for (path, _) in files {
                let hashed = assets::record(path).unwrap().hashed.unwrap();
                assert!(hashed.starts_with(&format!("{tree}/{hash}/")), "{hashed}");
            }
        }
    }

    #[tokio::test]
    async fn a_held_validator_gets_304_on_the_page_a_fragment_and_a_filled_fragment() {
        let journal = established_journal();
        let app = crate::router(journal.path().to_owned());
        for path in [
            "/",
            "/app/home/",
            "/app/transcripts/20260101",
            "/app/home/workspace",
            "/app/health/workspace",
            "/static/app.js",
        ] {
            let (status, headers, body) = fetch(&app, path, None).await;
            assert_eq!(status, StatusCode::OK, "{path}");
            assert!(!body.is_empty(), "{path}");
            let tag = headers[header::ETAG].to_str().unwrap().to_owned();
            let opaque = tag.trim_matches('"');
            for held in [
                tag.clone(),
                format!("W/{tag}"),
                format!("\"{opaque}-gzip\""),
                format!("\"other\", {tag}"),
            ] {
                let (status, headers, body) = fetch(&app, path, Some(&held)).await;
                assert_eq!(status, StatusCode::NOT_MODIFIED, "{path} with {held}");
                assert!(body.is_empty(), "{path}");
                assert_eq!(headers[header::ETAG], tag.as_str(), "{path}");
                assert_eq!(policy(&headers), "no-cache", "{path}");
                for name in headers.keys() {
                    assert_eq!(
                        headers.get_all(name).iter().count(),
                        1,
                        "{path} repeats {name}"
                    );
                }
            }
            let (status, _, body) = fetch(&app, path, Some("\"0000\"")).await;
            assert_eq!(status, StatusCode::OK, "{path}");
            assert!(!body.is_empty(), "{path}");
        }
    }

    // A caller the gates turn away gets the gates' answer, not a verdict on the hash.
    #[tokio::test]
    async fn an_unknown_hash_is_judged_only_inside_the_gates() {
        let unset = tempfile::tempdir().unwrap();
        let app = crate::router(unset.path().to_owned());
        for path in [
            "/app/health/static/0123456789abcdef/health.js",
            hashed("/app/home/static/home.js"),
        ] {
            let (status, headers, _) = fetch(&app, path, None).await;
            assert_ne!(status, StatusCode::NOT_FOUND, "{path}");
            assert_ne!(status, StatusCode::OK, "{path}");
            assert_eq!(policy(&headers), "no-store", "{path}");
        }
        // The shell's own files are open before setup, hashed or not.
        let (status, _, _) = fetch(&app, "/static/0123456789abcdef/app.js", None).await;
        assert_eq!(status, StatusCode::NOT_FOUND);
        let (status, _, _) = fetch(&app, hashed("/static/app.js"), None).await;
        assert_eq!(status, StatusCode::OK);
    }

    #[test]
    fn hashed_shapes_are_recognised_whatever_the_hash() {
        for path in [
            "/static/0123456789abcdef/app.js",
            "/static/vendor/leaflet/0123456789abcdef/images/layers.png",
            "/app/home/static/0123456789abcdef/home.js",
        ] {
            assert!(has_tree_hash_shape(path), "{path}");
        }
        for path in [
            "/static/app.js",
            "/static/vendor/leaflet/leaflet.js",
            "/app/home/static/home.js",
            "/static/0123456789ABCDEF/app.js",
            "/static/0123456789abcde/app.js",
            "/app/home/workspace",
        ] {
            assert!(!has_tree_hash_shape(path), "{path}");
        }
    }

    #[test]
    fn templates_are_rewritten_by_the_same_rule_as_the_build() {
        let html = r#"<link href="/static/app.css?v=1"><script src="/app/backup/static/backup-status.js"></script><a href="/app/backup/">"#;
        let rewritten = rewrite_references("/app/health/workspace", html);
        assert!(rewritten.contains(&format!("href=\"{}?v=1\"", hashed("/static/app.css"))));
        assert!(rewritten.contains(&format!(
            "src=\"{}\"",
            hashed("/app/backup/static/backup-status.js")
        )));
        assert!(rewritten.contains(r#"href="/app/backup/""#));
    }
}
