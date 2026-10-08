// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use crate::archive_seal::SealedArchiveSet;
use crate::digest::sha256_hex;
use crate::inventory::{Entry, Inventory, digest_const_hex, format_named_list};
use crate::onnx_runtime;
use crate::pdfium;
use crate::produce::payload_dest;
use crate::select::ArtifactId;
use crate::stage;

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct FileRecord {
    pub dest: String,
    pub kind: String,
    pub mode: u32,
    pub digest: String,
}

impl FileRecord {
    #[must_use]
    pub fn file(dest: impl Into<String>, mode: u32, digest: impl Into<String>) -> Self {
        Self {
            dest: dest.into(),
            kind: "file".to_owned(),
            mode,
            digest: digest.into(),
        }
    }

    #[must_use]
    pub fn key(&self) -> String {
        format!(
            "{} {} {:04o} {}",
            self.kind, self.dest, self.mode, self.digest
        )
    }
}

pub fn compare_records(
    _left_label: &str,
    left: &[FileRecord],
    right_label: &str,
    right: &[FileRecord],
) -> Result<(), String> {
    let left_keys = left.iter().map(FileRecord::key).collect::<BTreeSet<_>>();
    let right_keys = right.iter().map(FileRecord::key).collect::<BTreeSet<_>>();
    let missing = left_keys
        .difference(&right_keys)
        .cloned()
        .collect::<BTreeSet<_>>();
    let unexpected = right_keys
        .difference(&left_keys)
        .cloned()
        .collect::<BTreeSet<_>>();
    if missing.is_empty() && unexpected.is_empty() {
        return Ok(());
    }
    let mut sections = Vec::new();
    if !missing.is_empty() {
        sections.push(format_named_list(
            &format!("missing in {right_label}"),
            &missing,
        ));
    }
    if !unexpected.is_empty() {
        sections.push(format_named_list(
            &format!("unexpected in {right_label}"),
            &unexpected,
        ));
    }
    Err(sections.join("\n"))
}

