// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Read-only classification of managed wrapper and service guard artifacts.

#[cfg(any(not(windows), test))]
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use solstone_core_installation_identity::{
    ArtifactBindingEvidence, GuardFields, InstallationBinding, NamespaceName,
};

#[cfg(any(not(windows), test))]
use solstone_core_installation_identity::parse_service_guard_environment;

use crate::events::StepName;
use crate::legacy_launcher;
use crate::steps::service_artifact_path;
use crate::wrapper::{WrapperCommand, parse_wrapper, wrapper_paths};

#[cfg(any(not(windows), test))]
const SERVICE_GUARD_KEYS: [&str; 4] = [
    "SOLSTONE_INSTALLATION_NAMESPACE",
    "SOLSTONE_INSTALLATION_ID",
    "SOLSTONE_INSTALLATION_GENERATION",
    "SOLSTONE_INSTALLATION_JOURNAL_TOKEN",
];

#[derive(Clone, Debug)]
enum PresentArtifact {
    Unguarded,
    LegacyLauncher,
    Guarded(GuardFields),
}

/// Per-artifact identity evidence retained before the setup-wide fold.
///
/// This is public because native owner callers in other crates need the measured
/// state of each artifact. Construction remains private to this module.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ArtifactSlotEvidence {
    Fresh,
    LegacyUnguarded,
    Guarded(GuardFields),
    Foreign,
    Malformed,
    Ambiguous,
}

#[derive(Clone, Debug)]
pub struct WrapperSlotEvidence {
    evidence: ArtifactSlotEvidence,
    guard: Option<GuardFields>,
    target: Option<PathBuf>,
    exact_v1: bool,
    setup_rejected: bool,
}

impl WrapperSlotEvidence {
    #[must_use]
    pub const fn evidence(&self) -> &ArtifactSlotEvidence {
        &self.evidence
    }

    #[must_use]
    pub fn guard(&self) -> Option<&GuardFields> {
        self.guard.as_ref()
    }

    #[must_use]
    pub fn target(&self) -> Option<&Path> {
        self.target.as_deref()
    }

    #[must_use]
    pub const fn exact_v1(&self) -> bool {
        self.exact_v1
    }
}

#[derive(Clone, Debug)]
pub struct ServiceSlotEvidence {
    evidence: ArtifactSlotEvidence,
    guard: Option<GuardFields>,
}

impl ServiceSlotEvidence {
    #[must_use]
    pub const fn evidence(&self) -> &ArtifactSlotEvidence {
        &self.evidence
    }

    #[must_use]
    pub fn guard(&self) -> Option<&GuardFields> {
        self.guard.as_ref()
    }
}

#[derive(Clone, Debug)]
struct ReadWrapper {
    artifact: PresentArtifact,
    target: Option<PathBuf>,
    setup_rejected: bool,
}

/// Setup artifact evidence plus the guard-bearing artifacts that may need repair.
#[derive(Debug)]
pub struct SetupArtifactEvidence {
    artifacts: ArtifactBindingEvidence,
    solstone_wrapper: WrapperSlotEvidence,
    journal_wrapper: WrapperSlotEvidence,
    service: ServiceSlotEvidence,
    legacy_transition: bool,
}

impl SetupArtifactEvidence {
    #[must_use]
    pub fn artifacts(&self) -> &ArtifactBindingEvidence {
        &self.artifacts
    }

    #[must_use]
    pub const fn solstone_wrapper(&self) -> &WrapperSlotEvidence {
        &self.solstone_wrapper
    }

    #[must_use]
    pub const fn journal_wrapper(&self) -> &WrapperSlotEvidence {
        &self.journal_wrapper
    }

    #[must_use]
    pub const fn service(&self) -> &ServiceSlotEvidence {
        &self.service
    }

    #[must_use]
    pub fn repair_steps(&self, binding: &InstallationBinding) -> Vec<StepName> {
        let expected = GuardFields::from_binding(binding);
        let mut steps = Vec::new();
        // The retired journal slot cannot schedule a wrapper repair. An exact
        // V1 launcher still sets `legacy_transition`, which repairs the service.
        if matches!(
            self.solstone_wrapper.evidence,
            ArtifactSlotEvidence::LegacyUnguarded
        ) || self
            .solstone_wrapper
            .guard()
            .is_some_and(|guard| guard.same_identity(&expected) && guard != &expected)
        {
            steps.push(StepName::Wrapper);
        }
        if self.legacy_transition
            || matches!(self.service.evidence, ArtifactSlotEvidence::LegacyUnguarded)
            || self
                .service
                .guard()
                .is_some_and(|guard| guard.same_identity(&expected) && guard != &expected)
        {
            steps.push(StepName::Service);
        }
        steps
    }

    #[must_use]
    pub const fn legacy_transition(&self) -> bool {
        self.legacy_transition
    }
}

