// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Pinned CPU runtime table. `solstone-distribution acquire onnx` stages
//! from this table through the builder-input download policy.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::Path;

#[cfg(not(windows))]
use solstone_core_artifact_download::{BUILDER_INPUT_DOWNLOAD_POLICY, ensure_verified_url};

use crate::digest::sha256_hex;
use crate::zip;

pub const LIB_MODE: u32 = 0o755;
pub const NOTICE_MODE: u32 = 0o644;
pub const STAGED_NAME: &str = "libonnxruntime.so.1";
pub const FORBIDDEN_GPU_MEMBERS: &[&str] = &[
    "onnxruntime/capi/libonnxruntime_providers_cuda.so",
    "onnxruntime/capi/libonnxruntime_providers_tensorrt.so",
];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NoticeSpec {
    pub source_member: &'static str,
    pub staged_name: &'static str,
    pub sha256: &'static str,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TargetSpec {
    pub key: &'static str,
    pub wheel_url: &'static str,
    pub wheel_sha256: &'static str,
    pub runtime_member: &'static str,
    pub runtime_sha256: &'static str,
    pub runtime_staged_name: &'static str,
    pub link_names: &'static [&'static str],
    pub notices: &'static [NoticeSpec],
}

pub const COMMON_NOTICES: &[NoticeSpec] = &[
    NoticeSpec {
        source_member: "onnxruntime/LICENSE",
        staged_name: "onnxruntime-LICENSE.txt",
        sha256: "2f07c72751aed99790b8a4869cf2311df85a860b22ded05fa22803587a48922c",
    },
    NoticeSpec {
        source_member: "onnxruntime/ThirdPartyNotices.txt",
        staged_name: "onnxruntime-ThirdPartyNotices.txt",
        sha256: "0e07b95f3a8d6230037707c5c4a2b554d12c4cb67369669ac255635528ffcee2",
    },
];

pub const TARGETS: &[TargetSpec] = &[
    TargetSpec {
        key: "linux-x86_64",
        wheel_url: "https://files.pythonhosted.org/packages/a9/1b/d681878f227513917d8620e4ea504af5eb3313fc01f8aea7b19a976c65db/onnxruntime-1.25.0-cp312-cp312-manylinux_2_27_x86_64.manylinux_2_28_x86_64.whl",
        wheel_sha256: "be93baa694ef8e5831fcb7b542da21f502b122918b5b9612d9f02972e043ee01",
        runtime_member: "onnxruntime/capi/libonnxruntime.so.1.25.0",
        runtime_sha256: "6976c9c6b2db120e835a7091e2f4bd2308a76d3856a7181beb7e7a9b1e08f9e5",
        runtime_staged_name: "libonnxruntime.so.1",
        link_names: &[
            "libonnxruntime.so.1.25.0",
            "libonnxruntime.so.1",
            "libonnxruntime.so",
        ],
        notices: COMMON_NOTICES,
    },
    TargetSpec {
        key: "linux-aarch64",
        wheel_url: "https://files.pythonhosted.org/packages/5a/c6/19c5bfbc60396791e975652f982bcff9ff4b27947c8e2bf0064ac5d5727b/onnxruntime-1.25.0-cp312-cp312-manylinux_2_27_aarch64.manylinux_2_28_aarch64.whl",
        wheel_sha256: "9c99238d20bfa80ac68c7b03c2c936d389189ae40997f78a30d151570d7e18bf",
        runtime_member: "onnxruntime/capi/libonnxruntime.so.1.25.0",
        runtime_sha256: "d47425026b2474e1deb0b8cf22f74cd943539af85873aa3fb8052862445beef3",
        runtime_staged_name: "libonnxruntime.so.1",
        link_names: &[
            "libonnxruntime.so.1.25.0",
            "libonnxruntime.so.1",
            "libonnxruntime.so",
        ],
        notices: COMMON_NOTICES,
    },
    TargetSpec {
        key: "macos-arm64",
        wheel_url: "https://files.pythonhosted.org/packages/7a/69/f98c6bda4c34ac382b70c36033a989ceffd1caf5afba47bd2ef26535850f/onnxruntime-1.25.0-cp312-cp312-macosx_14_0_arm64.whl",
        wheel_sha256: "8ecd3362de3fb496fb3e2d055a95d5acab611cf759a27609c6d99704c9d8f184",
        runtime_member: "onnxruntime/capi/libonnxruntime.1.25.0.dylib",
        runtime_sha256: "bafe7d3f3fa8e31195501e5694e73ef240708d5df039feb272b8d506d2783a74",
        runtime_staged_name: "libonnxruntime.1.25.0.dylib",
        link_names: &["libonnxruntime.1.25.0.dylib", "libonnxruntime.dylib"],
        notices: COMMON_NOTICES,
    },
];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StagedRuntime {
    pub library: Vec<u8>,
    pub notices: BTreeMap<String, Vec<u8>>,
}

