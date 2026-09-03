// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Read-only inspection for the managed journal launch route.

use std::env;
use std::ffi::OsStr;
use std::fs;
use std::io::{self, Write};
use std::path::Path;
use std::process::ExitCode;

use solstone_core_installation_identity::{
    GuardFields, IdentityError, InstallationBinding, OwnerBase, PlatformTag,
    load_installation_binding, lower_hex, namespace_name, root_token_from_path,
};
use solstone_core_journal::{
    resolve_identity_root_from_executable_dir, resolve_installation_root_from_executable_dir,
};
use solstone_core_setup::identity_evidence::{
    ArtifactSlotEvidence, ServiceSlotEvidence, WrapperSlotEvidence, gather_setup_artifact_evidence,
};
use solstone_core_setup::wrapper::{
    render_wrapper, wrapper_lock, wrapper_paths, write_wrappers_atomically,
};

use crate::{discover_binary_home, service};

mod coordination_lock;
mod record;

trait RouteRecord {
    fn set(&mut self, key: &'static str, value: impl Into<String>);
    fn set_path_hex(&mut self, key: &'static str, path: Option<&Path>);
}

impl RouteRecord for record::InspectRecord {
    fn set(&mut self, key: &'static str, value: impl Into<String>) {
        record::InspectRecord::set(self, key, value);
    }

    fn set_path_hex(&mut self, key: &'static str, path: Option<&Path>) {
        record::InspectRecord::set_path_hex(self, key, path);
    }
}

impl RouteRecord for record::RepairRecord {
    fn set(&mut self, key: &'static str, value: impl Into<String>) {
        record::RepairRecord::set(self, key, value);
    }

    fn set_path_hex(&mut self, key: &'static str, path: Option<&Path>) {
        record::RepairRecord::set_path_hex(self, key, path);
    }
}

/// Inspect the current installation's wrapper/service route without mutating it.
pub fn inspect() -> ExitCode {
    let mut record = record::InspectRecord::success();
    let executable_dir = match env::current_exe()
        .ok()
        .and_then(|path| path.parent().map(Path::to_path_buf))
    {
        Some(directory) => directory,
        None => return emit_refusal(record, "observation-failed"),
    };
    let Some(prefix) = resolve_identity_root_from_executable_dir(&executable_dir)
        .or_else(|| resolve_installation_root_from_executable_dir(&executable_dir))
    else {
        return emit_refusal(record, "observation-failed");
    };
    record.set_path_hex("prefix_hex", Some(&prefix));
    let current_bin = prefix.join("current/bin");
    record.set_path_hex("current_bin_hex", Some(&current_bin));
    record.set(
        "current_state",
        current_selection_state(&current_bin, &executable_dir),
    );

    let platform = match service::platform() {
        Ok(platform) => platform,
        Err(_) => {
            record.set("platform", "unsupported-not-applicable");
            record.set("current_state", "not-applicable");
            record.set("tuple_state", "not-applicable");
            return emit_refusal(record, "unsupported-platform");
        }
    };
    record.set(
        "platform",
        match platform {
            service::Platform::Linux => "linux",
            service::Platform::Darwin => "darwin",
        },
    );

    let root_token = match root_token_from_path(&prefix) {
        Ok(token) => token,
        Err(_) => return emit_refusal(record, "observation-failed"),
    };
    let namespace = namespace_name(PlatformTag::current(), &root_token);
    let home = match discover_binary_home() {
        Ok(home) => home,
        Err(_) => return emit_refusal(record, "observation-failed"),
    };
    let owner = match OwnerBase::at_home(home.clone(), PlatformTag::current()) {
        Ok(owner) => owner,
        Err(_) => return emit_refusal(record, "observation-failed"),
    };
    let binding = match load_installation_binding(&owner, &root_token) {
        Ok(binding) => {
            record_binding(&mut record, &binding);
            record.set("identity_state", "present");
            Some(binding)
        }
        Err(error) => {
            record.set("identity_state", identity_state(&error));
            None
        }
    };

    let evidence = gather_setup_artifact_evidence(&home, &namespace, true);
    let paths = wrapper_paths(&home);
    record_wrapper_observation(
        &mut record,
        "journal_wrapper",
        evidence.journal_wrapper(),
        &paths.journal,
        &binding,
        &prefix,
        &executable_dir,
    );
    record_wrapper_observation(
        &mut record,
        "solstone_wrapper",
        evidence.solstone_wrapper(),
        &paths.solstone,
        &binding,
        &prefix,
        &executable_dir,
    );

    let service_path = service::unit_path(platform, &home);
    record_service_observation(
        &mut record,
        evidence.service(),
        &service_path,
        &paths.journal,
        platform,
        &binding,
        &executable_dir,
    );
    record.set(
        "tuple_state",
        tuple_state(
            binding.is_some(),
            record.get("journal_wrapper_state").unwrap_or("malformed"),
            record.get("solstone_wrapper_state").unwrap_or("malformed"),
            record.get("service_state").unwrap_or("malformed"),
        ),
    );
    emit(record, ExitCode::SUCCESS)
}

/// Repair an already-owned route after the caller's shell acquired the route lock.
pub fn repair(lock_owner: &str) -> ExitCode {
    let mut record = record::RepairRecord::success();
    let executable_dir = match env::current_exe()
        .ok()
        .and_then(|path| path.parent().map(Path::to_path_buf))
    {
        Some(directory) => directory,
        None => return emit_repair_refusal(record, "observation-failed"),
    };
    let Some(prefix) = resolve_identity_root_from_executable_dir(&executable_dir)
        .or_else(|| resolve_installation_root_from_executable_dir(&executable_dir))
        .or_else(|| route_prefix_for_unselected_version(&executable_dir))
    else {
        return emit_repair_refusal(record, "observation-failed");
    };
    record.set_path_hex("prefix_hex", Some(&prefix));
    let current_bin = prefix.join("current/bin");
    record.set_path_hex("current_bin_hex", Some(&current_bin));
    let current_state = current_selection_state(&current_bin, &executable_dir);
    record.set("current_state", current_state);

    let platform = match service::platform() {
        Ok(platform) => platform,
        Err(_) => {
            record.set("platform", "unsupported-not-applicable");
            record.set("current_state", "not-applicable");
            record.set("tuple_state", "not-applicable");
            return emit_repair_refusal(record, "unsupported-platform");
        }
    };
    record.set(
        "platform",
        match platform {
            service::Platform::Linux => "linux",
            service::Platform::Darwin => "darwin",
        },
    );
    if current_state != "selected" {
        return emit_repair_refusal(record, "not-current");
    }

    let lock_state = coordination_lock::validate(&prefix, lock_owner);
    record.set("route_lock_state", route_lock_state(lock_state));
    if lock_state != coordination_lock::RouteLockState::Validated {
        return emit_repair_refusal(record, route_lock_refusal(lock_state));
    }
    if platform != service::Platform::Linux {
        return emit_repair_refusal(record, "unsupported-platform");
    }

    let root_token = match root_token_from_path(&prefix) {
        Ok(token) => token,
        Err(_) => return emit_repair_refusal(record, "observation-failed"),
    };
    let namespace = namespace_name(PlatformTag::current(), &root_token);
    let home = match discover_binary_home() {
        Ok(home) => home,
        Err(_) => return emit_repair_refusal(record, "observation-failed"),
    };
    let owner = match OwnerBase::at_home(home.clone(), PlatformTag::current()) {
        Ok(owner) => owner,
        Err(_) => return emit_repair_refusal(record, "observation-failed"),
    };
    let binding = match load_installation_binding(&owner, &root_token) {
        Ok(binding) => {
            record_binding(&mut record, &binding);
            record.set("identity_state", "present");
            Some(binding)
        }
        Err(error) => {
            record.set("identity_state", identity_state(&error));
            None
        }
    };
    let evidence = gather_setup_artifact_evidence(&home, &namespace, true);
    let paths = wrapper_paths(&home);
    record_wrapper_observation(
        &mut record,
        "journal_wrapper",
        evidence.journal_wrapper(),
        &paths.journal,
        &binding,
        &prefix,
        &executable_dir,
    );
    record_wrapper_observation(
        &mut record,
        "solstone_wrapper",
        evidence.solstone_wrapper(),
        &paths.solstone,
        &binding,
        &prefix,
        &executable_dir,
    );
    let service_path = service::unit_path(platform, &home);
    record_service_observation(
        &mut record,
        evidence.service(),
        &service_path,
        &paths.journal,
        platform,
        &binding,
        &executable_dir,
    );
    let journal_state = record
        .get("journal_wrapper_state")
        .expect("wrapper record state")
        .to_owned();
    let solstone_state = record
        .get("solstone_wrapper_state")
        .expect("wrapper record state")
        .to_owned();
    let service_state = record
        .get("service_state")
        .expect("service record state")
        .to_owned();
    record.set(
        "tuple_state",
        tuple_state(
            binding.is_some(),
            &journal_state,
            &solstone_state,
            &service_state,
        ),
    );

    let Some(initial_binding) = binding else {
        return emit_repair_refusal(record, "missing-identity");
    };
    if all_artifacts_missing(&journal_state, &solstone_state, &service_state) {
        record.set("tuple_state", "not-applicable");
        record.set("terminal_identity_state", "matched");
        return emit_repair(record, ExitCode::SUCCESS);
    }
    if let Some(refusal) =
        repair_eligibility_refusal(&journal_state, &solstone_state, &service_state)
    {
        return emit_repair_refusal(record, refusal);
    }

    let repair_wrappers = journal_state == "drifted" || solstone_state == "drifted";
    // Runtime drift may leave the selected route pointing at an old process,
    // so it must refresh the manager even if the static unit is aligned.
    let repair_service = service_state == "drifted"
        || service_state == "runtime-drifted"
        || (repair_wrappers && service_state != "missing");
    let initial_runtime = repair_service.then(|| {
        service::observe_runtime(platform, &service_path)
            .unwrap_or_else(service::RuntimeTruth::Unknown)
    });
    record.set(
        "repair_wrapper",
        if repair_wrappers {
            "not-run"
        } else {
            "unchanged"
        },
    );
    record.set(
        "repair_service",
        if service_state == "missing" || repair_service {
            "not-run"
        } else {
            "unchanged"
        },
    );

    let mut runtime_state_preserved = true;
    if repair_wrappers || repair_service {
        let _service_lock = match service::service_lock(&home) {
            Ok(lock) => lock,
            Err(_) => return emit_repair_refusal(record, "service-lock-unavailable"),
        };
        if repair_wrappers {
            if rewrite_drifted_wrappers(
                &home,
                &paths,
                &initial_binding,
                &executable_dir,
                &journal_state,
                &solstone_state,
            )
            .is_err()
            {
                return emit_repair_partial(record, "failed", "not-run");
            }
            record.set("repair_wrapper", "rewritten");
        }
        if repair_service
            && service::republish_linux_unit_for_route_repair(
                &home,
                &executable_dir,
                &GuardFields::from_binding(&initial_binding),
                initial_runtime
                    .as_ref()
                    .expect("service repair has an initial runtime observation"),
            )
            .is_err()
        {
            record.set("repair_service", "failed");
            record.set("outcome", "partial-failure");
            return emit_repair(record, ExitCode::from(3));
        }
        if repair_service {
            record.set("repair_service", "rewritten");
            runtime_state_preserved = matches!(
                (
                    initial_runtime
                        .as_ref()
                        .expect("service repair has an initial runtime observation"),
                    service::observe_runtime(platform, &service_path),
                ),
                (
                    service::RuntimeTruth::Managed { active: true },
                    Ok(service::RuntimeTruth::Managed { active: true }),
                ) | (
                    service::RuntimeTruth::Managed { active: false }
                        | service::RuntimeTruth::Absent,
                    Ok(service::RuntimeTruth::Managed { active: false }
                        | service::RuntimeTruth::Absent,),
                ) | (
                    service::RuntimeTruth::Foreign(_) | service::RuntimeTruth::Unknown(_),
                    Ok(service::RuntimeTruth::Managed { .. } | service::RuntimeTruth::Absent)
                )
            );
        }
    }

    match load_installation_binding(&owner, &root_token) {
        Ok(terminal) if terminal == initial_binding => {
            record.set("terminal_identity_state", "matched");
            if runtime_state_preserved {
                emit_repair(record, ExitCode::SUCCESS)
            } else {
                record.set("outcome", "partial-failure");
                emit_repair(record, ExitCode::from(3))
            }
        }
        _ => {
            record.set("terminal_identity_state", "changed");
            record.set("outcome", "partial-failure");
            emit_repair(record, ExitCode::from(3))
        }
    }
}

fn rewrite_drifted_wrappers(
    home: &Path,
    paths: &solstone_core_setup::wrapper::WrapperPaths,
    binding: &InstallationBinding,
    executable_dir: &Path,
    journal_state: &str,
    solstone_state: &str,
) -> Result<(), String> {
    let journal = crate::resolve_process_journal_path()
        .map_err(|_| "route repair could not resolve journal".to_owned())?
        .path;
    let guard = GuardFields::from_binding(binding);
    let mut contents = Vec::new();
    if journal_state == "drifted" {
        contents.push((
            paths.journal.clone(),
            render_wrapper("journal", &journal, &executable_dir.join("journal"), &guard),
        ));
    }
    if solstone_state == "drifted" {
        contents.push((
            paths.solstone.clone(),
            render_wrapper(
                "solstone",
                &journal,
                &executable_dir.join("solstone"),
                &guard,
            ),
        ));
    }
    let _lock = wrapper_lock(home).map_err(|error| error.to_string())?;
    write_wrappers_atomically(&contents).map_err(|error| error.to_string())
}

// The shared resolver intentionally refuses an orphaned version directory so
// it cannot borrow a stable identity. Repair needs only enough shape evidence
// to report `not-current` before it opens a lock or reads identity state.
fn route_prefix_for_unselected_version(executable_dir: &Path) -> Option<std::path::PathBuf> {
    (executable_dir.file_name()? == OsStr::new("bin")).then_some(())?;
    let version = executable_dir.parent()?;
    let versions = version.parent()?;
    (versions.file_name()? == OsStr::new("versions")).then_some(())?;
    let prefix = versions.parent()?;
    fs::symlink_metadata(prefix.join("current"))
        .ok()?
        .file_type()
        .is_symlink()
        .then(|| prefix.to_path_buf())
}

fn all_artifacts_missing(journal: &str, solstone: &str, service: &str) -> bool {
    [journal, solstone, service]
        .iter()
        .all(|state| *state == "missing")
}

fn repair_eligibility_refusal(
    journal: &str,
    solstone: &str,
    service: &str,
) -> Option<&'static str> {
    for state in [journal, solstone, service] {
        match state {
            "foreign" => return Some("artifact-foreign"),
            "malformed" => return Some("artifact-malformed"),
            "unguarded" => return Some("artifact-unguarded"),
            "ambiguous" => return Some("artifact-ambiguous"),
            "exact-v1" => return Some("artifact-exact-v1"),
            _ => {}
        }
    }
    if matches!(journal, "missing") || matches!(solstone, "missing") {
        return Some("tuple-not-repair-eligible");
    }
    if !matches!(journal, "aligned" | "drifted")
        || !matches!(solstone, "aligned" | "drifted")
        || !matches!(
            service,
            "aligned" | "drifted" | "runtime-drifted" | "missing"
        )
    {
        return Some("tuple-not-repair-eligible");
    }
    None
}

fn route_lock_state(state: coordination_lock::RouteLockState) -> &'static str {
    match state {
        coordination_lock::RouteLockState::Validated => "validated",
        coordination_lock::RouteLockState::Missing => "missing",
        coordination_lock::RouteLockState::Invalid => "invalid",
        coordination_lock::RouteLockState::OwnerMismatch => "owner-mismatch",
    }
}

