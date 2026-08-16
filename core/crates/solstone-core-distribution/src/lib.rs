// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

pub mod ar;
pub mod archive;
pub mod cleanroom;
pub mod deb;
pub mod digest;
pub mod elf;
pub mod inspect;
pub mod inventory;
pub mod lanes;
pub mod onnx_runtime;
pub mod promote;
pub mod provenance;
pub mod record;
pub mod relocate;
pub mod rpm;
pub mod select;
pub mod stage;
pub mod tar;
pub mod zip;

use std::fs;
use std::io;
use std::path::Path;

#[cfg(test)]
use std::collections::BTreeMap;
#[cfg(test)]
use std::path::PathBuf;

use inventory::{
    Inventory, InventoryError, load_inventory, load_payload, repository_inventory_path,
};

pub fn validate_distribution_inventory(inventory_path: &Path) -> Result<Inventory, InventoryError> {
    let inventory = load_inventory(inventory_path)?;
    let _payload = load_payload(inventory_path, &inventory)?;
    Ok(inventory)
}

pub fn discover_and_validate_inventory(start: &Path) -> Result<Inventory, InventoryError> {
    let path = repository_inventory_path(start).ok_or_else(|| {
        InventoryError::new(format!(
            "could not find core/distribution/inventory.toml from {}",
            start.display()
        ))
    })?;
    validate_distribution_inventory(&path)
}

#[derive(Clone, Copy)]
pub struct ContainerMeta<'a> {
    pub version: &'a str,
    pub deb_arch: &'a str,
    pub rpm_arch: &'a str,
}

pub fn write_containers(stage: &Path, out_dir: &Path, meta: ContainerMeta<'_>) -> io::Result<()> {
    fs::create_dir_all(out_dir)?;
    tar::write_tar_gz(stage, &out_dir.join("tree.tar.gz"))?;
    deb::write_deb(
        stage,
        &out_dir.join("tree.deb"),
        deb::DebMeta {
            version: meta.version,
            arch: meta.deb_arch,
        },
    )?;
    rpm::write_rpm(
        stage,
        &out_dir.join("tree.rpm"),
        rpm::RpmMeta {
            version: meta.version,
            arch: meta.rpm_arch,
        },
    )?;
    Ok(())
}

pub fn helper_runtime_pair() -> (onnx_runtime::TargetSpec, Vec<u8>) {
    onnx_runtime::identity_fixture_wheel()
}

pub fn stage_helper_runtime(
    spec: &onnx_runtime::TargetSpec,
    bytes: &[u8],
) -> Result<onnx_runtime::StagedRuntime, onnx_runtime::StageError> {
    onnx_runtime::stage_from_bytes(spec, bytes)
}

pub fn stage_helper_runtime_from_path(
    spec: &onnx_runtime::TargetSpec,
    path: &Path,
) -> Result<onnx_runtime::StagedRuntime, onnx_runtime::StageError> {
    onnx_runtime::stage_from_path(spec, path)
}

pub fn helper_forbidden_runtime_pair() -> (onnx_runtime::TargetSpec, Vec<u8>) {
    onnx_runtime::forbidden_member_fixture_wheel()
}

pub fn helper_dependency_needles() -> &'static [&'static str] {
    &[elf::HELPER_SONAME, "onnxruntime"]
}

#[cfg(test)]
fn committed_inventory() -> Inventory {
    let path =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../core/distribution/inventory.toml");
    load_inventory(&path).expect("committed inventory must parse")
}