#[derive(Debug)]
pub struct StageError {
    pub message: String,
}

impl StageError {
    fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

impl std::fmt::Display for StageError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for StageError {}

pub fn spec_for(key: &str) -> Option<&'static TargetSpec> {
    TARGETS.iter().find(|spec| spec.key == key)
}

#[must_use]
pub fn staged_member_names(spec: &TargetSpec) -> BTreeSet<&'static str> {
    let mut names = BTreeSet::new();
    names.insert(spec.runtime_staged_name);
    names.extend(spec.link_names.iter().copied());
    names.extend(spec.notices.iter().map(|notice| notice.staged_name));
    names
}

pub fn stage_from_bytes(spec: &TargetSpec, wheel: &[u8]) -> Result<StagedRuntime, StageError> {
    let digest = sha256_hex(wheel);
    if digest != spec.wheel_sha256 {
        return Err(StageError::new(format!(
            "missing required:\n  wheel digest {digest}"
        )));
    }
    let members = zip::read_members(wheel).map_err(|error| StageError::new(error.to_string()))?;
    let unexpected = members
        .iter()
        .filter(|member| FORBIDDEN_GPU_MEMBERS.contains(&member.name.as_str()))
        .map(|member| member.name.clone())
        .collect::<Vec<_>>();
    if !unexpected.is_empty() {
        return Err(StageError::new(format!(
            "unexpected:\n  {}",
            unexpected.join("\n  ")
        )));
    }
    let library = zip::member(&members, spec.runtime_member)
        .map_err(|error| StageError::new(error.to_string()))?;
    if sha256_hex(library) != spec.runtime_sha256 {
        return Err(StageError::new(
            "missing required:\n  runtime member digest",
        ));
    }
    let mut notices = BTreeMap::new();
    for notice in spec.notices {
        let bytes = zip::member(&members, notice.source_member)
            .map_err(|error| StageError::new(error.to_string()))?;
        if sha256_hex(bytes) != notice.sha256 {
            return Err(StageError::new(format!(
                "missing required:\n  notice {}",
                notice.staged_name
            )));
        }
        notices.insert(notice.staged_name.to_owned(), bytes.to_vec());
    }
    Ok(StagedRuntime {
        library: library.to_vec(),
        notices,
    })
}

pub fn stage_from_path(spec: &TargetSpec, path: &Path) -> Result<StagedRuntime, StageError> {
    let bytes = fs::read(path).map_err(|error| StageError::new(error.to_string()))?;
    stage_from_bytes(spec, &bytes)
}

/// Single origin primitive: only pin-table URLs may be fetched, and only
/// through the builder-input policy. This is not the owner-facing allow-list.
#[cfg(not(windows))]
pub fn fetch_origin(url: &str) -> Result<Vec<u8>, StageError> {
    let spec = TARGETS
        .iter()
        .find(|item| item.wheel_url == url)
        .ok_or_else(|| StageError::new(format!("unexpected:\n  {url}")))?;
    let dest =
        std::env::temp_dir().join(format!("solstone-builder-onnx-{}.whl", spec.wheel_sha256));
    ensure_verified_url(
        spec.wheel_url,
        spec.wheel_sha256,
        None,
        &dest,
        &BUILDER_INPUT_DOWNLOAD_POLICY,
        |_, _| {},
    )
    .map_err(|error| StageError::new(error.to_string()))?;
    fs::read(&dest).map_err(|error| StageError::new(error.to_string()))
}

