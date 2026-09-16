// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Local Windows production from fixed, typed input paths. Input files provide
//! no digests, output destinations, trust overrides or new admission authority.

use std::collections::BTreeSet;
use std::fs::OpenOptions;
use std::io::Write;
use std::path::{Component, Path, PathBuf};

use serde::Deserialize;

use super::windows_archives::{admit_msvc, admit_pdfium, admit_rclone, admit_restic};
use super::windows_build::{build_windows_product, capture_source, read_bounded};
use super::windows_inputs::{
    ControlledInputPaths, OnnxInputPaths, RfdetrInputPaths, admit_ced, admit_ffmpeg_notices,
    admit_onnx, admit_parakeet, admit_rfdetr,
};
use super::windows_stage::{AdmittedWindowsNativeInputs, stage_windows_payload};

const USAGE: &str = "produce windows-x86_64 DEST --inputs LOCAL_JSON --logs FRESH_DIRECTORY (absolute paths required)";

#[derive(Debug, Deserialize)]
struct LockPackage {
    name: String,
    version: String,
    source: Option<String>,
}

#[derive(Debug, Deserialize)]
struct Lock {
    #[serde(default)]
    package: Vec<LockPackage>,
}

/// The digest `population.source` binds to: every external (non-workspace)
/// `[[package]]` row in `Cargo.lock`, identified the same way the committed
/// index identifies its own rows (`name@version (source)`). A workspace
/// member has no `source` and is excluded, so a workspace-internal
/// dependency edge cannot move this digest -- only an added, removed, or
/// upgraded external package can.
fn external_population_sha256(lock: &[u8]) -> Result<String, String> {
    let text = std::str::from_utf8(lock).map_err(|e| e.to_string())?;
    let parsed: Lock = toml_edit::de::from_str(text).map_err(|e| e.to_string())?;
    let mut identities: Vec<String> = parsed
        .package
        .into_iter()
        .filter_map(|package| {
            package
                .source
                .map(|source| format!("{}@{} ({source})", package.name, package.version))
        })
        .collect();
    identities.sort();
    Ok(crate::digest::sha256_hex(identities.join("\n").as_bytes()))
}

/// Every path `core/Cargo.toml`'s own `[workspace]` table declares -- as a
/// `members` entry or an `exclude` entry -- resolved to that path's own
/// `Cargo.toml` `[package].name`. This is the checkable form of "sol pbc's
/// own" CLO's 2026-09-16 sign-off requires: excluding a crate from the
/// workspace build does not disown it, so both lists count.
fn workspace_owned_crate_names(repo: &Path) -> Result<BTreeSet<String>, String> {
    let workspace_manifest = repo.join("core/Cargo.toml");
    let text = std::fs::read_to_string(&workspace_manifest)
        .map_err(|e| format!("{}: {e}", workspace_manifest.display()))?;
    let doc: toml_edit::DocumentMut = text
        .parse()
        .map_err(|e: toml_edit::TomlError| e.to_string())?;
    let workspace = doc
        .get("workspace")
        .and_then(|item| item.as_table())
        .ok_or_else(|| format!("{}: no [workspace] table", workspace_manifest.display()))?;
    let mut declared_paths = Vec::new();
    for key in ["members", "exclude"] {
        if let Some(array) = workspace.get(key).and_then(|item| item.as_array()) {
            declared_paths.extend(
                array
                    .iter()
                    .filter_map(|item| item.as_str().map(str::to_string)),
            );
        }
    }
    let mut names = BTreeSet::new();
    for path in declared_paths {
        let member_manifest = repo.join("core").join(&path).join("Cargo.toml");
        let member_text = std::fs::read_to_string(&member_manifest)
            .map_err(|e| format!("{}: {e}", member_manifest.display()))?;
        let member_doc: toml_edit::DocumentMut = member_text
            .parse()
            .map_err(|e: toml_edit::TomlError| e.to_string())?;
        let name = member_doc
            .get("package")
            .and_then(|item| item.get("name"))
            .and_then(|item| item.as_str())
            .ok_or_else(|| format!("{}: no [package].name", member_manifest.display()))?;
        names.insert(name.to_string());
    }
    Ok(names)
}