/// Classifies the current solstone wrapper only. `journal config journal` uses
/// this while it holds the same identity admission lease as setup.
pub fn gather_wrapper_artifact_evidence(
    home_dir: &Path,
    expected_namespace: &NamespaceName,
) -> ArtifactBindingEvidence {
    let paths = wrapper_paths(home_dir);
    let mut solstone = wrapper_slot(read_wrapper(&paths.solstone, WrapperCommand::Solstone));
    let mut journal = wrapper_slot(journal_admission_read(
        home_dir,
        &paths.legacy_journal,
        false,
    ));
    let mut service = service_slot(Ok(None));
    classify_slots(
        &mut solstone,
        &mut journal,
        &mut service,
        expected_namespace,
    )
}

/// Classifies every setup-owned artifact currently present for this owner.
pub fn gather_artifact_evidence(
    home_dir: &Path,
    expected_namespace: &NamespaceName,
) -> ArtifactBindingEvidence {
    gather_setup_artifact_evidence(home_dir, expected_namespace, false).artifacts
}

/// Classifies setup artifacts and retains guarded artifacts for targeted repair.
pub fn gather_setup_artifact_evidence(
    home_dir: &Path,
    expected_namespace: &NamespaceName,
    allow_legacy_launchers: bool,
) -> SetupArtifactEvidence {
    let wrapper_paths = wrapper_paths(home_dir);
    let solstone = read_setup_wrapper(
        home_dir,
        &wrapper_paths.solstone,
        "solstone",
        allow_legacy_launchers,
    );
    let journal = read_setup_wrapper(
        home_dir,
        &wrapper_paths.legacy_journal,
        "journal",
        allow_legacy_launchers,
    );
    let service = service_artifact_path(home_dir)
        .map_err(|_| ())
        .and_then(|path| path.map(|path| read_service(&path)).unwrap_or(Ok(None)));
    let mut solstone_wrapper = wrapper_slot(solstone);
    let mut journal_wrapper = wrapper_slot(journal);
    let mut service = service_slot(service);
    let legacy_transition = ![
        solstone_wrapper.evidence(),
        journal_wrapper.evidence(),
        service.evidence(),
    ]
    .into_iter()
    .any(|slot| matches!(slot, ArtifactSlotEvidence::Malformed))
        && !solstone_wrapper.setup_rejected
        && !journal_wrapper.setup_rejected
        && [&solstone_wrapper, &journal_wrapper]
            .into_iter()
            .any(WrapperSlotEvidence::exact_v1);
    let artifacts = classify_slots(
        &mut solstone_wrapper,
        &mut journal_wrapper,
        &mut service,
        expected_namespace,
    );
    SetupArtifactEvidence {
        artifacts,
        solstone_wrapper,
        journal_wrapper,
        service,
        legacy_transition,
    }
}

/// Whether either managed wrapper's own recorded launch target has drifted
/// from the executable directory this run resolved to.
///
/// A version swap moves `executable_dir` -- the running binary now lives
/// under a different `versions/<ver>-<digest>/bin`, even though
/// `resolve_identity_root_from_executable_dir` deliberately keeps the
/// installation identity (and so every guard field) unchanged. That means the
/// ordinary guard-field comparison in [`SetupArtifactEvidence::repair_steps`]
/// never notices a version swap on its own: the wrapper is still guarded, and
/// the guard still matches. Reading each wrapper's own `SOL_BIN=` line is the
/// only place that swap is visible, and it is what lets `journal setup`
/// repoint the wrapper -- and, forced alongside it, the service unit it
/// installs -- onto the newly installed build instead of silently leaving
/// both pinned to the version that was current at the last setup run.
#[must_use]
pub fn wrapper_targets_drifted(home_dir: &Path, executable_dir: &Path) -> bool {
    let paths = wrapper_paths(home_dir);
    fs::read_to_string(&paths.solstone)
        .ok()
        .as_deref()
        .and_then(|content| parse_wrapper(WrapperCommand::Solstone, content))
        .is_some_and(|wrapper| wrapper.sol_bin.parent() != Some(executable_dir))
}

const APP_OWNED_CHILD_PREFIX: &[u8] = b"#!/bin/sh\n# managed-version: app-owned-child\n";