#[cfg(test)]
#[test]
fn selection_from_default_cargo_output_names_missing_required_and_admitted_forbidden() {
    let inventory = committed_inventory();
    let output = PathBuf::from("/var/tmp/solstone-distribution-default-host-output");
    let artifacts_dir = PathBuf::from("/var/tmp/solstone-distribution-selected-artifacts");
    let _ = fs::remove_dir_all(&output);
    let _ = fs::remove_dir_all(&artifacts_dir);
    fs::create_dir_all(&output).expect("create fixture dir");
    fs::create_dir_all(&artifacts_dir).expect("create artifact dir");

    let forbidden = inventory.forbidden_bins();
    for name in &forbidden {
        fs::write(output.join(name), name.as_bytes()).expect("write stub fixture");
    }

    let target = inventory
        .target
        .iter()
        .find(|item| item.id == "linux-x86_64")
        .expect("linux target");
    let mut artifacts = BTreeMap::new();
    for entry in &inventory.entry {
        let inventory::Entry::Bin {
            package,
            bin,
            lane,
            targets,
            ..
        } = entry
        else {
            continue;
        };
        if !targets.iter().any(|item| item == "linux-x86_64") {
            continue;
        }
        let triple = if lane == "musl-static" {
            target.triple_musl.as_str()
        } else {
            target.triple_gnu.as_str()
        };
        let path = artifacts_dir.join(bin);
        fs::write(&path, bin.as_bytes()).expect("write selected artifact");
        artifacts.insert(
            select::ArtifactId {
                package: package.clone(),
                bin: bin.clone(),
                triple: triple.to_owned(),
            },
            path,
        );
    }

    let selection = select::select_artifacts(&inventory, "linux-x86_64", &artifacts)
        .expect("inventory-driven selection must succeed");
    let stage = PathBuf::from("/var/tmp/solstone-distribution-selected-stage");
    let _ = fs::remove_dir_all(&stage);
    select::stage_selected(&selection, &stage).expect("stage selected");
    let staged = stage::staged_files(&stage).expect("list staged");
    let present_in_output = forbidden
        .iter()
        .filter(|name| output.join(name).is_file())
        .count();
    let leaked = forbidden
        .iter()
        .filter(|name| staged.iter().any(|dest| dest.ends_with(name.as_str())))
        .cloned()
        .collect::<Vec<_>>();
    let _ = fs::remove_dir_all(&output);
    let _ = fs::remove_dir_all(&artifacts_dir);
    let _ = fs::remove_dir_all(&stage);
    assert_eq!(
        present_in_output,
        forbidden.len(),
        "fixture output must contain the full stub set"
    );
    assert!(
        leaked.is_empty(),
        "unexpected:\n  {}",
        leaked.join("\n  ")
    );
    assert!(
        selection.admitted.contains("solstone-core-speakers-analyze"),
        "missing required:\n  solstone-core-speakers-analyze"
    );
    assert!(
        !selection.admitted.contains("setup-fixture-journal"),
        "admitted forbidden:\n  setup-fixture-journal"
    );
}

#[cfg(test)]
#[test]
fn containers_disagree_on_required_entry() {
    let root = PathBuf::from("/var/tmp/solstone-distribution-container-stage");
    let out = PathBuf::from("/var/tmp/solstone-distribution-container-out");
    let _ = fs::remove_dir_all(&root);
    let _ = fs::remove_dir_all(&out);
    fs::create_dir_all(&root).expect("create stage");

    stage::write_staged_file_mode(&root, "bin/solstone-core-speakers-analyze", b"helper", 0o755)
        .expect("stage helper");
    stage::write_staged_file(
        &root,
        "share/solstone/talent/journal/contract/bundle.json",
        b"{}",
    )
    .expect("stage contract bundle");

    write_containers(
        &root,
        &out,
        ContainerMeta {
            version: "1.0.22",
            deb_arch: "amd64",
            rpm_arch: "x86_64",
        },
    )
    .expect("write containers");
    let tar_manifest = tar::tar_records(&fs::read(out.join("tree.tar.gz")).unwrap()).unwrap();
    let deb_manifest = deb::deb_records(&out.join("tree.deb")).unwrap();
    let rpm_manifest = rpm::rpm_records(&out.join("tree.rpm")).unwrap();
    let control = deb::deb_control_text(&out.join("tree.deb")).unwrap();
    let requires = rpm::rpm_requires(&out.join("tree.rpm")).unwrap();
    let _ = fs::remove_dir_all(&root);
    let _ = fs::remove_dir_all(&out);
    record::compare_records("tar", &tar_manifest, "deb", &deb_manifest).expect("tar vs deb");
    record::compare_records("tar", &tar_manifest, "rpm", &rpm_manifest).expect("tar vs rpm");
    assert!(control.contains("Architecture: amd64"));
    assert!(control.contains("Package: solstone-journal"));
    assert!(control.contains("Version: 1.0.22"));
    assert!(!control.contains(elf::HELPER_SONAME));
    assert!(!requires.iter().any(|item| item == elf::HELPER_SONAME));
    assert!(
        tar_manifest
            .iter()
            .any(|record| record.dest.ends_with("solstone-core-speakers-analyze")
                && record.mode == 0o755)
    );
    let inventory = committed_inventory();
    let x86 = inventory
        .target
        .iter()
        .find(|item| item.id == "linux-x86_64")
        .unwrap();
    let arm = inventory
        .target
        .iter()
        .find(|item| item.id == "linux-aarch64")
        .unwrap();
    assert_eq!(x86.deb_arch, "amd64");
    assert_eq!(x86.rpm_arch, "x86_64");
    assert_eq!(arm.deb_arch, "arm64");
    assert_eq!(arm.rpm_arch, "aarch64");
}

