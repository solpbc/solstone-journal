// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Every talent prompt carries the owner-register rule.
//!
//! The rule is what keeps generated text out of the retired vocabulary: no
//! "capture" in any form, and no sentence in which the software watches,
//! observes or records the owner — in any voice, including the passive, which
//! is how "no other actions were recorded" reached a live narrative on
//! 2026-09-06 after the rule had already shipped. A prompt that drifts away
//! from the clause fails silently: the model simply writes the banned phrasing
//! again, and nothing in the build notices.

use std::fs;
use std::path::{Path, PathBuf};

/// Fragments of the register rule that every prompt must carry verbatim.
///
/// These are asserted individually rather than as one long sentence so that a
/// prompt which legitimately rewraps or extends the paragraph still passes,
/// while a prompt that drops a clause does not.
const REQUIRED_CLAUSES: &[&str] = &[
    r#"Never write "capture" in any form"#,
    "never say the software watches, observes, records, monitors, tracks, listens, sees, hears or surveils",
    "in any voice, including the passive",
    "attach every claim to what the journal holds",
];

/// Prompts exempted from the register rule.
///
/// A prompt belongs here only when nothing it produces can reach the owner —
/// a genuinely machine-only prompt whose entire output is consumed by code.
/// Every name listed must exist, so an exemption cannot outlive its file.
const EXEMPT: &[&str] = &[];

/// The prompts are known to be a substantial set; a walk that returns far
/// fewer than this found the wrong directory rather than a shrunken bundle.
const MINIMUM_PROMPTS: usize = 16;

fn repository_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(3)
        .expect("core crate has repository parent")
        .to_path_buf()
}

fn talent_prompt_dir() -> PathBuf {
    repository_root().join("core/payload/solstone/talent")
}

/// Top-level `*.md` files in the talent directory, sorted by file name.
///
/// Nested directories hold reference bundles and shared patterns rather than
/// prompts, so the walk is deliberately one level deep.
fn talent_prompts() -> Vec<PathBuf> {
    let directory = talent_prompt_dir();
    let mut prompts: Vec<PathBuf> = fs::read_dir(&directory)
        .expect("talent prompt directory is readable")
        .map(|entry| entry.expect("talent prompt entry is readable").path())
        .filter(|path| path.is_file() && path.extension().is_some_and(|ext| ext == "md"))
        .collect();
    prompts.sort();
    prompts
}

fn file_name(path: &Path) -> String {
    path.file_name()
        .expect("talent prompt has a file name")
        .to_string_lossy()
        .into_owned()
}

/// Collapse every run of whitespace to one space.
///
/// Several prompts hard-wrap their rules, so a clause can straddle a newline
/// and a line-oriented search would miss it.
fn flatten(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

#[test]
fn every_talent_prompt_carries_the_register_rule() {
    let prompts = talent_prompts();
    assert!(
        prompts.len() >= MINIMUM_PROMPTS,
        "found only {} talent prompts under {}; expected at least {MINIMUM_PROMPTS}",
        prompts.len(),
        talent_prompt_dir().display()
    );

    let mut offenders = Vec::new();
    for path in &prompts {
        let name = file_name(path);
        if EXEMPT.contains(&name.as_str()) {
            continue;
        }
        let text = flatten(&fs::read_to_string(path).expect("talent prompt is readable"));
        let missing: Vec<&str> = REQUIRED_CLAUSES
            .iter()
            .copied()
            .filter(|clause| !text.contains(clause))
            .collect();
        if !missing.is_empty() {
            offenders.push(format!("{name}: missing {missing:?}"));
        }
    }

    assert!(
        offenders.is_empty(),
        "every talent prompt must carry the owner-register rule; {} of {} do not:\n  {}",
        offenders.len(),
        prompts.len(),
        offenders.join("\n  ")
    );
}

#[test]
fn register_rule_exemptions_name_existing_prompts() {
    let names: Vec<String> = talent_prompts()
        .iter()
        .map(|path| file_name(path))
        .collect();
    let stale: Vec<&&str> = EXEMPT
        .iter()
        .filter(|name| !names.contains(&(*name).to_string()))
        .collect();
    assert!(
        stale.is_empty(),
        "register-rule exemptions name prompts that no longer exist: {stale:?}"
    );
}
