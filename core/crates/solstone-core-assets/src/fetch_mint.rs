// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Compile-time and runtime validation for minting runtime fetches.

use std::fmt;

#[cfg(feature = "runtime-fetch-test")]
use crate::fetch_set::unit_id;
use crate::fetch_set::{FetchSetError, runtime_fetch_set};
use crate::{Artifact, Backend, Platform, catalog};

/// An immutable, validated token granting admission to download a runtime artifact.
///
/// Fields are private so that handles can only be created via [`mint_runtime_fetch`]
/// (or the test fixture constructor under `runtime-fetch-test`).
///
/// ```compile_fail,E0451
/// use solstone_core_assets::RuntimeFetchHandle;
///
/// let _ = RuntimeFetchHandle {
///     unit: "ced-model",
///     origin_key: "assets/ced-model/ced-tiny-q8_0.gguf",
///     sha256: "00",
///     size_bytes: 42,
/// };
/// ```
///
/// ```compile_fail,E0425
/// use solstone_core_assets::runtime_fetch_handle_fixture;
///
/// let _ = runtime_fetch_handle_fixture("ced-model", "key", "sha", 100);
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeFetchHandle {
    unit: &'static str,
    origin_key: &'static str,
    sha256: &'static str,
    size_bytes: u64,
}

impl RuntimeFetchHandle {
    #[must_use]
    pub fn unit(&self) -> &'static str {
        self.unit
    }

    #[must_use]
    pub fn origin_key(&self) -> &'static str {
        self.origin_key
    }

    #[must_use]
    pub fn sha256(&self) -> &'static str {
        self.sha256
    }

    #[must_use]
    pub fn size_bytes(&self) -> u64 {
        self.size_bytes
    }
}

/// Query discriminators for requesting a runtime fetch mint.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RuntimeFetchQuery<'a> {
    pub unit: Option<&'a str>,
    pub platform: Option<Platform>,
    pub backend: Option<Backend>,
    pub artifact_key: Option<&'a str>,
    pub filename: Option<&'a str>,
    pub origin_key: Option<&'a str>,
}

/// Error returned when an artifact is not admitted for runtime fetch.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FetchMintError;

impl FetchMintError {
    pub const REASON_CODE: &'static str = "component_packaged";
}

impl fmt::Display for FetchMintError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "build defect: this component ships inside the package, and retrying will not help"
        )
    }
}

impl std::error::Error for FetchMintError {}

#[cfg(feature = "runtime-fetch-test")]
thread_local! {
    static TARGET_OVERRIDE: std::cell::Cell<Option<&'static str>> = const { std::cell::Cell::new(None) };
    static UNIT_EXCLUSION: std::cell::Cell<Option<&'static str>> = const { std::cell::Cell::new(None) };
}

#[cfg(feature = "runtime-fetch-test")]
pub fn with_runtime_fetch_target<T>(target: &'static str, f: impl FnOnce() -> T) -> T {
    struct Guard(Option<&'static str>);
    impl Drop for Guard {
        fn drop(&mut self) {
            TARGET_OVERRIDE.with(|cell| cell.set(self.0));
        }
    }
    let prev = TARGET_OVERRIDE.with(|cell| cell.replace(Some(target)));
    let _guard = Guard(prev);
    f()
}

#[cfg(feature = "runtime-fetch-test")]
pub fn without_runtime_fetch_unit<T>(unit: &'static str, f: impl FnOnce() -> T) -> T {
    struct Guard(Option<&'static str>);
    impl Drop for Guard {
        fn drop(&mut self) {
            UNIT_EXCLUSION.with(|cell| cell.set(self.0));
        }
    }
    let prev = UNIT_EXCLUSION.with(|cell| cell.replace(Some(unit)));
    let _guard = Guard(prev);
    f()
}

#[cfg(feature = "runtime-fetch-test")]
#[must_use]
pub fn runtime_fetch_handle_fixture(
    unit: &'static str,
    origin_key: &'static str,
    sha256: &'static str,
    size_bytes: u64,
) -> RuntimeFetchHandle {
    RuntimeFetchHandle {
        unit,
        origin_key,
        sha256,
        size_bytes,
    }
}

fn compiled_fetch_target() -> Result<&'static str, FetchMintError> {
    #[cfg(all(target_os = "linux", target_arch = "x86_64"))]
    {
        Ok("linux-x86_64")
    }
    #[cfg(all(target_os = "linux", target_arch = "aarch64"))]
    {
        Ok("linux-aarch64")
    }
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    {
        Ok("macos-arm64")
    }
    #[cfg(all(target_os = "windows", target_arch = "x86_64"))]
    {
        Ok("windows-x86_64")
    }
    #[cfg(not(any(
        all(target_os = "linux", target_arch = "x86_64"),
        all(target_os = "linux", target_arch = "aarch64"),
        all(target_os = "macos", target_arch = "aarch64"),
        all(target_os = "windows", target_arch = "x86_64")
    )))]
    {
        Err(FetchMintError)
    }
}

/// Mint a runtime fetch handle for the current compiled host target.
pub fn mint_runtime_fetch(
    query: &RuntimeFetchQuery<'_>,
) -> Result<RuntimeFetchHandle, FetchMintError> {
    #[cfg(feature = "runtime-fetch-test")]
    let target = if let Some(t) = TARGET_OVERRIDE.with(|cell| cell.get()) {
        t
    } else {
        compiled_fetch_target()?
    };

    #[cfg(not(feature = "runtime-fetch-test"))]
    let target = compiled_fetch_target()?;

    mint_runtime_fetch_impl(target, query)
}

