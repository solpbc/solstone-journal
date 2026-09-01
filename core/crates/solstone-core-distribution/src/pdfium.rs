// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Pinned PDFium runtime table. The producer stages from this table.
//!
//! The previous Python stager verified a GitHub attestation in addition to
//! these digests. That check now lives in `acquire::stage_pdfium`, which
//! still shells to `gh attestation verify` after the digest-pinned fetch.

use std::collections::{BTreeMap, BTreeSet};
use std::io::Read;
use std::path::Path;

use flate2::read::GzDecoder;

use crate::digest::sha256_hex;

pub const LIB_MODE: u32 = 0o755;
pub const NOTICE_MODE: u32 = 0o644;
pub const RELEASE_TAG: &str = "chromium/7920";
pub const RELEASE_URL: &str =
    "https://github.com/bblanchon/pdfium-binaries/releases/download/chromium/7920";
pub const ATTESTATION_NAME: &str = "pdfium-attestation.json";
pub const ATTESTATION_SHA256: &str =
    "41cdeff1f9db4f340e80857fcdec11e9ef168204b9aafb663aaf0a34c6052aee";
pub const ATTESTATION_REPOSITORY: &str = "bblanchon/pdfium-binaries";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TargetSpec {
    pub key: &'static str,
    pub archive_name: &'static str,
    pub archive_sha256: &'static str,
    pub library_member: &'static str,
    pub library_name: &'static str,
    pub library_sha256: &'static str,
}

impl TargetSpec {
    #[must_use]
    pub fn archive_url(&self) -> String {
        format!("{RELEASE_URL}/{}", self.archive_name)
    }
}

pub const TARGETS: &[TargetSpec] = &[
    TargetSpec {
        key: "linux-x86_64",
        archive_name: "pdfium-linux-x64.tgz",
        archive_sha256: "49ab3afbd4e6c1e284b5f2898129c8bb8a10fd785c1c5392c8c1fc70242f9ced",
        library_member: "lib/libpdfium.so",
        library_name: "libpdfium.so",
        library_sha256: "687dce861f959c7097d47c5864509d51a926a71b38322596a8ee3e7a99c6b96e",
    },
    TargetSpec {
        key: "linux-aarch64",
        archive_name: "pdfium-linux-arm64.tgz",
        archive_sha256: "00551476a77fbc1a31c37573eadc9b63f1c366f65ad727539326927da083bb4d",
        library_member: "lib/libpdfium.so",
        library_name: "libpdfium.so",
        library_sha256: "933f3d620cc8b58fb30a7f12a1bce8bf276da65caf39ff8fb2d04bc1268d53a3",
    },
    TargetSpec {
        key: "macos-x86_64",
        archive_name: "pdfium-mac-x64.tgz",
        archive_sha256: "0c78b8d55a4c97e02c9bb516997253cb972739373009cf29554c959a2f6b194a",
        library_member: "lib/libpdfium.dylib",
        library_name: "libpdfium.dylib",
        library_sha256: "8fdf8fc61c85676515321b0c214fb1afa0e157cffdadbdff40802e7b4bed7ad6",
    },
    TargetSpec {
        key: "macos-arm64",
        archive_name: "pdfium-mac-arm64.tgz",
        archive_sha256: "c032aa59be58b0f12e41e76a8ef707e347b9841b0426446f646b2568d350ec4f",
        library_member: "lib/libpdfium.dylib",
        library_name: "libpdfium.dylib",
        library_sha256: "df568fcd17a6a6296956aa79abea1181db187458432f360b084fec1cea7cd4d9",
    },
];

