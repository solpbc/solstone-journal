// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io;
use std::path::{Component, Path};

use super::{
    ChatLabels, ContentResolution, parse_records_for_family, produce_chunks_by_shape,
    resolve_content_shape,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TalentTextProjection {
    pub key: String,
    pub stem: String,
    pub relative_path: String,
    pub source_suffix: String,
    pub text: String,
}

/// Return one rendered text projection per talent-output key.
pub fn iter_talent_text_projections(
    talents_dir: &Path,
    talents_dir_rel: &str,
    stem_filter: Option<&dyn Fn(&str) -> bool>,
) -> io::Result<Vec<TalentTextProjection>> {
    if !talents_dir.is_dir() {
        return Ok(Vec::new());
    }

    let mut keys = BTreeSet::new();
    collect_keys(talents_dir, talents_dir, &mut keys)?;

    let mut projections = Vec::new();
    for key in keys {
        let stem = key
            .rsplit('/')
            .next()
            .expect("nonempty talent projection key");
        if stem_filter.is_some_and(|filter| !filter(stem)) {
            continue;
        }

        let json_path = talents_dir.join(format!("{key}.json"));
        let json_rel = journal_relative_path(talents_dir_rel, &key, ".json");
        // The fixture's journal-relative directory includes `chronicle/`, while
        // native day-rooted formatter patterns are relative to that directory.
        let classifier_rel = json_rel
            .strip_prefix("chronicle/")
            .unwrap_or(&json_rel)
            .to_string();
        if json_path.is_file()
            && let ContentResolution::Indexed(family) =
                resolve_content_shape(&json_path, &classifier_rel)
        {
            let text = fs::read_to_string(&json_path)?;
            let records = parse_records_for_family(family, &text);
            let produced = produce_chunks_by_shape(
                family,
                Some(&classifier_rel),
                &records,
                &ChatLabels::default(),
            );
            let text = produced
                .chunks
                .into_iter()
                .map(|chunk| chunk.content)
                .collect::<Vec<_>>()
                .join("\n")
                .trim()
                .to_string();
            if !text.is_empty() {
                projections.push(TalentTextProjection {
                    key: key.clone(),
                    stem: stem.to_string(),
                    relative_path: format!("{key}.json"),
                    source_suffix: ".json".to_string(),
                    text,
                });
            }
            continue;
        }

        let md_path = talents_dir.join(format!("{key}.md"));
        if md_path.is_file() {
            let text = fs::read_to_string(md_path)?.trim().to_string();
            if !text.is_empty() {
                projections.push(TalentTextProjection {
                    key: key.clone(),
                    stem: stem.to_string(),
                    relative_path: format!("{key}.md"),
                    source_suffix: ".md".to_string(),
                    text,
                });
            }
        }
    }
    Ok(projections)
}

/// Return talent-output projections keyed by their relative stem path.
pub fn talent_projection_map(
    talents_dir: &Path,
    talents_dir_rel: &str,
) -> io::Result<BTreeMap<String, String>> {
    Ok(
        iter_talent_text_projections(talents_dir, talents_dir_rel, None)?
            .into_iter()
            .map(|projection| (projection.key, projection.text))
            .collect(),
    )
}

fn collect_keys(root: &Path, directory: &Path, keys: &mut BTreeSet<String>) -> io::Result<()> {
    for entry in fs::read_dir(directory)? {
        let entry = entry?;
        let path = entry.path();
        let file_type = entry.file_type()?;
        if file_type.is_dir() {
            collect_keys(root, &path, keys)?;
            continue;
        }
        if !file_type.is_file()
            || !matches!(
                path.extension().and_then(|suffix| suffix.to_str()),
                Some("json" | "md")
            )
        {
            continue;
        }

        let relative = path
            .strip_prefix(root)
            .expect("walked talent file must be rooted under talents directory")
            .with_extension("");
        keys.insert(posix_relative_path(&relative)?);
    }
    Ok(())
}

fn posix_relative_path(path: &Path) -> io::Result<String> {
    path.components()
        .map(|component| match component {
            Component::Normal(part) => part.to_str().map(str::to_string).ok_or_else(|| {
                io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!("talent projection path is not UTF-8: {}", path.display()),
                )
            }),
            _ => Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("talent projection path is not relative: {}", path.display()),
            )),
        })
        .collect::<io::Result<Vec<_>>>()
        .map(|parts| parts.join("/"))
}

