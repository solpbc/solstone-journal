// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Windows CED (ced.cpp + ggml) controlled-build identity: pins, validators,
//! and receipt-draft assembly. No build is performed here.

use std::collections::BTreeSet;
use std::fmt;

use crate::controlled_build::{
    BuildConfiguration, BuilderIdentity, ControlledBuildReceiptDraft, OutputIdentityEntry,
    SourceIdentity,
};
use crate::pe::{PeInfo, PeSymbol};
use crate::provenance::{self, ProvenanceError};

pub const CED_CPP_COMMIT: &str = "c04ac14b7992d00584d9e812c9bb6268598a6ce7";
pub const GGML_COMMIT: &str = "e705c5fed490514458bdd2eaddc43bd098fcce9b";

pub const CED_WINDOWS_TARGET_TRIPLE: &str = "x86_64-pc-windows-msvc";
pub const CED_WINDOWS_BUILD_PROFILE: &str = "Release";
// Intended CPU-only/shared-library CMake inputs for a future controlled build;
// not yet verified against upstream CMakeLists.txt (no build has run).
pub const CED_WINDOWS_BUILD_FLAGS: &[&str] = &[
    "-DBUILD_SHARED_LIBS=ON",
    "-DGGML_CUDA=OFF",
    "-DGGML_VULKAN=OFF",
    "-DGGML_OPENCL=OFF",
    "-DGGML_HIP=OFF",
    "-DGGML_METAL=OFF",
];

pub const CED_WINDOWS_EXPORTS: &[&str] = &[
    "ced_capi_abi_version",
    "ced_capi_load",
    "ced_capi_free",
    "ced_capi_last_error",
    "ced_capi_classify_pcm_json",
    "ced_capi_free_string",
];

/// Catalog identity for the GGUF model. `solstone-core-assets::ARTIFACTS`'s
/// `ced-model` row is the sole authority for that unit's sha256 and it is
/// deliberately not duplicated here.
pub const CED_MODEL_UNIT: &str = "ced-model";
pub const CED_MODEL_VERSION: &str = "b5e9a4aad6438763c8da16079d77563fbed35c65";
pub const CED_MODEL_FILENAME: &str = "ced-tiny-q8_0.gguf";
pub const CED_MODEL_SIZE_BYTES: u64 = 6_211_616;

pub const CED_DLL_OUTPUT_LABEL: &str = "bin/ced.dll";

const FORBIDDEN_IMPORT_NEEDLES: &[&str] = &[
    "cuda", "nvcuda", "cudart", "cublas", "cudnn", "nvrtc", "vulkan", "opencl",
];

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CedWindowsError {
    ExportAllowlist {
        missing: Vec<String>,
        unexpected: Vec<String>,
    },
    GpuImport {
        libraries: Vec<String>,
    },
}

impl fmt::Display for CedWindowsError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ExportAllowlist {
                missing,
                unexpected,
            } => write!(
                formatter,
                "CED Windows export set is not the reviewed C API\n  missing: {}\n  unexpected: {}",
                format_diff_side(missing),
                format_diff_side(unexpected)
            ),
            Self::GpuImport { libraries } => write!(
                formatter,
                "CED Windows import closure is not CPU-only\n  missing: <none>\n  unexpected: {}",
                format_diff_side(libraries)
            ),
        }
    }
}

impl std::error::Error for CedWindowsError {}

fn format_diff_side(names: &[String]) -> String {
    if names.is_empty() {
        "<none>".to_owned()
    } else {
        names.join(", ")
    }
}

pub fn verify_source_commits(identity: &SourceIdentity) -> Result<(), ProvenanceError> {
    provenance::require_commit(CED_CPP_COMMIT, &identity.product.commit)?;
    provenance::require_commit(GGML_COMMIT, &identity.windows_dependency.revision)?;
    Ok(())
}

pub fn ced_windows_build_configuration() -> BuildConfiguration {
    BuildConfiguration {
        target_triple: CED_WINDOWS_TARGET_TRIPLE.to_owned(),
        profile: CED_WINDOWS_BUILD_PROFILE.to_owned(),
        flags: CED_WINDOWS_BUILD_FLAGS
            .iter()
            .map(|flag| (*flag).to_owned())
            .collect(),
        network_access_denied: true,
    }
}