/// Admission for the retired `~/.local/bin/journal` slot.
///
/// An exact V1 launcher is still the legacy transition. A classify error on a
/// regular file, and a regular file that starts like an app-owned child but
/// was not accepted, stay malformed. A symlink is fresh and is not followed.
/// Every other V2 state is fresh: it cannot make admission malformed, cannot
/// be drift, and cannot schedule repair.
fn journal_admission_read(
    home_dir: &Path,
    path: &Path,
    allow_legacy_launchers: bool,
) -> Result<Option<ReadWrapper>, ()> {
    if allow_legacy_launchers {
        match legacy_launcher::classify(home_dir, path, "journal") {
            Ok(Some(_)) => {
                return Ok(Some(ReadWrapper {
                    artifact: PresentArtifact::LegacyLauncher,
                    target: None,
                    setup_rejected: false,
                }));
            }
            Err(_) if regular_file(path) => return Err(()),
            Err(_) | Ok(None) => {}
        }
    }
    match fs::symlink_metadata(path) {
        Err(_) => return Ok(None),
        Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_file() => {
            return Ok(None);
        }
        Ok(_) => {}
    }
    if fs::read(path).is_ok_and(|bytes| bytes.starts_with(APP_OWNED_CHILD_PREFIX)) {
        return Err(());
    }
    Ok(None)
}

fn regular_file(path: &Path) -> bool {
    fs::symlink_metadata(path)
        .is_ok_and(|metadata| metadata.file_type().is_file() && !metadata.file_type().is_symlink())
}

/// Route inspection of the retired journal slot. A symlink is `foreign` and is
/// not followed. This classification is not fed back into setup admission.
#[must_use]
pub fn route_journal_wrapper_slot(path: &Path) -> WrapperSlotEvidence {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() => WrapperSlotEvidence {
            evidence: ArtifactSlotEvidence::Foreign,
            guard: None,
            target: None,
            exact_v1: false,
            setup_rejected: false,
        },
        _ => wrapper_slot(read_wrapper(path, WrapperCommand::LegacyJournal)),
    }
}

fn read_setup_wrapper(
    home_dir: &Path,
    path: &Path,
    command: &str,
    allow_legacy_launchers: bool,
) -> Result<Option<ReadWrapper>, ()> {
    if command == "journal" {
        return journal_admission_read(home_dir, path, allow_legacy_launchers);
    }
    if allow_legacy_launchers {
        let classified = legacy_launcher::classify(home_dir, path, command).map_err(|_| ())?;
        if classified.is_some() {
            return Ok(Some(ReadWrapper {
                artifact: PresentArtifact::LegacyLauncher,
                target: None,
                setup_rejected: false,
            }));
        }
    }
    let command = match command {
        "solstone" => WrapperCommand::Solstone,
        "journal" => WrapperCommand::LegacyJournal,
        _ => return Err(()),
    };
    let mut wrapper = read_wrapper(path, command)?;
    if allow_legacy_launchers
        && matches!(
            wrapper.as_ref().map(|wrapper| &wrapper.artifact),
            Some(PresentArtifact::Unguarded)
        )
    {
        wrapper
            .as_mut()
            .expect("matched wrapper is present")
            .setup_rejected = true;
    }
    Ok(wrapper)
}

fn read_wrapper(path: &Path, command: WrapperCommand) -> Result<Option<ReadWrapper>, ()> {
    if !path.exists() && !path.is_symlink() {
        return Ok(None);
    }
    let content = fs::read_to_string(path).map_err(|_| ())?;
    let wrapper = parse_wrapper(command, &content).ok_or(())?;
    Ok(Some(ReadWrapper {
        artifact: wrapper
            .guard
            .map_or(PresentArtifact::Unguarded, PresentArtifact::Guarded),
        target: Some(wrapper.sol_bin),
        setup_rejected: false,
    }))
}

fn read_service(path: &Path) -> Result<Option<PresentArtifact>, ()> {
    if !path.exists() && !path.is_symlink() {
        return Ok(None);
    }
    #[cfg(windows)]
    {
        let bytes = fs::read(path).map_err(|_| ())?;
        let xml = solstone_core_service_unit::decode_windows_task_xml(&bytes).map_err(|_| ())?;
        let definition =
            solstone_core_service_unit::parse_windows_task_xml(&xml).map_err(|_| ())?;
        Ok(Some(PresentArtifact::Guarded(definition.action.guard)))
    }
    #[cfg(not(windows))]
    read_unix_service(path)
}

#[cfg(not(windows))]
fn read_unix_service(path: &Path) -> Result<Option<PresentArtifact>, ()> {
    let content = fs::read_to_string(path).map_err(|_| ())?;
    let environment = extract_service_guards(&content)?;
    parse_service_guard_environment(&environment)
        .map(|guard| guard.map_or(PresentArtifact::Unguarded, PresentArtifact::Guarded))
        .map(Some)
        .map_err(|_| ())
}

