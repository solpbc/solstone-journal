// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Detection of Strava bulk downloads and activity files.

use std::fs::{self, File};
use std::io::Read;
use std::path::Path;

const ACTIVITIES_CSV: &str = "activities.csv";
const MAX_HEADER_READ_BYTES: usize = 64 * 1024;

/// Returns true if the byte buffer's first line has a first comma-separated field
/// of exactly "Activity ID" or "Aktivitäts-ID".
pub fn looks_like_strava_csv_bytes(bytes: &[u8]) -> bool {
    let mut data = bytes;
    if data.starts_with(b"\xef\xbb\xbf") {
        data = &data[3..];
    }
    let end = data
        .iter()
        .position(|&b| b == b'\n' || b == b'\r')
        .unwrap_or(data.len());
    let first_line = match std::str::from_utf8(&data[..end]) {
        Ok(s) => s.trim_end_matches('\r'),
        Err(_) => return false,
    };
    let first_field = first_line.split(',').next().unwrap_or(first_line).trim();
    first_field == "Activity ID" || first_field == "Aktivitäts-ID"
}

fn file_passes_csv_rule(path: &Path) -> bool {
    let mut file = match File::open(path) {
        Ok(f) => f,
        Err(_) => return false,
    };
    let mut buf = vec![0u8; MAX_HEADER_READ_BYTES];
    let n = match file.read(&mut buf) {
        Ok(n) => n,
        Err(_) => return false,
    };
    looks_like_strava_csv_bytes(&buf[..n])
}