/// Cargo.lock rows with no `source` that CLO's 2026-09-16 sign-off already
/// reviewed and accepted as third party despite that: vendored via
/// `[patch.crates-io]` (`core/Cargo.toml`), which strips `source` the same
/// way a workspace member's absence does. Adding a name here is itself the
/// loud, reviewed admission the invariant below requires -- not a silent
/// pass. `ffmpeg-sys-next`'s redistribution-basis determination is a
/// separate, open question carried on `clo-50`; it is not a condition on
/// this check.
const KNOWN_THIRD_PARTY_NO_SOURCE_EXCEPTIONS: &[&str] = &["ffmpeg-sys-next"];

/// The condition CLO's 2026-09-16 sign-off puts on landing the rebind above:
/// a Cargo.lock row with no `source` that is not sol pbc's own must not
/// pass silently. This is a guard on the no-source population, not a
/// notices change -- it adds no package to `windows-rust-NOTICES.txt` and
/// leaves `population.source_sha256` untouched.
fn assert_no_source_population_is_accounted_for(
    lock: &[u8],
    owned: &BTreeSet<String>,
) -> Result<(), String> {
    let text = std::str::from_utf8(lock).map_err(|e| e.to_string())?;
    let parsed: Lock = toml_edit::de::from_str(text).map_err(|e| e.to_string())?;
    let unaccounted: Vec<String> = parsed
        .package
        .into_iter()
        .filter(|package| package.source.is_none())
        .map(|package| package.name)
        .filter(|name| {
            !owned.contains(name)
                && !KNOWN_THIRD_PARTY_NO_SOURCE_EXCEPTIONS.contains(&name.as_str())
        })
        .collect();
    if !unaccounted.is_empty() {
        return Err(format!(
            "Cargo.lock package(s) with no `source` are neither a declared \
             core/Cargo.toml [workspace] member/exclude nor a named \
             third-party exception -- not sol pbc's own by the checkable \
             definition, and must not pass silently: {}",
            unaccounted.join(", ")
        ));
    }
    Ok(())
}

fn validate_rust_notices(index: &[u8], notices: &[u8], lock: &[u8]) -> Result<(), String> {
    let index: serde_json::Value = serde_json::from_slice(index).map_err(|e| e.to_string())?;
    let population_sha256 = external_population_sha256(lock)?;
    if index["schema"].as_str() != Some("solstone.windows-rust-notices.v2")
        || index["population"]["source_sha256"].as_str() != Some(&population_sha256)
        || index["notices_sha256"].as_str() != Some(&crate::digest::sha256_hex(notices))
    {
        return Err(
            "Windows Rust notices do not match the current external package population and notice bytes"
                .into(),
        );
    }
    Ok(())
}

#[derive(Debug)]
struct InputPath(PathBuf);

impl<'de> Deserialize<'de> for InputPath {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let path = PathBuf::deserialize(deserializer)?;
        require_absolute(&path).map_err(serde::de::Error::custom)?;
        Ok(Self(path))
    }
}

