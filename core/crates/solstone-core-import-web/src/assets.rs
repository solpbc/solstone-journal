// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use axum::{
    extract::Path,
    response::{IntoResponse, Response},
};

use crate::http::{bytes, error, html_not_found};

const WORKSPACE: &[u8] = include_bytes!("../assets/workspace.html");
const IMPORT_DETAIL_JS: &[u8] = include_bytes!("../assets/import_detail.js");
const SHELL: &[u8] = include_bytes!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../../solstone/convey/static/shell.html"
));
const CHATGPT_GUIDE: &[u8] = include_bytes!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../../solstone/apps/import/guides/chatgpt.md"
));
const CLAUDE_GUIDE: &[u8] = include_bytes!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../../solstone/apps/import/guides/claude.md"
));
const GEMINI_GUIDE: &[u8] = include_bytes!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../../solstone/apps/import/guides/gemini.md"
));
const ICS_GUIDE: &[u8] = include_bytes!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../../solstone/apps/import/guides/ics.md"
));
const JOURNAL_ARCHIVE_GUIDE: &[u8] = include_bytes!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../../solstone/apps/import/guides/journal_archive.md"
));
const KINDLE_GUIDE: &[u8] = include_bytes!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../../solstone/apps/import/guides/kindle.md"
));
const OBSIDIAN_GUIDE: &[u8] = include_bytes!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../../solstone/apps/import/guides/obsidian.md"
));

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
        "import_detail.js" => {
            bytes(IMPORT_DETAIL_JS, "text/javascript; charset=utf-8").into_response()
        }
        _ => html_not_found().into_response(),
    }
}

pub(crate) async fn guide(Path(source): Path<String>) -> Response {
    if !source
        .bytes()
        .all(|byte| byte.is_ascii_lowercase() || byte == b'_')
        || source.is_empty()
    {
        return error(
            axum::http::StatusCode::BAD_REQUEST,
            "I couldn't use one of those values.",
            "invalid_request_value",
            "Invalid source name".to_owned(),
        );
    }
    let Some(guide) = (match source.as_str() {
        "chatgpt" => Some(CHATGPT_GUIDE),
        "claude" => Some(CLAUDE_GUIDE),
        "gemini" => Some(GEMINI_GUIDE),
        "ics" => Some(ICS_GUIDE),
        "journal_archive" => Some(JOURNAL_ARCHIVE_GUIDE),
        "kindle" => Some(KINDLE_GUIDE),
        "obsidian" => Some(OBSIDIAN_GUIDE),
        _ => None,
    }) else {
        return error(
            axum::http::StatusCode::NOT_FOUND,
            "I couldn't find that file.",
            "file_not_found",
            format!("No guide available for '{source}'"),
        );
    };
    bytes(guide, "text/markdown; charset=utf-8").into_response()
}

#[cfg(test)]
mod tests {
    #[test]
    fn ac2_assets_match_python_sources() {
        // Retire this half when the import Python surface is deleted; until then it makes that cut safe.
        assert_eq!(
            include_bytes!("../assets/workspace.html"),
            include_bytes!(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/../../../solstone/apps/import/workspace.html"
            )),
        );
        assert_eq!(
            include_bytes!("../assets/import_detail.js"),
            include_bytes!(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/../../../solstone/apps/import/static/import_detail.js"
            )),
        );
    }
}
