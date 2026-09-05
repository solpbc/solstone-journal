// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Native owner-facing `journal identity` operations.
//!
//! ## Intentional reference divergences
//!
//! Native help uses a stable plain-text layout instead of Typer/Rich's boxed,
//! terminal-width-dependent rendering.
//!
//! `-h` is accepted consistently with the other native owner-facing verbs,
//! although it is not present in the captured reference help.
//!
//! A malformed `--day` is rejected with `journal identity briefing` usage and
//! exit 2 instead of reaching Python's uncaught `ValueError` traceback.
//!
//! Unknown options, missing option values, and attached boolean spellings use
//! this binary's shared `error: invalid arguments` diagnostic rather than
//! Click's token-specific error. This follows the navigate, export, transfer,
//! and observer parser precedent.
//!
//! Click accepts bundled short boolean repeats such as `-ww`; native partner
//! parsing accepts only exact `-w` and rejects bundled repeats.
//!
//! Python translates universal newlines and splits identity text with
//! `splitlines()`, which also breaks on `\v`, `\f`, `\x1c`–`\x1e`, `\x85`,
//! U+2028, U+2029, and a lone `\r`; native preserves line separators and uses
//! `str::lines()`. A lone-CR heading is therefore stripped by the reference and
//! retained natively. This follows the existing line-splitting divergence
//! documented in `solstone-core-format/src/briefing.rs:17`.

use std::fs;
use std::io::{self, Read};
use std::path::Path;
use std::process::ExitCode;

use solstone_core_cli::{
    IdentityBriefingOptions, IdentityCommand, IdentityHealthOptions, IdentityPartnerOptions,
};
use solstone_core_format::briefing::{load_morning_briefing, most_recent_morning_briefing_day};
use solstone_core_format::content::render_morning_briefing_text;
use solstone_core_identity::{ensure_identity_directory, update_identity_section, write_identity};
use solstone_core_transcribe::require_solstone;

use crate::{EXIT_TEMPFAIL, eprint_journal_path_error, resolve_process_journal_path};

#[cfg(unix)]
mod steward;

const SPECIES_PREAMBLE: &str =
    include_str!("../../../fixtures/native-identity/species-preamble.md");
const EXIT_FAILURE: u8 = 1;

pub(crate) fn run(command: IdentityCommand) -> ExitCode {
    let journal = match resolve_process_journal_path() {
        Ok(journal) => journal,
        Err(error) => {
            eprint_journal_path_error(error);
            return ExitCode::from(EXIT_TEMPFAIL);
        }
    };
    if let Err(error) = require_solstone(&journal.path) {
        if let Some(message) = error.message() {
            eprintln!("{message}");
        }
        return ExitCode::from(error.exit_code() as u8);
    }

    match command {
        IdentityCommand::Hydrate => hydrate(&journal.path),
        IdentityCommand::Partner(options) => partner(&journal.path, options),
        IdentityCommand::Health(options) => health(&journal.path, options),
        IdentityCommand::Briefing(options) => briefing(&journal.path, options),
    }
}