fn journal_relative_path(talents_dir_rel: &str, key: &str, suffix: &str) -> String {
    let talents_dir_rel = talents_dir_rel.trim_end_matches('/');
    if talents_dir_rel.is_empty() {
        format!("{key}{suffix}")
    } else {
        format!("{talents_dir_rel}/{key}{suffix}")
    }
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::{Path, PathBuf};

    use super::*;
    use crate::test_support::reserve_temp_path;

    const EXPECTED_TALENT_PROJECTION_COUNT: usize = 5;

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

    #[test]
    fn talent_projection_fixture_matches_the_reference_walker() {
        let fixture: serde_json::Value =
            serde_json::from_str(include_str!("../../../../fixtures/talent_projections.json"))
                .expect("talent projection fixture parses");
        let temporary = TempDir::new("talent-projections");
        let talents_dir = temporary.path.join(
            fixture["talents_dir_rel"]
                .as_str()
                .expect("talents directory relative path"),
        );
        for file in fixture["files"].as_array().expect("fixture files") {
            write(
                &talents_dir.join(file["rel"].as_str().expect("file relative path")),
                file["text"].as_str().expect("file text"),
            );
        }

        let projections = iter_talent_text_projections(
            &talents_dir,
            fixture["talents_dir_rel"]
                .as_str()
                .expect("talents directory rel"),
            None,
        )
        .expect("walk talent projections");
        assert_eq!(projections.len(), EXPECTED_TALENT_PROJECTION_COUNT);
        let actual = projections
            .iter()
            .map(|projection| {
                serde_json::json!({
                    "key": projection.key,
                    "relative_path": projection.relative_path,
                    "source_suffix": projection.source_suffix,
                    "stem": projection.stem,
                    "text": projection.text,
                })
            })
            .collect::<Vec<_>>();
        assert_eq!(serde_json::Value::Array(actual), fixture["projections"]);

        let map = talent_projection_map(
            &talents_dir,
            fixture["talents_dir_rel"]
                .as_str()
                .expect("talents directory rel"),
        )
        .expect("build talent projection map");
        let map_keys = map.keys().cloned().collect::<Vec<_>>();
        let expected_map_keys = fixture["map_keys"]
            .as_array()
            .expect("map keys")
            .iter()
            .map(|key| key.as_str().expect("map key").to_string())
            .collect::<Vec<_>>();
        assert_eq!(map_keys, expected_map_keys);

        let starts_with_s = |stem: &str| stem.starts_with('s');
        let filtered = iter_talent_text_projections(
            &talents_dir,
            fixture["talents_dir_rel"]
                .as_str()
                .expect("talents directory rel"),
            Some(&starts_with_s),
        )
        .expect("walk filtered talent projections");
        let filtered_keys = filtered
            .iter()
            .map(|projection| projection.key.clone())
            .collect::<Vec<_>>();
        let expected_filtered_keys = fixture["stem_filter_s_keys"]
            .as_array()
            .expect("stem filter keys")
            .iter()
            .map(|key| key.as_str().expect("filtered key").to_string())
            .collect::<Vec<_>>();
        assert_eq!(filtered_keys, expected_filtered_keys);
    }

    const SCREEN_SENSE_OBJECT: &str = r#"{"narrative":"09:00 Alice discussed the repo.","content_type":"meeting","activity_summary":"Reviewed launch.","entities":[{"type":"Person","name":"Alice","role":"attendee","context":"Visible"}]}"#;

    #[test]
    fn written_sense_shape_overrides_path_derived_screen() {
        let temporary = TempDir::new("written-sense");
        let talents_dir = temporary
            .path
            .join("chronicle/20260804/workstation/120000_60/talents");
        write(&talents_dir.join("screen.json"), SCREEN_SENSE_OBJECT);
        write(
            &talents_dir.join("shape.json"),
            r#"{"screen.json":"Sense"}"#,
        );

        let projections = iter_talent_text_projections(
            &talents_dir,
            "chronicle/20260804/workstation/120000_60/talents",
            None,
        )
        .expect("walk talent projections");
        assert_eq!(projections.len(), 1);
        assert_eq!(projections[0].key, "screen");
        assert!(projections[0].text.contains("## Sense: meeting"));
        assert!(projections[0].text.contains("Reviewed launch."));
        assert!(!projections[0].text.contains("Person: Alice (attendee)"));
        assert!(
            !projections[0]
                .text
                .contains("- Person: Alice (attendee) - Visible")
        );
    }

    #[test]
    fn unusable_shape_sidecar_does_not_path_render_screen() {
        let temporary = TempDir::new("unusable-screen");
        let talents_dir = temporary
            .path
            .join("chronicle/20260804/workstation/120000_60/talents");
        write(&talents_dir.join("screen.json"), SCREEN_SENSE_OBJECT);
        write(&talents_dir.join("shape.json"), "[]");
        write(
            &talents_dir.join("screen.md"),
            "# Fallback\n\nmarkdown sibling\n",
        );

        let projections = iter_talent_text_projections(
            &talents_dir,
            "chronicle/20260804/workstation/120000_60/talents",
            None,
        )
        .expect("walk talent projections");
        assert!(
            projections.iter().all(|projection| {
                !projection.text.contains("Person: Alice (attendee)")
                    && !projection
                        .text
                        .contains("- Person: Alice (attendee) - Visible")
            }),
            "unusable sidecar must not path-render Screen"
        );
    }

    #[test]
    fn stem_filter_skips_unreadable_render_input_before_reading_it() {
        let temporary = TempDir::new("talent-filter");
        let talents_dir = temporary
            .path
            .join("chronicle/20260304/workstation/090000_300/talents");
        write(&talents_dir.join("screen.json"), "{}");
        write(&talents_dir.join("sense.json"), [0xff]);

        let screen_only = |stem: &str| stem == "screen";
        let projections = iter_talent_text_projections(
            &talents_dir,
            "chronicle/20260304/workstation/090000_300/talents",
            Some(&screen_only),
        )
        .expect("filtered walker must not read excluded invalid UTF-8 JSON");
        assert_eq!(
            projections
                .iter()
                .map(|projection| projection.key.as_str())
                .collect::<Vec<_>>(),
            ["screen"]
        );
    }

    #[test]
    fn dispatched_empty_json_drops_the_key_without_markdown_fallback() {
        let temporary = TempDir::new("talent-empty-json");
        let talents_dir = temporary
            .path
            .join("chronicle/20260304/workstation/090000_300/talents");
        write(&talents_dir.join("screen.json"), "[]");
        write(
            &talents_dir.join("screen.md"),
            "# Screen\n\nfallback text\n",
        );

        let projections = iter_talent_text_projections(
            &talents_dir,
            "chronicle/20260304/workstation/090000_300/talents",
            None,
        )
        .expect("walk talent projections");
        assert!(
            projections
                .iter()
                .all(|projection| projection.key != "screen"),
            "an empty dispatched JSON projection must suppress its Markdown sibling"
        );
    }
}
