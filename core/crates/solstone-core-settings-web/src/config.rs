// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use solstone_core_convey_http::window_catalog::{strings, text};

use std::path::PathBuf;

use axum::{body::Bytes, response::Response};
use serde_json::{Map, Value, json};
use solstone_core_journal_config::is_path_shaped_name;
use solstone_core_journal_config_write::{
    JournalConfigMutation, LockError, LockOptions, mutate_journal_config,
};
use solstone_core_system::stt_backend_choice::{SttRuntimeKind, stt_host_policy};

use crate::{
    http::{
        config_busy, invalid_config_value, json_response, missing_request_body,
        missing_required_field, owner_voice_consent_required, settings_operation_failed,
    },
    request_body::{JsonBody, json_body},
};

// Compatibility-only declaration retained by the 2026-09-10 support-client
// excision decision. These stored keys are neither projected nor writable.
#[allow(dead_code)]
const RETIRED_SUPPORT_CONFIG_KEYS: &[&str] =
    &["enabled", "proactive", "anonymous_feedback", "portal_url"];

pub async fn get(locale: &str, journal_root: PathBuf) -> Response {
    let config = solstone_core_journal_config::read_journal_config(&journal_root)
        .expect("session gate handled corrupt config")
        .config
        .unwrap_or_default();
    match project_public_config(config) {
        Ok(config) => json_response(Value::Object(config)),
        Err(()) => settings_operation_failed(locale),
    }
}

fn processing_validation_detail(
    locale: &str,
    error: solstone_core_journal_config::processing::ProcessingValidationError,
) -> String {
    use solstone_core_journal_config::processing::ProcessingValidationError;
    match error {
        ProcessingValidationError::NotObject => text(
            locale,
            "journal.window.settings.config.detail.processing.not.object",
        ),
        ProcessingValidationError::HistoryConfirmedNotBoolean => text(
            locale,
            "journal.window.settings.config.detail.processing.history.confirmed.boolean",
        ),
        ProcessingValidationError::InvalidMode => text(
            locale,
            "journal.window.settings.config.detail.processing.mode.invalid",
        ),
        ProcessingValidationError::GateNotObject => text(
            locale,
            "journal.window.settings.config.detail.processing.gate.not.object",
        ),
        ProcessingValidationError::ConditionNotObject(name) => strings(
            locale,
            "journal.window.settings.config.detail.processing.condition.not.object",
            [("name", name)],
        ),
        ProcessingValidationError::ConditionEnabledNotBoolean(name) => strings(
            locale,
            "journal.window.settings.config.detail.processing.condition.enabled.boolean",
            [("name", name)],
        ),
        ProcessingValidationError::TimeWindowFieldFormat(key) => strings(
            locale,
            "journal.window.settings.config.detail.processing.time.window.format",
            [("key", key)],
        ),
    }
}

/// A partial update keeps the other condition and its unspecified fields.
fn merge_processing(target: &mut Map<String, Value>, patch: &Map<String, Value>) {
    for key in ["mode", "history_confirmed"] {
        if let Some(value) = patch.get(key) {
            target.insert(key.into(), value.clone());
        }
    }
    if let Some(gate_patch) = patch.get("gate").and_then(Value::as_object) {
        let gate = target.entry("gate").or_insert_with(|| json!({}));
        if !gate.is_object() {
            *gate = json!({});
        }
        let gate = gate.as_object_mut().expect("object");
        for (name, condition_patch) in gate_patch {
            if !matches!(name.as_str(), "time_window" | "display_powersave") {
                continue;
            }
            let condition = gate.entry(name.clone()).or_insert_with(|| json!({}));
            if !condition.is_object() {
                *condition = json!({});
            }
            let condition = condition.as_object_mut().expect("object");
            for (key, value) in condition_patch.as_object().expect("validated condition") {
                condition.insert(key.clone(), value.clone());
            }
        }
    }
}

/// The write closure's refusal when voice-only transcription is turned on without consent.
const OWNER_VOICE_CONSENT_REFUSAL: &str = "owner voice consent required";

/// The face pass runs in describe on desktop journals only; a phone journal never offers it.
pub(crate) const FACES_AVAILABLE: bool = !cfg!(any(target_os = "android", target_os = "ios"));

/// Off, then the three budgets an owner can choose on the "your own model" lane.
const BYO_THINKING_BUDGETS: [u64; 4] = [0, 8192, 16384, 32768];

