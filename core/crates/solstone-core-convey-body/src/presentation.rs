// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

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

pub(crate) fn friendly_unit_label(record_type: &str, unit: Option<&str>) -> Option<String> {
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

pub(crate) fn display_number(record_type: &str, value: f64, unit: Option<&str>) -> String {
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

pub(crate) fn display_value(record_type: &str, value: f64, unit: Option<&str>) -> String {
    let number = display_number(record_type, value, unit);
    match friendly_unit_label(record_type, unit).as_deref() {
        None | Some("") => number,
        Some("%") => format!("{number}%"),
        Some(label) => format!("{number} {label}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn units_and_values_read_as_a_person_says_them() {
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
        assert_eq!(
            display_value("HKQuantityTypeIdentifierStepCount", 6412.0, Some("count")),
            "6,412"
        );
    }
}
