// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Asking an owner at the terminal for the Oura app they registered with Oura,
//! and the words a connect ends with.

use std::io::{BufRead, Write};

use solstone_core_import::{BodyIngestError, OuraApp};

const HOW: &str =
    "https://github.com/solpbc/solstone-journal/blob/main/docs/health_imports.md#connecting-oura";

/// What a connect with nobody at the terminal says when there is no app yet.
pub(crate) fn app_needed_words() -> String {
    format!(
        "{}\n",
        solstone_core_cli_copy::display("journal.cli.importer.oura.app.needed", [("how", &HOW)])
    )
}

/// Ask for the app's client ID and secret. `None` when either is left empty or
/// the terminal closes.
pub(crate) fn ask_oura_app(input: &mut dyn BufRead, output: &mut dyn Write) -> Option<OuraApp> {
    let locale = solstone_core_cli_copy::cli_locale();
    let _ = write!(
        output,
        "{}\n{}\n{}\n{}\n{}\n\n",
        solstone_core_cli_copy::render_for(
            locale,
            "journal.cli.importer.oura.app.prompt.intro",
            &[]
        ),
        solstone_core_cli_copy::display_for(
            locale,
            "journal.cli.importer.oura.app.prompt.step1",
            [("indent", &"  ")],
        ),
        solstone_core_cli_copy::display_for(
            locale,
            "journal.cli.importer.oura.app.prompt.step2",
            [("indent", &"  ")],
        ),
        solstone_core_cli_copy::display_for(
            locale,
            "journal.cli.importer.oura.app.prompt.step3",
            [("indent", &"  ")],
        ),
        solstone_core_cli_copy::display_for(
            locale,
            "journal.cli.importer.oura.app.prompt.note",
            [("how", &HOW)],
        ),
    );
    let client_id_prompt = format!(
        "{} ",
        solstone_core_cli_copy::render_for(
            locale,
            "journal.cli.importer.oura.app.prompt.client.id",
            &[],
        )
    );
    let client_secret_prompt = format!(
        "{} ",
        solstone_core_cli_copy::render_for(
            locale,
            "journal.cli.importer.oura.app.prompt.client.secret",
            &[],
        )
    );
    let client_id = answer(input, output, &client_id_prompt)?;
    let client_secret = answer(input, output, &client_secret_prompt)?;
    Some(OuraApp {
        client_id,
        client_secret,
    })
}

fn answer(input: &mut dyn BufRead, output: &mut dyn Write, prompt: &str) -> Option<String> {
    let _ = write!(output, "{prompt}");
    let _ = output.flush();
    let mut line = String::new();
    input.read_line(&mut line).ok()?;
    let line = line.trim();
    (!line.is_empty()).then(|| line.to_owned())
}

/// What an owner reads when a connect stops.
pub(crate) fn connect_failed_words(error: &BodyIngestError) -> String {
    let id = match error.stage() {
        "authorization_timeout" => "journal.cli.importer.oura.connect.failed.timeout",
        "authorization_declined" => "journal.cli.importer.oura.connect.failed.declined",
        "authorization_exchange" => "journal.cli.importer.oura.connect.failed.exchange",
        "authorization_callback_unavailable" => "journal.cli.importer.oura.connect.failed.callback",
        "authorization_browser" => "journal.cli.importer.oura.connect.failed.browser",
        _ => return format!("{error}\n"),
    };
    format!("{}\n", solstone_core_cli_copy::text(id))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_owner_types_the_app_and_it_is_handed_back_trimmed() {
        let mut input = "  my-client \nmy-secret\n".as_bytes();
        let mut output = Vec::new();
        let app = ask_oura_app(&mut input, &mut output).expect("app");
        assert_eq!(app.client_id, "my-client");
        assert_eq!(app.client_secret, "my-secret");
        let shown = String::from_utf8(output).unwrap();
        assert!(shown.contains("http://localhost:8765/callback"));
        assert!(shown.ends_with("client secret: "));
    }

    #[test]
    fn an_empty_answer_or_a_closed_terminal_gives_no_app() {
        for typed in ["\nsecret\n", "client\n\n", "client\n", ""] {
            let mut input = typed.as_bytes();
            assert!(
                ask_oura_app(&mut input, &mut Vec::new()).is_none(),
                "{typed:?}"
            );
        }
    }
}