pub async fn update(
    locale: &str,
    journal_root: PathBuf,
    lock_options: LockOptions,
    body: Bytes,
) -> Response {
    let JsonBody::Value(request) = json_body(body) else {
        return missing_request_body(locale);
    };
    let Some(request) = request.as_object() else {
        return invalid_config_value(
            locale,
            text(
                locale,
                "journal.window.settings.config.detail.request.must.be.an.object",
            ),
        );
    };
    if request.is_empty() {
        return missing_request_body(locale);
    }
    // Only the settings page's confirmation step sends this; it is never stored.
    let owner_voice_consent = request.get("owner_voice_consent") == Some(&Value::Bool(true));
    let mut section = request
        .get("section")
        .and_then(Value::as_str)
        .map(ToOwned::to_owned);
    let mut data = request
        .get("data")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    if let (Some(section), Some(key), Some(value)) = (
        section.as_deref(),
        request.get("key").and_then(Value::as_str),
        request.get("value"),
    ) && data.is_empty()
    {
        data.insert(key.to_owned(), value.clone());
        let _ = section;
    }
    if section.is_none()
        && let Some(identity) = request.get("identity").and_then(Value::as_object)
    {
        section = Some("identity".to_owned());
        data = identity.clone();
    }
    let Some(section) = section else {
        return missing_required_field(
            locale,
            text(
                locale,
                "journal.window.settings.config.detail.no.section.specified",
            ),
        );
    };
    let allowed: &[&str] = match section.as_str() {
        "identity" => &[
            "name",
            "preferred",
            "bio",
            "pronouns",
            "aliases",
            "email_addresses",
            "timezone",
        ],
        "transcribe" => &[
            "backend",
            "preserve_all",
            "confidential_audio",
            "owner_voice_only",
        ],
        "env" => &["PLAUD_ACCESS_TOKEN"],
        // Photo suggestions for people, from faces in meetings.
        "faces" => &["enabled"],
        "processing" => &[],
        // The "your own model" thinking budget: one value for the lane.
        "providers" => &["byo_thinking_budget"],
        // Days before the trash erases what is put in it from now on.
        "trash" => &[
            solstone_core_trash::ERASE_AFTER_DAYS_KEY,
            solstone_core_trash::ERASE_FROM_BACKUPS_KEY,
        ],
        _ => {
            return invalid_config_value(
                locale,
                strings(
                    locale,
                    "journal.window.settings.config.detail.unknown.section",
                    [("section", &section)],
                ),
            );
        }
    };
    if section == "transcribe" {
        if let Some(error) = data.get("backend").and_then(|value| {
            backend_validation_error_on(locale, value, std::env::consts::OS, std::env::consts::ARCH)
        }) {
            return invalid_config_value(locale, error);
        }
        if let Some(device) = data
            .get("parakeet-cpp")
            .and_then(Value::as_object)
            .and_then(|nested| nested.get("device"))
            && !matches!(device.as_str(), Some("auto" | "cpu"))
        {
            // The transcribe stage refuses any other value at run time.
            return invalid_config_value(
                locale,
                text(
                    locale,
                    "journal.window.settings.config.detail.transcribe.parakeet.cpp.device.must.be",
                ),
            );
        }
        for key in ["preserve_all", "confidential_audio", "owner_voice_only"] {
            if data.contains_key(key) && !data[key].is_boolean() {
                return invalid_config_value(
                    locale,
                    strings(
                        locale,
                        "journal.window.settings.config.detail.transcribe.boolean",
                        [("field", &format!("transcribe.{key}"))],
                    ),
                );
            }
        }
    }
    if section == "faces"
        && let Some(enabled) = data.get("enabled")
    {
        if !enabled.is_boolean() {
            return invalid_config_value(
                locale,
                text(
                    locale,
                    "journal.window.settings.config.detail.faces.enabled.must.be.a.boolean",
                ),
            );
        }
        if enabled == &Value::Bool(true) && !FACES_AVAILABLE {
            return invalid_config_value(
                locale,
                text(
                    locale,
                    "journal.window.settings.config.detail.photo.suggestions.run.only.in.a",
                ),
            );
        }
    }
    if section == "providers"
        && data.get("byo_thinking_budget").is_some_and(|value| {
            !value
                .as_u64()
                .is_some_and(|budget| BYO_THINKING_BUDGETS.contains(&budget))
        })
    {
        return invalid_config_value(
            locale,
            text(
                locale,
                "journal.window.settings.config.detail.providers.byo.thinking.budget.must.be",
            ),
        );
    }
    if section == "identity"
        && ["name", "preferred"].iter().any(|key| {
            data.get(*key)
                .and_then(Value::as_str)
                .is_some_and(is_path_shaped_name)
        })
    {
        return invalid_config_value(
            locale,
            text(
                locale,
                "journal.window.settings.config.detail.owner.name.must.not.be.a",
            ),
        );
    }
    if section == "identity"
        && let Some(timezone) = data.get("timezone")
        && !timezone.as_str().is_some_and(|zone| {
            zone.trim().is_empty() || solstone_core_journal_config::parse_zone(zone).is_some()
        })
    {
        return invalid_config_value(
            locale,
            text(
                locale,
                "journal.window.settings.config.detail.home.timezone.must.be.a.name",
            ),
        );
    }
    if section == "processing"
        && let Err(error) = solstone_core_journal_config::processing::validate_processing_typed(
            Some(&Value::Object(data.clone())),
        )
    {
        return invalid_config_value(locale, processing_validation_detail(locale, error));
    }
    if section == solstone_core_trash::SETTINGS_SECTION
        && let Some(days) = data.get(solstone_core_trash::ERASE_AFTER_DAYS_KEY)
        && !days.as_u64().is_some_and(|days| {
            (u64::from(solstone_core_trash::MIN_ERASE_AFTER_DAYS)
                ..=u64::from(solstone_core_trash::MAX_ERASE_AFTER_DAYS))
                .contains(&days)
        })
    {
        return invalid_config_value(
            locale,
            strings(
                locale,
                "journal.window.settings.config.detail.erase.window.days",
                [
                    (
                        "min",
                        &solstone_core_trash::MIN_ERASE_AFTER_DAYS.to_string(),
                    ),
                    (
                        "max",
                        &solstone_core_trash::MAX_ERASE_AFTER_DAYS.to_string(),
                    ),
                ],
            ),
        );
    }
    if section == solstone_core_trash::SETTINGS_SECTION
        && data
            .get(solstone_core_trash::ERASE_FROM_BACKUPS_KEY)
            .is_some_and(|value| !value.is_boolean())
    {
        return invalid_config_value(
            locale,
            text(
                locale,
                "journal.window.settings.config.detail.also.removing.erased.things.from.backups",
            ),
        );
    }
    let data_for_write = data.clone();
    let section_for_write = section.clone();
    let result = mutate_journal_config(&journal_root, lock_options, move |config| {
        let target = config
            .entry(section_for_write.clone())
            .or_insert_with(|| Value::Object(Map::new()));
        let Some(target) = target.as_object_mut() else {
            return JournalConfigMutation {
                changed: false,
                value: Err(text(
                    locale,
                    "journal.window.settings.config.detail.section.must.be.an.object",
                )),
            };
        };
        let mut changed = false;
        let mut changed_fields = Map::new();
        // Turning voice-only transcription on needs the owner's confirmation, decided here
        // against the stored value so concurrent writes cannot slip past it.
        if section_for_write == "transcribe"
            && data_for_write.get("owner_voice_only") == Some(&Value::Bool(true))
            && target.get("owner_voice_only") != Some(&Value::Bool(true))
            && !owner_voice_consent
        {
            return JournalConfigMutation {
                changed: false,
                value: Err(OWNER_VOICE_CONSENT_REFUSAL.to_owned()),
            };
        }
        if section_for_write == "processing" {
            let before = Value::Object(target.clone());
            let mut proposed = target.clone();
            merge_processing(&mut proposed, &data_for_write);
            if let Err(error) = solstone_core_journal_config::processing::validate_processing_typed(
                Some(&Value::Object(proposed.clone())),
            ) {
                return JournalConfigMutation {
                    changed: false,
                    value: Err(processing_validation_detail(locale, error)),
                };
            }
            *target = proposed;
            changed = before != Value::Object(target.clone());
            if changed {
                changed_fields.insert(
                    "processing".to_owned(),
                    json!({"old": before, "new": Value::Object(target.clone())}),
                );
            }
        } else {
            for key in allowed {
                if let Some(value) = data_for_write.get(*key) {
                    let old = target.get(*key).cloned();
                    if old.as_ref() != Some(value) {
                        changed_fields.insert((*key).to_owned(), json!({"old": old, "new": value}));
                        changed = true;
                    }
                    target.insert((*key).to_owned(), value.clone());
                }
            }
            if section_for_write == "transcribe" {
                for backend in ["parakeet", "parakeet-cpp"] {
                    if let Some(nested) = data_for_write.get(backend).and_then(Value::as_object) {
                        let inner = target
                            .entry(backend.to_owned())
                            .or_insert_with(|| Value::Object(Map::new()))
                            .as_object_mut()
                            .expect("created object");
                        for (key, value) in nested {
                            if ["model_version", "device", "timeout_sec"].contains(&key.as_str()) {
                                let old = inner.get(key).cloned();
                                if old.as_ref() != Some(value) {
                                    changed_fields.insert(
                                        format!("{backend}.{key}"),
                                        json!({"old": old, "new": value}),
                                    );
                                    changed = true;
                                }
                                inner.insert(key.clone(), value.clone());
                            }
                        }
                    }
                }
            }
        }
        if section_for_write == "env"
            && data_for_write.contains_key("PLAUD_ACCESS_TOKEN")
            && let Some(validation) = config
                .get_mut("service_key_validation")
                .and_then(Value::as_object_mut)
        {
            changed |= validation.remove("plaud").is_some();
        }
        JournalConfigMutation {
            changed,
            value: Ok((config.clone(), changed_fields)),
        }
    });
    let (config, changed_fields) = match result {
        Ok(value) => match value.value {
            Ok(config) => config,
            Err(error) if error == OWNER_VOICE_CONSENT_REFUSAL => {
                return owner_voice_consent_required(locale);
            }
            Err(error) => return invalid_config_value(locale, error),
        },
        Err(error) => {
            return match error {
                solstone_core_journal_config_write::ConfigMutationError::Lock(
                    LockError::Timeout(_),
                ) => config_busy(locale),
                _ => settings_operation_failed(locale),
            };
        }
    };
    if section == "identity"
        && solstone_core_entity::ensure_configured_principal(&journal_root).is_err()
    {
        return settings_operation_failed(locale);
    }
    // Calendar days and connection times follow the zone: a full pass brings
    // them current now rather than at the daily check.
    if changed_fields.contains_key("timezone")
        && let Err(error) = solstone_core_indexer_store::todo::enqueue_token(
            &journal_root,
            solstone_core_indexer_store::todo::Token::ScanFull,
        )
    {
        log::warn!(target: "solstone::indexer", "search pass after a zone change not queued: {error}");
    }
    if !changed_fields.is_empty() {
        let owner_voice_enabled = changed_fields
            .get("owner_voice_only")
            .is_some_and(|change| change["new"] == Value::Bool(true));
        let log_fields = if section == "env" {
            changed_fields
                .keys()
                .map(|key| (key.clone(), json!({"old":"***","new":"***"})))
                .collect::<Map<_, _>>()
        } else {
            changed_fields
        };
        let mut log_body = json!({"changed_fields": log_fields});
        if section == "transcribe" && owner_voice_consent && owner_voice_enabled {
            log_body["consent"] = json!("owner_confirmed");
        }
        if solstone_core_facets::append_action_log(
            &journal_root,
            None,
            "app",
            "settings",
            &format!("{section}_update"),
            log_body,
        )
        .is_err()
        {
            return settings_operation_failed(locale);
        }
    }
    let key_validation = config
        .get("service_key_validation")
        .and_then(Value::as_object)
        .and_then(|values| values.get("plaud"))
        .cloned()
        .map(|value| json!({"plaud": value}))
        .unwrap_or_else(|| json!({}));
    match project_public_config(config.clone()) {
        Ok(public) => json_response(
            json!({"config": public, "key_validation": key_validation, "success": true}),
        ),
        Err(()) => settings_operation_failed(locale),
    }
}

