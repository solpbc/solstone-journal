// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::fs;
use std::io;
use std::path::Path;

use serde_json::Value;

use super::{ContentResolution, Family, RawPerceptFamily, classify};

/// Basename of the reserved per-directory written-shape sidecar.
pub const SHAPE_SIDECAR_BASENAME: &str = "shape.json";

/// Map a written PascalCase shape name onto a content resolution.
///
/// The seventeen names are the fifteen [`Family`] variants plus the two
/// unindexed raw-percept families. Unknown spellings return [`None`].
pub fn parse_shape_name(name: &str) -> Option<ContentResolution> {
    Some(match name {
        "Markdown" => ContentResolution::Indexed(Family::Markdown),
        "Event" => ContentResolution::Indexed(Family::Event),
        "Activity" => ContentResolution::Indexed(Family::Activity),
        "ActionLog" => ContentResolution::Indexed(Family::ActionLog),
        "StructuredImport" => ContentResolution::Indexed(Family::StructuredImport),
        "AiChat" => ContentResolution::Indexed(Family::AiChat),
        "Chat" => ContentResolution::Indexed(Family::Chat),
        "Browser" => ContentResolution::Indexed(Family::Browser),
        "DayAccumulator" => ContentResolution::Indexed(Family::DayAccumulator),
        "FacetEntity" => ContentResolution::Indexed(Family::FacetEntity),
        "Observation" => ContentResolution::Indexed(Family::Observation),
        "Documents" => ContentResolution::Indexed(Family::Documents),
        "Screen" => ContentResolution::Indexed(Family::Screen),
        "Sense" => ContentResolution::Indexed(Family::Sense),
        "MorningBriefing" => ContentResolution::Indexed(Family::MorningBriefing),
        "Audio" => ContentResolution::Unindexed(RawPerceptFamily::Audio),
        "RawScreen" => ContentResolution::Unindexed(RawPerceptFamily::RawScreen),
        _ => return None,
    })
}

enum WrittenLookup {
    Absent,
    Omitted,
    Hit(ContentResolution),
    Unusable,
}

/// Resolve a content file's shape, preferring a sibling `shape.json`.
///
/// The sidecar lives in the same directory as `content_path`. Only a proven
/// absence (`NotFound`) or an omitted basename key falls through to the
/// path-derived [`classify`]. Anything that exists must yield a usable written
/// value or the file is [`ContentResolution::Unrecognized`].
pub fn resolve_content_shape(content_path: &Path, rel: &str) -> ContentResolution {
    match lookup_written_shape(content_path) {
        WrittenLookup::Absent | WrittenLookup::Omitted => classify(rel),
        WrittenLookup::Hit(resolution) => resolution,
        WrittenLookup::Unusable => ContentResolution::Unrecognized,
    }
}

fn lookup_written_shape(content_path: &Path) -> WrittenLookup {
    let sidecar = content_path.with_file_name(SHAPE_SIDECAR_BASENAME);
    match fs::symlink_metadata(&sidecar) {
        Err(error) if error.kind() == io::ErrorKind::NotFound => WrittenLookup::Absent,
        Err(_) => WrittenLookup::Unusable,
        Ok(meta) if meta.file_type().is_dir() => WrittenLookup::Unusable,
        Ok(_) => match fs::read(&sidecar) {
            Err(_) => WrittenLookup::Unusable,
            Ok(bytes) => interpret_sidecar(&bytes, content_path),
        },
    }
}

