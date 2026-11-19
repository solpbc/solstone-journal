// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use axum::http::{StatusCode, header};
use axum::response::Response;

use crate::window_catalog::{ENGLISH, escape_html, html, language_tag};

pub fn cant_open_response(status: StatusCode, reason: &str) -> Response {
    cant_open_response_for(ENGLISH, status, reason)
}

pub fn cant_open_response_for(locale: &str, status: StatusCode, reason: &str) -> Response {
    let lang = language_tag(locale);
    let heading = html(locale, "journal.window.link.heading");
    let back = html(locale, "journal.window.link.back");
    let reason = escape_html(reason);
    let script = include_str!("../../solstone-core-convey-shell/assets/static/source_link_back.js");
    let body = format!(
        r#"<!doctype html>
<html lang="{lang}">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1"/>
<title>{heading}</title>
<link rel="stylesheet" href="/static/tokens.css">
<link rel="stylesheet" href="/static/tokens-dark.css">
</head>
<body>
<h1>{heading}</h1>
<p>{reason}</p>
<p><a href="/app/home/" id="back-link">{back}</a></p>
<script>
{script}
</script>
</body>
</html>
"#
    );
    Response::builder()
        .status(status)
        .header(header::CONTENT_TYPE, "text/html; charset=utf-8")
        .body(axum::body::Body::from(body))
        .expect("cant open response builds")
}

/// The one way back a journal-link page offers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LinkPageAction<'a> {
    /// "open your journal": a plain navigation to the journal's root. Never
    /// the answer to the link itself.
    OpenJournal,
    /// "try again": the same link once more. The address is the caller's own
    /// rebuilt route, never text taken from the request.
    TryAgain(&'a str),
}

/// The page a journal link answers with when it can't open its place.
///
/// The reason is the heading and takes focus; one action leads back. Nothing
/// about the place is shown: a hidden place and an absent one read the same.
pub fn link_cant_open_response(
    locale: &str,
    status: StatusCode,
    reason: &str,
    supporting: Option<&str>,
    action: LinkPageAction<'_>,
) -> Response {
    let lang = language_tag(locale);
    let reason = escape_html(reason);
    let supporting = supporting
        .map(|text| format!("<p>{}</p>\n", escape_html(text)))
        .unwrap_or_default();
    let (href, label) = match action {
        LinkPageAction::OpenJournal => (
            "/".to_owned(),
            html(locale, "journal.window.link.open.journal"),
        ),
        // Escaped here, whatever the caller checked: this is the one value on
        // the page that isn't a fixed string.
        LinkPageAction::TryAgain(href) => (
            escape_html(href),
            html(locale, "journal.window.link.try.again"),
        ),
    };
    let body = format!(
        r#"<!doctype html>
<html lang="{lang}">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1"/>
<title>{reason}</title>
<link rel="stylesheet" href="/static/tokens.css">
<link rel="stylesheet" href="/static/tokens-dark.css">
</head>
<body>
<h1 id="reason" tabindex="-1" autofocus>{reason}</h1>
{supporting}<p><a href="{href}" id="link-action">{label}</a></p>
</body>
</html>
"#
    );
    Response::builder()
        .status(status)
        .header(header::CONTENT_TYPE, "text/html; charset=utf-8")
        .header(header::CACHE_CONTROL, "no-store")
        .body(axum::body::Body::from(body))
        .expect("link cant open response builds")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn the_try_again_address_is_escaped_whatever_it_holds() {
        let response = link_cant_open_response(
            ENGLISH,
            StatusCode::INTERNAL_SERVER_ERROR,
            "your journal couldn't check this source.",
            None,
            LinkPageAction::TryAgain("/app/open/\"><script>alert(1)</script>&x"),
        );
        let body = axum::body::to_bytes(response.into_body(), 1 << 20)
            .await
            .unwrap();
        let body = String::from_utf8(body.to_vec()).unwrap();
        assert!(body.contains(
            r#"<a href="/app/open/&quot;&gt;&lt;script&gt;alert(1)&lt;/script&gt;&amp;x" id="link-action">try again</a>"#
        ));
        assert!(!body.contains("<script>"));
    }
}
