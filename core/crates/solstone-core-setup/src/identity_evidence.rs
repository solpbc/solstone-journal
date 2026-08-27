// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Read-only classification of managed wrapper and service guard artifacts.

use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

use solstone_core_installation_identity::{
    ArtifactBindingEvidence, GuardFields, InstallationBinding, NamespaceName,
    parse_service_guard_environment, parse_wrapper_guard,
};

use crate::events::StepName;
use crate::legacy_launcher;
use crate::steps::service_artifact_path;
use crate::wrapper::{parse_wrapper, wrapper_paths};

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

/// Setup artifact evidence plus the guard-bearing artifacts that may need repair.
#[derive(Debug)]
pub struct SetupArtifactEvidence {
    artifacts: ArtifactBindingEvidence,
    wrapper_guards: Vec<GuardFields>,
    service_guard: Option<GuardFields>,
    wrapper_unguarded: bool,
    service_unguarded: bool,
    legacy_transition: bool,
}

impl SetupArtifactEvidence {
    #[must_use]
    pub fn artifacts(&self) -> &ArtifactBindingEvidence {
        &self.artifacts
    }

    #[must_use]
    pub fn repair_steps(&self, binding: &InstallationBinding) -> Vec<StepName> {
        let expected = GuardFields::from_binding(binding);
        let mut steps = Vec::new();
        if self.wrapper_unguarded
            || self
                .wrapper_guards
                .iter()
                .any(|guard| guard.same_identity(&expected) && guard != &expected)
        {
            steps.push(StepName::Wrapper);
        }
        if self.legacy_transition
            || self.service_unguarded
            || self
                .service_guard
                .as_ref()
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
    let artifacts = match (read_wrapper(&paths.solstone), read_wrapper(&paths.journal)) {
        (Ok(solstone), Ok(journal)) => solstone.into_iter().chain(journal).collect(),
        _ => return ArtifactBindingEvidence::Malformed,
    };
    classify(Ok(artifacts), expected_namespace)
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
        &wrapper_paths.journal,
        "journal",
        allow_legacy_launchers,
    );
    let service = service_artifact_path(home_dir)
        .map(|path| read_service(&path))
        .unwrap_or(Ok(None));
    let (solstone, journal, service) = match (solstone, journal, service) {
        (Ok(solstone), Ok(journal), Ok(service)) => (solstone, journal, service),
        _ => {
            return SetupArtifactEvidence {
                artifacts: ArtifactBindingEvidence::Malformed,
                wrapper_guards: Vec::new(),
                service_guard: None,
                wrapper_unguarded: false,
                service_unguarded: false,
                legacy_transition: false,
            };
        }
    };
    let wrapper_guards = [&solstone, &journal]
        .into_iter()
        .filter_map(|artifact| match artifact {
            Some(PresentArtifact::Guarded(guard)) => Some(guard.clone()),
            _ => None,
        })
        .collect();
    let service_guard = match &service {
        Some(PresentArtifact::Guarded(guard)) => Some(guard.clone()),
        _ => None,
    };
    let wrapper_unguarded = [&solstone, &journal].into_iter().any(|artifact| {
        matches!(
            artifact,
            Some(PresentArtifact::Unguarded | PresentArtifact::LegacyLauncher)
        )
    });
    let service_unguarded = matches!(&service, Some(PresentArtifact::Unguarded));
    let legacy_transition = [&solstone, &journal]
        .into_iter()
        .any(|artifact| matches!(artifact, Some(PresentArtifact::LegacyLauncher)));
    let artifacts = solstone
        .into_iter()
        .chain(journal)
        .chain(service)
        .collect::<Vec<_>>();
    SetupArtifactEvidence {
        artifacts: classify(Ok(artifacts), expected_namespace),
        wrapper_guards,
        service_guard,
        wrapper_unguarded,
        service_unguarded,
        legacy_transition,
    }
}

