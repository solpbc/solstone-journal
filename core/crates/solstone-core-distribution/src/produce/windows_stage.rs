// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Inventory-driven Windows staging from retained admitted input bytes.
//! Publication uses the existing Windows producer leaf; signing remains later.

use std::borrow::Cow;
use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

use super::windows_build::{BuiltWindowsProduct, capture_source, read_bounded};
use super::windows_inputs::AdmittedControlledInput;
use crate::digest::sha256_hex;
use crate::inventory::{
    Entry, Inventory, WindowsNativeComponent, digest_const_hex, load_inventory, load_payload,
};
use crate::record::FileRecord;
use crate::select::ArtifactId;
use crate::windows_payload::{
    WINDOWS_PAYLOAD_MANIFEST, WINDOWS_PAYLOAD_SIGNATURE, WINDOWS_PAYLOAD_TARGET,
    render_windows_payload_manifest,
};
use crate::windows_pe_closure::{RuntimeEdge, inspect_runtime_closure};
use crate::windows_publish::{
    PublishedWindowsPayload, WindowsPublishFailure, assemble_windows_payload,
};

/// A retained input set, keyed by component/output label, never destination.
/// Only dependency admission can supply its members; inventory maps them to paths.
#[derive(Default)]
pub struct AdmittedWindowsNativeInputs {
    members: BTreeMap<(WindowsNativeComponent, String), Vec<u8>>,
}

impl AdmittedWindowsNativeInputs {
    pub fn with_archives(
        mut self,
        inputs: Vec<super::windows_archives::AdmittedArchiveInput>,
    ) -> Result<Self, String> {
        for input in inputs {
            for (label, bytes) in input.members {
                self.insert(input.component, &label, bytes)?;
            }
        }
        Ok(self)
    }

    pub fn from_controlled(inputs: Vec<AdmittedControlledInput>) -> Result<Self, String> {
        let mut result = Self::default();
        for input in inputs {
            // These labels are disjoint and the input's constructors have already
            // verified the exact dependency-specific source/configuration/outputs.
            let component = match input.receipt().outputs.as_slice() {
                [output] if output.label == crate::ced_windows::CED_DLL_OUTPUT_LABEL => {
                    WindowsNativeComponent::Ced
                }
                [output]
                    if output.label
                        == crate::onnx_windows_source::ONNX_WINDOWS_DLL_OUTPUT_LABEL =>
                {
                    WindowsNativeComponent::Onnx
                }
                [output]
                    if output.label == crate::parakeet_windows::PARAKEET_SERVER_OUTPUT_LABEL =>
                {
                    WindowsNativeComponent::Parakeet
                }
                [output] if output.label == crate::rfdetr_windows::RFDETR_CLI_OUTPUT_LABEL => {
                    WindowsNativeComponent::Rfdetr
                }
                _ => return Err("unsupported controlled Windows input output set".into()),
            };
            for (label, bytes) in input.into_retained_members()? {
                result.insert(component, &label, bytes)?;
            }
        }
        Ok(result)
    }

    fn insert(
        &mut self,
        component: WindowsNativeComponent,
        member: &str,
        bytes: Vec<u8>,
    ) -> Result<(), String> {
        if bytes.is_empty() || self.members.contains_key(&(component, member.into())) {
            return Err(format!(
                "empty or duplicate admitted native member: {component:?}/{member}"
            ));
        }
        self.members.insert((component, member.into()), bytes);
        Ok(())
    }
}

struct PlannedFile<'a> {
    bytes: Cow<'a, [u8]>,
    record: FileRecord,
}

type Plan<'a> = BTreeMap<String, PlannedFile<'a>>;

#[derive(Debug)]
pub struct StagedWindowsPayload {
    pub files: Vec<FileRecord>,
    pub runtime_edges: Vec<RuntimeEdge>,
    pub unsigned_manifest_sha256: String,
}

