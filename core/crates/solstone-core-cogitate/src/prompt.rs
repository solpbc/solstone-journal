// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use crate::{COGITATE_DIAGNOSTIC_PREAMBLE, COGITATE_JOURNAL_COMMANDS, COGITATE_RUNTIME_PREAMBLE};

const READ_SCOPE_HINT: &str = "Limit filesystem reads to today's segment dir unless the task explicitly requires broader history. If you need broader scope, state what and why in your reasoning.";

/// Return the model-visible command-routing hint for the native `solstone` tool.
pub fn cogitate_sol_tool_hint(tool_name: &str) -> String {
    let families = COGITATE_JOURNAL_COMMANDS.join(", ");
    format!(
        "When the instructions tell you to run `solstone ...` or approved `journal ...` commands, invoke them through the `{tool_name}` tool. Normal journal access uses `solstone` / `solstone call ...`; the approved direct `journal` families are {families} and must be run unprefixed as `journal <family> ...`. Examples: `{tool_name}(command=\"solstone call activities list\")`, `{tool_name}(command=\"journal identity partner\")`. Do not invent or call a tool literally named `solstone`, and do not rewrite approved `journal` commands as `solstone call journal ...`."
    )
}

/// Compose the provider system instruction from native cogitate request inputs.
pub fn compose_system_instruction(
    diagnostic: bool,
    talent_instruction: Option<&str>,
    sol_tool_name: Option<&str>,
    read_scope_configured: bool,
) -> Option<String> {
    let talent_instruction = talent_instruction.filter(|value| !value.is_empty());
    let sol_tool_name = sol_tool_name.filter(|value| !value.is_empty());
    let mut system_instruction = if diagnostic {
        join_parts([
            Some(
                COGITATE_DIAGNOSTIC_PREAMBLE
                    .trim_end_matches('\n')
                    .to_owned(),
            ),
            talent_instruction.map(ToOwned::to_owned),
        ])
    } else if let Some(tool_name) = sol_tool_name {
        join_parts([
            Some(COGITATE_RUNTIME_PREAMBLE.trim_end_matches('\n').to_owned()),
            talent_instruction.map(ToOwned::to_owned),
            Some(cogitate_sol_tool_hint(tool_name)),
        ])
    } else {
        talent_instruction.map(ToOwned::to_owned)
    };

    if read_scope_configured && !diagnostic {
        system_instruction = Some(match system_instruction {
            Some(instruction) => format!("{instruction}\n\n{READ_SCOPE_HINT}"),
            None => READ_SCOPE_HINT.to_owned(),
        });
    }
    system_instruction
}

fn join_parts<const N: usize>(parts: [Option<String>; N]) -> Option<String> {
    let parts = parts.into_iter().flatten().collect::<Vec<_>>();
    (!parts.is_empty()).then(|| parts.join("\n\n"))
}

#[cfg(test)]
mod tests {
    use serde_json::Value;

    use super::*;
    use crate::oracle::{self, PromptPartFixture};

    #[test]
    fn prompt_assembly_vectors_match_frozen_and_live_comparands() {
        let fixture = oracle::fixture();
        assert_eq!(fixture.prompt_assembly.len(), 9);
        for vector in &fixture.prompt_assembly {
            let expected = &vector.expect.system_instruction;
            if let Some(order) = &expected.order {
                assert_eq!(
                    order,
                    &expected
                        .parts
                        .iter()
                        .map(|part| part.role.clone())
                        .collect::<Vec<_>>(),
                    "{} order",
                    vector.id
                );
            }
            let frozen = reassemble(&expected.parts, &expected.separator, |role| match role {
                "runtime_preamble" => Some(fixture.preambles.runtime.text.clone()),
                "diagnostic_preamble" => Some(fixture.preambles.diagnostic.text.clone()),
                _ => None,
            });
            let live = reassemble(&expected.parts, &expected.separator, |role| match role {
                "runtime_preamble" => Some(COGITATE_RUNTIME_PREAMBLE.to_owned()),
                "diagnostic_preamble" => Some(COGITATE_DIAGNOSTIC_PREAMBLE.to_owned()),
                _ => None,
            });
            if let (Some(length), Some(digest)) = (expected.byte_length, &expected.sha256) {
                // Prompt-vector digests use today's runtime preamble. The fixture retains
                // its older runtime preamble solely for the recorded divergence ledger
                // verified by preambles.rs.
                assert_eq!(live.len(), length, "{} live length", vector.id);
                assert_eq!(
                    oracle::sha256_hex(live.as_bytes()),
                    *digest,
                    "{} live digest",
                    vector.id
                );
                if expected
                    .parts
                    .iter()
                    .any(|part| part.role == "runtime_preamble")
                {
                    assert_ne!(frozen, live, "{} ledgered runtime preamble", vector.id);
                } else {
                    assert_eq!(frozen, live, "{} frozen preamble", vector.id);
                }
            }

            let prompt_body = prompt_body(&vector.config);
            if let Some(expected_body) = &vector.expect.prompt_body {
                assert_eq!(&prompt_body, expected_body, "{} prompt body", vector.id);
            }
            if vector.id == "sol_tool_hint" {
                let hint = expected.parts.first().and_then(|part| part.text.as_deref());
                assert_eq!(
                    hint,
                    vector
                        .sol_tool_name
                        .as_deref()
                        .map(cogitate_sol_tool_hint)
                        .as_deref(),
                );
                continue;
            }

            assert_eq!(
                compose_system_instruction(
                    vector.diagnostic,
                    vector
                        .config
                        .get("system_instruction")
                        .and_then(Value::as_str),
                    vector.sol_tool_name.as_deref(),
                    vector
                        .config
                        .get("read_scope")
                        .and_then(Value::as_array)
                        .is_some_and(|scope| !scope.is_empty()),
                )
                .as_deref(),
                Some(live.as_str()),
                "{} live composition",
                vector.id
            );
        }
    }