#[allow(clippy::too_many_arguments)]
pub fn declared_records(
    inventory: &Inventory,
    target_id: &str,
    repo: &Path,
    payload: &[String],
    artifacts: &BTreeMap<ArtifactId, PathBuf>,
    onnx: Option<(&onnx_runtime::TargetSpec, &onnx_runtime::StagedRuntime)>,
    pdfium: Option<(&pdfium::TargetSpec, &pdfium::StagedRuntime)>,
    sealed_archives: Option<&SealedArchiveSet>,
) -> Result<Vec<FileRecord>, String> {
    let target = inventory
        .target
        .iter()
        .find(|target| target.id == target_id)
        .ok_or_else(|| format!("missing required:\n  target {target_id}"))?;
    let mut records = Vec::new();
    for entry in &inventory.entry {
        match entry {
            Entry::WindowsNative { targets, .. } | Entry::WindowsBuildEvidence { targets, .. } => {
                if targets.iter().any(|item| item == target_id) {
                    return Err("Windows native members require retained admission records".into());
                }
            }
            Entry::Bin {
                package,
                bin,
                dest,
                mode,
                lane,
                targets,
            } => {
                if !targets.iter().any(|item| item == target_id) {
                    continue;
                }
                let lane = target.lane_for(lane);
                let triple = target.triple_for_lane(lane);
                let id = ArtifactId {
                    package: package.clone(),
                    bin: bin.clone(),
                    triple: triple.to_owned(),
                };
                let path = artifacts.get(&id).ok_or_else(|| {
                    format!("missing required:\n  artifact {package} {bin} {triple}")
                })?;
                let bytes = std::fs::read(path).map_err(|error| error.to_string())?;
                records.push(FileRecord::file(
                    dest,
                    stage::recorded_mode(*mode),
                    sha256_hex(&bytes),
                ));
            }
            Entry::Launcher {
                source,
                dest,
                mode,
                targets,
            }
            | Entry::Copy {
                source,
                dest,
                mode,
                targets,
            } => {
                if !targets.iter().any(|item| item == target_id) {
                    continue;
                }
                let bytes = std::fs::read(repo.join(source)).map_err(|error| error.to_string())?;
                records.push(FileRecord::file(
                    dest,
                    stage::recorded_mode(*mode),
                    sha256_hex(&bytes),
                ));
            }
            Entry::ModelAsset {
                source,
                dest,
                mode,
                digest_const,
                digest_source,
                targets,
                archive_slot,
            } => {
                if !targets.iter().any(|item| item == target_id) {
                    continue;
                }
                let bytes = if let Some(slot) = archive_slot
                    && !slot.inspect_only
                {
                    let sealed = sealed_archives
                        .and_then(|archives| archives.by_slot_id(&slot.id))
                        .ok_or_else(|| {
                            format!("missing required:\n  sealed archive slot {}", slot.id)
                        })?;
                    if sealed.staged_dest != *dest {
                        return Err(format!(
                            "unexpected:\n  sealed archive slot {} dest {} (want {dest})",
                            slot.id, sealed.staged_dest
                        ));
                    }
                    sealed.bytes.clone()
                } else {
                    let bytes =
                        std::fs::read(repo.join(source)).map_err(|error| error.to_string())?;
                    let expected = digest_const_hex(
                        &std::fs::read_to_string(repo.join(digest_source))
                            .map_err(|error| error.to_string())?,
                        digest_const,
                    )
                    .ok_or_else(|| format!("missing required:\n  digest {digest_const}"))?;
                    let actual = sha256_hex(&bytes);
                    if actual != expected {
                        return Err(format!("unexpected:\n  {dest} digest {actual}"));
                    }
                    bytes
                };
                records.push(FileRecord::file(
                    dest,
                    stage::recorded_mode(*mode),
                    sha256_hex(&bytes),
                ));
            }
            Entry::PinnedNative {
                component: _,
                input,
                dest,
                mode,
                identity: _,
                targets,
            } => {
                if !targets.iter().any(|item| item == target_id) {
                    continue;
                }
                let (bytes, pin, filename) =
                    crate::pinned_stage::resolve_pinned_input(dest, repo, target_id, input)
                        .map_err(|e| e.to_string())?;
                let staged_member = crate::inventory::StagedMember {
                    relpath: String::new(),
                    dest: dest.clone(),
                    mode: *mode,
                    extracted_sha256: pin.sha256_hex.clone(),
                    identity: None,
                };
                let plans = crate::pinned_stage::plan_pinned_input(
                    dest,
                    &bytes,
                    &pin,
                    &filename,
                    &[staged_member],
                    &[],
                )
                .map_err(|e| e.to_string())?;
                let contents =
                    crate::pinned_stage::extract_and_verify_plans(dest, &bytes, &filename, &plans)
                        .map_err(|e| e.to_string())?;
                for plan in plans {
                    let file_bytes = &contents[&plan.inner_path];
                    records.push(FileRecord::file(
                        &plan.dest,
                        stage::recorded_mode(plan.mode),
                        sha256_hex(file_bytes),
                    ));
                }
            }
            Entry::PinnedMembers {
                component: _,
                input,
                staged,
                ignored,
                targets,
            } => {
                if !targets.iter().any(|item| item == target_id) {
                    continue;
                }
                let entry_name = staged
                    .first()
                    .map(|m| m.dest.as_str())
                    .unwrap_or("pinned-members");
                let (bytes, pin, filename) =
                    crate::pinned_stage::resolve_pinned_input(entry_name, repo, target_id, input)
                        .map_err(|e| e.to_string())?;
                let plans = crate::pinned_stage::plan_pinned_input(
                    entry_name, &bytes, &pin, &filename, staged, ignored,
                )
                .map_err(|e| e.to_string())?;
                let contents = crate::pinned_stage::extract_and_verify_plans(
                    entry_name, &bytes, &filename, &plans,
                )
                .map_err(|e| e.to_string())?;
                for plan in plans {
                    let file_bytes = &contents[&plan.inner_path];
                    records.push(FileRecord::file(
                        &plan.dest,
                        stage::recorded_mode(plan.mode),
                        sha256_hex(file_bytes),
                    ));
                }
            }
            Entry::LicenceTree {
                source,
                component,
                targets,
            } => {
                if !targets.iter().any(|item| item == target_id) {
                    continue;
                }
                let comp = component
                    .as_deref()
                    .ok_or_else(|| format!("licence-tree {source}: missing component"))?;
                let source_dir = repo.join(source);
                let rel_files =
                    crate::inventory::collect_licence_relative_paths(&source_dir, source)
                        .map_err(|e| e.to_string())?;
                let target_obj = inventory.target.iter().find(|t| t.id == target_id);
                let is_windows = target_obj
                    .map(|t| t.os.as_str() == "windows")
                    .unwrap_or(false);
                let prefix = if is_windows {
                    format!("share/licenses/{comp}/")
                } else {
                    format!("share/solstone-journal/licenses/{comp}/")
                };
                for rel in rel_files {
                    let dest = format!("{prefix}{rel}");
                    let bytes =
                        std::fs::read(source_dir.join(&rel)).map_err(|error| error.to_string())?;
                    records.push(FileRecord::file(
                        &dest,
                        stage::recorded_mode(0o644),
                        sha256_hex(&bytes),
                    ));
                }
            }
            Entry::OnnxRuntime {
                dest_dir, targets, ..
            } => {
                if !targets.iter().any(|item| item == target_id) {
                    continue;
                }
                let (spec, staged) =
                    onnx.ok_or_else(|| format!("missing required:\n  onnx runtime {target_id}"))?;
                for name in onnx_runtime::staged_member_names(spec) {
                    let dest = format!("{dest_dir}/{name}");
                    let (bytes, mode) =
                        if spec.runtime_staged_name == name || spec.link_names.contains(&name) {
                            (staged.library.as_slice(), onnx_runtime::LIB_MODE)
                        } else {
                            let bytes = staged.notices.get(name).ok_or_else(|| {
                                format!("missing required:\n  onnx notice {name}")
                            })?;
                            (bytes.as_slice(), onnx_runtime::NOTICE_MODE)
                        };
                    records.push(FileRecord::file(
                        dest,
                        stage::recorded_mode(mode),
                        sha256_hex(bytes),
                    ));
                }
            }
            Entry::Pdfium {
                dest_dir, targets, ..
            } => {
                if !targets.iter().any(|item| item == target_id) {
                    continue;
                }
                let (spec, staged) = pdfium
                    .ok_or_else(|| format!("missing required:\n  pdfium runtime {target_id}"))?;
                for name in pdfium::staged_member_names(spec) {
                    let dest = format!("{dest_dir}/{name}");
                    let (bytes, mode) = if name == spec.library_name {
                        (staged.library.as_slice(), pdfium::LIB_MODE)
                    } else {
                        let bytes = staged
                            .notices
                            .get(&name)
                            .ok_or_else(|| format!("missing required:\n  pdfium notice {name}"))?;
                        (bytes.as_slice(), pdfium::NOTICE_MODE)
                    };
                    records.push(FileRecord::file(
                        dest,
                        stage::recorded_mode(mode),
                        sha256_hex(bytes),
                    ));
                }
            }
        }
    }
    for source in payload {
        let dest = payload_dest(&inventory.payload_dest_prefix, source);
        let bytes = std::fs::read(repo.join(&inventory.payload_src_root).join(source))
            .map_err(|error| error.to_string())?;
        records.push(FileRecord::file(
            dest,
            stage::recorded_mode(0o644),
            sha256_hex(&bytes),
        ));
    }
    Ok(records)
}