fn route_lock_refusal(state: coordination_lock::RouteLockState) -> &'static str {
    match state {
        coordination_lock::RouteLockState::Missing => "lock-missing",
        coordination_lock::RouteLockState::Invalid => "lock-invalid",
        coordination_lock::RouteLockState::OwnerMismatch => "lock-owner-mismatch",
        coordination_lock::RouteLockState::Validated => unreachable!("validated lock proceeds"),
    }
}

fn emit_repair_refusal(mut record: record::RepairRecord, refusal: &str) -> ExitCode {
    record.set("outcome", "refused");
    record.set("refusal", refusal);
    emit_repair(record, ExitCode::from(2))
}

fn emit_repair_partial(
    mut record: record::RepairRecord,
    wrapper: &'static str,
    service: &'static str,
) -> ExitCode {
    record.set("repair_wrapper", wrapper);
    record.set("repair_service", service);
    record.set("outcome", "partial-failure");
    emit_repair(record, ExitCode::from(3))
}

fn emit_repair(record: record::RepairRecord, code: ExitCode) -> ExitCode {
    let encoded = record.encode();
    if io::stdout().lock().write_all(encoded.as_bytes()).is_err() {
        return ExitCode::from(3);
    }
    code
}

fn emit_refusal(mut record: record::InspectRecord, refusal: &str) -> ExitCode {
    record.set("outcome", "refused");
    record.set("refusal", refusal);
    emit(record, ExitCode::from(2))
}

