// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Morning-briefing loading boundary.
//!
//! The Python path authority is `morning_briefing_path`
//! (`solstone/think/talent.py:231`) → `day_path`
//! (`solstone/think/utils.py:289`) → `get_output_path`
//! (`solstone/think/talent.py:204`), which composes
//! `{journal}/chronicle/{day}/talents/morning_briefing.json`.
//!
//! The `.json` filename is deliberately pinned: it derives from `output:
//! "json"` in `solstone/talent/morning_briefing.md:9` through
//! `_briefing_output_format` (`solstone/think/talent.py:222`). Changing that
//! frontmatter is a known break, not a silent one.
//!
//! The renderer's preamble handling has an accepted line-splitting divergence:
//! Python `str.splitlines()` also splits on `\x0b`, `\x0c`, `\x1c`–`\x1e`,
//! `\x85`, U+2028, U+2029, and a lone `\r`; Rust `str::lines()` splits on
//! `\n` only while stripping a trailing `\r`. `clean_value`
//! (`content/mod.rs:547`) trims the preamble before per-line `>` quoting. That
//! trim is load-bearing: leading blank lines cannot produce a leading bare `>`
//! row.
//!
//! Absent, unreadable, unparseable, non-object, and missing-required-key files
//! all deliberately collapse to `None`, matching
//! `solstone/think/briefing.py:34-51`. The Python loader logs parse failures;
//! this path-parameterized library seam intentionally does not, so those cases
//! remain indistinguishable to callers.

use std::fs;
use std::path::Path;

use serde_json::Value;

use crate::content::JsonObject;
use crate::paths::CHRONICLE_DIR;
use crate::segment::is_date_key;

const REQUIRED_ROOT_KEYS: [&str; 6] = [
    "metadata",
    "your_day",
    "yesterday",
    "needs_attention",
    "forward_look",
    "reading",
];

const TALENTS_DIR: &str = "talents";
const MORNING_BRIEFING_FILE: &str = "morning_briefing.json";

/// Load one day's morning briefing when it has the required root shape.
pub fn load_morning_briefing(journal: &Path, day: &str) -> Option<JsonObject> {
    let path = journal
        .join(CHRONICLE_DIR)
        .join(day)
        .join(TALENTS_DIR)
        .join(MORNING_BRIEFING_FILE);
    let text = fs::read_to_string(path).ok()?;
    let value = serde_json::from_str::<Value>(&text).ok()?;
    let Value::Object(briefing) = value else {
        return None;
    };
    REQUIRED_ROOT_KEYS
        .iter()
        .all(|key| briefing.contains_key(*key))
        .then_some(briefing)
}

