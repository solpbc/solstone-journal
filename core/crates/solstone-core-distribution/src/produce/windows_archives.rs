// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Local pinned upstream inputs; this module performs no downloads or installation.
//! The inventory alone assigns destinations to these retained member labels.

use solstone_core_installed_payload::WindowsArchitecture;
use std::collections::{BTreeMap, BTreeSet};
use std::io::{Cursor, Read};
use std::path::Path;

use super::windows_build::read_bounded;
use crate::digest::sha256_hex;
use crate::inventory::WindowsNativeComponent;

pub struct AdmittedArchiveInput {
    pub(super) architecture: Option<WindowsArchitecture>,
    pub(super) component: WindowsNativeComponent,
    pub(super) members: BTreeMap<String, Vec<u8>>,
}

impl AdmittedArchiveInput {
    pub fn outputs(&self) -> &BTreeMap<String, Vec<u8>> {
        &self.members
    }
}

#[derive(serde::Serialize)]
pub struct ArchivePin {
    pub version: &'static str,
    pub url: &'static str,
    pub bytes: u64,
    pub sha256: &'static str,
    pub members: &'static [MemberPin],
}

#[derive(serde::Serialize)]
pub struct MemberPin {
    pub label: &'static str,
    pub path: &'static str,
    pub bytes: u64,
    pub sha256: &'static str,
    pub dll: bool,
}

pub fn admit_restic(
    archive: &Path,
    license: &Path,
    expected: WindowsArchitecture,
) -> Result<AdmittedArchiveInput, String> {
    if expected != WindowsArchitecture::X86_64 {
        return Err("missing pinned ARM64 restic input".into());
    }
    let members = admit_zip(archive, &RESTIC, expected)?;
    // licence-tree publishes this text; the pin still rejects a different file.
    let _ = pinned_file(
        license,
        1345,
        "6f08a01a9fab5b24e139a09f15cc24a73087c7bc09e3bacf099fdf2d767bf897",
    )?;
    Ok(AdmittedArchiveInput {
        architecture: Some(expected),
        component: WindowsNativeComponent::Restic,
        members,
    })
}

pub fn admit_rclone(
    archive: &Path,
    license: &Path,
    expected: WindowsArchitecture,
) -> Result<AdmittedArchiveInput, String> {
    let pin = match expected {
        WindowsArchitecture::X86_64 => &RCLONE,
        WindowsArchitecture::Aarch64 => &RCLONE_ARM64,
    };
    let members = admit_zip(archive, pin, expected)?;
    // licence-tree publishes this text; the pin still rejects a different file.
    let _ = pinned_file(
        license,
        1095,
        "8cd2e9e750b90a04b7d82dbbca3930c696ae0309d7c10464f90a44f45754cd04",
    )?;
    Ok(AdmittedArchiveInput {
        architecture: Some(expected),
        component: WindowsNativeComponent::Rclone,
        members,
    })
}

pub fn admit_msvc(
    archive: &Path,
    runtime_license: &Path,
    expected: WindowsArchitecture,
) -> Result<AdmittedArchiveInput, String> {
    let pin = match expected {
        WindowsArchitecture::X86_64 => &MSVC,
        WindowsArchitecture::Aarch64 => &MSVC_ARM64,
    };
    // Only the reviewed release CRT/OpenMP files, never debug_nonredist or
    // additional DLLs merely because the upstream VSIX happens to carry them.
    let mut members = admit_zip(archive, pin, expected)?;
    // Original Microsoft runtime terms, not a grant of redistribution rights.
    // The applicable Visual Studio license remains an operator/release concern.
    members.insert(
        "runtime-license.docx".into(),
        pinned_file(
            runtime_license,
            39644,
            "f1e3d56ceb2ad68aae0711b910375009e651ac5530fa0760f0dea6e81e54fae1",
        )?,
    );
    members.insert("runtime-license-source.json".into(), serde_json::to_vec(&serde_json::json!({
        "url": "https://visualstudio.microsoft.com/wp-content/uploads/2021/09/Visual-C-Runtime-2015-2022-License-1.docx",
        "bytes": 39644,
        "sha256": "f1e3d56ceb2ad68aae0711b910375009e651ac5530fa0760f0dea6e81e54fae1",
    })).map_err(|e| e.to_string())?);
    Ok(AdmittedArchiveInput {
        architecture: Some(expected),
        component: WindowsNativeComponent::Msvc,
        members,
    })
}

