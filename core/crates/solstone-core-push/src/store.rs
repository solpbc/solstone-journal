// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::collections::BTreeMap;
use std::error::Error;
use std::fmt;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use solstone_core_convey_http::identity::LinkedDeviceCid;
use solstone_core_journal_io::{
    AtomicWriteError, JsonWriteOptions, LockError, LockOptions, hold_lock, write_json,
};
use time::{OffsetDateTime, UtcOffset, format_description::well_known::Rfc3339};

use crate::model::{PushDeviceStatus, PushEnvironment, PushPlatform};

const REGISTRY_FILE: &str = "push-registry.json";
const OWNER_ONLY_MODE: u32 = 0o600;

/// Durable device-registration store owned by this crate.
#[derive(Clone, Debug)]
pub(crate) struct PushRegistry {
    path: PathBuf,
}

impl PushRegistry {
    pub(crate) fn new(journal_root: impl AsRef<Path>) -> Self {
        Self {
            path: journal_root.as_ref().join("config").join(REGISTRY_FILE),
        }
    }

    #[cfg(test)]
    pub(crate) fn path(&self) -> &Path {
        &self.path
    }

    pub(crate) fn register(
        &self,
        cid: &LinkedDeviceCid,
        device_token: String,
        bundle_id: String,
        environment: PushEnvironment,
        platform: PushPlatform,
    ) -> Result<(), PushStoreError> {
        let _lock = hold_lock(
            &self.path,
            LockOptions {
                mode: Some(OWNER_ONLY_MODE),
                ..LockOptions::default()
            },
        )
        .map_err(PushStoreError::Lock)?;
        let mut registry = self.read_registry()?;
        let cid = cid.as_str();

        registry.devices.retain(|existing_cid, existing| {
            existing_cid != cid && existing.device_token != device_token
        });
        registry.devices.insert(
            cid.to_owned(),
            StoredDevice {
                device_token,
                bundle_id,
                environment,
                platform,
                registered_at: now_rfc3339_utc()?,
            },
        );
        self.write_registry(&registry)
    }

    pub(crate) fn deregister(&self, cid: &LinkedDeviceCid) -> Result<bool, PushStoreError> {
        let _lock = hold_lock(
            &self.path,
            LockOptions {
                mode: Some(OWNER_ONLY_MODE),
                ..LockOptions::default()
            },
        )
        .map_err(PushStoreError::Lock)?;
        let mut registry = self.read_registry()?;
        let removed = registry.devices.remove(cid.as_str()).is_some();
        if removed {
            self.write_registry(&registry)?;
        }
        Ok(removed)
    }

    pub(crate) fn status(&self) -> Result<Vec<PushDeviceStatus>, PushStoreError> {
        let registry = self.read_registry()?;
        let mut devices = registry
            .devices
            .into_values()
            .map(PushDeviceStatus::from)
            .collect::<Vec<_>>();
        devices.sort_by(|left, right| {
            parse_registered_at(&right.registered_at)
                .expect("registry validation checked timestamp")
                .cmp(
                    &parse_registered_at(&left.registered_at)
                        .expect("registry validation checked timestamp"),
                )
                .then_with(|| left.device_token.cmp(&right.device_token))
        });
        Ok(devices)
    }

    pub(crate) fn device_count(&self) -> Result<usize, PushStoreError> {
        Ok(self.read_registry()?.devices.len())
    }

    fn read_registry(&self) -> Result<Registry, PushStoreError> {
        let contents = match fs::read_to_string(&self.path) {
            Ok(contents) => contents,
            Err(source) if source.kind() == io::ErrorKind::NotFound => {
                return Ok(Registry::default());
            }
            Err(source) => {
                return Err(PushStoreError::Read {
                    path: self.path.clone(),
                    source,
                });
            }
        };
        let registry = serde_json::from_str::<Registry>(&contents).map_err(|source| {
            PushStoreError::Parse {
                path: self.path.clone(),
                source,
            }
        })?;
        validate_registry(&self.path, &registry)?;
        Ok(registry)
    }

    fn write_registry(&self, registry: &Registry) -> Result<(), PushStoreError> {
        write_json(
            &self.path,
            registry,
            JsonWriteOptions {
                mode: Some(OWNER_ONLY_MODE),
                ..JsonWriteOptions::default()
            },
        )
        .map_err(PushStoreError::Write)
    }
}

#[derive(Debug)]
pub(crate) enum PushStoreError {
    Lock(LockError),
    Read {
        path: PathBuf,
        source: io::Error,
    },
    Parse {
        path: PathBuf,
        source: serde_json::Error,
    },
    InvalidRegistry {
        path: PathBuf,
        detail: String,
    },
    Write(AtomicWriteError),
    Clock,
}

