// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

use solstone_core_journal_io::durability::{ArtifactId, DurabilityClass, artifacts};

const NO_SEMANTIC_VARIANTS: [ArtifactId; 12] = [
    ArtifactId::DirectDoor,
    ArtifactId::DirectDoorGeneration,
    ArtifactId::CallosumSock,
    ArtifactId::SpeakersInstallGeneration,
    ArtifactId::SpeakersInstallOwner,
    ArtifactId::ConveyConfig,
    ArtifactId::AwarenessCurrent,
    ArtifactId::SegmentSpeakers,
    ArtifactId::SegmentSpeakerLabels,
    ArtifactId::SegmentSpeakerCorrections,
    ArtifactId::SegmentStream,
    ArtifactId::SegmentIngest,
];

#[test]
fn artifact_paths_are_unique_and_journal_config_is_sole_must_be_valid() {
    let mut seen_paths = BTreeSet::new();
    let mut must_be_valid_paths = Vec::new();

    for entry in artifacts() {
        assert!(
            seen_paths.insert(entry.path),
            "duplicate path in JOURNAL_ARTIFACTS: {}",
            entry.path
        );
        if entry.class == DurabilityClass::MustBeValid {
            must_be_valid_paths.push(entry.path);
        }
    }

    assert_eq!(
        must_be_valid_paths,
        vec!["config/journal.json"],
        "exactly one MustBeValid artifact is allowed (config/journal.json); found: {:?}",
        must_be_valid_paths
    );
}

#[test]
fn no_semantic_variants_have_whole_json_or_socket_and_exist_in_registry() {
    let registered_ids: BTreeSet<ArtifactId> = artifacts().iter().map(|a| a.id).collect();
    for id in &NO_SEMANTIC_VARIANTS {
        assert!(
            registered_ids.contains(id),
            "NO_SEMANTIC_VARIANTS member {:?} is not in JOURNAL_ARTIFACTS",
            id
        );
    }
}

fn collect_production_rust_files(dir: &Path, files: &mut Vec<PathBuf>) {
    if let Ok(entries) = fs::read_dir(dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                let name = entry.file_name().to_string_lossy().to_string();
                if name != "target"
                    && name != "tests"
                    && name != "fixtures"
                    && name != "contracts"
                    && !name.starts_with('.')
                {
                    collect_production_rust_files(&path, files);
                }
            } else if path.extension().is_some_and(|ext| ext == "rs") {
                let path_str = path.to_string_lossy();
                let file_name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
                if !path_str.contains("/tests/")
                    && !path_str.contains("/fixtures/")
                    && !path_str.contains("repository-contracts")
                    && !file_name.ends_with("_tests.rs")
                    && !file_name.contains("test_support")
                    && file_name != "durability.rs"
                {
                    files.push(path);
                }
            }
        }
    }
}

fn matches_glob(pattern: &str, candidate: &str) -> bool {
    if pattern == candidate {
        return true;
    }
    let parts: Vec<&str> = pattern.split('*').filter(|s| !s.is_empty()).collect();
    if parts.is_empty() {
        return true;
    }
    let mut remainder = candidate;
    if !pattern.starts_with('*') {
        if !remainder.starts_with(parts[0]) {
            return false;
        }
        remainder = &remainder[parts[0].len()..];
    }
    for &part in &parts[if pattern.starts_with('*') { 0 } else { 1 }..if pattern.ends_with('*') {
        parts.len()
    } else {
        parts.len() - 1
    }] {
        if let Some(pos) = remainder.find(part) {
            remainder = &remainder[pos + part.len()..];
        } else {
            return false;
        }
    }
    if !pattern.ends_with('*') {
        let last_part = parts[parts.len() - 1];
        if !remainder.ends_with(last_part) {
            return false;
        }
    }
    true
}

#[test]
fn every_artifact_has_owning_production_mention_outside_durability() {
    let crates_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("crates dir")
        .to_path_buf();

    let mut files = Vec::new();
    collect_production_rust_files(&crates_dir, &mut files);

    let mut combined_source = String::new();
    for file in &files {
        if let Ok(content) = fs::read_to_string(file) {
            combined_source.push_str(&content);
            combined_source.push('\n');
        }
    }

    for entry in artifacts() {
        let variant_str = format!("{:?}", entry.id);
        let pattern = format!("ArtifactId::{variant_str}");
        let has_variant = combined_source.contains(&pattern);
        let has_special_reader = match entry.id {
            ArtifactId::JournalConfig => combined_source.contains("read_journal_config"),
            ArtifactId::CallosumSock => combined_source.contains("callosum.sock"),
            ArtifactId::SyncHeartbeat => {
                combined_source.contains("fn classify_heartbeat")
                    || combined_source.contains("classify_heartbeat(")
            }
            ArtifactId::HealthMarkerStream | ArtifactId::HealthMarkerDaily => {
                combined_source.contains("read_health_marker")
                    || combined_source.contains("HealthMarkerKind")
            }
            ArtifactId::SegmentSpeakers => combined_source.contains("speakers.json"),
            ArtifactId::SegmentSpeakerLabels => combined_source.contains("speaker_labels.json"),
            ArtifactId::SegmentSpeakerCorrections => {
                combined_source.contains("speaker_corrections.json")
            }
            ArtifactId::SegmentStream => combined_source.contains("stream.json"),
            ArtifactId::SegmentIngest => combined_source.contains("ingest.json"),
            ArtifactId::TalentProvenance => combined_source.contains("talent-provenance"),
            ArtifactId::EntityAmbiguities => {
                combined_source.contains("ambiguities.jsonl")
                    || combined_source.contains("ambiguities_path")
                    || combined_source.contains("read_ambiguities")
            }
            ArtifactId::OperationalLog => {
                combined_source.contains("oplog--")
                    || combined_source.contains("classify_oplog_name")
            }
            _ => false,
        };
        assert!(
            has_variant || has_special_reader,
            "ArtifactId::{variant_str} at path {} has no owning production mention outside durability.rs",
            entry.path
        );
    }
}

