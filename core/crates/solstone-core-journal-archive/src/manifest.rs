// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use zip::DateTime;

use crate::writer::ArchiveEncodingError;

/// Plain values rendered into a portable archive manifest.
pub(crate) struct ManifestFields<'a> {
    pub(crate) solstone_version: &'a str,
    pub(crate) exported_at: &'a str,
    pub(crate) source_journal: &'a str,
    pub(crate) day_count: usize,
    pub(crate) entity_count: usize,
    pub(crate) facet_count: usize,
}

/// A validated manifest ready to write into a portable archive.
pub(crate) struct Manifest {
    pub(crate) json: Vec<u8>,
    pub(crate) timestamp: DateTime,
}

/// Validate plain manifest fields and render their fixed JSON representation.
pub(crate) fn build(fields: ManifestFields<'_>) -> Result<Manifest, ArchiveEncodingError> {
    validate_version(fields.solstone_version)?;
    let timestamp = parse_exported_at(fields.exported_at)?;
    Ok(Manifest {
        json: render_json(&fields).into_bytes(),
        timestamp,
    })
}

fn validate_version(value: &str) -> Result<(), ArchiveEncodingError> {
    if value.is_empty() {
        return Err(invalid_metadata(
            "solstone_version",
            value,
            "must not be empty",
        ));
    }
    if value.chars().any(|character| character.is_ascii_control()) {
        return Err(invalid_metadata(
            "solstone_version",
            value,
            "must not contain ASCII control characters",
        ));
    }
    Ok(())
}

fn parse_exported_at(value: &str) -> Result<DateTime, ArchiveEncodingError> {
    let bytes = value.as_bytes();
    if bytes.len() != 20
        || bytes[4] != b'-'
        || bytes[7] != b'-'
        || bytes[10] != b'T'
        || bytes[13] != b':'
        || bytes[16] != b':'
        || bytes[19] != b'Z'
    {
        return Err(invalid_metadata(
            "exported_at",
            value,
            "must use YYYY-MM-DDTHH:MM:SSZ",
        ));
    }

    let year = parse_digits(&bytes[0..4], value)?;
    let month = parse_digits(&bytes[5..7], value)?;
    let day = parse_digits(&bytes[8..10], value)?;
    let hour = parse_digits(&bytes[11..13], value)?;
    let minute = parse_digits(&bytes[14..16], value)?;
    let second = parse_digits(&bytes[17..19], value)?;
    if !(1980..=2107).contains(&year) {
        return Err(invalid_metadata(
            "exported_at",
            value,
            "year must be in 1980..=2107",
        ));
    }

    DateTime::from_date_and_time(
        year,
        u8::try_from(month).map_err(|_| invalid_metadata("exported_at", value, "invalid month"))?,
        u8::try_from(day).map_err(|_| invalid_metadata("exported_at", value, "invalid day"))?,
        u8::try_from(hour).map_err(|_| invalid_metadata("exported_at", value, "invalid hour"))?,
        u8::try_from(minute)
            .map_err(|_| invalid_metadata("exported_at", value, "invalid minute"))?,
        u8::try_from(second)
            .map_err(|_| invalid_metadata("exported_at", value, "invalid second"))?,
    )
    .map_err(|_| invalid_metadata("exported_at", value, "invalid calendar date or time"))
}

fn parse_digits(bytes: &[u8], value: &str) -> Result<u16, ArchiveEncodingError> {
    let mut parsed = 0_u16;
    for byte in bytes {
        if !byte.is_ascii_digit() {
            return Err(invalid_metadata(
                "exported_at",
                value,
                "must contain digits in numeric positions",
            ));
        }
        parsed = parsed * 10 + u16::from(*byte - b'0');
    }
    Ok(parsed)
}

fn invalid_metadata(
    field: &'static str,
    value: &str,
    reason: &'static str,
) -> ArchiveEncodingError {
    ArchiveEncodingError::InvalidMetadata {
        field,
        value: value.to_owned(),
        reason,
    }
}