fn hydrate(journal: &Path) -> ExitCode {
    let partner_path = journal.join("identity").join("partner.md");
    let partner = match fs::read_to_string(&partner_path) {
        Ok(content) => strip_section_heading("partner", content.trim()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => "(not present)".to_owned(),
        Err(error) => return print_io_error(&partner_path, error),
    };
    print!("# species\n\n{SPECIES_PREAMBLE}\n\n# partner\n\n{partner}\n");
    ExitCode::SUCCESS
}

fn partner(journal: &Path, options: IdentityPartnerOptions) -> ExitCode {
    let identity_dir = match ensure_identity_directory(journal) {
        Ok(identity_dir) => identity_dir,
        Err(error) => return print_identity_error(error),
    };
    let partner_path = identity_dir.join("partner.md");

    if options
        .update_section
        .as_deref()
        .is_some_and(|heading| !heading.is_empty())
    {
        let heading = options.update_section.as_deref().expect("truthy heading");
        let content = match resolve_content(options.value) {
            Ok(content) => content,
            Err(exit) => return exit,
        };
        match update_identity_section(
            journal,
            "partner.md",
            heading,
            content.trim(),
            "journal identity partner --update-section <heading>",
            "manual section update",
        ) {
            Ok(true) => {
                println!("Updated ## {heading} in partner.md.");
                ExitCode::SUCCESS
            }
            Ok(false) => {
                eprintln!("Error: section '## {heading}' not found.");
                ExitCode::from(EXIT_FAILURE)
            }
            Err(error) => print_identity_error(error),
        }
    } else if options.write {
        let content = match resolve_content(options.value) {
            Ok(content) => content,
            Err(exit) => return exit,
        };
        match write_identity(
            journal,
            "partner.md",
            "journal identity partner --write",
            "replace",
            None,
            &content,
            "manual replace",
        ) {
            Ok(()) => {
                println!("partner.md updated.");
                ExitCode::SUCCESS
            }
            Err(error) => print_identity_error(error),
        }
    } else {
        print_file_with_echo(&partner_path, "partner.md")
    }
}

fn health(journal: &Path, options: IdentityHealthOptions) -> ExitCode {
    let identity_dir = match ensure_identity_directory(journal) {
        Ok(identity_dir) => identity_dir,
        Err(error) => return print_identity_error(error),
    };
    if options.refresh {
        #[cfg(unix)]
        {
            return steward::refresh(journal, &identity_dir.join("health.md"));
        }
        #[cfg(not(unix))]
        {
            return ExitCode::from(crate::EXIT_UNAVAILABLE);
        }
    }
    print_file_with_echo(&identity_dir.join("health.md"), "health.md")
}

fn briefing(journal: &Path, options: IdentityBriefingOptions) -> ExitCode {
    let day = options
        .day
        .or_else(|| most_recent_morning_briefing_day(journal));
    let Some(day) = day else {
        eprintln!("No briefing found.");
        return ExitCode::from(EXIT_FAILURE);
    };
    let Some(briefing) = load_morning_briefing(journal, &day) else {
        eprintln!("No briefing found.");
        return ExitCode::from(EXIT_FAILURE);
    };
    println!("{}", render_morning_briefing_text(&briefing));
    ExitCode::SUCCESS
}

fn strip_section_heading(stem: &str, text: &str) -> String {
    let lines = text.lines().collect::<Vec<_>>();
    let Some(first) = lines.first() else {
        return text.to_owned();
    };
    let Some(after_hash) = first.strip_prefix('#') else {
        return text.to_owned();
    };
    if !after_hash.chars().next().is_some_and(char::is_whitespace)
        || !after_hash.trim().eq_ignore_ascii_case(stem)
    {
        return text.to_owned();
    }
    let start = if lines.get(1).is_some_and(|line| line.trim().is_empty()) {
        2
    } else {
        1
    };
    lines[start..].join("\n")
}

fn resolve_content(value: Option<String>) -> Result<String, ExitCode> {
    let content = match value {
        Some(value) => value,
        None => {
            let mut content = String::new();
            if io::stdin().read_to_string(&mut content).is_err() {
                eprintln!("Error: no content provided.");
                return Err(ExitCode::from(EXIT_FAILURE));
            }
            content
        }
    };
    if content.trim().is_empty() {
        eprintln!("Error: no content provided.");
        return Err(ExitCode::from(EXIT_FAILURE));
    }
    Ok(content)
}

fn print_file_with_echo(path: &Path, label: &str) -> ExitCode {
    match fs::read_to_string(path) {
        Ok(content) => {
            println!("{content}");
            ExitCode::SUCCESS
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            eprintln!("{label} not found.");
            ExitCode::from(EXIT_FAILURE)
        }
        Err(error) => print_io_error(path, error),
    }
}

fn print_identity_error(error: solstone_core_identity::IdentityError) -> ExitCode {
    eprintln!("Error: {error}");
    ExitCode::from(EXIT_FAILURE)
}

fn print_io_error(path: &Path, error: io::Error) -> ExitCode {
    eprintln!("Error: {}: {error}", path.display());
    ExitCode::from(EXIT_FAILURE)
}

#[cfg(test)]
mod tests {
    use super::strip_section_heading;

    #[test]
    fn heading_strip_matches_the_reference_shape() {
        assert_eq!(
            strip_section_heading("partner", "# partner\n\nbody"),
            "body"
        );
        assert_eq!(strip_section_heading("partner", "# PARTNER\nbody"), "body");
        assert_eq!(
            strip_section_heading("partner", "## partner\nbody"),
            "## partner\nbody"
        );
    }
}