pub fn verify_exports(census: &PeInfo) -> Result<(), CedWindowsError> {
    let expected: BTreeSet<&str> = CED_WINDOWS_EXPORTS.iter().copied().collect();
    let actual: BTreeSet<String> = census
        .exports
        .iter()
        .map(|symbol| match symbol {
            PeSymbol::Named(name) => name.clone(),
            PeSymbol::Ordinal(ordinal) => format!("#{ordinal}"),
        })
        .collect();
    let missing: Vec<String> = expected
        .iter()
        .filter(|name| !actual.contains(**name))
        .map(|name| (*name).to_owned())
        .collect();
    let unexpected: Vec<String> = actual
        .into_iter()
        .filter(|name| !expected.contains(name.as_str()))
        .collect();
    if missing.is_empty() && unexpected.is_empty() {
        Ok(())
    } else {
        Err(CedWindowsError::ExportAllowlist {
            missing,
            unexpected,
        })
    }
}

pub fn verify_cpu_only_imports(census: &PeInfo) -> Result<(), CedWindowsError> {
    let libraries: Vec<String> = census
        .imports
        .iter()
        .filter(|library| {
            let lower = library.name.to_ascii_lowercase();
            FORBIDDEN_IMPORT_NEEDLES
                .iter()
                .any(|needle| lower.contains(needle))
        })
        .map(|library| library.name.clone())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();
    if libraries.is_empty() {
        Ok(())
    } else {
        Err(CedWindowsError::GpuImport { libraries })
    }
}

