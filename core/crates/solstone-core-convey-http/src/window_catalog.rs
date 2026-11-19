// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Use the journal-window locale resolved for the current request.

use axum::Extension;

use crate::coalesce::ResponseLocale;

pub use solstone_core_window_catalog::{
    ENGLISH, Value, escape_html, html, language_tag, render, resolve_table, strings, text,
};

pub fn response_locale(locale: Option<Extension<ResponseLocale>>) -> &'static str {
    locale.map_or(ENGLISH, |Extension(locale)| locale.0)
}
