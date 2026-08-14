// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::path::Path;

use serde_json::{Map, Value};
use solstone_core_talent_config::{
    TalentFilter, get_output_name, load_talent_configs, output_extension,
};

use crate::JournalStatsError;

#[derive(Debug, Default)]
pub(crate) struct DailyOutputCounts {
    pub processed: u64,
    pub pending: u64,
}

pub(crate) fn daily_output_counts(
    day_dir: &Path,
    system_root: &Path,
    apps_root: &Path,
    talent_overrides: Option<&Map<String, Value>>,
) -> Result<DailyOutputCounts, JournalStatsError> {
    let configs = load_talent_configs(
        system_root,
        apps_root,
        talent_overrides,
        TalentFilter {
            r#type: Some("generate"),
            schedule: Some("daily"),
            include_disabled: true,
        },
    )
    .map_err(JournalStatsError::Validation)?;
    let mut counts = DailyOutputCounts::default();
    for config in configs {
        let filename = format!(
            "{}.{}",
            get_output_name(&config.key),
            output_extension(config.metadata.get("output").and_then(Value::as_str))
        );
        if day_dir.join("talents").join(filename).exists() {
            counts.processed += 1;
        } else {
            counts.pending += 1;
        }
    }
    Ok(counts)
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::*;

    // §7 criteria 1, 2, and 4: counts are this consumer's nearest observable.
    const CASES: [(&str, &str, bool); 11] = [
        (
            "lf",
            "{\n\"type\":\"generate\",\"output\":\"md\",\"schedule\":\"daily\",\"priority\":50\n}\nbody",
            true,
        ),
        (
            "leading_blank",
            "\n{\n\"type\":\"generate\",\"output\":\"md\",\"schedule\":\"daily\",\"priority\":50\n}\nbody",
            true,
        ),
        ("unclosed", "{\n\"type\":\"generate\"\nbody", false),
        (
            "crlf",
            "{\r\n\"type\":\"generate\",\"output\":\"md\",\"schedule\":\"daily\",\"priority\":50\r\n}\r\nbody",
            true,
        ),
        ("opening_space", "{ \n\"type\":\"generate\"\n}\nbody", false),
        (
            "nested_column_zero",
            "{\n\"type\":\"generate\",\"output\":\"md\",\"schedule\":\"daily\",\"priority\":50,\n\"nested\": {\n\"x\":1\n}\n}\nbody",
            false,
        ),
        (
            "nested_indented",
            "{\n\"type\":\"generate\",\"output\":\"md\",\"schedule\":\"daily\",\"priority\":50,\n\"nested\": {\n\"x\":1\n }\n}\nbody",
            true,
        ),
        ("invalid", "{\n\"type\": generate\n}\nbody", false),
        ("none", "body", false),
        ("empty", "", false),
        ("array", "[\"generate\"]\nbody", false),
    ];

    fn roots() -> tempfile::TempDir {
        let root = tempfile::tempdir().unwrap();
        fs::create_dir_all(root.path().join("talent")).unwrap();
        fs::create_dir_all(root.path().join("apps")).unwrap();
        fs::create_dir_all(root.path().join("day")).unwrap();
        root
    }

    #[test]
    fn criterion_1_2_4_count_conformance() {
        for (name, contents, counted) in CASES {
            let root = roots();
            fs::write(root.path().join("talent/case.md"), contents).unwrap();
            let result = daily_output_counts(
                &root.path().join("day"),
                &root.path().join("talent"),
                &root.path().join("apps"),
                None,
            );
            if matches!(name, "nested_column_zero" | "invalid") {
                assert!(result.is_err(), "{name}");
            } else {
                assert_eq!(result.unwrap().pending, u64::from(counted), "{name}");
            }
        }
    }

    #[test]
    fn criterion_2_crlf_equals_lf_and_disabled_still_counts() {
        let lf = roots();
        let crlf = roots();
        fs::write(lf.path().join("talent/case.md"), CASES[0].1).unwrap();
        fs::write(crlf.path().join("talent/case.md"), CASES[3].1).unwrap();
        assert_eq!(
            daily_output_counts(
                &lf.path().join("day"),
                &lf.path().join("talent"),
                &lf.path().join("apps"),
                None
            )
            .unwrap()
            .pending,
            daily_output_counts(
                &crlf.path().join("day"),
                &crlf.path().join("talent"),
                &crlf.path().join("apps"),
                None
            )
            .unwrap()
            .pending
        );
        let overrides = Map::from_iter([(
            "talent.system.case".to_owned(),
            serde_json::json!({"disabled":true}),
        )]);
        assert_eq!(
            daily_output_counts(
                &lf.path().join("day"),
                &lf.path().join("talent"),
                &lf.path().join("apps"),
                Some(&overrides)
            )
            .unwrap()
            .pending,
            1
        );
    }
}
