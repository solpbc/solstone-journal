// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use axum::{
    extract::Request,
    http::{StatusCode, header},
    middleware::Next,
    response::{IntoResponse, Response},
};
use solstone_core_convey_http::{
    coalesce::ResponseLocale,
    envelope::error_envelope,
    window_catalog::{ENGLISH, strings, text},
};

#[derive(Clone)]
pub(crate) enum Detail {
    Raw(String),
    Catalog(&'static str, Option<(&'static str, String)>),
}

impl From<String> for Detail {
    fn from(value: String) -> Self {
        Self::Raw(value)
    }
}

impl Detail {
    pub(crate) fn catalog(id: &'static str) -> Self {
        Self::Catalog(id, None)
    }

    pub(crate) fn with_arg(id: &'static str, name: &'static str, value: String) -> Self {
        Self::Catalog(id, Some((name, value)))
    }

    fn render(&self, locale: &str) -> String {
        match self {
            Self::Raw(value) => value.clone(),
            Self::Catalog(id, None) => text(locale, id),
            Self::Catalog(id, Some((name, value))) => strings(locale, id, [(*name, value)]),
        }
    }
}

#[derive(Clone)]
struct ErrorCopy {
    reason: String,
    message: &'static str,
    detail: Detail,
}

impl ErrorCopy {
    fn render(&self, status: StatusCode, locale: &str) -> Response {
        error_envelope(
            &self.reason,
            text(locale, self.message),
            self.detail.render(locale),
            status,
        )
        .into_response()
    }
}

pub(crate) fn error(
    reason: &str,
    message: &'static str,
    detail: Detail,
    status: StatusCode,
) -> Response {
    let copy = ErrorCopy {
        reason: reason.to_owned(),
        message,
        detail,
    };
    let mut response = copy.render(status, ENGLISH);
    response.extensions_mut().insert(copy);
    response
}

pub(crate) async fn localize(request: Request, next: Next) -> Response {
    let locale = request
        .extensions()
        .get::<ResponseLocale>()
        .map_or(ENGLISH, |locale| locale.0);
    let mut response = next.run(request).await;
    if let Some(copy) = response
        .extensions_mut()
        .remove::<ErrorCopy>()
        .filter(|_| locale != ENGLISH)
    {
        let localized = copy.render(response.status(), locale);
        *response.body_mut() = localized.into_body();
        response.headers_mut().remove(header::CONTENT_LENGTH);
    }
    response
}
