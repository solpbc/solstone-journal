// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

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

use crate::envelope::PushKey;
use crate::model::{
    PushDeviceItem, PushEnvironment, PushPlatform, device_token_is_valid, mask_target,
};

const REGISTRY_FILE: &str = "push-registry.json";
const OWNER_ONLY_MODE: u32 = 0o600;

#[cfg(test)]
thread_local! {
    static TEST_CLOCK: std::cell::RefCell<Option<OffsetDateTime>> = const { std::cell::RefCell::new(None) };
}

#[cfg(test)]
pub(crate) fn set_test_clock(time: Option<OffsetDateTime>) {
    TEST_CLOCK.with(|c| *c.borrow_mut() = time);
}

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
        push_key: PushKey,
    ) -> Result<(bool, PushDeviceItem), PushStoreError> {
        match platform {
            PushPlatform::Ios => {}
        }
        let _lock = hold_lock(
            &self.path,
            LockOptions {
                mode: Some(OWNER_ONLY_MODE),
                ..LockOptions::default()
            },
        )
        .map_err(PushStoreError::Lock)?;

        let loaded = self.read_registry()?;
        let mut devices = loaded.registry.devices;
        let cid_str = cid.as_str();

        // Check if this pair existed
        let existing_index = devices.iter().position(|d| match d {
            StoredDevice::Ios {
                cid: row_cid,
                device_token: row_token,
                ..
            } => row_cid == cid_str && row_token == &device_token,
        });
        let is_created = existing_index.is_none();

        // Steal: drop any other row with this token on another CID
        devices.retain(|d| match d {
            StoredDevice::Ios {
                cid: row_cid,
                device_token: row_token,
                ..
            } => row_token != &device_token || row_cid == cid_str,
        });

        let registered_at = now_rfc3339_utc()?;
        let target = mask_target(&device_token);

        let new_device = StoredDevice::Ios {
            cid: cid_str.to_owned(),
            device_token,
            bundle_id,
            environment,
            push_key,
            registered_at: registered_at.clone(),
        };

        if let Some(idx) = devices.iter().position(|d| match d {
            StoredDevice::Ios {
                cid: row_cid,
                device_token: row_token,
                ..
            } => {
                row_cid == cid_str
                    && row_token
                        == match &new_device {
                            StoredDevice::Ios { device_token, .. } => device_token,
                        }
            }
        }) {
            devices[idx] = new_device;
        } else {
            devices.push(new_device);
        }

        // Sort rows by (cid, device_token)
        devices.sort_by(|a, b| match (a, b) {
            (
                StoredDevice::Ios {
                    cid: c1,
                    device_token: t1,
                    ..
                },
                StoredDevice::Ios {
                    cid: c2,
                    device_token: t2,
                    ..
                },
            ) => c1.cmp(c2).then_with(|| t1.cmp(t2)),
        });

        if let Some(n) = loaded.legacy_discarded
            && n >= 1
        {
            log::info!("discarded {n} legacy push registrations");
        }

        let registry = RegistryV2 {
            version: 2,
            devices,
        };
        self.write_registry(&registry)?;

        Ok((
            is_created,
            PushDeviceItem {
                platform: PushPlatform::Ios,
                target,
                environment,
                registered_at,
            },
        ))
    }

    pub(crate) fn deregister(
        &self,
        cid: &LinkedDeviceCid,
        platform: PushPlatform,
        device_token: &str,
    ) -> Result<bool, PushStoreError> {
        match platform {
            PushPlatform::Ios => {}
        }
        let _lock = hold_lock(
            &self.path,
            LockOptions {
                mode: Some(OWNER_ONLY_MODE),
                ..LockOptions::default()
            },
        )
        .map_err(PushStoreError::Lock)?;

        let loaded = self.read_registry()?;
        let mut devices = loaded.registry.devices;
        let cid_str = cid.as_str();

        let initial_len = devices.len();
        devices.retain(|d| match d {
            StoredDevice::Ios {
                cid: row_cid,
                device_token: row_token,
                ..
            } => !(row_cid == cid_str && row_token == device_token),
        });

        let removed = devices.len() != initial_len;
        if removed {
            devices.sort_by(|a, b| match (a, b) {
                (
                    StoredDevice::Ios {
                        cid: c1,
                        device_token: t1,
                        ..
                    },
                    StoredDevice::Ios {
                        cid: c2,
                        device_token: t2,
                        ..
                    },
                ) => c1.cmp(c2).then_with(|| t1.cmp(t2)),
            });

            let registry = RegistryV2 {
                version: 2,
                devices,
            };
            self.write_registry(&registry)?;
        }
        Ok(removed)
    }

    pub(crate) fn status(&self) -> Result<(Vec<PushDeviceItem>, usize), PushStoreError> {
        let loaded = self.read_registry()?;
        let mut items = loaded
            .registry
            .devices
            .into_iter()
            .map(|device| match device {
                StoredDevice::Ios {
                    cid,
                    device_token,
                    environment,
                    registered_at,
                    ..
                } => {
                    let parsed_time = parse_registered_at(&registered_at).ok_or_else(|| {
                        PushStoreError::InvalidRegistry {
                            path: self.path.clone(),
                            detail: "registered_at",
                        }
                    })?;
                    let target = mask_target(&device_token);
                    Ok((
                        parsed_time,
                        target.clone(),
                        cid,
                        PushDeviceItem {
                            platform: PushPlatform::Ios,
                            target,
                            environment,
                            registered_at,
                        },
                    ))
                }
            })
            .collect::<Result<Vec<_>, PushStoreError>>()?;

        items.sort_by(
            |(time_a, target_a, cid_a, _), (time_b, target_b, cid_b, _)| {
                time_b
                    .cmp(time_a)
                    .then_with(|| target_a.cmp(target_b))
                    .then_with(|| cid_a.cmp(cid_b))
            },
        );

        let result_items: Vec<PushDeviceItem> =
            items.into_iter().map(|(_, _, _, item)| item).collect();
        let total = result_items.len();
        Ok((result_items, total))
    }

    pub(crate) fn device_count(&self) -> Result<usize, PushStoreError> {
        Ok(self.read_registry()?.registry.devices.len())
    }

    fn read_registry(&self) -> Result<LoadedRegistry, PushStoreError> {
        let contents = match fs::read_to_string(&self.path) {
            Ok(contents) => contents,
            Err(source) if source.kind() == io::ErrorKind::NotFound => {
                return Ok(LoadedRegistry {
                    registry: RegistryV2::default(),
                    legacy_discarded: None,
                });
            }
            Err(source) => {
                return Err(PushStoreError::Read {
                    path: self.path.clone(),
                    source,
                });
            }
        };

        let raw_value: serde_json::Value =
            serde_json::from_str(&contents).map_err(|_| PushStoreError::Parse {
                path: self.path.clone(),
            })?;

        // Check for v1 shape
        if let Some(legacy_count) = check_v1_shape(&raw_value) {
            return Ok(LoadedRegistry {
                registry: RegistryV2::default(),
                legacy_discarded: Some(legacy_count),
            });
        }

        // Deserialize v2
        let registry: RegistryV2 =
            serde_json::from_value(raw_value).map_err(|_| PushStoreError::Parse {
                path: self.path.clone(),
            })?;

        validate_registry_v2(&self.path, &registry)?;
        Ok(LoadedRegistry {
            registry,
            legacy_discarded: None,
        })
    }

    fn write_registry(&self, registry: &RegistryV2) -> Result<(), PushStoreError> {
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

struct LoadedRegistry {
    registry: RegistryV2,
    legacy_discarded: Option<usize>,
}

fn check_v1_shape(val: &serde_json::Value) -> Option<usize> {
    let obj = val.as_object()?;
    if obj.len() != 1 || !obj.contains_key("devices") {
        return None;
    }
    let devices_obj = obj.get("devices")?.as_object()?;
    for (_cid, device_val) in devices_obj {
        let dev_obj = device_val.as_object()?;
        if dev_obj.len() != 5 {
            return None;
        }
        for field in [
            "device_token",
            "bundle_id",
            "environment",
            "platform",
            "registered_at",
        ] {
            if !dev_obj.get(field).is_some_and(serde_json::Value::is_string) {
                return None;
            }
        }
    }
    Some(devices_obj.len())
}

#[derive(Debug)]
pub(crate) enum PushStoreError {
    Lock(LockError),
    Read { path: PathBuf, source: io::Error },
    Parse { path: PathBuf },
    InvalidRegistry { path: PathBuf, detail: &'static str },
    Write(AtomicWriteError),
    Clock,
}

impl fmt::Display for PushStoreError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Lock(error) => error.fmt(formatter),
            Self::Read { path, source } => write!(formatter, "{}: {source}", path.display()),
            Self::Parse { path } => {
                write!(formatter, "{}: invalid push registry JSON", path.display())
            }
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
            Self::Write(error) => Some(error),
            Self::Parse { .. } | Self::InvalidRegistry { .. } | Self::Clock => None,
        }
    }
}

