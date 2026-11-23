// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Use the journal-window locale resolved for the current request.

use std::sync::OnceLock;

use axum::{
    Extension,
    body::Body,
    http::{StatusCode, header},
    response::Response,
};

use crate::coalesce::ResponseLocale;

pub use solstone_core_window_catalog::{
    ENGLISH, Value, escape_html, html, language_tag, packaged_locale, render, resolve_table,
    screen_category_label, strings, text,
};

pub fn response_locale(locale: Option<Extension<ResponseLocale>>) -> &'static str {
    locale.map_or(ENGLISH, |Extension(locale)| locale.0)
}

pub fn not_found(locale: &str) -> String {
    let language = language_tag(locale);
    let title = html(locale, "journal.window.not.found.title");
    let heading = html(locale, "journal.window.not.found.heading");
    let detail = html(locale, "journal.window.not.found.detail");
    format!(
        "<!doctype html>\n<html lang={language}>\n<title>{title}</title>\n<h1>{heading}</h1>\n<p>{detail}</p>\n"
    )
}

fn english_not_found() -> &'static str {
    static HTML: OnceLock<String> = OnceLock::new();
    HTML.get_or_init(|| not_found(ENGLISH))
}

#[derive(Clone)]
pub struct MissingPage;

pub fn not_found_response() -> Response {
    Response::builder()
        .status(StatusCode::NOT_FOUND)
        .header(header::CONTENT_TYPE, "text/html; charset=utf-8")
        .extension(MissingPage)
        .body(Body::from(english_not_found()))
        .expect("not found response builds")
}

pub fn redirect_body(location: &str, locale: &str) -> String {
    let lang = language_tag(locale);
    let title = html(locale, "journal.window.shell.redirect.title");
    let detail = redirect_instruction(location, locale);
    format!(
        "<!doctype html>\n<html lang={lang}>\n<title>{title}</title>\n<h1>{title}</h1>\n<p>{detail}\n"
    )
}

fn redirect_instruction(location: &str, locale: &str) -> String {
    // Let Fluent position and isolate the link as a single argument. The
    // address is escaped separately and cannot introduce an interpolation marker.
    const LINK: &str = "\u{fdd0}";
    let message = strings(
        locale,
        "journal.window.shell.redirect.instruction",
        [("destination_link", LINK)],
    );
    let location = escape_html(location);
    let link = format!("<a href=\"{location}\">{location}</a>");
    message
        .split(LINK)
        .map(escape_html)
        .collect::<Vec<_>>()
        .join(&link)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn redirect_link_escapes_the_address_in_the_href_and_label() {
        let body = redirect_body("/app/home/?a=1&b=\"<journal>\"", ENGLISH);
        assert!(body.contains(concat!(
            "<a href=\"/app/home/?a=1&amp;b=&quot;&lt;journal&gt;&quot;\">",
            "/app/home/?a=1&amp;b=&quot;&lt;journal&gt;&quot;</a>"
        )));
        assert!(!body.contains("<journal>"));
    }
}
