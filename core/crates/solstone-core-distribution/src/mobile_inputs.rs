// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Pinned input revisions and source-archive digests for mobile tools.

use std::sync::OnceLock;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ToolInputPin {
    pub revision: &'static str,
    pub archive_sha256: &'static str,
}

/// Identity of the Regorus `build.rs` revision substitution recorded by the
/// mobile native driver.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RegorusBuildRevisionPin {
    pub revision: &'static str,
    pub original_sha256: &'static str,
    pub patched_sha256: &'static str,
}

struct MobileInputPins {
    by_name: Vec<(&'static str, ToolInputPin)>,
    regorus_build: RegorusBuildRevisionPin,
}

static PINS: OnceLock<MobileInputPins> = OnceLock::new();

fn leak_pin(revision: &str, archive_sha256: &str) -> ToolInputPin {
    ToolInputPin {
        revision: Box::leak(revision.to_string().into_boxed_str()),
        archive_sha256: Box::leak(archive_sha256.to_string().into_boxed_str()),
    }
}

fn pin_from(inputs: &serde_json::Value, name: &str) -> ToolInputPin {
    let entry = inputs
        .get(name)
        .unwrap_or_else(|| panic!("inputs.{name} missing from inputs.json"));
    let revision = entry
        .get("revision")
        .and_then(|value| value.as_str())
        .unwrap_or_else(|| panic!("inputs.{name}.revision missing"));
    let archive_sha256 = entry
        .get("sha256")
        .and_then(|value| value.as_str())
        .unwrap_or_else(|| panic!("inputs.{name}.sha256 missing"));
    leak_pin(revision, archive_sha256)
}

fn regorus_build_from(sources: &serde_json::Value) -> RegorusBuildRevisionPin {
    let entries = sources
        .get("native_sources")
        .and_then(|value| value.as_array())
        .expect("native_sources missing from nvattest-windows-sources.json");
    let entry = entries
        .iter()
        .find(|value| {
            value.get("name").and_then(|name| name.as_str()) == Some("regorus-build-revision")
        })
        .expect("regorus-build-revision missing from nvattest-windows-sources.json");
    let revision = entry
        .get("revision")
        .and_then(|value| value.as_str())
        .expect("regorus-build-revision.revision missing");
    let original_sha256 = entry
        .get("original_sha256")
        .and_then(|value| value.as_str())
        .expect("regorus-build-revision.original_sha256 missing");
    let patched_sha256 = entry
        .get("sha256")
        .and_then(|value| value.as_str())
        .expect("regorus-build-revision.sha256 missing");
    RegorusBuildRevisionPin {
        revision: Box::leak(revision.to_string().into_boxed_str()),
        original_sha256: Box::leak(original_sha256.to_string().into_boxed_str()),
        patched_sha256: Box::leak(patched_sha256.to_string().into_boxed_str()),
    }
}

fn get_pins() -> &'static MobileInputPins {
    PINS.get_or_init(|| {
        let text = include_str!("../../../distribution/mobile/inputs.json");
        let val: serde_json::Value =
            serde_json::from_str(text).expect("inputs.json must parse as json");
        let inputs = val
            .get("inputs")
            .expect("inputs key missing from inputs.json");
        let map = inputs
            .as_object()
            .expect("inputs.json inputs must be an object");
        let mut by_name: Vec<(&'static str, ToolInputPin)> = Vec::with_capacity(map.len());
        for (name, _) in map {
            let leaked: &'static str = Box::leak(name.clone().into_boxed_str());
            by_name.push((leaked, pin_from(inputs, name)));
        }
        let sources: serde_json::Value = serde_json::from_str(include_str!(
            "../../../distribution/nvattest-windows-sources.json"
        ))
        .expect("nvattest-windows-sources.json must parse as json");
        MobileInputPins {
            by_name,
            regorus_build: regorus_build_from(&sources),
        }
    })
}

fn named_pin(name: &str) -> ToolInputPin {
    get_pins()
        .by_name
        .iter()
        .find(|(entry, _)| *entry == name)
        .map(|(_, pin)| *pin)
        .unwrap_or_else(|| panic!("inputs.{name} missing from inputs.json"))
}

#[must_use]
pub fn restic_input_pin() -> ToolInputPin {
    named_pin("restic")
}

#[must_use]
pub fn rclone_input_pin() -> ToolInputPin {
    named_pin("rclone")
}

/// Mobile nvattest source-archive pin. `archive_sha256` is the source archive,
/// not a built `libnvat.so` or `nvattest` binary.
#[must_use]
pub fn nvattest_input_pin() -> ToolInputPin {
    named_pin("nvattest")
}

/// One compiled `inputs.json` entry. `name` is the driver input key.
#[must_use]
pub fn mobile_input_pin(name: &str) -> ToolInputPin {
    named_pin(name)
}

/// Compiled Regorus build-revision patch identity. `patched_sha256` is the
/// `sha256` of the committed `regorus-build-revision` source entry.
#[must_use]
pub fn regorus_build_revision_pin() -> RegorusBuildRevisionPin {
    get_pins().regorus_build
}

/// Reads `share/ca/ca-bundle.pem` from the repository's pinned linux-aarch64 archive.
///
/// Used for test fixtures to verify against the true compiled pin without embedding 7MB.
/// The archive is decompressed once per process.
pub fn read_test_ca_bundle_pem() -> Result<Vec<u8>, std::io::Error> {
    static CA_BUNDLE: OnceLock<Result<Vec<u8>, (std::io::ErrorKind, String)>> = OnceLock::new();
    CA_BUNDLE
        .get_or_init(|| read_ca_bundle_pem_uncached().map_err(|e| (e.kind(), e.to_string())))
        .clone()
        .map_err(|(kind, message)| std::io::Error::new(kind, message))
}

fn read_ca_bundle_pem_uncached() -> Result<Vec<u8>, std::io::Error> {
    let repo_root = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../..");
    let archive_path = repo_root
        .join("core/models/assets/nvattest/libnvat-linux-aarch64-1.2.2-sol.8-archive.tar.xz");
    let file = std::fs::File::open(archive_path)?;
    let mut decompressed = Vec::new();
    lzma_rs::xz_decompress(&mut std::io::BufReader::new(file), &mut decompressed)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
    let mut archive = tar::Archive::new(&decompressed[..]);
    for entry in archive.entries()? {
        let mut entry = entry?;
        if entry.path()?.to_string_lossy() == "share/ca/ca-bundle.pem" {
            let mut bytes = Vec::new();
            std::io::Read::read_to_end(&mut entry, &mut bytes)?;
            return Ok(bytes);
        }
    }
    Err(std::io::Error::new(
        std::io::ErrorKind::NotFound,
        "ca-bundle.pem not found in libnvat-linux-aarch64 archive",
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_mobile_inputs_matches_include_str() {
        let text = include_str!("../../../distribution/mobile/inputs.json");
        let val: serde_json::Value = serde_json::from_str(text).unwrap();
        let restic = &val["inputs"]["restic"];
        let rclone = &val["inputs"]["rclone"];
        let nvattest = &val["inputs"]["nvattest"];
        let restic_pin = restic_input_pin();
        let rclone_pin = rclone_input_pin();
        let nvattest_pin = nvattest_input_pin();
        assert_eq!(restic_pin.revision, restic["revision"].as_str().unwrap());
        assert_eq!(
            restic_pin.archive_sha256,
            restic["sha256"].as_str().unwrap()
        );
        assert_eq!(rclone_pin.revision, rclone["revision"].as_str().unwrap());
        assert_eq!(
            rclone_pin.archive_sha256,
            rclone["sha256"].as_str().unwrap()
        );
        assert_eq!(
            nvattest_pin.revision,
            nvattest["revision"].as_str().unwrap()
        );
        assert_eq!(
            nvattest_pin.archive_sha256,
            nvattest["sha256"].as_str().unwrap()
        );
        let openssl = mobile_input_pin("nvattest-openssl");
        assert_eq!(
            openssl.revision,
            val["inputs"]["nvattest-openssl"]["revision"]
                .as_str()
                .unwrap()
        );
        assert_eq!(
            openssl.archive_sha256,
            val["inputs"]["nvattest-openssl"]["sha256"]
                .as_str()
                .unwrap()
        );
        let sources: serde_json::Value = serde_json::from_str(include_str!(
            "../../../distribution/nvattest-windows-sources.json"
        ))
        .unwrap();
        let regorus = sources["native_sources"]
            .as_array()
            .unwrap()
            .iter()
            .find(|entry| entry["name"] == "regorus-build-revision")
            .unwrap();
        let pin = regorus_build_revision_pin();
        assert_eq!(pin.revision, regorus["revision"].as_str().unwrap());
        assert_eq!(
            pin.original_sha256,
            regorus["original_sha256"].as_str().unwrap()
        );
        assert_eq!(pin.patched_sha256, regorus["sha256"].as_str().unwrap());
        assert_eq!(pin.revision, mobile_input_pin("nvattest-regorus").revision);
    }
}
