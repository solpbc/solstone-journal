// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::collections::BTreeSet;
use std::env;
use std::path::Path;

use solstone_core_cogitate::{COGITATE_ACCESS_TIERS, capabilities_for_access_tier};

use crate::oracle::{fixture, generated_contract_fixture, sha256_hex};
use crate::sol_execution::orchestrate_slot_cycle;
use crate::{
    EMIT_FINAL_TOOL, FINISH_TOOL, KNOWN_TOOL_NAMES, NoopSlotLease, ReadBudget, SlotLease,
    SlotReacquireError, SolCallBudget, ToolName, ToolSpec, bound_tools, format_shell_output, glob,
    resolve_tool_spec, run_command, run_sol_command, truncate_output,
};

#[test]
fn tool_metadata_matches_the_oracle_except_native_finish_text() {
    let fixture = fixture();
    let actual = all_tools();
    assert_eq!(actual.len(), 7);
    for (name, expected) in &fixture.tool_surface.tools {
        let tool = actual
            .iter()
            .copied()
            .find(|tool| tool.name == name)
            .expect("fixture tool has native metadata");
        assert_eq!(tool.name, expected.name, "{name} name");
        if name != "finish" {
            assert_eq!(tool.description, expected.description, "{name} description");
            assert_eq!(
                tool.arguments.len(),
                expected.action_properties.len(),
                "{name} args"
            );
            for argument in tool.arguments {
                let expected_argument = expected
                    .action_properties
                    .get(argument.name)
                    .expect("fixture argument");
                assert_eq!(
                    argument.description, expected_argument.description,
                    "{name} {} description",
                    argument.name
                );
            }
        }
    }
    let finish = &fixture.tool_surface.tools["finish"];
    assert_eq!(FINISH_TOOL.name, finish.name);
    assert!(
        finish
            .action_properties
            .contains_key(FINISH_TOOL.arguments[0].name)
    );
    assert_eq!(
        FINISH_TOOL.arguments,
        &[crate::ToolArgumentSpec {
            name: "message",
            description: "Concise record of what changed, what was found, or that already-persisted work is complete.",
            required: true
        }]
    );
}

#[test]
fn sol_descriptions_derive_the_host_command_vocabulary() {
    let expected = format!(
        "approved `journal` families ({}) directly",
        solstone_core_cogitate::COGITATE_JOURNAL_COMMANDS.join(", ")
    );
    let sol = crate::sol_tool();
    assert!(sol.description.contains(&expected));
    assert!(sol.arguments[0].description.contains(&expected));
}

/// `finish_description` is hand-maintained because no Python source owns the
/// native finish text, so the Python fixture generator cannot emit it this wave.
#[test]
fn hand_maintained_finish_description_is_pinned_by_digest() {
    let expected = &generated_contract_fixture()["finish_description"];
    assert_eq!(expected["text"].as_str(), Some(FINISH_TOOL.description));
    assert_eq!(
        expected["byte_length"].as_u64(),
        Some(FINISH_TOOL.description.len() as u64)
    );
    assert_eq!(
        expected["digest"].as_str(),
        Some(sha256_hex(FINISH_TOOL.description.as_bytes()).as_str())
    );
    assert_eq!(expected["algorithm"].as_str(), Some("sha256"));
    assert_eq!(expected["encoding"].as_str(), Some("utf-8"));
}

