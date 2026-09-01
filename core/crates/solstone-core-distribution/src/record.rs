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
                let bytes = if let Some(slot) = archive_slot {
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
    if target.is_windows() {
        if !payload.is_empty() {
            return Err("windows payload is not implemented in this lode".to_owned());
        }
    } else {
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
    }
    Ok(records)
}