const ROOT_MEMBERS: &[&str] = &["LICENSE", "PDFiumConfig.cmake", "VERSION", "args.gn"];
const HEADER_MEMBERS: &[&str] = &[
    "include/cpp/fpdf_deleters.h",
    "include/cpp/fpdf_scopers.h",
    "include/fpdf_annot.h",
    "include/fpdf_attachment.h",
    "include/fpdf_catalog.h",
    "include/fpdf_dataavail.h",
    "include/fpdf_doc.h",
    "include/fpdf_edit.h",
    "include/fpdf_ext.h",
    "include/fpdf_flatten.h",
    "include/fpdf_formfill.h",
    "include/fpdf_fwlevent.h",
    "include/fpdf_javascript.h",
    "include/fpdf_ppo.h",
    "include/fpdf_progressive.h",
    "include/fpdf_save.h",
    "include/fpdf_searchex.h",
    "include/fpdf_signature.h",
    "include/fpdf_structtree.h",
    "include/fpdf_sysfontinfo.h",
    "include/fpdf_text.h",
    "include/fpdf_thumbnail.h",
    "include/fpdf_transformpage.h",
    "include/fpdfview.h",
    "include/fpdfview.h.orig",
];
const NOTICE_MEMBERS: &[&str] = &[
    "licenses/abseil.txt",
    "licenses/agg23.txt",
    "licenses/fast_float.txt",
    "licenses/freetype.txt",
    "licenses/icu.txt",
    "licenses/lcms.txt",
    "licenses/libjpeg_turbo.ijg",
    "licenses/libjpeg_turbo.md",
    "licenses/libopenjpeg.txt",
    "licenses/libpng.txt",
    "licenses/libtiff.txt",
    "licenses/llvm-libc.txt",
    "licenses/pdfium.txt",
    "licenses/simdutf.txt",
    "licenses/zlib.txt",
];
const DIRECTORY_MEMBERS: &[&str] = &["include", "include/cpp", "lib", "licenses"];
const NOTICE_SHA256: &[(&str, &str)] = &[
    (
        "licenses/abseil.txt",
        "c79a7fea0e3cac04cd43f20e7b648e5a0ff8fa5344e644b0ee09ca1162b62747",
    ),
    (
        "licenses/agg23.txt",
        "c110d3ea2ad77467ce0dcff7d3337e6c8be8049a5103f4b9bd5fd911a77972e5",
    ),
    (
        "licenses/fast_float.txt",
        "e562f3f974ced7e69dd1db77b820b36bcf8f30377f1aa105723fba449c53c4e6",
    ),
    (
        "licenses/freetype.txt",
        "f4b133e25df1f86ad3ffea453aa0e613f0474f34778dbbb3e437e7b2724937d8",
    ),
    (
        "licenses/icu.txt",
        "e55522d81edc687a341a4411e0776e54ca654e90147f354a90458aaced4116af",
    ),
    (
        "licenses/lcms.txt",
        "7312b68c5b25e9bf2b828706fb4e29588f22705112f411fd42e1f7d84c3d139a",
    ),
    (
        "licenses/libjpeg_turbo.ijg",
        "75815e3bf6484201a3c3d17a1bbf10f2e8e3237f84df10a2357ea896db2a81d6",
    ),
    (
        "licenses/libjpeg_turbo.md",
        "96f5b328adbb78eeaaec6980d73fd558cb1e4d62560ed615646bc3cf5e532430",
    ),
    (
        "licenses/libopenjpeg.txt",
        "c5ab0890a737c2dfa7ba675036554f6d17741d98629b0c2a145354d00617e6b2",
    ),
    (
        "licenses/libpng.txt",
        "bdb0a645ea18c60507d0368379b1ac5474b92255fcc2d115e07486a7672ba526",
    ),
    (
        "licenses/libtiff.txt",
        "92b72ba97e6c2749c2a94bc0ef646b47080217f1e772a482b33cf5a5f98a6506",
    ),
    (
        "licenses/llvm-libc.txt",
        "ebcd9bbf783a73d05c53ba4d586b8d5813dcdf3bbec50265860ccc885e606f47",
    ),
    (
        "licenses/pdfium.txt",
        "961eacd9633fff6d051db7208b755e9210e30efac7adec3e6a6d52798f0ccf0e",
    ),
    (
        "licenses/simdutf.txt",
        "fc8dbc04e03ad4efc08a647ffe7f995b811a95bc04c0e85a56d5277c6593fa5f",
    ),
    (
        "licenses/zlib.txt",
        "33fd641c9f3b0e0be64bc78fea9e94807674cdd70c48477599226cb8956565fe",
    ),
];
const MACOS_PDFIUM_NOTICE_SHA256: &str =
    "1fe9dea718fbd75cf149adaf4d8a22a4335604d964ddb76d1b45383dec8668c9";

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
pub fn staged_member_names(spec: &TargetSpec) -> BTreeSet<String> {
    let mut names = BTreeSet::new();
    names.insert(spec.library_name.to_owned());
    for member in NOTICE_MEMBERS {
        if let Some(name) = Path::new(member).file_name().and_then(|name| name.to_str()) {
            names.insert(name.to_owned());
        }
    }
    names
}