fn wrapper_slot(value: Result<Option<ReadWrapper>, ()>) -> WrapperSlotEvidence {
    match value {
        Err(()) => WrapperSlotEvidence {
            evidence: ArtifactSlotEvidence::Malformed,
            guard: None,
            target: None,
            exact_v1: false,
            setup_rejected: false,
        },
        Ok(None) => WrapperSlotEvidence {
            evidence: ArtifactSlotEvidence::Fresh,
            guard: None,
            target: None,
            exact_v1: false,
            setup_rejected: false,
        },
        Ok(Some(ReadWrapper {
            artifact: PresentArtifact::Unguarded,
            target,
            setup_rejected,
        })) => WrapperSlotEvidence {
            evidence: ArtifactSlotEvidence::LegacyUnguarded,
            guard: None,
            target,
            exact_v1: false,
            setup_rejected,
        },
        Ok(Some(ReadWrapper {
            artifact: PresentArtifact::LegacyLauncher,
            target,
            setup_rejected,
        })) => WrapperSlotEvidence {
            evidence: ArtifactSlotEvidence::LegacyUnguarded,
            guard: None,
            target,
            exact_v1: true,
            setup_rejected,
        },
        Ok(Some(ReadWrapper {
            artifact: PresentArtifact::Guarded(guard),
            target,
            setup_rejected,
        })) => WrapperSlotEvidence {
            evidence: ArtifactSlotEvidence::Guarded(guard.clone()),
            guard: Some(guard),
            target,
            exact_v1: false,
            setup_rejected,
        },
    }
}

fn service_slot(value: Result<Option<PresentArtifact>, ()>) -> ServiceSlotEvidence {
    match value {
        Err(()) => ServiceSlotEvidence {
            evidence: ArtifactSlotEvidence::Malformed,
            guard: None,
        },
        Ok(None) => ServiceSlotEvidence {
            evidence: ArtifactSlotEvidence::Fresh,
            guard: None,
        },
        Ok(Some(PresentArtifact::Unguarded | PresentArtifact::LegacyLauncher)) => {
            ServiceSlotEvidence {
                evidence: ArtifactSlotEvidence::LegacyUnguarded,
                guard: None,
            }
        }
        Ok(Some(PresentArtifact::Guarded(guard))) => ServiceSlotEvidence {
            evidence: ArtifactSlotEvidence::Guarded(guard.clone()),
            guard: Some(guard),
        },
    }
}

fn classify_slots(
    solstone: &mut WrapperSlotEvidence,
    journal: &mut WrapperSlotEvidence,
    service: &mut ServiceSlotEvidence,
    expected_namespace: &NamespaceName,
) -> ArtifactBindingEvidence {
    if solstone.setup_rejected || journal.setup_rejected {
        return ArtifactBindingEvidence::Malformed;
    }
    let states = [&solstone.evidence, &journal.evidence, &service.evidence];
    if states
        .iter()
        .any(|state| matches!(state, ArtifactSlotEvidence::Malformed))
    {
        return ArtifactBindingEvidence::Malformed;
    }
    let active = states
        .iter()
        .filter(|state| !matches!(state, ArtifactSlotEvidence::Fresh))
        .collect::<Vec<_>>();
    if active.is_empty() {
        return ArtifactBindingEvidence::Fresh;
    }
    if active
        .iter()
        .all(|state| matches!(state, ArtifactSlotEvidence::LegacyUnguarded))
    {
        return ArtifactBindingEvidence::LegacyUnguarded;
    }
    let guards = [&solstone.guard, &journal.guard, &service.guard]
        .into_iter()
        .filter_map(Option::as_ref)
        .collect::<Vec<_>>();
    let first = guards[0];
    if guards.iter().any(|guard| !guard.same_identity(first)) {
        for slot in [&mut solstone.evidence, &mut journal.evidence] {
            if matches!(slot, ArtifactSlotEvidence::Guarded(_)) {
                *slot = ArtifactSlotEvidence::Ambiguous;
            }
        }
        if matches!(service.evidence, ArtifactSlotEvidence::Guarded(_)) {
            service.evidence = ArtifactSlotEvidence::Ambiguous;
        }
        return ArtifactBindingEvidence::Ambiguous;
    }
    if first.namespace != *expected_namespace {
        for slot in [&mut solstone.evidence, &mut journal.evidence] {
            if matches!(slot, ArtifactSlotEvidence::Guarded(_)) {
                *slot = ArtifactSlotEvidence::Foreign;
            }
        }
        if matches!(service.evidence, ArtifactSlotEvidence::Guarded(_)) {
            service.evidence = ArtifactSlotEvidence::Foreign;
        }
        return ArtifactBindingEvidence::Foreign;
    }
    ArtifactBindingEvidence::Guarded(first.clone())
}