pub fn admit_pdfium(
    archive: &Path,
    expected: WindowsArchitecture,
) -> Result<AdmittedArchiveInput, String> {
    let bytes = read_bounded(archive, 16 * 1024 * 1024)?;
    let spec = crate::pdfium::spec_for(expected.target_id())
        .ok_or("missing pinned Windows PDFium spec")?;
    // The existing validator checks the archive digest before parsing, then
    // the exact member census, library and every original notice digest.
    let staged = crate::pdfium::stage_from_bytes(spec, &bytes).map_err(|e| e.to_string())?;
    if !crate::pe_dependencies::inspect_dependencies(&staged.library, expected)?.is_dll {
        return Err("pinned PDFium member is not a DLL".into());
    }
    let mut members = staged.notices;
    members.insert(spec.library_name.into(), staged.library);
    let provenance = serde_json::json!({
        "release": crate::pdfium::RELEASE_TAG,
        "url": spec.archive_url(),
        "archive_bytes": bytes.len(),
        "archive_sha256": spec.archive_sha256,
        "library_sha256": spec.library_sha256,
    });
    members.insert(
        "archive.json".into(),
        serde_json::to_vec(&provenance).map_err(|e| e.to_string())?,
    );
    Ok(AdmittedArchiveInput {
        architecture: Some(expected),
        component: WindowsNativeComponent::Pdfium,
        members,
    })
}

fn pinned_file(path: &Path, size: u64, digest: &str) -> Result<Vec<u8>, String> {
    let bytes = read_bounded(path, size)?;
    require_pin(&bytes, size, digest)?;
    Ok(bytes)
}

fn require_pin(bytes: &[u8], size: u64, digest: &str) -> Result<(), String> {
    if bytes.len() as u64 != size || sha256_hex(bytes) != digest {
        return Err("upstream input differs from pinned size or SHA-256".into());
    }
    Ok(())
}

fn admit_zip(
    path: &Path,
    pin: &ArchivePin,
    architecture: WindowsArchitecture,
) -> Result<BTreeMap<String, Vec<u8>>, String> {
    let bytes = read_bounded(path, pin.bytes)?;
    admit_zip_bytes(&bytes, pin, architecture)
}

fn admit_zip_bytes(
    bytes: &[u8],
    pin: &ArchivePin,
    architecture: WindowsArchitecture,
) -> Result<BTreeMap<String, Vec<u8>>, String> {
    require_pin(bytes, pin.bytes, pin.sha256)?;
    let archive = zip::ZipArchive::new(Cursor::new(bytes)).map_err(|e| e.to_string())?;
    if archive.len() > 512 {
        return Err("pinned upstream archive exceeds member limit".into());
    }
    let (mut archive, names) =
        crate::zip_names::prove_open_zip(archive).map_err(|failure| match failure {
            solstone_core_journal_io::zip2_name::Zip2NameFailure::Collision { .. } => {
                "duplicate upstream archive member".to_owned()
            }
            other => other.to_string(),
        })?;
    let mut unique = BTreeSet::new();
    for name in &names {
        if !unique.insert(name.clone()) {
            return Err("duplicate upstream archive member".into());
        }
    }
    let mut members = BTreeMap::new();
    for expected in pin.members {
        let index = names
            .iter()
            .position(|name| name == expected.path)
            .ok_or_else(|| format!("missing upstream member {}", expected.path))?;
        let mut entry = archive.by_index(index).map_err(|e| e.to_string())?;
        if entry.is_dir()
            || entry.size() != expected.bytes
            || entry
                .unix_mode()
                .is_some_and(|mode| !matches!(mode & 0o170000, 0 | 0o100000))
        {
            return Err(format!(
                "unexpected upstream member type/size: {}",
                expected.path
            ));
        }
        let mut bytes = Vec::new();
        (&mut entry)
            .take(expected.bytes + 1)
            .read_to_end(&mut bytes)
            .map_err(|e| e.to_string())?;
        require_pin(&bytes, expected.bytes, expected.sha256)?;
        if crate::pe_dependencies::inspect_dependencies(&bytes, architecture)?.is_dll
            != expected.dll
        {
            return Err(format!("unexpected upstream PE kind: {}", expected.path));
        }
        if members.insert(expected.label.into(), bytes).is_some() {
            return Err("duplicate retained upstream member".into());
        }
    }
    // Informational original input identities, not a new admission authority.
    members.insert(
        "archive.json".into(),
        serde_json::to_vec(pin).map_err(|e| e.to_string())?,
    );
    Ok(members)
}