/// Check if a path points to a Strava export, activities CSV, or export directory.
///
/// Any I/O or parse failure returns false.
pub fn looks_like_strava_download(path: &Path, name_hint: Option<&str>) -> bool {
    let name = name_hint
        .filter(|s| !s.is_empty())
        .or_else(|| path.file_name().and_then(|n| n.to_str()))
        .unwrap_or("");

    if name.eq_ignore_ascii_case(ACTIVITIES_CSV) {
        return true;
    }

    if path.is_file() {
        let name_ends_zip = name.to_ascii_lowercase().ends_with(".zip");
        let is_zip = if name_ends_zip {
            true
        } else {
            match File::open(path) {
                Ok(mut f) => {
                    let mut magic = [0u8; 4];
                    f.read_exact(&mut magic).is_ok() && &magic == b"PK\x03\x04"
                }
                Err(_) => false,
            }
        };

        if is_zip {
            // A zip is recognized from its central-directory names. The member bytes are not read.
            let file = match File::open(path) {
                Ok(f) => f,
                Err(_) => return false,
            };
            let archive = match zip::ZipArchive::new(file) {
                Ok(a) => a,
                Err(_) => return false,
            };
            for entry_name in archive.file_names() {
                let parts: Vec<&str> = entry_name
                    .split(['/', '\\'])
                    .filter(|s| !s.is_empty())
                    .collect();
                if (parts.len() == 1 || parts.len() == 2)
                    && parts
                        .last()
                        .is_some_and(|last| last.eq_ignore_ascii_case(ACTIVITIES_CSV))
                {
                    return true;
                }
            }
            return false;
        }

        if name.to_ascii_lowercase().ends_with(".csv") {
            return file_passes_csv_rule(path);
        }

        return false;
    }

    if path.is_dir() {
        let root_activities = path.join(ACTIVITIES_CSV);
        if root_activities.is_file() && file_passes_csv_rule(&root_activities) {
            return true;
        }

        if let Ok(entries) = fs::read_dir(path) {
            for entry in entries.flatten() {
                let child_path = entry.path();
                if child_path.is_dir() {
                    let child_activities = child_path.join(ACTIVITIES_CSV);
                    if child_activities.is_file() && file_passes_csv_rule(&child_activities) {
                        return true;
                    }
                }
            }
        }
    }

    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::TempDir;

    #[test]
    fn detector_table_cases() {
        let temp = TempDir::new().unwrap();

        let zip_root = temp.path().join("root.zip");
        {
            let file = File::create(&zip_root).unwrap();
            let mut zip = zip::ZipWriter::new(file);
            zip.start_file("activities.csv", zip::write::SimpleFileOptions::default())
                .unwrap();
            zip.write_all(b"arbitrary").unwrap();
            zip.finish().unwrap();
        }

        let zip_nested = temp.path().join("nested.zip");
        {
            let file = File::create(&zip_nested).unwrap();
            let mut zip = zip::ZipWriter::new(file);
            zip.start_file(
                "export_1/activities.csv",
                zip::write::SimpleFileOptions::default(),
            )
            .unwrap();
            zip.write_all(b"arbitrary").unwrap();
            zip.finish().unwrap();
        }

        let extensionless_zip = temp.path().join("export_raw");
        {
            let file = File::create(&extensionless_zip).unwrap();
            let mut zip = zip::ZipWriter::new(file);
            zip.start_file("activities.csv", zip::write::SimpleFileOptions::default())
                .unwrap();
            zip.write_all(b"arbitrary").unwrap();
            zip.finish().unwrap();
        }

        let upper_csv = temp.path().join("ACTIVITIES.CSV");
        fs::write(&upper_csv, b"arbitrary content").unwrap();

        let bom_csv = temp.path().join("workouts.csv");
        {
            let mut bytes = b"\xef\xbb\xbf".to_vec();
            bytes.extend_from_slice(b"Activity ID,Activity Date\n1,2026-01-01\n");
            fs::write(&bom_csv, bytes).unwrap();
        }

        let de_csv = temp.path().join("x.csv");
        fs::write(
            &de_csv,
            b"Aktivit\xc3\xa4ts-ID,Aktivit\xc3\xa4tsdatum\n1,2\n",
        )
        .unwrap();

        let dir_root_csv = temp.path().join("dir_root_csv");
        fs::create_dir_all(&dir_root_csv).unwrap();
        fs::write(
            dir_root_csv.join("activities.csv"),
            b"Activity ID,Activity Date\n1,2026-01-01\n",
        )
        .unwrap();

        let dir_nested_csv = temp.path().join("dir_nested_csv");
        fs::create_dir_all(dir_nested_csv.join("export_1")).unwrap();
        fs::write(
            dir_nested_csv.join("export_1/activities.csv"),
            b"Activity ID,Activity Date\n1,2026-01-01\n",
        )
        .unwrap();

        let zip_conv = temp.path().join("conversations.zip");
        {
            let file = File::create(&zip_conv).unwrap();
            let mut zip = zip::ZipWriter::new(file);
            zip.start_file(
                "conversations.json",
                zip::write::SimpleFileOptions::default(),
            )
            .unwrap();
            zip.write_all(b"arbitrary").unwrap();
            zip.finish().unwrap();
        }

        let zip_deep = temp.path().join("deep.zip");
        {
            let file = File::create(&zip_deep).unwrap();
            let mut zip = zip::ZipWriter::new(file);
            zip.start_file(
                "a/b/activities.csv",
                zip::write::SimpleFileOptions::default(),
            )
            .unwrap();
            zip.write_all(b"arbitrary").unwrap();
            zip.finish().unwrap();
        }

        let notes_csv = temp.path().join("notes.csv");
        fs::write(&notes_csv, b"Title,Body\nNote 1,Hello\n").unwrap();

        let trunc_zip = temp.path().join("x.zip");
        fs::write(&trunc_zip, b"PK\x03\x04truncated").unwrap();

        let empty_dir = temp.path().join("empty_dir");
        fs::create_dir_all(&empty_dir).unwrap();

        let missing_export = temp.path().join("missing-export.zip");

        let dir_nested_non_workout = temp.path().join("dir_nested_non_workout");
        fs::create_dir_all(dir_nested_non_workout.join("notes")).unwrap();
        fs::write(
            dir_nested_non_workout.join("notes/activities.csv"),
            b"Title,Body\nNote 1,Hello\n",
        )
        .unwrap();

        let dir_workouts_csv = temp.path().join("dir_workouts_csv");
        fs::create_dir_all(&dir_workouts_csv).unwrap();
        fs::write(
            dir_workouts_csv.join("workouts.csv"),
            b"Activity ID,Activity Date\n1,2026-01-01\n",
        )
        .unwrap();

        let dir_bad_root_csv = temp.path().join("dir_bad_root_csv");
        fs::create_dir_all(&dir_bad_root_csv).unwrap();
        fs::write(
            dir_bad_root_csv.join("activities.csv"),
            b"Title,Body\nNote 1,Hello\n",
        )
        .unwrap();

        let table: Vec<(&Path, Option<&str>, bool)> = vec![
            (&zip_root, None, true),
            (&zip_nested, None, true),
            (&extensionless_zip, None, true),
            (&upper_csv, None, true),
            (&bom_csv, None, true),
            (&de_csv, None, true),
            (&dir_root_csv, None, true),
            (&dir_nested_csv, None, true),
            (&zip_conv, None, false),
            (&zip_deep, None, false),
            (&notes_csv, None, false),
            (&trunc_zip, None, false),
            (&empty_dir, None, false),
            (&missing_export, None, false),
            (&dir_nested_non_workout, None, false),
            (&dir_workouts_csv, None, false),
            (&dir_bad_root_csv, None, false),
        ];

        for (path, hint, expected) in table {
            let actual = looks_like_strava_download(path, hint);
            assert_eq!(
                actual,
                expected,
                "failed for path: {} with hint: {:?}",
                path.display(),
                hint
            );
        }
    }
}