#[cfg(feature = "runtime-fetch-test")]
pub fn mint_runtime_fetch_for_target(
    target: &str,
    query: &RuntimeFetchQuery<'_>,
) -> Result<RuntimeFetchHandle, FetchMintError> {
    mint_runtime_fetch_impl(target, query)
}

fn mint_runtime_fetch_impl(
    target: &str,
    query: &RuntimeFetchQuery<'_>,
) -> Result<RuntimeFetchHandle, FetchMintError> {
    #[allow(unused_mut)]
    let mut set =
        runtime_fetch_set(target).map_err(|FetchSetError::UnknownTarget(_)| FetchMintError)?;

    #[cfg(feature = "runtime-fetch-test")]
    if let Some(excluded) = UNIT_EXCLUSION.with(|cell| cell.get()) {
        let excluded_id = unit_id(excluded);
        set.retain(|item| {
            if let Some(id) = unit_id(item.unit()) {
                Some(id) != excluded_id
            } else {
                item.unit() != excluded
            }
        });
    }

    if let Some(origin_key) = query.origin_key {
        let member = set
            .into_iter()
            .find(|item| item.origin_key() == origin_key)
            .ok_or(FetchMintError)?;
        return Ok(RuntimeFetchHandle {
            unit: member.unit(),
            origin_key: member.origin_key(),
            sha256: member.sha256(),
            size_bytes: member.size_bytes(),
        });
    }

    let matching_rows: Vec<&'static Artifact> = catalog()
        .iter()
        .filter(|row| {
            if let Some(u) = query.unit
                && row.unit != u
            {
                return false;
            }
            if let Some(p) = query.platform
                && row.platform != Some(p)
            {
                return false;
            }
            if let Some(b) = query.backend
                && row.backend != Some(b)
            {
                return false;
            }
            if let Some(k) = query.artifact_key
                && row.artifact_key != Some(k)
            {
                return false;
            }
            if let Some(f) = query.filename
                && row.filename != f
            {
                return false;
            }
            true
        })
        .collect();

    if matching_rows.len() != 1 {
        return Err(FetchMintError);
    }

    let row = matching_rows[0];
    let member = set
        .into_iter()
        .find(|item| item.origin_key() == row.origin_key)
        .ok_or(FetchMintError)?;

    Ok(RuntimeFetchHandle {
        unit: member.unit(),
        origin_key: member.origin_key(),
        sha256: member.sha256(),
        size_bytes: member.size_bytes(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::RuntimeFetch;

    #[test]
    fn runtime_mint_parakeet_coreml_refuses_on_linux_x86_64() {
        #[cfg(all(target_os = "linux", target_arch = "x86_64"))]
        {
            let query = RuntimeFetchQuery {
                unit: Some("parakeet-coreml"),
                ..Default::default()
            };
            let err = mint_runtime_fetch(&query).expect_err("coreml must refuse on linux x86_64");
            assert_eq!(err, FetchMintError);
            assert_eq!(FetchMintError::REASON_CODE, "component_packaged");
        }
    }

    #[cfg(feature = "runtime-fetch-test")]
    #[test]
    fn four_target_complement_agrees_with_runtime_fetch_set() {
        use crate::inventory_for_tests;

        let targets = [
            "linux-x86_64",
            "linux-aarch64",
            "macos-arm64",
            "windows-x86_64",
        ];
        let all_inventory = inventory_for_tests();

        let nvat_authority = solstone_core_nvattest_authority::parse(
            solstone_core_nvattest_authority::AUTHORITY_JSON,
        )
        .expect("nvattest authority");

        for &target in &targets {
            let set = runtime_fetch_set(target).expect("runtime_fetch_set");
            let set_origin_keys: std::collections::BTreeSet<_> =
                set.iter().map(RuntimeFetch::origin_key).collect();

            // 1. Every member in runtime_fetch_set must mint successfully by origin_key
            for member in &set {
                let query = RuntimeFetchQuery {
                    origin_key: Some(member.origin_key()),
                    ..Default::default()
                };
                let handle = mint_runtime_fetch_for_target(target, &query).unwrap_or_else(|_| {
                    panic!("member {} must mint for {}", member.origin_key(), target)
                });
                assert_eq!(handle.origin_key(), member.origin_key());
                assert_eq!(handle.sha256(), member.sha256());
                assert_eq!(handle.size_bytes(), member.size_bytes());
                assert_eq!(handle.unit(), member.unit());
            }

            // 2. Every ARTIFACTS row not in runtime_fetch_set must refuse
            for row in all_inventory {
                if !set_origin_keys.contains(row.origin_key) {
                    let query = RuntimeFetchQuery {
                        origin_key: Some(row.origin_key),
                        ..Default::default()
                    };
                    assert_eq!(
                        mint_runtime_fetch_for_target(target, &query),
                        Err(FetchMintError),
                        "row {} should refuse on {}",
                        row.origin_key,
                        target
                    );
                }
            }

            // 3. Every nvattest archive and companion key outside runtime_fetch_set must refuse
            if let Ok(pins) = solstone_core_nvattest_authority::origin_pins(&nvat_authority) {
                for pin in pins {
                    let full_key = format!("providers/nvattest/{}", pin.origin_key);
                    if !set_origin_keys.contains(full_key.as_str()) {
                        let query = RuntimeFetchQuery {
                            origin_key: Some(&full_key),
                            ..Default::default()
                        };
                        assert_eq!(
                            mint_runtime_fetch_for_target(target, &query),
                            Err(FetchMintError),
                            "nvattest pin {} should refuse on {}",
                            full_key,
                            target
                        );
                    }
                }
            }
        }
    }
}