#[test]
fn bindings_match_oracle_capabilities_and_are_closed() {
    assert_eq!(KNOWN_TOOL_NAMES.len(), 7);
    let known = KNOWN_TOOL_NAMES.into_iter().collect::<BTreeSet<_>>();
    let fixture = fixture();
    for tier in COGITATE_ACCESS_TIERS {
        let expected = fixture.tool_surface.tier_bindings[tier]
            .model_tools_excluding_finalization
            .iter()
            .map(String::as_str)
            .collect::<BTreeSet<_>>();
        for expects_emit_final in [false, true] {
            let tools = bound_tools(tier, expects_emit_final).expect("known tier");
            let names = tools.iter().map(|tool| tool.name).collect::<BTreeSet<_>>();
            assert!(names.iter().all(|name| known.contains(name)), "{tier}");
            let non_final = names
                .iter()
                .copied()
                .filter(|name| *name != "emit_final" && *name != "finish")
                .collect::<BTreeSet<_>>();
            assert_eq!(non_final, expected, "{tier}");
            assert_eq!(names.contains("emit_final"), expects_emit_final, "{tier}");
            assert_eq!(names.contains("finish"), !expects_emit_final, "{tier}");
            assert!(
                tools
                    .iter()
                    .all(|tool| tool.read_only_hint && !tool.destructive_hint)
            );
        }
    }
}

#[test]
fn access_capabilities_gate_all_four_read_tools() {
    for tier in COGITATE_ACCESS_TIERS {
        let capabilities = capabilities_for_access_tier(tier).expect("known tier");
        let tools = bound_tools(tier, true).expect("known tier");
        let reads = tools
            .iter()
            .filter(|tool| {
                matches!(
                    tool.name,
                    "read_file" | "list_directory" | "glob" | "grep_search"
                )
            })
            .count();
        assert_eq!(reads, usize::from(capabilities.reads) * 4, "{tier}");
    }
}

#[test]
fn read_tool_vocabulary_and_binding_order_match_the_oracle() {
    let fixture = fixture();
    let names = [
        ToolName::ReadFile,
        ToolName::ListDirectory,
        ToolName::Glob,
        ToolName::GrepSearch,
    ]
    .map(ToolName::as_str);
    assert_eq!(fixture.vocabularies.read_tools, names.map(str::to_owned));

    let bound = bound_tools("normal", true).expect("normal is reads-capable");
    let read_names = bound
        .iter()
        .map(|tool| tool.name)
        .filter(|name| {
            matches!(
                *name,
                "read_file" | "list_directory" | "glob" | "grep_search"
            )
        })
        .collect::<Vec<_>>();
    assert_eq!(
        read_names,
        fixture
            .vocabularies
            .read_tools
            .iter()
            .map(String::as_str)
            .collect::<Vec<_>>()
    );
}

#[test]
fn resolve_tool_spec_round_trips_tool_names() {
    for tool in [
        ToolName::ReadFile,
        ToolName::ListDirectory,
        ToolName::Glob,
        ToolName::GrepSearch,
    ] {
        assert_eq!(
            resolve_tool_spec(tool.as_str()).map(|spec| spec.name),
            Some(tool.as_str())
        );
    }
    for name in ["sol", "emit_final", "nope"] {
        assert_eq!(resolve_tool_spec(name), None, "{name}");
    }
}

#[test]
fn sol_execution_vectors_match_the_oracle() {
    let fixture = fixture();
    assert_eq!(fixture.sol_execution.format_shell_output.len(), 9);
    for vector in &fixture.sol_execution.format_shell_output {
        assert_eq!(
            format_shell_output(
                &vector.args.stdout,
                &vector.args.stderr,
                vector.args.returncode,
                vector.args.timed_out,
            ),
            vector.expect,
            "{}",
            vector.id
        );
    }
    assert_eq!(fixture.sol_execution.truncate_output.len(), 6);
    for vector in &fixture.sol_execution.truncate_output {
        let input = truncate_input(&vector.id);
        let actual = truncate_output(&input, vector.cap);
        assert_eq!(actual, vector.expect, "{}", vector.id);
        assert_eq!(
            actual.chars().count(),
            vector.expect_chars,
            "{} chars",
            vector.id
        );
        assert_eq!(actual.len(), vector.expect_bytes, "{} bytes", vector.id);
    }
    assert_eq!(fixture.sol_execution.run_command.len(), 1);
    for vector in &fixture.sol_execution.run_command {
        let actual = run_command(&vector.argv, Path::new(".")).expect("command handling");
        assert_eq!(actual.text, vector.expect.text, "{}", vector.id);
        assert_eq!(actual.is_error, vector.expect.is_error, "{}", vector.id);
    }
}

