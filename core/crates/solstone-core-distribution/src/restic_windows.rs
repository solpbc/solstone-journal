// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Controlled source build of the pinned restic Windows ARM64 command.
//! The x64 release continues to use its independently pinned upstream archive.

use std::collections::BTreeMap;

use crate::controlled_build::{
    BuildConfiguration, BuilderIdentity, ControlledBuildReceiptDraft, DependencySource,
    InputIdentityEntry, SourceIdentity, SupportingArtifactRef,
};
use crate::digest::sha256_hex;
use solstone_core_installed_payload::WindowsArchitecture;

pub const VERSION: &str = "0.19.0";
pub const REVISION: &str = "12875cc48ed34111f71fe94a24a47ad11660f3a4";
pub const SOURCE_URL: &str =
    "https://github.com/restic/restic/releases/download/v0.19.0/restic-0.19.0.tar.gz";
pub const SOURCE_BYTES: u64 = 24_158_333;
pub const SOURCE_SHA256: &str = "800779b6c4c2396971c0567b09ccdd435e03155e1a0ec94e8bbf3d98641a8bc2";
pub const GO_URL: &str = "https://go.dev/dl/go1.26.4.windows-amd64.zip";
pub const GO_BYTES: u64 = 74_910_419;
pub const GO_SHA256: &str = "3ca8fb4630b07c419cbdd51f754e31363cfcfb83b3a5354d9e895c90be2cc345";
pub const MODULES_BYTES: u64 = 129_997_737;
pub const MODULES_SHA256: &str = "5fa7af01cd61dbecaa97bf6aa942cfcd1b109c3ae5a5c20946210c9c8428e163";
pub const MODULE_MANIFEST_SHA256: &str =
    "61ae6c281a83df96aed10d1527f5f87d7420894da1f4a6f413affe0275fbbdb6";
pub const GO_MOD_SHA256: &str = "a0c67833f2ca1485f1272e1214d22cac65c56641c24a7fad1491cde8d7c21d9a";
pub const GO_SUM_SHA256: &str = "94840327111dc755890d04c23208d5b17f549846494d4225445cc70ba8ed0b2b";
pub const OUTPUT_LABEL: &str = "restic.exe";
pub const EVIDENCE_LABEL: &str = "provenance/windows-aarch64/restic-build-evidence.json";
pub const EVIDENCE_SCHEMA: &str = "solstone.restic-windows-build-evidence.v1";
pub const SOURCE_LABEL: &str = "sources/restic-0.19.0.tar.gz";
pub const GO_LABEL: &str = "tools/go1.26.4.windows-amd64.zip";
pub const MODULES_LABEL: &str = "sources/restic-0.19.0-offline-modules.tar.gz";

fn identity(label: &str, bytes: u64, sha256: &str) -> InputIdentityEntry {
    InputIdentityEntry {
        label: label.into(),
        size: bytes,
        sha256: sha256.into(),
    }
}

pub fn pinned_inputs() -> Vec<InputIdentityEntry> {
    vec![
        identity(SOURCE_LABEL, SOURCE_BYTES, SOURCE_SHA256),
        identity(GO_LABEL, GO_BYTES, GO_SHA256),
        identity(MODULES_LABEL, MODULES_BYTES, MODULES_SHA256),
    ]
}

/// Fixed logical arguments. The driver supplies the exclusive output path.
/// No temporary `go run` host program, implicit VCS lookup, or network lookup.
pub fn build_arguments() -> Vec<String> {
    [
        "build",
        "-p=2",
        "-trimpath",
        "-buildvcs=false",
        "-tags=selfupdate,disable_grpc_modules",
        "-ldflags=-s -w -X main.version=0.19.0",
        "-o",
        OUTPUT_LABEL,
        "./cmd/restic",
    ]
    .into_iter()
    .map(str::to_owned)
    .collect()
}

pub fn build_environment() -> BTreeMap<String, String> {
    [
        ("GOOS", "windows"),
        ("GOARCH", "arm64"),
        ("GOARM64", "v8.0"),
        ("CGO_ENABLED", "0"),
        ("GOTOOLCHAIN", "local"),
        ("GOENV", "off"),
        ("GOWORK", "off"),
        ("GOFLAGS", "-mod=readonly"),
        ("GOPROXY", "off"),
        ("GOSUMDB", "off"),
    ]
    .into_iter()
    .map(|(k, v)| (k.into(), v.into()))
    .collect()
}

