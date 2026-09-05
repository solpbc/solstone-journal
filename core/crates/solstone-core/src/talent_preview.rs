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

    use serde_json::{Value, json};

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

    fn write_facet(root: &tempfile::TempDir, facet: &str) {
        let directory = root.path().join("facets").join(facet);
        fs::create_dir_all(directory.join("activities")).expect("facet activities");
        fs::write(
            directory.join("facet.json"),
            format!(r#"{{"name":"{facet}","description":"test facet"}}"#),
        )
        .expect("facet declaration");
    }

    fn write_activity_rows(root: &tempfile::TempDir, facet: &str, day: &str, rows: &[Value]) {
        let path = root
            .path()
            .join("facets")
            .join(facet)
            .join("activities")
            .join(format!("{day}.jsonl"));
        let contents = rows
            .iter()
            .map(Value::to_string)
            .collect::<Vec<_>>()
            .join("\n");
        fs::write(path, format!("{contents}\n")).expect("activity rows");
    }

    fn write_segment(root: &tempfile::TempDir, day: &str, segment: &str, marker: &str) {
        let directory = root.path().join("chronicle").join(day).join(segment);
        fs::create_dir_all(directory.join("talents")).expect("segment");
        fs::write(
            directory.join("imported.md"),
            format!("transcript-{marker}"),
        )
        .expect("transcript");
        fs::write(
            directory.join("screen.jsonl"),
            format!(r#"{{"timestamp":0,"content":{{"marker":"percept-{marker}"}}}}"#),
        )
        .expect("percept");
        fs::write(
            directory.join("talents/sense.md"),
            format!("sense-{marker}"),
        )
        .expect("sense");
        fs::write(
            directory.join("talents/other.md"),
            format!("other-{marker}"),
        )
        .expect("other talent");
    }

    fn activity_talent(kind: &str, talents: &str) -> String {
        format!(
            concat!(
                "{{\n",
                "\"type\":\"generate\",\n",
                "\"schedule\":\"activity\",\n",
                "\"priority\":1,\n",
                "\"output\":\"md\",\n",
                "\"activities\":[\"{kind}\"],\n",
                "\"load\":{{\"transcripts\":true,\"percepts\":true,\"talents\":{talents}}}\n",
                "}}\n",
                "activity body"
            ),
            kind = kind,
            talents = talents,
        )
    }

    fn copy_shipped_activity_talents(root: &tempfile::TempDir) {
        let payload = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../payload/solstone/talent");
        for name in ["participation", "conversation", "event", "work"] {
            fs::copy(
                payload.join(format!("{name}.md")),
                root.path().join("talent").join(format!("{name}.md")),
            )
            .expect("shipped activity talent");
        }
        for schema in ["participation.schema.json", "story.schema.json"] {
            fs::copy(
                payload.join(schema),
                root.path().join("talent").join(schema),
            )
            .expect("shipped activity schema");
        }
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

    #[test]
    fn activity_preview_selects_the_exact_record_span_and_day_preview_stays_unchanged() {
        let root = root();
        write_facet(&root, "work");
        write_talent(&root, "activity_probe", &activity_talent("*", "false"));
        write_segment(&root, "20260101", "090000_60", "A-only");
        write_segment(&root, "20260101", "100000_60", "B-only");
        write_segment(&root, "20260102", "110000_60", "C-other-day");
        // Deliberately store B first: selection may not mean first row.
        write_activity_rows(
            &root,
            "work",
            "20260101",
            &[
                json!({"id":"B","activity":"work","segments":["100000_60"]}),
                json!({"id":"A","activity":"work","segments":["090000_60"]}),
            ],
        );
        write_activity_rows(
            &root,
            "work",
            "20260102",
            &[json!({"id":"A","activity":"work","segments":["110000_60"]})],
        );
        let before = snapshot(root.path());

        for (activity, own, other) in [("A", "A-only", "B-only"), ("B", "B-only", "A-only")] {
            let output = run(
                &root,
                &[
                    "show",
                    "activity_probe",
                    "--prompt",
                    "--full",
                    "--day",
                    "20260101",
                    "--facet",
                    "work",
                    "--activity",
                    activity,
                ],
            );
            assert_eq!(output.exit_code, 0, "{}", output.stderr);
            assert!(output.stdout.contains(own), "{}", output.stdout);
            assert!(!output.stdout.contains(other), "{}", output.stdout);
            assert!(!output.stdout.contains("C-other-day"), "{}", output.stdout);
        }

        let day = run(
            &root,
            &[
                "show",
                "activity_probe",
                "--prompt",
                "--full",
                "--day",
                "20260101",
                "--facet",
                "work",
            ],
        );
        assert_eq!(day.exit_code, 0, "{}", day.stderr);
        assert!(day.stdout.contains("A-only"));
        assert!(day.stdout.contains("B-only"));
        assert!(!day.stdout.contains("C-other-day"));
        assert_eq!(snapshot(root.path()), before, "preview mutated the journal");
    }

    #[test]
    fn activity_preview_preserves_the_four_shipped_source_matrices() {
        let root = root();
        write_facet(&root, "work");
        write_segment(&root, "20260101", "090000_60", "matrix");
        write_activity_rows(
            &root,
            "work",
            "20260101",
            &[
                json!({"id":"participation-a","activity":"meeting","segments":["090000_60"]}),
                json!({"id":"conversation-a","activity":"meeting","segments":["090000_60"]}),
                json!({"id":"event-a","activity":"event","segments":["090000_60"]}),
                json!({"id":"work-a","activity":"coding","segments":["090000_60"]}),
            ],
        );
        copy_shipped_activity_talents(&root);

        for talent in ["participation", "conversation", "event", "work"] {
            let id = format!("{talent}-a");
            let output = run(
                &root,
                &[
                    "show",
                    talent,
                    "--prompt",
                    "--full",
                    "--day",
                    "20260101",
                    "--facet",
                    "work",
                    "--activity",
                    &id,
                ],
            );
            assert_eq!(output.exit_code, 0, "{talent}: {}", output.stderr);
            assert!(output.stdout.contains("transcript-matrix"), "{talent}");
            assert!(output.stdout.contains("percept-matrix"), "{talent}");
            if talent == "participation" {
                assert!(output.stdout.contains("sense-matrix"));
            } else {
                assert!(!output.stdout.contains("sense-matrix"), "{talent}");
            }
            assert!(!output.stdout.contains("other-matrix"), "{talent}");
        }
    }

    #[test]
    fn activity_preview_json_failures_are_closed_and_read_only() {
        let root = root();
        write_facet(&root, "work");
        write_talent(&root, "activity_probe", &activity_talent("work", "false"));
        write_segment(&root, "20260101", "090000_60", "kept");

        let cases = [
            (
                "missing",
                json!({"id":"other","activity":"work","segments":["090000_60"]}),
                "activity_not_found",
            ),
            (
                "empty",
                json!({"id":"empty","activity":"work","segments":[]}),
                "activity_span_empty",
            ),
            (
                "invalid",
                json!({"id":"invalid","activity":"work","segments":["090000_60",7]}),
                "activity_span_invalid",
            ),
            (
                "unavailable",
                json!({"id":"unavailable","activity":"work","segments":["missing_60"]}),
                "activity_segment_unavailable",
            ),
        ];
        for (id, row, code) in cases {
            write_activity_rows(&root, "work", "20260101", &[row]);
            let before = snapshot(root.path());
            let output = run(
                &root,
                &[
                    "show",
                    "activity_probe",
                    "--prompt",
                    "--json",
                    "--day",
                    "20260101",
                    "--facet",
                    "work",
                    "--activity",
                    id,
                ],
            );
            assert_eq!(output.exit_code, 1, "{code}");
            assert!(output.stdout.is_empty());
            let error: Value = serde_json::from_str(output.stderr.trim()).expect("JSON error");
            assert_eq!(error["code"], code);
            assert_eq!(error["day"], "20260101");
            assert_eq!(error["facet"], "work");
            assert_eq!(error["activity"], id);
            assert!(
                error["recovery"]
                    .as_str()
                    .is_some_and(|value| !value.is_empty())
            );
            assert_eq!(snapshot(root.path()), before, "{code} mutated the journal");
        }

        let duplicates = [
            json!({"id":"duplicate","activity":"work","segments":["090000_60"],"marker":"first"}),
            json!({"id":"duplicate","activity":"work","segments":["090000_60"],"marker":"second"}),
        ];
        let mut errors = Vec::new();
        for rows in [
            duplicates.clone(),
            [duplicates[1].clone(), duplicates[0].clone()],
        ] {
            write_activity_rows(&root, "work", "20260101", &rows);
            let output = run(
                &root,
                &[
                    "show",
                    "activity_probe",
                    "--prompt",
                    "--json",
                    "--day",
                    "20260101",
                    "--facet",
                    "work",
                    "--activity",
                    "duplicate",
                ],
            );
            assert_eq!(output.exit_code, 1);
            assert!(output.stdout.is_empty());
            assert_eq!(
                serde_json::from_str::<Value>(output.stderr.trim()).unwrap()["code"],
                "activity_ambiguous"
            );
            errors.push(output.stderr);
        }
        assert_eq!(errors[0], errors[1]);
    }

    #[test]
    fn activity_preview_reports_activity_store_io_failures() {
        let root = root();
        write_facet(&root, "work");
        write_talent(&root, "activity_probe", &activity_talent("work", "false"));
        fs::create_dir(root.path().join("facets/work/activities/20260101.jsonl"))
            .expect("unreadable activity fixture");
        let before = snapshot(root.path());

        let output = run(
            &root,
            &[
                "show",
                "activity_probe",
                "--prompt",
                "--json",
                "--day",
                "20260101",
                "--facet",
                "work",
                "--activity",
                "unavailable",
            ],
        );

        assert_eq!(output.exit_code, 1);
        assert!(output.stdout.is_empty());
        assert_eq!(
            serde_json::from_str::<Value>(output.stderr.trim()).unwrap()["code"],
            "activity_record_unavailable"
        );
        assert_eq!(snapshot(root.path()), before, "failure mutated the journal");
    }

    #[cfg(unix)]
    #[test]
    fn activity_preview_refuses_a_segment_directory_it_cannot_enumerate() {
        use std::os::unix::fs::PermissionsExt;

        let root = root();
        write_facet(&root, "work");
        write_talent(&root, "activity_probe", &activity_talent("work", "false"));
        write_segment(&root, "20260101", "090000_60", "unreadable");
        write_activity_rows(
            &root,
            "work",
            "20260101",
            &[json!({"id":"A","activity":"work","segments":["090000_60"]})],
        );
        let segment = root.path().join("chronicle/20260101/090000_60");
        let mut permissions = fs::metadata(&segment).unwrap().permissions();
        permissions.set_mode(0o000);
        fs::set_permissions(&segment, permissions).expect("make segment unreadable");

        let output = run(
            &root,
            &[
                "show",
                "activity_probe",
                "--prompt",
                "--json",
                "--day",
                "20260101",
                "--facet",
                "work",
                "--activity",
                "A",
            ],
        );

        let mut permissions = fs::metadata(&segment).unwrap().permissions();
        permissions.set_mode(0o700);
        fs::set_permissions(&segment, permissions).expect("restore segment permissions");
        assert_eq!(output.exit_code, 1);
        assert!(output.stdout.is_empty());
        let error = serde_json::from_str::<Value>(output.stderr.trim()).unwrap();
        assert_eq!(error["code"], "activity_segment_unavailable");
        assert_eq!(error["segment"], "090000_60");
    }

    #[test]
    fn activity_preview_uses_production_validation_and_structures_prepare_failures() {
        let absent = root();
        let absent_output = run(
            &absent,
            &[
                "show",
                "absent_activity",
                "--prompt",
                "--json",
                "--day",
                "20260101",
                "--facet",
                "work",
                "--activity",
                "A",
            ],
        );
        assert_eq!(absent_output.exit_code, 1);
        assert!(absent_output.stdout.is_empty());
        assert_eq!(absent_output.stderr.lines().count(), 1);
        assert_eq!(
            serde_json::from_str::<Value>(absent_output.stderr.trim()).unwrap()["code"],
            "activity_talent_unavailable"
        );

        let invalid = root();
        write_facet(&invalid, "work");
        write_segment(&invalid, "20260101", "090000_60", "invalid-config");
        write_activity_rows(
            &invalid,
            "work",
            "20260101",
            &[json!({"id":"A","activity":"work","segments":["090000_60"]})],
        );
        write_talent(
            &invalid,
            "invalid_activity",
            concat!(
                "{\n",
                "\"type\":\"generate\",\n",
                "\"schedule\":\"activity\",\n",
                "\"activities\":[\"work\"]\n",
                "}\n",
                "invalid activity"
            ),
        );
        let invalid_output = run(
            &invalid,
            &[
                "show",
                "invalid_activity",
                "--prompt",
                "--json",
                "--day",
                "20260101",
                "--facet",
                "work",
                "--activity",
                "A",
            ],
        );
        assert_eq!(invalid_output.exit_code, 1);
        assert!(invalid_output.stdout.is_empty());
        assert_eq!(
            serde_json::from_str::<Value>(invalid_output.stderr.trim()).unwrap()["code"],
            "activity_talent_unavailable"
        );
        assert_eq!(invalid_output.stderr.lines().count(), 1);

        let missing_schema = root();
        write_facet(&missing_schema, "work");
        write_segment(&missing_schema, "20260101", "090000_60", "missing-schema");
        write_activity_rows(
            &missing_schema,
            "work",
            "20260101",
            &[json!({"id":"A","activity":"work","segments":["090000_60"]})],
        );
        write_talent(
            &missing_schema,
            "missing_schema",
            concat!(
                "{\n",
                "\"type\":\"generate\",\n",
                "\"schedule\":\"activity\",\n",
                "\"priority\":1,\n",
                "\"output\":\"json\",\n",
                "\"schema\":\"absent.schema.json\",\n",
                "\"activities\":[\"work\"]\n",
                "}\n",
                "missing schema"
            ),
        );
        let before = snapshot(missing_schema.path());
        let schema_output = run(
            &missing_schema,
            &[
                "show",
                "missing_schema",
                "--prompt",
                "--json",
                "--day",
                "20260101",
                "--facet",
                "work",
                "--activity",
                "A",
            ],
        );
        assert_eq!(schema_output.exit_code, 1);
        assert!(schema_output.stdout.is_empty());
        assert_eq!(
            serde_json::from_str::<Value>(schema_output.stderr.trim()).unwrap()["code"],
            "activity_talent_unavailable"
        );
        assert_eq!(schema_output.stderr.lines().count(), 1);
        assert_eq!(snapshot(missing_schema.path()), before);
    }

    #[test]
    fn activity_preview_routes_malformed_metadata_to_structured_runtime_refusals() {
        for (name, talent) in [
            ("malformed", "{\nnot-json\n}\nbody"),
            (
                "wrong_type",
                concat!(
                    "{\n",
                    "\"type\":7,\n",
                    "\"schedule\":\"activity\",\n",
                    "\"priority\":1,\n",
                    "\"activities\":[\"work\"]\n",
                    "}\n",
                    "body"
                ),
            ),
        ] {
            let root = root();
            write_facet(&root, "work");
            write_segment(&root, "20260101", "090000_60", name);
            write_activity_rows(
                &root,
                "work",
                "20260101",
                &[json!({"id":"A","activity":"work","segments":["090000_60"]})],
            );
            write_talent(&root, name, talent);

            let output = run(
                &root,
                &[
                    "show",
                    name,
                    "--prompt",
                    "--json",
                    "--day",
                    "20260101",
                    "--facet",
                    "work",
                    "--activity",
                    "A",
                ],
            );

            assert_eq!(output.exit_code, 1, "{name}: {}", output.stderr);
            assert!(output.stdout.is_empty());
            assert_eq!(output.stderr.lines().count(), 1);
            assert_eq!(
                serde_json::from_str::<Value>(output.stderr.trim()).unwrap()["code"],
                "activity_talent_unavailable"
            );
        }
    }

    #[test]
    fn cogitate_activity_preview_resolves_activity_before_showing_the_initial_prompt() {
        let root = root();
        write_facet(&root, "work");
        write_segment(&root, "20260101", "090000_60", "cogitate");
        write_activity_rows(
            &root,
            "work",
            "20260101",
            &[json!({"id":"A","activity":"work","segments":["090000_60"]})],
        );
        write_talent(
            &root,
            "activity_cogitate",
            concat!(
                "{\n",
                "\"type\":\"cogitate\",\n",
                "\"schedule\":\"activity\",\n",
                "\"priority\":1,\n",
                "\"activities\":[\"work\"],\n",
                "\"load\":{\"transcripts\":true}\n",
                "}\n",
                "cogitate-activity-body"
            ),
        );

        let missing = run(
            &root,
            &[
                "show",
                "activity_cogitate",
                "--prompt",
                "--json",
                "--day",
                "20260101",
                "--facet",
                "work",
                "--activity",
                "missing",
            ],
        );
        assert_eq!(missing.exit_code, 1);
        assert_eq!(
            serde_json::from_str::<Value>(missing.stderr.trim()).unwrap()["code"],
            "activity_not_found"
        );

        let before = snapshot(root.path());
        let selected = run(
            &root,
            &[
                "show",
                "activity_cogitate",
                "--prompt",
                "--full",
                "--day",
                "20260101",
                "--facet",
                "work",
                "--activity",
                "A",
            ],
        );
        assert_eq!(selected.exit_code, 0, "{}", selected.stderr);
        assert!(
            selected
                .stdout
                .contains("Processing activity 'A' (work) in facet 'work' for 2026-01-01.")
        );
        assert!(!selected.stdout.contains("cogitate-activity-body"));
        assert!(!selected.stdout.contains("transcript-cogitate"));
        assert_eq!(snapshot(root.path()), before);
    }

    #[test]
    fn activity_preview_refuses_structural_and_production_eligibility_mismatches() {
        let root = root();
        write_facet(&root, "work");
        write_segment(&root, "20260101", "090000_60", "eligible");
        write_talent(&root, "activity_probe", &activity_talent("work", "false"));
        write_talent(
            &root,
            "daily_probe",
            "{\n\"type\":\"generate\",\"schedule\":\"daily\",\"priority\":1,\"output\":\"md\"\n}\ndaily",
        );

        let run_error = |args: &[&str]| {
            let output = run(&root, args);
            assert_eq!(output.exit_code, 1, "{}", output.stderr);
            assert!(output.stdout.is_empty());
            serde_json::from_str::<Value>(output.stderr.trim()).expect("JSON error")
        };
        let no_day = run_error(&[
            "show",
            "activity_probe",
            "--prompt",
            "--json",
            "--facet",
            "work",
            "--activity",
            "A",
        ]);
        assert_eq!(no_day["code"], "activity_requires_day");
        let no_facet = run_error(&[
            "show",
            "activity_probe",
            "--prompt",
            "--json",
            "--day",
            "20260101",
            "--activity",
            "A",
        ]);
        assert_eq!(no_facet["code"], "activity_requires_facet");
        let segment_conflict = run_error(&[
            "show",
            "activity_probe",
            "--prompt",
            "--json",
            "--day",
            "20260101",
            "--facet",
            "work",
            "--activity",
            "A",
            "--segment",
            "090000_60",
        ]);
        assert_eq!(segment_conflict["code"], "activity_segment_conflict");
        assert_eq!(segment_conflict["segment"], "090000_60");

        let cases = [
            (
                "daily_probe",
                json!({"id":"daily","activity":"work","segments":["090000_60"]}),
                "daily",
                "activity_schedule_unsupported",
            ),
            (
                "activity_probe",
                json!({"id":"kind","activity":"meeting","segments":["090000_60"]}),
                "kind",
                "activity_kind_unsupported",
            ),
            (
                "activity_probe",
                json!({"id":"synthetic","activity":"work","source":"cogitate","segments":["090000_60"]}),
                "synthetic",
                "activity_synthetic",
            ),
        ];
        for (talent, row, id, code) in cases {
            write_activity_rows(&root, "work", "20260101", &[row]);
            let error = run_error(&[
                "show",
                talent,
                "--prompt",
                "--json",
                "--day",
                "20260101",
                "--facet",
                "work",
                "--activity",
                id,
            ]);
            assert_eq!(error["code"], code);
        }

        write_talent(&root, "work", &activity_talent("reading", "false"));
        write_activity_rows(
            &root,
            "work",
            "20260101",
            &[json!({
                "id":"low",
                "activity":"reading",
                "level_avg":0.39,
                "segments":["090000_60"]
            })],
        );
        let low = run_error(&[
            "show",
            "work",
            "--prompt",
            "--json",
            "--day",
            "20260101",
            "--facet",
            "work",
            "--activity",
            "low",
        ]);
        assert_eq!(low["code"], "low_level_activity");
    }
}