fn emit(record: record::InspectRecord, success: ExitCode) -> ExitCode {
    let encoded = record.encode();
    if io::stdout().lock().write_all(encoded.as_bytes()).is_err() {
        return ExitCode::from(2);
    }
    success
}

fn current_selection_state(current_bin: &Path, executable_dir: &Path) -> &'static str {
    match (
        fs::canonicalize(current_bin),
        fs::canonicalize(executable_dir),
    ) {
        (Ok(current), Ok(executable)) if current == executable => "selected",
        (Ok(_), Ok(_)) => "not-selected",
        _ => "malformed",
    }
}

fn identity_state(error: &IdentityError) -> &'static str {
    match error {
        IdentityError::Io { source, .. } if source.kind() == io::ErrorKind::NotFound => "missing",
        IdentityError::NotAdopted(_) => "not-adopted",
        IdentityError::AdmissionRefused(_) => "mismatch",
        IdentityError::Io { .. }
        | IdentityError::InvalidInput(_)
        | IdentityError::UnsafeState(_)
        | IdentityError::Record(_)
        | IdentityError::Guard(_) => "malformed",
    }
}

fn record_binding<R: RouteRecord>(record: &mut R, binding: &InstallationBinding) {
    record.set("identity_namespace", binding.namespace.as_hex().to_owned());
    record.set("identity_id", binding.id.as_hex().to_owned());
    record.set("identity_generation", binding.generation.get().to_string());
    record.set(
        "identity_journal_token_hex",
        lower_hex(binding.journal_token.as_bytes()),
    );
}

