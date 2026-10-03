// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Qualified signed XML collateral embedded in every native journal payload,
//! and the per-image GPU profile that pairs it with a status mode.

use crate::error::GpuAppraisalReason;
use ring::digest::{SHA256, digest};
use std::{
    fs,
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
    time::{SystemTime, UNIX_EPOCH},
};

/// How a GPU's certificate revocation status is established for one image.
///
/// The verified CPU fingerprint selects it locally. Nothing the engine sends,
/// and no failure, can select or downgrade it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StatusMode {
    /// nvattest asks NVIDIA's OCSP responder, with a fresh request nonce,
    /// during this appraisal.
    OnlineNonce,
    /// The engine carries raw NVIDIA-signed OCSP responses in its certificate.
    /// nvattest judges them offline by signed age on this device's clock and
    /// makes no NVIDIA, RIM or NRAS request.
    OfflineSignedAge,
}

#[derive(Debug)]
pub struct PackagedRim {
    filename: &'static str,
    sha256: &'static str,
    xml: &'static [u8],
}

/// A finite, digest-pinned set of NVIDIA reference manifests.
#[derive(Debug, Clone, Copy)]
pub struct ManifestSet(&'static [PackagedRim]);

const QUALIFIED_595_71_05: &[PackagedRim] = &[
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

impl ManifestSet {
    /// Driver 595.71.05 with both qualified H100 VBIOS variants.
    pub const QUALIFIED_595_71_05: Self = Self(QUALIFIED_595_71_05);
}

/// One admitted image's complete GPU profile: its manifests and status mode.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GpuProfile {
    pcr_sha256: String,
    manifests: ManifestSet,
    status: StatusMode,
}

impl PartialEq for ManifestSet {
    fn eq(&self, other: &Self) -> bool {
        std::ptr::eq(self.0, other.0)
    }
}

impl Eq for ManifestSet {}

impl GpuProfile {
    pub fn new(pcr_sha256: impl Into<String>, manifests: ManifestSet, status: StatusMode) -> Self {
        Self {
            pcr_sha256: pcr_sha256.into(),
            manifests,
            status,
        }
    }

    pub fn pcr_sha256(&self) -> &str {
        &self.pcr_sha256
    }

    pub fn status(&self) -> StatusMode {
        self.status
    }
}

// Each admitted production pin and its complete profile. This mapping is
// separate from CPU admission: a profile here never adds its fingerprint to
// pins.rs, and the integrity test refuses an admitted pin without a profile.
// A later pin admits its exact fingerprint, authenticated manifests and status
// mode together, keeping this one during the overlap.
const PRODUCTION_PROFILES: &[(&str, ManifestSet, StatusMode)] = &[(
    "b162f46105c80d3e45028e37cc649404c9d65297ad1cda8f953208582060b0e3",
    ManifestSet::QUALIFIED_595_71_05,
    StatusMode::OnlineNonce,
)];

/// The GPU profiles a verifier may select from after CPU verification.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GpuProfiles(Vec<GpuProfile>);

impl GpuProfiles {
    /// The profiles compiled into this release, one per admitted pin.
    pub fn production() -> Self {
        Self(
            PRODUCTION_PROFILES
                .iter()
                .map(|(pcr, manifests, status)| GpuProfile::new(*pcr, *manifests, *status))
                .collect(),
        )
    }

    /// An explicit profile list, for tests and qualification instruments that
    /// inject their own CPU policy too. It is never read from configuration.
    pub fn from_profiles(profiles: Vec<GpuProfile>) -> Self {
        Self(profiles)
    }

    /// Selects the one profile for a verified CPU fingerprint.
    pub fn select(&self, pcr_sha256: &str) -> Result<&GpuProfile, GpuAppraisalReason> {
        let mut matches = self
            .0
            .iter()
            .filter(|profile| profile.pcr_sha256 == pcr_sha256);
        match (matches.next(), matches.next()) {
            (Some(profile), None) => Ok(profile),
            _ => Err(GpuAppraisalReason::StatusProfileMissing),
        }
    }
}

static NEXT_DIR: AtomicU64 = AtomicU64::new(0);

pub(super) struct TempRimDir(PathBuf);

impl TempRimDir {
    pub(super) fn write(profile: &GpuProfile) -> Result<Self, GpuAppraisalReason> {
        let rims = profile.manifests.0;
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
        let profiles = GpuProfiles::production();
        for pin in crate::pins::PRODUCTION_PCR_SHA256_PINS {
            let profile = profiles
                .select(pin)
                .expect("admitted pin has a complete profile");
            assert!(!profile.manifests.0.is_empty());
            for rim in profile.manifests.0 {
                verify_digest(rim.xml, rim.sha256).expect("approved digest");
            }
        }
        // Every compiled profile belongs to an admitted pin: no fixture or
        // candidate fingerprint reaches production policy.
        for (pcr, _, _) in PRODUCTION_PROFILES {
            assert!(crate::pins::PRODUCTION_PCR_SHA256_PINS.contains(pcr));
        }
    }

    #[test]
    fn the_current_image_keeps_online_nonce_status() {
        let profiles = GpuProfiles::production();
        let profile = profiles
            .select("b162f46105c80d3e45028e37cc649404c9d65297ad1cda8f953208582060b0e3")
            .expect("current image profile");
        assert_eq!(profile.status(), StatusMode::OnlineNonce);
    }

    #[test]
    fn unknown_duplicate_profile_and_tampered_collateral_fail_closed() {
        assert_eq!(
            GpuProfiles::production().select(&"00".repeat(32)),
            Err(GpuAppraisalReason::StatusProfileMissing)
        );
        let pin = "11".repeat(32);
        let duplicated = GpuProfiles::from_profiles(vec![
            GpuProfile::new(
                &pin,
                ManifestSet::QUALIFIED_595_71_05,
                StatusMode::OnlineNonce,
            ),
            GpuProfile::new(
                &pin,
                ManifestSet::QUALIFIED_595_71_05,
                StatusMode::OfflineSignedAge,
            ),
        ]);
        assert_eq!(
            duplicated.select(&pin),
            Err(GpuAppraisalReason::StatusProfileMissing)
        );
        let rim = &QUALIFIED_595_71_05[0];
        let mut xml = rim.xml.to_vec();
        xml[100] ^= 1;
        assert_eq!(
            verify_digest(&xml, rim.sha256),
            Err(GpuAppraisalReason::NvattestIntegrityFailed)
        );
    }

    #[test]
    fn test_profiles_can_map_several_pins_to_different_modes() {
        let current = "b162f46105c80d3e45028e37cc649404c9d65297ad1cda8f953208582060b0e3";
        let successor = "22".repeat(32);
        let profiles = GpuProfiles::from_profiles(vec![
            GpuProfile::new(
                current,
                ManifestSet::QUALIFIED_595_71_05,
                StatusMode::OnlineNonce,
            ),
            GpuProfile::new(
                &successor,
                ManifestSet::QUALIFIED_595_71_05,
                StatusMode::OfflineSignedAge,
            ),
        ]);
        assert_eq!(
            profiles.select(current).unwrap().status(),
            StatusMode::OnlineNonce
        );
        assert_eq!(
            profiles.select(&successor).unwrap().status(),
            StatusMode::OfflineSignedAge
        );
    }

    #[cfg(feature = "full-tests")]
    #[test]
    fn embedded_collateral_stages_without_an_installed_resource_directory() {
        let profiles = GpuProfiles::production();
        let profile = profiles
            .select(crate::pins::PRODUCTION_PCR_SHA256_PINS[0])
            .expect("production profile");
        let dir = TempRimDir::write(profile).expect("stage embedded collateral");
        let path = dir.path().to_path_buf();
        for rim in profile.manifests.0 {
            let bytes = fs::read(path.join(rim.filename)).expect("staged XML");
            verify_digest(&bytes, rim.sha256).expect("staged digest");
        }
        drop(dir);
        assert!(!path.exists());
    }
}
