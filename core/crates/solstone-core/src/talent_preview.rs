// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use solstone_core_talent_cli::preview::{PreviewRequest, PromptPreview, PromptPreviewer};
use solstone_core_talent_cli::{CliRun, run_cli};
use solstone_core_talent_runtime::ExecutionContext;
use solstone_core_talent_runtime::assemble::assemble_prompt_preview;
use solstone_core_talent_runtime::prepare::RuntimePaths;

pub(crate) struct CorePromptPreviewer {
    paths: RuntimePaths,
}

impl PromptPreviewer for CorePromptPreviewer {
    fn preview(&self, journal_root: &Path, request: &PreviewRequest) -> PromptPreview {
        assemble_prompt_preview(
            request,
            &self.paths,
            &ExecutionContext {
                journal: journal_root.to_path_buf(),
            },
        )
    }
}

fn paths_from_cli_roots(talent_root: &Path, apps_root: &Path) -> RuntimePaths {
    RuntimePaths {
        talent_root: talent_root.to_path_buf(),
        apps_root: apps_root.to_path_buf(),
        templates_dir: talent_root
            .parent()
            .map(|parent| parent.join("think/templates"))
            .unwrap_or_else(|| PathBuf::from("think/templates")),
    }
}

pub(crate) fn run_talent_cli(
    args: &[OsString],
    talent_root: &Path,
    apps_root: &Path,
    journal_root: &Path,
    now: SystemTime,
) -> CliRun {
    let previewer = CorePromptPreviewer {
        paths: paths_from_cli_roots(talent_root, apps_root),
    };
    run_cli(args, talent_root, apps_root, journal_root, now, &previewer)
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::fs;
    use std::path::Path;
    use std::time::{Duration, UNIX_EPOCH};

    use super::*;

    fn root() -> tempfile::TempDir {
        let root = tempfile::tempdir().expect("root");
        fs::create_dir_all(root.path().join("talent")).expect("talent");
        fs::create_dir_all(root.path().join("apps")).expect("apps");
        fs::create_dir_all(root.path().join("think/templates")).expect("templates");
        fs::create_dir_all(root.path().join("health")).expect("health");
        root
    }

    fn run(root: &tempfile::TempDir, args: &[&str]) -> CliRun {
        run_talent_cli(
            &args.iter().map(OsString::from).collect::<Vec<_>>(),
            &root.path().join("talent"),
            &root.path().join("apps"),
            root.path(),
            UNIX_EPOCH + Duration::from_secs(1_000),
        )
    }

    fn write_talent(root: &tempfile::TempDir, name: &str, contents: &str) {
        fs::write(
            root.path().join("talent").join(format!("{name}.md")),
            contents,
        )
        .expect("talent");
    }

    fn snapshot(root: &Path) -> BTreeMap<PathBuf, Vec<u8>> {
        let mut files = BTreeMap::new();
        visit(root, root, &mut files);
        files
    }

    fn visit(root: &Path, current: &Path, files: &mut BTreeMap<PathBuf, Vec<u8>>) {
        for entry in fs::read_dir(current).expect("read dir") {
            let entry = entry.expect("entry");
            let path = entry.path();
            let relative = path.strip_prefix(root).expect("relative").to_path_buf();
            if entry.file_type().expect("type").is_dir() {
                visit(root, &path, files);
            } else {
                files.insert(relative, fs::read(&path).expect("read"));
            }
        }
    }

    #[test]
    fn criterion_1_preview_assembled_no_hook_talent_uses_live_join() {
        let root = root();
        write_talent(
            &root,
            "plain",
            "{\n\"type\": \"generate\"\n}\nplain instruction",
        );
        let output = run(&root, &["show", "plain", "--prompt"]);
        assert_eq!(output.exit_code, 0, "{}", output.stderr);
        assert!(output.stdout.starts_with(
            "Preview of what this talent would send. Same assembly a real run uses; the model is not called.\n"
        ));
        assert!(output.stdout.contains("  INSTRUCTION\n"));
        assert!(output.stdout.contains("plain instruction"));
        assert!(!output.stdout.contains("SYSTEM INSTRUCTION"));
        assert!(!output.stdout.contains("tools: "));
        assert!(output.stderr.is_empty());
    }

    #[test]
    fn criterion_2a_preview_join_is_transcript_instruction_prompt() {
        let root = root();
        write_talent(
            &root,
            "joined",
            concat!(
                "{\n",
                "\"type\": \"generate\",\n",
                "\"transcript\": \"TRANSCRIPT-A\",\n",
                "\"prompt\": \"PROMPT-C\"\n",
                "}\n",
                "INSTRUCTION-B"
            ),
        );
        let output = run(&root, &["show", "joined", "--prompt"]);
        assert_eq!(output.exit_code, 0, "{}", output.stderr);
        assert!(
            output
                .stdout
                .contains("\nTRANSCRIPT-A\n\nINSTRUCTION-B\n\nPROMPT-C\n")
        );
    }

    #[test]
    fn criterion_2b_preview_join_uses_messages_including_empty() {
        let root = root();
        write_talent(
            &root,
            "messages",
            concat!(
                "{\n",
                "\"type\": \"generate\",\n",
                "\"messages\": [{\"content\":\"M1\"},{\"content\":\"\"},{\"content\":\"M2\"}],\n",
                "\"transcript\": \"TRANSCRIPT-A\",\n",
                "\"prompt\": \"PROMPT-C\"\n",
                "}\n",
                "INSTRUCTION-B"
            ),
        );
        let output = run(&root, &["show", "messages", "--prompt"]);
        assert_eq!(output.exit_code, 0, "{}", output.stderr);
        assert!(output.stdout.contains("\nM1\n\n\n\nM2\n"));
        assert!(!output.stdout.contains("TRANSCRIPT-A"));
        assert!(!output.stdout.contains("INSTRUCTION-B"));
        assert!(!output.stdout.contains("PROMPT-C"));
    }

    #[test]
    fn criterion_2c_preview_join_empty_is_no_input_provided() {
        let root = root();
        write_talent(
            &root,
            "empty",
            "{\n\"type\": \"generate\",\n\"day\": \"20260101\"\n}\n",
        );
        let output = run(&root, &["show", "empty", "--prompt"]);
        assert_eq!(output.exit_code, 0, "{}", output.stderr);
        assert!(output.stdout.contains("\nNo input provided.\n"));
        assert!(!output.stdout.contains("(empty)"));
    }

    #[test]
    fn criterion_3_no_day_banner_only_when_sources_enabled() {
        let root = root();
        write_talent(
            &root,
            "sense",
            "{\n\"type\": \"generate\",\n\"load\": {\"transcripts\": true}\n}\nbody",
        );
        write_talent(
            &root,
            "steward",
            "{\n\"type\": \"generate\",\n\"load\": {\"transcripts\": false, \"percepts\": false, \"talents\": false}\n}\nbody",
        );
        let with_sources = run(&root, &["show", "sense", "--prompt"]);
        assert_eq!(with_sources.exit_code, 0, "{}", with_sources.stderr);
        assert!(with_sources.stdout.contains(
            "No day given, so this preview has no day's recordings in it. Pass --day YYYYMMDD to include them.\n"
        ));
        let without = run(&root, &["show", "steward", "--prompt"]);
        assert_eq!(without.exit_code, 0, "{}", without.stderr);
        assert!(!without.stdout.contains("No day given"));
    }

    #[test]
    fn criterion_1_day_with_sources_runs_prepare() {
        let root = root();
        write_talent(
            &root,
            "sense",
            "{\n\"type\": \"generate\",\n\"load\": {\"transcripts\": true}\n}\nbody that compose-only would print",
        );
        let output = run(&root, &["show", "sense", "--prompt", "--day", "20260101"]);
        assert_eq!(output.exit_code, 1, "{}", output.stderr);
        assert_eq!(output.stdout, "This talent would not run: no_input\n");
        assert!(!output.stdout.contains("body that compose-only would print"));
    }

    #[test]
    fn criterion_6_preview_pulse_reads_previous_without_writes() {
        let root = root();
        write_talent(
            &root,
            "pulse",
            "{\n\"type\": \"generate\",\n\"hook\": {\"pre\": \"pulse\"}\n}\n$previous_pulse",
        );
        let before = snapshot(root.path());
        let output = run(&root, &["show", "pulse", "--prompt"]);
        assert_eq!(output.exit_code, 0, "{}", output.stderr);
        assert!(output.stdout.contains("(none - first run)"));
        let after = snapshot(root.path());
        assert_eq!(before, after);
    }

    #[test]
    fn criterion_8_post_only_talent_assembles_without_post_hook() {
        let root = root();
        write_talent(
            &root,
            "work",
            "{\n\"type\": \"generate\",\n\"hook\": {\"post\": \"story\"}\n}\nwork body",
        );
        let output = run(&root, &["show", "work", "--prompt"]);
        assert_eq!(output.exit_code, 0, "{}", output.stderr);
        assert!(output.stdout.contains("  INSTRUCTION\n"));
        assert!(output.stdout.contains("work body"));
    }

    #[test]
    fn criterion_9_unported_pre_is_unavailable() {
        let root = root();
        write_talent(
            &root,
            "unknown",
            "{\n\"type\": \"generate\",\n\"hook\": {\"pre\": \"not_a_real_hook\"}\n}\nbody",
        );
        let output = run(&root, &["show", "unknown", "--prompt"]);
        assert_eq!(output.exit_code, 1);
        assert_eq!(
            output.stdout,
            "Prompt preview cannot assemble this talent: a required pre-step is not available.\n"
        );
        assert!(output.stderr.is_empty());
    }

    #[test]
    fn criterion_10_prepare_failure_is_failed() {
        let root = root();
        write_talent(&root, "broken", "{\n\"type\": \"generate\"\n}\nbody");
        let output = run(
            &root,
            &["show", "broken", "--prompt", "--segment", "090000_60"],
        );
        assert_eq!(output.exit_code, 1);
        assert!(output.stdout.is_empty());
        assert_eq!(
            output.stderr,
            "Invalid config: 'segment' or 'span' requires 'day'\n"
        );
    }

    #[test]
    fn criterion_5_preview_steward_leaves_health_unwritten() {
        let root = root();
        write_talent(
            &root,
            "steward",
            "{\n\"type\": \"generate\",\n\"hook\": {\"pre\": \"steward\"}\n}\n$health_state",
        );
        let output = run(&root, &["show", "steward", "--prompt"]);
        assert_eq!(output.exit_code, 0, "{}", output.stderr);
        assert!(!root.path().join("identity/health.md").exists());
        assert!(output.stdout.contains("  INSTRUCTION\n"));
        assert!(
            !output.stdout.contains("$health_state"),
            "steward preview must substitute $health_state, got:\n{}",
            output.stdout
        );
    }

    #[test]
    fn criterion_7_preview_speaker_attribution_is_read_only() {
        let root = root();
        write_talent(
            &root,
            "speaker_attribution",
            "{\n\"type\": \"generate\",\n\"hook\": {\"pre\": \"speaker_attribution\"}\n}\nbody",
        );
        let output = run(
            &root,
            &[
                "show",
                "speaker_attribution",
                "--prompt",
                "--day",
                "20260101",
                "--segment",
                "090000_300",
            ],
        );
        assert_eq!(output.exit_code, 1, "{}", output.stderr);
        // A read-only preview never creates the segment, so the segment really is
        // absent and the preview now says so. It used to report `no_embeddings`,
        // naming a cause the code had not reached.
        assert_eq!(output.stdout, "This talent would not run: no_segment\n");
        assert!(
            !root
                .path()
                .join("chronicle/20260101/main/090000_300")
                .exists()
        );
    }

    #[test]
    fn criterion_9_4_preview_of_writing_pres_leaves_journal_identical_except_steward_lock() {
        let root = root();
        write_talent(
            &root,
            "steward",
            "{\n\"type\": \"generate\",\n\"hook\": {\"pre\": \"steward\"}\n}\n$health_state",
        );
        write_talent(
            &root,
            "speaker_attribution",
            "{\n\"type\": \"generate\",\n\"hook\": {\"pre\": \"speaker_attribution\"}\n}\nbody",
        );
        let before = snapshot(root.path());
        let steward = run(&root, &["show", "steward", "--prompt"]);
        assert_eq!(steward.exit_code, 0, "{}", steward.stderr);
        let speakers = run(
            &root,
            &[
                "show",
                "speaker_attribution",
                "--prompt",
                "--day",
                "20260101",
                "--segment",
                "090000_300",
            ],
        );
        assert_eq!(speakers.exit_code, 1, "{}", speakers.stderr);
        let mut after = snapshot(root.path());
        after.remove(Path::new("health/.steward.lock"));
        assert_eq!(before, after);
    }
}
