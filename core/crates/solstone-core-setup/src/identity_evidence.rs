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
    Guarded(GuardFields),
}

/// Setup artifact evidence plus the guard-bearing artifacts that may need repair.
#[derive(Debug)]
pub struct SetupArtifactEvidence {
    artifacts: ArtifactBindingEvidence,
    wrapper_guards: Vec<GuardFields>,
    service_guard: Option<GuardFields>,
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
        if self
            .wrapper_guards
            .iter()
            .any(|guard| guard.same_identity(&expected) && guard != &expected)
        {
            steps.push(StepName::Wrapper);
        }
        if self
            .service_guard
            .as_ref()
            .is_some_and(|guard| guard.same_identity(&expected) && guard != &expected)
        {
            steps.push(StepName::Service);
        }
        steps
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
    gather_setup_artifact_evidence(home_dir, expected_namespace).artifacts
}

/// Classifies setup artifacts and retains guarded artifacts for targeted repair.
pub fn gather_setup_artifact_evidence(
    home_dir: &Path,
    expected_namespace: &NamespaceName,
) -> SetupArtifactEvidence {
    let wrapper_paths = wrapper_paths(home_dir);
    let solstone = read_wrapper(&wrapper_paths.solstone);
    let journal = read_wrapper(&wrapper_paths.journal);
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
    let artifacts = solstone
        .into_iter()
        .chain(journal)
        .chain(service)
        .collect::<Vec<_>>();
    SetupArtifactEvidence {
        artifacts: classify(Ok(artifacts), expected_namespace),
        wrapper_guards,
        service_guard,
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
    if artifacts
        .iter()
        .all(|artifact| matches!(artifact, PresentArtifact::Unguarded))
    {
        return ArtifactBindingEvidence::LegacyUnguarded;
    }
    let guards = artifacts
        .iter()
        .map(|artifact| match artifact {
            PresentArtifact::Guarded(fields) => Some(fields),
            PresentArtifact::Unguarded => None,
        })
        .collect::<Option<Vec<_>>>();
    let Some(guards) = guards else {
        return ArtifactBindingEvidence::Ambiguous;
    };
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
}
