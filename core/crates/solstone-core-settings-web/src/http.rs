// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use solstone_core_convey_http::window_catalog::text;

use axum::{
    Json,
    http::StatusCode,
    response::{IntoResponse, Response},
};
use serde_json::{Value, json};
use solstone_core_convey_http::envelope::error_envelope;

pub fn json_response(value: Value) -> Response {
    Json(value).into_response()
}

pub fn facet_not_found(locale: &str) -> Response {
    (
        axum::http::StatusCode::NOT_FOUND,
        Json(json!({
            "error": text(locale, "journal.window.settings.http.facet.not.found.that.facet.isn.t.in.your"),
            "reason_code": "facet_not_found",
            "detail": text(locale, "journal.window.settings.http.facet.not.found.facet.not.found"),
        })),
    )
        .into_response()
}

pub fn last_enabled_facet(locale: &str) -> Response {
    error_envelope(
        "last_enabled_facet",
        text(
            locale,
            "journal.window.settings.http.last.enabled.facet.your.journal.needs.at.least.one",
        ),
        text(
            locale,
            "journal.window.settings.http.last.enabled.facet.unmute.or.add.another.facet.first",
        ),
        StatusCode::CONFLICT,
    )
    .into_response()
}

pub fn retired_facet_names_unreadable(locale: &str) -> Response {
    error_envelope(
        "retired_facet_names_unreadable",
        text(locale, "journal.window.settings.http.retired.facet.names.unreadable.your.journal.s.record.of.past"),
        text(locale, "journal.window.settings.http.retired.facet.names.unreadable.run.solstone.journal.facet.doctor.fix"),
        StatusCode::CONFLICT,
    )
    .into_response()
}

pub fn settings_operation_failed(locale: &str) -> Response {
    settings_operation_failed_with_detail(
        locale,
        text(
            locale,
            "journal.window.settings.http.detail.something.went.wrong.try.again.and",
        ),
    )
}

pub fn activity_settings_unavailable(locale: &str) -> Response {
    error_envelope(
        "settings_operation_failed",
        text(locale, "journal.window.settings.http.activity.settings.unavailable.those.activity.settings.couldn.t.be"),
        text(locale, "journal.window.settings.http.activity.settings.unavailable.the.saved.settings.were.left.unchanged"),
        StatusCode::INTERNAL_SERVER_ERROR,
    )
    .into_response()
}

pub fn settings_operation_failed_with_detail(locale: &str, detail: impl Into<String>) -> Response {
    error_envelope(
        "settings_operation_failed",
        text(locale, "journal.window.settings.http.settings.operation.failed.with.detail.those.settings.couldn.t.be.saved"),
        detail.into(),
        StatusCode::INTERNAL_SERVER_ERROR,
    )
    .into_response()
}

pub fn owner_voice_consent_required(locale: &str) -> Response {
    error_envelope(
        "owner_voice_consent_required",
        text(locale, "!journal.window.settings.http.owner.voice.consent.required.only.the.journal.s.owner.can"),
        text(locale, "!journal.window.settings.http.owner.voice.consent.required.turn.on.leave.out.other.voices"),
        StatusCode::FORBIDDEN,
    )
    .into_response()
}

pub fn invalid_config_value(locale: &str, detail: impl Into<String>) -> Response {
    error_envelope(
        "invalid_config_value",
        text(
            locale,
            "journal.window.settings.http.invalid.config.value.that.setting.couldn.t.be.saved",
        ),
        detail.into(),
        StatusCode::BAD_REQUEST,
    )
    .into_response()
}

pub fn invalid_request_value(locale: &str, detail: impl Into<String>) -> Response {
    error_envelope(
        "invalid_request_value",
        text(
            locale,
            "journal.window.settings.http.invalid.request.value.one.of.those.values.couldn.t",
        ),
        detail.into(),
        StatusCode::BAD_REQUEST,
    )
    .into_response()
}

pub fn missing_request_body(locale: &str) -> Response {
    error_envelope(
        "missing_request_body",
        text(
            locale,
            "journal.window.settings.http.missing.request.body.that.request.had.no.data.in",
        ),
        text(
            locale,
            "journal.window.settings.http.missing.request.body.no.data.provided",
        ),
        StatusCode::BAD_REQUEST,
    )
    .into_response()
}

pub fn missing_required_field(locale: &str, detail: impl Into<String>) -> Response {
    error_envelope(
        "missing_required_field",
        text(
            locale,
            "journal.window.settings.http.missing.required.field.a.required.field.is.missing",
        ),
        detail.into(),
        StatusCode::BAD_REQUEST,
    )
    .into_response()
}

pub fn config_busy(locale: &str) -> Response {
    error_envelope(
        "config_busy",
        text(
            locale,
            "journal.window.settings.http.config.busy.those.settings.couldn.t.be.saved",
        ),
        text(
            locale,
            "journal.window.settings.http.config.busy.settings.configuration.is.busy",
        ),
        StatusCode::SERVICE_UNAVAILABLE,
    )
    .into_response()
}

pub fn activity_protected(locale: &str) -> Response {
    error_envelope(
        "activity_protected",
        text(
            locale,
            "journal.window.settings.http.activity.protected.that.activity.couldn.t.be.removed",
        ),
        text(
            locale,
            "journal.window.settings.http.activity.protected.cannot.remove.always.on.activity",
        ),
        StatusCode::BAD_REQUEST,
    )
    .into_response()
}

pub fn activity_not_found(locale: &str) -> Response {
    (
        StatusCode::NOT_FOUND,
        Json(json!({
            "error": text(locale, "journal.window.settings.http.activity.not.found.that.activity.isn.t.in.the"),
            "reason_code": "activity_not_found",
            "detail": text(locale, "journal.window.settings.http.activity.not.found.activity.not.found.in.facet"),
        })),
    )
        .into_response()
}