/// Consumes live-Cargo evidence, not paths to old binaries or replacement logs.
/// The checkout's canonical inventory and content must still match that build.
/// The Windows command joins every native component through its typed admission.
pub fn stage_windows_payload(
    checkout: &Path,
    product: &BuiltWindowsProduct,
    native: &AdmittedWindowsNativeInputs,
    destination: &Path,
) -> Result<(StagedWindowsPayload, PublishedWindowsPayload), WindowsPublishFailure> {
    assemble_windows_payload(destination, |stage| {
        let source = &product.evidence().source;
        if &capture_source(checkout)? != source {
            return Err("staging checkout differs from fresh Cargo source".into());
        }
        let inventory_path = checkout.join("core/distribution/inventory.toml");
        let inventory = load_inventory(&inventory_path).map_err(|e| e.to_string())?;
        let plan = collect_plan(
            checkout,
            &inventory_path,
            &inventory,
            product.bytes(),
            native,
            product.evidence_bytes(),
        )?;
        if &capture_source(checkout)? != source {
            return Err("staging source changed while capturing content".into());
        }
        let runtime_edges = inspect_plan_runtime_closure(&plan)?;
        let files = write_plan(stage, &plan)?;
        let manifest = render_checked_manifest(stage, &plan, &source.commit, &source.lock_sha256)?;
        write_new(stage, WINDOWS_PAYLOAD_MANIFEST, &manifest, 0o644)?;
        Ok(StagedWindowsPayload {
            files,
            runtime_edges,
            unsigned_manifest_sha256: sha256_hex(&manifest),
        })
    })
}

fn inspect_plan_runtime_closure(plan: &Plan<'_>) -> Result<Vec<RuntimeEdge>, String> {
    inspect_runtime_closure(plan.iter().filter_map(|(path, file)| {
        let folded = path.to_ascii_lowercase();
        (folded.ends_with(".exe") || folded.ends_with(".dll"))
            .then_some((path.as_str(), file.bytes.as_ref()))
    }))
}

fn render_checked_manifest(
    root: &Path,
    plan: &Plan<'_>,
    commit: &str,
    lock: &str,
) -> Result<Vec<u8>, String> {
    let bytes = render_windows_payload_manifest(root, commit, lock).map_err(|e| e.to_string())?;
    let manifest: crate::windows_payload::WindowsPayloadManifest =
        serde_json::from_slice(&bytes).map_err(|e| e.to_string())?;
    if manifest.files.len() != plan.len()
        || manifest.files.iter().any(|file| {
            !plan.get(&file.path).is_some_and(|expected| {
                file.sha256 == expected.record.digest && file.bytes == expected.bytes.len() as u64
            })
        })
    {
        return Err("unsigned manifest differs from retained staging admission".into());
    }
    Ok(bytes)
}