pub fn attestation_url() -> String {
    format!("{RELEASE_URL}/{ATTESTATION_NAME}")
}

pub fn expected_members(spec: &TargetSpec) -> BTreeSet<&'static str> {
    let mut names = BTreeSet::new();
    names.extend(ROOT_MEMBERS.iter().copied());
    names.extend(HEADER_MEMBERS.iter().copied());
    names.extend(NOTICE_MEMBERS.iter().copied());
    names.extend(DIRECTORY_MEMBERS.iter().copied());
    names.insert(spec.library_member);
    names
}

fn notice_sha256(spec: &TargetSpec, member: &str) -> Option<&'static str> {
    if member == "licenses/pdfium.txt" && spec.key.starts_with("macos-") {
        return Some(MACOS_PDFIUM_NOTICE_SHA256);
    }
    NOTICE_SHA256
        .iter()
        .find(|(name, _)| *name == member)
        .map(|(_, digest)| *digest)
}

pub fn stage_from_bytes(spec: &TargetSpec, archive: &[u8]) -> Result<StagedRuntime, StageError> {
    let digest = sha256_hex(archive);
    if digest != spec.archive_sha256 {
        return Err(StageError::new(format!(
            "PDFium archive digest mismatch\n  expected: {}\n  actual: {digest}",
            spec.archive_sha256
        )));
    }
    let decoder = GzDecoder::new(archive);
    let mut tar = tar::Archive::new(decoder);
    let mut names = BTreeSet::new();
    let mut extracted = BTreeMap::new();
    for entry in tar
        .entries()
        .map_err(|error| StageError::new(error.to_string()))?
    {
        let mut entry = entry.map_err(|error| StageError::new(error.to_string()))?;
        let name = entry
            .path()
            .map_err(|error| StageError::new(error.to_string()))?
            .to_string_lossy()
            .trim_end_matches('/')
            .to_owned();
        names.insert(name.clone());
        let is_dir = entry.header().entry_type().is_dir();
        if is_dir {
            continue;
        }
        if !entry.header().entry_type().is_file() {
            return Err(StageError::new(format!(
                "PDFium archive member is not a regular file: {name}"
            )));
        }
        let mut data = Vec::new();
        entry
            .read_to_end(&mut data)
            .map_err(|error| StageError::new(error.to_string()))?;
        extracted.insert(name, data);
    }
    let expected = expected_members(spec);
    let expected_owned = expected.iter().map(|name| (*name).to_owned()).collect();
    if names != expected_owned {
        let missing = expected
            .iter()
            .filter(|name| !names.contains(**name))
            .copied()
            .collect::<Vec<_>>();
        let unexpected = names
            .iter()
            .filter(|name| !expected.contains(name.as_str()))
            .cloned()
            .collect::<Vec<_>>();
        return Err(StageError::new(format!(
            "PDFium archive member set is not the reviewed {RELEASE_TAG} layout\n  missing: {}\n  unexpected: {}",
            if missing.is_empty() {
                "<none>".to_owned()
            } else {
                missing.join(", ")
            },
            if unexpected.is_empty() {
                "<none>".to_owned()
            } else {
                unexpected.join(", ")
            }
        )));
    }
    let library = extracted.get(spec.library_member).ok_or_else(|| {
        StageError::new(format!(
            "PDFium archive member cannot be read: {}",
            spec.library_member
        ))
    })?;
    if sha256_hex(library) != spec.library_sha256 {
        return Err(StageError::new(format!(
            "PDFium extracted member digest mismatch: {}\n  expected: {}\n  actual: {}",
            spec.library_member,
            spec.library_sha256,
            sha256_hex(library)
        )));
    }
    let mut notices = BTreeMap::new();
    for member in NOTICE_MEMBERS {
        let bytes = extracted.get(*member).ok_or_else(|| {
            StageError::new(format!("PDFium archive member cannot be read: {member}"))
        })?;
        let expected = notice_sha256(spec, member).ok_or_else(|| {
            StageError::new(format!("PDFium notice has no pinned digest: {member}"))
        })?;
        if sha256_hex(bytes) != expected {
            return Err(StageError::new(format!(
                "PDFium extracted member digest mismatch: {member}\n  expected: {expected}\n  actual: {}",
                sha256_hex(bytes)
            )));
        }
        let staged_name = Path::new(member)
            .file_name()
            .and_then(|name| name.to_str())
            .ok_or_else(|| StageError::new(format!("PDFium notice has no file name: {member}")))?;
        notices.insert(staged_name.to_owned(), bytes.clone());
    }
    Ok(StagedRuntime {
        library: library.clone(),
        notices,
    })
}