pub fn assemble_receipt_draft(
    source: SourceIdentity,
    builder: BuilderIdentity,
    outputs: Vec<OutputIdentityEntry>,
) -> ControlledBuildReceiptDraft {
    ControlledBuildReceiptDraft {
        source: Some(source),
        builder: Some(builder),
        configuration: Some(ced_windows_build_configuration()),
        outputs: Some(outputs),
        ..ControlledBuildReceiptDraft::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::controlled_build::{
        ControlledBuildReceiptError, DependencySource, ReceiptCategory, census_outputs,
    };
    use crate::pe::{FixtureSpec, ImportSpec, PeSymbolSpec, fixture, parse_pe};
    use crate::provenance::Provenance;

    fn pinned_source() -> SourceIdentity {
        SourceIdentity {
            product: Provenance {
                commit: CED_CPP_COMMIT.to_owned(),
                lock_sha256: String::new(),
            },
            windows_dependency: DependencySource {
                repository: "ggml".into(),
                revision: GGML_COMMIT.to_owned(),
                content_sha256: String::new(),
            },
        }
    }

    fn dummy_builder() -> BuilderIdentity {
        BuilderIdentity {
            host: "builder-1".into(),
            toolchain: "msvc".into(),
        }
    }

    fn named_exports<'a>(names: &'a [&'a str]) -> Vec<PeSymbolSpec<'a>> {
        names.iter().copied().map(PeSymbolSpec::Named).collect()
    }

    fn census_with_exports(exports: &[PeSymbolSpec<'_>]) -> PeInfo {
        parse_pe(&fixture(&FixtureSpec {
            dll: true,
            exports,
            ..FixtureSpec::default()
        }))
        .expect("fixture parses")
    }

    #[test]
    fn assemble_receipt_draft_is_missing_schema_first() {
        let draft = assemble_receipt_draft(pinned_source(), dummy_builder(), vec![]);
        match draft.validate() {
            Err(ControlledBuildReceiptError::Missing {
                category: ReceiptCategory::Schema,
            }) => {}
            other => panic!("expected Missing Schema, got {other:?}"),
        }
    }

    #[test]
    fn census_outputs_is_fail_closed_and_names_the_ced_dll() {
        let exports = named_exports(CED_WINDOWS_EXPORTS);
        let good = fixture(&FixtureSpec {
            dll: true,
            exports: &exports,
            ..FixtureSpec::default()
        });
        let bad = b"not-a-pe";
        match census_outputs(&[
            (CED_DLL_OUTPUT_LABEL, bad.as_slice()),
            ("other", good.as_slice()),
        ]) {
            Err(ControlledBuildReceiptError::OutputParse { label, .. }) => {
                assert_eq!(label, CED_DLL_OUTPUT_LABEL);
            }
            Ok(_) => panic!("must not return Ok with a partial census"),
            Err(other) => panic!("expected OutputParse, got {other:?}"),
        }
    }

    #[test]
    fn verify_source_commits_rejects_wrong_ced_cpp_commit() {
        let mut identity = pinned_source();
        identity.product.commit = "wrong-ced-cpp".into();
        let error = verify_source_commits(&identity).expect_err("wrong ced.cpp commit");
        assert!(
            error
                .to_string()
                .contains("mismatched-commit wrong-ced-cpp"),
            "{error}"
        );
    }

    #[test]
    fn verify_source_commits_rejects_wrong_ggml_revision() {
        let mut identity = pinned_source();
        identity.windows_dependency.revision = "wrong-ggml".into();
        let error = verify_source_commits(&identity).expect_err("wrong ggml revision");
        assert!(
            error.to_string().contains("mismatched-commit wrong-ggml"),
            "{error}"
        );
    }

    #[test]
    fn verify_source_commits_accepts_pinned_commits() {
        verify_source_commits(&pinned_source()).expect("pinned commits");
    }

    #[test]
    fn verify_exports_rejects_seventh_export() {
        let mut exports = named_exports(CED_WINDOWS_EXPORTS);
        exports.push(PeSymbolSpec::Named("ced_capi_extra_thing"));
        match verify_exports(&census_with_exports(&exports)) {
            Err(CedWindowsError::ExportAllowlist {
                missing,
                unexpected,
            }) => {
                assert!(missing.is_empty(), "{missing:?}");
                assert_eq!(unexpected, vec!["ced_capi_extra_thing".to_owned()]);
            }
            other => panic!("expected ExportAllowlist, got {other:?}"),
        }
    }

    #[test]
    fn verify_exports_rejects_missing_export() {
        let exports = named_exports(&CED_WINDOWS_EXPORTS[..5]);
        match verify_exports(&census_with_exports(&exports)) {
            Err(CedWindowsError::ExportAllowlist {
                missing,
                unexpected,
            }) => {
                assert_eq!(missing, vec!["ced_capi_free_string".to_owned()]);
                assert!(unexpected.is_empty(), "{unexpected:?}");
            }
            other => panic!("expected ExportAllowlist, got {other:?}"),
        }
    }

    #[test]
    fn verify_exports_accepts_exact_six_in_any_order() {
        let mut names = CED_WINDOWS_EXPORTS.to_vec();
        names.reverse();
        let exports = named_exports(&names);
        verify_exports(&census_with_exports(&exports)).expect("six exports in reverse order");
    }

    #[test]
    fn verify_cpu_only_imports_rejects_cuda_library() {
        let info = parse_pe(&fixture(&FixtureSpec {
            imports: &[ImportSpec {
                name: "nvcuda.dll",
                symbols: &[],
            }],
            ..FixtureSpec::default()
        }))
        .expect("fixture parses");
        match verify_cpu_only_imports(&info) {
            Err(CedWindowsError::GpuImport { libraries }) => {
                assert_eq!(libraries, vec!["nvcuda.dll".to_owned()]);
            }
            other => panic!("expected GpuImport, got {other:?}"),
        }
    }

    #[test]
    fn verify_cpu_only_imports_allows_empty_and_system_libs() {
        let empty = parse_pe(&fixture(&FixtureSpec::default())).expect("empty fixture");
        verify_cpu_only_imports(&empty).expect("empty imports");
        let system = parse_pe(&fixture(&FixtureSpec {
            imports: &[ImportSpec {
                name: "KERNEL32.dll",
                symbols: &[],
            }],
            ..FixtureSpec::default()
        }))
        .expect("system fixture");
        verify_cpu_only_imports(&system).expect("system imports");
    }
}