#[cfg(any(not(windows), test))]
fn extract_service_guards(content: &str) -> Result<BTreeMap<String, String>, ()> {
    let mut values = BTreeMap::new();
    if content
        .lines()
        .any(|line| line.trim().starts_with("Environment="))
    {
        for line in content.lines() {
            let Some(value) = line.trim().strip_prefix("Environment=") else {
                continue;
            };
            let value = value
                .strip_prefix('"')
                .and_then(|value| value.strip_suffix('"'))
                .unwrap_or(value);
            insert_service_guard(&mut values, value)?;
        }
    } else if let Some(start) = content.find("<key>EnvironmentVariables</key>") {
        let remainder = &content[start + "<key>EnvironmentVariables</key>".len()..];
        let Some(end) = remainder.find("</dict>") else {
            return Err(());
        };
        let mut dictionary = &remainder[..end];
        while let Some(key_start) = dictionary.find("<key>") {
            dictionary = &dictionary[key_start + "<key>".len()..];
            let Some(key_end) = dictionary.find("</key>") else {
                return Err(());
            };
            let key = &dictionary[..key_end];
            dictionary = &dictionary[key_end + "</key>".len()..];
            let Some(value_start) = dictionary.find("<string>") else {
                return Err(());
            };
            dictionary = &dictionary[value_start + "<string>".len()..];
            let Some(value_end) = dictionary.find("</string>") else {
                return Err(());
            };
            let value = &dictionary[..value_end];
            dictionary = &dictionary[value_end + "</string>".len()..];
            insert_service_guard(&mut values, &format!("{key}={value}"))?;
        }
    }
    Ok(values)
}

