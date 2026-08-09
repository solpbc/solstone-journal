// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use crate::access_tiers::{AccessTierError, COGITATE_ACCESS_TIERS, capabilities_for_access_tier};
use crate::preambles::COGITATE_JOURNAL_COMMANDS;

const SHELL_COMPOSITION_DENY: &str = "policy_deny: shell composition is not available; run one `sol` or approved `journal` command per call with no pipes, redirects, chaining, or command substitution";
const EMPTY_COMMAND_DENY: &str = "policy_deny: empty command";
const RESTRICTED_COMMAND_DENY: &str =
    "policy_deny: run_shell_command restricted to sol or approved journal invocations";
const SUPPORT_SEND_VERBS: [&str; 7] = [
    "create",
    "reply",
    "attach",
    "feedback",
    "close",
    "resolved",
    "still-need-help",
];

/// The policy result for one command invocation.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CommandDecision {
    pub allowed: bool,
    pub reason: String,
    pub argv: Option<Vec<String>>,
}

/// Classify a parsed-command-only cogitate tool request.
pub fn classify_command(
    command: &str,
    access_tier: &str,
    outbound_approval: Option<&str>,
) -> Result<CommandDecision, AccessTierError> {
    let submit_allowed = capabilities_for_access_tier(access_tier)?.submit;

    if shell_syntax_violation(command) {
        return Ok(deny(SHELL_COMPOSITION_DENY));
    }
    let argv = match posix_split(command) {
        Ok(argv) => argv,
        Err(()) => return Ok(deny(SHELL_COMPOSITION_DENY)),
    };
    if argv.is_empty() {
        return Ok(deny(EMPTY_COMMAND_DENY));
    }
    if argv.get(0..3).is_some_and(|prefix| {
        prefix
            .iter()
            .map(String::as_str)
            .eq(["sol", "call", "journal"])
    }) && argv.len() >= 4
        && COGITATE_JOURNAL_COMMANDS.contains(&argv[3].as_str())
    {
        return Ok(deny(&hybrid_journal_deny(&argv)));
    }
    if argv[0] == "journal" && argv.len() >= 2 && bare_journal_repair_prefix(&argv[1]).is_some() {
        return Ok(deny(&bare_journal_repair_deny(&argv)));
    }
    if !(argv[0] == "sol"
        || (argv[0] == "journal"
            && argv.len() >= 2
            && COGITATE_JOURNAL_COMMANDS.contains(&argv[1].as_str())))
    {
        return Ok(deny(RESTRICTED_COMMAND_DENY));
    }

    if let Some(send_verb) = support_send_verb(&argv) {
        if !submit_allowed {
            let required = submit_tiers().join(" or ");
            return Ok(deny(&format!(
                "policy_deny: 'sol call support {send_verb}' requires access_tier '{required}'; this run is '{access_tier}'"
            )));
        }
        if outbound_approval.is_none_or(str::is_empty) {
            return Ok(deny(&format!(
                "policy_deny: 'sol call support {send_verb}' requires a per-send owner approval; this run was not launched with one"
            )));
        }
    }

    Ok(CommandDecision {
        allowed: true,
        reason: "ok".to_owned(),
        argv: Some(argv),
    })
}

fn deny(reason: &str) -> CommandDecision {
    CommandDecision {
        allowed: false,
        reason: reason.to_owned(),
        argv: None,
    }
}

fn shell_syntax_violation(command: &str) -> bool {
    if command.contains("$(") || command.contains('`') {
        return true;
    }
    if command.contains('\n') || command.contains('\r') {
        return true;
    }
    let chars: Vec<char> = command.chars().collect();
    let mut index = 0;
    let mut quote = None;
    while index < chars.len() {
        let character = chars[index];
        match quote {
            Some('\'') => {
                if character == '\'' {
                    quote = None;
                }
            }
            Some('"') => {
                if character == '\\' && index + 1 < chars.len() {
                    index += 2;
                    continue;
                }
                if character == '"' {
                    quote = None;
                }
            }
            None => {
                if character == '\\' && index + 1 < chars.len() {
                    index += 2;
                    continue;
                }
                if matches!(character, '\'' | '"') {
                    quote = Some(character);
                } else if matches!(character, '(' | ')' | ';' | '<' | '>' | '|' | '&') {
                    return true;
                }
            }
            Some(_) => unreachable!("quote is only single or double"),
        }
        index += 1;
    }
    quote.is_some()
}