#[test]
fn sol_and_raw_read_budgets_are_independent() {
    let mut sol_budget = SolCallBudget::new(1);
    let mut slot = NoopSlotLease;
    let journal = temp_journal();
    let first = run_sol_command(
        "sol call activities list",
        "normal",
        None,
        &journal,
        &mut sol_budget,
        &mut slot,
    )
    .expect("known tier");
    let second = run_sol_command(
        "sol call activities list",
        "normal",
        None,
        &journal,
        &mut sol_budget,
        &mut slot,
    )
    .expect("known tier");
    assert_eq!(first.budget_exhausted_event, None);
    assert_eq!(second.budget_exhausted_event.expect("first event").count, 2);
    let third = run_sol_command(
        "sol call activities list",
        "normal",
        None,
        &journal,
        &mut sol_budget,
        &mut slot,
    )
    .expect("known tier");
    assert_eq!(third.budget_exhausted_event, None);
    assert_eq!(sol_budget.count(), 3);

    let mut read_budget = ReadBudget::new(1);
    let options = crate::GlobOptions::default();
    let _ = glob(
        &journal,
        "*.missing",
        "entities",
        &options,
        Some(&mut read_budget),
    );
    let _ = glob(
        &journal,
        "*.missing",
        "entities",
        &options,
        Some(&mut read_budget),
    );
    assert_eq!(read_budget.count(), 1);
    std::fs::remove_dir_all(journal).expect("remove journal");
}

#[test]
fn slot_reacquire_branches_match_the_provider_sequence() {
    let journal = temp_journal();
    let mut budget = SolCallBudget::new(3);
    let mut cancelled = FakeLease::cancelled();
    let completed = orchestrate_slot_cycle(&mut cancelled, || {
        Ok(crate::SolObservation {
            text: "complete".to_owned(),
            is_error: false,
        })
    });
    assert_eq!(completed.observation.text, "complete");
    assert_eq!(cancelled.yields, 1);
    assert_eq!(cancelled.reacquires, 1);

    let mut rejected = FakeLease::ok();
    let denied = run_sol_command(
        "cat secret",
        "normal",
        None,
        &journal,
        &mut budget,
        &mut rejected,
    )
    .expect("known tier");
    assert!(denied.observation.is_error);
    assert_eq!(rejected.yields, 0);

    let mut exhausted_budget = SolCallBudget::new(0);
    let mut exhausted = FakeLease::ok();
    let exhausted_result = run_sol_command(
        "sol call activities list",
        "normal",
        None,
        &journal,
        &mut exhausted_budget,
        &mut exhausted,
    )
    .expect("known tier");
    assert!(exhausted_result.observation.is_error);
    assert_eq!(exhausted.yields, 0);

    let mut other = FakeLease::other("reacquire failed");
    let error = orchestrate_slot_cycle(&mut other, || {
        Ok(crate::SolObservation {
            text: "complete".to_owned(),
            is_error: false,
        })
    });
    assert_eq!(error.observation.text, "reacquire failed");
    assert!(error.observation.is_error);

    let mut spawn_failure = FakeLease::ok();
    let failed = orchestrate_slot_cycle(&mut spawn_failure, || Err("spawn failure".to_owned()));
    assert_eq!(failed.observation.text, "spawn failure");
    assert_eq!(spawn_failure.reacquires, 1);

    let mut no_result = FakeLease::cancelled();
    let cancelled_error =
        orchestrate_slot_cycle(&mut no_result, || Err("spawn failure".to_owned()));
    assert_eq!(
        cancelled_error.observation.text,
        "local_admission_cancelled: cogitate run interrupted before reacquiring local inference"
    );
    std::fs::remove_dir_all(journal).expect("remove journal");
}