#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct RegistryV2 {
    version: u32,
    #[serde(default)]
    devices: Vec<StoredDevice>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(tag = "platform", deny_unknown_fields)]
#[serde(rename_all = "snake_case")]
pub(crate) enum StoredDevice {
    Ios {
        cid: String,
        device_token: String,
        bundle_id: String,
        environment: PushEnvironment,
        push_key: PushKey,
        registered_at: String,
    },
}

fn validate_registry_v2(path: &Path, registry: &RegistryV2) -> Result<(), PushStoreError> {
    if registry.version != 2 {
        return Err(PushStoreError::InvalidRegistry {
            path: path.to_path_buf(),
            detail: "version",
        });
    }

    let mut seen_pairs = std::collections::HashSet::new();
    let mut seen_tokens = std::collections::HashMap::new();

    for device in &registry.devices {
        match device {
            StoredDevice::Ios {
                cid,
                device_token,
                bundle_id,
                registered_at,
                ..
            } => {
                LinkedDeviceCid::try_from(cid.as_str()).map_err(|_| {
                    PushStoreError::InvalidRegistry {
                        path: path.to_path_buf(),
                        detail: "cid",
                    }
                })?;
                if !device_token_is_valid(device_token) {
                    return Err(PushStoreError::InvalidRegistry {
                        path: path.to_path_buf(),
                        detail: "device_token",
                    });
                }
                if bundle_id.trim().is_empty() {
                    return Err(PushStoreError::InvalidRegistry {
                        path: path.to_path_buf(),
                        detail: "bundle_id",
                    });
                }
                parse_registered_at(registered_at).ok_or(PushStoreError::InvalidRegistry {
                    path: path.to_path_buf(),
                    detail: "registered_at",
                })?;

                if !seen_pairs.insert((cid.clone(), device_token.clone())) {
                    return Err(PushStoreError::InvalidRegistry {
                        path: path.to_path_buf(),
                        detail: "device_token",
                    });
                }

                if let Some(existing_cid) = seen_tokens.insert(device_token.clone(), cid.clone())
                    && existing_cid != *cid
                {
                    return Err(PushStoreError::InvalidRegistry {
                        path: path.to_path_buf(),
                        detail: "device_token",
                    });
                }
            }
        }
    }
    Ok(())
}