fn read_setup_wrapper(
    home_dir: &Path,
    path: &Path,
    command: &str,
    allow_legacy_launchers: bool,
) -> Result<Option<PresentArtifact>, ()> {
    match read_wrapper(path) {
        Ok(artifact) => Ok(artifact),
        Err(()) if allow_legacy_launchers => legacy_launcher::classify(home_dir, path, command)
            .map_err(|_| ())
            .and_then(|artifact| {
                artifact.map_or(Err(()), |_| Ok(Some(PresentArtifact::LegacyLauncher)))
            }),
        Err(()) => Err(()),
    }
}

fn read_wrapper(path: &Path) -> Result<Option<PresentArtifact>, ()> {
    if !path.exists() && !path.is_symlink() {
        return Ok(None);
    }
    let content = fs::read_to_string(path).map_err(|_| ())?;
    if parse_wrapper(&content).is_none() {
        return Err(());
    }
    parse_wrapper_guard(&content)
        .map(|guard| guard.map_or(PresentArtifact::Unguarded, PresentArtifact::Guarded))
        .map(Some)
        .map_err(|_| ())
}

fn read_service(path: &Path) -> Result<Option<PresentArtifact>, ()> {
    if !path.exists() && !path.is_symlink() {
        return Ok(None);
    }
    let content = fs::read_to_string(path).map_err(|_| ())?;
    let environment = extract_service_guards(&content)?;
    parse_service_guard_environment(&environment)
        .map(|guard| guard.map_or(PresentArtifact::Unguarded, PresentArtifact::Guarded))
        .map(Some)
        .map_err(|_| ())
}

fn classify(
    artifacts: Result<Vec<PresentArtifact>, ()>,
    expected_namespace: &NamespaceName,
) -> ArtifactBindingEvidence {
    let Ok(artifacts) = artifacts else {
        return ArtifactBindingEvidence::Malformed;
    };
    if artifacts.is_empty() {
        return ArtifactBindingEvidence::Fresh;
    }
    if artifacts.iter().all(|artifact| {
        matches!(
            artifact,
            PresentArtifact::Unguarded | PresentArtifact::LegacyLauncher
        )
    }) {
        return ArtifactBindingEvidence::LegacyUnguarded;
    }
    let guards = artifacts
        .iter()
        .filter_map(|artifact| match artifact {
            PresentArtifact::Guarded(fields) => Some(fields),
            PresentArtifact::Unguarded | PresentArtifact::LegacyLauncher => None,
        })
        .collect::<Vec<_>>();
    let first = guards[0];
    if guards.iter().any(|guard| !guard.same_identity(first)) {
        return ArtifactBindingEvidence::Ambiguous;
    }
    if first.namespace != *expected_namespace {
        return ArtifactBindingEvidence::Foreign;
    }
    ArtifactBindingEvidence::Guarded(first.clone())
}

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
    use solstone_core_installation_identity::{
        Generation, InstallationId, JournalToken, NamespaceName,
    };
    use std::os::unix::fs::{PermissionsExt, symlink};

    fn fields() -> GuardFields {
        GuardFields {
            namespace: NamespaceName::parse(
                "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef",
            )
            .expect("namespace"),
            id: InstallationId::parse("00112233445566778899aabbccddeeff").expect("id"),
            generation: Generation::new(1).expect("generation"),
            journal_token: JournalToken::from_raw_absolute(b"/journal".to_vec()).expect("journal"),
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
            journal_token: JournalToken::from_raw_absolute(b"/changed-journal".to_vec())
                .expect("changed journal"),
            ..original.clone()
        };
        assert_eq!(
            classify(
                Ok(vec![
                    PresentArtifact::Guarded(original.clone()),
                    PresentArtifact::Guarded(changed),
                ]),
                &original.namespace,
            ),
            ArtifactBindingEvidence::Guarded(original)
        );
    }

    #[test]
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
        let _ = fs::remove_dir_all(root);
    }
}
