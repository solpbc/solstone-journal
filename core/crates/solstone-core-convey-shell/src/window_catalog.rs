// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::sync::OnceLock;

use axum::http::HeaderMap;

#[allow(clippy::all, dead_code)]
mod compiled {
    include!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../../i18n/generated/window/rust/journal-window/api.rs"
    ));
}

const LOCALE_HEADER: &str = "x-solstone-locale";
pub(crate) const ENGLISH: &str = "en-US";

#[derive(Clone)]
pub(crate) struct MissingPage;

/// Resolve the client preference within the locales packaged in this binary.
pub(crate) fn request_locale(headers: &HeaderMap) -> &'static str {
    let mut values = headers.get_all(LOCALE_HEADER).iter();
    let Some(value) = values.next().and_then(|value| value.to_str().ok()) else {
        return ENGLISH;
    };
    if values.next().is_some() {
        return ENGLISH;
    }
    crate::assets::localized_shell(value).map_or(ENGLISH, |(locale, _)| locale)
}

pub(crate) fn not_found(locale: &str) -> String {
    let render = |id| {
        let text = compiled::render_dotted_with_options(locale, id, &[], locale != ENGLISH)
            .expect("missing-page messages compile for every packaged locale");
        text.replace('&', "&amp;")
            .replace('<', "&lt;")
            .replace('>', "&gt;")
    };
    let language = if locale == ENGLISH { "en" } else { locale };
    let title = render("journal.window.not.found.title");
    let heading = render("journal.window.not.found.heading");
    let detail = render("journal.window.not.found.detail");
    format!(
        "<!doctype html>\n<html lang={language}>\n<title>{title}</title>\n<h1>{heading}</h1>\n<p>{detail}</p>\n"
    )
}

pub(crate) fn english_not_found() -> &'static str {
    static HTML: OnceLock<String> = OnceLock::new();
    HTML.get_or_init(|| not_found(ENGLISH))
}