fn now_rfc3339_utc() -> Result<String, PushStoreError> {
    #[cfg(test)]
    let now = TEST_CLOCK.with(|c| c.borrow().unwrap_or_else(OffsetDateTime::now_utc));
    #[cfg(not(test))]
    let now = OffsetDateTime::now_utc();

    now.format(&Rfc3339).map_err(|_| PushStoreError::Clock)
}

fn parse_registered_at(value: &str) -> Option<OffsetDateTime> {
    value
        .ends_with('Z')
        .then(|| OffsetDateTime::parse(value, &Rfc3339).ok())
        .flatten()
        .filter(|timestamp| timestamp.offset() == UtcOffset::UTC)
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::sync::{Arc, Barrier};
    use std::thread;

    use solstone_core_convey_http::identity::LinkedDeviceCid;
    use tempfile::TempDir;

    use super::*;

    const CID_A: &str = "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    const TOKEN_1: &str = "0123456789abcdef";
    const TOKEN_2: &str = "fedcba9876543210";
    const VALID_KEY: [u8; 32] = [42u8; 32];

    fn cid(value: &str) -> LinkedDeviceCid {
        LinkedDeviceCid::try_from(value).expect("fixture cid")
    }

    fn registry(root: &TempDir) -> PushRegistry {
        PushRegistry::new(root.path())
    }

    fn key() -> PushKey {
        PushKey::from_bytes(VALID_KEY)
    }

    #[test]
    fn concurrent_registers_maintain_distinct_and_deduplicate_same() {
        let root = TempDir::new_in("/var/tmp").expect("journal root");
        set_test_clock(None);
        let reg = Arc::new(registry(&root));

        // Two different valid tokens for one CID both remain
        reg.register(
            &cid(CID_A),
            TOKEN_1.to_owned(),
            "org.example".to_owned(),
            PushEnvironment::Development,
            PushPlatform::Ios,
            key(),
        )
        .unwrap();
        reg.register(
            &cid(CID_A),
            TOKEN_2.to_owned(),
            "org.example".to_owned(),
            PushEnvironment::Development,
            PushPlatform::Ios,
            key(),
        )
        .unwrap();
        assert_eq!(reg.device_count().unwrap(), 2);
        assert!(!root.path().join("config/push_devices.json").exists());

        // Same token across threads collapses to one row
        let barrier = Arc::new(Barrier::new(3));
        let mut workers = Vec::new();
        for _ in 0..2 {
            let r = Arc::clone(&reg);
            let b = Arc::clone(&barrier);
            workers.push(thread::spawn(move || {
                b.wait();
                let _ = r.register(
                    &LinkedDeviceCid::try_from(CID_A).unwrap(),
                    TOKEN_1.to_owned(),
                    "org.example.concurrent".to_owned(),
                    PushEnvironment::Development,
                    PushPlatform::Ios,
                    PushKey::from_bytes(VALID_KEY),
                );
            }));
        }
        barrier.wait();
        for worker in workers {
            worker.join().expect("worker");
        }

        assert_eq!(reg.device_count().unwrap(), 2);
    }

    #[test]
    fn deregister_is_token_scoped_and_persists_across_reopen() {
        let root = TempDir::new_in("/var/tmp").expect("journal root");
        set_test_clock(None);
        let reg = registry(&root);
        reg.register(
            &cid(CID_A),
            TOKEN_1.to_owned(),
            "org.example".to_owned(),
            PushEnvironment::Development,
            PushPlatform::Ios,
            key(),
        )
        .unwrap();
        reg.register(
            &cid(CID_A),
            TOKEN_2.to_owned(),
            "org.example".to_owned(),
            PushEnvironment::Development,
            PushPlatform::Ios,
            key(),
        )
        .unwrap();
        assert_eq!(reg.device_count().unwrap(), 2);

        let reopened = PushRegistry::new(root.path());
        assert!(
            reopened
                .deregister(&cid(CID_A), PushPlatform::Ios, TOKEN_1)
                .unwrap()
        );
        assert_eq!(reopened.device_count().unwrap(), 1);
        assert!(
            !reopened
                .deregister(&cid(CID_A), PushPlatform::Ios, TOKEN_1)
                .unwrap()
        );
        assert_eq!(reopened.device_count().unwrap(), 1);
    }

    #[test]
    fn malformed_file_is_unavailable_and_unmodified() {
        let root = TempDir::new_in("/var/tmp").expect("journal root");
        set_test_clock(None);
        let reg = registry(&root);
        fs::create_dir_all(root.path().join("config")).unwrap();
        fs::write(reg.path(), b"not JSON").unwrap();
        let before = fs::read(reg.path()).unwrap();

        assert!(reg.status().is_err());
        assert!(
            reg.register(
                &cid(CID_A),
                TOKEN_1.to_owned(),
                "org.example".to_owned(),
                PushEnvironment::Development,
                PushPlatform::Ios,
                key(),
            )
            .is_err()
        );
        assert_eq!(fs::read(reg.path()).unwrap(), before);
    }
}