fn record_guard<R: RouteRecord>(record: &mut R, prefix: &str, guard: Option<&GuardFields>) {
    let Some(guard) = guard else {
        return;
    };
    record.set(
        match prefix {
            "journal_wrapper" => "journal_wrapper_guard_namespace",
            "solstone_wrapper" => "solstone_wrapper_guard_namespace",
            "service" => "service_guard_namespace",
            _ => unreachable!("known route artifact prefix"),
        },
        guard.namespace.as_hex().to_owned(),
    );
    record.set(
        match prefix {
            "journal_wrapper" => "journal_wrapper_guard_id",
            "solstone_wrapper" => "solstone_wrapper_guard_id",
            "service" => "service_guard_id",
            _ => unreachable!("known route artifact prefix"),
        },
        guard.id.as_hex().to_owned(),
    );
    record.set(
        match prefix {
            "journal_wrapper" => "journal_wrapper_guard_generation",
            "solstone_wrapper" => "solstone_wrapper_guard_generation",
            "service" => "service_guard_generation",
            _ => unreachable!("known route artifact prefix"),
        },
        guard.generation.get().to_string(),
    );
    record.set(
        match prefix {
            "journal_wrapper" => "journal_wrapper_guard_journal_token_hex",
            "solstone_wrapper" => "solstone_wrapper_guard_journal_token_hex",
            "service" => "service_guard_journal_token_hex",
            _ => unreachable!("known route artifact prefix"),
        },
        lower_hex(guard.journal_token.as_bytes()),
    );
}