#[cfg(any(not(windows), test))]
fn insert_service_guard(values: &mut BTreeMap<String, String>, entry: &str) -> Result<(), ()> {
    let Some((key, value)) = entry.split_once('=') else {
        return Ok(());
    };
    if !SERVICE_GUARD_KEYS.contains(&key) {
        return Ok(());
    }
    if values.insert(key.to_owned(), value.to_owned()).is_some() {
        return Err(());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use solstone_core_installation_identity::{Generation, InstallationId, NamespaceName};
    #[cfg(unix)]
    use std::os::unix::fs::{PermissionsExt, symlink};

    fn absolute_test_path(relative: &str) -> PathBuf {
        let root = if cfg!(windows) { r"C:\" } else { "/" };
        relative.split('/').filter(|part| !part.is_empty()).fold(
            PathBuf::from(root),
            |mut path, part| {
                path.push(part);
                path
            },
        )
    }

    fn fields() -> GuardFields {
        GuardFields {
            namespace: NamespaceName::parse(
                "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef",
            )
            .expect("namespace"),
            id: InstallationId::parse("00112233445566778899aabbccddeeff").expect("id"),
            generation: Generation::new(1).expect("generation"),
            journal_token: solstone_core_installation_identity::journal_token_from_path(
                &absolute_test_path("journal"),
            )
            .expect("journal"),
        }
    }

    #[test]
    fn service_environment_extracts_only_identity_keys_and_rejects_duplicates() {
        let fields = fields();
        let environment = solstone_core_installation_identity::service_guard_environment(&fields);
        let content = environment
            .iter()
            .map(|(key, value)| format!("Environment=\"{key}={value}\""))
            .collect::<Vec<_>>()
            .join("\n");
        assert_eq!(
            parse_service_guard_environment(&extract_service_guards(&content).expect("extract")),
            Ok(Some(fields.clone()))
        );
        assert!(
            extract_service_guards(&format!(
                "{content}\nEnvironment=SOLSTONE_INSTALLATION_ID=x"
            ))
            .is_err()
        );
    }

    #[test]
    fn journal_token_drift_for_one_identity_remains_guarded() {
        let original = fields();
        let changed = GuardFields {
            journal_token: solstone_core_installation_identity::journal_token_from_path(
                &absolute_test_path("changed-journal"),
            )
            .expect("changed journal"),
            ..original.clone()
        };
        let mut solstone = WrapperSlotEvidence {
            evidence: ArtifactSlotEvidence::Guarded(original.clone()),
            guard: Some(original.clone()),
            target: None,
            exact_v1: false,
            setup_rejected: false,
        };
        let mut journal = WrapperSlotEvidence {
            evidence: ArtifactSlotEvidence::Guarded(changed.clone()),
            guard: Some(changed),
            target: None,
            exact_v1: false,
            setup_rejected: false,
        };
        let mut service = service_slot(Ok(None));
        assert_eq!(
            classify_slots(
                &mut solstone,
                &mut journal,
                &mut service,
                &original.namespace,
            ),
            ArtifactBindingEvidence::Guarded(original)
        );
    }

    /// A version swap leaves the guard fields untouched (see
    /// `identity_root_uses_a_versioned_prefix_from_the_resolved_version_directory`
    /// in `solstone-core-journal`) but does move `executable_dir`, so this is
    /// the one signal that catches it: without it, `journal setup` would
    /// report success after a respin while silently leaving the wrapper
    /// pointed at the build it just replaced.
    // Managed POSIX shell wrappers are not published on Windows.
    #[cfg(unix)]
    #[test]
    fn wrapper_targets_drifted_detects_a_version_swap_the_guard_cannot_see() {
        let root = std::env::temp_dir().join(format!(
            "solstone-identity-wrapper-drift-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&root);
        let home = root.join("home");
        let bin = home.join(".local/bin");
        fs::create_dir_all(&bin).unwrap();
        let old_executable_dir = root.join("versions/1.0.0-aaaaaaaa/bin");
        let new_executable_dir = root.join("versions/2.0.0-bbbbbbbb/bin");
        fs::create_dir_all(&old_executable_dir).unwrap();
        fs::create_dir_all(&new_executable_dir).unwrap();

        let guard = fields();
        fs::write(
            bin.join("solstone"),
            crate::wrapper::render_wrapper(
                WrapperCommand::Solstone,
                &absolute_test_path("journal"),
                &old_executable_dir.join("solstone"),
                &guard,
            )
            .unwrap(),
        )
        .unwrap();

        assert!(
            !wrapper_targets_drifted(&home, &old_executable_dir),
            "no drift while executable_dir still matches the wrapper's own SOL_BIN"
        );
        assert!(
            wrapper_targets_drifted(&home, &new_executable_dir),
            "a version swap must be visible even though the guard fields never changed"
        );

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    #[cfg(unix)]
    fn exact_v1_launcher_is_admitted_only_by_the_setup_transition() {
        let root = std::env::temp_dir().join(format!(
            "solstone-identity-v1-launcher-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&root);
        let home = root.join("home");
        let legacy_bin = home.join(".local/share/uv/tools/solstone/bin");
        let public_bin = home.join(".local/bin");
        fs::create_dir_all(&legacy_bin).unwrap();
        fs::create_dir_all(&public_bin).unwrap();
        let launcher = legacy_bin.join("solstone");
        fs::write(
            &launcher,
            concat!(
                "#!/usr/bin/python3\n",
                "# -*- coding: utf-8 -*-\n",
                "import sys\n",
                "from solstone.think.sol_cli import main\n",
                "if __name__ == '__main__':\n",
                "    if sys.argv[0].endswith('-script.pyw'):\n",
                "        sys.argv[0] = sys.argv[0][:-11]\n",
                "    elif sys.argv[0].endswith('.exe'):\n",
                "        sys.argv[0] = sys.argv[0][:-4]\n",
                "    sys.exit(main())\n"
            ),
        )
        .unwrap();
        fs::set_permissions(&launcher, fs::Permissions::from_mode(0o755)).unwrap();
        symlink(&launcher, public_bin.join("solstone")).unwrap();

        let namespace = fields().namespace;
        assert_eq!(
            gather_artifact_evidence(&home, &namespace),
            ArtifactBindingEvidence::Malformed
        );
        let setup = gather_setup_artifact_evidence(&home, &namespace, true);
        assert_eq!(setup.artifacts(), &ArtifactBindingEvidence::LegacyUnguarded);
        assert!(setup.legacy_transition());

        let near_twin = public_bin.join("journal");
        fs::write(
            &near_twin,
            concat!(
                "#!/bin/sh\n",
                "# managed-version: 7\n",
                ": \"${SOLSTONE_JOURNAL:=/journal}\"\n",
                "SOL_BIN='/owner-authored/journal'\n",
            ),
        )
        .unwrap();
        fs::set_permissions(&near_twin, fs::Permissions::from_mode(0o755)).unwrap();
        let refused = gather_setup_artifact_evidence(&home, &namespace, true);
        assert!(matches!(
            refused.journal_wrapper().evidence(),
            ArtifactSlotEvidence::Fresh
        ));
        assert_eq!(
            refused.artifacts(),
            &ArtifactBindingEvidence::LegacyUnguarded
        );
        assert!(refused.legacy_transition());
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    #[cfg(unix)]
    fn exact_v1_3_31_app_owned_children_form_one_legacy_transition() {
        let root = std::env::temp_dir().join(format!(
            "solstone-identity-v1-app-owned-child-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        // The recognizer requires the wrapper's literal target under the
        // canonical home; macOS's temp dir is a /var -> /private/var alias.
        let root = fs::canonicalize(&root).unwrap();
        let home = root.join("home");
        let generation =
            home.join("Library/Application Support/sol/runtime/0.6.24_py20260510_bbd54541379bee6d");
        let bin = generation.join("bin");
        let tools = generation.join("tools/solstone/bin");
        fs::create_dir_all(home.join(".local/bin")).unwrap();
        fs::create_dir_all(&bin).unwrap();
        fs::create_dir_all(&tools).unwrap();
        for command in ["sol", "journal"] {
            let executable = tools.join(command);
            fs::write(&executable, "#!/bin/sh\nexit 0\n").unwrap();
            fs::set_permissions(&executable, fs::Permissions::from_mode(0o755)).unwrap();
            symlink(&executable, bin.join(command)).unwrap();
            let public = home.join(".local/bin").join(command);
            fs::write(
                &public,
                format!(
                    "#!/bin/sh\n# managed-version: app-owned-child\nexec '{}' \"$@\"\n",
                    bin.join(command).display()
                ),
            )
            .unwrap();
            fs::set_permissions(&public, fs::Permissions::from_mode(0o755)).unwrap();
        }

        let namespace = fields().namespace;
        let setup = gather_setup_artifact_evidence(&home, &namespace, true);
        assert_eq!(setup.artifacts(), &ArtifactBindingEvidence::LegacyUnguarded);
        assert!(setup.legacy_transition());

        let journal = home.join(".local/bin/journal");
        let exact = fs::read_to_string(&journal).unwrap();
        fs::write(&journal, exact.replace(" \"$@\"", " \"$@\" extra")).unwrap();
        let malformed = gather_setup_artifact_evidence(&home, &namespace, true);
        assert_eq!(malformed.artifacts(), &ArtifactBindingEvidence::Malformed);
        assert!(!malformed.legacy_transition());
        let _ = fs::remove_dir_all(root);
    }

    // Managed POSIX shell wrappers are not published on Windows.
    #[cfg(unix)]
    #[test]
    fn legacy_classify_error_refuses_before_valid_wrapper_fallback() {
        let root = std::env::temp_dir().join(format!(
            "solstone-identity-classify-error-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&root);
        let home_dir = root.join("missing-home");
        let wrapper_dir = root.join("separate-wrapper");
        fs::create_dir_all(&wrapper_dir).unwrap();
        let path = wrapper_dir.join("journal");
        fs::write(
            &path,
            crate::wrapper::render_wrapper_version(
                WrapperCommand::LegacyJournal,
                &absolute_test_path("journal"),
                &absolute_test_path("owner-authored/journal"),
                Some(&fields()),
                8,
            )
            .unwrap(),
        )
        .unwrap();

        assert!(matches!(
            read_wrapper(&path, WrapperCommand::LegacyJournal),
            Ok(Some(wrapper)) if matches!(wrapper.artifact, PresentArtifact::Guarded(_))
        ));
        assert!(matches!(
            read_setup_wrapper(&home_dir, &path, "journal", true),
            Err(())
        ));

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    #[cfg(unix)]
    fn published_v8_wrapper_pair_is_admitted_for_a_version_swap() {
        let root = std::env::temp_dir().join(format!(
            "solstone-published-v8-upgrade-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&root);
        let home = root.join("home");
        let bin = home.join(".local/bin");
        let prior = root.join("versions/2.0.30-prior/bin");
        let next = root.join("versions/2.0.31-next/bin");
        fs::create_dir_all(&bin).unwrap();
        let guard = fields();
        for command in [WrapperCommand::Solstone, WrapperCommand::LegacyJournal] {
            let rendered = if command == WrapperCommand::LegacyJournal {
                crate::wrapper::render_wrapper_version(
                    command,
                    &absolute_test_path("journal"),
                    &prior.join(command.as_str()),
                    Some(&guard),
                    8,
                )
            } else {
                crate::wrapper::render_wrapper(
                    command,
                    &absolute_test_path("journal"),
                    &prior.join(command.as_str()),
                    &guard,
                )
            }
            .unwrap();
            let published = rendered.replacen(
                "managed by 'solstone journal config'.",
                "managed by 'journal config'.",
                1,
            );
            fs::write(bin.join(command.as_str()), published).unwrap();
        }
        let evidence = gather_setup_artifact_evidence(&home, &guard.namespace, true);
        assert_eq!(
            evidence.artifacts(),
            &ArtifactBindingEvidence::Guarded(guard.clone())
        );
        assert!(!wrapper_targets_drifted(&home, &prior));
        assert!(wrapper_targets_drifted(&home, &next));

        let alias = bin.join("journal");
        let original = fs::read_to_string(&alias).unwrap();
        fs::write(
            &alias,
            original.replace("exec \"$SOL_BIN\" \"$@\"", "exec /foreign/journal \"$@\""),
        )
        .unwrap();
        let corrupted = gather_setup_artifact_evidence(&home, &guard.namespace, true);
        assert!(matches!(
            corrupted.journal_wrapper().evidence(),
            ArtifactSlotEvidence::Fresh
        ));
        assert_eq!(
            corrupted.artifacts(),
            &ArtifactBindingEvidence::Guarded(guard.clone())
        );
        let _ = fs::remove_dir_all(root);
    }

    // Managed POSIX shell wrappers are not published on Windows.
    #[cfg(unix)]
    #[test]
    fn named_slot_accessors_preserve_each_artifact_before_the_aggregate_fold() {
        let root =
            std::env::temp_dir().join(format!("solstone-identity-slots-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        let home = root.join("home");
        let bin = home.join(".local/bin");
        let runtime = root.join("runtime");
        fs::create_dir_all(&bin).unwrap();
        fs::create_dir_all(&runtime).unwrap();
        let guard = fields();
        for command in [WrapperCommand::LegacyJournal, WrapperCommand::Solstone] {
            let rendered = if command == WrapperCommand::LegacyJournal {
                crate::wrapper::render_wrapper_version(
                    command,
                    &absolute_test_path("journal"),
                    &runtime.join(command.as_str()),
                    Some(&guard),
                    8,
                )
            } else {
                crate::wrapper::render_wrapper(
                    command,
                    &absolute_test_path("journal"),
                    &runtime.join(command.as_str()),
                    &guard,
                )
            }
            .unwrap();
            fs::write(bin.join(command.as_str()), rendered).unwrap();
        }
        let service_path = service_artifact_path(&home)
            .expect("resolve service artifact")
            .expect("supported test platform");
        fs::create_dir_all(service_path.parent().unwrap()).unwrap();
        let service = solstone_core_installation_identity::service_guard_environment(&guard)
            .into_iter()
            .map(|(key, value)| format!("Environment={key}={value}"))
            .collect::<Vec<_>>()
            .join("\n");
        fs::write(&service_path, service).unwrap();

        let evidence = gather_setup_artifact_evidence(&home, &guard.namespace, false);
        assert!(matches!(
            evidence.journal_wrapper().evidence(),
            ArtifactSlotEvidence::Fresh
        ));
        assert_eq!(evidence.journal_wrapper().target(), None);
        assert!(matches!(
            evidence.solstone_wrapper().evidence(),
            ArtifactSlotEvidence::Guarded(_)
        ));
        assert!(matches!(
            evidence.service().evidence(),
            ArtifactSlotEvidence::Guarded(_)
        ));

        fs::remove_file(bin.join("solstone")).unwrap();
        let missing = gather_setup_artifact_evidence(&home, &guard.namespace, false);
        assert!(matches!(
            missing.journal_wrapper().evidence(),
            ArtifactSlotEvidence::Fresh
        ));
        assert!(matches!(
            missing.solstone_wrapper().evidence(),
            ArtifactSlotEvidence::Fresh
        ));

        fs::remove_file(&service_path).unwrap();
        let foreign_guard = GuardFields {
            namespace: NamespaceName::parse(
                "fedcba9876543210fedcba9876543210fedcba9876543210fedcba9876543210",
            )
            .unwrap(),
            ..guard.clone()
        };
        fs::write(
            bin.join("journal"),
            crate::wrapper::render_wrapper_version(
                WrapperCommand::LegacyJournal,
                &absolute_test_path("journal"),
                &runtime.join("journal"),
                Some(&foreign_guard),
                8,
            )
            .unwrap(),
        )
        .unwrap();
        let foreign = gather_setup_artifact_evidence(&home, &guard.namespace, false);
        assert!(matches!(
            foreign.journal_wrapper().evidence(),
            ArtifactSlotEvidence::Fresh
        ));
        assert!(matches!(
            foreign.solstone_wrapper().evidence(),
            ArtifactSlotEvidence::Fresh
        ));

        fs::write(
            bin.join("solstone"),
            crate::wrapper::render_wrapper(
                WrapperCommand::Solstone,
                &absolute_test_path("journal"),
                &runtime.join("solstone"),
                &guard,
            )
            .unwrap(),
        )
        .unwrap();
        fs::write(
            bin.join("journal"),
            crate::wrapper::render_wrapper_version(
                WrapperCommand::LegacyJournal,
                &absolute_test_path("journal"),
                &runtime.join("journal"),
                Some(&guard),
                8,
            )
            .unwrap(),
        )
        .unwrap();
        let ambiguous_guard = GuardFields {
            id: InstallationId::parse("ffeeddccbbaa99887766554433221100").unwrap(),
            ..guard.clone()
        };
        let service =
            solstone_core_installation_identity::service_guard_environment(&ambiguous_guard)
                .into_iter()
                .map(|(key, value)| format!("Environment={key}={value}"))
                .collect::<Vec<_>>()
                .join("\n");
        fs::create_dir_all(service_path.parent().unwrap()).unwrap();
        fs::write(&service_path, service).unwrap();
        let ambiguous = gather_setup_artifact_evidence(&home, &guard.namespace, false);
        assert!(matches!(
            ambiguous.service().evidence(),
            ArtifactSlotEvidence::Ambiguous
        ));
        assert!(matches!(
            ambiguous.journal_wrapper().evidence(),
            ArtifactSlotEvidence::Fresh
        ));

        let _ = fs::remove_dir_all(root);
    }
}