#[cfg(windows)]
pub fn fetch_origin(url: &str) -> Result<Vec<u8>, StageError> {
    Err(StageError::new(format!(
        "onnx runtime fetch is not supported on windows: {url}"
    )))
}

pub fn fetch_wheel(spec: &TargetSpec) -> Result<Vec<u8>, StageError> {
    fetch_origin(spec.wheel_url)
}

pub fn stage_from_url(spec: &TargetSpec) -> Result<StagedRuntime, StageError> {
    let bytes = fetch_wheel(spec)?;
    stage_from_bytes(spec, &bytes)
}

pub fn write_staged_runtime(
    spec: &TargetSpec,
    staged: &StagedRuntime,
    dest_dir: &Path,
) -> Result<(), StageError> {
    fs::create_dir_all(dest_dir).map_err(|error| StageError::new(error.to_string()))?;
    crate::stage::write_staged_file_mode(
        dest_dir,
        spec.runtime_staged_name,
        &staged.library,
        LIB_MODE,
    )
    .map_err(|error| StageError::new(error.to_string()))?;
    for name in spec.link_names {
        crate::stage::write_staged_file_mode(dest_dir, name, &staged.library, LIB_MODE)
            .map_err(|error| StageError::new(error.to_string()))?;
    }
    for (name, bytes) in &staged.notices {
        crate::stage::write_staged_file_mode(dest_dir, name, bytes, NOTICE_MODE)
            .map_err(|error| StageError::new(error.to_string()))?;
    }
    Ok(())
}

pub fn identity_fixture_spec() -> TargetSpec {
    TargetSpec {
        key: "fixture",
        wheel_url: "fixture://wheel",
        wheel_sha256: "",
        runtime_member: "onnxruntime/capi/libonnxruntime.so.1.25.0",
        runtime_sha256: "",
        runtime_staged_name: STAGED_NAME,
        link_names: &["libonnxruntime.so.1"],
        notices: &[],
    }
}

pub fn identity_fixture_wheel() -> (TargetSpec, Vec<u8>) {
    let library = b"fixture-runtime-bytes";
    let files = [(
        "onnxruntime/capi/libonnxruntime.so.1.25.0",
        library.as_slice(),
    )];
    let wheel = zip::write_stored_zip(&files).expect("fixture zip");
    let mut spec = identity_fixture_spec();
    spec.wheel_sha256 = Box::leak(sha256_hex(&wheel).into_boxed_str());
    spec.runtime_sha256 = Box::leak(sha256_hex(library).into_boxed_str());
    (spec, wheel)
}

pub fn forbidden_member_fixture_wheel() -> (TargetSpec, Vec<u8>) {
    let (mut spec, _) = identity_fixture_wheel();
    let files = [(FORBIDDEN_GPU_MEMBERS[0], b"gpu".as_slice())];
    let wheel = zip::write_stored_zip(&files).expect("forbidden fixture zip");
    spec.wheel_sha256 = Box::leak(sha256_hex(&wheel).into_boxed_str());
    (spec, wheel)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn staged_member_names_dedupe_the_overlapping_soname() {
        let spec = spec_for("linux-x86_64").expect("linux-x86_64");
        let names = staged_member_names(spec);
        assert!(names.contains("libonnxruntime.so.1"));
        assert!(names.contains("libonnxruntime.so.1.25.0"));
        assert!(names.contains("libonnxruntime.so"));
        assert!(names.contains("onnxruntime-LICENSE.txt"));
        assert!(names.contains("onnxruntime-ThirdPartyNotices.txt"));
        assert_eq!(names.len(), 5);
    }
}
