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
    TargetSpec {
        key: "windows-x86_64",
        archive_name: "pdfium-win-x64.tgz",
        archive_sha256: "bf25149815b34b00042f48a886653d469c817529dd9cccabb4b509b6465a9526",
        library_member: "bin/pdfium.dll",
        library_name: "pdfium.dll",
        library_sha256: "0aa3abb1aa20798094c1a5f2d8cdea45b24a6e12cdc6c774de261dd522dbdf81",
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
const WINDOWS_DIRECTORY_MEMBERS: &[&str] = &["bin"];
const WINDOWS_IMPORT_LIBRARY_MEMBER: &str = "lib/pdfium.dll.lib";
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
const WINDOWS_NOTICE_SHA256: &[(&str, &str)] = &[
    (
        "licenses/abseil.txt",
        "f54fff0b905df5b3464527c652a30e903b172d6dcab4d89b5e6f105d5e4a4603",
    ),
    (
        "licenses/fast_float.txt",
        "bf1b57355feca8fce77ee95f48002f8d4789fb71b30ec7599c06cda4901fbb2b",
    ),
    (
        "licenses/icu.txt",
        "93679f4389d53b6835d89843f251844fb9bc455b35bab036d3c8e7abe497a47a",
    ),
    (
        "licenses/libjpeg_turbo.ijg",
        "db16a04128171879c60708d171b88d97345a2dd20f9bfc173680a4497c73f704",
    ),
    (
        "licenses/libjpeg_turbo.md",
        "be2b2b5ab168bce87bc3e31f2a5c5adba4b7f6e9e51d618e958d1d46972ebd95",
    ),
    (
        "licenses/libpng.txt",
        "452390433ba0f88aa3e2b122c647741b72a0c117cd6ed7a329b49785aecb5511",
    ),
    (
        "licenses/llvm-libc.txt",
        "3b6226c32e168c83b891d8d6f0d3c29c2116dc3ef93dc93c307b54f279ecf383",
    ),
    (
        "licenses/simdutf.txt",
        "c172a0ba936ff31230febb5dad869e25cb7c1a07480c7a381be8cf011bb52719",
    ),
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
    if spec.key.starts_with("windows-") {
        names.extend(WINDOWS_DIRECTORY_MEMBERS.iter().copied());
        // Census-only: never staged; bytes already covered by archive_sha256.
        names.insert(WINDOWS_IMPORT_LIBRARY_MEMBER);
    }
    names
}

fn notice_sha256(spec: &TargetSpec, member: &str) -> Option<&'static str> {
    if member == "licenses/pdfium.txt" && spec.key.starts_with("macos-") {
        return Some(MACOS_PDFIUM_NOTICE_SHA256);
    }
    if spec.key.starts_with("windows-")
        && let Some((_, digest)) = WINDOWS_NOTICE_SHA256
            .iter()
            .find(|(name, _)| *name == member)
    {
        return Some(digest);
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

    const PLACEHOLDER_DLL: &[u8] = b"windows-pdfium-dll-placeholder";
    const PLACEHOLDER_IMPORT_LIB: &[u8] = b"windows-pdfium-import-lib-placeholder";

    fn windows_license(member: &str) -> &'static [u8] {
        match member {
            "licenses/abseil.txt" => {
                include_bytes!("../fixtures/pdfium-win-licenses/abseil.txt")
            }
            "licenses/agg23.txt" => include_bytes!("../fixtures/pdfium-win-licenses/agg23.txt"),
            "licenses/fast_float.txt" => {
                include_bytes!("../fixtures/pdfium-win-licenses/fast_float.txt")
            }
            "licenses/freetype.txt" => {
                include_bytes!("../fixtures/pdfium-win-licenses/freetype.txt")
            }
            "licenses/icu.txt" => include_bytes!("../fixtures/pdfium-win-licenses/icu.txt"),
            "licenses/lcms.txt" => include_bytes!("../fixtures/pdfium-win-licenses/lcms.txt"),
            "licenses/libjpeg_turbo.ijg" => {
                include_bytes!("../fixtures/pdfium-win-licenses/libjpeg_turbo.ijg")
            }
            "licenses/libjpeg_turbo.md" => {
                include_bytes!("../fixtures/pdfium-win-licenses/libjpeg_turbo.md")
            }
            "licenses/libopenjpeg.txt" => {
                include_bytes!("../fixtures/pdfium-win-licenses/libopenjpeg.txt")
            }
            "licenses/libpng.txt" => include_bytes!("../fixtures/pdfium-win-licenses/libpng.txt"),
            "licenses/libtiff.txt" => {
                include_bytes!("../fixtures/pdfium-win-licenses/libtiff.txt")
            }
            "licenses/llvm-libc.txt" => {
                include_bytes!("../fixtures/pdfium-win-licenses/llvm-libc.txt")
            }
            "licenses/pdfium.txt" => include_bytes!("../fixtures/pdfium-win-licenses/pdfium.txt"),
            "licenses/simdutf.txt" => {
                include_bytes!("../fixtures/pdfium-win-licenses/simdutf.txt")
            }
            "licenses/zlib.txt" => include_bytes!("../fixtures/pdfium-win-licenses/zlib.txt"),
            other => panic!("unknown windows license fixture {other}"),
        }
    }

    fn leak_sha256(bytes: &[u8]) -> &'static str {
        Box::leak(sha256_hex(bytes).into_boxed_str())
    }

    fn windows_spec_matching(archive: &[u8], library: &[u8]) -> TargetSpec {
        let mut spec = spec_for("windows-x86_64").expect("windows-x86_64").clone();
        spec.archive_sha256 = leak_sha256(archive);
        spec.library_sha256 = leak_sha256(library);
        spec
    }

    struct WindowsArchiveLayout<'a> {
        dll: Option<&'a [u8]>,
        import_lib: Option<&'a [u8]>,
        notices: BTreeMap<&'a str, &'a [u8]>,
        extra: Vec<(&'a str, &'a [u8])>,
    }

    fn default_windows_layout() -> WindowsArchiveLayout<'static> {
        WindowsArchiveLayout {
            dll: Some(PLACEHOLDER_DLL),
            import_lib: Some(PLACEHOLDER_IMPORT_LIB),
            notices: BTreeMap::new(),
            extra: Vec::new(),
        }
    }

    fn gzip_windows_archive(layout: &WindowsArchiveLayout<'_>) -> Vec<u8> {
        let mut builder = tar::Builder::new(crate::tar::deterministic_gzip(Vec::new()));
        for name in ROOT_MEMBERS {
            crate::tar::append_regular(&mut builder, name, b"", 0o644).expect("root");
        }
        crate::tar::append_directory(&mut builder, "include", 0o755).expect("include");
        crate::tar::append_directory(&mut builder, "include/cpp", 0o755).expect("include/cpp");
        for name in HEADER_MEMBERS {
            crate::tar::append_regular(&mut builder, name, b"", 0o644).expect("header");
        }
        crate::tar::append_directory(&mut builder, "bin", 0o755).expect("bin");
        if let Some(dll) = layout.dll {
            crate::tar::append_regular(&mut builder, "bin/pdfium.dll", dll, 0o644).expect("dll");
        }
        crate::tar::append_directory(&mut builder, "lib", 0o755).expect("lib");
        if let Some(import_lib) = layout.import_lib {
            crate::tar::append_regular(
                &mut builder,
                WINDOWS_IMPORT_LIBRARY_MEMBER,
                import_lib,
                0o644,
            )
            .expect("import lib");
        }
        crate::tar::append_directory(&mut builder, "licenses", 0o755).expect("licenses");
        for member in NOTICE_MEMBERS {
            let bytes = layout
                .notices
                .get(member)
                .copied()
                .unwrap_or_else(|| windows_license(member));
            crate::tar::append_regular(&mut builder, member, bytes, 0o644).expect("notice");
        }
        for (name, bytes) in &layout.extra {
            crate::tar::append_regular(&mut builder, name, bytes, 0o644).expect("extra");
        }
        builder.finish().expect("tar finish");
        builder
            .into_inner()
            .expect("gzip encoder")
            .finish()
            .expect("gzip finish")
    }

    #[test]
    fn windows_x86_64_is_pinned_and_windows_aarch64_is_not() {
        const PINS: &[(&str, &str, &str)] = &[
            (
                "linux-x86_64",
                "49ab3afbd4e6c1e284b5f2898129c8bb8a10fd785c1c5392c8c1fc70242f9ced",
                "687dce861f959c7097d47c5864509d51a926a71b38322596a8ee3e7a99c6b96e",
            ),
            (
                "linux-aarch64",
                "00551476a77fbc1a31c37573eadc9b63f1c366f65ad727539326927da083bb4d",
                "933f3d620cc8b58fb30a7f12a1bce8bf276da65caf39ff8fb2d04bc1268d53a3",
            ),
            (
                "macos-x86_64",
                "0c78b8d55a4c97e02c9bb516997253cb972739373009cf29554c959a2f6b194a",
                "8fdf8fc61c85676515321b0c214fb1afa0e157cffdadbdff40802e7b4bed7ad6",
            ),
            (
                "macos-arm64",
                "c032aa59be58b0f12e41e76a8ef707e347b9841b0426446f646b2568d350ec4f",
                "df568fcd17a6a6296956aa79abea1181db187458432f360b084fec1cea7cd4d9",
            ),
            (
                "windows-x86_64",
                "bf25149815b34b00042f48a886653d469c817529dd9cccabb4b509b6465a9526",
                "0aa3abb1aa20798094c1a5f2d8cdea45b24a6e12cdc6c774de261dd522dbdf81",
            ),
        ];
        for (key, archive_sha256, library_sha256) in PINS {
            let spec = spec_for(key).unwrap_or_else(|| panic!("missing {key}"));
            assert_eq!(spec.archive_sha256, *archive_sha256, "{key} archive");
            assert_eq!(spec.library_sha256, *library_sha256, "{key} library");
        }
        let windows = spec_for("windows-x86_64").expect("windows-x86_64");
        assert_eq!(windows.archive_name, "pdfium-win-x64.tgz");
        assert_eq!(windows.library_member, "bin/pdfium.dll");
        assert_eq!(windows.library_name, "pdfium.dll");
        assert!(spec_for("windows-aarch64").is_none());
    }

    #[test]
    fn windows_expected_members_include_bin_and_the_import_library() {
        let windows = expected_members(spec_for("windows-x86_64").expect("windows-x86_64"));
        assert!(windows.contains("bin"));
        assert!(windows.contains("lib/pdfium.dll.lib"));
        let linux = expected_members(spec_for("linux-x86_64").expect("linux-x86_64"));
        assert!(!linux.contains("bin"));
        assert!(!linux.contains("lib/pdfium.dll.lib"));
    }

    #[test]
    fn windows_notice_sha256_overrides_only_the_crlf_files() {
        let spec = spec_for("windows-x86_64").expect("windows-x86_64");
        for (member, digest) in WINDOWS_NOTICE_SHA256 {
            assert_eq!(notice_sha256(spec, member), Some(*digest), "{member}");
            let generic = NOTICE_SHA256
                .iter()
                .find(|(name, _)| name == member)
                .map(|(_, generic)| *generic);
            assert_ne!(notice_sha256(spec, member), generic, "{member} generic");
        }
        assert_eq!(
            notice_sha256(spec, "licenses/agg23.txt"),
            Some("c110d3ea2ad77467ce0dcff7d3337e6c8be8049a5103f4b9bd5fd911a77972e5")
        );
        assert_eq!(
            notice_sha256(spec, "licenses/pdfium.txt"),
            Some("961eacd9633fff6d051db7208b755e9210e30efac7adec3e6a6d52798f0ccf0e")
        );
        assert_ne!(
            notice_sha256(spec, "licenses/pdfium.txt"),
            Some(MACOS_PDFIUM_NOTICE_SHA256)
        );
    }

    #[test]
    fn windows_stage_from_bytes_succeeds_and_drops_the_import_library() {
        let archive = gzip_windows_archive(&default_windows_layout());
        let spec = windows_spec_matching(&archive, PLACEHOLDER_DLL);
        let staged = stage_from_bytes(&spec, &archive).expect("windows archive");
        assert_eq!(staged.library, PLACEHOLDER_DLL);
        assert_eq!(staged.notices.len(), NOTICE_MEMBERS.len());
        for member in NOTICE_MEMBERS {
            let name = Path::new(member)
                .file_name()
                .and_then(|name| name.to_str())
                .expect("notice basename");
            assert_eq!(
                staged.notices.get(name).map(Vec::as_slice),
                Some(windows_license(member)),
                "{member}"
            );
        }
        assert!(!staged.notices.contains_key("pdfium.dll.lib"));
        assert!(!staged.notices.contains_key("lib/pdfium.dll.lib"));
    }

    #[test]
    fn windows_archive_missing_the_dll_is_refused() {
        let mut layout = default_windows_layout();
        layout.dll = None;
        let archive = gzip_windows_archive(&layout);
        let spec = windows_spec_matching(&archive, PLACEHOLDER_DLL);
        let error = stage_from_bytes(&spec, &archive)
            .expect_err("missing dll")
            .to_string();
        assert!(error.contains("bin/pdfium.dll"), "{error}");
        assert!(error.contains("missing:"), "{error}");
    }

    #[test]
    fn windows_archive_missing_the_import_library_is_refused() {
        let mut layout = default_windows_layout();
        layout.import_lib = None;
        let archive = gzip_windows_archive(&layout);
        let spec = windows_spec_matching(&archive, PLACEHOLDER_DLL);
        let error = stage_from_bytes(&spec, &archive)
            .expect_err("missing import lib")
            .to_string();
        assert!(error.contains("lib/pdfium.dll.lib"), "{error}");
        assert!(error.contains("missing:"), "{error}");
    }

    #[test]
    fn windows_archive_with_an_unexpected_member_is_refused() {
        let mut layout = default_windows_layout();
        layout.extra.push(("unexpected.txt", b"nope".as_slice()));
        let archive = gzip_windows_archive(&layout);
        let spec = windows_spec_matching(&archive, PLACEHOLDER_DLL);
        let error = stage_from_bytes(&spec, &archive)
            .expect_err("unexpected member")
            .to_string();
        assert!(error.contains("unexpected.txt"), "{error}");
        assert!(error.contains("unexpected:"), "{error}");
    }

    #[test]
    fn windows_library_digest_mismatch_is_refused() {
        let archive = gzip_windows_archive(&default_windows_layout());
        let mut spec = spec_for("windows-x86_64").expect("windows-x86_64").clone();
        spec.archive_sha256 = leak_sha256(&archive);
        let error = stage_from_bytes(&spec, &archive)
            .expect_err("library digest")
            .to_string();
        assert!(
            error.contains("PDFium extracted member digest mismatch: bin/pdfium.dll"),
            "{error}"
        );
        assert!(error.contains(spec.library_sha256), "{error}");
        assert!(error.contains(&sha256_hex(PLACEHOLDER_DLL)), "{error}");
    }

    #[test]
    fn windows_notice_digest_mismatch_is_refused() {
        let mut layout = default_windows_layout();
        layout
            .notices
            .insert("licenses/abseil.txt", b"corrupted-abseil".as_slice());
        let archive = gzip_windows_archive(&layout);
        let spec = windows_spec_matching(&archive, PLACEHOLDER_DLL);
        let error = stage_from_bytes(&spec, &archive)
            .expect_err("notice digest")
            .to_string();
        assert!(
            error.contains("PDFium extracted member digest mismatch: licenses/abseil.txt"),
            "{error}"
        );
        assert!(
            error.contains("f54fff0b905df5b3464527c652a30e903b172d6dcab4d89b5e6f105d5e4a4603"),
            "{error}"
        );
        assert!(
            !error.contains("c79a7fea0e3cac04cd43f20e7b648e5a0ff8fa5344e644b0ee09ca1162b62747"),
            "{error}"
        );
    }

    #[test]
    fn windows_staged_member_names_exclude_the_import_library() {
        let spec = spec_for("windows-x86_64").expect("windows-x86_64");
        let names = staged_member_names(spec);
        assert!(names.contains("pdfium.dll"));
        for member in NOTICE_MEMBERS {
            let basename = Path::new(member)
                .file_name()
                .and_then(|name| name.to_str())
                .expect("notice basename");
            assert!(names.contains(basename), "{basename}");
        }
        assert!(!names.contains("pdfium.dll.lib"));
        assert!(!names.contains("lib/pdfium.dll.lib"));
        assert_eq!(names.len(), 1 + NOTICE_MEMBERS.len());
    }

    #[test]
    fn windows_write_staged_library_does_not_write_the_import_library() {
        let spec = spec_for("windows-x86_64").expect("windows-x86_64");
        let mut notices = BTreeMap::new();
        for member in NOTICE_MEMBERS {
            let name = Path::new(member)
                .file_name()
                .and_then(|name| name.to_str())
                .expect("notice basename");
            notices.insert(name.to_owned(), windows_license(member).to_vec());
        }
        let staged = StagedRuntime {
            library: PLACEHOLDER_DLL.to_vec(),
            notices,
        };
        let dir = tempfile::tempdir().expect("tempdir");
        write_staged_library(spec, &staged, dir.path()).expect("stage");
        let files = crate::stage::staged_files(dir.path()).expect("listing");
        assert!(files.contains(&"pdfium.dll".to_owned()));
        assert!(!files.iter().any(|name| name.contains("pdfium.dll.lib")));
        assert_eq!(files.len(), 1 + NOTICE_MEMBERS.len());
    }
}
