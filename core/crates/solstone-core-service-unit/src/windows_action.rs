// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! The guarded Task Scheduler action, without shell evaluation.

use std::collections::BTreeMap;

use solstone_core_installation_identity::{
    GuardFields, parse_service_guard_environment, service_guard_environment,
};

const GUARD_FLAGS: [(&str, &str); 4] = [
    (
        "--installation-namespace",
        "SOLSTONE_INSTALLATION_NAMESPACE",
    ),
    ("--installation-id", "SOLSTONE_INSTALLATION_ID"),
    (
        "--installation-generation",
        "SOLSTONE_INSTALLATION_GENERATION",
    ),
    (
        "--installation-journal-token",
        "SOLSTONE_INSTALLATION_JOURNAL_TOKEN",
    ),
];

/// Parsed task action. A guard is evidence to compare with the loaded binding;
/// these fields do not grant process or generation-borrowing authority.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WindowsServiceAction {
    pub port: u16,
    pub journal: String,
    pub guard: GuardFields,
}

impl WindowsServiceAction {
    /// Argument values passed to the real journal facade, excluding argv[0].
    pub fn arguments(&self) -> Result<Vec<String>, &'static str> {
        if self.port == 0 || self.journal.is_empty() || !xml_text_is_valid(&self.journal) {
            return Err("invalid Windows service port or journal path");
        }
        let environment = service_guard_environment(&self.guard);
        let mut arguments = vec![
            "supervisor".to_owned(),
            self.port.to_string(),
            "--journal".to_owned(),
            self.journal.clone(),
            "--windows-service".to_owned(),
        ];
        for (flag, key) in GUARD_FLAGS {
            arguments.push(flag.to_owned());
            arguments.push(environment[key].clone());
        }
        Ok(arguments)
    }

    /// Parse only the installed task's exact grammar. Ordinary supervisor
    /// invocation remains governed by the existing CLI grammar.
    pub fn parse(arguments: &[String]) -> Result<Self, &'static str> {
        if arguments.len() != 13
            || arguments[0] != "supervisor"
            || arguments[2] != "--journal"
            || arguments[4] != "--windows-service"
        {
            return Err("invalid Windows service action shape");
        }
        let port = arguments[1]
            .parse::<u16>()
            .map_err(|_| "invalid Windows service port")?;
        if port == 0 || port.to_string() != arguments[1] {
            return Err("invalid Windows service port");
        }
        if arguments[3].is_empty() || !xml_text_is_valid(&arguments[3]) {
            return Err("invalid Windows service journal path");
        }
        let mut environment = BTreeMap::new();
        for ((flag, key), pair) in GUARD_FLAGS.into_iter().zip(arguments[5..].chunks_exact(2)) {
            if pair[0] != flag {
                return Err("missing, duplicate, or misplaced Windows service guard");
            }
            environment.insert(key.to_owned(), pair[1].clone());
        }
        let guard = parse_service_guard_environment(&environment)
            .map_err(|_| "malformed Windows service guard")?
            .ok_or("missing Windows service guard")?;
        Ok(Self {
            port,
            journal: arguments[3].clone(),
            guard,
        })
    }
}

pub(crate) fn xml_text_is_valid(value: &str) -> bool {
    value.chars().all(|ch| matches!(ch, '\t' | '\n' | '\r' | '\u{20}'..='\u{d7ff}' | '\u{e000}'..='\u{fffd}' | '\u{10000}'..='\u{10ffff}'))
}

/// Encode argument values for the Windows CRT, with no executable or shell.
/// Always quoting makes trailing backslashes and empty values explicit.
pub fn encode_windows_task_arguments(arguments: &[String]) -> Result<String, &'static str> {
    let mut encoded = Vec::with_capacity(arguments.len());
    for argument in arguments {
        if !xml_text_is_valid(argument) {
            return Err("Windows task argument is not XML text");
        }
        let mut value = String::from("\"");
        let mut slashes = 0;
        for ch in argument.chars() {
            if ch == '\\' {
                slashes += 1;
                continue;
            }
            value.extend(std::iter::repeat_n(
                '\\',
                if ch == '"' { 2 * slashes + 1 } else { slashes },
            ));
            value.push(ch);
            slashes = 0;
        }
        value.extend(std::iter::repeat_n('\\', 2 * slashes));
        value.push('"');
        encoded.push(value);
    }
    Ok(encoded.join(" "))
}

