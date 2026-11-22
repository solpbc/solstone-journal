// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! `journal talent passages`: an activity's source passages.

use std::path::Path;

use crate::CliRun;
use crate::args::{PassagesOptions, ShowOptions};
use crate::preview::{PassageRun, PreviewRequest, PromptPreview, PromptPreviewer};
use crate::show::{failure, render_preview_refusal};

pub(crate) fn run(
    journal_root: &Path,
    options: &PassagesOptions,
    previewer: &dyn PromptPreviewer,
) -> CliRun {
    let request = PreviewRequest {
        name: options.name.clone(),
        day: options.day.clone(),
        segment: None,
        facet: options.facet.clone(),
        activity: options.activity.clone(),
    };
    let locale = crate::output_locale(options.json);
    match previewer.passages_in((!options.json).then_some(locale), journal_root, &request) {
        // An incomplete document is still printed whole, so the units that
        // have no passages say why; the exit status says it is incomplete.
        PassageRun::Extracted {
            document,
            readable,
            complete,
        } => CliRun {
            stdout: if options.json {
                format!("{document}\n")
            } else {
                readable
            },
            stderr: if complete {
                String::new()
            } else {
                format!(
                    "{}\n",
                    solstone_core_cli_copy::render_for(
                        locale,
                        "journal.cli.talent.passages.incomplete",
                        &[]
                    )
                )
            },
            exit_code: if complete { 0 } else { 1 },
        },
        PassageRun::NotPrepared(PromptPreview::Refused(refusal)) => render_preview_refusal(
            &ShowOptions {
                name: options.name.clone(),
                json: options.json,
                day: options.day.clone(),
                facet: options.facet.clone(),
                activity: options.activity.clone(),
                ..ShowOptions::default()
            },
            refusal,
        ),
        PassageRun::NotPrepared(PromptPreview::WouldNotRun { reason }) => {
            failure(solstone_core_cli_copy::display_for(
                locale,
                "journal.cli.talent.preview.skipped",
                [("reason", &reason)],
            ))
        }
        PassageRun::NotPrepared(PromptPreview::Failed { error }) => failure(error),
        PassageRun::NotPrepared(other) => failure(solstone_core_cli_copy::display_for(
            locale,
            "journal.cli.talent.passages.unavailable",
            [("detail", &(format!("{other:?}")))],
        )),
    }
}

#[cfg(test)]
mod tests {
    use std::ffi::OsString;
    use std::time::UNIX_EPOCH;

    use super::*;
    use crate::preview::PromptPreviewRefusal;

    struct Stub(PassageRun);

    impl PromptPreviewer for Stub {
        fn preview(&self, _: &Path, _: &PreviewRequest) -> PromptPreview {
            unreachable!("passages never assembles a prompt")
        }

        fn passages(&self, _: &Path, request: &PreviewRequest) -> PassageRun {
            assert_eq!(request.activity.as_deref(), Some("a1"));
            assert_eq!(request.segment, None);
            self.0.clone()
        }
    }

    fn run(args: &[&str], stub: Stub) -> CliRun {
        let root = tempfile::tempdir().unwrap();
        crate::run_cli(
            &args.iter().map(OsString::from).collect::<Vec<_>>(),
            root.path(),
            root.path(),
            root.path(),
            UNIX_EPOCH,
            &stub,
        )
    }

    const ARGS: [&str; 8] = [
        "passages",
        "conversation",
        "--day",
        "20260101",
        "--facet=work",
        "--activity",
        "a1",
        "--json",
    ];

    fn extracted(complete: bool) -> Stub {
        Stub(PassageRun::Extracted {
            document: r#"{"schema":"solstone-source-passages-v1"}"#.to_owned(),
            readable: "Verbatim from source:\nAri asks.\n".to_owned(),
            complete,
        })
    }

    #[test]
    fn json_prints_the_document_and_an_incomplete_one_exits_nonzero() {
        let output = run(&ARGS, extracted(true));
        assert_eq!(output.exit_code, 0);
        assert_eq!(
            output.stdout,
            "{\"schema\":\"solstone-source-passages-v1\"}\n"
        );
        let output = run(&ARGS, extracted(false));
        assert_eq!(output.exit_code, 1);
        assert!(output.stdout.contains("solstone-source-passages-v1"));
        assert!(!output.stderr.is_empty());
        let readable = run(&ARGS[..7], extracted(true));
        assert_eq!(readable.stdout, "Verbatim from source:\nAri asks.\n");
    }

    #[test]
    fn every_activity_coordinate_is_required_and_a_refusal_is_reported() {
        let missing = run(
            &["passages", "conversation", "--day", "20260101"],
            extracted(true),
        );
        assert_eq!(missing.exit_code, 2);
        assert!(missing.stderr.contains("--facet, --activity"));
        let refused = run(
            &ARGS,
            Stub(PassageRun::NotPrepared(PromptPreview::Refused(
                PromptPreviewRefusal {
                    code: "activity_not_found".to_owned(),
                    segment: None,
                    recovery: "Choose an activity ID.".to_owned(),
                },
            ))),
        );
        assert_eq!(refused.exit_code, 1);
        assert!(refused.stderr.contains("activity_not_found"));
    }
}