impl fmt::Display for PushStoreError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Lock(error) => error.fmt(formatter),
            Self::Read { path, source } => write!(formatter, "{}: {source}", path.display()),
            Self::Parse { path, source } => write!(formatter, "{}: {source}", path.display()),
            Self::InvalidRegistry { path, detail } => {
                write!(
                    formatter,
                    "invalid push registry {}: {detail}",
                    path.display()
                )
            }
            Self::Write(error) => error.fmt(formatter),
            Self::Clock => formatter.write_str("could not format push registration timestamp"),
        }
    }
}

impl Error for PushStoreError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Lock(error) => Some(error),
            Self::Read { source, .. } => Some(source),
            Self::Parse { source, .. } => Some(source),
            Self::Write(error) => Some(error),
            Self::InvalidRegistry { .. } | Self::Clock => None,
        }
    }
}

#[derive(Debug, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Registry {
    devices: BTreeMap<String, StoredDevice>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct StoredDevice {
    device_token: String,
    bundle_id: String,
    environment: PushEnvironment,
    platform: PushPlatform,
    registered_at: String,
}

impl From<StoredDevice> for PushDeviceStatus {
    fn from(device: StoredDevice) -> Self {
        Self {
            bundle_id: device.bundle_id,
            environment: device.environment,
            platform: device.platform,
            registered_at: device.registered_at,
            device_token: mask_token(&device.device_token),
        }
    }
}

fn validate_registry(path: &Path, registry: &Registry) -> Result<(), PushStoreError> {
    for (cid, device) in &registry.devices {
        LinkedDeviceCid::try_from(cid.as_str()).map_err(|_| PushStoreError::InvalidRegistry {
            path: path.to_path_buf(),
            detail: format!("invalid linked-device CID {cid:?}"),
        })?;
        if device.device_token.trim().is_empty() {
            return Err(invalid_registry(path, "device_token must not be blank"));
        }
        if device.bundle_id.trim().is_empty() {
            return Err(invalid_registry(path, "bundle_id must not be blank"));
        }
        parse_registered_at(&device.registered_at).ok_or_else(|| {
            invalid_registry(
                path,
                "registered_at must be an RFC3339 UTC timestamp ending in Z",
            )
        })?;
    }
    Ok(())
}

fn invalid_registry(path: &Path, detail: impl Into<String>) -> PushStoreError {
    PushStoreError::InvalidRegistry {
        path: path.to_path_buf(),
        detail: detail.into(),
    }
}

fn now_rfc3339_utc() -> Result<String, PushStoreError> {
    OffsetDateTime::now_utc()
        .format(&Rfc3339)
        .map_err(|_| PushStoreError::Clock)
}

fn parse_registered_at(value: &str) -> Option<OffsetDateTime> {
    value
        .ends_with('Z')
        .then(|| OffsetDateTime::parse(value, &Rfc3339).ok())
        .flatten()
        .filter(|timestamp| timestamp.offset() == UtcOffset::UTC)
}