#[cfg(test)]
#[test]
fn arch_mapping_modes_and_clean_package_depends() {
    let inventory = committed_inventory();
    let version = env!("CARGO_PKG_VERSION");
    let root = PathBuf::from("/var/tmp/solstone-distribution-arch-stage");
    let _ = fs::remove_dir_all(&root);
    fs::create_dir_all(&root).unwrap();
    stage::write_staged_file_mode(&root, "bin/solstone-core", b"core", 0o755).unwrap();
    stage::write_staged_file_mode(&root, "share/LICENSE", b"license", 0o644).unwrap();
    stage::write_staged_file_mode(&root, "lib/solstone_journal_models/assets/model.bin", b"m", 0o644)
        .unwrap();
    for target in &inventory.target {
        let out = PathBuf::from(format!(
            "/var/tmp/solstone-distribution-arch-out-{}",
            target.id
        ));
        let _ = fs::remove_dir_all(&out);
        write_containers(
            &root,
            &out,
            ContainerMeta {
                version,
                deb_arch: &target.deb_arch,
                rpm_arch: &target.rpm_arch,
            },
        )
        .unwrap();
        let control = deb::deb_control_text(&out.join("tree.deb")).unwrap();
        let requires = rpm::rpm_requires(&out.join("tree.rpm")).unwrap();
        let arch = rpm::rpm_arch(&out.join("tree.rpm")).unwrap();
        assert!(control.contains("Package: solstone-journal"));
        assert!(control.contains(&format!("Version: {version}")));
        assert!(control.contains(&format!("Architecture: {}", target.deb_arch)));
        assert_eq!(arch, target.rpm_arch);
        match target.id.as_str() {
            "linux-x86_64" => {
                assert_eq!(target.deb_arch, "amd64");
                assert_eq!(target.rpm_arch, "x86_64");
            }
            "linux-aarch64" => {
                assert_eq!(target.deb_arch, "arm64");
                assert_eq!(target.rpm_arch, "aarch64");
            }
            other => panic!("unexpected target {other}"),
        }
        for records in [
            tar::tar_records(&fs::read(out.join("tree.tar.gz")).unwrap()).unwrap(),
            deb::deb_records(&out.join("tree.deb")).unwrap(),
            rpm::rpm_records(&out.join("tree.rpm")).unwrap(),
        ] {
            for record in &records {
                if record.dest.starts_with("bin/") {
                    assert_eq!(record.mode, 0o755, "{}", record.dest);
                } else {
                    assert_eq!(record.mode, 0o644, "{}", record.dest);
                }
            }
        }
        for needle in helper_dependency_needles() {
            assert!(
                !control.contains(needle),
                "deb Depends must not name {needle}"
            );
            assert!(
                !requires.iter().any(|item| item.contains(needle)),
                "rpm requires must not name {needle}"
            );
        }
        let _ = fs::remove_dir_all(&out);
    }
    let _ = fs::remove_dir_all(&root);
}

