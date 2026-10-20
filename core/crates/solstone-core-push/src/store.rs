// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::error::Error;
use std::fmt;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use base64::Engine as _;
use serde::{Deserialize, Serialize};
use solstone_core_convey_http::identity::LinkedDeviceCid;
use solstone_core_journal_io::{
    AtomicWriteError, JsonWriteOptions, LockError, LockOptions, hold_lock, write_json,
};
use solstone_core_sol_link::ledger::{AuthorizedClientsRead, read_authorized_clients};
use time::{Duration, OffsetDateTime, UtcOffset, format_description::well_known::Rfc3339};

use crate::capability::{
    CapabilityDocument, CapabilityGrant, CapabilityReadStatus, grant_matches,
    read_capability_document, write_capability_document,
};
use crate::envelope::PushKey;
use crate::model::{
    PushDeviceItem, PushEnvironment, PushPlatform, device_token_is_valid, mask_target,
};

const REGISTRY_FILE: &str = "push-registry.json";
const OWNER_ONLY_MODE: u32 = 0o600;

#[cfg(test)]
thread_local! {
    pub(crate) static TEST_CLOCK: std::cell::RefCell<Option<OffsetDateTime>> = const { std::cell::RefCell::new(None) };
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

    pub(crate) fn register_ios(
        &self,
        cid: &LinkedDeviceCid,
        device_token: String,
        bundle_id: String,
        environment: PushEnvironment,
        push_key: PushKey,
        present: bool,
    ) -> Result<(bool, PushDeviceItem), PushStoreError> {
        let _lock = hold_lock(
            &self.path,
            LockOptions {
                mode: Some(OWNER_ONLY_MODE),
                ..LockOptions::default()
            },
        )
        .map_err(PushStoreError::Lock)?;

        let _authorization_lock = self.hold_authorized_cid(cid)?;

        let loaded = self.read_registry()?;
        let mut devices = loaded.registry.devices;
        let cid_str = cid.as_str();

        let existing_index = devices.iter().position(|d| match d {
            StoredDevice::Ios {
                cid: row_cid,
                device_token: row_token,
                ..
            } => row_cid == cid_str && row_token == &device_token,
            _ => false,
        });
        let is_created = existing_index.is_none();

        let mut removed_identities =
            vec![(PushPlatform::Ios, cid_str.to_string(), device_token.clone())];

        // Steal: drop any other row with this token on another CID
        devices.retain(|d| match d {
            StoredDevice::Ios {
                cid: row_cid,
                device_token: row_token,
                ..
            } if row_token == &device_token && row_cid != cid_str => {
                removed_identities.push((PushPlatform::Ios, row_cid.clone(), row_token.clone()));
                false
            }
            _ => true,
        });

        let registry_timestamps: Vec<&str> = devices
            .iter()
            .map(|d| match d {
                StoredDevice::Ios { registered_at, .. }
                | StoredDevice::Android { registered_at, .. } => registered_at.as_str(),
            })
            .collect();

        let natural_registered_at = now_rfc3339_utc()?;
        let target = mask_target(&device_token);

        let cap_path = self.path.with_file_name("push-capabilities.json");
        let cap_outcome = prepare_capabilities_for_register(
            &cap_path,
            &removed_identities,
            &registry_timestamps,
            PushPlatform::Ios,
            cid_str,
            &device_token,
            &push_key,
            &natural_registered_at,
            present,
        );
        let registered_at = cap_outcome.effective_registered_at.clone();

        let new_device = StoredDevice::Ios {
            cid: cid_str.to_owned(),
            device_token: device_token.clone(),
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
            } => row_cid == cid_str && row_token == &device_token,
            _ => false,
        }) {
            devices[idx] = new_device;
        } else {
            devices.push(new_device);
        }

        sort_devices(&mut devices);

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

        let new_grant = if present {
            Some(CapabilityGrant {
                platform: PushPlatform::Ios,
                cid: cid_str.to_owned(),
                identity: device_token,
                push_key,
                registered_at: registered_at.clone(),
            })
        } else {
            None
        };

        finish_register_capabilities(&cap_path, cap_outcome, new_grant);

        Ok((
            is_created,
            PushDeviceItem {
                platform: PushPlatform::Ios,
                target,
                environment: Some(environment),
                registered_at,
            },
        ))
    }

    pub(crate) fn register_android(
        &self,
        cid: &LinkedDeviceCid,
        endpoint: String,
        p256dh: String,
        auth: String,
        push_key: PushKey,
        present: bool,
    ) -> Result<(bool, PushDeviceItem), PushStoreError> {
        let _lock = hold_lock(
            &self.path,
            LockOptions {
                mode: Some(OWNER_ONLY_MODE),
                ..LockOptions::default()
            },
        )
        .map_err(PushStoreError::Lock)?;

        let _authorization_lock = self.hold_authorized_cid(cid)?;

        let loaded = self.read_registry()?;
        let mut devices = loaded.registry.devices;
        let cid_str = cid.as_str();

        let existing_index = devices.iter().position(|d| match d {
            StoredDevice::Android {
                cid: row_cid,
                endpoint: row_endpoint,
                ..
            } => row_cid == cid_str && row_endpoint == &endpoint,
            _ => false,
        });
        let is_created = existing_index.is_none();

        let mut removed_identities =
            vec![(PushPlatform::Android, cid_str.to_string(), endpoint.clone())];

        // Steal: drop any other row with this endpoint on another CID
        devices.retain(|d| match d {
            StoredDevice::Android {
                cid: row_cid,
                endpoint: row_endpoint,
                ..
            } if row_endpoint == &endpoint && row_cid != cid_str => {
                removed_identities.push((
                    PushPlatform::Android,
                    row_cid.clone(),
                    row_endpoint.clone(),
                ));
                false
            }
            _ => true,
        });

        let registry_timestamps: Vec<&str> = devices
            .iter()
            .map(|d| match d {
                StoredDevice::Ios { registered_at, .. }
                | StoredDevice::Android { registered_at, .. } => registered_at.as_str(),
            })
            .collect();

        let natural_registered_at = now_rfc3339_utc()?;
        let target = crate::endpoint::endpoint_target(&endpoint)
            .map(|(host, _)| host)
            .unwrap_or_else(|| "invalid".to_owned());

        let cap_path = self.path.with_file_name("push-capabilities.json");
        let cap_outcome = prepare_capabilities_for_register(
            &cap_path,
            &removed_identities,
            &registry_timestamps,
            PushPlatform::Android,
            cid_str,
            &endpoint,
            &push_key,
            &natural_registered_at,
            present,
        );
        let registered_at = cap_outcome.effective_registered_at.clone();

        let new_device = StoredDevice::Android {
            cid: cid_str.to_owned(),
            endpoint: endpoint.clone(),
            p256dh,
            auth,
            push_key,
            registered_at: registered_at.clone(),
        };

        if let Some(idx) = devices.iter().position(|d| match d {
            StoredDevice::Android {
                cid: row_cid,
                endpoint: row_endpoint,
                ..
            } => row_cid == cid_str && row_endpoint == &endpoint,
            _ => false,
        }) {
            devices[idx] = new_device;
        } else {
            devices.push(new_device);
        }

        sort_devices(&mut devices);

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

        let new_grant = if present {
            Some(CapabilityGrant {
                platform: PushPlatform::Android,
                cid: cid_str.to_owned(),
                identity: endpoint,
                push_key,
                registered_at: registered_at.clone(),
            })
        } else {
            None
        };

        finish_register_capabilities(&cap_path, cap_outcome, new_grant);

        Ok((
            is_created,
            PushDeviceItem {
                platform: PushPlatform::Android,
                target,
                environment: None,
                registered_at,
            },
        ))
    }

    pub(crate) fn deregister_ios(
        &self,
        cid: &LinkedDeviceCid,
        device_token: &str,
    ) -> Result<bool, PushStoreError> {
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
            _ => true,
        });

        let removed = devices.len() != initial_len;
        if removed {
            sort_devices(&mut devices);

            let registry = RegistryV2 {
                version: 2,
                devices,
            };
            let cap_path = self.path.with_file_name("push-capabilities.json");
            let removed_identities = [(
                PushPlatform::Ios,
                cid_str.to_string(),
                device_token.to_string(),
            )];
            handle_deregister_capabilities(&cap_path, &removed_identities, || {
                self.write_registry(&registry)
            })?;
        }
        Ok(removed)
    }

    pub(crate) fn deregister_android(
        &self,
        cid: &LinkedDeviceCid,
        endpoint: &str,
    ) -> Result<bool, PushStoreError> {
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
            StoredDevice::Android {
                cid: row_cid,
                endpoint: row_endpoint,
                ..
            } => !(row_cid == cid_str && row_endpoint == endpoint),
            _ => true,
        });

        let removed = devices.len() != initial_len;
        if removed {
            sort_devices(&mut devices);

            let registry = RegistryV2 {
                version: 2,
                devices,
            };
            let cap_path = self.path.with_file_name("push-capabilities.json");
            let removed_identities = [(
                PushPlatform::Android,
                cid_str.to_string(),
                endpoint.to_string(),
            )];
            handle_deregister_capabilities(&cap_path, &removed_identities, || {
                self.write_registry(&registry)
            })?;
        }
        Ok(removed)
    }

    pub(crate) fn remove_cid(&self, cid: &str) -> Result<bool, PushStoreError> {
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

        let initial_len = devices.len();
        let mut removed_identities = Vec::new();
        devices.retain(|d| match d {
            StoredDevice::Ios {
                cid: row_cid,
                device_token,
                ..
            } => {
                if row_cid == cid {
                    removed_identities.push((
                        PushPlatform::Ios,
                        row_cid.clone(),
                        device_token.clone(),
                    ));
                    false
                } else {
                    true
                }
            }
            StoredDevice::Android {
                cid: row_cid,
                endpoint,
                ..
            } => {
                if row_cid == cid {
                    removed_identities.push((
                        PushPlatform::Android,
                        row_cid.clone(),
                        endpoint.clone(),
                    ));
                    false
                } else {
                    true
                }
            }
        });

        let removed = devices.len() != initial_len;
        if removed {
            sort_devices(&mut devices);
            let registry = RegistryV2 {
                version: 2,
                devices,
            };
            let cap_path = self.path.with_file_name("push-capabilities.json");
            handle_deregister_capabilities(&cap_path, &removed_identities, || {
                self.write_registry(&registry)
            })?;
        }
        Ok(removed)
    }

    pub(crate) fn read_devices_locked(
        &self,
    ) -> Result<(Vec<StoredDevice>, Vec<CapabilityGrant>), PushStoreError> {
        let _lock = hold_lock(
            &self.path,
            LockOptions {
                mode: Some(OWNER_ONLY_MODE),
                ..LockOptions::default()
            },
        )
        .map_err(PushStoreError::Lock)?;

        let loaded = self.read_registry()?;
        let cap_path = self.path.with_file_name("push-capabilities.json");
        let grants = match read_capability_document(&cap_path) {
            CapabilityReadStatus::Ready(g) => g,
            CapabilityReadStatus::Missing
            | CapabilityReadStatus::Malformed
            | CapabilityReadStatus::Unreadable => Vec::new(),
        };
        Ok((loaded.registry.devices, grants))
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
                            environment: Some(environment),
                            registered_at,
                        },
                    ))
                }
                StoredDevice::Android {
                    cid,
                    endpoint,
                    registered_at,
                    ..
                } => {
                    let parsed_time = parse_registered_at(&registered_at).ok_or_else(|| {
                        PushStoreError::InvalidRegistry {
                            path: self.path.clone(),
                            detail: "registered_at",
                        }
                    })?;
                    let target = crate::endpoint::endpoint_target(&endpoint)
                        .map(|(host, _)| host)
                        .unwrap_or_else(|| "invalid".to_owned());
                    Ok((
                        parsed_time,
                        target.clone(),
                        cid,
                        PushDeviceItem {
                            platform: PushPlatform::Android,
                            target,
                            environment: None,
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

    #[cfg(test)]
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

    fn hold_authorized_cid(
        &self,
        cid: &LinkedDeviceCid,
    ) -> Result<solstone_core_journal_io::FileLock, PushStoreError> {
        let journal = self
            .path
            .parent()
            .and_then(Path::parent)
            .ok_or(PushStoreError::AuthorizationUnavailable)?;
        let path = journal.join("link").join("authorized_clients.json");
        let lock = hold_lock(&path, LockOptions::default()).map_err(PushStoreError::Lock)?;
        match read_authorized_clients(&path) {
            AuthorizedClientsRead::Present(entries)
                if entries
                    .iter()
                    .any(|entry| entry.fingerprint == cid.as_str()) =>
            {
                Ok(lock)
            }
            AuthorizedClientsRead::Present(_) | AuthorizedClientsRead::Missing => {
                Err(PushStoreError::Unauthorized)
            }
            AuthorizedClientsRead::Unreadable
            | AuthorizedClientsRead::Malformed
            | AuthorizedClientsRead::DuplicateCid => Err(PushStoreError::AuthorizationUnavailable),
        }
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

fn disambiguate_registered_at(
    natural_registered_at: &str,
    registry_timestamps: &[&str],
    known_grants: &[CapabilityGrant],
) -> String {
    let mut occupied: std::collections::HashSet<&str> = std::collections::HashSet::new();
    occupied.insert(natural_registered_at);
    for &ts in registry_timestamps {
        occupied.insert(ts);
    }
    for grant in known_grants {
        occupied.insert(&grant.registered_at);
    }

    let natural_instant =
        parse_registered_at(natural_registered_at).unwrap_or_else(OffsetDateTime::now_utc);
    let mut max_instant = natural_instant;
    for &ts in &occupied {
        if let Some(parsed) = parse_registered_at(ts)
            && parsed > max_instant
        {
            max_instant = parsed;
        }
    }

    for step in 1..=10_000i64 {
        let candidate = max_instant + Duration::nanoseconds(step);
        if let Ok(mut formatted) = candidate.format(&Rfc3339) {
            if formatted.ends_with("+00:00") {
                formatted.truncate(formatted.len() - 6);
                formatted.push('Z');
            }
            if formatted.ends_with('Z')
                && !occupied.contains(formatted.as_str())
                && parse_registered_at(&formatted).is_some()
            {
                return formatted;
            }
        }
    }

    for step in 1..=120i64 {
        let candidate = max_instant + Duration::seconds(step);
        if let Ok(mut formatted) = candidate.format(&Rfc3339) {
            if formatted.ends_with("+00:00") {
                formatted.truncate(formatted.len() - 6);
                formatted.push('Z');
            }
            if formatted.ends_with('Z')
                && !occupied.contains(formatted.as_str())
                && parse_registered_at(&formatted).is_some()
            {
                return formatted;
            }
        }
    }

    let candidate = max_instant + Duration::seconds(121);
    if let Ok(mut formatted) = candidate.format(&Rfc3339) {
        if formatted.ends_with("+00:00") {
            formatted.truncate(formatted.len() - 6);
            formatted.push('Z');
        }
        if formatted.ends_with('Z')
            && !occupied.contains(formatted.as_str())
            && parse_registered_at(&formatted).is_some()
        {
            return formatted;
        }
    }

    let fallback = OffsetDateTime::now_utc() + Duration::seconds(122);
    let mut formatted = fallback.format(&Rfc3339).unwrap();
    if formatted.ends_with("+00:00") {
        formatted.truncate(formatted.len() - 6);
        formatted.push('Z');
    }
    formatted
}

struct CapRegistrationOutcome {
    effective_registered_at: String,
    should_publish: bool,
    grants_to_publish: Option<Vec<CapabilityGrant>>,
    is_malformed: bool,
}

#[allow(clippy::too_many_arguments)]
fn prepare_capabilities_for_register(
    cap_path: &Path,
    removed_identities: &[(PushPlatform, String, String)],
    registry_timestamps: &[&str],
    platform: PushPlatform,
    cid: &str,
    identity: &str,
    push_key: &PushKey,
    natural_registered_at: &str,
    present: bool,
) -> CapRegistrationOutcome {
    let read_status = read_capability_document(cap_path);
    match read_status {
        CapabilityReadStatus::Malformed => CapRegistrationOutcome {
            effective_registered_at: natural_registered_at.to_string(),
            should_publish: true,
            grants_to_publish: Some(Vec::new()),
            is_malformed: true,
        },
        CapabilityReadStatus::Unreadable => {
            let empty_doc = CapabilityDocument {
                version: 1,
                grants: Vec::new(),
            };
            let effective_registered_at = if write_capability_document(cap_path, empty_doc).is_ok()
            {
                natural_registered_at.to_string()
            } else {
                disambiguate_registered_at(natural_registered_at, registry_timestamps, &[])
            };
            CapRegistrationOutcome {
                effective_registered_at,
                should_publish: false,
                grants_to_publish: None,
                is_malformed: false,
            }
        }
        CapabilityReadStatus::Missing => prepare_from_base(
            cap_path,
            removed_identities,
            registry_timestamps,
            platform,
            cid,
            identity,
            push_key,
            natural_registered_at,
            present,
            Vec::new(),
        ),
        CapabilityReadStatus::Ready(base) => prepare_from_base(
            cap_path,
            removed_identities,
            registry_timestamps,
            platform,
            cid,
            identity,
            push_key,
            natural_registered_at,
            present,
            base,
        ),
    }
}

#[allow(clippy::too_many_arguments)]
fn prepare_from_base(
    cap_path: &Path,
    removed_identities: &[(PushPlatform, String, String)],
    registry_timestamps: &[&str],
    platform: PushPlatform,
    cid: &str,
    identity: &str,
    push_key: &PushKey,
    natural_registered_at: &str,
    present: bool,
    base: Vec<CapabilityGrant>,
) -> CapRegistrationOutcome {
    let cleared: Vec<CapabilityGrant> = base
        .iter()
        .filter(|g| {
            !removed_identities
                .iter()
                .any(|(p, c, id)| g.platform == *p && &g.cid == c && &g.identity == id)
        })
        .cloned()
        .collect();

    if cleared != base {
        let write_res = write_capability_document(
            cap_path,
            CapabilityDocument {
                version: 1,
                grants: cleared.clone(),
            },
        );
        if write_res.is_ok() {
            CapRegistrationOutcome {
                effective_registered_at: natural_registered_at.to_string(),
                should_publish: present,
                grants_to_publish: Some(cleared),
                is_malformed: false,
            }
        } else {
            let collides = base.iter().any(|g| {
                grant_matches(g, platform, cid, identity, push_key, natural_registered_at)
            });
            let effective_registered_at = if collides {
                disambiguate_registered_at(natural_registered_at, registry_timestamps, &base)
            } else {
                natural_registered_at.to_string()
            };
            CapRegistrationOutcome {
                effective_registered_at,
                should_publish: false,
                grants_to_publish: None,
                is_malformed: false,
            }
        }
    } else {
        CapRegistrationOutcome {
            effective_registered_at: natural_registered_at.to_string(),
            should_publish: present,
            grants_to_publish: Some(cleared),
            is_malformed: false,
        }
    }
}

fn finish_register_capabilities(
    cap_path: &Path,
    outcome: CapRegistrationOutcome,
    new_grant: Option<CapabilityGrant>,
) {
    if outcome.is_malformed {
        let grants = if let Some(grant) = new_grant {
            vec![grant]
        } else {
            Vec::new()
        };
        let _ = write_capability_document(cap_path, CapabilityDocument { version: 1, grants });
    } else if let (true, Some(mut grants), Some(grant)) =
        (outcome.should_publish, outcome.grants_to_publish, new_grant)
    {
        grants.push(grant);
        let _ = write_capability_document(cap_path, CapabilityDocument { version: 1, grants });
    }
}

fn handle_deregister_capabilities(
    cap_path: &Path,
    removed_identities: &[(PushPlatform, String, String)],
    write_registry: impl FnOnce() -> Result<(), PushStoreError>,
) -> Result<(), PushStoreError> {
    let read_status = read_capability_document(cap_path);
    let base = match read_status {
        CapabilityReadStatus::Ready(g) => g,
        CapabilityReadStatus::Missing => Vec::new(),
        CapabilityReadStatus::Malformed | CapabilityReadStatus::Unreadable => {
            write_registry()?;
            let _ = write_capability_document(
                cap_path,
                CapabilityDocument {
                    version: 1,
                    grants: Vec::new(),
                },
            );
            return Ok(());
        }
    };

    let cleared: Vec<CapabilityGrant> = base
        .iter()
        .filter(|g| {
            !removed_identities
                .iter()
                .any(|(p, c, id)| g.platform == *p && &g.cid == c && &g.identity == id)
        })
        .cloned()
        .collect();

    if cleared != base {
        let _ = write_capability_document(
            cap_path,
            CapabilityDocument {
                version: 1,
                grants: cleared,
            },
        );
    }
    write_registry()
}

/// Remove all registered push devices associated with the specified linked device CID.
pub fn remove_cid_registrations(journal_root: &Path, cid: &str) -> Result<(), PushStoreError> {
    let registry = PushRegistry::new(journal_root);
    registry.remove_cid(cid)?;
    Ok(())
}

#[derive(Debug)]
pub enum PushStoreError {
    Lock(LockError),
    Read { path: PathBuf, source: io::Error },
    Parse { path: PathBuf },
    InvalidRegistry { path: PathBuf, detail: &'static str },
    Write(AtomicWriteError),
    Unauthorized,
    AuthorizationUnavailable,
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
            Self::Unauthorized => formatter.write_str("linked device is not authorized"),
            Self::AuthorizationUnavailable => {
                formatter.write_str("authorization ledger is unavailable")
            }
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
            Self::Parse { .. }
            | Self::InvalidRegistry { .. }
            | Self::Unauthorized
            | Self::AuthorizationUnavailable
            | Self::Clock => None,
        }
    }
}

#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(crate) struct RegistryV2 {
    pub(crate) version: u32,
    #[serde(default)]
    pub(crate) devices: Vec<StoredDevice>,
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
    Android {
        cid: String,
        endpoint: String,
        p256dh: String,
        auth: String,
        push_key: PushKey,
        registered_at: String,
    },
}

impl StoredDevice {
    #[cfg(test)]
    pub(crate) fn cid(&self) -> &str {
        match self {
            Self::Ios { cid, .. } => cid,
            Self::Android { cid, .. } => cid,
        }
    }
}

fn sort_devices(devices: &mut [StoredDevice]) {
    devices.sort_by(|a, b| {
        let rank_a = match a {
            StoredDevice::Ios { .. } => 0,
            StoredDevice::Android { .. } => 1,
        };
        let rank_b = match b {
            StoredDevice::Ios { .. } => 0,
            StoredDevice::Android { .. } => 1,
        };
        let cid_a = match a {
            StoredDevice::Ios { cid, .. } => cid.as_str(),
            StoredDevice::Android { cid, .. } => cid.as_str(),
        };
        let cid_b = match b {
            StoredDevice::Ios { cid, .. } => cid.as_str(),
            StoredDevice::Android { cid, .. } => cid.as_str(),
        };
        let ident_a = match a {
            StoredDevice::Ios { device_token, .. } => device_token.as_str(),
            StoredDevice::Android { endpoint, .. } => endpoint.as_str(),
        };
        let ident_b = match b {
            StoredDevice::Ios { device_token, .. } => device_token.as_str(),
            StoredDevice::Android { endpoint, .. } => endpoint.as_str(),
        };

        rank_a
            .cmp(&rank_b)
            .then_with(|| cid_a.cmp(cid_b))
            .then_with(|| ident_a.cmp(ident_b))
    });
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
            StoredDevice::Android {
                cid,
                endpoint,
                p256dh,
                auth,
                registered_at,
                ..
            } => {
                LinkedDeviceCid::try_from(cid.as_str()).map_err(|_| {
                    PushStoreError::InvalidRegistry {
                        path: path.to_path_buf(),
                        detail: "cid",
                    }
                })?;
                if !crate::endpoint::endpoint_stored_shape_ok(endpoint) {
                    return Err(PushStoreError::InvalidRegistry {
                        path: path.to_path_buf(),
                        detail: "endpoint",
                    });
                }
                if p256dh.len() != 87 {
                    return Err(PushStoreError::InvalidRegistry {
                        path: path.to_path_buf(),
                        detail: "p256dh",
                    });
                }
                let p256dh_bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
                    .decode(p256dh)
                    .map_err(|_| PushStoreError::InvalidRegistry {
                        path: path.to_path_buf(),
                        detail: "p256dh",
                    })?;
                if p256dh_bytes.len() != 65 || p256dh_bytes[0] != 0x04 {
                    return Err(PushStoreError::InvalidRegistry {
                        path: path.to_path_buf(),
                        detail: "p256dh",
                    });
                }
                if auth.len() != 22 {
                    return Err(PushStoreError::InvalidRegistry {
                        path: path.to_path_buf(),
                        detail: "auth",
                    });
                }
                let auth_bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
                    .decode(auth)
                    .map_err(|_| PushStoreError::InvalidRegistry {
                        path: path.to_path_buf(),
                        detail: "auth",
                    })?;
                if auth_bytes.len() != 16 {
                    return Err(PushStoreError::InvalidRegistry {
                        path: path.to_path_buf(),
                        detail: "auth",
                    });
                }
                parse_registered_at(registered_at).ok_or(PushStoreError::InvalidRegistry {
                    path: path.to_path_buf(),
                    detail: "registered_at",
                })?;

                if !seen_pairs.insert((cid.clone(), endpoint.clone())) {
                    return Err(PushStoreError::InvalidRegistry {
                        path: path.to_path_buf(),
                        detail: "endpoint",
                    });
                }

                if let Some(existing_cid) = seen_tokens.insert(endpoint.clone(), cid.clone())
                    && existing_cid != *cid
                {
                    return Err(PushStoreError::InvalidRegistry {
                        path: path.to_path_buf(),
                        detail: "endpoint",
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

pub(crate) fn parse_registered_at(value: &str) -> Option<OffsetDateTime> {
    value
        .ends_with('Z')
        .then(|| OffsetDateTime::parse(value, &Rfc3339).ok())
        .flatten()
        .filter(|timestamp| timestamp.offset() == UtcOffset::UTC)
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::sync::{Arc, Barrier, mpsc};
    use std::thread;

    use solstone_core_convey_http::identity::LinkedDeviceCid;
    use solstone_core_sol_link::ledger::{AuthorizationLedger, ClientEntry, ClientRole};
    use tempfile::TempDir;

    use super::*;

    const CID_A: &str = "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    const CID_B: &str = "sha256:bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
    const TOKEN_1: &str = "0123456789abcdef";
    const TOKEN_2: &str = "fedcba9876543210";
    const VALID_KEY: [u8; 32] = [42u8; 32];

    fn cid(value: &str) -> LinkedDeviceCid {
        LinkedDeviceCid::try_from(value).expect("fixture cid")
    }

    fn registry(root: &TempDir) -> PushRegistry {
        let mut ledger = AuthorizationLedger::new(root.path());
        for value in [CID_A, CID_B] {
            if !matches!(
                read_authorized_clients(&root.path().join("link/authorized_clients.json")),
                AuthorizedClientsRead::Present(ref entries)
                    if entries.iter().any(|entry| entry.fingerprint == value)
            ) {
                ledger
                    .add(ClientEntry::new(
                        value,
                        "test",
                        "2026-01-01T00:00:00Z",
                        "test-instance",
                        ClientRole::Roleless,
                    ))
                    .expect("authorized test device");
            }
        }
        PushRegistry::new(root.path())
    }

    fn key() -> PushKey {
        PushKey::from_bytes(VALID_KEY)
    }

    #[test]
    fn registration_refuses_revoked_cid_for_both_platforms() {
        let root = TempDir::new_in("/var/tmp").expect("journal root");
        let reg = registry(&root);
        AuthorizationLedger::new(root.path())
            .remove(CID_A)
            .expect("revoke test device");

        assert!(matches!(
            reg.register_ios(
                &cid(CID_A),
                TOKEN_1.to_owned(),
                "org.example".to_owned(),
                PushEnvironment::Development,
                key(),
                false,
            ),
            Err(PushStoreError::Unauthorized)
        ));
        assert!(matches!(
            reg.register_android(
                &cid(CID_A),
                ENDPOINT_1.to_owned(),
                ANDROID_P256DH.to_owned(),
                ANDROID_AUTH.to_owned(),
                key(),
                false,
            ),
            Err(PushStoreError::Unauthorized)
        ));
        assert!(!reg.path().exists());
    }

    #[test]
    fn concurrent_registration_rechecks_authorization_after_waiting_for_push_lock() {
        let root = TempDir::new_in("/var/tmp").expect("journal root");
        let reg = registry(&root);
        let push_lock = hold_lock(reg.path(), LockOptions::default()).unwrap();
        let journal = root.path().to_path_buf();
        let worker_registry = reg.clone();
        let (started_tx, started_rx) = mpsc::channel();
        let worker = thread::spawn(move || {
            started_tx.send(()).unwrap();
            worker_registry.register_ios(
                &cid(CID_A),
                TOKEN_1.to_owned(),
                "org.example".to_owned(),
                PushEnvironment::Development,
                key(),
                false,
            )
        });
        started_rx.recv().unwrap();
        AuthorizationLedger::new(&journal)
            .remove(CID_A)
            .expect("concurrent revocation");
        drop(push_lock);

        assert!(matches!(
            worker.join().unwrap(),
            Err(PushStoreError::Unauthorized)
        ));
        assert!(!reg.path().exists());
    }

    const ANDROID_P256DH: &str =
        "BCVxsr7N_eNgVRqvHtD0zTZsEc6-VV-JvLexhqUzORcxaOzi6-AYWXvTBHm4bjyPjs7Vd8pZGH6SRpkNtoIAiw4";
    const ANDROID_AUTH: &str = "BTBZMqHH6r4Tts7J_aSIgg";
    const ENDPOINT_1: &str = "https://push.example.com/sub/1";
    const ENDPOINT_2: &str = "https://push.example.com/sub/2";

    #[test]
    fn remove_cid_registrations_removes_all_tokens_for_cid_and_preserves_others() {
        let root = TempDir::new_in("/var/tmp").expect("journal root");
        set_test_clock(None);
        let reg = registry(&root);
        let key_b = PushKey::from_bytes([99u8; 32]);
        reg.register_ios(
            &cid(CID_A),
            TOKEN_1.to_owned(),
            "org.example".to_owned(),
            PushEnvironment::Development,
            key(),
            false,
        )
        .unwrap();
        reg.register_ios(
            &cid(CID_A),
            TOKEN_2.to_owned(),
            "org.example".to_owned(),
            PushEnvironment::Development,
            key(),
            false,
        )
        .unwrap();
        reg.register_android(
            &cid(CID_A),
            ENDPOINT_1.to_owned(),
            ANDROID_P256DH.to_owned(),
            ANDROID_AUTH.to_owned(),
            key(),
            false,
        )
        .unwrap();
        reg.register_ios(
            &cid(CID_B),
            "1122334455667788".to_owned(),
            "org.example".to_owned(),
            PushEnvironment::Development,
            key_b,
            false,
        )
        .unwrap();
        reg.register_android(
            &cid(CID_B),
            ENDPOINT_2.to_owned(),
            ANDROID_P256DH.to_owned(),
            ANDROID_AUTH.to_owned(),
            key_b,
            false,
        )
        .unwrap();
        assert_eq!(reg.device_count().unwrap(), 5);

        remove_cid_registrations(root.path(), CID_A).unwrap();
        assert_eq!(reg.device_count().unwrap(), 2);

        let (devices, _) = reg.read_devices_locked().unwrap();
        assert_eq!(devices.len(), 2);
        assert_eq!(devices[0].cid(), CID_B);
        assert_eq!(devices[1].cid(), CID_B);

        // Idempotent / missing CID does not error
        remove_cid_registrations(
            root.path(),
            "sha256:cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc",
        )
        .unwrap();
        assert_eq!(reg.device_count().unwrap(), 2);
    }

    #[test]
    fn concurrent_registers_maintain_distinct_and_deduplicate_same() {
        let root = TempDir::new_in("/var/tmp").expect("journal root");
        set_test_clock(None);
        let reg = Arc::new(registry(&root));

        // Two different valid tokens for one CID both remain
        reg.register_ios(
            &cid(CID_A),
            TOKEN_1.to_owned(),
            "org.example".to_owned(),
            PushEnvironment::Development,
            key(),
            false,
        )
        .unwrap();
        reg.register_ios(
            &cid(CID_A),
            TOKEN_2.to_owned(),
            "org.example".to_owned(),
            PushEnvironment::Development,
            key(),
            false,
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
                let _ = r.register_ios(
                    &LinkedDeviceCid::try_from(CID_A).unwrap(),
                    TOKEN_1.to_owned(),
                    "org.example.concurrent".to_owned(),
                    PushEnvironment::Development,
                    PushKey::from_bytes(VALID_KEY),
                    false,
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
        reg.register_ios(
            &cid(CID_A),
            TOKEN_1.to_owned(),
            "org.example".to_owned(),
            PushEnvironment::Development,
            key(),
            false,
        )
        .unwrap();
        reg.register_ios(
            &cid(CID_A),
            TOKEN_2.to_owned(),
            "org.example".to_owned(),
            PushEnvironment::Development,
            key(),
            false,
        )
        .unwrap();
        reg.register_android(
            &cid(CID_A),
            ENDPOINT_1.to_owned(),
            ANDROID_P256DH.to_owned(),
            ANDROID_AUTH.to_owned(),
            key(),
            false,
        )
        .unwrap();
        assert_eq!(reg.device_count().unwrap(), 3);

        let reopened = PushRegistry::new(root.path());
        assert!(reopened.deregister_ios(&cid(CID_A), TOKEN_1).unwrap());
        assert_eq!(reopened.device_count().unwrap(), 2);
        assert!(!reopened.deregister_ios(&cid(CID_A), TOKEN_1).unwrap());
        assert_eq!(reopened.device_count().unwrap(), 2);

        assert!(
            reopened
                .deregister_android(&cid(CID_A), ENDPOINT_1)
                .unwrap()
        );
        assert_eq!(reopened.device_count().unwrap(), 1);
        assert!(
            !reopened
                .deregister_android(&cid(CID_A), ENDPOINT_1)
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
            reg.register_ios(
                &cid(CID_A),
                TOKEN_1.to_owned(),
                "org.example".to_owned(),
                PushEnvironment::Development,
                key(),
                false,
            )
            .is_err()
        );
        assert_eq!(fs::read(reg.path()).unwrap(), before);
    }

    #[test]
    fn frozen_clock_headerless_reregister_drops_grant() {
        let root = TempDir::new_in("/var/tmp").expect("journal root");
        let t = OffsetDateTime::from_unix_timestamp(1_790_208_000).unwrap();
        set_test_clock(Some(t));
        let reg = registry(&root);

        // Register with present = true
        let (created, item1) = reg
            .register_ios(
                &cid(CID_A),
                TOKEN_1.to_owned(),
                "org.example".to_owned(),
                PushEnvironment::Development,
                key(),
                true,
            )
            .expect("register ios");
        assert!(created);

        let (devices, grants) = reg.read_devices_locked().expect("read locked");
        assert_eq!(devices.len(), 1);
        assert_eq!(grants.len(), 1);
        let StoredDevice::Ios {
            registered_at: ref reg_at_1,
            ..
        } = devices[0]
        else {
            panic!("expected ios device");
        };
        assert!(grant_matches(
            &grants[0],
            PushPlatform::Ios,
            CID_A,
            TOKEN_1,
            &key(),
            reg_at_1,
        ));

        // Re-register headerless with frozen clock, same key and identity
        let (created2, item2) = reg
            .register_ios(
                &cid(CID_A),
                TOKEN_1.to_owned(),
                "org.example".to_owned(),
                PushEnvironment::Development,
                key(),
                false,
            )
            .expect("re-register ios");
        assert!(!created2);
        assert_eq!(item1.registered_at, item2.registered_at);

        let (devices2, grants2) = reg.read_devices_locked().expect("read locked");
        assert_eq!(devices2.len(), 1);
        assert!(grants2.is_empty());
        let StoredDevice::Ios {
            registered_at: ref reg_at_2,
            ..
        } = devices2[0]
        else {
            panic!("expected ios device");
        };
        assert_eq!(reg_at_1, reg_at_2);
        assert!(!grants2.iter().any(|g| grant_matches(
            g,
            PushPlatform::Ios,
            CID_A,
            TOKEN_1,
            &key(),
            reg_at_2,
        )));

        // Registry file still deserializes cleanly as RegistryV2
        let file_contents = fs::read_to_string(reg.path()).expect("read registry");
        let parsed_reg: RegistryV2 =
            serde_json::from_str(&file_contents).expect("deserialize registry");
        assert_eq!(parsed_reg.devices.len(), 1);

        set_test_clock(None);
    }

    #[test]
    fn replaced_key_changed_identity_and_steal_do_not_match_old_grant() {
        let root = TempDir::new_in("/var/tmp").expect("journal root");
        set_test_clock(None);
        let reg = registry(&root);

        // Register initial row with opt-in
        reg.register_ios(
            &cid(CID_A),
            TOKEN_1.to_owned(),
            "org.example".to_owned(),
            PushEnvironment::Development,
            key(),
            true,
        )
        .unwrap();

        let (_devices, grants) = reg.read_devices_locked().unwrap();
        assert_eq!(grants.len(), 1);

        // 1. New push key without opt-in leaves grant unmatched
        let new_key = PushKey::from_bytes([88u8; 32]);
        reg.register_ios(
            &cid(CID_A),
            TOKEN_1.to_owned(),
            "org.example".to_owned(),
            PushEnvironment::Development,
            new_key,
            false,
        )
        .unwrap();
        let (devices_after_key, grants_after_key) = reg.read_devices_locked().unwrap();
        assert!(grants_after_key.is_empty());
        let StoredDevice::Ios {
            ref push_key,
            ref registered_at,
            ..
        } = devices_after_key[0]
        else {
            panic!();
        };
        assert_eq!(push_key, &new_key);
        assert!(!grants_after_key.iter().any(|g| grant_matches(
            g,
            PushPlatform::Ios,
            CID_A,
            TOKEN_1,
            push_key,
            registered_at,
        )));

        // 2. Steal onto another CID drops stolen-from row and leaves grant unmatched
        reg.register_ios(
            &cid(CID_A),
            TOKEN_1.to_owned(),
            "org.example".to_owned(),
            PushEnvironment::Development,
            key(),
            true,
        )
        .unwrap();

        // CID_B steals TOKEN_1
        reg.register_ios(
            &cid(CID_B),
            TOKEN_1.to_owned(),
            "org.example.b".to_owned(),
            PushEnvironment::Development,
            key(),
            false,
        )
        .unwrap();

        let (devices_after_steal, grants_after_steal) = reg.read_devices_locked().unwrap();
        // Stolen-from row on CID_A is gone
        assert_eq!(devices_after_steal.len(), 1);
        assert_eq!(devices_after_steal[0].cid(), CID_B);
        // Grant for CID_A's token is gone
        assert!(!grants_after_steal.iter().any(|g| g.cid == CID_A));
        assert!(grants_after_steal.is_empty());
    }

    #[test]
    fn deregister_and_remove_cid_drop_grants() {
        let root = TempDir::new_in("/var/tmp").expect("journal root");
        set_test_clock(None);
        let reg = registry(&root);

        // Register iOS and Android with opt-in
        reg.register_ios(
            &cid(CID_A),
            TOKEN_1.to_owned(),
            "org.example".to_owned(),
            PushEnvironment::Development,
            key(),
            true,
        )
        .unwrap();
        reg.register_android(
            &cid(CID_A),
            ENDPOINT_1.to_owned(),
            ANDROID_P256DH.to_owned(),
            ANDROID_AUTH.to_owned(),
            key(),
            true,
        )
        .unwrap();

        let (_, grants) = reg.read_devices_locked().unwrap();
        assert_eq!(grants.len(), 2);

        // Deregister iOS removes its grant
        assert!(reg.deregister_ios(&cid(CID_A), TOKEN_1).unwrap());
        let (_, grants_after_ios) = reg.read_devices_locked().unwrap();
        assert_eq!(grants_after_ios.len(), 1);
        assert_eq!(grants_after_ios[0].identity, ENDPOINT_1);

        // Deregister Android removes its grant
        assert!(reg.deregister_android(&cid(CID_A), ENDPOINT_1).unwrap());
        let (_, grants_after_android) = reg.read_devices_locked().unwrap();
        assert!(grants_after_android.is_empty());

        // Register multiple across CID_A and CID_B
        reg.register_ios(
            &cid(CID_A),
            TOKEN_1.to_owned(),
            "org.example".to_owned(),
            PushEnvironment::Development,
            key(),
            true,
        )
        .unwrap();
        reg.register_ios(
            &cid(CID_B),
            TOKEN_2.to_owned(),
            "org.example".to_owned(),
            PushEnvironment::Development,
            key(),
            true,
        )
        .unwrap();
        let (_, grants_before_remove) = reg.read_devices_locked().unwrap();
        assert_eq!(grants_before_remove.len(), 2);

        // remove_cid_registrations on CID_A drops CID_A grants and keeps CID_B
        remove_cid_registrations(root.path(), CID_A).unwrap();
        let (devices_after_remove, grants_after_remove) = reg.read_devices_locked().unwrap();
        assert_eq!(devices_after_remove.len(), 1);
        assert_eq!(devices_after_remove[0].cid(), CID_B);
        assert_eq!(grants_after_remove.len(), 1);
        assert_eq!(grants_after_remove[0].cid, CID_B);
    }

    #[test]
    fn stale_grant_and_expired_row_are_not_capable() {
        let root = TempDir::new_in("/var/tmp").expect("journal root");
        let t_old = OffsetDateTime::from_unix_timestamp(1_700_000_000).unwrap();
        let t_now = OffsetDateTime::from_unix_timestamp(1_790_208_000).unwrap();
        set_test_clock(Some(t_old));
        let reg = registry(&root);

        reg.register_ios(
            &cid(CID_A),
            TOKEN_1.to_owned(),
            "org.example".to_owned(),
            PushEnvironment::Development,
            key(),
            true,
        )
        .unwrap();

        // 1. Stale grant timestamp does not match
        let (devices, grants) = reg.read_devices_locked().unwrap();
        assert_eq!(grants.len(), 1);
        let grant_stale = CapabilityGrant {
            registered_at: "2020-01-01T00:00:00Z".to_owned(),
            ..grants[0].clone()
        };
        let StoredDevice::Ios {
            ref registered_at, ..
        } = devices[0]
        else {
            panic!()
        };
        assert!(!grant_matches(
            &grant_stale,
            PushPlatform::Ios,
            CID_A,
            TOKEN_1,
            &key(),
            registered_at,
        ));

        // 2. Row older than 30 days is filtered out by deliver_notification cutoff
        let notif = crate::envelope::Notification {
            at: t_now,
            kind: "test".to_owned(),
            title: "solstone".to_owned(),
            body: "test".to_owned(),
            open: None,
        };
        let outcome = crate::router::deliver_notification(
            root.path(),
            "https://portal.example.com",
            t_now,
            &notif,
            &crate::relay::UreqRelay::default(),
            None,
        );
        // Because row was registered > 30 days ago, deliver_notification finds no eligible recipient
        assert!(matches!(
            outcome,
            crate::router::DeliveryOutcome::Refused(crate::router::Refused::NoRecipient)
        ));

        set_test_clock(None);
    }

    #[test]
    fn capability_write_fault_on_frozen_clock_reregister_keeps_english_row() {
        let root = TempDir::new_in("/var/tmp").expect("journal root");
        let t = OffsetDateTime::from_unix_timestamp(1_790_208_000).unwrap();
        set_test_clock(Some(t));
        let reg = registry(&root);

        // Opt in at a frozen TEST_CLOCK
        let (created, item1) = reg
            .register_ios(
                &cid(CID_A),
                TOKEN_1.to_owned(),
                "org.example".to_owned(),
                PushEnvironment::Development,
                key(),
                true,
            )
            .expect("initial opt-in succeeds");
        assert!(created);
        assert_eq!(item1.registered_at, "2026-09-24T00:00:00Z");

        // Set write hook to return Err for every document
        crate::capability::set_test_write_hook(Some(Box::new(|_| {
            Err(io::Error::other("write hook fault"))
        })));

        // Headerless re-register with the same key and identity returns Ok
        let (created2, item2) = reg
            .register_ios(
                &cid(CID_A),
                TOKEN_1.to_owned(),
                "org.example".to_owned(),
                PushEnvironment::Development,
                key(),
                false,
            )
            .expect("headerless re-register under fault succeeds");
        assert!(!created2);

        crate::capability::clear_test_capability_hooks();
        set_test_clock(None);

        // After clear_test_capability_hooks, grant_matches against current row is false
        let (devices, grants) = reg.read_devices_locked().expect("read devices");
        assert_eq!(devices.len(), 1);
        let StoredDevice::Ios {
            registered_at: ref stored_at,
            ..
        } = devices[0]
        else {
            panic!("expected ios device");
        };
        assert_eq!(stored_at, &item2.registered_at);
        assert_ne!(stored_at, "2026-09-24T00:00:00Z");

        assert_eq!(grants.len(), 1);
        assert!(!crate::capability::grant_matches(
            &grants[0],
            PushPlatform::Ios,
            cid(CID_A).as_str(),
            TOKEN_1,
            &key(),
            stored_at,
        ));

        // parse_registered_at accepts the stored registered_at
        assert!(parse_registered_at(stored_at).is_some());
        assert!(stored_at.ends_with('Z'));

        // The registry still deserializes as RegistryV2
        let file_contents = fs::read_to_string(reg.path()).expect("read registry");
        let parsed_reg: RegistryV2 =
            serde_json::from_str(&file_contents).expect("deserialize registry");
        assert_eq!(parsed_reg.devices.len(), 1);
    }

    #[test]
    fn unreadable_capability_write_fault_repeated_reregister_advances_registered_at() {
        let root = TempDir::new_in("/var/tmp").expect("journal root");
        let t = OffsetDateTime::from_unix_timestamp(1_790_208_000).unwrap();
        set_test_clock(Some(t));
        let clock_str = "2026-09-24T00:00:00Z";
        let reg = registry(&root);

        // 1. Opt in with no fault. registered_at is the clock string. The grant matches that row.
        let (created, item1) = reg
            .register_ios(
                &cid(CID_A),
                TOKEN_1.to_owned(),
                "org.example".to_owned(),
                PushEnvironment::Development,
                key(),
                true,
            )
            .expect("initial opt-in succeeds");
        assert!(created);
        assert_eq!(item1.registered_at, clock_str);

        let (devices1, grants1) = reg.read_devices_locked().expect("read devices");
        assert_eq!(devices1.len(), 1);
        assert_eq!(grants1.len(), 1);
        assert!(crate::capability::grant_matches(
            &grants1[0],
            PushPlatform::Ios,
            cid(CID_A).as_str(),
            TOKEN_1,
            &key(),
            &item1.registered_at,
        ));

        // 2. Inject read fault and a write hook that always returns Err.
        crate::capability::set_test_read_fault(true);
        crate::capability::set_test_write_hook(Some(Box::new(|_| {
            Err(io::Error::other("write hook fault"))
        })));

        let (created2, item2) = reg
            .register_ios(
                &cid(CID_A),
                TOKEN_1.to_owned(),
                "org.example".to_owned(),
                PushEnvironment::Development,
                key(),
                false,
            )
            .expect("headerless re-register under fault succeeds");
        assert!(!created2);

        let parsed_t2 = parse_registered_at(&item2.registered_at).expect("parses t2");
        assert!(item2.registered_at.ends_with('Z'));
        let parsed_clock = parse_registered_at(clock_str).expect("parses clock");
        assert!(parsed_t2 > parsed_clock);

        crate::capability::clear_test_capability_hooks();
        let (devices2, grants2) = reg.read_devices_locked().expect("read devices");
        assert_eq!(devices2.len(), 1);
        assert_eq!(grants2.len(), 1);
        let StoredDevice::Ios {
            registered_at: ref stored_at2,
            ..
        } = devices2[0]
        else {
            panic!("expected ios device");
        };
        assert_eq!(stored_at2, &item2.registered_at);
        assert!(!crate::capability::grant_matches(
            &grants2[0],
            PushPlatform::Ios,
            cid(CID_A).as_str(),
            TOKEN_1,
            &key(),
            stored_at2,
        ));

        // 3. Repeat the same read+write fault and headerless re-register.
        crate::capability::set_test_read_fault(true);
        crate::capability::set_test_write_hook(Some(Box::new(|_| {
            Err(io::Error::other("write hook fault"))
        })));

        let (created3, item3) = reg
            .register_ios(
                &cid(CID_A),
                TOKEN_1.to_owned(),
                "org.example".to_owned(),
                PushEnvironment::Development,
                key(),
                false,
            )
            .expect("second headerless re-register under fault succeeds");
        assert!(!created3);

        let parsed_t3 = parse_registered_at(&item3.registered_at).expect("parses t3");
        assert!(item3.registered_at.ends_with('Z'));
        assert!(parsed_t3 > parsed_t2);
        assert_ne!(item3.registered_at, clock_str);
        assert_ne!(item3.registered_at, item2.registered_at);

        crate::capability::clear_test_capability_hooks();
        set_test_clock(None);

        let (devices3, grants3) = reg.read_devices_locked().expect("read devices");
        assert_eq!(devices3.len(), 1);
        let StoredDevice::Ios {
            registered_at: ref stored_at3,
            ..
        } = devices3[0]
        else {
            panic!("expected ios device");
        };
        assert_eq!(stored_at3, &item3.registered_at);
        for grant in &grants3 {
            assert!(!crate::capability::grant_matches(
                grant,
                PushPlatform::Ios,
                cid(CID_A).as_str(),
                TOKEN_1,
                &key(),
                stored_at3,
            ));
        }
    }
}
