// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! The per-user `~/.config/solstone/config.toml` contract.

use std::collections::BTreeMap;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};

use toml_edit::{DocumentMut, Item};

static TEMP_SEQUENCE: AtomicUsize = AtomicUsize::new(0);

/// Return the user configuration path below an explicit home directory.
#[must_use]
pub fn config_path(home_dir: &Path) -> PathBuf {
    home_dir
        .join(".config")
        .join("solstone")
        .join("config.toml")
}

/// Return the default journal path below an explicit home directory.
#[must_use]
pub fn default_journal(home_dir: &Path) -> PathBuf {
    home_dir.join("journal")
}

/// Read every top-level string value, failing open for unreadable or invalid TOML.
#[must_use]
pub fn read_user_config(path: &Path) -> BTreeMap<String, String> {
    let Ok(content) = fs::read_to_string(path) else {
        return BTreeMap::new();
    };
    let Ok(document) = content.parse::<DocumentMut>() else {
        return BTreeMap::new();
    };
    document
        .iter()
        .filter_map(|(key, item)| top_level_string(item).map(|value| (key.to_owned(), value)))
        .collect()
}

fn top_level_string(item: &Item) -> Option<String> {
    item.as_value()?.as_str().map(ToOwned::to_owned)
}

/// Write the setup-owned journal entry in the exact Python reference format.
pub fn write_user_config(path: &Path, journal: &str) -> io::Result<PathBuf> {
    let parent = path.parent().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "user config path must have a parent directory",
        )
    })?;
    fs::create_dir_all(parent)?;

    let escaped = journal.replace('\\', "\\\\").replace('"', "\\\"");
    let content = format!("journal = \"{escaped}\"\n");
    let (temp_path, mut file) = create_temp_file(parent)?;
    let result = (|| {
        file.write_all(content.as_bytes())?;
        file.flush()?;
        drop(file);
        fs::rename(&temp_path, path)?;
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temp_path);
    }
    result.map(|()| path.to_path_buf())
}

fn create_temp_file(parent: &Path) -> io::Result<(PathBuf, File)> {
    for _ in 0..128 {
        let sequence = TEMP_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let path = parent.join(format!(".tmp_config{}_{sequence}.toml", std::process::id()));
        match OpenOptions::new().write(true).create_new(true).open(&path) {
            Ok(file) => return Ok((path, file)),
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error),
        }
    }
    Err(io::Error::new(
        io::ErrorKind::AlreadyExists,
        "could not allocate temporary user config file",
    ))
}

#[cfg(test)]
mod tests {
    use super::{config_path, default_journal, read_user_config, write_user_config};
    use std::fs;
    use std::path::PathBuf;

    fn temp_root(name: &str) -> PathBuf {
        let path =
            std::env::temp_dir().join(format!("solstone-core-setup-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&path);
        fs::create_dir_all(&path).unwrap();
        path
    }

    #[test]
    fn config_and_default_paths_are_home_scoped() {
        let home = temp_root("paths");
        assert_eq!(
            config_path(&home),
            home.join(".config/solstone/config.toml")
        );
        assert_eq!(default_journal(&home), home.join("journal"));
    }

    #[test]
    fn fresh_write_is_single_key_and_escapes_reference_characters() {
        let home = temp_root("fresh");
        let path = config_path(&home);
        write_user_config(&path, r#"/a\b"c"#).unwrap();
        assert_eq!(
            fs::read_to_string(&path).unwrap(),
            "journal = \"/a\\\\b\\\"c\"\n"
        );
        assert_eq!(
            read_user_config(&path).get("journal"),
            Some(&r#"/a\b"c"#.to_owned())
        );
    }

    #[test]
    fn reader_returns_all_top_level_strings_and_ignores_other_values() {
        let home = temp_root("reader");
        let path = config_path(&home);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, "journal = \"/one\"\nname = \"two\"\ncount = 3\n").unwrap();
        let config = read_user_config(&path);
        assert_eq!(config.get("journal"), Some(&"/one".to_owned()));
        assert_eq!(config.get("name"), Some(&"two".to_owned()));
        assert!(!config.contains_key("count"));
    }

    #[test]
    fn invalid_or_absent_config_fails_open() {
        let home = temp_root("invalid");
        let path = config_path(&home);
        assert!(read_user_config(&path).is_empty());
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, "journal = [").unwrap();
        assert!(read_user_config(&path).is_empty());
    }

    #[test]
    fn persisted_match_is_a_noop_for_the_existing_file() {
        let home = temp_root("persisted");
        let path = config_path(&home);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        let existing = "journal = \"/already\"\nother = \"keep\"\n";
        fs::write(&path, existing).unwrap();
        let configured = read_user_config(&path);
        assert_eq!(configured.get("journal"), Some(&"/already".to_owned()));
        assert_eq!(fs::read_to_string(&path).unwrap(), existing);
    }
}