fn collect_plan<'a>(
    checkout: &Path,
    inventory_path: &Path,
    inventory: &Inventory,
    products: &'a BTreeMap<ArtifactId, Vec<u8>>,
    native: &'a AdmittedWindowsNativeInputs,
    build_evidence: &'a [u8],
) -> Result<Plan<'a>, String> {
    let target = inventory
        .target
        .iter()
        .find(|target| target.id == WINDOWS_PAYLOAD_TARGET)
        .ok_or("missing Windows inventory target")?;
    let mut plan = Plan::new();
    let mut binaries = BTreeSet::new();
    let mut native_members = BTreeSet::new();
    let mut evidence_selected = false;
    for entry in &inventory.entry {
        match entry {
            Entry::WindowsBuildEvidence {
                dest,
                mode,
                targets,
            } if targets.iter().any(|t| t == WINDOWS_PAYLOAD_TARGET) => {
                if evidence_selected || build_evidence.is_empty() {
                    return Err("missing or duplicate fresh Cargo evidence mapping".into());
                }
                evidence_selected = true;
                add_file(&mut plan, dest, *mode, Cow::Borrowed(build_evidence))?;
            }
            Entry::Bin {
                package,
                bin,
                dest,
                mode,
                lane,
                targets,
            } if targets.iter().any(|t| t == WINDOWS_PAYLOAD_TARGET) => {
                if inventory.forbidden_bins().contains(bin) {
                    return Err(format!("forbidden Windows binary: {bin}"));
                }
                let id = ArtifactId {
                    package: package.clone(),
                    bin: bin.clone(),
                    triple: target.triple_for_lane(target.lane_for(lane)).into(),
                };
                let bytes = products
                    .get(&id)
                    .ok_or_else(|| format!("missing fresh Cargo binary: {bin}"))?;
                binaries.insert(id);
                add_file(&mut plan, dest, *mode, Cow::Borrowed(bytes))?;
            }
            Entry::WindowsNative {
                component,
                member,
                dest,
                mode,
                targets,
            } if targets.iter().any(|t| t == WINDOWS_PAYLOAD_TARGET) => {
                let key = (*component, member.clone());
                let bytes = native.members.get(&key).ok_or_else(|| {
                    format!("missing admitted native input: {component:?}/{member}")
                })?;
                if !native_members.insert(key) {
                    return Err("duplicate native input mapping".into());
                }
                add_file(&mut plan, dest, *mode, Cow::Borrowed(bytes))?;
            }
            Entry::Copy {
                source,
                dest,
                mode,
                targets,
            } if targets.iter().any(|t| t == WINDOWS_PAYLOAD_TARGET) => {
                let bytes = source_file(checkout, source, 16 * 1024 * 1024)?;
                add_file(&mut plan, dest, *mode, Cow::Owned(bytes))?;
            }
            Entry::ModelAsset {
                source,
                dest,
                mode,
                digest_const,
                digest_source,
                archive_slot,
                targets,
            } if targets.iter().any(|t| t == WINDOWS_PAYLOAD_TARGET) => {
                if archive_slot.is_some() {
                    return Err("Windows model cannot use a Unix archive slot".into());
                }
                let code = source_file(checkout, digest_source, 4 * 1024 * 1024)?;
                let expected = digest_const_hex(
                    std::str::from_utf8(&code).map_err(|e| e.to_string())?,
                    digest_const,
                )
                .ok_or_else(|| format!("missing model digest: {digest_const}"))?;
                let bytes = source_file(checkout, source, 1024 * 1024 * 1024)?;
                if sha256_hex(&bytes) != expected {
                    return Err(format!("model digest mismatch: {dest}"));
                }
                add_file(&mut plan, dest, *mode, Cow::Owned(bytes))?;
            }
            Entry::Launcher { targets, .. }
            | Entry::OnnxRuntime { targets, .. }
            | Entry::Pdfium { targets, .. }
                if targets.iter().any(|t| t == WINDOWS_PAYLOAD_TARGET) =>
            {
                return Err("Windows inventory selected a Unix staging entry".into());
            }
            _ => {}
        }
    }
    if binaries.len() != products.len()
        || native_members.len() != native.members.len()
        || (!build_evidence.is_empty() && !evidence_selected)
    {
        return Err("undeclared fresh Cargo or admitted native input".into());
    }
    for relative in load_payload(inventory_path, inventory).map_err(|e| e.to_string())? {
        let source = format!("{}/{relative}", inventory.payload_src_root);
        let bytes = source_file(checkout, &source, 16 * 1024 * 1024)?;
        let dest = super::payload_dest(&inventory.payload_dest_prefix, &relative);
        add_file(&mut plan, &dest, 0o644, Cow::Owned(bytes))?;
    }
    Ok(plan)
}

fn source_file(checkout: &Path, relative: &str, limit: u64) -> Result<Vec<u8>, String> {
    safe_relative(relative)?;
    read_bounded(&join_components(checkout, relative), limit)
}

fn safe_relative(path: &str) -> Result<(), String> {
    if path.is_empty()
        || !path.is_ascii()
        || path.contains(['\\', ':'])
        || path.split('/').any(|part| {
            part.is_empty()
                || matches!(part, "." | "..")
                || part.ends_with([' ', '.'])
                || part.contains(['<', '>', '"', '|', '?', '*'])
        })
    {
        return Err(format!("invalid Windows staging path: {path:?}"));
    }
    for part in path.split('/') {
        let stem = part.split('.').next().unwrap_or(part).to_ascii_uppercase();
        if matches!(
            stem.as_str(),
            "CON" | "PRN" | "AUX" | "NUL" | "CONIN$" | "CONOUT$"
        ) || ((stem.starts_with("COM") || stem.starts_with("LPT"))
            && stem.len() == 4
            && matches!(stem.as_bytes()[3], b'1'..=b'9'))
            || part.bytes().any(|byte| byte < 32)
        {
            return Err(format!("reserved Windows staging path: {path:?}"));
        }
    }
    Ok(())
}

fn join_components(root: &Path, relative: &str) -> PathBuf {
    // Explicit components also work when the native root uses a verbatim prefix.
    relative
        .split('/')
        .fold(root.to_owned(), |path, part| path.join(part))
}