/// Decode scheduler readback using the CRT's non-argv[0] rules. Unterminated
/// quoting is refused instead of repaired during ownership classification.
pub fn decode_windows_task_arguments(value: &str) -> Result<Vec<String>, &'static str> {
    if !xml_text_is_valid(value) {
        return Err("Windows task arguments are not XML text");
    }
    let chars: Vec<_> = value.chars().collect();
    let mut index = 0;
    let mut result = Vec::new();
    while index < chars.len() {
        while index < chars.len() && matches!(chars[index], ' ' | '\t') {
            index += 1;
        }
        if index == chars.len() {
            break;
        }
        let mut argument = String::new();
        let mut quoted = false;
        loop {
            let mut slashes = 0;
            while index < chars.len() && chars[index] == '\\' {
                slashes += 1;
                index += 1;
            }
            if index < chars.len() && chars[index] == '"' {
                argument.extend(std::iter::repeat_n('\\', slashes / 2));
                if slashes % 2 == 1 {
                    argument.push('"');
                    index += 1;
                } else if quoted && chars.get(index + 1) == Some(&'"') {
                    argument.push('"');
                    index += 2;
                } else {
                    quoted = !quoted;
                    index += 1;
                }
                continue;
            }
            argument.extend(std::iter::repeat_n('\\', slashes));
            if index == chars.len() || (!quoted && matches!(chars[index], ' ' | '\t')) {
                break;
            }
            argument.push(chars[index]);
            index += 1;
        }
        if quoted {
            return Err("Windows task arguments contain unterminated quoting");
        }
        result.push(argument);
    }
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    use solstone_core_installation_identity::{Generation, InstallationId, NamespaceName};

    fn action() -> WindowsServiceAction {
        WindowsServiceAction {
            port: 6123,
            journal: "C:\\Users\\Zoë\\Journal & notes\\".to_owned(),
            guard: GuardFields {
                namespace: NamespaceName::parse(
                    "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef",
                )
                .unwrap(),
                id: InstallationId::parse("0123456789abcdef0123456789abcdef").unwrap(),
                generation: Generation::new(7).unwrap(),
                journal_token: solstone_core_installation_identity::journal_token_from_path(
                    std::path::Path::new(if cfg!(windows) {
                        "C:\\journal"
                    } else {
                        "/journal"
                    }),
                )
                .unwrap(),
            },
        }
    }

    #[test]
    fn guarded_action_round_trips_nondefault_port_and_unicode_path() {
        let expected = action();
        let argv = expected.arguments().unwrap();
        assert_eq!(argv[1], "6123");
        let encoded = encode_windows_task_arguments(&argv).unwrap();
        assert_eq!(
            WindowsServiceAction::parse(&decode_windows_task_arguments(&encoded).unwrap()).unwrap(),
            expected
        );
    }

    #[test]
    fn rejects_missing_partial_duplicate_malformed_and_extra_guard_fields() {
        let argv = action().arguments().unwrap();
        for end in 0..argv.len() {
            assert!(WindowsServiceAction::parse(&argv[..end]).is_err());
        }
        let mut duplicate = argv.clone();
        duplicate[7] = duplicate[5].clone();
        assert!(WindowsServiceAction::parse(&duplicate).is_err());
        let mut malformed = argv.clone();
        malformed[10] = "not-a-generation".to_owned();
        assert!(WindowsServiceAction::parse(&malformed).is_err());
        let mut extra = argv;
        extra.push("--no-convey".to_owned());
        assert!(WindowsServiceAction::parse(&extra).is_err());
    }

    #[test]
    fn quoting_matches_independent_literal_vectors() {
        assert_eq!(
            encode_windows_task_arguments(&[
                "".into(),
                "a b".into(),
                "a\"b".into(),
                "C:\\a b\\".into()
            ])
            .unwrap(),
            "\"\" \"a b\" \"a\\\"b\" \"C:\\a b\\\\\""
        );
        assert_eq!(
            decode_windows_task_arguments(r#"plain "two words" "tail\\" "a\"b""#).unwrap(),
            vec!["plain", "two words", "tail\\", "a\"b"]
        );
        assert!(decode_windows_task_arguments("\"unfinished").is_err());
        assert!(encode_windows_task_arguments(&["bad\0path".into()]).is_err());
    }
}