/// Top-level `config/journal.json` sections that hold credentials or private
/// service state and must never appear in public settings responses.
const REDACTED_CONFIG_SECTIONS: &[&str] = &[
    "service_key_validation",
    "support",
    "email",
    "granola",
    "garmin",
    "providers",
    "oura",
    "backup",
    "services",
    "voice",
];

pub fn project_public_config(mut config: Map<String, Value>) -> Result<Map<String, Value>, ()> {
    let validation = config
        .get("service_key_validation")
        .and_then(Value::as_object)
        .and_then(|values| values.get("plaud"))
        .cloned();
    for section in REDACTED_CONFIG_SECTIONS {
        config.remove(*section);
    }
    if let Some(value) = validation {
        config.insert("key_validation".to_owned(), json!({"plaud": value}));
    }
    let env = config.get("env").and_then(Value::as_object);
    config.insert(
        "env".to_owned(),
        json!({"PLAUD_ACCESS_TOKEN": env.and_then(|values| values.get("PLAUD_ACCESS_TOKEN")).is_some_and(truthy)}),
    );
    if let Some(journal) = config.get_mut("journal").and_then(Value::as_object_mut) {
        journal.remove("name");
        if journal.is_empty() {
            config.remove("journal");
        }
    }
    let convey = config
        .entry("convey".to_owned())
        .or_insert_with(|| Value::Object(Map::new()));
    let Value::Object(convey) = convey else {
        return Err(());
    };
    convey.remove("secret");
    convey.remove("password_hash");
    convey.remove("password");
    if let Some(transcribe) = config.get("transcribe").cloned() {
        config.insert(
            "transcribe".to_owned(),
            project_transcribe(transcribe, false),
        );
    }
    // An unset switch on a computer stays unset; a phone journal always says
    // it can't run photo suggestions, so its page offers no switch.
    if let Some(faces) = project_faces(config.get("faces"), FACES_AVAILABLE) {
        config.insert("faces".to_owned(), faces);
    }
    config.insert(
        "runtime_env".to_owned(),
        json!({"PLAUD_ACCESS_TOKEN": std::env::var_os("PLAUD_ACCESS_TOKEN").is_some_and(|value| !value.is_empty())}),
    );
    Ok(config)
}

/// The switch as every face reader takes it: on unless the owner turned it
/// off, and any stored value other than `true` or `false` reads as off.
/// `available` says whether this journal runs the face pass at all. A
/// computer's journal that never set it projects nothing, and the page reads
/// that absence as on.
fn project_faces(faces: Option<&Value>, available: bool) -> Option<Value> {
    if faces.is_none() && available {
        return None;
    }
    let enabled = available
        && match faces.and_then(|f| f.get("enabled")) {
            None => true,
            Some(value) => value == &Value::Bool(true),
        };
    let photo_match_enabled = available
        && match faces.and_then(|f| f.get("photo_match_enabled")) {
            None => true,
            Some(value) => value == &Value::Bool(true),
        };
    let mut obj = Map::new();
    obj.insert("enabled".to_owned(), Value::Bool(enabled));
    obj.insert("available".to_owned(), Value::Bool(available));
    if available {
        obj.insert(
            "photo_match_enabled".to_owned(),
            Value::Bool(photo_match_enabled),
        );
    }
    Some(Value::Object(obj))
}

/// A known backend this host cannot run. A mac has no parakeet.cpp server, so
/// saving that backend there would leave the journal with no transcription.
fn backend_refusal_on(backend: &str, os: &str) -> Option<&'static str> {
    (backend == "parakeet-cpp" && matches!(os, "macos" | "darwin"))
        .then_some("journal.window.settings.config.detail.macos.backend")
}

fn embedded_speech_on(os: &str, arch: &str) -> bool {
    stt_host_policy(os, arch).runtime_kind == Some(SttRuntimeKind::MobileEmbeddedCpp)
}

fn backend_validation_error_on(
    locale: &str,
    value: &Value,
    os: &str,
    arch: &str,
) -> Option<String> {
    let backend = value.as_str().unwrap_or_default();
    let embedded = embedded_speech_on(os, arch);
    if !matches!(backend, "parakeet" | "parakeet-cpp") && !(embedded && backend == "confidential") {
        let allowed = if embedded {
            "parakeet, parakeet-cpp, confidential"
        } else {
            "parakeet, parakeet-cpp"
        };
        return Some(strings(
            locale,
            "journal.window.settings.config.detail.backend",
            [("backend", backend), ("allowed", allowed)],
        ));
    }
    backend_refusal_on(backend, os).map(|id| text(locale, id))
}

pub fn project_transcribe(value: Value, include_confidential_audio: bool) -> Value {
    project_transcribe_on(
        value,
        include_confidential_audio,
        std::env::consts::OS,
        std::env::consts::ARCH,
    )
}

fn project_transcribe_on(
    value: Value,
    include_confidential_audio: bool,
    os: &str,
    arch: &str,
) -> Value {
    let Some(values) = value.as_object() else {
        return json!({});
    };
    let mut projected = Map::new();
    for (key, value) in values {
        match key.as_str() {
            "parakeet" | "parakeet-cpp" => {
                if let Some(nested) = value.as_object() {
                    let allowed: &[&str] = if key == "parakeet" {
                        &["model_version", "device", "timeout_sec"]
                    } else {
                        &["device"]
                    };
                    projected.insert(
                        key.clone(),
                        Value::Object(
                            nested
                                .iter()
                                .filter(|(nested_key, _)| allowed.contains(&nested_key.as_str()))
                                .map(|(nested_key, nested_value)| {
                                    (nested_key.clone(), nested_value.clone())
                                })
                                .collect(),
                        ),
                    );
                }
            }
            "backend" | "preserve_all" | "confidential_audio" | "min_speech_seconds"
                if !value.is_object() =>
            {
                projected.insert(key.clone(), value.clone());
            }
            // Only a JSON `true` is on, as the transcriber reads it.
            "owner_voice_only" => {
                projected.insert(key.clone(), Value::Bool(value == &Value::Bool(true)));
            }
            _ => {}
        }
    }
    // A mobile owner can choose confidential speech explicitly. An unset backend
    // must stay unset so the page does not present a manufactured local choice.
    if !embedded_speech_on(os, arch)
        && !matches!(
            projected.get("backend").and_then(Value::as_str),
            Some("parakeet" | "parakeet-cpp")
        )
    {
        projected.insert("backend".to_owned(), Value::String("parakeet".to_owned()));
    }
    if include_confidential_audio {
        projected.insert(
            "confidential_audio".to_owned(),
            Value::Bool(values.get("confidential_audio").map(truthy).unwrap_or(true)),
        );
    }
    Value::Object(projected)
}