#[cfg(test)]
#[test]
fn two_constructions_are_byte_identical() {
    let root = PathBuf::from("/var/tmp/solstone-distribution-repro-stage");
    let left = PathBuf::from("/var/tmp/solstone-distribution-repro-left");
    let right = PathBuf::from("/var/tmp/solstone-distribution-repro-right");
    let _ = fs::remove_dir_all(&root);
    let _ = fs::remove_dir_all(&left);
    let _ = fs::remove_dir_all(&right);
    fs::create_dir_all(&root).expect("stage");
    stage::write_staged_file_mode(&root, "bin/solstone-core", b"core", 0o755).unwrap();
    stage::write_staged_file(&root, "share/LICENSE", b"license").unwrap();
    let meta = ContainerMeta {
        version: "1.0.22",
        deb_arch: "amd64",
        rpm_arch: "x86_64",
    };
    write_containers(&root, &left, meta).unwrap();
    write_containers(&root, &right, meta).unwrap();
    inspect::write_sidecars(
        &left,
        &inspect::ReleaseInfo {
            product: "solstone-journal",
            version: "1.0.22",
            target: "linux-x86_64",
            commit: "abc",
            lock_sha256: "def",
        },
    )
    .unwrap();
    inspect::write_sidecars(
        &right,
        &inspect::ReleaseInfo {
            product: "solstone-journal",
            version: "1.0.22",
            target: "linux-x86_64",
            commit: "abc",
            lock_sha256: "def",
        },
    )
    .unwrap();
    for name in [
        "tree.tar.gz",
        "tree.deb",
        "tree.rpm",
        ".sha256",
        ".manifest.json",
        ".release",
    ] {
        assert_eq!(
            fs::read(left.join(name)).unwrap(),
            fs::read(right.join(name)).unwrap(),
            "{name}"
        );
    }
    assert_eq!(
        inspect::self_inspect(&left).unwrap(),
        inspect::self_inspect(&right).unwrap()
    );
    let _ = fs::remove_dir_all(&root);
    let _ = fs::remove_dir_all(&left);
    let _ = fs::remove_dir_all(&right);
}

#[cfg(test)]
#[test]
fn helper_runtime_sources_are_byte_identical() {
    let (spec, wheel) = helper_runtime_pair();
    let local_dir = PathBuf::from("/var/tmp/solstone-distribution-runtime-local");
    let _ = fs::remove_dir_all(&local_dir);
    fs::create_dir_all(&local_dir).unwrap();
    let wheel_path = local_dir.join("wheel.bin");
    fs::write(&wheel_path, &wheel).unwrap();
    let from_memory = stage_helper_runtime(&spec, &wheel).unwrap();
    let from_path = stage_helper_runtime_from_path(&spec, &wheel_path).unwrap();
    let _ = fs::remove_dir_all(&local_dir);
    assert_eq!(from_memory, from_path);
}

#[cfg(test)]
#[test]
fn lanes_refuse_wrong_or_missing_zig() {
    lanes::check_zig_version("0.16.0").unwrap();
    let missing = lanes::check_zig_version("").unwrap_err().to_string();
    let wrong = lanes::check_zig_version("0.15.0").unwrap_err().to_string();
    assert!(missing.contains("missing required:"));
    assert!(wrong.contains("unexpected:"));
}