fn record_wrapper_observation<R: RouteRecord>(
    record: &mut R,
    name: &'static str,
    slot: &WrapperSlotEvidence,
    path: &Path,
    binding: &Option<InstallationBinding>,
    prefix: &Path,
    executable_dir: &Path,
) {
    record.set_path_hex(
        match name {
            "journal_wrapper" => "journal_wrapper_path_hex",
            "solstone_wrapper" => "solstone_wrapper_path_hex",
            _ => unreachable!("wrapper name"),
        },
        Some(path),
    );
    record.set_path_hex(
        match name {
            "journal_wrapper" => "journal_wrapper_target_hex",
            "solstone_wrapper" => "solstone_wrapper_target_hex",
            _ => unreachable!("wrapper name"),
        },
        slot.target(),
    );
    record_guard(record, name, slot.guard());
    record.set(
        match name {
            "journal_wrapper" => "journal_wrapper_state",
            "solstone_wrapper" => "solstone_wrapper_state",
            _ => unreachable!("wrapper name"),
        },
        wrapper_state(slot, binding.as_ref(), prefix, executable_dir),
    );
}

fn wrapper_state(
    slot: &WrapperSlotEvidence,
    binding: Option<&InstallationBinding>,
    prefix: &Path,
    executable_dir: &Path,
) -> &'static str {
    match slot.evidence() {
        ArtifactSlotEvidence::Fresh => "missing",
        ArtifactSlotEvidence::LegacyUnguarded if slot.exact_v1() => "exact-v1",
        ArtifactSlotEvidence::LegacyUnguarded => "unguarded",
        ArtifactSlotEvidence::Malformed => "malformed",
        ArtifactSlotEvidence::Ambiguous => "ambiguous",
        ArtifactSlotEvidence::Foreign => "foreign",
        ArtifactSlotEvidence::Guarded(_) => {
            let Some(binding) = binding else {
                return "missing-identity";
            };
            let guard = slot.guard().expect("guarded slot retains its guard");
            if !guard.same_identity(&GuardFields::from_binding(binding)) {
                return "foreign";
            }
            if guard != &GuardFields::from_binding(binding) {
                return "drifted";
            }
            let Some(target) = slot.target() else {
                return "malformed";
            };
            if !target.is_file() {
                return "dangling";
            }
            if target.parent() == Some(executable_dir) {
                "aligned"
            } else if target.starts_with(prefix) {
                "drifted"
            } else {
                "cross-prefix"
            }
        }
    }
}

