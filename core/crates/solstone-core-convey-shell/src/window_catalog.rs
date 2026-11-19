// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::sync::OnceLock;

use axum::http::HeaderMap;
use serde_json::Value;

pub(crate) use solstone_core_convey_http::window_catalog::{
    ENGLISH, escape_html, html, language_tag, response_locale, text,
};

const LOCALE_HEADER: &str = "x-solstone-locale";

#[derive(Clone)]
pub(crate) struct MissingPage;

/// Operations outlive their initiating request. Keep the reference in the
/// registry and fill the response for the client reading the operation.
pub(crate) fn handoff_guidance(group: &str, code: &str, status: Option<u16>) -> Option<String> {
    static IDS: OnceLock<Value> = OnceLock::new();
    let ids = IDS.get_or_init(|| {
        serde_json::from_str(include_str!("../assets/outcomes.json"))
            .expect("handoff copy table parses")
    });
    let id = ids.get(group)?.get(code)?.as_str()?;
    Some(match status {
        Some(status) => format!("{id} {status}"),
        None => id.to_owned(),
    })
}

pub(crate) fn localize_handoff(mut operation: Value, locale: &str) -> Value {
    let Some(guidance) = operation.get_mut("guidance") else {
        return operation;
    };
    let Some(reference) = guidance.as_str().filter(|value| {
        value
            .trim_start_matches('!')
            .starts_with("journal.window.handoff.")
    }) else {
        return operation;
    };
    let (id, status) = reference.split_once(' ').unwrap_or((reference, ""));
    *guidance = Value::String(text(locale, id).replace("{code}", status));
    operation
}

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
    let language = language_tag(locale);
    let title = html(locale, "journal.window.not.found.title");
    let heading = html(locale, "journal.window.not.found.heading");
    let detail = html(locale, "journal.window.not.found.detail");
    format!(
        "<!doctype html>\n<html lang={language}>\n<title>{title}</title>\n<h1>{heading}</h1>\n<p>{detail}</p>\n"
    )
}

pub(crate) fn english_not_found() -> &'static str {
    static HTML: OnceLock<String> = OnceLock::new();
    HTML.get_or_init(|| not_found(ENGLISH))
}