fn interpret_sidecar(bytes: &[u8], content_path: &Path) -> WrittenLookup {
    let Ok(value) = serde_json::from_slice::<Value>(bytes) else {
        return WrittenLookup::Unusable;
    };
    let Some(object) = value.as_object() else {
        return WrittenLookup::Unusable;
    };
    let Some(basename) = content_path.file_name().and_then(|name| name.to_str()) else {
        return WrittenLookup::Unusable;
    };
    match object.get(basename) {
        None => WrittenLookup::Omitted,
        Some(value) => {
            let Some(name) = value.as_str() else {
                return WrittenLookup::Unusable;
            };
            match parse_shape_name(name) {
                Some(resolution) => WrittenLookup::Hit(resolution),
                None => WrittenLookup::Unusable,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::{Path, PathBuf};

    use super::*;
    use crate::test_support::reserve_temp_path;

    const CHAT_REL: &str = "20260804/chat/120000_60/chat.jsonl";
    const BROWSER_REL: &str = "20260804/workstation/120000_60/browser_tab.jsonl";

    struct TempDir {
        path: PathBuf,
    }

    impl TempDir {
        fn new(name: &str) -> Self {
            let path = reserve_temp_path(&format!("solstone-core-format-shape-{name}"));
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

    fn content_path(root: &Path, rel: &str) -> PathBuf {
        let path = root.join(rel);
        write(&path, b"{}\n");
        path
    }

    #[test]
    fn parse_shape_name_accepts_the_seventeen_names() {
        let indexed = [
            ("Markdown", Family::Markdown),
            ("Event", Family::Event),
            ("Activity", Family::Activity),
            ("ActionLog", Family::ActionLog),
            ("StructuredImport", Family::StructuredImport),
            ("AiChat", Family::AiChat),
            ("Chat", Family::Chat),
            ("Browser", Family::Browser),
            ("DayAccumulator", Family::DayAccumulator),
            ("FacetEntity", Family::FacetEntity),
            ("Observation", Family::Observation),
            ("Documents", Family::Documents),
            ("Screen", Family::Screen),
            ("Sense", Family::Sense),
            ("MorningBriefing", Family::MorningBriefing),
        ];
        for (name, family) in indexed {
            assert_eq!(
                parse_shape_name(name),
                Some(ContentResolution::Indexed(family)),
                "{name}"
            );
        }
        assert_eq!(
            parse_shape_name("Audio"),
            Some(ContentResolution::Unindexed(RawPerceptFamily::Audio))
        );
        assert_eq!(
            parse_shape_name("RawScreen"),
            Some(ContentResolution::Unindexed(RawPerceptFamily::RawScreen))
        );
        for name in ["markdown", "", "IndexedElsewhere"] {
            assert_eq!(parse_shape_name(name), None, "{name:?}");
        }
    }

    #[test]
    fn absent_sidecar_uses_derived_classify() {
        let temporary = TempDir::new("absent");
        let path = content_path(&temporary.path, CHAT_REL);
        assert_eq!(
            resolve_content_shape(&path, CHAT_REL),
            ContentResolution::Indexed(Family::Chat)
        );
    }

    #[test]
    fn omitted_key_uses_derived_classify() {
        let temporary = TempDir::new("omitted");
        let path = content_path(&temporary.path, CHAT_REL);
        write(
            &path.with_file_name(SHAPE_SIDECAR_BASENAME),
            r#"{"other.jsonl":"Browser"}"#,
        );
        assert_eq!(
            resolve_content_shape(&path, CHAT_REL),
            ContentResolution::Indexed(Family::Chat)
        );
    }

    #[test]
    fn unusable_sidecar_is_unrecognized() {
        let temporary = TempDir::new("unusable");
        let path = content_path(&temporary.path, CHAT_REL);
        let sidecar = path.with_file_name(SHAPE_SIDECAR_BASENAME);

        let cases: &[(&str, &[u8])] = &[
            ("non-json", b"not-json"),
            ("array", b"[]"),
            ("string", b"\"Chat\""),
            ("number", b"1"),
            ("null", b"null"),
            ("bool", b"true"),
            ("object-value", br#"{"chat.jsonl":{}}"#),
            ("bool-value", br#"{"chat.jsonl":true}"#),
            ("array-value", br#"{"chat.jsonl":[]}"#),
            ("number-value", br#"{"chat.jsonl":1}"#),
            ("null-value", br#"{"chat.jsonl":null}"#),
            ("unknown-spelling", br#"{"chat.jsonl":"chat"}"#),
            ("unknown-name", br#"{"chat.jsonl":"Unknown"}"#),
        ];
        for (label, bytes) in cases {
            let _ = fs::remove_file(&sidecar);
            let _ = fs::remove_dir_all(&sidecar);
            write(&sidecar, bytes);
            assert_eq!(
                resolve_content_shape(&path, CHAT_REL),
                ContentResolution::Unrecognized,
                "{label}"
            );
        }

        let _ = fs::remove_file(&sidecar);
        fs::create_dir(&sidecar).expect("directory named shape.json");
        assert_eq!(
            resolve_content_shape(&path, CHAT_REL),
            ContentResolution::Unrecognized,
            "directory"
        );
        fs::remove_dir(&sidecar).expect("remove directory sidecar");

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            write(&sidecar, br#"{"chat.jsonl":"Chat"}"#);
            fs::set_permissions(&sidecar, fs::Permissions::from_mode(0o000))
                .expect("chmod unreadable sidecar");
            if fs::read(&sidecar).is_ok() {
                let _ = fs::set_permissions(&sidecar, fs::Permissions::from_mode(0o644));
                return;
            }
            assert_eq!(
                resolve_content_shape(&path, CHAT_REL),
                ContentResolution::Unrecognized,
                "unreadable file"
            );
            fs::set_permissions(&sidecar, fs::Permissions::from_mode(0o644))
                .expect("restore sidecar mode");
        }
    }

    #[test]
    fn written_indexed_wins_over_derived_indexed() {
        let temporary = TempDir::new("written-indexed");
        let chat_path = content_path(&temporary.path, CHAT_REL);
        write(
            &chat_path.with_file_name(SHAPE_SIDECAR_BASENAME),
            r#"{"chat.jsonl":"Browser"}"#,
        );
        assert_eq!(
            resolve_content_shape(&chat_path, CHAT_REL),
            ContentResolution::Indexed(Family::Browser)
        );

        let browser_path = content_path(&temporary.path, BROWSER_REL);
        write(
            &browser_path.with_file_name(SHAPE_SIDECAR_BASENAME),
            r#"{"browser_tab.jsonl":"Chat"}"#,
        );
        assert_eq!(
            resolve_content_shape(&browser_path, BROWSER_REL),
            ContentResolution::Indexed(Family::Chat)
        );
    }

    #[test]
    fn written_raw_percept_names_are_unindexed() {
        let temporary = TempDir::new("raw-percept");
        let path = content_path(&temporary.path, CHAT_REL);
        write(
            &path.with_file_name(SHAPE_SIDECAR_BASENAME),
            r#"{"chat.jsonl":"Audio"}"#,
        );
        assert_eq!(
            resolve_content_shape(&path, CHAT_REL),
            ContentResolution::Unindexed(RawPerceptFamily::Audio)
        );
        write(
            &path.with_file_name(SHAPE_SIDECAR_BASENAME),
            r#"{"chat.jsonl":"RawScreen"}"#,
        );
        assert_eq!(
            resolve_content_shape(&path, CHAT_REL),
            ContentResolution::Unindexed(RawPerceptFamily::RawScreen)
        );
    }
}
