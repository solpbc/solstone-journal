// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Qualified signed XML collateral embedded in every native journal payload.

use crate::error::GpuAppraisalReason;
use ring::digest::{SHA256, digest};
use std::{
    fs,
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
    time::{SystemTime, UNIX_EPOCH},
};

// This mapping is separate from CPU admission. Packaging a new profile never
// adds its PCR fingerprint to the production policy in pins.rs.
const PROFILE_PCR: &str = "b162f46105c80d3e45028e37cc649404c9d65297ad1cda8f953208582060b0e3";

struct PackagedRim {
    filename: &'static str,
    sha256: &'static str,
    xml: &'static [u8],
}

const RIMS: &[PackagedRim] = &[
    PackagedRim {
        filename: "NV_GPU_DRIVER_GH100_595.71.05.xml",
        sha256: "143005c060a5eb2866f7db046c234d27f87ae386cab20ab40d9daa706be46ea4",
        xml: include_bytes!("../../rims/NV_GPU_DRIVER_GH100_595.71.05.xml"),
    },
    PackagedRim {
        filename: "NV_GPU_VBIOS_1010_0210_886_96009F0004.xml",
        sha256: "d5dd2bc3f138713ce9a6bd9ba472cb2833e362316e4528b7f2b5d6890e4315db",
        xml: include_bytes!("../../rims/NV_GPU_VBIOS_1010_0210_886_96009F0004.xml"),
    },
    PackagedRim {
        filename: "NV_GPU_VBIOS_1010_0210_886_9600880011.xml",
        sha256: "a948d7b1e183a8dbede96a3288a1a14b54ddf4f8d4e8576dc4ccb2798022b183",
        xml: include_bytes!("../../rims/NV_GPU_VBIOS_1010_0210_886_9600880011.xml"),
    },
];

static NEXT_DIR: AtomicU64 = AtomicU64::new(0);

pub(super) struct TempRimDir(PathBuf);

impl TempRimDir {
    pub(super) fn write(pcr_sha256: &str) -> Result<Self, GpuAppraisalReason> {
        let rims = profile_rims(pcr_sha256)?;
        // Check the release-approved digests before exposing any XML to the SDK.
        for rim in rims {
            verify_digest(rim.xml, rim.sha256)?;
        }
        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|_| GpuAppraisalReason::NvattestIntegrityFailed)?
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "solstone-nvattest-rims-{}-{timestamp}-{}",
            std::process::id(),
            NEXT_DIR.fetch_add(1, Ordering::Relaxed)
        ));
        let builder = fs::DirBuilder::new();
        #[cfg(unix)]
        let builder = {
            use std::os::unix::fs::DirBuilderExt;
            let mut builder = builder;
            builder.mode(0o700);
            builder
        };
        builder
            .create(&path)
            .map_err(|_| GpuAppraisalReason::NvattestIntegrityFailed)?;
        let dir = Self(path);
        for rim in rims {
            fs::write(dir.0.join(rim.filename), rim.xml)
                .map_err(|_| GpuAppraisalReason::NvattestIntegrityFailed)?;
        }
        Ok(dir)
    }

    pub(super) fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TempRimDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn profile_rims(pcr_sha256: &str) -> Result<&'static [PackagedRim], GpuAppraisalReason> {
    if pcr_sha256 == PROFILE_PCR {
        Ok(RIMS)
    } else {
        Err(GpuAppraisalReason::GpuAppraisalFailed)
    }
}

fn verify_digest(xml: &[u8], expected: &str) -> Result<(), GpuAppraisalReason> {
    let actual: String = digest(&SHA256, xml)
        .as_ref()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    if actual == expected {
        Ok(())
    } else {
        Err(GpuAppraisalReason::NvattestIntegrityFailed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_admitted_cpu_profile_has_integrity_checked_collateral() {
        for pin in crate::pins::PRODUCTION_PCR_SHA256_PINS {
            for rim in profile_rims(pin).expect("admitted profile has RIMs") {
                verify_digest(rim.xml, rim.sha256).expect("approved digest");
            }
        }
    }

    #[test]
    fn unknown_profile_and_tampered_collateral_fail_closed() {
        assert!(profile_rims(&"00".repeat(32)).is_err());
        let rim = &RIMS[0];
        let mut xml = rim.xml.to_vec();
        xml[100] ^= 1;
        assert_eq!(
            verify_digest(&xml, rim.sha256),
            Err(GpuAppraisalReason::NvattestIntegrityFailed)
        );
    }

    #[cfg(feature = "full-tests")]
    #[test]
    fn embedded_collateral_stages_without_an_installed_resource_directory() {
        let dir = TempRimDir::write(PROFILE_PCR).expect("stage embedded collateral");
        let path = dir.path().to_path_buf();
        for rim in RIMS {
            let bytes = fs::read(path.join(rim.filename)).expect("staged XML");
            verify_digest(&bytes, rim.sha256).expect("staged digest");
        }
        drop(dir);
        assert!(!path.exists());
    }
}
