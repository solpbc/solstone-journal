// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Local pinned upstream inputs; this module performs no downloads or installation.
//! The inventory alone assigns destinations to these retained member labels.

use std::collections::{BTreeMap, BTreeSet};
use std::io::{Cursor, Read};
use std::path::Path;

use super::windows_build::read_bounded;
use crate::digest::sha256_hex;
use crate::inventory::WindowsNativeComponent;

pub struct AdmittedArchiveInput {
    pub(super) component: WindowsNativeComponent,
    pub(super) members: BTreeMap<String, Vec<u8>>,
}

impl AdmittedArchiveInput {
    pub fn outputs(&self) -> &BTreeMap<String, Vec<u8>> {
        &self.members
    }
}

#[derive(serde::Serialize)]
struct ArchivePin {
    version: &'static str,
    url: &'static str,
    bytes: u64,
    sha256: &'static str,
    members: &'static [MemberPin],
}

#[derive(serde::Serialize)]
struct MemberPin {
    label: &'static str,
    path: &'static str,
    bytes: u64,
    sha256: &'static str,
    dll: bool,
}

pub fn admit_restic(archive: &Path, license: &Path) -> Result<AdmittedArchiveInput, String> {
    let mut members = admit_zip(archive, &RESTIC)?;
    members.insert(
        "LICENSE".into(),
        pinned_file(
            license,
            1345,
            "6f08a01a9fab5b24e139a09f15cc24a73087c7bc09e3bacf099fdf2d767bf897",
        )?,
    );
    Ok(AdmittedArchiveInput {
        component: WindowsNativeComponent::Restic,
        members,
    })
}

pub fn admit_rclone(archive: &Path, license: &Path) -> Result<AdmittedArchiveInput, String> {
    let mut members = admit_zip(archive, &RCLONE)?;
    members.insert(
        "COPYING".into(),
        pinned_file(
            license,
            1095,
            "8cd2e9e750b90a04b7d82dbbca3930c696ae0309d7c10464f90a44f45754cd04",
        )?,
    );
    Ok(AdmittedArchiveInput {
        component: WindowsNativeComponent::Rclone,
        members,
    })
}

pub fn admit_msvc(archive: &Path, runtime_license: &Path) -> Result<AdmittedArchiveInput, String> {
    // Only the reviewed release CRT/OpenMP files, never debug_nonredist or
    // additional DLLs merely because the upstream VSIX happens to carry them.
    let mut members = admit_zip(archive, &MSVC)?;
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
        component: WindowsNativeComponent::Msvc,
        members,
    })
}

pub fn admit_pdfium(archive: &Path) -> Result<AdmittedArchiveInput, String> {
    let bytes = read_bounded(archive, 16 * 1024 * 1024)?;
    let spec =
        crate::pdfium::spec_for("windows-x86_64").ok_or("missing pinned Windows PDFium spec")?;
    // The existing validator checks the archive digest before parsing, then
    // the exact member census, library and every original notice digest.
    let staged = crate::pdfium::stage_from_bytes(spec, &bytes).map_err(|e| e.to_string())?;
    if !crate::pe_dependencies::inspect_dependencies(&staged.library)?.is_dll {
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

fn admit_zip(path: &Path, pin: &ArchivePin) -> Result<BTreeMap<String, Vec<u8>>, String> {
    let bytes = pinned_file(path, pin.bytes, pin.sha256)?;
    let mut archive = zip::ZipArchive::new(Cursor::new(bytes)).map_err(|e| e.to_string())?;
    if archive.len() > 512 {
        return Err("pinned upstream archive exceeds member limit".into());
    }
    let mut names = BTreeSet::new();
    for index in 0..archive.len() {
        let entry = archive.by_index(index).map_err(|e| e.to_string())?;
        if !names.insert(entry.name().to_owned()) {
            return Err("duplicate upstream archive member".into());
        }
    }
    let mut members = BTreeMap::new();
    for expected in pin.members {
        let mut entry = archive.by_name(expected.path).map_err(|e| e.to_string())?;
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
        if crate::pe_dependencies::inspect_dependencies(&bytes)?.is_dll != expected.dll {
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
const RESTIC: ArchivePin = ArchivePin {
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
const RCLONE: ArchivePin = ArchivePin {
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
const MSVC: ArchivePin = ArchivePin {
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

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
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("upstream.zip");
        std::fs::write(&path, &bytes).unwrap();
        let admitted = admit_zip(&path, &pin).unwrap();
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
            admit_zip(&path, &wrong_kind)
                .unwrap_err()
                .contains("PE kind")
        );
        let mut replacement = bytes;
        replacement[0] ^= 1;
        std::fs::write(&path, &replacement).unwrap();
        assert!(
            admit_zip(&path, &wrong_kind)
                .unwrap_err()
                .contains("pinned size or SHA-256")
        );
        assert_eq!(admitted["engine.dll"], original);
    }
}
