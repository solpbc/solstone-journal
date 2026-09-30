// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use axum::http::{StatusCode, header};
use axum::response::Response;

pub fn cant_open_response(status: StatusCode, reason: &str) -> Response {
    let script = include_str!("../../solstone-core-convey-shell/assets/static/source_link_back.js");
    let body = format!(
        r#"<!doctype html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1"/>
<title>this source can't be opened</title>
<link rel="stylesheet" href="/static/tokens.css">
<link rel="stylesheet" href="/static/tokens-dark.css">
</head>
<body>
<h1>this source can't be opened</h1>
<p>{reason}</p>
<p><a href="/app/home/" id="back-link">← back</a></p>
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
