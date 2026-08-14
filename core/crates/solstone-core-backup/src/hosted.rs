// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::fs;
use std::path::{Path, PathBuf};

use serde_json::{Map, Value};
use solstone_core_journal_io::{JsonWriteOptions, write_json};

use crate::{BackupError, HostedBinding};

fn hosted_dir(journal: &Path) -> std::io::Result<PathBuf> {
    let path = journal.join("backup").join("hosted");
    fs::create_dir_all(&path)?;
    Ok(path)
}

fn hosted_binding_path(journal: &Path) -> std::io::Result<PathBuf> {
    Ok(hosted_dir(journal)?.join("binding.json"))
}

fn hosted_binding_read_path(journal: &Path) -> PathBuf {
    journal.join("backup").join("hosted").join("binding.json")
}

pub fn load_hosted_binding(journal: &Path) -> Option<HostedBinding> {
    let bytes = read_binding_bytes(&hosted_binding_read_path(journal)).ok()?;
    let raw = serde_json::from_slice::<Value>(&bytes).ok()?;
    let raw = raw.as_object()?;
    Some(HostedBinding {
        broker_endpoint: non_blank(raw, "broker_endpoint")?,
        account_id: non_blank(raw, "account_id")?,
        instance_id: non_blank(raw, "instance_id")?,
        bucket: non_blank(raw, "bucket")?,
        prefix: non_blank(raw, "prefix")?,
        broker_token: non_blank(raw, "broker_token")?,
    })
}

pub fn save_hosted_binding(journal: &Path, binding: &HostedBinding) -> Result<(), BackupError> {
    let path = hosted_binding_path(journal).map_err(|_| BackupError::HostedWrite)?;
    let value = Map::from_iter([
        (
            "broker_endpoint".to_owned(),
            Value::String(binding.broker_endpoint.clone()),
        ),
        (
            "account_id".to_owned(),
            Value::String(binding.account_id.clone()),
        ),
        (
            "instance_id".to_owned(),
            Value::String(binding.instance_id.clone()),
        ),
        ("bucket".to_owned(), Value::String(binding.bucket.clone())),
        ("prefix".to_owned(), Value::String(binding.prefix.clone())),
        (
            "broker_token".to_owned(),
            Value::String(binding.broker_token.clone()),
        ),
    ]);
    write_json(
        &path,
        &value,
        JsonWriteOptions {
            mode: Some(0o600),
            ..JsonWriteOptions::default()
        },
    )
    .map_err(|_| BackupError::HostedWrite)
}

pub fn delete_hosted_binding(journal: &Path) -> Result<(), BackupError> {
    let path = hosted_binding_path(journal).map_err(|_| BackupError::HostedDelete)?;
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(_) => Err(BackupError::HostedDelete),
    }
}

fn non_blank(raw: &Map<String, Value>, key: &str) -> Option<String> {
    raw.get(key)
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .map(str::to_owned)
}

fn read_binding_bytes(path: &Path) -> std::io::Result<Vec<u8>> {
    #[cfg(test)]
    if let Some(source) = HOSTED_READ_SOURCE.with(|source| source.borrow().clone()) {
        return source(path);
    }
    fs::read(path)
}

