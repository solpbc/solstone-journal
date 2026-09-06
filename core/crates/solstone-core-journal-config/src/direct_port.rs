// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Journal-local paired-device door port.

use std::error::Error;
use std::fmt;
use std::path::{Path, PathBuf};

use serde_json::{Map, Value};

use crate::{ConfigLoadError, get_journal_config_path, read_journal_config};

/// Omitted-key default for the journal-local direct-door listen port.
pub const DEFAULT_DIRECT_DOOR_PORT: u16 = 7657;

/// Invalid, explicit `pairing.direct_port` value.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DirectDoorPortValueError;

impl fmt::Display for DirectDoorPortValueError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("pairing.direct_port must be a decimal port from 1 through 65535")
    }
}

impl Error for DirectDoorPortValueError {}

/// Failure reading the journal-local direct-door port.
#[derive(Debug)]
pub enum DirectDoorPortError {
    Config(ConfigLoadError),
    Invalid { path: PathBuf },
}

impl fmt::Display for DirectDoorPortError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Config(error) => error.fmt(formatter),
            Self::Invalid { path } => write!(
                formatter,
                "pairing.direct_port in {} couldn't be used. it must be a decimal port from 1 through 65535. your settings were not changed.",
                path.display()
            ),
        }
    }
}

impl Error for DirectDoorPortError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Config(error) => Some(error),
            Self::Invalid { .. } => None,
        }
    }
}

impl From<ConfigLoadError> for DirectDoorPortError {
    fn from(error: ConfigLoadError) -> Self {
        Self::Config(error)
    }
}

/// Return `pairing.direct_port` from an already-loaded config map.
///
/// A missing key resolves to [`DEFAULT_DIRECT_DOOR_PORT`]; an explicit value
/// must be an integer in the valid TCP/UDP port range. Existing configs are
/// not merged with defaults, so every reader must use this.
pub fn direct_door_port_from_config(
    config: &Map<String, Value>,
) -> Result<u16, DirectDoorPortValueError> {
    let Some(value) = config.get("pairing").and_then(Value::as_object) else {
        return Ok(DEFAULT_DIRECT_DOOR_PORT);
    };
    match value.get("direct_port") {
        None => Ok(DEFAULT_DIRECT_DOOR_PORT),
        Some(value) => parse_port(value).ok_or(DirectDoorPortValueError),
    }
}

/// Read the journal-local direct-door port from `config/journal.json`.
///
/// A missing file resolves to [`DEFAULT_DIRECT_DOOR_PORT`]. A present but
/// unreadable or invalid file is an error; it never silently picks another
/// listener port.
pub fn read_direct_door_port(journal_path: &Path) -> Result<u16, DirectDoorPortError> {
    let read = read_journal_config(journal_path)?;
    let Some(config) = read.config.as_ref() else {
        return Ok(DEFAULT_DIRECT_DOOR_PORT);
    };
    direct_door_port_from_config(config).map_err(|_| DirectDoorPortError::Invalid {
        path: get_journal_config_path(journal_path),
    })
}

fn parse_port(value: &Value) -> Option<u16> {
    let number = value.as_u64()?;
    let port = u16::try_from(number).ok()?;
    (port != 0).then_some(port)
}

#[cfg(test)]
mod tests {
    use std::fs;

    use serde_json::json;

    use super::*;
    use crate::get_journal_config_path;
    use crate::test_support::TempDir;

    #[test]
    fn omitted_key_and_missing_file_resolve_to_default() {
        assert_eq!(
            direct_door_port_from_config(&Map::new()),
            Ok(DEFAULT_DIRECT_DOOR_PORT)
        );
        let temporary = TempDir::new();
        assert_eq!(
            read_direct_door_port(temporary.path()).unwrap(),
            DEFAULT_DIRECT_DOOR_PORT
        );
    }

    #[test]
    fn present_port_is_returned() {
        let mut config = Map::new();
        config.insert(
            "pairing".to_owned(),
            json!({"home_address": null, "direct_port": 9000}),
        );
        assert_eq!(direct_door_port_from_config(&config), Ok(9000));

        let temporary = TempDir::new();
        let path = get_journal_config_path(temporary.path());
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, b"{\"pairing\":{\"direct_port\":9000}}\n").unwrap();
        assert_eq!(read_direct_door_port(temporary.path()).unwrap(), 9000);
    }

    #[test]
    fn explicit_invalid_values_fail_closed() {
        for value in [
            json!(null),
            json!(0),
            json!(65536),
            json!("9000"),
            json!(-1),
            json!(true),
        ] {
            let mut config = Map::new();
            config.insert("pairing".to_owned(), json!({"direct_port": value}));
            assert_eq!(
                direct_door_port_from_config(&config),
                Err(DirectDoorPortValueError)
            );
        }
    }

    #[test]
    fn corrupt_file_is_a_load_error() {
        let temporary = TempDir::new();
        let path = get_journal_config_path(temporary.path());
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, b"{not json\n").unwrap();
        assert!(matches!(
            read_direct_door_port(temporary.path()),
            Err(DirectDoorPortError::Config(ConfigLoadError::Corrupt { .. }))
        ));
    }

    #[test]
    fn explicit_invalid_file_value_is_a_load_error_not_the_default() {
        let temporary = TempDir::new();
        let path = get_journal_config_path(temporary.path());
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, b"{\"pairing\":{\"direct_port\":0}}\n").unwrap();
        assert!(matches!(
            read_direct_door_port(temporary.path()),
            Err(DirectDoorPortError::Invalid { .. })
        ));
    }
}
