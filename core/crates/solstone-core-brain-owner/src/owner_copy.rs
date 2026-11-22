// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use super::OwnerView;
use solstone_core_cli_copy as copy;
use solstone_core_window_catalog as window;

pub fn config_failure_in(
    error: solstone_core_journal_config::ConfigLoadError,
    locale: &str,
) -> String {
    let detail = config_detail(&error, locale);
    copy::display_for(
        locale,
        "journal.cli.brain.check.config",
        [("detail", &detail)],
    )
}

pub(super) fn check_failure(error: &dyn std::fmt::Display, locale: &str) -> String {
    copy::display_for(
        locale,
        "journal.cli.brain.check.failure",
        [("detail", &error)],
    )
}

pub fn human_text_in(view: &OwnerView, locale: &str) -> String {
    if view.transient == Some("busy") {
        return copy::render_for(locale, "journal.cli.brain.busy", &[]);
    }
    if view.aggregate_state == "ready" {
        return ready(view, locale);
    }
    let state = view.aggregate_state.as_str();
    let reason = solstone_core_brain::brain_reason_text_in(view.reason_code.as_deref(), locale);
    let mut args = vec![("reason", copy::Value::String(&reason))];
    if let Some(component) = &view.failing_component {
        args.push(("component", copy::Value::String(component)));
    }
    let id = match (state, view.failing_component.is_some()) {
        ("blocked", false) => "journal.cli.brain.heading.blocked",
        ("blocked", true) => "journal.cli.brain.heading.blocked.component",
        ("checking", false) => "journal.cli.brain.heading.checking",
        ("checking", true) => "journal.cli.brain.heading.checking.component",
        ("unknown", false) => "journal.cli.brain.heading.unknown",
        ("unknown", true) => "journal.cli.brain.heading.unknown.component",
        ("unhealthy", false) => "journal.cli.brain.heading.unhealthy",
        ("unhealthy", true) => "journal.cli.brain.heading.unhealthy.component",
        (_, false) => {
            args.push(("state", copy::Value::String(state)));
            "journal.cli.brain.heading.other"
        }
        (_, true) => {
            args.push(("state", copy::Value::String(state)));
            "journal.cli.brain.heading.other.component"
        }
    };
    copy::render_for(locale, id, &args)
}

fn ready(view: &OwnerView, locale: &str) -> String {
    let model = view
        .provider
        .as_deref()
        .zip(view.model.as_deref())
        .map(|(provider, model)| solstone_core_brain::provider_model_text(provider, model));
    let lane = view.lane.as_deref();
    let id = match (lane, model.is_some(), view.checked_age.is_some()) {
        (Some("local" | "bundled"), false, false) => "journal.cli.brain.ready.local",
        (Some("local" | "bundled"), false, true) => "journal.cli.brain.ready.local.checked",
        (Some("local" | "bundled"), true, false) => "journal.cli.brain.ready.local.model",
        (Some("local" | "bundled"), true, true) => "journal.cli.brain.ready.local.model.checked",
        (Some("byo" | "byo-cloud" | "byo-endpoint"), false, false) => "journal.cli.brain.ready.byo",
        (Some("byo" | "byo-cloud" | "byo-endpoint"), false, true) => {
            "journal.cli.brain.ready.byo.checked"
        }
        (Some("byo" | "byo-cloud" | "byo-endpoint"), true, false) => {
            "journal.cli.brain.ready.byo.model"
        }
        (Some("byo" | "byo-cloud" | "byo-endpoint"), true, true) => {
            "journal.cli.brain.ready.byo.model.checked"
        }
        (Some("spp" | "confidential"), false, false) => "journal.cli.brain.ready.spp",
        (Some("spp" | "confidential"), false, true) => "journal.cli.brain.ready.spp.checked",
        (Some("spp" | "confidential"), true, false) => "journal.cli.brain.ready.spp.model",
        (Some("spp" | "confidential"), true, true) => "journal.cli.brain.ready.spp.model.checked",
        (None, false, false) => "journal.cli.brain.ready.unknown",
        (None, false, true) => "journal.cli.brain.ready.unknown.checked",
        (None, true, false) => "journal.cli.brain.ready.unknown.model",
        (None, true, true) => "journal.cli.brain.ready.unknown.model.checked",
        (Some(_), false, false) => "journal.cli.brain.ready.other",
        (Some(_), false, true) => "journal.cli.brain.ready.other.checked",
        (Some(_), true, false) => "journal.cli.brain.ready.other.model",
        (Some(_), true, true) => "journal.cli.brain.ready.other.model.checked",
    };
    let mut args = Vec::new();
    if let Some(lane) = lane
        && !matches!(
            lane,
            "local" | "bundled" | "byo" | "byo-cloud" | "byo-endpoint" | "spp" | "confidential"
        )
    {
        args.push(("lane", copy::Value::String(lane)));
    }
    if let Some(model) = model.as_deref() {
        args.push(("model", copy::Value::String(model)));
    }
    if let Some(age) = view.checked_age.as_deref() {
        args.push(("age", copy::Value::String(age)));
    }
    copy::render_for(locale, id, &args)
}

pub(super) fn credential_failure(missing: bool, locale: &str) -> String {
    if missing {
        copy::render_for(locale, "journal.cli.brain.check.key.missing", &[])
    } else {
        copy::render_for(locale, "journal.cli.brain.check.key.rejected", &[])
    }
}

fn config_detail(error: &solstone_core_journal_config::ConfigLoadError, locale: &str) -> String {
    let solstone_core_journal_config::ConfigLoadError::Corrupt { path, .. } = error;
    window::strings(
        locale,
        "journal.window.facets.awareness.response.detail.unreadable.settings",
        [("settings_path", path.display().to_string().as_str())],
    )
}

pub(super) fn config_check_failure_in(
    error: solstone_core_journal_config::ConfigLoadError,
    locale: &str,
) -> String {
    check_failure(&config_detail(&error, locale), locale)
}

pub(super) fn writer_detail(error: &solstone_core_brain::WriterError, locale: &str) -> String {
    use solstone_core_brain::WriterError;
    use solstone_core_journal_io::FlatDirectoryError as Dir;
    let err = match error {
        WriterError::Io(io) => match io.get_ref().and_then(|source| source.downcast_ref::<Dir>()) {
            Some(directory) => directory,
            None => return io.to_string(),
        },
        WriterError::Conflict(detail) | WriterError::Fingerprint(detail) => {
            return detail.render(locale);
        }
        WriterError::Config(error) => return config_detail(error, locale),
        WriterError::Validation(error) => return error.render(locale),
        WriterError::Random => {
            return copy::render_for(locale, "journal.cli.brain.check.random", &[]);
        }
        _ => return error.to_string(),
    };
    solstone_core_journal_io::cli_diagnostics::flat_message(err).render(locale)
}