fn render_json(fields: &ManifestFields<'_>) -> String {
    let mut json = String::from("{\n  \"solstone_version\": ");
    push_json_string(&mut json, fields.solstone_version);
    json.push_str(",\n  \"exported_at\": ");
    push_json_string(&mut json, fields.exported_at);
    json.push_str(",\n  \"source_journal\": ");
    push_json_string(&mut json, fields.source_journal);
    json.push_str(",\n  \"day_count\": ");
    json.push_str(&fields.day_count.to_string());
    json.push_str(",\n  \"entity_count\": ");
    json.push_str(&fields.entity_count.to_string());
    json.push_str(",\n  \"facet_count\": ");
    json.push_str(&fields.facet_count.to_string());
    json.push_str("\n}");
    json
}

fn push_json_string(output: &mut String, value: &str) {
    output.push('"');
    for character in value.chars() {
        match character {
            '"' => output.push_str("\\\""),
            '\\' => output.push_str("\\\\"),
            '\u{0008}' => output.push_str("\\b"),
            '\u{000C}' => output.push_str("\\f"),
            '\n' => output.push_str("\\n"),
            '\r' => output.push_str("\\r"),
            '\t' => output.push_str("\\t"),
            character if character <= '\u{001F}' => {
                let escaped = format!("\\u{:04x}", character as u32);
                output.push_str(&escaped);
            }
            character => output.push(character),
        }
    }
    output.push('"');
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fields<'a>(exported_at: &'a str, source_journal: &'a str) -> ManifestFields<'a> {
        ManifestFields {
            solstone_version: "0.9.0",
            exported_at,
            source_journal,
            day_count: 3,
            entity_count: 4,
            facet_count: 5,
        }
    }

    #[test]
    fn renders_fixed_order_json_with_literal_utf8() {
        let manifest = build(fields("2026-08-07T21:22:23Z", "/journål/\"slash\\\u{0001}"))
            .expect("build manifest");
        let expected = "{\n  \"solstone_version\": \"0.9.0\",\n  \"exported_at\": \"2026-08-07T21:22:23Z\",\n  \"source_journal\": \"/journål/\\\"slash\\\\\\u0001\",\n  \"day_count\": 3,\n  \"entity_count\": 4,\n  \"facet_count\": 5\n}";
        assert_eq!(manifest.json, expected.as_bytes());
    }

    #[test]
    fn odd_seconds_truncate_in_dos_time() {
        let manifest = build(fields("2026-08-07T21:22:23Z", "/journal")).expect("build manifest");
        assert_eq!(manifest.timestamp.second(), 22);
    }

    #[test]
    fn boundary_years_are_accepted() {
        for timestamp in ["1980-01-01T00:00:00Z", "2107-12-31T23:59:58Z"] {
            assert!(build(fields(timestamp, "/journal")).is_ok());
        }
    }

    #[test]
    fn invalid_timestamp_forms_are_rejected() {
        for timestamp in [
            "1979-12-31T23:59:58Z",
            "2108-01-01T00:00:00Z",
            "2026-08-07T21:22:23.1Z",
            "2026-08-07T21:22:23+00:00",
            "2026-08-07T21:22:23X",
            "2026-02-29T00:00:00Z",
            "2026-08-07T24:00:00Z",
            "2026/08/07T21:22:23Z",
        ] {
            assert!(matches!(
                build(fields(timestamp, "/journal")),
                Err(ArchiveEncodingError::InvalidMetadata {
                    field: "exported_at",
                    ..
                })
            ));
        }
    }

    #[test]
    fn invalid_versions_are_rejected() {
        for version in ["", "bad\nversion"] {
            let mut values = fields("2026-08-07T21:22:22Z", "/journal");
            values.solstone_version = version;
            assert!(matches!(
                build(values),
                Err(ArchiveEncodingError::InvalidMetadata {
                    field: "solstone_version",
                    ..
                })
            ));
        }
    }
}