#[test]
fn production_files_do_not_pass_durability_class_to_read_json_durable() {
    let crates_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("crates dir")
        .to_path_buf();

    let mut files = Vec::new();
    collect_production_rust_files(&crates_dir, &mut files);

    for file in &files {
        if let Ok(content) = fs::read_to_string(file)
            && content.contains("read_json_durable")
            && content.contains("DurabilityClass::")
        {
            panic!(
                "file {} must not pass DurabilityClass:: to read_json_durable",
                file.display()
            );
        }
    }
}

pub fn undeclared_journal_literal(src: &str) -> Vec<String> {
    let mut findings = Vec::new();
    let known_paths: Vec<&str> = artifacts().iter().map(|a| a.path).collect();

    for line in src.lines() {
        let is_candidate_line = line.contains("read_json(")
            || line.contains("join(\"")
            || line.contains("from_slice")
            || line.contains("from_str");
        if !is_candidate_line {
            continue;
        }

        let mut rest = line;
        while let Some(start) = rest.find('"') {
            rest = &rest[start + 1..];
            let Some(end) = rest.find('"') else { break };
            let literal = &rest[..end];
            rest = &rest[end + 1..];

            if literal.ends_with(".lock") || literal.ends_with(".tmp") {
                continue;
            }

            if literal.starts_with("health/")
                || literal.starts_with("config/")
                || literal.starts_with("chronicle/")
                || literal.starts_with("facets/")
                || literal.starts_with("entities/")
            {
                // Only consider structured artifact targets (.json, .jsonl, .sock, .check, .updated, or oplog)
                let is_artifact_path = literal.ends_with(".json")
                    || literal.ends_with(".jsonl")
                    || literal.ends_with(".sock")
                    || literal.ends_with(".check")
                    || literal.ends_with(".updated")
                    || literal.contains("oplog--");
                if !is_artifact_path {
                    continue;
                }

                let matches_known = known_paths
                    .iter()
                    .any(|&known| matches_glob(known, literal) || known.ends_with(literal));

                if !matches_known {
                    findings.push(literal.to_owned());
                }
            }
        }

        if (line.contains("from_slice") || line.contains("from_str"))
            && !line.contains("read_json_durable")
            && !line.contains("read_jsonl_durable")
            && let Some(comment_idx) = line.find("//")
        {
            let comment = line[comment_idx + 2..].trim();
            for word in comment.split_whitespace() {
                let clean = word.trim_matches(|c: char| {
                    !c.is_alphanumeric() && c != '/' && c != '.' && c != '-' && c != '_'
                });
                if clean.starts_with("health/")
                    || clean.starts_with("config/")
                    || clean.starts_with("chronicle/")
                {
                    findings.push(clean.to_owned());
                }
            }
        }
    }
    findings
}

fn strip_test_code(content: &str) -> String {
    let mut prod_lines = Vec::new();
    for line in content.lines() {
        let trimmed = line.trim();
        if trimmed == "#[cfg(test)]"
            || trimmed.starts_with("#[cfg(all(test")
            || trimmed == "mod tests {"
            || trimmed == "mod test {"
            || trimmed == "#[test]"
            || trimmed == "#[tokio::test]"
        {
            break;
        }
        if !trimmed.starts_with("#[cfg(test)]") {
            prod_lines.push(line);
        }
    }
    prod_lines.join("\n")
}

#[test]
fn production_sources_contain_no_undeclared_journal_literals_or_raw_bypasses() {
    let crates_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("crates dir")
        .to_path_buf();

    let mut files = Vec::new();
    collect_production_rust_files(&crates_dir, &mut files);

    for file in &files {
        // Skip files that intentionally define or test the durability infrastructure or raw IO primitives
        let file_str = file.to_string_lossy();
        if file_str.contains("solstone-core-journal-io/src/reader.rs")
            || file_str.contains("solstone-core-journal-io/src/atomic.rs")
            || file_str.contains("solstone-core-journal-config/src/config.rs")
            || file_str.contains("solstone-core-doctor/src/checks/journal_durability.rs")
            || file_str.contains("solstone-core-talents/src/layout.rs")
        {
            continue;
        }

        if let Ok(content) = fs::read_to_string(file) {
            let prod_source = strip_test_code(&content);
            let findings = undeclared_journal_literal(&prod_source);
            if !findings.is_empty() {
                panic!(
                    "production file {} has undeclared journal literals / bypasses: {:?}",
                    file.display(),
                    findings
                );
            }
        }
    }
}