#[cfg(test)]
use std::{cell::RefCell, rc::Rc};
#[cfg(test)]
type HostedReadSource = Rc<dyn Fn(&Path) -> std::io::Result<Vec<u8>>>;
#[cfg(test)]
thread_local! {
    static HOSTED_READ_SOURCE: RefCell<Option<HostedReadSource>> = const { RefCell::new(None) };
}
#[cfg(test)]
struct HostedReadGuard(Option<HostedReadSource>);
#[cfg(test)]
impl Drop for HostedReadGuard {
    fn drop(&mut self) {
        HOSTED_READ_SOURCE.with(|source| *source.borrow_mut() = self.0.take());
    }
}
#[cfg(test)]
fn install_hosted_read_source(source: HostedReadSource) -> HostedReadGuard {
    HostedReadGuard(HOSTED_READ_SOURCE.with(|current| current.replace(Some(source))))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn temp() -> tempfile::TempDir {
        tempfile::tempdir().unwrap()
    }
    fn binding() -> HostedBinding {
        HostedBinding {
            broker_endpoint: "https://b".into(),
            account_id: "a".into(),
            instance_id: "i".into(),
            bucket: "bucket".into(),
            prefix: "prefix".into(),
            broker_token: "secret".into(),
        }
    }

    #[test]
    fn read_has_no_side_effect_and_invalid_is_unbound() {
        let journal = temp();
        assert_eq!(load_hosted_binding(journal.path()), None);
        let _guard = install_hosted_read_source(Rc::new(|_| {
            Err(std::io::Error::from(std::io::ErrorKind::PermissionDenied))
        }));
        assert_eq!(load_hosted_binding(journal.path()), None);
        assert!(!journal.path().join("backup").exists());
        fs::create_dir_all(journal.path().join("backup/hosted")).unwrap();
        fs::write(journal.path().join("backup/hosted/binding.json"), b"{").unwrap();
        assert_eq!(load_hosted_binding(journal.path()), None);
    }

    #[test]
    fn malformed_binding_shapes_are_unbound_without_rewriting_bytes() {
        let journal = temp();
        let path = journal.path().join("backup/hosted/binding.json");
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        for contents in [
            b"[]".as_slice(),
            br#""x""#.as_slice(),
            b"3".as_slice(),
            br#"{"broker_endpoint":"b"}"#.as_slice(),
            br#"{"broker_endpoint":" ","account_id":"a","instance_id":"i","bucket":"bucket","prefix":"prefix","broker_token":"token"}"#.as_slice(),
        ] {
            fs::write(&path, contents).unwrap();
            let before = fs::read(&path).unwrap();
            assert_eq!(load_hosted_binding(journal.path()), None);
            assert!(path.exists());
            assert_eq!(fs::read(&path).unwrap(), before);
        }
    }

    #[test]
    fn save_and_delete_have_expected_side_effects() {
        let journal = temp();
        let value = binding();
        save_hosted_binding(journal.path(), &value).unwrap();
        let path = journal.path().join("backup/hosted/binding.json");
        assert_eq!(load_hosted_binding(journal.path()), Some(value));
        #[cfg(unix)]
        assert_eq!(
            std::os::unix::fs::MetadataExt::mode(&fs::metadata(&path).unwrap()) & 0o777,
            0o600
        );
        delete_hosted_binding(journal.path()).unwrap();
        delete_hosted_binding(journal.path()).unwrap();
        assert!(!path.exists());
    }

    #[cfg(unix)]
    #[test]
    fn save_failure_preserves_existing_binding_bytes() {
        use std::os::unix::fs::PermissionsExt;

        let journal = temp();
        let prior = binding();
        save_hosted_binding(journal.path(), &prior).unwrap();
        let path = journal.path().join("backup/hosted/binding.json");
        let before = fs::read(&path).unwrap();
        let directory = path.parent().unwrap();
        fs::set_permissions(directory, fs::Permissions::from_mode(0o500)).unwrap();
        let result = save_hosted_binding(
            journal.path(),
            &HostedBinding {
                bucket: "new-bucket".into(),
                ..prior
            },
        );
        fs::set_permissions(directory, fs::Permissions::from_mode(0o700)).unwrap();
        assert!(result.is_err());
        assert_eq!(fs::read(&path).unwrap(), before);
    }

    #[test]
    fn delete_missing_binding_creates_hosted_directory() {
        let journal = temp();
        delete_hosted_binding(journal.path()).unwrap();
        assert!(journal.path().join("backup/hosted").is_dir());
    }

    #[test]
    fn delete_surfaces_directory_and_unlink_failures() {
        let directory_failure = temp();
        fs::write(directory_failure.path().join("backup"), b"not a directory").unwrap();
        assert!(delete_hosted_binding(directory_failure.path()).is_err());

        let unlink_failure = temp();
        fs::create_dir_all(unlink_failure.path().join("backup/hosted/binding.json")).unwrap();
        assert!(delete_hosted_binding(unlink_failure.path()).is_err());
    }
}