// Upstream descriptors supplied by the backup lane, exact pinned release bytes.
pub(crate) const RESTIC: ArchivePin = ArchivePin {
    version: "0.19.0",
    url: "https://github.com/restic/restic/releases/download/v0.19.0/restic_0.19.0_windows_amd64.zip",
    bytes: 11234817,
    sha256: "6fa4219a70b1b5d1c429bb106a7f97f3d2a5aab74494db2e490b625edc486d8f",
    members: &[MemberPin {
        label: "restic.exe",
        path: "restic_0.19.0_windows_amd64.exe",
        bytes: 31651328,
        sha256: "40576f77c1d40245a9f4af92a0b37b0d2514e6be0dffbf16ca8855820c13693e",
        dll: false,
    }],
};

// Upstream descriptors supplied by the backup lane, exact pinned release bytes.
pub(crate) const RCLONE: ArchivePin = ArchivePin {
    version: "1.74.4",
    url: "https://downloads.rclone.org/v1.74.4/rclone-v1.74.4-windows-amd64.zip",
    bytes: 29347029,
    sha256: "ef097ef9de37a57feb7d9f9c7afb34148ad3c65be8025f1d8f7f521554a701ea",
    members: &[MemberPin {
        label: "rclone.exe",
        path: "rclone-v1.74.4-windows-amd64/rclone.exe",
        bytes: 78797824,
        sha256: "492648a3867dbc620188a305e05ff3216aecbf4622bf1a6b5b978ed9c939e18c",
        dll: false,
    }],
};

// Measured VSIX size, rather than its inconsistent catalog size; digest is identical.
pub const MSVC: ArchivePin = ArchivePin {
    version: "14.44.35211.0",
    url: "https://download.visualstudio.microsoft.com/download/pr/45d3b8dd-bced-4b37-9974-142f748d710c/4aaf54db0bfc9435f7c3660e1a00237a4b556042bfeea64bde44c2e0194e6ee5/Microsoft.VC.14.44.17.14.CRT.Redist.X64.base.vsix",
    bytes: 3224191,
    sha256: "4aaf54db0bfc9435f7c3660e1a00237a4b556042bfeea64bde44c2e0194e6ee5",
    members: &[
        MemberPin {
            label: "msvcp140.dll",
            path: "Contents/VC/Redist/MSVC/14.44.35112/x64/Microsoft.VC143.CRT/msvcp140.dll",
            bytes: 557728,
            sha256: "0f885b509a685d2bbfa652fed26b5fb31d88fbdab0a978c641d1c7b8aa460aa9",
            dll: true,
        },
        MemberPin {
            label: "vcruntime140.dll",
            path: "Contents/VC/Redist/MSVC/14.44.35112/x64/Microsoft.VC143.CRT/vcruntime140.dll",
            bytes: 124544,
            sha256: "d5e4d9a3e835fa679450145d6a7d94e36573a509317111904d9b3712c30d9066",
            dll: true,
        },
        MemberPin {
            label: "vcruntime140_1.dll",
            path: "Contents/VC/Redist/MSVC/14.44.35112/x64/Microsoft.VC143.CRT/vcruntime140_1.dll",
            bytes: 49792,
            sha256: "1f2d41c4aa5db0bc33ebf7b66d72943a817d7ce6cbe880502a9403823633093f",
            dll: true,
        },
        MemberPin {
            label: "vcomp140.dll",
            path: "Contents/VC/Redist/MSVC/14.44.35112/x64/Microsoft.VC143.OpenMP/vcomp140.dll",
            bytes: 193152,
            sha256: "55aba23cdcd6484fbb06f4155b8ca75adfce7a881f10afd0c49457165e677164",
            dll: true,
        },
    ],
};