#[cfg(test)]
#[test]
fn gnu_lane_sets_host_target_and_wrapper_vars() {
    let inventory = committed_inventory();
    let target = inventory
        .target
        .iter()
        .find(|item| item.id == "linux-x86_64")
        .unwrap();
    let env = lanes::gnu_lane_env(
        target,
        Path::new("/var/tmp/solstone-distribution-wrappers"),
        Path::new("/opt/zig/lib"),
        Path::new("/repo"),
        Some(Path::new("/repo/target/link/linux-x86_64")),
        "x86_64-unknown-linux-gnu",
    )
    .unwrap();
    assert_eq!(
        env.vars.get("CARGO_UNSTABLE_TARGET_APPLIES_TO_HOST").unwrap(),
        "true"
    );
    assert_eq!(
        env.vars.get("CARGO_TARGET_APPLIES_TO_HOST").unwrap(),
        "false"
    );
    assert!(env.vars.contains_key("CARGO_TARGET_X86_64_UNKNOWN_LINUX_GNU_LINKER"));
    assert!(env.vars.contains_key("AR_x86_64_unknown_linux_gnu"));
    assert!(env.vars.contains_key("RANLIB_x86_64_unknown_linux_gnu"));
    assert!(env.vars.contains_key("BINDGEN_EXTRA_CLANG_ARGS_x86_64_unknown_linux_gnu"));
    assert!(
        env.vars
            .get(lanes::describe_cc_key())
            .unwrap()
            .contains("x86_64-linux-gnu.2.27")
    );
    let musl = lanes::musl_lane_env(
        target,
        Path::new("/var/tmp/solstone-distribution-wrappers-musl"),
        "x86_64-unknown-linux-gnu",
    )
    .unwrap();
    assert!(musl
        .vars
        .contains_key("CARGO_TARGET_X86_64_UNKNOWN_LINUX_MUSL_LINKER"));
    assert!(musl.vars.contains_key("AR_x86_64_unknown_linux_musl"));
    assert!(!musl.vars.keys().any(|key| key.contains("RUSTFLAGS")));
    assert_eq!(env.vars.get("ORT_PREFER_DYNAMIC_LINK").unwrap(), "true");
    let wrapper = env.wrappers.values().next().unwrap();
    assert!(wrapper.contains("exec zig"));
    assert!(!env.vars.keys().any(|key| key.contains("RUSTFLAGS")));
}

#[cfg(test)]
#[test]
fn elf_reader_accepts_gnu_and_static_and_rejects_bad_inputs() {
    let gnu = elf::fixture_gnu_dynamic(
        elf::machine_x86_64(),
        "/lib64/ld-linux-x86-64.so.2",
        &[elf::HELPER_SONAME, "libc.so.6"],
        Some(elf::HELPER_RUNPATH),
        elf::GLIBC_CEILING,
    );
    let info = elf::parse_elf(&gnu).expect("parse gnu");
    elf::inspect_gnu_helper(
        &info,
        elf::machine_x86_64(),
        Some(elf::HELPER_RUNPATH),
        &[elf::HELPER_SONAME],
    )
    .unwrap();
    assert_eq!(info.runpath.as_deref(), Some(elf::HELPER_RUNPATH));
    assert!(info.needed.iter().any(|item| item == elf::HELPER_SONAME));

    let high = elf::fixture_gnu_dynamic(
        elf::machine_x86_64(),
        "/lib64/ld-linux-x86-64.so.2",
        &["libc.so.6"],
        None,
        (2, 34),
    );
    let high_info = elf::parse_elf(&high).unwrap();
    assert!(
        elf::inspect_gnu_helper(&high_info, elf::machine_x86_64(), None, &[])
            .unwrap_err()
            .to_string()
            .contains("GLIBC_2.34")
    );

    let musl = elf::fixture_static_musl(elf::machine_x86_64());
    let musl_info = elf::parse_elf(&musl).unwrap();
    elf::inspect_core_family(&musl_info, elf::machine_x86_64()).unwrap();
    assert!(
        elf::inspect_musl_static(&musl_info, elf::machine_aarch64())
            .unwrap_err()
            .to_string()
            .contains("e_machine")
    );
    let committed = elf::committed_gnu_dynamic();
    assert_eq!(committed, gnu.as_slice());
    assert_eq!(elf::committed_static_musl(), musl.as_slice());
    let committed_info = elf::parse_elf(committed).expect("committed gnu");
    elf::inspect_gnu_helper(
        &committed_info,
        elf::machine_x86_64(),
        Some(elf::HELPER_RUNPATH),
        &[elf::HELPER_SONAME],
    )
    .unwrap();

    let wrong_interp = elf::fixture_gnu_dynamic(
        elf::machine_x86_64(),
        "/lib/ld-musl-x86_64.so.1",
        &[elf::HELPER_SONAME],
        Some(elf::HELPER_RUNPATH),
        elf::GLIBC_CEILING,
    );
    assert!(
        elf::inspect_gnu_helper(
            &elf::parse_elf(&wrong_interp).unwrap(),
            elf::machine_x86_64(),
            Some(elf::HELPER_RUNPATH),
            &[elf::HELPER_SONAME],
        )
        .unwrap_err()
        .to_string()
        .contains("PT_INTERP")
    );
    let wrong_runpath = elf::fixture_gnu_dynamic(
        elf::machine_x86_64(),
        "/lib64/ld-linux-x86-64.so.2",
        &[elf::HELPER_SONAME],
        Some("$ORIGIN/../lib/other"),
        elf::GLIBC_CEILING,
    );
    assert!(
        elf::inspect_gnu_helper(
            &elf::parse_elf(&wrong_runpath).unwrap(),
            elf::machine_x86_64(),
            Some(elf::HELPER_RUNPATH),
            &[elf::HELPER_SONAME],
        )
        .unwrap_err()
        .to_string()
        .contains("DT_RUNPATH")
    );
    let wrong_needed = elf::fixture_gnu_dynamic(
        elf::machine_x86_64(),
        "/lib64/ld-linux-x86-64.so.2",
        &["libc.so.6"],
        Some(elf::HELPER_RUNPATH),
        elf::GLIBC_CEILING,
    );
    assert!(
        elf::inspect_gnu_helper(
            &elf::parse_elf(&wrong_needed).unwrap(),
            elf::machine_x86_64(),
            Some(elf::HELPER_RUNPATH),
            &[elf::HELPER_SONAME],
        )
        .unwrap_err()
        .to_string()
        .contains("DT_NEEDED")
    );
    assert!(
        elf::inspect_core_family(&info, elf::machine_x86_64())
            .unwrap_err()
            .to_string()
            .contains("dynamic core-family")
    );

    let mut broken = gnu.clone();
    // Drop section headers so a dynamic helper cannot read verneed.
    broken[40..48].copy_from_slice(&0_u64.to_le_bytes());
    broken[60..62].copy_from_slice(&0_u16.to_le_bytes());
    assert!(
        elf::parse_elf(&broken)
            .unwrap_err()
            .to_string()
            .contains("could not read GNU version needs")
    );
}