    #[test]
    fn diagnostic_flip_replaces_runtime_and_suppresses_scope_hint() {
        let instruction = compose_system_instruction(false, Some("RULES"), Some("solstone"), true)
            .expect("runtime instruction");
        let diagnostic = compose_system_instruction(true, Some("RULES"), Some("solstone"), true)
            .expect("diagnostic instruction");
        assert!(instruction.contains(COGITATE_RUNTIME_PREAMBLE.trim_end_matches('\n')));
        assert!(instruction.contains(READ_SCOPE_HINT));
        assert!(diagnostic.contains(COGITATE_DIAGNOSTIC_PREAMBLE.trim_end_matches('\n')));
        assert!(!diagnostic.contains("When the instructions tell you"));
        assert!(!diagnostic.contains(READ_SCOPE_HINT));
    }

    #[test]
    fn removing_talent_instruction_removes_only_its_part() {
        let with_instruction =
            compose_system_instruction(false, Some("RULES"), Some("solstone"), false)
                .expect("instruction");
        let without_instruction =
            compose_system_instruction(false, None, Some("solstone"), false).expect("instruction");
        assert!(with_instruction.contains("RULES"));
        assert!(!without_instruction.contains("RULES"));
        assert!(without_instruction.contains(COGITATE_RUNTIME_PREAMBLE.trim_end_matches('\n')));
        assert!(without_instruction.contains("When the instructions tell you"));
    }

    #[test]
    fn removing_sol_tool_name_drops_both_preamble_and_hint() {
        assert_eq!(
            compose_system_instruction(false, Some("TALENT RULES"), None, false),
            Some("TALENT RULES".to_owned())
        );
    }

    #[test]
    fn toggling_read_scope_changes_only_the_non_diagnostic_suffix() {
        let without_scope =
            compose_system_instruction(false, Some("RULES"), Some("solstone"), false)
                .expect("instruction");
        let with_scope = compose_system_instruction(false, Some("RULES"), Some("solstone"), true)
            .expect("instruction");
        assert_eq!(with_scope, format!("{without_scope}\n\n{READ_SCOPE_HINT}"));
        assert_eq!(
            compose_system_instruction(true, Some("RULES"), Some("solstone"), false),
            compose_system_instruction(true, Some("RULES"), Some("solstone"), true),
        );
    }

    fn reassemble(
        parts: &[PromptPartFixture],
        separator: &str,
        substitute: impl Fn(&str) -> Option<String>,
    ) -> String {
        parts
            .iter()
            .map(|part| {
                part.text
                    .clone()
                    .or_else(|| {
                        substitute(&part.role).map(|text| text.trim_end_matches('\n').to_owned())
                    })
                    .unwrap_or_else(|| panic!("unknown null prompt part role: {}", part.role))
            })
            .collect::<Vec<_>>()
            .join(separator)
    }

    fn prompt_body(config: &serde_json::Map<String, Value>) -> String {
        ["transcript", "extra_context", "user_instruction", "prompt"]
            .into_iter()
            .filter_map(|key| config.get(key).and_then(Value::as_str))
            .filter(|value| !value.is_empty())
            .collect::<Vec<_>>()
            .join("\n\n")
    }
}