fn posix_split(command: &str) -> Result<Vec<String>, ()> {
    let chars: Vec<char> = command.chars().collect();
    let mut argv = Vec::new();
    let mut token = String::new();
    let mut token_started = false;
    let mut quote = None;
    let mut index = 0;

    while index < chars.len() {
        let character = chars[index];
        match quote {
            Some('\'') => {
                if character == '\'' {
                    quote = None;
                } else {
                    token.push(character);
                }
            }
            Some('"') => {
                if character == '"' {
                    quote = None;
                } else if character == '\\' {
                    let next = *chars.get(index + 1).ok_or(())?;
                    if matches!(next, '"' | '\\') {
                        token.push(next);
                    } else {
                        token.push('\\');
                        token.push(next);
                    }
                    index += 1;
                } else {
                    token.push(character);
                }
            }
            None => {
                if character.is_whitespace() {
                    if token_started {
                        argv.push(std::mem::take(&mut token));
                        token_started = false;
                    }
                } else if character == '\\' {
                    token.push(*chars.get(index + 1).ok_or(())?);
                    token_started = true;
                    index += 1;
                } else if matches!(character, '\'' | '"') {
                    quote = Some(character);
                    token_started = true;
                } else {
                    token.push(character);
                    token_started = true;
                }
            }
            Some(_) => unreachable!("quote is only single or double"),
        }
        index += 1;
    }
    if quote.is_some() {
        return Err(());
    }
    if token_started {
        argv.push(token);
    }
    Ok(argv)
}

fn hybrid_journal_deny(argv: &[String]) -> String {
    let family = &argv[3];
    let repair = shlex_join(
        std::iter::once("journal".to_owned())
            .chain(argv[3..].iter().cloned())
            .collect(),
    );
    format!(
        "policy_deny: `sol call journal {family}` is not a `sol call` verb; `{family}` is an approved host command family — run it directly as `{repair}`"
    )
}

fn bare_journal_repair_deny(argv: &[String]) -> String {
    let family = &argv[1];
    let prefix = bare_journal_repair_prefix(family).expect("known repair family");
    let repair = shlex_join(
        prefix
            .iter()
            .map(|value| (*value).to_owned())
            .chain(argv[2..].iter().cloned())
            .collect(),
    );
    format!("policy_deny: `journal {family}` is not a host command; run `{repair}` instead")
}

fn bare_journal_repair_prefix(family: &str) -> Option<[&str; 4]> {
    match family {
        "search" => Some(["sol", "call", "journal", "search"]),
        "facet" => Some(["sol", "call", "journal", "facet"]),
        _ => None,
    }
}

fn support_send_verb(argv: &[String]) -> Option<&str> {
    for index in 0..argv.len().saturating_sub(3) {
        if argv[index..index + 3]
            .iter()
            .map(String::as_str)
            .eq(["sol", "call", "support"])
        {
            let verb = argv[index + 3].as_str();
            if SUPPORT_SEND_VERBS.contains(&verb) {
                return Some(verb);
            }
        }
    }
    None
}

fn submit_tiers() -> Vec<&'static str> {
    COGITATE_ACCESS_TIERS
        .iter()
        .copied()
        .filter(|tier| capabilities_for_access_tier(tier).is_ok_and(|caps| caps.submit))
        .collect()
}

fn shlex_join(argv: Vec<String>) -> String {
    argv.iter()
        .map(|value| shlex_quote(value))
        .collect::<Vec<_>>()
        .join(" ")
}

fn shlex_quote(value: &str) -> String {
    if value.is_empty() {
        return "''".to_owned();
    }
    if value.chars().all(is_shlex_safe) {
        return value.to_owned();
    }
    format!("'{}'", value.replace('\'', "'\"'\"'"))
}

fn is_shlex_safe(character: char) -> bool {
    character.is_ascii_alphanumeric()
        || matches!(
            character,
            '_' | '@' | '%' | '+' | '=' | ':' | ',' | '.' | '/' | '-'
        )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::oracle;

    #[test]
    fn policy_command_vectors_match_the_oracle() {
        let fixture = oracle::fixture();
        assert_eq!(fixture.policy_commands.len(), 62);
        for vector in &fixture.policy_commands {
            let decision = classify_command(
                &vector.command,
                &vector.access_tier,
                vector.outbound_approval.as_deref(),
            )
            .expect("policy fixture uses known tiers");
            assert_eq!(
                decision.allowed, vector.expect.allowed,
                "{} allowed",
                vector.id
            );
            assert_eq!(
                decision.reason, vector.expect.reason,
                "{} reason",
                vector.id
            );
            assert_eq!(decision.argv, vector.expect.argv, "{} argv", vector.id);
        }
    }

    #[test]
    fn short_support_command_is_not_scanned_for_a_send_verb() {
        let decision = classify_command("sol call support", "normal", None).expect("known tier");
        assert!(
            decision.allowed,
            "a three-token argv has range(0) in Python"
        );
    }
}
