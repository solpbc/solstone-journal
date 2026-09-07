// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::fmt;

pub const SOURCE_APPLE_HEALTH: &str = "apple_health";
pub const SOURCE_OURA: &str = "oura";
pub const SOURCE_OURA_API: &str = "oura_api";
pub const SOURCE_DEXCOM_CLARITY: &str = "dexcom_clarity";

pub const HEALTH_CARD_STREAM_BY_FAMILY: [(&str, Option<&str>); 4] = [
    (SOURCE_APPLE_HEALTH, Some("import.apple_health")),
    (SOURCE_OURA_API, Some("import.oura")),
    (SOURCE_OURA, None),
    (SOURCE_DEXCOM_CLARITY, None),
];

pub const FRIENDLY_TYPE_NAMES: [(&str, &str); 51] = [
    ("HKQuantityTypeIdentifierBloodGlucose", "glucose"),
    ("HKQuantityTypeIdentifierHeartRate", "heart rate"),
    (
        "HKQuantityTypeIdentifierHeartRateVariabilitySDNN",
        "heart rate variability",
    ),
    (
        "HKQuantityTypeIdentifierRestingHeartRate",
        "resting heart rate",
    ),
    (
        "HKQuantityTypeIdentifierWalkingHeartRateAverage",
        "walking heart rate average",
    ),
    (
        "HKQuantityTypeIdentifierHeartRateRecoveryOneMinute",
        "heart rate recovery",
    ),
    ("HKQuantityTypeIdentifierOxygenSaturation", "blood oxygen"),
    (
        "HKQuantityTypeIdentifierRespiratoryRate",
        "respiratory rate",
    ),
    (
        "HKQuantityTypeIdentifierBloodPressureSystolic",
        "blood pressure (systolic)",
    ),
    (
        "HKQuantityTypeIdentifierBloodPressureDiastolic",
        "blood pressure (diastolic)",
    ),
    (
        "HKCategoryTypeIdentifierIrregularHeartRhythmEvent",
        "irregular rhythm notification",
    ),
    (
        "HKCategoryTypeIdentifierHighHeartRateEvent",
        "high heart-rate notification",
    ),
    (
        "HKCategoryTypeIdentifierLowHeartRateEvent",
        "low heart-rate notification",
    ),
    (
        "HKQuantityTypeIdentifierAtrialFibrillationBurden",
        "AFib burden",
    ),
    ("HKQuantityTypeIdentifierVO2Max", "VO2 max"),
    ("HKQuantityTypeIdentifierStepCount", "step count"),
    (
        "HKQuantityTypeIdentifierActiveEnergyBurned",
        "active energy",
    ),
    (
        "HKQuantityTypeIdentifierBasalEnergyBurned",
        "resting energy",
    ),
    (
        "HKQuantityTypeIdentifierDistanceWalkingRunning",
        "walking + running distance",
    ),
    (
        "HKQuantityTypeIdentifierDistanceCycling",
        "cycling distance",
    ),
    ("HKQuantityTypeIdentifierFlightsClimbed", "flights climbed"),
    (
        "HKQuantityTypeIdentifierAppleExerciseTime",
        "exercise minutes",
    ),
    ("HKQuantityTypeIdentifierAppleStandTime", "stand time"),
    ("HKCategoryTypeIdentifierAppleStandHour", "stand hours"),
    ("HKQuantityTypeIdentifierPhysicalEffort", "physical effort"),
    ("HKCategoryTypeIdentifierSleepAnalysis", "sleep"),
    (
        "HKQuantityTypeIdentifierAppleSleepingWristTemperature",
        "wrist temperature",
    ),
    ("HKCategoryTypeIdentifierMindfulSession", "mindful sessions"),
    (
        "HKQuantityTypeIdentifierHeadphoneAudioExposure",
        "headphone audio level",
    ),
    (
        "HKQuantityTypeIdentifierEnvironmentalAudioExposure",
        "environmental audio level",
    ),
    ("HKQuantityTypeIdentifierTimeInDaylight", "time in daylight"),
    ("HKQuantityTypeIdentifierBodyMass", "body mass"),
    ("HKQuantityTypeIdentifierBodyMassIndex", "body mass index"),
    ("HKQuantityTypeIdentifierBodyFatPercentage", "body fat"),
    ("HKQuantityTypeIdentifierLeanBodyMass", "lean body mass"),
    ("HKQuantityTypeIdentifierHeight", "height"),
    ("oura.daily_sleep", "sleep score"),
    ("oura.daily_readiness", "readiness"),
    ("oura.daily_resilience", "resilience"),
    ("oura.daily_stress", "daytime stress"),
    ("oura.daily_spo2", "nightly blood oxygen"),
    ("oura.temperature_deviation", "temperature deviation"),
    ("oura.sleep", "sleep period"),
    ("oura.daily_activity", "daily activity"),
    ("oura.heartrate", "heart rate"),
    ("oura.daily_cardiovascular_age", "cardiovascular age"),
    ("oura.blood_glucose", "blood glucose"),
    ("oura.workout", "workout"),
    ("oura.session", "session"),
    ("oura.enhanced_tag", "tag"),
    ("oura.vo2_max", "VO2 max"),
];