fn add_file<'a>(
    plan: &mut Plan<'a>,
    dest: &str,
    mode: u32,
    bytes: Cow<'a, [u8]>,
) -> Result<(), String> {
    safe_relative(dest)?;
    crate::layout::windows_dest_role(dest).map_err(|e| e.to_string())?;
    if bytes.is_empty()
        || !matches!(mode, 0o644 | 0o755)
        || [WINDOWS_PAYLOAD_MANIFEST, WINDOWS_PAYLOAD_SIGNATURE]
            .iter()
            .any(|p| p.eq_ignore_ascii_case(dest))
    {
        return Err(format!("invalid or reserved Windows staged member: {dest}"));
    }
    let folded = dest.to_ascii_lowercase();
    if plan.keys().any(|prior| {
        let prior = prior.to_ascii_lowercase();
        prior == folded
            || prior.starts_with(&(folded.clone() + "/"))
            || folded.starts_with(&(prior + "/"))
    }) {
        return Err(format!("colliding Windows staged member: {dest}"));
    }
    let record = FileRecord::file(dest, crate::stage::recorded_mode(mode), sha256_hex(&bytes));
    plan.insert(dest.into(), PlannedFile { bytes, record });
    Ok(())
}

fn write_new(root: &Path, dest: &str, bytes: &[u8], mode: u32) -> Result<(), String> {
    let path = join_components(root, dest);
    fs::create_dir_all(path.parent().ok_or("staged member has no parent")?)
        .map_err(|e| e.to_string())?;
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&path)
        .map_err(|e| e.to_string())?;
    file.write_all(bytes).map_err(|e| e.to_string())?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        file.set_permissions(fs::Permissions::from_mode(mode))
            .map_err(|e| e.to_string())?;
    }
    #[cfg(not(unix))]
    let _ = mode;
    Ok(())
}