fn mask_token(token: &str) -> String {
    let suffix = token
        .chars()
        .rev()
        .take(4)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect::<String>();
    format!("...{suffix}")
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::sync::{Arc, Barrier};
    use std::thread;

    use serde_json::{Value, json};
    use solstone_core_convey_http::identity::LinkedDeviceCid;
    use tempfile::TempDir;

    use super::{PushEnvironment, PushPlatform, PushRegistry};

    const CID_A: &str = "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    const CID_B: &str = "sha256:bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";

    fn cid(value: &str) -> LinkedDeviceCid {
        LinkedDeviceCid::try_from(value).expect("fixture cid")
    }

    fn registry(root: &TempDir) -> PushRegistry {
        PushRegistry::new(root.path())
    }

    fn register(registry: &PushRegistry, cid: &str, token: &str, bundle_id: &str) {
        registry
            .register(
                &super::LinkedDeviceCid::try_from(cid).expect("fixture cid"),
                token.to_owned(),
                bundle_id.to_owned(),
                PushEnvironment::Development,
                PushPlatform::Ios,
            )
            .expect("register device");
    }

    #[test]
    fn register_creates_the_fresh_registry_with_exact_values() {
        let root = TempDir::new_in("/var/tmp").expect("journal root");
        let registry = registry(&root);
        register(&registry, CID_A, " Token AbCd ", " org.example.push ");

        assert!(!root.path().join("config/push_devices.json").exists());
        let value: Value =
            serde_json::from_slice(&fs::read(registry.path()).expect("registry bytes"))
                .expect("registry JSON");
        let row = &value["devices"][CID_A];
        assert_eq!(row["device_token"], " Token AbCd ");
        assert_eq!(row["bundle_id"], " org.example.push ");
        assert_eq!(row["environment"], "development");
        assert_eq!(row["platform"], "ios");
        assert!(
            row["registered_at"]
                .as_str()
                .expect("timestamp")
                .ends_with('Z')
        );
    }

    #[test]
    fn reregister_replaces_same_identity_and_token_steal_drops_old_identity() {
        let root = TempDir::new_in("/var/tmp").expect("journal root");
        let registry = registry(&root);
        register(&registry, CID_A, "first", "org.example.first");
        register(&registry, CID_A, "second", "org.example.second");
        assert_eq!(registry.device_count().unwrap(), 1);
        let value: Value = serde_json::from_slice(&fs::read(registry.path()).unwrap()).unwrap();
        assert_eq!(value["devices"][CID_A]["device_token"], "second");
        assert_eq!(value["devices"][CID_A]["bundle_id"], "org.example.second");

        register(&registry, CID_B, "second", "org.example.stolen");
        assert_eq!(registry.device_count().unwrap(), 1);
        let value: Value = serde_json::from_slice(&fs::read(registry.path()).unwrap()).unwrap();
        assert_eq!(value["devices"][CID_A], Value::Null);
        assert_eq!(value["devices"][CID_B]["device_token"], "second");
    }

    #[test]
    fn concurrent_same_identity_registers_leave_one_row() {
        let root = TempDir::new_in("/var/tmp").expect("journal root");
        let registry = Arc::new(registry(&root));
        let barrier = Arc::new(Barrier::new(3));
        let mut workers = Vec::new();
        for token in ["first", "second"] {
            let registry = Arc::clone(&registry);
            let barrier = Arc::clone(&barrier);
            workers.push(thread::spawn(move || {
                barrier.wait();
                register(&registry, CID_A, token, "org.example.concurrent");
            }));
        }
        barrier.wait();
        for worker in workers {
            worker.join().expect("worker");
        }

        assert_eq!(registry.device_count().unwrap(), 1);
        let value: Value = serde_json::from_slice(&fs::read(registry.path()).unwrap()).unwrap();
        assert!(matches!(
            value["devices"][CID_A]["device_token"].as_str(),
            Some("first" | "second")
        ));
    }

    #[test]
    fn deregister_is_idempotent_and_persists_across_reopen() {
        let root = TempDir::new_in("/var/tmp").expect("journal root");
        let registry = registry(&root);
        register(&registry, CID_A, "token", "org.example");
        let reopened = PushRegistry::new(root.path());
        assert_eq!(reopened.device_count().unwrap(), 1);
        assert!(reopened.deregister(&cid(CID_A)).unwrap());
        assert!(!reopened.deregister(&cid(CID_A)).unwrap());
    }

    #[test]
    fn status_masks_exact_token_and_orders_newest_first() {
        let root = TempDir::new_in("/var/tmp").expect("journal root");
        let registry = registry(&root);
        fs::create_dir_all(root.path().join("config")).unwrap();
        fs::write(
            registry.path(),
            serde_json::to_vec(&json!({
                "devices": {
                    CID_A: {
                        "device_token": "first-token",
                        "bundle_id": "org.example.first",
                        "environment": "development",
                        "platform": "ios",
                        "registered_at": "2026-08-27T10:00:00Z"
                    },
                    CID_B: {
                        "device_token": " Token AbCd ",
                        "bundle_id": " org.example.latest ",
                        "environment": "production",
                        "platform": "ios",
                        "registered_at": "2026-08-27T11:00:00Z"
                    }
                }
            }))
            .unwrap(),
        )
        .unwrap();

        let status = registry.status().unwrap();
        assert_eq!(status.len(), 2);
        assert_eq!(status[0].bundle_id, " org.example.latest ");
        assert_eq!(status[0].device_token, "...bCd ");
        assert_eq!(status[1].device_token, "...oken");
    }

    #[test]
    fn malformed_registry_is_not_treated_as_empty_or_overwritten() {
        let root = TempDir::new_in("/var/tmp").expect("journal root");
        let registry = registry(&root);
        fs::create_dir_all(root.path().join("config")).unwrap();
        fs::write(registry.path(), b"not JSON").unwrap();
        let before = fs::read(registry.path()).unwrap();

        assert!(registry.status().is_err());
        assert!(
            registry
                .register(
                    &cid(CID_A),
                    "token".to_owned(),
                    "org.example".to_owned(),
                    PushEnvironment::Development,
                    PushPlatform::Ios,
                )
                .is_err()
        );
        assert_eq!(fs::read(registry.path()).unwrap(), before);
    }
}