pub const FRIENDLY_CONTRIBUTOR_NAMES: [(&str, &str); 16] = [
    ("activity_balance", "activity balance"),
    ("body_temperature", "body temperature"),
    ("hrv_balance", "HRV balance"),
    ("previous_day_activity", "previous day activity"),
    ("previous_night", "previous night"),
    ("recovery_index", "recovery index"),
    ("resting_heart_rate", "resting heart rate"),
    ("sleep_balance", "sleep balance"),
    ("sleep_regularity", "sleep regularity"),
    ("deep_sleep", "deep sleep"),
    ("efficiency", "efficiency"),
    ("latency", "latency"),
    ("rem_sleep", "REM sleep"),
    ("restfulness", "restfulness"),
    ("timing", "timing"),
    ("total_sleep", "total sleep"),
];

const FRACTION_PERCENT_FRAGMENTS: [&str; 6] = [
    "OxygenSaturation",
    "AppleWalkingSteadiness",
    "WalkingAsymmetryPercentage",
    "WalkingDoubleSupportPercentage",
    "BodyFatPercentage",
    "AtrialFibrillationBurden",
];
const FRIENDLY_UNIT_LABELS: [(&str, &str); 6] = [
    ("dBASPL", "dB"),
    ("kcal", "Cal"),
    ("mi/hr", "mph"),
    ("km/hr", "km/h"),
    ("score", ""),
    ("degC", "°C"),
];

#[derive(Debug)]
pub enum HealthCardStreamError {
    UnknownFamily { family: String },
    NoCardStream { family: String },
}

impl fmt::Display for HealthCardStreamError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnknownFamily { family } => {
                write!(formatter, "Unknown health source family: {family:?}")
            }
            Self::NoCardStream { family } => write!(
                formatter,
                "Health source family {family:?} does not declare a chronicle card stream"
            ),
        }
    }
}
impl std::error::Error for HealthCardStreamError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::UnknownFamily { .. } | Self::NoCardStream { .. } => None,
        }
    }
}

pub fn health_card_stream(family: &str) -> Result<&'static str, HealthCardStreamError> {
    match HEALTH_CARD_STREAM_BY_FAMILY
        .iter()
        .find(|(known, _)| *known == family)
    {
        None => Err(HealthCardStreamError::UnknownFamily {
            family: family.to_owned(),
        }),
        Some((_, Some(stream))) => Ok(stream),
        Some((_, None)) => Err(HealthCardStreamError::NoCardStream {
            family: family.to_owned(),
        }),
    }
}

pub fn friendly_contributor_name(key: &str) -> String {
    if let Some((_, name)) = FRIENDLY_CONTRIBUTOR_NAMES
        .iter()
        .find(|(known, _)| *known == key)
    {
        return (*name).to_owned();
    }
    let words = key.replace('_', " ");
    let words = words.trim();
    if words.is_empty() {
        return key.to_owned();
    }
    words.to_owned()
}

pub fn friendly_type_name(record_type: &str) -> String {
    if let Some((_, name)) = FRIENDLY_TYPE_NAMES
        .iter()
        .find(|(known, _)| *known == record_type)
    {
        return (*name).to_owned();
    }
    let stripped = [
        "HKQuantityTypeIdentifier",
        "HKCategoryTypeIdentifier",
        "HKDataType",
        "HKWorkoutActivityType",
    ]
    .into_iter()
    .find_map(|prefix| record_type.strip_prefix(prefix))
    .unwrap_or(record_type);
    if stripped.is_empty() {
        return record_type.to_owned();
    }
    let chars = stripped.chars().collect::<Vec<_>>();
    let mut words = String::new();
    for (index, character) in chars.iter().enumerate() {
        if index > 0 && character.is_uppercase() {
            let previous = chars[index - 1];
            let next_is_lower = chars.get(index + 1).is_some_and(|next| next.is_lowercase());
            if previous.is_lowercase()
                || previous.is_ascii_digit()
                || (previous.is_uppercase() && next_is_lower)
            {
                words.push(' ');
            }
        }
        words.push(*character);
    }
    let lowered = words.to_lowercase();
    if lowered.is_empty() {
        return record_type.to_owned();
    }
    lowered
}

