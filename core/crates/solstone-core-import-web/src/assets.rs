// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use axum::{
    Extension,
    extract::Path,
    http::{Method, header},
    response::{IntoResponse, Response},
};
use sha2::{Digest, Sha256};
use solstone_core_convey_http::{
    coalesce::ResponseLocale,
    window_catalog::{response_locale, text},
};

use crate::http::{bytes, error, html_not_found};

const WORKSPACE: &[u8] = include_bytes!("../assets/workspace.html");
const IMPORT_DETAIL_JS: &[u8] = include_bytes!("../assets/import_detail.js");
const SOURCE_CONNECTIONS_JS: &[u8] = include_bytes!("../assets/source_connections.js");
const SHELL: &[u8] = include_bytes!("../../solstone-core-convey-shell/assets/static/shell.html");
const CHATGPT_GUIDE: &str = include_str!("../assets/guides/chatgpt.md");
const CLAUDE_GUIDE: &str = include_str!("../assets/guides/claude.md");
const GEMINI_GUIDE: &str = include_str!("../assets/guides/gemini.md");
const ICS_GUIDE: &str = include_str!("../assets/guides/ics.md");
const JOURNAL_ARCHIVE_GUIDE: &str = include_str!("../assets/guides/journal_archive.md");
const STRAVA_GUIDE: &str = include_str!("../assets/guides/strava.md");
const APPLE_HEALTH_GUIDE: &str = include_str!("../assets/guides/apple_health.md");

pub(crate) async fn workspace() -> Response {
    bytes(WORKSPACE, "text/html; charset=utf-8").into_response()
}

pub(crate) async fn shell() -> Response {
    bytes(SHELL, "text/html; charset=utf-8").into_response()
}

pub(crate) async fn background_not_found() -> Response {
    html_not_found().into_response()
}

pub(crate) async fn static_asset(Path(path): Path<String>) -> Response {
    match path.as_str() {
        "source_connections.js" => {
            bytes(SOURCE_CONNECTIONS_JS, "text/javascript; charset=utf-8").into_response()
        }
        "import_detail.js" => {
            bytes(IMPORT_DETAIL_JS, "text/javascript; charset=utf-8").into_response()
        }
        _ => html_not_found().into_response(),
    }
}

pub(crate) async fn guide(
    selected_locale: Option<Extension<ResponseLocale>>,
    Path(source): Path<String>,
    method: Method,
) -> Response {
    if !source
        .bytes()
        .all(|byte| byte.is_ascii_lowercase() || byte == b'_')
        || source.is_empty()
    {
        return error(
            axum::http::StatusCode::BAD_REQUEST,
            "journal.window.import.response.error.one.of.those.values.couldn.t",
            "invalid_request_value",
            "Invalid source name".to_owned(),
            None,
        );
    }
    let Some(guide) = (match source.as_str() {
        "chatgpt" => Some(CHATGPT_GUIDE),
        "claude" => Some(CLAUDE_GUIDE),
        "gemini" => Some(GEMINI_GUIDE),
        "ics" => Some(ICS_GUIDE),
        "journal_archive" => Some(JOURNAL_ARCHIVE_GUIDE),
        "strava" => Some(STRAVA_GUIDE),
        "apple_health" => Some(APPLE_HEALTH_GUIDE),
        _ => None,
    }) else {
        return error(
            axum::http::StatusCode::NOT_FOUND,
            "journal.window.import.response.error.that.file.isn.t.available",
            "file_not_found",
            format!("No guide available for '{source}'"),
            None,
        );
    };
    let markdown = guide_markdown(response_locale(selected_locale), guide);
    let validator = (method == Method::HEAD).then(|| {
        let digest = format!("{:x}", Sha256::digest(markdown.as_bytes()));
        format!("\"{}\"", &digest[..32])
    });
    let mut response = (
        [
            (header::CONTENT_TYPE, "text/markdown; charset=utf-8"),
            (header::VARY, "X-Solstone-Locale"),
        ],
        markdown,
    )
        .into_response();
    // Axum strips a HEAD body before the shell can hash the rendered guide.
    if let Some(tag) = validator.and_then(|value| value.parse().ok()) {
        response.headers_mut().insert(header::ETAG, tag);
    }
    response
}

pub(crate) fn guide_markdown(locale: &str, template: &str) -> String {
    let mut markdown = String::with_capacity(template.len());
    for line in template.split_inclusive('\n') {
        if let Some((prefix, tail)) = line.split_once("{{")
            && let Some((id, suffix)) = tail.split_once("}}")
        {
            markdown.push_str(prefix);
            let (language, message) = id.strip_prefix('!').map_or((locale, id), |id| {
                (solstone_core_convey_http::window_catalog::ENGLISH, id)
            });
            markdown.push_str(&text(language, message));
            markdown.push_str(suffix);
        } else {
            markdown.push_str(line);
        }
    }
    markdown
}