fn require_absolute(path: &Path) -> Result<(), String> {
    if !path.is_absolute() || path.components().any(|part| part == Component::ParentDir) {
        return Err(format!(
            "absolute path without parent traversal required: {}",
            path.display()
        ));
    }
    Ok(())
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ControlledFiles {
    receipt: InputPath,
    evidence: InputPath,
    validation: InputPath,
    source_archive: InputPath,
    cmake_archive: InputPath,
    output_root: InputPath,
}

impl ControlledFiles {
    fn paths(&self) -> ControlledInputPaths<'_> {
        ControlledInputPaths {
            receipt: &self.receipt.0,
            evidence: &self.evidence.0,
            validation: &self.validation.0,
            source_archive: &self.source_archive.0,
            cmake_archive: &self.cmake_archive.0,
            output_root: &self.output_root.0,
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ParakeetFiles {
    build: ControlledFiles,
    model: InputPath,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct OnnxFiles {
    build: ControlledFiles,
    mirror_archive: InputPath,
    python_archive: InputPath,
    protoc_archive: InputPath,
    cmake_cache: InputPath,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RfdetrFiles {
    build: ControlledFiles,
    ggml_bundle: InputPath,
    cmake_cache: InputPath,
    build_options: InputPath,
    subprocess_evidence: InputPath,
    license: InputPath,
    ggml_license: InputPath,
    stb_license: InputPath,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ArchiveWithLicense {
    archive: InputPath,
    license: InputPath,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct MsvcFiles {
    archive: InputPath,
    runtime_license: InputPath,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct LocalInputs {
    ced: ControlledFiles,
    parakeet: ParakeetFiles,
    onnx: OnnxFiles,
    rfdetr: RfdetrFiles,
    restic: ArchiveWithLicense,
    rclone: ArchiveWithLicense,
    msvc: MsvcFiles,
    pdfium_archive: InputPath,
    ffmpeg_archive: InputPath,
}

impl LocalInputs {
    fn admit(&self, repo: &Path) -> Result<AdmittedWindowsNativeInputs, String> {
        let controlled = vec![
            admit_ced(repo, self.ced.paths())?,
            admit_parakeet(repo, self.parakeet.build.paths(), &self.parakeet.model.0)?,
            admit_onnx(
                repo,
                OnnxInputPaths {
                    build: self.onnx.build.paths(),
                    mirror_archive: &self.onnx.mirror_archive.0,
                    python_archive: &self.onnx.python_archive.0,
                    protoc_archive: &self.onnx.protoc_archive.0,
                    cmake_cache: &self.onnx.cmake_cache.0,
                },
            )?,
            admit_rfdetr(
                repo,
                RfdetrInputPaths {
                    build: self.rfdetr.build.paths(),
                    ggml_bundle: &self.rfdetr.ggml_bundle.0,
                    cmake_cache: &self.rfdetr.cmake_cache.0,
                    build_options: &self.rfdetr.build_options.0,
                    subprocess_evidence: &self.rfdetr.subprocess_evidence.0,
                    license: &self.rfdetr.license.0,
                    ggml_license: &self.rfdetr.ggml_license.0,
                    stb_license: &self.rfdetr.stb_license.0,
                },
            )?,
        ];
        AdmittedWindowsNativeInputs::from_controlled(controlled)?.with_archives(vec![
            admit_restic(&self.restic.archive.0, &self.restic.license.0)?,
            admit_rclone(&self.rclone.archive.0, &self.rclone.license.0)?,
            admit_msvc(&self.msvc.archive.0, &self.msvc.runtime_license.0)?,
            admit_pdfium(&self.pdfium_archive.0)?,
            admit_ffmpeg_notices(repo, &self.ffmpeg_archive.0)?,
        ])
    }
}

#[derive(Debug)]
struct Args {
    destination: PathBuf,
    inputs: PathBuf,
    logs: PathBuf,
}

fn parse_args(args: &[String]) -> Result<Args, String> {
    let Some(destination) = args.first() else {
        return Err(USAGE.into());
    };
    let mut inputs = None;
    let mut logs = None;
    let mut flags = args[1..].chunks_exact(2);
    for flag in &mut flags {
        match flag[0].as_str() {
            "--inputs" if inputs.is_none() => inputs = Some(PathBuf::from(&flag[1])),
            "--logs" if logs.is_none() => logs = Some(PathBuf::from(&flag[1])),
            _ => {
                return Err(format!(
                    "unknown or duplicate Windows production option: {}; {USAGE}",
                    flag[0]
                ));
            }
        }
    }
    if !flags.remainder().is_empty() {
        return Err(USAGE.into());
    }
    let result = Args {
        destination: destination.into(),
        inputs: inputs.ok_or(USAGE)?,
        logs: logs.ok_or(USAGE)?,
    };
    for path in [&result.destination, &result.inputs, &result.logs] {
        require_absolute(path)?;
    }
    Ok(result)
}

/// The reviewed native wrapper owns the existing host fence, finite wait,
/// isolated tool environment and cleanup evidence. Build tools remain Unowned.
/// This command produces an unsigned local tree; it cannot sign or promote it.
pub fn run_cli(start: &Path, args: &[String]) -> Result<String, String> {
    let args = parse_args(args)?;
    if !cfg!(windows) {
        return Err("Windows production requires its native MSVC host".into());
    }
    let inventory_path = crate::inventory::repository_inventory_path(start)
        .ok_or("could not locate canonical distribution inventory")?;
    let inventory =
        crate::validate_distribution_inventory(&inventory_path).map_err(|e| e.to_string())?;
    let repo = inventory_path
        .ancestors()
        .nth(3)
        .ok_or("missing repository root")?;
    let before = capture_source(repo)?;
    let lock_bytes = read_bounded(
        &super::windows_stage::join_components(repo, "core/Cargo.lock"),
        4 * 1024 * 1024,
    )?;
    validate_rust_notices(
        &read_bounded(
            &super::windows_stage::join_components(
                repo,
                "core/distribution/windows-rust-sources.json",
            ),
            4 * 1024 * 1024,
        )?,
        &read_bounded(
            &super::windows_stage::join_components(
                repo,
                "core/distribution/windows-rust-NOTICES.txt",
            ),
            16 * 1024 * 1024,
        )?,
        &lock_bytes,
    )?;
    assert_no_source_population_is_accounted_for(&lock_bytes, &workspace_owned_crate_names(repo)?)?;
    let input_bytes = read_bounded(&args.inputs, 1024 * 1024)?;
    let inputs: LocalInputs = serde_json::from_slice(&input_bytes).map_err(|e| e.to_string())?;
    let native = inputs.admit(repo)?;
    if capture_source(repo)? != before {
        return Err("product source changed during native input admission".into());
    }
    let product = build_windows_product(repo, &inventory, &args.logs, &inputs.ffmpeg_archive.0)?;
    // Retain the actual operator file, including original spelling, independently
    // of the inventory and original dependency receipts. No input file is a grant.
    let mut retained_inputs = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(args.logs.join("input-paths.json"))
        .map_err(|e| e.to_string())?;
    retained_inputs
        .write_all(&input_bytes)
        .map_err(|e| e.to_string())?;
    retained_inputs.sync_all().map_err(|e| e.to_string())?;
    let (staged, published) = stage_windows_payload(repo, &product, &native, &args.destination)
        .map_err(|e| e.to_string())?;
    // Informational command output. The existing unsigned manifest remains the
    // payload identity; this summary never admits a replacement tree or bytes.
    serde_json::to_string(&serde_json::json!({
        "target": "windows-x86_64",
        "source": product.evidence().source,
        "destination": published.destination,
        "unsigned_manifest_sha256": staged.unsigned_manifest_sha256,
        "durability_proven": published.durability_proven,
        "signed": false,
        "files": staged.files.iter().map(|file| serde_json::json!({
            "path": file.dest, "sha256": file.digest, "mode": file.mode,
        })).collect::<Vec<_>>(),
        "runtime_edges": staged.runtime_edges.iter().map(|edge| serde_json::json!({
            "importer": edge.importer, "kind": edge.kind, "library": edge.library, "member": edge.member,
        })).collect::<Vec<_>>(),
    })).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    // If this test is red, the external (non-workspace) Cargo.lock package
    // population moved and the Windows Rust notices no longer describe it.
    // The fix is to refresh them: rebuild and republish the
    // `dependency_source_companion` archive for the new lock, then
    // regenerate the index. `scripts/refresh_windows_rust_notices.py` does
    // this -- see its module docstring -- for the cases where the vendored
    // bytes provably cannot have moved; it refuses with a clear reason when
    // the external population moved, or when the Windows notice closure moved,
    // either of which needs a fresh `cargo vendor` acquisition instead. A
    // resolved-graph change that leaves that closure untouched is admitted, on
    // the measurement rather than on assertion.
    //
    // DO NOT hand-edit `population.source_sha256` or `cargo_lock_sha256` on
    // their own. `validate_rust_notices` above checks `population.source_sha256`
    // (recomputed from the current lock's external packages) and
    // `notices_sha256` -- `cargo_lock_sha256` is retained only as the anchor
    // `dependency_source_companion` is keyed to for the refresh tool's own
    // `--prior-archive` bookkeeping, and no longer gates this check. Editing
    // either by hand leaves the companion archive naming or contents stale
    // for bytes nobody produced. The shortcut is reachable, it is one line,
    // and it is the reason this comment is here rather than in a tracker.
    //
    // Reached 2026-09-12 by an ordinary dependency pin bump; the bump was
    // reverted rather than the binding weakened.
    //
    // IGNORED 2026-09-15: this check has zero Windows-specific compilation and
    // was running in the default `--workspace --lib --bins` sweep, so any
    // workspace-internal dependency-edge change anywhere in the ~620-package
    // lock could red ordinary, non-Windows journal dev. It is a deliberate
    // pre-release step in the Windows release procedure now, rather than new
    // automation. Run it explicitly with `cargo test -p
    // solstone-core-distribution --lib -- --ignored
    // committed_rust_notices_match_workspace_lock` before cutting a Windows
    // release.
    //
    // REBOUND 2026-09-16: the trigger above was the whole-workspace-lock
    // digest, so a workspace-internal-only dependency edge (no external
    // package touched) still reddened it on every workspace build where
    // someone happened to run the ignored test by hand. The attestation now
    // binds `population.source_sha256` -- the digest of the complete
    // Cargo.lock external-source set (`population.source`, 484 packages
    // today), strictly broader than the Windows notice closure itself
    // (`population.notices`, 396 packages, "conservative inclusion, not
    // exact PE link graph" per `population.metadata_feature_scope`) -- so a
    // workspace-internal edge no longer reds this check, and any added,
    // removed, or upgraded external package still does.
    #[ignore = "run manually before a Windows release per the release playbook, not on every workspace build (2026-09-15)"]
    #[test]
    fn committed_rust_notices_match_workspace_lock() {
        validate_rust_notices(
            include_bytes!("../../../../distribution/windows-rust-sources.json"),
            include_bytes!("../../../../distribution/windows-rust-NOTICES.txt"),
            include_bytes!("../../../../Cargo.lock"),
        )
        .expect("refresh Windows Rust notices when the external package population changes");
    }

    // Unlike the ignored test above, this one does not red on an ordinary
    // workspace-internal dependency edge -- it only reds when a no-`source`
    // row is neither a declared `core/Cargo.toml` `[workspace]` member/
    // exclude nor a named exception, which is a structural change to what
    // the workspace vendors or declares, not routine dependency churn. Runs
    // in the default `cargo test --lib` sweep.
    #[test]
    fn no_source_population_is_accounted_for_at_head() {
        let repo = Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/../../.."));
        let owned = workspace_owned_crate_names(repo)
            .expect("resolve core/Cargo.toml's [workspace] members/exclude to crate names");
        assert_no_source_population_is_accounted_for(
            include_bytes!("../../../../Cargo.lock"),
            &owned,
        )
        .expect(
            "Cargo.lock has a no-source package that is neither a declared workspace \
                 member/exclude nor a named third-party exception (see \
                 KNOWN_THIRD_PARTY_NO_SOURCE_EXCEPTIONS)",
        );
    }

    const LOCK: &[u8] = br#"
version = 4

[[package]]
name = "workspace-crate"
version = "0.1.0"
dependencies = [
 "external-crate",
]

[[package]]
name = "external-crate"
version = "1.2.3"
source = "registry+https://github.com/rust-lang/crates.io-index"
checksum = "deadbeef"
"#;

    // Same external population, a workspace-internal edge added (a second
    // workspace member, and a dependency edge onto it) -- the exact shape of
    // the defect this binding fixes: no external `[[package]]` moved.
    const WORKSPACE_EDGE_MOVED: &[u8] = br#"
version = 4

[[package]]
name = "workspace-crate"
version = "0.1.0"
dependencies = [
 "external-crate",
 "another-workspace-crate",
]

[[package]]
name = "another-workspace-crate"
version = "0.1.0"

[[package]]
name = "external-crate"
version = "1.2.3"
source = "registry+https://github.com/rust-lang/crates.io-index"
checksum = "deadbeef"
"#;

    // The external package itself moved -- this must still red.
    const EXTERNAL_PACKAGE_UPGRADED: &[u8] = br#"
version = 4

[[package]]
name = "workspace-crate"
version = "0.1.0"
dependencies = [
 "external-crate",
]

[[package]]
name = "external-crate"
version = "1.2.4"
source = "registry+https://github.com/rust-lang/crates.io-index"
checksum = "cafebabe"
"#;

    #[test]
    fn stale_lock_or_changed_rust_notices_refuse_before_production() {
        let notices = b"original upstream notices";
        let index = serde_json::to_vec(&serde_json::json!({
            "schema": "solstone.windows-rust-notices.v2",
            "population": {
                "source_sha256": external_population_sha256(LOCK).unwrap(),
            },
            "notices_sha256": crate::digest::sha256_hex(notices),
        }))
        .unwrap();
        assert!(validate_rust_notices(&index, notices, LOCK).is_ok());

        // A workspace-internal-only edge move leaves the external population
        // -- and therefore the gate -- untouched. Demonstrated, not argued.
        assert!(validate_rust_notices(&index, notices, WORKSPACE_EDGE_MOVED).is_ok());

        // An external package add/remove/upgrade moves the population digest
        // and reds, exactly as it should.
        assert!(validate_rust_notices(&index, notices, EXTERNAL_PACKAGE_UPGRADED).is_err());

        assert!(validate_rust_notices(&index, b"replaced", LOCK).is_err());
        assert!(validate_rust_notices(b"{}", notices, LOCK).is_err());
        assert!(validate_rust_notices(b"not-json", notices, LOCK).is_err());
    }

    const LOCK_UNOWNED_NO_SOURCE: &[u8] = br#"
version = 4

[[package]]
name = "workspace-crate"
version = "0.1.0"

[[package]]
name = "vendored-third-party"
version = "1.0.0"
"#;

    const LOCK_EXCEPTION_NO_SOURCE: &[u8] = br#"
version = 4

[[package]]
name = "workspace-crate"
version = "0.1.0"

[[package]]
name = "ffmpeg-sys-next"
version = "9.0.0"
"#;

    // The invariant CLO's 2026-09-16 sign-off conditions the rebind on: a
    // no-`source` row that is neither a declared workspace member/exclude
    // nor a named exception must not pass silently. This is the exact shape
    // of the gap the sign-off found -- a vendored `[patch.crates-io]`
    // package loses its `source` the same way a workspace member does.
    #[test]
    fn no_source_row_outside_workspace_and_exceptions_reds() {
        let owned = BTreeSet::from(["workspace-crate".to_string()]);
        assert!(
            assert_no_source_population_is_accounted_for(LOCK_UNOWNED_NO_SOURCE, &owned).is_err()
        );
    }

    #[test]
    fn no_source_row_that_is_a_named_exception_is_ok() {
        let owned = BTreeSet::from(["workspace-crate".to_string()]);
        assert!(
            assert_no_source_population_is_accounted_for(LOCK_EXCEPTION_NO_SOURCE, &owned).is_ok()
        );
    }

    fn arguments(root: &Path) -> Vec<String> {
        [
            root.join("payload").display().to_string(),
            "--inputs".into(),
            root.join("inputs.json").display().to_string(),
            "--logs".into(),
            root.join("logs").display().to_string(),
        ]
        .into()
    }

    #[test]
    fn command_requires_explicit_paths_and_refuses_extra_authorities() {
        let root = tempfile::tempdir().unwrap();
        let args = arguments(root.path());
        assert_eq!(parse_args(&args).unwrap().logs, root.path().join("logs"));
        for extra in [
            vec!["--inputs", "other"],
            vec!["--signature-key", "key"],
            vec!["trailing"],
        ] {
            let mut candidate = args.clone();
            candidate.extend(extra.into_iter().map(String::from));
            assert!(parse_args(&candidate).is_err());
        }
        assert!(parse_args(&args[..3]).is_err());
        let mut relative = args;
        relative[0] = "relative".into();
        assert!(parse_args(&relative).is_err());
    }

    #[test]
    fn local_input_objects_reject_relative_paths_and_unrecognized_evidence_fields() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("input");
        let value = serde_json::json!({"archive": path, "license": path});
        assert!(serde_json::from_value::<ArchiveWithLicense>(value.clone()).is_ok());
        for field in ["sha256", "destination", "signature", "optional"] {
            let mut candidate = value.clone();
            candidate[field] = serde_json::json!("override");
            assert!(serde_json::from_value::<ArchiveWithLicense>(candidate).is_err());
        }
        let mut relative = value;
        relative["license"] = serde_json::json!("relative");
        assert!(serde_json::from_value::<ArchiveWithLicense>(relative).is_err());
    }

    #[cfg(not(windows))]
    #[test]
    fn non_native_run_refuses_before_input_read_or_output_creation() {
        let root = tempfile::tempdir().unwrap();
        assert!(
            run_cli(root.path(), &arguments(root.path()))
                .unwrap_err()
                .contains("native MSVC host")
        );
        assert_eq!(std::fs::read_dir(root.path()).unwrap().count(), 0);
    }
}