pub fn build_configuration() -> BuildConfiguration {
    let mut flags = build_arguments();
    flags.extend(
        build_environment()
            .into_iter()
            .map(|(k, v)| format!("{k}={v}")),
    );
    BuildConfiguration {
        target_triple: WindowsArchitecture::Aarch64.rust_triple().into(),
        profile: "release".into(),
        flags,
        network_access_denied: true,
    }
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ResticWindowsBuildEvidence {
    pub schema: String,
    pub inputs: Vec<InputIdentityEntry>,
    pub module_manifest_sha256: String,
    pub go_mod_sha256: String,
    pub go_sum_sha256: String,
    pub go_version: String,
    /// Every executable in the sealed Go archive's bin and host-tool folders.
    pub tools: Vec<InputIdentityEntry>,
    pub network_denied_tools: Vec<String>,
    pub arguments: Vec<String>,
    pub environment: BTreeMap<String, String>,
    pub build_log_sha256: String,
    pub go_build_info_sha256: String,
}

impl ResticWindowsBuildEvidence {
    /// Tool identities are independently derived from the pinned archive by
    /// the recorder and again at admission, never accepted from the report.
    pub fn validate(&self, expected_tools: &[InputIdentityEntry]) -> Result<(), String> {
        if self.schema != EVIDENCE_SCHEMA
            || self.inputs != pinned_inputs()
            || self.module_manifest_sha256 != MODULE_MANIFEST_SHA256
            || self.go_mod_sha256 != GO_MOD_SHA256
            || self.go_sum_sha256 != GO_SUM_SHA256
            || self.go_version != "go version go1.26.4 windows/amd64"
            || self.arguments != build_arguments()
            || self.environment != build_environment()
            || self.tools != expected_tools
            || expected_tools.is_empty()
        {
            return Err("restic source, offline inputs, tools or fixed recipe differ".into());
        }
        let denied: Vec<String> = expected_tools.iter().map(|t| t.label.clone()).collect();
        if self.network_denied_tools != denied {
            return Err("restic network denial does not cover the sealed Go tools".into());
        }
        for (field, value) in [
            ("build log", &self.build_log_sha256),
            ("Go build info", &self.go_build_info_sha256),
        ] {
            if value.len() != 64
                || !value
                    .bytes()
                    .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
            {
                return Err(format!("invalid restic {field} digest"));
            }
        }
        Ok(())
    }
}

pub fn source_identity(product: crate::provenance::Provenance) -> SourceIdentity {
    SourceIdentity {
        product,
        windows_dependency: DependencySource {
            repository: "https://github.com/restic/restic.git".into(),
            revision: REVISION.into(),
            content_sha256: SOURCE_SHA256.into(),
        },
    }
}

pub fn assemble_receipt_draft(
    source: SourceIdentity,
    evidence: &ResticWindowsBuildEvidence,
    expected_tools: &[InputIdentityEntry],
    builder: BuilderIdentity,
    output_bytes: &[u8],
) -> Result<ControlledBuildReceiptDraft, String> {
    evidence.validate(expected_tools)?;
    if source.windows_dependency != source_identity(source.product.clone()).windows_dependency {
        return Err("restic controlled source identity differs".into());
    }
    let dependencies =
        crate::pe_dependencies::inspect_dependencies(output_bytes, WindowsArchitecture::Aarch64)?;
    if dependencies.is_dll
        || dependencies
            .imports
            .iter()
            .map(|name| name.to_ascii_lowercase())
            .collect::<Vec<_>>()
            != ["kernel32.dll"]
        || !dependencies.delay_imports.is_empty()
        || !dependencies.forwarders.is_empty()
    {
        return Err(
            "restic requires an ARM64 command with only its qualified Go runtime dependency".into(),
        );
    }
    let outputs = crate::controlled_build::census_outputs(&[(OUTPUT_LABEL, output_bytes)])
        .map_err(|e| e.to_string())?;
    let evidence_bytes = serde_json::to_vec(evidence).map_err(|e| e.to_string())?;
    Ok(ControlledBuildReceiptDraft {
        source: Some(source),
        inputs: Some(pinned_inputs()),
        builder: Some(builder),
        configuration: Some(build_configuration()),
        outputs: Some(outputs),
        supporting: Some(vec![SupportingArtifactRef {
            label: EVIDENCE_LABEL.into(),
            sha256: sha256_hex(&evidence_bytes),
        }]),
        ..ControlledBuildReceiptDraft::default()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tools() -> Vec<InputIdentityEntry> {
        vec![identity("go/bin/go.exe", 1, &"a".repeat(64))]
    }
    fn evidence() -> ResticWindowsBuildEvidence {
        let tools = tools();
        ResticWindowsBuildEvidence {
            schema: EVIDENCE_SCHEMA.into(),
            inputs: pinned_inputs(),
            module_manifest_sha256: MODULE_MANIFEST_SHA256.into(),
            go_mod_sha256: GO_MOD_SHA256.into(),
            go_sum_sha256: GO_SUM_SHA256.into(),
            go_version: "go version go1.26.4 windows/amd64".into(),
            network_denied_tools: tools.iter().map(|t| t.label.clone()).collect(),
            tools,
            arguments: build_arguments(),
            environment: build_environment(),
            build_log_sha256: "b".repeat(64),
            go_build_info_sha256: "c".repeat(64),
        }
    }
    fn source() -> SourceIdentity {
        source_identity(crate::provenance::Provenance {
            commit: "d".repeat(40),
            lock_sha256: "e".repeat(64),
        })
    }
    fn exe(machine: u16, dll: bool, library: &str) -> Vec<u8> {
        let mut bytes = crate::pe_dependencies::tests::with_import(false);
        // The strict dependency-census fixture includes full PE headers and a
        // bounded ordinary import table. Use literal COFF machines here.
        bytes[0x84..0x86].copy_from_slice(&machine.to_le_bytes());
        let characteristics = if dll { 0x2022u16 } else { 0x22u16 };
        bytes[0x96..0x98].copy_from_slice(&characteristics.to_le_bytes());
        assert!(library.len() < 0x40);
        bytes[0x280..0x2c0].fill(0);
        bytes[0x280..0x280 + library.len()].copy_from_slice(library.as_bytes());
        bytes
    }
    #[test]
    fn fixed_offline_recipe_refuses_tool_and_configuration_substitution() {
        let good = evidence();
        let tools = tools();
        good.validate(&tools).unwrap();
        for key in ["GOPROXY", "GOARCH", "CGO_ENABLED", "GOTOOLCHAIN"] {
            let mut bad = good.clone();
            bad.environment.insert(key.into(), "foreign".into());
            assert!(bad.validate(&tools).is_err());
        }
        let mut bad = good.clone();
        *bad.arguments
            .iter_mut()
            .find(|arg| arg.starts_with("-tags="))
            .unwrap() = "-tags=selfupdate".into();
        assert!(bad.validate(&tools).is_err());
        let mut bad = good.clone();
        bad.inputs[2].sha256 = "f".repeat(64);
        assert!(bad.validate(&tools).is_err());
        let mut bad = good.clone();
        bad.go_version = "go version go1.26.5 windows/amd64".into();
        assert!(bad.validate(&tools).is_err());
        let mut bad = good.clone();
        bad.network_denied_tools.clear();
        assert!(bad.validate(&tools).is_err());
        let mut bad = good.clone();
        bad.tools[0].sha256 = "f".repeat(64);
        assert!(bad.validate(&tools).is_err());
        let mut bad = good;
        bad.build_log_sha256 = "not a digest".into();
        assert!(bad.validate(&tools).is_err());
    }
    #[test]
    fn original_source_and_native_pe_closure_are_required_before_receipt_assembly() {
        let build = |source: SourceIdentity, bytes: &[u8]| {
            assemble_receipt_draft(
                source,
                &evidence(),
                &tools(),
                BuilderIdentity {
                    host: "controlled-test-host".into(),
                    toolchain: "Go1.26.4".into(),
                },
                bytes,
            )
        };
        let good = exe(0xaa64, false, "KERNEL32.dll");
        let draft = build(source(), &good).unwrap();
        assert_eq!(
            draft.outputs.unwrap()[0].pre_signing_sha256,
            sha256_hex(&good)
        );
        for bytes in [
            exe(0x8664, false, "KERNEL32.dll"),
            exe(0xaa64, true, "KERNEL32.dll"),
            exe(0xaa64, false, "unauthorized.dll"),
        ] {
            assert!(build(source(), &bytes).is_err());
        }
        let mut other = source();
        other.windows_dependency.revision = "f".repeat(40);
        assert!(build(other, &good).is_err());
    }
}