fn write_plan(root: &Path, plan: &Plan<'_>) -> Result<Vec<FileRecord>, String> {
    for (dest, file) in plan {
        write_new(root, dest, &file.bytes, file.record.mode)?;
    }
    let expected = plan
        .values()
        .map(|file| file.record.clone())
        .collect::<Vec<_>>();
    let actual = crate::stage::staged_records(root).map_err(|e| e.to_string())?;
    crate::record::compare_records("retained admission", &expected, "staged", &actual)?;
    Ok(expected)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_evidence_uses_inventory_and_retained_bytes_exactly_once() {
        let repo = Path::new(env!("CARGO_MANIFEST_DIR"))
            .ancestors()
            .nth(3)
            .unwrap();
        let mut inventory = load_inventory(&repo.join("core/distribution/inventory.toml")).unwrap();
        inventory
            .entry
            .retain(|entry| matches!(entry, Entry::WindowsBuildEvidence { .. }));
        assert_eq!(inventory.entry.len(), 1);
        let root = tempfile::tempdir().unwrap();
        inventory.payload = "empty.list".into();
        fs::write(root.path().join("empty.list"), b"").unwrap();
        let inventory_path = root.path().join("inventory.toml");
        let products = BTreeMap::new();
        let native = AdmittedWindowsNativeInputs::default();
        let bytes = b"retained fixture bytes\r\n";
        let plan =
            collect_plan(repo, &inventory_path, &inventory, &products, &native, bytes).unwrap();
        assert_eq!(
            plan["share/provenance/cargo-evidence.json"].bytes.as_ref(),
            bytes
        );
        assert!(collect_plan(repo, &inventory_path, &inventory, &products, &native, &[]).is_err());
        inventory.entry.push(inventory.entry[0].clone());
        assert!(
            collect_plan(repo, &inventory_path, &inventory, &products, &native, bytes).is_err()
        );
        inventory.entry.clear();
        assert!(
            collect_plan(repo, &inventory_path, &inventory, &products, &native, bytes).is_err()
        );
    }

    #[test]
    fn native_inventory_requires_exact_admitted_input_set() {
        let repo = Path::new(env!("CARGO_MANIFEST_DIR"))
            .ancestors()
            .nth(3)
            .unwrap();
        let inventory_path = repo.join("core/distribution/inventory.toml");
        let mut inventory = load_inventory(&inventory_path).unwrap();
        inventory.entry.retain(|entry| {
            matches!(entry,
            Entry::WindowsNative { component: WindowsNativeComponent::Ced, member, .. }
            if member == "bin/ced.dll")
        });
        assert_eq!(inventory.entry.len(), 1);
        let root = tempfile::tempdir().unwrap();
        inventory.payload = "empty.list".into();
        fs::write(root.path().join("empty.list"), b"").unwrap();
        let inventory_path = root.path().join("inventory.toml");
        let products = BTreeMap::new();
        let mut native = AdmittedWindowsNativeInputs::default();
        assert!(
            collect_plan(repo, &inventory_path, &inventory, &products, &native, &[])
                .err()
                .unwrap()
                .contains("missing admitted native input")
        );
        native
            .insert(
                WindowsNativeComponent::Ced,
                "bin/ced.dll",
                b"admitted-original".to_vec(),
            )
            .unwrap();
        let plan =
            collect_plan(repo, &inventory_path, &inventory, &products, &native, &[]).unwrap();
        assert_eq!(plan["bin/ced.dll"].bytes.as_ref(), b"admitted-original");
        drop(plan);
        native
            .insert(WindowsNativeComponent::Ced, "unexpected", b"extra".to_vec())
            .unwrap();
        assert!(
            collect_plan(repo, &inventory_path, &inventory, &products, &native, &[])
                .err()
                .unwrap()
                .contains("undeclared")
        );
    }

    #[test]
    fn windows_staging_refuses_unexpected_files_in_its_fresh_directory() {
        let root = tempfile::tempdir().unwrap();
        fs::write(root.path().join("unexpected"), b"not in inventory").unwrap();
        let mut plan = Plan::new();
        add_file(
            &mut plan,
            "share/declared",
            0o644,
            Cow::Borrowed(b"admitted"),
        )
        .unwrap();
        assert!(
            write_plan(root.path(), &plan)
                .unwrap_err()
                .contains("unexpected in staged")
        );
    }

    #[test]
    fn manifest_render_cannot_restamp_substituted_staging_bytes() {
        let root = tempfile::tempdir().unwrap();
        let mut plan = Plan::new();
        add_file(
            &mut plan,
            "share/original",
            0o644,
            Cow::Borrowed(b"original"),
        )
        .unwrap();
        write_plan(root.path(), &plan).unwrap();
        render_checked_manifest(root.path(), &plan, &"a".repeat(40), &"b".repeat(64)).unwrap();
        fs::write(root.path().join("share/original"), b"replaced").unwrap();
        assert!(
            render_checked_manifest(root.path(), &plan, &"a".repeat(40), &"b".repeat(64))
                .unwrap_err()
                .contains("retained staging admission")
        );
    }

    #[test]
    fn pe_census_cannot_skip_uppercase_or_mixed_case_images() {
        for path in ["bin/WORKER.EXE", "bin/runtime.DlL"] {
            let mut plan = Plan::new();
            add_file(
                &mut plan,
                "bin/valid.dll",
                0o644,
                Cow::Owned(crate::pe_dependencies::tests::image()),
            )
            .unwrap();
            inspect_plan_runtime_closure(&plan).unwrap();
            add_file(&mut plan, path, 0o644, Cow::Borrowed(b"not a PE image")).unwrap();
            let error = inspect_plan_runtime_closure(&plan).unwrap_err();
            assert!(error.contains(path), "{error}");
        }
    }

    #[test]
    fn declared_members_refuse_case_prefix_and_reserved_collisions() {
        let mut plan = Plan::new();
        add_file(
            &mut plan,
            "bin/member.exe",
            0o755,
            Cow::Borrowed(b"original"),
        )
        .unwrap();
        for dest in [
            "bin/MEMBER.exe",
            "bin/member.exe/child",
            "bin",
            "bin/NUL.exe",
            "bin/a:stream",
            WINDOWS_PAYLOAD_MANIFEST,
            "share/provenance/WINDOWS-PAYLOAD.JSON.MINISIG",
        ] {
            assert!(
                add_file(&mut plan, dest, 0o644, Cow::Borrowed(b"other")).is_err(),
                "{dest}"
            );
        }
        assert_eq!(plan.len(), 1);
    }

    #[test]
    fn staging_writes_original_bytes_and_refuses_a_late_member() {
        let root = tempfile::tempdir().unwrap();
        let mut plan = Plan::new();
        add_file(
            &mut plan,
            "share/original",
            0o644,
            Cow::Borrowed(b"original"),
        )
        .unwrap();
        write_plan(root.path(), &plan).unwrap();
        assert_eq!(
            fs::read(root.path().join("share/original")).unwrap(),
            b"original"
        );
        assert!(write_plan(root.path(), &plan).is_err());
        assert_eq!(
            fs::read(root.path().join("share/original")).unwrap(),
            b"original"
        );
    }
}