fn record_service_observation<R: RouteRecord>(
    record: &mut R,
    slot: &ServiceSlotEvidence,
    path: &Path,
    launcher: &Path,
    platform: service::Platform,
    binding: &Option<InstallationBinding>,
    executable_dir: &Path,
) {
    record.set_path_hex("service_path_hex", Some(path));
    record.set_path_hex("service_launcher_hex", Some(launcher));
    record.set_path_hex(
        "service_runtime_dir_hex",
        Some(&service::version_independent_runtime_dir(executable_dir)),
    );
    record_guard(record, "service", slot.guard());
    record.set(
        "service_state",
        service_state(slot, path, launcher, platform, binding.as_ref()),
    );
}

fn service_state(
    slot: &ServiceSlotEvidence,
    path: &Path,
    launcher: &Path,
    platform: service::Platform,
    binding: Option<&InstallationBinding>,
) -> &'static str {
    match slot.evidence() {
        ArtifactSlotEvidence::Fresh => "missing",
        ArtifactSlotEvidence::LegacyUnguarded => "unguarded",
        ArtifactSlotEvidence::Malformed => "malformed",
        ArtifactSlotEvidence::Ambiguous => "ambiguous",
        ArtifactSlotEvidence::Foreign => "foreign",
        ArtifactSlotEvidence::Guarded(_) => {
            let Some(binding) = binding else {
                return "missing-identity";
            };
            let guard = slot.guard().expect("guarded slot retains its guard");
            let expected = GuardFields::from_binding(binding);
            if !guard.same_identity(&expected) {
                return "foreign";
            }
            if guard != &expected {
                return "drifted";
            }
            match service::classify_unit(platform, path) {
                Ok(service::UnitTruth::Absent) => "missing",
                Ok(service::UnitTruth::Foreign) => "foreign",
                Ok(service::UnitTruth::Unknown(_)) | Err(_) => "malformed",
                Ok(service::UnitTruth::Managed(_)) => {
                    if !launcher.is_file() {
                        return "dangling";
                    }
                    match service::observe_runtime(platform, path) {
                        Ok(
                            service::RuntimeTruth::Foreign(_) | service::RuntimeTruth::Unknown(_),
                        ) => "runtime-drifted",
                        Ok(
                            service::RuntimeTruth::Absent | service::RuntimeTruth::Managed { .. },
                        ) => "aligned",
                        Err(_) => "runtime-drifted",
                    }
                }
            }
        }
    }
}

fn tuple_state(
    binding_present: bool,
    journal: &str,
    solstone: &str,
    service: &str,
) -> &'static str {
    if !binding_present {
        return "missing-identity";
    }
    if [journal, solstone, service].contains(&"malformed") {
        return "malformed";
    }
    if [journal, solstone, service].contains(&"ambiguous") {
        return "ambiguous";
    }
    if [journal, solstone, service].contains(&"foreign") {
        return "foreign";
    }
    if [journal, solstone, service].contains(&"unguarded") {
        return "unguarded";
    }
    if [journal, solstone, service].contains(&"exact-v1") {
        return "exact-v1";
    }
    if [journal, solstone, service].contains(&"missing") {
        return "missing";
    }
    if [journal, solstone, service].iter().any(|state| {
        matches!(
            *state,
            "drifted" | "cross-prefix" | "dangling" | "runtime-drifted"
        )
    }) {
        return "drifted";
    }
    "aligned"
}