#[test]
fn sol_wrapper_relays_every_policy_refusal_family() {
    const REFUSAL_IDS: [&str; 7] = [
        "shell_pipe",
        "empty_command",
        "restricted_cat",
        "hybrid_health",
        "bare_journal_search",
        "support_create_normal_noapproval",
        "support_create_outbound_noapproval",
    ];

    let journal = temp_journal();
    let fixture = fixture();
    for id in REFUSAL_IDS {
        let vector = fixture
            .policy_commands
            .iter()
            .find(|vector| vector.id == id)
            .expect("refusal vector");
        assert!(!vector.expect.allowed, "{id}");
        let mut budget = SolCallBudget::new(2);
        let mut slot = FakeLease::ok();
        let result = run_sol_command(
            &vector.command,
            &vector.access_tier,
            vector.outbound_approval.as_deref(),
            &journal,
            &mut budget,
            &mut slot,
        )
        .expect("fixture tier");
        assert_eq!(result.observation.text, vector.expect.reason, "{id}");
        assert!(result.observation.is_error, "{id}");
        assert_eq!(slot.yields, 0, "{id}");
    }
    std::fs::remove_dir_all(journal).expect("remove journal");
}

#[test]
fn policy_denial_leaves_the_sol_budget_unchanged() {
    let vector = fixture()
        .policy_commands
        .iter()
        .find(|vector| vector.id == "restricted_cat")
        .expect("restricted command vector");
    let journal = temp_journal();
    let mut budget = SolCallBudget::new(1);
    let mut slot = FakeLease::ok();
    let result = run_sol_command(
        &vector.command,
        &vector.access_tier,
        vector.outbound_approval.as_deref(),
        &journal,
        &mut budget,
        &mut slot,
    )
    .expect("fixture tier");
    assert!(result.observation.is_error);
    assert_eq!(budget.count(), 0);
    std::fs::remove_dir_all(journal).expect("remove journal");
}

fn all_tools() -> Vec<&'static ToolSpec> {
    vec![
        crate::sol_tool(),
        &EMIT_FINAL_TOOL,
        &FINISH_TOOL,
        &crate::READ_FILE_TOOL,
        &crate::LIST_DIRECTORY_TOOL,
        &crate::GLOB_TOOL,
        &crate::GREP_SEARCH_TOOL,
    ]
}

fn truncate_input(id: &str) -> String {
    match id {
        "trunc_under_cap" => "abc".to_owned(),
        "trunc_at_cap" => "a".repeat(10),
        "trunc_over_cap" => "a".repeat(11),
        "trunc_multibyte_boundary" => "é".repeat(11),
        "trunc_mixed_multibyte" => format!("{}日本語", "a".repeat(8)),
        "trunc_empty" => String::new(),
        _ => panic!("unknown truncation vector {id}"),
    }
}

fn temp_journal() -> std::path::PathBuf {
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("clock")
        .as_nanos();
    let root = env::temp_dir().join(format!(
        "solstone-cogitate-runtime-{}-{stamp}",
        std::process::id()
    ));
    std::fs::create_dir_all(root.join("entities")).expect("journal fixture");
    root
}

struct FakeLease {
    outcome: Result<(), SlotReacquireError>,
    yields: usize,
    reacquires: usize,
}

impl FakeLease {
    fn ok() -> Self {
        Self {
            outcome: Ok(()),
            yields: 0,
            reacquires: 0,
        }
    }
    fn cancelled() -> Self {
        Self {
            outcome: Err(SlotReacquireError::Cancelled),
            yields: 0,
            reacquires: 0,
        }
    }
    fn other(message: &str) -> Self {
        Self {
            outcome: Err(SlotReacquireError::Other(message.to_owned())),
            yields: 0,
            reacquires: 0,
        }
    }
}

impl SlotLease for FakeLease {
    fn yield_slot(&mut self) {
        self.yields += 1;
    }
    fn reacquire(&mut self) -> Result<(), SlotReacquireError> {
        self.reacquires += 1;
        self.outcome.clone()
    }
    fn cancel_pending_reacquire(&mut self) {}
}