pub fn write_staged_library(
    spec: &TargetSpec,
    staged: &StagedRuntime,
    dest_dir: &Path,
) -> Result<(), StageError> {
    std::fs::create_dir_all(dest_dir).map_err(|error| StageError::new(error.to_string()))?;
    crate::stage::write_staged_file_mode(dest_dir, spec.library_name, &staged.library, LIB_MODE)
        .map_err(|error| StageError::new(error.to_string()))?;
    for (name, bytes) in &staged.notices {
        crate::stage::write_staged_file_mode(dest_dir, name, bytes, NOTICE_MODE)
            .map_err(|error| StageError::new(error.to_string()))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn linux_host_digests_match_the_makefile_pins() {
        let x86 = spec_for("linux-x86_64").expect("linux-x86_64");
        let arm = spec_for("linux-aarch64").expect("linux-aarch64");
        let mac = spec_for("macos-arm64").expect("macos-arm64");
        assert_eq!(
            x86.library_sha256,
            "687dce861f959c7097d47c5864509d51a926a71b38322596a8ee3e7a99c6b96e"
        );
        assert_eq!(
            arm.library_sha256,
            "933f3d620cc8b58fb30a7f12a1bce8bf276da65caf39ff8fb2d04bc1268d53a3"
        );
        assert_eq!(
            mac.library_sha256,
            "df568fcd17a6a6296956aa79abea1181db187458432f360b084fec1cea7cd4d9"
        );
    }

    #[test]
    fn staged_member_names_are_library_plus_notice_basenames() {
        let spec = spec_for("linux-x86_64").expect("linux-x86_64");
        let names = staged_member_names(spec);
        assert!(names.contains("libpdfium.so"));
        assert!(names.contains("pdfium.txt"));
        assert!(!names.contains("include/fpdfview.h"));
        assert_eq!(names.len(), 1 + NOTICE_MEMBERS.len());
    }

    #[test]
    fn archive_digest_mismatch_is_refused() {
        let spec = spec_for("linux-x86_64").expect("linux-x86_64");
        let error = stage_from_bytes(spec, b"not-an-archive")
            .expect_err("wrong digest")
            .to_string();
        assert!(error.contains("archive digest mismatch"));
    }
}