/// Return the newest chronicle day whose morning briefing is loadable.
pub fn most_recent_morning_briefing_day(journal: &Path) -> Option<String> {
    let entries = fs::read_dir(journal.join(CHRONICLE_DIR)).ok()?;
    let mut days = entries
        .filter_map(Result::ok)
        .filter_map(|entry| {
            entry
                .file_type()
                .ok()
                .filter(|file_type| file_type.is_dir())?;
            let day = entry.file_name();
            let day = day.to_str()?;
            is_date_key(day).then(|| day.to_string())
        })
        .collect::<Vec<_>>();
    days.sort_unstable();
    days.into_iter()
        .rev()
        .find(|day| load_morning_briefing(journal, day).is_some())
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::{Path, PathBuf};

    use serde_json::Value;

    use super::*;
    use crate::content::render_morning_briefing_text;
    use crate::test_support::reserve_temp_path;

    const COMPLETE_BRIEFING: &str = r#"{"metadata":{},"your_day":[],"yesterday":[],"needs_attention":[],"forward_look":[],"reading":[]}"#;

    struct TempDir {
        path: PathBuf,
    }

    impl TempDir {
        fn new(name: &str) -> Self {
            let path = reserve_temp_path(&format!("solstone-core-format-{name}"));
            fs::create_dir_all(&path).expect("create temporary directory");
            Self { path }
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.path);
        }
    }

    fn write(path: &Path, text: impl AsRef<[u8]>) {
        fs::create_dir_all(path.parent().expect("file parent")).expect("create file parent");
        fs::write(path, text).expect("write fixture file");
    }

    fn briefing_path(root: &Path, day: &str) -> PathBuf {
        root.join(CHRONICLE_DIR)
            .join(day)
            .join(TALENTS_DIR)
            .join(MORNING_BRIEFING_FILE)
    }

    #[test]
    fn renders_only_from_the_canonical_chronicle_path() {
        let canonical = TempDir::new("briefing-canonical");
        write(
            &briefing_path(&canonical.path, "20260717"),
            COMPLETE_BRIEFING,
        );
        let briefing =
            load_morning_briefing(&canonical.path, "20260717").expect("canonical briefing loads");
        assert!(render_morning_briefing_text(&briefing).contains("## Your Day"));

        let noncanonical = TempDir::new("briefing-noncanonical");
        write(
            &noncanonical
                .path
                .join("20260717")
                .join(TALENTS_DIR)
                .join(MORNING_BRIEFING_FILE),
            COMPLETE_BRIEFING,
        );
        assert_eq!(load_morning_briefing(&noncanonical.path, "20260717"), None);
    }

    #[test]
    fn day_enumeration_boundary() {
        let temporary = TempDir::new("briefing-days");
        let chronicle = temporary.path.join(CHRONICLE_DIR);
        for day in ["2026071", "202607171"] {
            fs::create_dir_all(chronicle.join(day)).expect("create invalid day directory");
        }
        write(&chronicle.join("20260718"), "not a directory");
        write(
            &briefing_path(&temporary.path, "20260717"),
            COMPLETE_BRIEFING,
        );
        assert_eq!(
            most_recent_morning_briefing_day(&temporary.path),
            Some("20260717".to_string())
        );

        let missing = TempDir::new("briefing-no-chronicle");
        assert_eq!(most_recent_morning_briefing_day(&missing.path), None);

        let chronicle_file = TempDir::new("briefing-chronicle-file");
        write(&chronicle_file.path.join(CHRONICLE_DIR), "not a directory");
        assert_eq!(most_recent_morning_briefing_day(&chronicle_file.path), None);
    }

    #[test]
    fn load_gate_requires_each_root_key() {
        let temporary = TempDir::new("briefing-required-keys");
        let path = briefing_path(&temporary.path, "20260717");
        write(&path, COMPLETE_BRIEFING);
        assert!(load_morning_briefing(&temporary.path, "20260717").is_some());

        for key in REQUIRED_ROOT_KEYS {
            let mut briefing: Value =
                serde_json::from_str(COMPLETE_BRIEFING).expect("complete briefing parses");
            briefing
                .as_object_mut()
                .expect("briefing root object")
                .remove(key);
            write(
                &path,
                serde_json::to_string(&briefing).expect("serialize incomplete briefing"),
            );
            assert!(
                load_morning_briefing(&temporary.path, "20260717").is_none(),
                "missing {key} must not load"
            );
        }
    }

    #[test]
    fn load_gate_is_key_presence_only() {
        let temporary = TempDir::new("briefing-key-presence");
        write(
            &briefing_path(&temporary.path, "20260717"),
            r#"{"metadata":null,"your_day":"nope","yesterday":[],"needs_attention":[],"forward_look":[],"reading":[]}"#,
        );
        let briefing = load_morning_briefing(&temporary.path, "20260717")
            .expect("all root keys load regardless of value shape");
        let rendered = render_morning_briefing_text(&briefing);
        assert_eq!(rendered.matches("Nothing to report.").count(), 5);
    }

    #[test]
    fn unloadable_inputs_are_indistinguishable() {
        let temporary = TempDir::new("briefing-unloadable");
        let path = briefing_path(&temporary.path, "20260717");
        assert_eq!(load_morning_briefing(&temporary.path, "20260717"), None);

        for text in ["{", "[]", r#""briefing""#] {
            write(&path, text);
            assert_eq!(load_morning_briefing(&temporary.path, "20260717"), None);
        }
    }

    #[test]
    fn preamble_leading_blank_lines_are_trimmed_before_quoting() {
        let briefing: JsonObject = serde_json::from_str(
            r#"{"metadata":{"coverage_preamble":"\n\nFirst line\n\nSecond line\n\n"},"your_day":[],"yesterday":[],"needs_attention":[],"forward_look":[],"reading":[]}"#,
        )
        .expect("briefing object parses");
        let rendered = render_morning_briefing_text(&briefing);
        let (preamble, _) = rendered
            .split_once("\n\n## Your Day")
            .expect("preamble precedes first section");
        assert_eq!(preamble.lines().next(), Some("> First line"));
        assert_ne!(preamble.lines().last(), Some(">"));
    }

    #[test]
    fn most_recent_means_most_recent_loadable() {
        let temporary = TempDir::new("briefing-most-recent-loadable");
        write(
            &briefing_path(&temporary.path, "20260717"),
            COMPLETE_BRIEFING,
        );
        write(
            &briefing_path(&temporary.path, "20260718"),
            r#"{"metadata":{},"your_day":[],"yesterday":[],"needs_attention":[],"forward_look":[]}"#,
        );
        assert_eq!(
            most_recent_morning_briefing_day(&temporary.path),
            Some("20260717".to_string())
        );
    }
}