pub fn truthy(value: &Value) -> bool {
    match value {
        Value::Null | Value::Bool(false) => false,
        Value::Bool(true) => true,
        Value::Number(number) => number.as_f64().is_none_or(|number| number != 0.0),
        Value::String(value) => !value.is_empty(),
        Value::Array(values) => !values.is_empty(),
        Value::Object(values) => !values.is_empty(),
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn a_mac_refuses_the_parakeet_cpp_backend() {
        assert!(super::backend_refusal_on("parakeet-cpp", "macos").is_some());
        assert!(super::backend_refusal_on("parakeet", "macos").is_none());
        assert!(super::backend_refusal_on("parakeet-cpp", "linux").is_none());
        assert!(super::backend_refusal_on("parakeet-cpp", "windows").is_none());
    }

    #[test]
    fn only_embedded_mobile_hosts_admit_the_confidential_speech_backend() {
        for os in ["android", "ios"] {
            for arch in ["aarch64", "arm64"] {
                for backend in ["parakeet", "parakeet-cpp", "confidential"] {
                    assert!(
                        super::backend_validation_error_on(
                            solstone_core_convey_http::window_catalog::ENGLISH,
                            &json!(backend),
                            os,
                            arch
                        )
                        .is_none(),
                        "{os}/{arch}: {backend}"
                    );
                }
                for value in [json!("unknown"), json!(null), json!(true), json!(1)] {
                    assert!(
                        super::backend_validation_error_on(
                            solstone_core_convey_http::window_catalog::ENGLISH,
                            &value,
                            os,
                            arch
                        )
                        .is_some(),
                        "{os}/{arch}: {value}"
                    );
                }
            }
        }
        for (os, arch) in [
            ("linux", "x86_64"),
            ("linux", "aarch64"),
            ("windows", "x86_64"),
            ("windows", "aarch64"),
            ("macos", "aarch64"),
            ("android", "x86_64"),
            ("ios", "x86_64"),
        ] {
            assert!(
                super::backend_validation_error_on(
                    solstone_core_convey_http::window_catalog::ENGLISH,
                    &json!("confidential"),
                    os,
                    arch
                )
                .is_some(),
                "{os}/{arch}"
            );
            assert!(
                super::backend_validation_error_on(
                    solstone_core_convey_http::window_catalog::ENGLISH,
                    &json!("parakeet"),
                    os,
                    arch
                )
                .is_none(),
                "{os}/{arch}"
            );
            assert_eq!(
                super::backend_validation_error_on(
                    solstone_core_convey_http::window_catalog::ENGLISH,
                    &json!("parakeet-cpp"),
                    os,
                    arch
                )
                .is_some(),
                os == "macos",
                "{os}/{arch}"
            );
        }
    }

    #[test]
    fn mobile_speech_projection_preserves_the_optional_backend_without_changing_input() {
        for os in ["android", "ios"] {
            for arch in ["aarch64", "arm64"] {
                for include_confidential_audio in [false, true] {
                    for backend in [
                        None,
                        Some("parakeet"),
                        Some("parakeet-cpp"),
                        Some("confidential"),
                    ] {
                        let mut raw = json!({});
                        if let Some(backend) = backend {
                            raw["backend"] = json!(backend);
                        }
                        let projected = super::project_transcribe_on(
                            raw.clone(),
                            include_confidential_audio,
                            os,
                            arch,
                        );
                        assert_eq!(projected.get("backend"), raw.get("backend"), "{os}/{arch}");
                        assert!(raw.get("confidential_audio").is_none());
                        assert_eq!(
                            projected.get("confidential_audio"),
                            include_confidential_audio.then_some(&json!(true)),
                            "{os}/{arch}"
                        );
                    }
                    let projected = super::project_transcribe_on(
                        json!({"backend":"confidential", "confidential_audio":false}),
                        include_confidential_audio,
                        os,
                        arch,
                    );
                    assert_eq!(projected["backend"], "confidential");
                    assert_eq!(projected["confidential_audio"], false);
                }
            }
        }
    }

    #[test]
    fn desktop_speech_projection_keeps_its_existing_local_backend_default() {
        for (os, arch) in [
            ("linux", "x86_64"),
            ("linux", "aarch64"),
            ("windows", "x86_64"),
            ("windows", "aarch64"),
            ("macos", "aarch64"),
        ] {
            for include_confidential_audio in [false, true] {
                for (raw, expected) in [
                    (json!({}), "parakeet"),
                    (json!({"backend":"confidential"}), "parakeet"),
                    (json!({"backend":"parakeet"}), "parakeet"),
                    (json!({"backend":"parakeet-cpp"}), "parakeet-cpp"),
                ] {
                    let projected =
                        super::project_transcribe_on(raw, include_confidential_audio, os, arch);
                    assert_eq!(projected["backend"], expected, "{os}/{arch}");
                }
            }
        }
    }

    #[tokio::test]
    async fn a_parakeet_cpp_device_the_stage_would_refuse_is_not_saved() {
        let root = crate::test_support::phase_root("established");
        let before = std::fs::read(root.path().join("config/journal.json")).expect("config");
        for (device, status) in [("cuda", 400), ("auto", 200)] {
            let body =
                json!({"section": "transcribe", "data": {"parakeet-cpp": {"device": device}}});
            let response = crate::test_support::shell_router(root.path())
                .oneshot(
                    Request::put("/app/settings/api/config")
                        .header("content-type", "application/json")
                        .body(Body::from(body.to_string()))
                        .expect("request"),
                )
                .await
                .expect("response");
            assert_eq!(response.status().as_u16(), status, "{device}");
            if status == 400 {
                assert_eq!(
                    std::fs::read(root.path().join("config/journal.json")).expect("config"),
                    before
                );
            }
        }
    }

    use axum::{
        body::{Body, to_bytes},
        http::Request,
    };
    use serde_json::json;
    use tower::ServiceExt;

    fn files_containing(root: &std::path::Path, needle: &str) -> usize {
        let mut count = 0;
        let mut stack = vec![root.to_path_buf()];
        while let Some(dir) = stack.pop() {
            for entry in std::fs::read_dir(&dir).into_iter().flatten().flatten() {
                let path = entry.path();
                if path.is_dir() {
                    stack.push(path);
                } else if std::fs::read_to_string(&path).is_ok_and(|text| text.contains(needle)) {
                    count += 1;
                }
            }
        }
        count
    }

    async fn send(
        root: &std::path::Path,
        method: &str,
        body: serde_json::Value,
    ) -> (u16, serde_json::Value) {
        let request = Request::builder()
            .method(method)
            .uri("/app/settings/api/config")
            .header("content-type", "application/json")
            .body(Body::from(body.to_string()))
            .unwrap();
        let response = crate::test_support::shell_router(root)
            .oneshot(request)
            .await
            .unwrap();
        let status = response.status().as_u16();
        let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        (
            status,
            serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null),
        )
    }

    #[tokio::test]
    async fn turning_voice_only_on_needs_the_confirmation_on_every_route() {
        let root = crate::test_support::phase_root("established");
        let path = root.path().join("config/journal.json");
        for method in ["PUT", "POST"] {
            for consent in [None, Some(json!(false)), Some(json!("true"))] {
                let before = std::fs::read(&path).unwrap();
                let mut body = json!({"section": "transcribe", "data": {"owner_voice_only": true}});
                if let Some(consent) = consent {
                    body["owner_voice_consent"] = consent;
                }
                let (status, response) = send(root.path(), method, body).await;
                assert_eq!(status, 403, "{method}");
                assert_eq!(response["reason_code"], "owner_voice_consent_required");
                assert!(!response.to_string().contains("owner_voice_consent\""));
                assert_eq!(std::fs::read(&path).unwrap(), before);
            }
            let (status, _) = send(
                root.path(),
                method,
                json!({"section": "transcribe", "key": "owner_voice_only", "value": true}),
            )
            .await;
            assert_eq!(status, 403, "key/value form on {method}");
        }
        for value in [json!("true"), json!(1), serde_json::Value::Null] {
            let before = std::fs::read(&path).unwrap();
            let (status, _) = send(
                root.path(),
                "PUT",
                json!({"section": "transcribe", "data": {"owner_voice_only": value}, "owner_voice_consent": true}),
            )
            .await;
            assert_eq!(status, 400, "{value}");
            assert_eq!(std::fs::read(&path).unwrap(), before);
        }

        let (status, response) = send(
            root.path(),
            "PUT",
            json!({"section": "transcribe", "data": {"owner_voice_only": true}, "owner_voice_consent": true}),
        )
        .await;
        assert_eq!(status, 200);
        assert_eq!(response["config"]["transcribe"]["owner_voice_only"], true);
        let stored = std::fs::read_to_string(&path).unwrap();
        assert!(stored.contains("owner_voice_only"));
        assert!(!stored.contains("owner_voice_consent"));
        assert_eq!(files_containing(root.path(), "owner_confirmed"), 1);

        // Already on: re-saving needs no confirmation and records none.
        let (status, _) = send(
            root.path(),
            "PUT",
            json!({"section": "transcribe", "data": {"owner_voice_only": true}}),
        )
        .await;
        assert_eq!(status, 200);
        // Turning it off needs none either.
        let (status, response) = send(
            root.path(),
            "PUT",
            json!({"section": "transcribe", "data": {"owner_voice_only": false}}),
        )
        .await;
        assert_eq!(status, 200);
        assert_eq!(response["config"]["transcribe"]["owner_voice_only"], false);
        let (status, _) = send(
            root.path(),
            "PUT",
            json!({"section": "transcribe", "data": {"backend": "parakeet"}, "owner_voice_consent": true}),
        )
        .await;
        assert_eq!(status, 200);
        assert_eq!(files_containing(root.path(), "owner_confirmed"), 1);
    }

    #[tokio::test]
    async fn photo_suggestions_turn_off_and_on_as_an_ordinary_setting() {
        let root = crate::test_support::phase_root("established");
        let path = root.path().join("config/journal.json");
        for value in [json!("false"), json!(0), serde_json::Value::Null] {
            let before = std::fs::read(&path).unwrap();
            let (status, _) = send(
                root.path(),
                "PUT",
                json!({"section": "faces", "data": {"enabled": value}}),
            )
            .await;
            assert_eq!(status, 400, "{value}");
            assert_eq!(std::fs::read(&path).unwrap(), before);
        }
        for value in [false, true, false] {
            let (status, response) = send(
                root.path(),
                "PUT",
                json!({"section": "faces", "data": {"enabled": value}}),
            )
            .await;
            assert_eq!(status, 200);
            assert_eq!(response["config"]["faces"]["enabled"], value);
            let stored: serde_json::Value =
                serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
            assert_eq!(stored["faces"], json!({"enabled": value}));
        }
    }

    #[test]
    fn photo_suggestions_are_on_unless_turned_off_and_say_where_they_run() {
        for (value, on) in [
            (json!({"enabled": true}), true),
            (json!({"enabled": false}), false),
            (json!({"enabled": "true"}), false),
            (json!({"enabled": 1}), false),
            (json!({}), true),
            (serde_json::Value::Null, true),
        ] {
            assert_eq!(
                super::project_faces(Some(&value), true),
                Some(json!({"enabled": on, "available": true, "photo_match_enabled": true})),
                "{value}"
            );
        }
        assert_eq!(super::project_faces(None, true), None);
        assert_eq!(
            super::project_faces(None, false),
            Some(json!({"enabled": false, "available": false}))
        );
        assert_eq!(
            super::project_faces(
                Some(&json!({"enabled": true, "photo_match_enabled": true})),
                true
            ),
            Some(json!({"enabled": true, "available": true, "photo_match_enabled": true}))
        );
        assert_eq!(
            super::project_faces(
                Some(&json!({"enabled": true, "photo_match_enabled": false})),
                true
            ),
            Some(json!({"enabled": true, "available": true, "photo_match_enabled": false}))
        );
        assert_eq!(
            super::project_faces(
                Some(&json!({"enabled": true, "photo_match_enabled": "true"})),
                true
            ),
            Some(json!({"enabled": true, "available": true, "photo_match_enabled": false}))
        );
        assert_eq!(
            super::project_faces(
                Some(&json!({"enabled": true, "photo_match_enabled": true})),
                false
            ),
            Some(json!({"enabled": false, "available": false}))
        );
    }

    #[test]
    fn the_projection_reads_only_json_true_as_on() {
        for (value, on) in [
            (json!(true), true),
            (json!(false), false),
            (json!("true"), false),
            (json!(1), false),
            (serde_json::Value::Null, false),
        ] {
            let projected = super::project_transcribe(json!({"owner_voice_only": value}), false);
            assert_eq!(projected["owner_voice_only"], json!(on), "{value}");
        }
        assert!(
            super::project_transcribe(json!({}), false)
                .get("owner_voice_only")
                .is_none()
        );
    }

    #[tokio::test]
    async fn processing_gate_roundtrips_partial_updates_and_rejects_invalid_without_writing() {
        let root = crate::test_support::phase_root("established");
        let path = root.path().join("config/journal.json");
        for (patch, status) in [
            (
                json!({"mode":"deferred","gate":{"time_window":{"start":"23:00","end":"05:00","enabled":true},"display_powersave":{"enabled":true}}}),
                200,
            ),
            (json!({"gate":{"time_window":{"end":"04:00"}}}), 200),
            (json!({"gate":{"time_window":{"start":"25:00"}}}), 400),
            (json!({"mode":"unknown"}), 400),
            (json!({"history_confirmed":true}), 200),
            (json!({"history_confirmed":"true"}), 400),
            (json!({"gate":{"display_powersave":{"enabled":"yes"}}}), 400),
        ] {
            let before = std::fs::read(&path).unwrap();
            let response = crate::test_support::shell_router(root.path())
                .oneshot(
                    Request::put("/app/settings/api/config")
                        .header("content-type", "application/json")
                        .body(Body::from(
                            json!({"section":"processing","data":patch}).to_string(),
                        ))
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(response.status().as_u16(), status, "{patch}");
            if status == 400 {
                assert_eq!(std::fs::read(&path).unwrap(), before);
            }
        }
        let response = crate::test_support::shell_router(root.path())
            .oneshot(
                Request::get("/app/settings/api/config")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), 200);
        let public: serde_json::Value =
            serde_json::from_slice(&to_bytes(response.into_body(), usize::MAX).await.unwrap())
                .unwrap();
        assert_eq!(public["processing"]["history_confirmed"], true);
        assert_eq!(
            public["processing"]["gate"]["time_window"]["start"],
            "23:00"
        );
        assert_eq!(public["processing"]["gate"]["time_window"]["end"], "04:00");
        assert_eq!(
            public["processing"]["gate"]["display_powersave"]["enabled"],
            true
        );
        let config = solstone_core_journal_config::read_journal_config(root.path())
            .unwrap()
            .config
            .unwrap();
        let settings =
            solstone_core_journal_config::processing::processing_settings(&config).unwrap();
        assert!(settings.deferred);
        assert!(
            solstone_core_journal_config::processing::evaluate_drain_gate(
                settings,
                jiff::civil::Time::constant(23, 30, 0, 0),
                solstone_core_journal_config::processing::DisplayPowersaveReading::UNAVAILABLE
            )
            .open
        );
    }

    #[tokio::test]
    async fn the_byo_thinking_budget_saves_only_a_value_processing_reads() {
        let root = crate::test_support::phase_root("established");
        let config_path = root.path().join("config/journal.json");
        for (budget, status) in [
            (json!(4096), 400),
            (json!("8192"), 400),
            (json!(16384), 200),
        ] {
            let before = std::fs::read(&config_path).expect("config");
            let body = json!({"section": "providers", "data": {"byo_thinking_budget": budget}});
            let response = crate::test_support::shell_router(root.path())
                .oneshot(
                    Request::put("/app/settings/api/config")
                        .header("content-type", "application/json")
                        .body(Body::from(body.to_string()))
                        .expect("request"),
                )
                .await
                .expect("response");
            assert_eq!(response.status().as_u16(), status, "{budget}");
            let after: serde_json::Value =
                serde_json::from_slice(&std::fs::read(&config_path).expect("config"))
                    .expect("JSON");
            if status == 400 {
                assert_eq!(std::fs::read(&config_path).expect("config"), before);
            } else {
                let mut expected: serde_json::Value =
                    serde_json::from_slice(&before).expect("JSON");
                expected["providers"]["byo_thinking_budget"] = budget;
                assert_eq!(after, expected);
            }
        }
    }

    #[tokio::test]
    async fn ac4_rich_config_preserves_future_section_and_exact_keys() {
        let root = crate::test_support::phase_root("rich");
        let response = crate::test_support::shell_router(root.path())
            .oneshot(
                Request::get("/app/settings/api/config")
                    .body(Body::empty())
                    .expect("request"),
            )
            .await
            .expect("response");
        let config: serde_json::Value = serde_json::from_slice(
            &to_bytes(response.into_body(), usize::MAX)
                .await
                .expect("body"),
        )
        .expect("JSON");
        assert_eq!(config["some_future_section"], json!({"kept": true, "n": 7}));
        let mut keys = config
            .as_object()
            .expect("config object")
            .keys()
            .cloned()
            .collect::<Vec<_>>();
        keys.sort();
        let mut expected =
            crate::test_support::corpus()["phases"]["rich"]["GET api/config"]["normalized"]
                .as_object()
                .expect("recorded config")
                .keys()
                .cloned()
                .collect::<Vec<_>>();
        expected.sort();
        assert_eq!(keys, expected);
    }

    #[tokio::test]
    async fn leftover_agent_object_survives_config_get() {
        let root = crate::test_support::established_root();
        let leftover = json!({
            "name": "Ada",
            "name_status": "chosen",
            "named_date": "2026-01-02",
            "sibling": true,
        });
        let config_path = root.path().join("config/journal.json");
        let mut config: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&config_path).expect("config")).expect("JSON");
        config["agent"] = leftover.clone();
        std::fs::write(
            &config_path,
            serde_json::to_vec(&config).expect("config JSON"),
        )
        .expect("config writes");
        let before = std::fs::read(&config_path).expect("before");

        let response = crate::test_support::shell_router(root.path())
            .oneshot(
                Request::get("/app/settings/api/config")
                    .body(Body::empty())
                    .expect("request"),
            )
            .await
            .expect("response");
        assert_eq!(response.status(), axum::http::StatusCode::OK);
        let body: serde_json::Value = serde_json::from_slice(
            &to_bytes(response.into_body(), usize::MAX)
                .await
                .expect("body"),
        )
        .expect("JSON");
        assert_eq!(body["agent"], leftover);
        assert_eq!(std::fs::read(&config_path).expect("after"), before);
    }

    #[tokio::test]
    async fn non_object_convey_returns_the_settings_operation_failure() {
        let root = crate::test_support::established_root();
        std::fs::write(
            root.path().join("config/journal.json"),
            serde_json::to_vec(&json!({
                "setup": {"completed_at": 1},
                "convey": "not an object",
            }))
            .expect("config JSON"),
        )
        .expect("config writes");
        let response = crate::test_support::shell_router(root.path())
            .oneshot(
                Request::get("/app/settings/api/config")
                    .body(Body::empty())
                    .expect("request"),
            )
            .await
            .expect("response");
        assert_eq!(
            response.status(),
            axum::http::StatusCode::INTERNAL_SERVER_ERROR
        );
        let body: serde_json::Value = serde_json::from_slice(
            &to_bytes(response.into_body(), usize::MAX)
                .await
                .expect("body"),
        )
        .expect("JSON");
        assert_eq!(body["reason_code"], "settings_operation_failed");
        assert_eq!(
            body["detail"],
            solstone_core_convey_http::window_catalog::text(
                solstone_core_convey_http::window_catalog::ENGLISH,
                "journal.window.settings.http.detail.status",
            )
        );
    }

    async fn post_identity(
        root: &std::path::Path,
        body: serde_json::Value,
    ) -> (axum::http::StatusCode, serde_json::Value) {
        let response = crate::test_support::shell_router(root)
            .oneshot(
                Request::post("/app/settings/api/config")
                    .body(Body::from(serde_json::to_vec(&body).expect("body")))
                    .expect("request"),
            )
            .await
            .expect("response");
        let status = response.status();
        let body: serde_json::Value = serde_json::from_slice(
            &to_bytes(response.into_body(), usize::MAX)
                .await
                .expect("body"),
        )
        .expect("JSON");
        (status, body)
    }

    fn identity_field(root: &std::path::Path, key: &str) -> Option<String> {
        let config: serde_json::Value = serde_json::from_slice(
            &std::fs::read(root.join("config/journal.json")).expect("config"),
        )
        .expect("config JSON");
        config["identity"][key].as_str().map(str::to_owned)
    }

    #[tokio::test]
    async fn agent_section_is_unknown_and_leaves_disk_unchanged() {
        let root = crate::test_support::established_root();
        let leftover = json!({
            "name": "Ada",
            "name_status": "chosen",
            "named_date": "2026-01-02",
            "sibling": true,
        });
        let config_path = root.path().join("config/journal.json");
        let mut config: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&config_path).expect("config")).expect("JSON");
        config["agent"] = leftover.clone();
        std::fs::write(
            &config_path,
            serde_json::to_vec(&config).expect("config JSON"),
        )
        .expect("config writes");
        let before = std::fs::read(&config_path).expect("before");

        let response = crate::test_support::shell_router(root.path())
            .oneshot(
                Request::put("/app/settings/api/config")
                    .body(Body::from(
                        serde_json::to_vec(&json!({"section":"agent","data":{"name":"Nova"}}))
                            .expect("body"),
                    ))
                    .expect("request"),
            )
            .await
            .expect("response");
        let status = response.status();
        let body: serde_json::Value = serde_json::from_slice(
            &to_bytes(response.into_body(), usize::MAX)
                .await
                .expect("body"),
        )
        .expect("JSON");

        assert_eq!(status, axum::http::StatusCode::BAD_REQUEST);
        assert_eq!(body["reason_code"], "invalid_config_value");
        assert_eq!(body["detail"], "Unknown section: agent");
        assert_eq!(std::fs::read(&config_path).expect("after"), before);
        let after: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&config_path).expect("after JSON"))
                .expect("JSON");
        assert_eq!(after["agent"], leftover);
    }

    #[tokio::test]
    async fn identity_name_refuses_a_path_shaped_value() {
        for name in ["~/x", "a/b", "a\\b"] {
            let root = crate::test_support::established_root();
            let (status, body) = post_identity(
                root.path(),
                json!({"section":"identity","data":{"name":name}}),
            )
            .await;
            assert_eq!(status, axum::http::StatusCode::BAD_REQUEST, "{name}");
            assert_eq!(body["reason_code"], "invalid_config_value");
            assert_eq!(identity_field(root.path(), "name"), None);
        }
    }

    #[tokio::test]
    async fn identity_name_accepts_a_benign_value() {
        let root = crate::test_support::established_root();
        let (status, _) = post_identity(
            root.path(),
            json!({"section":"identity","data":{"name":"Ada"}}),
        )
        .await;
        assert_eq!(status, axum::http::StatusCode::OK);
        assert_eq!(identity_field(root.path(), "name").as_deref(), Some("Ada"));
        let principal = solstone_core_entity::read_journal_principal(root.path())
            .unwrap()
            .expect("profile save creates the owner Person");
        assert_eq!(principal["name"], "Ada");
        let (status, _) = post_identity(
            root.path(),
            json!({"section":"identity","data":{"name":"Ada"}}),
        )
        .await;
        assert_eq!(status, axum::http::StatusCode::OK);
        assert_eq!(
            solstone_core_entity::load_all_journal_entities(root.path())
                .unwrap()
                .len(),
            1
        );
    }

    #[tokio::test]
    async fn identity_preferred_refuses_a_path_shaped_value() {
        for name in ["~/x", "a/b", "a\\b"] {
            let root = crate::test_support::established_root();
            let (status, body) = post_identity(
                root.path(),
                json!({"section":"identity","data":{"preferred":name}}),
            )
            .await;
            assert_eq!(status, axum::http::StatusCode::BAD_REQUEST, "{name}");
            assert_eq!(body["reason_code"], "invalid_config_value");
            assert_eq!(identity_field(root.path(), "preferred"), None);
        }
    }

    #[tokio::test]
    async fn identity_preferred_accepts_a_benign_value() {
        let root = crate::test_support::established_root();
        let (status, _) = post_identity(
            root.path(),
            json!({"section":"identity","data":{"preferred":"Ada"}}),
        )
        .await;
        assert_eq!(status, axum::http::StatusCode::OK);
        assert_eq!(
            identity_field(root.path(), "preferred").as_deref(),
            Some("Ada")
        );
    }

    #[tokio::test]
    async fn identity_path_shaped_write_is_refused_for_every_request_shape() {
        for body in [
            json!({"section":"identity","data":{"name":"~/x"}}),
            json!({"section":"identity","key":"preferred","value":"a/b"}),
            json!({"identity":{"name":"a\\b"}}),
        ] {
            let root = crate::test_support::established_root();
            let (status, response) = post_identity(root.path(), body.clone()).await;
            assert_eq!(status, axum::http::StatusCode::BAD_REQUEST, "{body}");
            assert_eq!(response["reason_code"], "invalid_config_value");
            assert_eq!(identity_field(root.path(), "name"), None);
            assert_eq!(identity_field(root.path(), "preferred"), None);
        }
    }

    #[tokio::test]
    async fn ac5_tokened_config_masks_token_and_live_responses_never_leak_it() {
        for (phase, expected) in [("tokened", true), ("rich", false)] {
            let root = crate::test_support::phase_root(phase);
            let router = crate::test_support::shell_router(root.path());
            let config = router
                .clone()
                .oneshot(
                    Request::get("/app/settings/api/config")
                        .body(Body::empty())
                        .expect("request"),
                )
                .await
                .expect("response");
            let body: serde_json::Value = serde_json::from_slice(
                &to_bytes(config.into_body(), usize::MAX)
                    .await
                    .expect("body"),
            )
            .expect("JSON");
            assert_eq!(body["env"]["PLAUD_ACCESS_TOKEN"], expected, "{phase}");
            for name in crate::test_support::corpus()["phases"][phase]
                .as_object()
                .expect("phase")
                .keys()
                .filter(|name| name.starts_with("GET "))
            {
                let response = router
                    .clone()
                    .oneshot(
                        Request::get(crate::test_support::request_path(name))
                            .body(Body::empty())
                            .expect("request"),
                    )
                    .await
                    .expect("response");
                let bytes = to_bytes(response.into_body(), usize::MAX)
                    .await
                    .expect("body");
                let text = String::from_utf8_lossy(&bytes);
                assert!(
                    !text.contains("plaud-token-MUST-NOT-LEAK"),
                    "{phase} {name}"
                );
                assert!(!text.contains("MUST-NOT-LEAK"), "{phase} {name}");
            }
        }
    }

    #[tokio::test]
    async fn config_loads_journal_name_projection_omits_it_and_section_journal_write_refused() {
        let root = crate::test_support::phase_root("rich");
        let raw_disk: serde_json::Value = serde_json::from_slice(
            &std::fs::read(root.path().join("config/journal.json")).expect("raw config"),
        )
        .expect("JSON");
        assert_eq!(raw_disk["journal"]["name"], "Analytical Engine");

        let router = crate::test_support::shell_router(root.path());
        let get_response = router
            .clone()
            .oneshot(
                Request::get("/app/settings/api/config")
                    .body(Body::empty())
                    .expect("request"),
            )
            .await
            .expect("response");
        assert_eq!(get_response.status(), axum::http::StatusCode::OK);
        let body: serde_json::Value = serde_json::from_slice(
            &to_bytes(get_response.into_body(), usize::MAX)
                .await
                .expect("body"),
        )
        .expect("JSON");
        assert!(body.get("journal").is_none());

        let put_response = router
            .oneshot(
                Request::put("/app/settings/api/config")
                    .body(Body::from(
                        serde_json::to_vec(&json!({
                            "section": "journal",
                            "data": { "name": "Difference Engine" }
                        }))
                        .expect("body"),
                    ))
                    .expect("request"),
            )
            .await
            .expect("response");
        assert_eq!(put_response.status(), axum::http::StatusCode::BAD_REQUEST);
        let put_body: serde_json::Value = serde_json::from_slice(
            &to_bytes(put_response.into_body(), usize::MAX)
                .await
                .expect("body"),
        )
        .expect("JSON");
        assert_eq!(put_body["reason_code"], "invalid_config_value");
        assert_eq!(put_body["detail"], "Unknown section: journal");

        let after_disk: serde_json::Value = serde_json::from_slice(
            &std::fs::read(root.path().join("config/journal.json")).expect("after config"),
        )
        .expect("JSON");
        assert_eq!(after_disk["journal"]["name"], "Analytical Engine");
    }

    #[test]
    fn processing_validation_messages_preserve_english_and_localize_for_packaged_locales() {
        use solstone_core_convey_http::window_catalog::{ENGLISH, packaged_locale};
        use solstone_core_journal_config::processing::ProcessingValidationError;

        let cases = [
            (
                ProcessingValidationError::NotObject,
                "processing must be an object",
            ),
            (
                ProcessingValidationError::HistoryConfirmedNotBoolean,
                "processing history_confirmed must be a boolean",
            ),
            (
                ProcessingValidationError::InvalidMode,
                "processing mode must be realtime or deferred",
            ),
            (
                ProcessingValidationError::GateNotObject,
                "processing gate must be an object",
            ),
            (
                ProcessingValidationError::ConditionNotObject("time_window"),
                "time_window must be an object",
            ),
            (
                ProcessingValidationError::ConditionEnabledNotBoolean("display_powersave"),
                "display_powersave.enabled must be a boolean",
            ),
            (
                ProcessingValidationError::TimeWindowFieldFormat("start"),
                "time_window.start must be HH:MM",
            ),
        ];
        let private = packaged_locale(Some("en-XA"));
        for (error, expected_english) in cases {
            assert_eq!(
                super::processing_validation_detail(ENGLISH, error),
                expected_english
            );
            if private != ENGLISH {
                assert_ne!(
                    super::processing_validation_detail(private, error),
                    expected_english
                );
            }
        }
    }

    fn credential_seeded_config() -> (
        serde_json::Map<String, serde_json::Value>,
        Vec<&'static str>,
    ) {
        let sentinels = vec![
            "oura-client-id-SECRET",
            "oura-client-secret-SECRET",
            "oura-access-token-SECRET",
            "oura-refresh-token-SECRET",
            "backup-daily-key-SECRET",
            "backup-recovery-key-SECRET",
            "backup-s3-key-id-SECRET",
            "backup-s3-secret-key-SECRET",
            "backup-pending-hmac-SECRET",
            "services-account-id-SECRET",
            "services-fingerprint-SECRET",
            "services-prior-local-cred-SECRET",
            "services-attempt-detail-SECRET",
            "voice-openai-key-SECRET",
            "email-app-password-SECRET",
            "granola-api-key-SECRET",
            "garmin-api-key-SECRET",
            "providers-local-cred-SECRET",
            "env-plaud-token-SECRET",
            "env-openai-key-SECRET",
            "env-anthropic-key-SECRET",
            "env-google-key-SECRET",
            "convey-secret-SECRET",
            "convey-password-hash-SECRET",
            "convey-password-SECRET",
        ];
        let value = json!({
            "setup": { "completed_at": 1_700_000_000_000_i64 },
            "identity": { "name": "Ada Lovelace", "preferred": "Ada", "bio": "mathematician" },
            "some_future_section": { "kept": true },
            "service_key_validation": {
                "plaud": { "valid": true },
                "other": { "valid": false }
            },
            "support": { "portal_url": "https://support.example.org" },
            "oura": {
                "client_id": "oura-client-id-SECRET",
                "client_secret": "oura-client-secret-SECRET",
                "tokens": {
                    "access_token": "oura-access-token-SECRET",
                    "refresh_token": "oura-refresh-token-SECRET",
                    "token_type": "Bearer",
                    "expires_at": 1_800_000_000_i64
                }
            },
            "backup": {
                "enabled": true,
                "daily_key": "backup-daily-key-SECRET",
                "recovery_key": "backup-recovery-key-SECRET",
                "destination": {
                    "kind": "s3",
                    "bucket": "example-bucket",
                    "credentials": {
                        "access_key_id": "backup-s3-key-id-SECRET",
                        "secret_access_key": "backup-s3-secret-key-SECRET"
                    }
                },
                "pending_key_revocation": {
                    "old_key_hmac": "backup-pending-hmac-SECRET"
                }
            },
            "services": {
                "confidential": {
                    "account_id": "services-account-id-SECRET",
                    "endpoint_url": "https://confidential.example.org",
                    "credential_fingerprint_sha256": "services-fingerprint-SECRET",
                    "prior_local_endpoint": {
                        "url": "http://127.0.0.1:8080/v1",
                        "credential": "services-prior-local-cred-SECRET"
                    }
                },
                "confidential_attempt": {
                    "detail": "services-attempt-detail-SECRET"
                }
            },
            "voice": {
                "openai_api_key": "voice-openai-key-SECRET",
                "model": "gpt-realtime",
                "brain_model": "haiku"
            },
            "granola": {
                "api_key": "granola-api-key-SECRET"
            },
            "email": {
                "provider": "gmail",
                "account": "ada@example.com",
                "app_password": "email-app-password-SECRET"
            },
            "garmin": {
                "api_key": "garmin-api-key-SECRET",
                "key_expires_on": "2027-02-22"
            },
            "providers": {
                "local": {
                    "url": "http://127.0.0.1:8080/v1",
                    "credential": "providers-local-cred-SECRET"
                }
            },
            "env": {
                "PLAUD_ACCESS_TOKEN": "env-plaud-token-SECRET",
                "OPENAI_API_KEY": "env-openai-key-SECRET",
                "ANTHROPIC_API_KEY": "env-anthropic-key-SECRET",
                "GOOGLE_API_KEY": "env-google-key-SECRET"
            },
            "convey": {
                "bind": "127.0.0.1",
                "secret": "convey-secret-SECRET",
                "password_hash": "convey-password-hash-SECRET",
                "password": "convey-password-SECRET"
            }
        });
        (value.as_object().expect("config object").clone(), sentinels)
    }

    #[test]
    fn project_public_config_strips_all_credential_bearing_sections() {
        let (raw, sentinels) = credential_seeded_config();
        let projected = super::project_public_config(raw).expect("projection succeeds");
        let serialized = serde_json::to_string(&projected).expect("JSON");

        for sentinel in &sentinels {
            assert!(
                !serialized.contains(sentinel),
                "projected config leaked {sentinel}"
            );
        }
        assert!(!serialized.contains("SECRET"));
        for section in [
            "oura",
            "backup",
            "services",
            "voice",
            "email",
            "granola",
            "garmin",
            "providers",
            "support",
            "service_key_validation",
        ] {
            assert!(
                projected.get(section).is_none(),
                "projected config retained redacted section {section}"
            );
        }
        assert_eq!(projected["env"], json!({"PLAUD_ACCESS_TOKEN": true}));
        assert_eq!(
            projected["key_validation"],
            json!({"plaud": {"valid": true}})
        );
        assert_eq!(projected["convey"], json!({"bind": "127.0.0.1"}));
        assert_eq!(projected["identity"]["name"], "Ada Lovelace");
        assert_eq!(projected["some_future_section"], json!({"kept": true}));

        // Malformed (non-object) shapes in redacted sections must still fail closed.
        let malformed = json!({
            "oura": "malformed-oura-SECRET",
            "email": "malformed-email-SECRET",
            "garmin": "malformed-garmin-SECRET",
            "backup": ["malformed-backup-SECRET"],
            "services": "malformed-services-SECRET",
            "voice": ["malformed-voice-SECRET"],
            "identity": { "name": "Ada" }
        })
        .as_object()
        .expect("object")
        .clone();
        let projected_malformed =
            super::project_public_config(malformed).expect("malformed projection succeeds");
        let malformed_text = serde_json::to_string(&projected_malformed).expect("JSON");
        assert!(!malformed_text.contains("SECRET"));
        for section in ["oura", "email", "garmin", "backup", "services", "voice"] {
            assert!(projected_malformed.get(section).is_none());
        }
    }

    #[tokio::test]
    async fn settings_api_config_redacts_secrets_across_get_put_and_post_without_clobbering_disk() {
        let root = crate::test_support::established_root();
        let (seeded, sentinels) = credential_seeded_config();
        let config_path = root.path().join("config/journal.json");
        std::fs::write(
            &config_path,
            serde_json::to_vec(&seeded).expect("seeded JSON"),
        )
        .expect("write seeded config");

        let router = crate::test_support::shell_router(root.path());

        let get_response = router
            .clone()
            .oneshot(
                Request::get("/app/settings/api/config")
                    .body(Body::empty())
                    .expect("GET request"),
            )
            .await
            .expect("GET response");
        assert_eq!(get_response.status(), axum::http::StatusCode::OK);
        let get_bytes = to_bytes(get_response.into_body(), usize::MAX)
            .await
            .expect("GET body");
        let get_text = String::from_utf8_lossy(&get_bytes);
        let get_json: serde_json::Value = serde_json::from_slice(&get_bytes).expect("GET JSON");

        let put_response = router
            .clone()
            .oneshot(
                Request::put("/app/settings/api/config")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        json!({"section": "identity", "data": {"preferred": "Augusta"}})
                            .to_string(),
                    ))
                    .expect("PUT request"),
            )
            .await
            .expect("PUT response");
        assert_eq!(put_response.status(), axum::http::StatusCode::OK);
        let put_bytes = to_bytes(put_response.into_body(), usize::MAX)
            .await
            .expect("PUT body");
        let put_text = String::from_utf8_lossy(&put_bytes);
        let put_json: serde_json::Value = serde_json::from_slice(&put_bytes).expect("PUT JSON");
        assert_eq!(put_json["config"]["identity"]["preferred"], "Augusta");

        let post_response = router
            .clone()
            .oneshot(
                Request::post("/app/settings/api/config")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        json!({"section": "identity", "data": {"bio": "analyst"}}).to_string(),
                    ))
                    .expect("POST request"),
            )
            .await
            .expect("POST response");
        assert_eq!(post_response.status(), axum::http::StatusCode::OK);
        let post_bytes = to_bytes(post_response.into_body(), usize::MAX)
            .await
            .expect("POST body");
        let post_text = String::from_utf8_lossy(&post_bytes);
        let post_json: serde_json::Value = serde_json::from_slice(&post_bytes).expect("POST JSON");
        assert_eq!(post_json["config"]["identity"]["bio"], "analyst");

        for (label, text, config_obj) in [
            ("GET", get_text.as_ref(), &get_json),
            ("PUT", put_text.as_ref(), &put_json["config"]),
            ("POST", post_text.as_ref(), &post_json["config"]),
        ] {
            for sentinel in &sentinels {
                assert!(!text.contains(sentinel), "{label} leaked {sentinel}");
            }
            assert!(!text.contains("SECRET"), "{label} leaked a secret sentinel");
            for section in [
                "oura",
                "backup",
                "services",
                "voice",
                "email",
                "granola",
                "garmin",
                "providers",
            ] {
                assert!(
                    config_obj.get(section).is_none(),
                    "{label} returned redacted section {section}"
                );
            }
        }

        // Direct writes to redacted sections remain refused and leak nothing.
        for section in ["oura", "email", "garmin", "backup", "services", "voice"] {
            let refused = router
                .clone()
                .oneshot(
                    Request::put("/app/settings/api/config")
                        .header("content-type", "application/json")
                        .body(Body::from(
                            json!({"section": section, "data": {"overwrite": true}}).to_string(),
                        ))
                        .expect("refused PUT"),
                )
                .await
                .expect("refused response");
            assert_eq!(
                refused.status(),
                axum::http::StatusCode::BAD_REQUEST,
                "PUT {section} must be refused"
            );
            let refused_bytes = to_bytes(refused.into_body(), usize::MAX)
                .await
                .expect("refused body");
            let refused_text = String::from_utf8_lossy(&refused_bytes);
            assert!(!refused_text.contains("SECRET"));
        }

        // Stored credentials on disk remain intact after GET/PUT/POST.
        let on_disk: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&config_path).expect("disk config"))
                .expect("disk JSON");
        for section in [
            "oura",
            "backup",
            "services",
            "voice",
            "email",
            "granola",
            "garmin",
            "providers",
        ] {
            assert_eq!(
                on_disk.get(section),
                seeded.get(section),
                "disk section {section} was altered"
            );
        }
        assert_eq!(on_disk["identity"]["preferred"], "Augusta");
        assert_eq!(on_disk["identity"]["bio"], "analyst");
    }
}
