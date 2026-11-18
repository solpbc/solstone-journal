// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Deferred analysis settings and pure gate evaluation.
use jiff::civil::Time;
use serde_json::{Map, Value};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TimeWindowSettings {
    pub enabled: bool,
    pub start: Time,
    pub end: Time,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DisplayPowersaveSettings {
    pub enabled: bool,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GateSettings {
    pub time_window: TimeWindowSettings,
    pub display_powersave: DisplayPowersaveSettings,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProcessingSettings {
    pub deferred: bool,
    pub gate: GateSettings,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DisplayPowersaveReading {
    pub available: bool,
    pub asleep: bool,
    pub debounced: bool,
}
impl DisplayPowersaveReading {
    pub const UNAVAILABLE: Self = Self {
        available: false,
        asleep: false,
        debounced: false,
    };
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ConditionState {
    pub enabled: bool,
    pub available: bool,
    pub open: bool,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GateState {
    pub open: bool,
    pub time_window: ConditionState,
    pub display_powersave: ConditionState,
}

pub fn evaluate_time_window(settings: TimeWindowSettings, now: Time) -> ConditionState {
    let open = if settings.start < settings.end {
        now >= settings.start && now < settings.end
    } else if settings.start > settings.end {
        now >= settings.start || now < settings.end
    } else {
        false
    };
    ConditionState {
        enabled: settings.enabled,
        available: true,
        open,
    }
}
pub fn evaluate_display_powersave(
    settings: DisplayPowersaveSettings,
    reading: DisplayPowersaveReading,
) -> ConditionState {
    if !settings.enabled {
        return ConditionState {
            enabled: false,
            available: false,
            open: false,
        };
    }
    ConditionState {
        enabled: true,
        available: reading.available,
        open: reading.asleep && reading.debounced,
    }
}
pub fn evaluate_drain_gate(
    settings: ProcessingSettings,
    now: Time,
    reading: DisplayPowersaveReading,
) -> GateState {
    let time_window = evaluate_time_window(settings.gate.time_window, now);
    let display_powersave = evaluate_display_powersave(settings.gate.display_powersave, reading);
    GateState {
        open: [time_window, display_powersave]
            .into_iter()
            .any(|condition| condition.enabled && condition.available && condition.open),
        time_window,
        display_powersave,
    }
}
pub fn derive_drain_state(
    settings: ProcessingSettings,
    gate: &GateState,
    no_engine: bool,
) -> &'static str {
    if no_engine {
        "no_engine"
    } else if !settings.deferred {
        "realtime"
    } else if gate.open {
        "window_open"
    } else if [gate.time_window, gate.display_powersave]
        .into_iter()
        .any(|condition| condition.enabled && condition.available)
    {
        "waiting_for_window"
    } else {
        "no_active_condition"
    }
}

pub fn processing_settings(config: &Map<String, Value>) -> Result<ProcessingSettings, String> {
    validate_processing(config.get("processing"))?;
    let processing = config.get("processing").and_then(Value::as_object);
    let gate = processing
        .and_then(|value| value.get("gate"))
        .and_then(Value::as_object);
    let window = gate
        .and_then(|value| value.get("time_window"))
        .and_then(Value::as_object);
    let display = gate
        .and_then(|value| value.get("display_powersave"))
        .and_then(Value::as_object);
    Ok(ProcessingSettings {
        deferred: processing
            .and_then(|value| value.get("mode"))
            .and_then(Value::as_str)
            == Some("deferred"),
        gate: GateSettings {
            time_window: TimeWindowSettings {
                enabled: window
                    .and_then(|value| value.get("enabled"))
                    .and_then(Value::as_bool)
                    .unwrap_or(true),
                start: parse_time(
                    window
                        .and_then(|value| value.get("start"))
                        .and_then(Value::as_str),
                )
                .unwrap_or_else(|| Time::constant(2, 0, 0, 0)),
                end: parse_time(
                    window
                        .and_then(|value| value.get("end"))
                        .and_then(Value::as_str),
                )
                .unwrap_or_else(|| Time::constant(6, 0, 0, 0)),
            },
            display_powersave: DisplayPowersaveSettings {
                enabled: display
                    .and_then(|value| value.get("enabled"))
                    .and_then(Value::as_bool)
                    .unwrap_or(false),
            },
        },
    })
}

fn parse_time(value: Option<&str>) -> Option<Time> {
    value.and_then(|value| Time::strptime("%H:%M", value).ok())
}

/// Reject malformed supplied fields, while absent fields retain their defaults.
pub fn validate_processing(value: Option<&Value>) -> Result<(), String> {
    let Some(value) = value else { return Ok(()) };
    let obj = value.as_object().ok_or("processing must be an object")?;
    if obj
        .get("history_confirmed")
        .is_some_and(|value| !value.is_boolean())
    {
        return Err("processing history_confirmed must be a boolean".into());
    }
    if let Some(mode) = obj.get("mode")
        && !matches!(mode.as_str(), Some("realtime" | "deferred"))
    {
        return Err("processing mode must be realtime or deferred".into());
    }
    if let Some(gate) = obj.get("gate") {
        let gate = gate
            .as_object()
            .ok_or("processing gate must be an object")?;
        for name in ["time_window", "display_powersave"] {
            if let Some(condition) = gate.get(name) {
                let condition = condition
                    .as_object()
                    .ok_or_else(|| format!("{name} must be an object"))?;
                if condition
                    .get("enabled")
                    .is_some_and(|value| !value.is_boolean())
                {
                    return Err(format!("{name}.enabled must be a boolean"));
                }
                if name == "time_window" {
                    for key in ["start", "end"] {
                        if let Some(value) = condition.get(key)
                            && (value.as_str().is_none_or(|s| s.len() != 5)
                                || parse_time(value.as_str()).is_none())
                        {
                            return Err(format!("time_window.{key} must be HH:MM"));
                        }
                    }
                }
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn processing_windows_are_half_open_wrap_midnight_and_equal_means_closed() {
        for (start, end, samples) in [
            (
                "02:00",
                "06:00",
                [
                    ("01:59", false),
                    ("02:00", true),
                    ("05:59", true),
                    ("06:00", false),
                ],
            ),
            (
                "22:00",
                "02:00",
                [
                    ("21:59", false),
                    ("22:00", true),
                    ("01:59", true),
                    ("02:00", false),
                ],
            ),
            (
                "02:00",
                "02:00",
                [
                    ("00:00", false),
                    ("02:00", false),
                    ("12:00", false),
                    ("23:59", false),
                ],
            ),
        ] {
            let settings = TimeWindowSettings {
                enabled: true,
                start: parse_time(Some(start)).unwrap(),
                end: parse_time(Some(end)).unwrap(),
            };
            for (at, expected) in samples {
                assert_eq!(
                    evaluate_time_window(settings, parse_time(Some(at)).unwrap()).open,
                    expected,
                    "{start}–{end} at {at}"
                );
            }
        }
    }

    #[test]
    fn processing_gate_is_or_and_unavailable_or_disabled_never_opens_it() {
        let mut settings = processing_settings(&Map::new()).unwrap();
        settings.deferred = true;
        settings.gate.display_powersave.enabled = true;
        let noon = parse_time(Some("12:00")).unwrap();
        let asleep = DisplayPowersaveReading {
            available: true,
            asleep: true,
            debounced: true,
        };
        assert!(evaluate_drain_gate(settings, noon, asleep).open);
        assert!(
            !evaluate_drain_gate(
                settings,
                noon,
                DisplayPowersaveReading {
                    debounced: false,
                    ..asleep
                }
            )
            .open
        );
        assert!(
            !evaluate_drain_gate(
                settings,
                noon,
                DisplayPowersaveReading {
                    available: false,
                    ..asleep
                }
            )
            .open
        );
        assert!(
            evaluate_drain_gate(
                settings,
                parse_time(Some("03:00")).unwrap(),
                DisplayPowersaveReading::UNAVAILABLE
            )
            .open
        );
        settings.gate.time_window.enabled = false;
        settings.gate.display_powersave.enabled = false;
        let gate = evaluate_drain_gate(settings, noon, asleep);
        assert!(!gate.open);
        assert_eq!(
            derive_drain_state(settings, &gate, false),
            "no_active_condition"
        );
        settings.deferred = false;
        assert_eq!(derive_drain_state(settings, &gate, false), "realtime");
    }

    #[test]
    fn processing_settings_refuse_invalid_fields_but_keep_partial_defaults() {
        for value in [
            json!({"mode":"later"}),
            json!({"mode":true}),
            json!({"gate":null}),
            json!({"gate":{"time_window":{"enabled":1}}}),
            json!({"gate":{"time_window":{"start":"25:00"}}}),
            json!({"gate":{"time_window":{"end":"2:00"}}}),
        ] {
            assert!(validate_processing(Some(&value)).is_err(), "{value}");
        }
        // The retired history rest is ignored wherever an older config left it.
        let retired = json!({"confidential_history_rest_seconds":"30"});
        assert!(validate_processing(Some(&retired)).is_ok());
        let config =
            json!({"processing":{"mode":"deferred","gate":{"time_window":{"start":"23:00"}}}});
        let settings = processing_settings(config.as_object().unwrap()).unwrap();
        assert!(settings.deferred);
        assert_eq!(
            settings.gate.time_window.start,
            parse_time(Some("23:00")).unwrap()
        );
        assert_eq!(
            settings.gate.time_window.end,
            parse_time(Some("06:00")).unwrap()
        );
    }
}