pub(crate) const RCLONE_ARM64: ArchivePin = ArchivePin {
    version: "1.74.4",
    url: "https://downloads.rclone.org/v1.74.4/rclone-v1.74.4-windows-arm64.zip",
    bytes: 26494445,
    sha256: "72194ad0aaf210d7a55808801191fecc7e175444dab7be7491b7a63074521f3a",
    members: &[MemberPin {
        label: "rclone.exe",
        path: "rclone-v1.74.4-windows-arm64/rclone.exe",
        bytes: 72152064,
        sha256: "37a68118889876ce61930c8c9284cf868871a44c0ac93f22ed4de91c249caaa0",
        dll: false,
    }],
};

// The catalog advertises 5,067,096 bytes; the downloaded, digest-matching
// package is 5,046,547 bytes. Select only native ARM64 release CRT members.
// Its vcruntime140_1.dll is AMD64/CHPE, and OpenMP is a separate source input.
pub const MSVC_ARM64: ArchivePin = ArchivePin {
    version: "14.44.35211.0",
    url: "https://download.visualstudio.microsoft.com/download/pr/45d3b8dd-bced-4b37-9974-142f748d710c/fc8f37250188509818258582758a3b704810c77b6d25845486d583ea99d9aa1f/Microsoft.VC.14.44.17.14.CRT.Redist.ARM64.base.vsix",
    bytes: 5046547,
    sha256: "fc8f37250188509818258582758a3b704810c77b6d25845486d583ea99d9aa1f",
    members: &[
        MemberPin {
            label: "msvcp140.dll",
            path: "Contents/VC/Redist/MSVC/14.44.35112/arm64/Microsoft.VC143.CRT/msvcp140.dll",
            bytes: 1372216,
            sha256: "045faeab0b5710816ae160ea20dae8495ffc11bb51feca15d22aaf7ff3752cb3",
            dll: true,
        },
        MemberPin {
            label: "vcruntime140.dll",
            path: "Contents/VC/Redist/MSVC/14.44.35112/arm64/Microsoft.VC143.CRT/vcruntime140.dll",
            bytes: 199200,
            sha256: "d8a8513921544569837e400d37cc71302819967ae6defa932266a7ecb41dbaf9",
            dll: true,
        },
    ],
};

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    #[ignore = "operator qualification with retained pinned archives; set SOLSTONE_WINDOWS_ARCHIVE_TEST_INPUTS"]
    fn real_archives_admit_both_architectures_and_refuse_cross_target_inputs() {
        let input_file = std::env::var_os("SOLSTONE_WINDOWS_ARCHIVE_TEST_INPUTS")
            .expect("retained archive input map required");
        let paths: BTreeMap<String, BTreeMap<String, std::path::PathBuf>> =
            serde_json::from_slice(&std::fs::read(input_file).unwrap()).unwrap();
        for (key, expected, foreign) in [
            (
                "x64",
                WindowsArchitecture::X86_64,
                WindowsArchitecture::Aarch64,
            ),
            (
                "arm64",
                WindowsArchitecture::Aarch64,
                WindowsArchitecture::X86_64,
            ),
        ] {
            let files = &paths[key];
            let rclone =
                admit_rclone(&files["rclone_archive"], &files["rclone_license"], expected).unwrap();
            assert!(rclone.outputs().contains_key("rclone.exe"));
            let msvc =
                admit_msvc(&files["msvc_archive"], &files["runtime_license"], expected).unwrap();
            assert!(msvc.outputs().contains_key("msvcp140.dll"));
            assert!(msvc.outputs().contains_key("vcruntime140.dll"));
            assert_eq!(
                msvc.outputs().contains_key("vcruntime140_1.dll"),
                expected == WindowsArchitecture::X86_64
            );
            assert_eq!(
                msvc.outputs().contains_key("vcomp140.dll"),
                expected == WindowsArchitecture::X86_64
            );
            let pdfium = admit_pdfium(&files["pdfium_archive"], expected).unwrap();
            assert!(pdfium.outputs().contains_key("pdfium.dll"));
            assert!(
                admit_rclone(&files["rclone_archive"], &files["rclone_license"], foreign).is_err()
            );
            assert!(
                admit_msvc(&files["msvc_archive"], &files["runtime_license"], foreign).is_err()
            );
            assert!(admit_pdfium(&files["pdfium_archive"], foreign).is_err());
        }
    }

    #[test]
    fn pinned_zip_admission_checks_member_kind_and_refuses_substitution() {
        let original = crate::pe_dependencies::tests::image();
        let mut zip = zip::ZipWriter::new(Cursor::new(Vec::new()));
        zip.start_file("engine.dll", zip::write::SimpleFileOptions::default())
            .unwrap();
        zip.write_all(&original).unwrap();
        let bytes = zip.finish().unwrap().into_inner();
        // Test-only pin for a complete synthetic PE/ZIP; public callers cannot
        // supply ArchivePin or construct an AdmittedArchiveInput.
        let member = Box::leak(Box::new([MemberPin {
            label: "engine.dll",
            path: "engine.dll",
            bytes: original.len() as u64,
            sha256: Box::leak(sha256_hex(&original).into_boxed_str()),
            dll: true,
        }]));
        let pin = ArchivePin {
            version: "fixture",
            url: "fixture",
            bytes: bytes.len() as u64,
            sha256: Box::leak(sha256_hex(&bytes).into_boxed_str()),
            members: member,
        };
        let admitted = admit_zip_bytes(&bytes, &pin, WindowsArchitecture::X86_64).unwrap();
        assert!(
            admit_zip_bytes(&bytes, &pin, WindowsArchitecture::Aarch64)
                .unwrap_err()
                .contains("ARM64")
        );
        assert_eq!(admitted["engine.dll"], original);
        let wrong_kind = ArchivePin {
            members: Box::leak(Box::new([MemberPin {
                label: "engine.exe",
                path: "engine.dll",
                bytes: original.len() as u64,
                sha256: member[0].sha256,
                dll: false,
            }])),
            ..pin
        };
        assert!(
            admit_zip_bytes(&bytes, &wrong_kind, WindowsArchitecture::X86_64)
                .unwrap_err()
                .contains("PE kind")
        );
        let mut replacement = bytes;
        replacement[0] ^= 1;

        assert!(
            admit_zip_bytes(&replacement, &wrong_kind, WindowsArchitecture::X86_64)
                .unwrap_err()
                .contains("pinned size or SHA-256")
        );
        assert_eq!(admitted["engine.dll"], original);
    }

    #[test]
    fn admit_zip_refuses_duplicate_proved_names() {
        let bytes = crate::zip_names::fixtures::colliding("engine.dll", b"x");
        let pin = ArchivePin {
            version: "fixture",
            url: "fixture",
            bytes: bytes.len() as u64,
            sha256: Box::leak(sha256_hex(&bytes).into_boxed_str()),
            members: &[],
        };

        assert_eq!(
            admit_zip_bytes(&bytes, &pin, WindowsArchitecture::X86_64).unwrap_err(),
            "duplicate upstream archive member"
        );
    }
}