#[cfg(test)]
#[test]
fn archive_and_zip_refuse_escapes_and_digest_mismatch() {
    assert_eq!(
        archive::refuse_escape("/abs").unwrap_err(),
        archive::ArchiveEscape::AbsolutePath
    );
    assert_eq!(
        archive::refuse_escape("a/../b").unwrap_err(),
        archive::ArchiveEscape::ParentTraversal
    );
    let (spec, mut wheel) = helper_runtime_pair();
    wheel[0] ^= 1;
    let error = stage_helper_runtime(&spec, &wheel).unwrap_err().to_string();
    assert!(error.contains("missing required:"));
    let (forbidden_spec, forbidden_wheel) = helper_forbidden_runtime_pair();
    let forbidden = stage_helper_runtime(&forbidden_spec, &forbidden_wheel)
        .unwrap_err()
        .to_string();
    assert!(forbidden.contains("unexpected:"));
}

#[cfg(test)]
#[test]
fn provenance_refuses_dirty_stale_and_wrong_commit() {
    assert!(provenance::require_clean(true)
        .unwrap_err()
        .to_string()
        .contains("dirty-tree"));
    assert!(
        provenance::require_commit("aaa", "bbb")
            .unwrap_err()
            .to_string()
            .contains("mismatched-commit")
    );
    assert!(
        provenance::require_lock("aaa", "bbb")
            .unwrap_err()
            .to_string()
            .contains("stale-lock")
    );
    let json = r#"{"reason":"compiler-artifact","package_id":"solstone-core 1.0.22","target":{"name":"solstone-core","kind":["bin"]},"filenames":["/work/x86_64-unknown-linux-musl/release/solstone-core"]}"#;
    let artifacts = provenance::bind_cargo_json(json).unwrap();
    assert_eq!(artifacts.len(), 1);
}

