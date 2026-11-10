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
        "this journal isn't connected to an app you've registered with Oura yet. register one, then \
         run `solstone journal importer --connect oura` at a terminal and paste in its client ID and \
         client secret. how: {HOW}\n"
    )
}

/// Ask for the app's client ID and secret. `None` when either is left empty or
/// the terminal closes.
pub(crate) fn ask_oura_app(input: &mut dyn BufRead, output: &mut dyn Write) -> Option<OuraApp> {
    let _ = write!(
        output,
        "this journal isn't connected to an app you've registered with Oura yet. to connect one:\n\
         \x20 1. sign in at https://cloud.ouraring.com/oauth/applications and create an app\n\
         \x20 2. add the redirect URI http://localhost:8765/callback\n\
         \x20 3. paste the app's client ID and client secret below\n\
         they're kept in this journal's settings and used only to connect to Oura. nothing is \
         saved until Oura says yes. how: {HOW}\n\n"
    );
    let client_id = answer(input, output, "client ID: ")?;
    let client_secret = answer(input, output, "client secret: ")?;
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
    let words = match error.stage() {
        "authorization_timeout" => {
            "Oura's sign-in didn't finish within five minutes, so nothing was saved. run it again \
             to retry."
        }
        "authorization_declined" => "Oura didn't connect, so nothing was saved.",
        "authorization_exchange" => {
            "Oura didn't accept the connection, so nothing was saved. check the client secret on \
             your app's page at Oura and run it again."
        }
        "authorization_callback_unavailable" => {
            "Oura's sign-in comes back to this computer on port 8765, and something else is using \
             it. close that and run it again."
        }
        "authorization_browser" => {
            "a browser couldn't be opened here. run this at the journal's computer, not over a \
             remote session."
        }
        _ => return format!("{error}\n"),
    };
    format!("{words}\n")
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