pub fn friendly_unit_label(record_type: &str, unit: Option<&str>) -> Option<String> {
    let unit = unit?;
    if unit == "count/min" {
        if record_type.contains("RespiratoryRate") {
            return Some("breaths/min".to_owned());
        }
        if record_type.contains("HeartRate") {
            return Some("bpm".to_owned());
        }
    }
    if let Some((_, label)) = FRIENDLY_UNIT_LABELS
        .iter()
        .find(|(known, _)| *known == unit)
    {
        return Some((*label).to_owned());
    }
    if unit == "count" {
        return Some(String::new());
    }
    Some(unit.to_owned())
}

fn format_quantity(value: f64) -> String {
    if value == value.trunc() {
        return group_integer(value as i64);
    }
    let rendered = format!("{value:.1}");
    let (whole, fraction) = rendered
        .split_once('.')
        .expect("fixed decimal has decimal point");
    format!(
        "{}.{}",
        group_integer(whole.parse().expect("formatted integer parses")),
        fraction
    )
}

fn group_integer(value: i64) -> String {
    let rendered = value.to_string();
    let (sign, digits) = rendered
        .strip_prefix('-')
        .map_or(("", rendered.as_str()), |digits| ("-", digits));
    let mut grouped = String::new();
    for (index, character) in digits.chars().enumerate() {
        if index > 0 && (digits.len() - index).is_multiple_of(3) {
            grouped.push(',');
        }
        grouped.push(character);
    }
    format!("{sign}{grouped}")
}

pub fn display_number(record_type: &str, value: f64, unit: Option<&str>) -> String {
    let scaled = if unit == Some("%")
        && FRACTION_PERCENT_FRAGMENTS
            .iter()
            .any(|fragment| record_type.contains(fragment))
    {
        (value * 100.0 * 1_000_000.0).round() / 1_000_000.0
    } else {
        value
    };
    format_quantity(scaled)
}

pub fn display_value(record_type: &str, value: f64, unit: Option<&str>) -> String {
    let number = display_number(record_type, value, unit);
    match friendly_unit_label(record_type, unit).as_deref() {
        None | Some("") => number,
        Some("%") => format!("{number}%"),
        Some(label) => format!("{number} {label}"),
    }
}

#[cfg(all(test, not(feature = "full-tests")))]
mod tests {
    use super::*;

    #[test]
    fn presentation_helpers_match_health_schema_rules() {
        assert_eq!(
            friendly_type_name("HKQuantityTypeIdentifierHeartRate"),
            "heart rate"
        );
        assert_eq!(
            friendly_type_name("HKQuantityTypeIdentifierSyntheticVO2Max"),
            "synthetic vo2 max"
        );
        assert_eq!(
            friendly_unit_label("x", Some("dBASPL")),
            Some("dB".to_owned())
        );
        assert_eq!(
            friendly_unit_label("HKQuantityTypeIdentifierHeartRate", Some("count/min")),
            Some("bpm".to_owned())
        );
        assert_eq!(
            friendly_unit_label("HKQuantityTypeIdentifierRespiratoryRate", Some("count/min")),
            Some("breaths/min".to_owned())
        );
        assert_eq!(
            friendly_unit_label("HKQuantityTypeIdentifierHeartRate", None),
            None
        );
        assert_eq!(friendly_unit_label("x", Some("count")), Some(String::new()));
        assert_eq!(
            friendly_unit_label("x", Some("furlong")),
            Some("furlong".to_owned())
        );
        assert_eq!(
            display_number("HKQuantityTypeIdentifierOxygenSaturation", 0.98, Some("%")),
            "98"
        );
        assert_eq!(display_number("SyntheticPercent", 0.98, Some("%")), "1.0");
        assert_eq!(
            display_value("HKQuantityTypeIdentifierStepCount", 6412.0, Some("count")),
            "6,412"
        );
    }

    #[test]
    fn heart_rate_unit_paths_have_identical_display_label() {
        assert_eq!(
            friendly_unit_label("HKQuantityTypeIdentifierHeartRate", Some("count/min")),
            friendly_unit_label("oura.heartrate", Some("bpm"))
        );
    }
}