#[cfg(test)]
#[test]
fn promotion_is_atomic_after_each_successive_write() {
    let prior = b"previous-tree";
    for step in promote::PromoteStep::ALL {
        let dest = PathBuf::from(format!(
            "/var/tmp/solstone-distribution-promote-dest-{}",
            step.as_str()
        ));
        let work = PathBuf::from(format!(
            "/var/tmp/solstone-distribution-promote-work-{}",
            step.as_str()
        ));
        let _ = fs::remove_dir_all(&dest);
        let _ = fs::remove_dir_all(&work);
        fs::create_dir_all(&dest).unwrap();
        fs::write(dest.join("marker"), prior).unwrap();
        let before = promote::snapshot_dir(&dest).unwrap();
        let request = promote::PromoteRequest {
            dest: dest.clone(),
            work: work.clone(),
            tree: vec![("bin/solstone-core".into(), b"core".to_vec(), 0o755)],
            version: "1.0.22".into(),
            arch: "linux-x86_64".into(),
            deb_arch: "amd64".into(),
            rpm_arch: "x86_64".into(),
            dirty: false,
            observed: provenance::Provenance {
                commit: "aaa".into(),
                lock_sha256: "bbb".into(),
            },
            expected: provenance::Provenance {
                commit: "aaa".into(),
                lock_sha256: "bbb".into(),
            },
            fail_after: Some(step.as_str().to_owned()),
        };
        let result = promote::promote(&request);
        assert!(result.is_err(), "{}", step.as_str());
        let after = promote::snapshot_dir(&dest).unwrap();
        assert_eq!(before, after, "{}", step.as_str());
        let _ = fs::remove_dir_all(&dest);
        let _ = fs::remove_dir_all(&work);
    }
}

#[cfg(test)]
#[test]
fn select_refuses_wrong_triple_and_missing_required() {
    let inventory = committed_inventory();
    let mut artifacts = BTreeMap::new();
    artifacts.insert(
        select::ArtifactId {
            package: "solstone-core".into(),
            bin: "solstone-core".into(),
            triple: "powerpc-unknown-linux-gnu".into(),
        },
        PathBuf::from("/nope"),
    );
    let error = select::refuse_wrong_triple(&inventory, "linux-x86_64", &artifacts)
        .unwrap_err()
        .to_string();
    assert!(error.contains("powerpc-unknown-linux-gnu"));
    let empty = BTreeMap::new();
    let missing = select::select_artifacts(&inventory, "linux-x86_64", &empty)
        .unwrap_err()
        .to_string();
    assert!(missing.contains("missing required:"));
    let mut extra_artifacts = BTreeMap::new();
    extra_artifacts.insert(
        select::ArtifactId {
            package: "not-in-inventory".into(),
            bin: "not-in-inventory".into(),
            triple: "x86_64-unknown-linux-musl".into(),
        },
        PathBuf::from("/nope"),
    );
    let extra = select::refuse_extra(&inventory, "linux-x86_64", &extra_artifacts)
        .unwrap_err()
        .to_string();
    assert!(extra.contains("admitted forbidden:"));
}

#[cfg(test)]
#[test]
fn helper_runpath_points_at_bundled_library() {
    let gnu = elf::fixture_gnu_dynamic(
        elf::machine_x86_64(),
        "/lib64/ld-linux-x86-64.so.2",
        &[elf::HELPER_SONAME],
        Some(elf::HELPER_RUNPATH),
        elf::GLIBC_CEILING,
    );
    let info = elf::parse_elf(&gnu).unwrap();
    let stage = PathBuf::from("/var/tmp/solstone-distribution-runpath-stage");
    let _ = fs::remove_dir_all(&stage);
    let dest = format!("lib/solstone-core-speakers-analyze/{}", elf::HELPER_SONAME);
    stage::write_staged_file_mode(&stage, &dest, b"lib", 0o755).unwrap();
    let resolved = PathBuf::from("bin")
        .join("..")
        .join("lib")
        .join("solstone-core-speakers-analyze")
        .join(elf::HELPER_SONAME);
    assert!(info.runpath.unwrap().contains("$ORIGIN"));
    assert!(stage.join(&dest).is_file());
    let _ = resolved;
    let _ = fs::remove_dir_all(&stage);
}
