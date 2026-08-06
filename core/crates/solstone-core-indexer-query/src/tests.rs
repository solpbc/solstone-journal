// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::collections::BTreeSet;

use chrono::NaiveDate;
use rusqlite::{Connection, params};

use crate::{
    CompileOutcome, EffectiveDateConstraint, PredicateInput, QueryPredicate, compile_query,
};

const REFERENCE_DATE: &str = "2026-08-06";
const CREATE_CHUNKS: &str = "\
CREATE VIRTUAL TABLE IF NOT EXISTS chunks USING fts5(
content,
path UNINDEXED,
day UNINDEXED,
facet UNINDEXED,
agent UNINDEXED,
stream UNINDEXED,
idx UNINDEXED,
time_bucket UNINDEXED
)";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Disposition {
    MatchesReference,
    Accepted,
    Defect,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct NativeOutput {
    expression: &'static str,
    compiled_day_from: Option<&'static str>,
    compiled_day_to: Option<&'static str>,
    remaining_text: &'static str,
    temporal_day_from: Option<&'static str>,
    temporal_day_to: Option<&'static str>,
}

#[derive(Debug, Eq, PartialEq)]
struct ActualOutput {
    expression: String,
    compiled_day_from: Option<String>,
    compiled_day_to: Option<String>,
    remaining_text: String,
    temporal_day_from: Option<String>,
    temporal_day_to: Option<String>,
}

impl NativeOutput {
    fn to_actual(self) -> ActualOutput {
        ActualOutput {
            expression: self.expression.to_string(),
            compiled_day_from: self.compiled_day_from.map(str::to_string),
            compiled_day_to: self.compiled_day_to.map(str::to_string),
            remaining_text: self.remaining_text.to_string(),
            temporal_day_from: self.temporal_day_from.map(str::to_string),
            temporal_day_to: self.temporal_day_to.map(str::to_string),
        }
    }
}

#[derive(Clone, Copy, Debug)]
struct LedgerEntry {
    case: &'static str,
    disposition: Disposition,
    native: NativeOutput,
    reason: &'static str,
}

macro_rules! entry {
    ($case:literal, $disposition:ident, $expression:literal, $remaining:literal, $from:expr, $to:expr, $reason:literal) => {
        LedgerEntry {
            case: $case,
            disposition: Disposition::$disposition,
            native: NativeOutput {
                expression: $expression,
                compiled_day_from: $from,
                compiled_day_to: $to,
                remaining_text: $remaining,
                temporal_day_from: $from,
                temporal_day_to: $to,
            },
            reason: $reason,
        }
    };
}

/// Unlike the sparse content-family divergence table, this ledger deliberately
/// records every frozen corpus case. It is the one-to-one decision record for
/// a compiler whose intended output changes in many, but not all, cases.
const LEDGER: &[LedgerEntry] = &[
    entry!(
        "empty",
        MatchesReference,
        "",
        "",
        None,
        None,
        "empty input remains expression-free"
    ),
    entry!(
        "whitespace_only",
        MatchesReference,
        "",
        "   ",
        None,
        None,
        "whitespace input remains expression-free"
    ),
    entry!(
        "interior_whitespace_run",
        Accepted,
        "\"alpha\" AND \"beta\"",
        "alpha     beta",
        None,
        None,
        "explicit AND replaces the reference NEAR branch"
    ),
    entry!(
        "leading_trailing_whitespace",
        Accepted,
        "\"alpha\" AND \"beta\"",
        "  alpha beta  ",
        None,
        None,
        "explicit AND replaces the reference NEAR branch"
    ),
    entry!(
        "single_term",
        Accepted,
        "\"standup\"",
        "standup",
        None,
        None,
        "uniform one-token phrase quoting is intentional"
    ),
    entry!(
        "two_terms",
        Accepted,
        "\"weekly\" AND \"meeting\"",
        "weekly meeting",
        None,
        None,
        "D12 removes NEAR; measured on 20 invented multi-word queries, match set identical 20/20, top-10 ordering identical 18/20"
    ),
    entry!(
        "four_terms",
        Accepted,
        "\"quarterly\" AND \"planning\" AND \"review\" AND \"notes\"",
        "quarterly planning review notes",
        None,
        None,
        "D12 removes NEAR in favor of explicit conjunction"
    ),
    entry!(
        "mixed_case_terms",
        Accepted,
        "\"Weekly\" AND \"Meeting\"",
        "Weekly Meeting",
        None,
        None,
        "uniform phrase quoting and explicit conjunction"
    ),
    entry!(
        "operator_or",
        Accepted,
        "\"standup\" OR \"retro\"",
        "standup OR retro",
        None,
        None,
        "valid OR is retained while operands are uniformly quoted"
    ),
    entry!(
        "operator_and",
        Accepted,
        "\"standup\" AND \"retro\"",
        "standup AND retro",
        None,
        None,
        "valid AND is retained while operands are uniformly quoted"
    ),
    entry!(
        "operator_not",
        Accepted,
        "\"standup\" NOT \"retro\"",
        "standup NOT retro",
        None,
        None,
        "valid binary NOT is retained while operands are uniformly quoted"
    ),
    entry!(
        "operator_or_three",
        Accepted,
        "\"alpha\" OR \"beta\" OR \"gamma\"",
        "alpha OR beta OR gamma",
        None,
        None,
        "valid OR operators are retained while operands are uniformly quoted"
    ),
    entry!(
        "lowercase_and_is_a_word",
        Accepted,
        "\"salt\" AND \"and\" AND \"pepper\"",
        "salt and pepper",
        None,
        None,
        "lowercase operator-shaped text remains a literal term"
    ),
    entry!(
        "lowercase_or_is_a_word",
        Accepted,
        "\"this\" AND \"or\" AND \"that\"",
        "this or that",
        None,
        None,
        "lowercase operator-shaped text remains a literal term"
    ),
    entry!(
        "balanced_quoted_phrase",
        MatchesReference,
        "\"release train\"",
        "\"release train\"",
        None,
        None,
        "single user phrase is already canonical"
    ),
    entry!(
        "quoted_phrase_plus_term",
        Accepted,
        "\"release train\" AND \"schedule\"",
        "\"release train\" schedule",
        None,
        None,
        "quoted spans no longer bypass uniform atom processing"
    ),
    entry!(
        "unterminated_quote",
        Accepted,
        "\"release train\"",
        "\"release train",
        None,
        None,
        "atomization closes the unterminated phrase at end-of-input; temporal extraction remains unquoted and unchanged"
    ),
    entry!(
        "quote_inside_terms",
        Accepted,
        "\"say\" AND \"hi\" AND \"now\"",
        "say \"hi\" now",
        None,
        None,
        "quoted spans no longer bypass uniform atom processing"
    ),
    entry!(
        "two_quoted_phrases",
        Accepted,
        "\"alpha beta\" AND \"gamma delta\"",
        "\"alpha beta\" \"gamma delta\"",
        None,
        None,
        "explicit AND replaces implicit FTS adjacency"
    ),
    entry!(
        "apostrophe_proper_noun",
        MatchesReference,
        "\"O'Brien\"",
        "O'Brien",
        None,
        None,
        "reference already phrase quotes apostrophes"
    ),
    entry!(
        "apostrophe_contraction",
        MatchesReference,
        "\"it's\"",
        "it's",
        None,
        None,
        "reference already phrase quotes apostrophes"
    ),
    entry!(
        "apostrophe_with_wildcard",
        MatchesReference,
        "\"O'Bri\"*",
        "O'Bri*",
        None,
        None,
        "reference already preserves the terminal wildcard"
    ),
    entry!(
        "trailing_wildcard",
        Accepted,
        "\"plan\"*",
        "plan*",
        None,
        None,
        "uniform phrase quoting retains prefix syntax"
    ),
    entry!(
        "short_trailing_wildcard",
        Accepted,
        "\"a\"*",
        "a*",
        None,
        None,
        "uniform phrase quoting retains prefix syntax"
    ),
    entry!(
        "wildcard_inside_quotes",
        MatchesReference,
        "\"plan*\"",
        "\"plan*\"",
        None,
        None,
        "user-quoted wildcard remains literal"
    ),
    entry!(
        "wildcard_midword",
        Accepted,
        "\"pl*an\"",
        "pl*an",
        None,
        None,
        "uniform phrase quoting preserves a buried wildcard literally"
    ),
    entry!(
        "accented_latin_jose",
        Accepted,
        "\"José\"",
        "José",
        None,
        None,
        "D1 preserves accented Latin instead of deleting the accent"
    ),
    entry!(
        "accented_latin_cafe",
        Accepted,
        "\"café\"",
        "café",
        None,
        None,
        "D1 preserves accented Latin instead of deleting the accent"
    ),
    entry!(
        "accented_latin_phrase",
        Accepted,
        "\"José\" AND \"café\" AND \"meeting\"",
        "José café meeting",
        None,
        None,
        "D1 preserves accented terms and D12 uses explicit conjunction"
    ),
    entry!(
        "umlaut",
        Accepted,
        "\"Müller\"",
        "Müller",
        None,
        None,
        "D1 preserves non-ASCII letters"
    ),
    entry!(
        "cedilla",
        Accepted,
        "\"façade\"",
        "façade",
        None,
        None,
        "D1 preserves non-ASCII letters"
    ),
    entry!(
        "greek",
        Accepted,
        "\"Αθήνα\"",
        "Αθήνα",
        None,
        None,
        "D1 preserves non-ASCII letters"
    ),
    entry!(
        "cyrillic",
        Accepted,
        "\"Москва\"",
        "Москва",
        None,
        None,
        "D1 preserves non-ASCII letters"
    ),
    entry!(
        "cjk_han",
        Accepted,
        "\"会議\"",
        "会議",
        None,
        None,
        "D1 preserves a CJK token instead of deleting it"
    ),
    entry!(
        "cjk_subrun",
        Accepted,
        "\"日本語\"",
        "日本語",
        None,
        None,
        "D1 preserves the CJK token; unicode61 has no CJK segmentation, so larger-run substring recall is excluded by policy"
    ),
    entry!(
        "cjk_kana",
        Accepted,
        "\"ミーティング\"",
        "ミーティング",
        None,
        None,
        "D1 preserves a Kana token instead of deleting it"
    ),
    entry!(
        "hangul",
        Accepted,
        "\"회의\"",
        "회의",
        None,
        None,
        "D1 preserves a Hangul token instead of deleting it"
    ),
    entry!(
        "arabic",
        Accepted,
        "\"اجتماع\"",
        "اجتماع",
        None,
        None,
        "D1 preserves an Arabic token instead of deleting it"
    ),
    entry!(
        "hebrew",
        Accepted,
        "\"פגישה\"",
        "פגישה",
        None,
        None,
        "D1 preserves a Hebrew token instead of deleting it"
    ),
    entry!(
        "devanagari",
        Accepted,
        "\"बैठक\"",
        "बैठक",
        None,
        None,
        "D1 preserves a Devanagari token instead of deleting it"
    ),
    entry!(
        "emoji",
        Accepted,
        "\"meeting\"",
        "meeting 📅",
        None,
        None,
        "the mixed query retains its indexable word and drops only the emoji symbol"
    ),
    entry!(
        "mixed_script",
        Accepted,
        "\"meeting\" AND \"会議\" AND \"réunion\"",
        "meeting 会議 réunion",
        None,
        None,
        "D1 preserves every indexable script and D12 uses explicit conjunction"
    ),
    entry!(
        "nfc_composed",
        Accepted,
        "\"café\"",
        "café",
        None,
        None,
        "D1 retains composed Unicode input"
    ),
    entry!(
        "nfd_decomposed",
        Accepted,
        "\"café\"",
        "cafe\u{301}",
        None,
        None,
        "NFC normalization gives decomposed Unicode the same searchable atom"
    ),
    entry!(
        "email_address",
        Accepted,
        "\"someone@example.com\"",
        "someone@example.com",
        None,
        None,
        "one punctuation-preserving phrase intentionally narrows recall from reference fragments"
    ),
    entry!(
        "domain",
        Accepted,
        "\"example.com\"",
        "example.com",
        None,
        None,
        "one punctuation-preserving phrase intentionally narrows recall from reference fragments"
    ),
    entry!(
        "posix_path",
        Accepted,
        "\"notes/2026/plan.md\"",
        "notes/2026/plan.md",
        None,
        None,
        "one punctuation-preserving phrase intentionally narrows recall from reference fragments"
    ),
    entry!(
        "windows_path",
        Accepted,
        "\"notes\\2026\\plan.md\"",
        "notes\\2026\\plan.md",
        None,
        None,
        "one punctuation-preserving phrase intentionally narrows recall from reference fragments"
    ),
    entry!(
        "hyphenated_compound",
        Accepted,
        "\"follow-up\"",
        "follow-up",
        None,
        None,
        "one punctuation-preserving phrase intentionally narrows recall from reference fragments"
    ),
    entry!(
        "underscored_identifier",
        Accepted,
        "\"weekly_reflection\"",
        "weekly_reflection",
        None,
        None,
        "one punctuation-preserving phrase intentionally narrows recall from reference fragments"
    ),
    entry!(
        "dotted_version",
        Accepted,
        "\"version\" AND \"1.0.22\"",
        "version 1.0.22",
        None,
        None,
        "the dotted version remains one atom, intentionally narrowing recall from fragments"
    ),
    entry!(
        "url",
        Accepted,
        "\"https://example.com/notes\"",
        "https://example.com/notes",
        None,
        None,
        "one punctuation-preserving phrase intentionally narrows recall from reference fragments"
    ),
    entry!(
        "parenthesised",
        Accepted,
        "\"planning\" AND \"(draft)\"",
        "planning (draft)",
        None,
        None,
        "the parenthesised atom is preserved rather than split into fragments"
    ),
    entry!(
        "colon_separated",
        Accepted,
        "\"note:draft\"",
        "note:draft",
        None,
        None,
        "one punctuation-preserving phrase intentionally narrows recall from reference fragments"
    ),
    entry!(
        "tab_separated",
        Accepted,
        "\"alpha\" AND \"beta\"",
        "alpha\tbeta",
        None,
        None,
        "whitespace atomization uses explicit conjunction"
    ),
    entry!(
        "newline_separated",
        Accepted,
        "\"alpha\" AND \"beta\"",
        "alpha\nbeta",
        None,
        None,
        "whitespace atomization uses explicit conjunction"
    ),
    entry!(
        "null_byte",
        Accepted,
        "\"alpha\" AND \"beta\"",
        "alpha\0beta",
        None,
        None,
        "control normalization reaches whitespace atomization, which uses explicit conjunction"
    ),
    entry!(
        "bell_character",
        Accepted,
        "\"alpha\" AND \"beta\"",
        "alpha\u{7}beta",
        None,
        None,
        "control normalization reaches whitespace atomization, which uses explicit conjunction"
    ),
    entry!(
        "temporal_yesterday",
        MatchesReference,
        "",
        "",
        Some("20260805"),
        Some("20260805"),
        "pure temporal query remains filters-only"
    ),
    entry!(
        "temporal_today",
        MatchesReference,
        "",
        "",
        Some("20260806"),
        Some("20260806"),
        "pure temporal query remains filters-only"
    ),
    entry!(
        "temporal_last_week",
        MatchesReference,
        "",
        "",
        Some("20260727"),
        Some("20260802"),
        "pure temporal query remains filters-only"
    ),
    entry!(
        "temporal_this_week",
        MatchesReference,
        "",
        "",
        Some("20260803"),
        Some("20260809"),
        "pure temporal query remains filters-only"
    ),
    entry!(
        "temporal_last_month",
        MatchesReference,
        "",
        "",
        Some("20260701"),
        Some("20260731"),
        "pure temporal query remains filters-only"
    ),
    entry!(
        "temporal_this_month",
        MatchesReference,
        "",
        "",
        Some("20260801"),
        Some("20260831"),
        "pure temporal query remains filters-only"
    ),
    entry!(
        "temporal_weekend_over",
        MatchesReference,
        "",
        "",
        Some("20260801"),
        Some("20260802"),
        "pure temporal query remains filters-only"
    ),
    entry!(
        "temporal_weekend_on",
        MatchesReference,
        "",
        "",
        Some("20260801"),
        Some("20260802"),
        "pure temporal query remains filters-only"
    ),
    entry!(
        "temporal_last_monday",
        MatchesReference,
        "",
        "",
        Some("20260803"),
        Some("20260803"),
        "pure temporal query remains filters-only"
    ),
    entry!(
        "temporal_last_sunday",
        MatchesReference,
        "",
        "",
        Some("20260802"),
        Some("20260802"),
        "pure temporal query remains filters-only"
    ),
    entry!(
        "temporal_last_friday_mixed_case",
        MatchesReference,
        "",
        "",
        Some("20260731"),
        Some("20260731"),
        "pure temporal query remains filters-only"
    ),
    entry!(
        "temporal_with_terms",
        Accepted,
        "\"standup\" AND \"notes\" AND \"from\"",
        "standup notes from",
        Some("20260805"),
        Some("20260805"),
        "temporal extraction matches reference; remaining atoms use explicit conjunction"
    ),
    entry!(
        "temporal_leading",
        Accepted,
        "\"standup\" AND \"notes\"",
        "standup notes",
        Some("20260805"),
        Some("20260805"),
        "temporal extraction matches reference; remaining atoms use explicit conjunction"
    ),
    entry!(
        "temporal_quoted_is_literal",
        Accepted,
        "\"yesterday\" AND \"standup\"",
        "\"yesterday\" standup",
        None,
        None,
        "quoted temporal text remains literal and atoms are explicit"
    ),
    entry!(
        "temporal_two_phrases",
        Accepted,
        "\"and\" AND \"last\" AND \"week\"",
        "and last week",
        Some("20260805"),
        Some("20260805"),
        "only the earliest temporal phrase is removed; remaining atoms use explicit conjunction"
    ),
    entry!(
        "temporal_inside_longer_phrase",
        Accepted,
        "\"notes\" AND \"from\" AND \"about\" AND \"planning\"",
        "notes from about planning",
        Some("20260803"),
        Some("20260803"),
        "last weekday resolution matches reference; remaining atoms use explicit conjunction"
    ),
    entry!(
        "temporal_with_quoted_phrase",
        MatchesReference,
        "\"release train\"",
        "\"release train\"",
        Some("20260727"),
        Some("20260802"),
        "single quoted residual phrase is already canonical"
    ),
    entry!(
        "operator_after_quote",
        Accepted,
        "\"release train\" OR \"standup\"",
        "\"release train\" OR standup",
        None,
        None,
        "valid OR is retained while the unquoted operand is uniformly quoted"
    ),
    entry!(
        "or_binds_looser_than_implicit_and",
        Accepted,
        "\"alpha\" OR \"beta\" AND \"gamma\"",
        "alpha OR beta gamma",
        None,
        None,
        "explicit conjunction replaces implicit adjacency"
    ),
    entry!(
        "stopword_heavy",
        Accepted,
        "\"what\" AND \"did\" AND \"i\" AND \"do\" AND \"about\" AND \"the\" AND \"meeting\"",
        "what did i do about the meeting",
        None,
        None,
        "relaxation stopwords are out of scope; compilation uses explicit conjunction"
    ),
    entry!(
        "single_stopword",
        Accepted,
        "\"the\"",
        "the",
        None,
        None,
        "uniform one-token phrase quoting is intentional"
    ),
];

fn reference_date() -> NaiveDate {
    NaiveDate::parse_from_str(REFERENCE_DATE, "%Y-%m-%d").expect("reference date parses")
}

fn expression(compiled: &CompileOutcome) -> &str {
    match compiled {
        CompileOutcome::Compiled { expression } => expression,
        CompileOutcome::NoInput
        | CompileOutcome::FiltersOnly
        | CompileOutcome::NoTokenizableTerm => "",
    }
}

fn actual_output(input: &str) -> (crate::QueryCompilation, ActualOutput) {
    let compiled = compile_query(input, reference_date());
    let output = ActualOutput {
        expression: expression(&compiled.outcome).to_string(),
        compiled_day_from: compiled.temporal.day_from.clone(),
        compiled_day_to: compiled.temporal.day_to.clone(),
        remaining_text: compiled.temporal.remaining_text.clone(),
        temporal_day_from: compiled.temporal.day_from.clone(),
        temporal_day_to: compiled.temporal.day_to.clone(),
    };
    (compiled, output)
}

fn fixture() -> serde_json::Value {
    serde_json::from_str(include_str!("../../../fixtures/query_compiler_cases.json"))
        .expect("query compiler fixture parses")
}

fn reference_output(case: &serde_json::Value) -> ActualOutput {
    ActualOutput {
        expression: case["compiled"]["expression"]
            .as_str()
            .expect("expression")
            .to_string(),
        compiled_day_from: case["compiled"]["day_from"].as_str().map(str::to_string),
        compiled_day_to: case["compiled"]["day_to"].as_str().map(str::to_string),
        remaining_text: case["temporal"]["remaining_text"]
            .as_str()
            .expect("remaining")
            .to_string(),
        temporal_day_from: case["temporal"]["day_from"].as_str().map(str::to_string),
        temporal_day_to: case["temporal"]["day_to"].as_str().map(str::to_string),
    }
}

#[test]
fn every_corpus_case_matches_its_recorded_decision() {
    for case in fixture()["cases"].as_array().expect("fixture cases") {
        let id = case["case"].as_str().expect("case id");
        let input = case["input"].as_str().expect("input");
        let entry = LEDGER
            .iter()
            .find(|entry| entry.case == id)
            .expect("ledger entry");
        let (_, actual) = actual_output(input);
        let reference = reference_output(case);
        match entry.disposition {
            Disposition::MatchesReference => {
                assert_eq!(actual, reference, "{id}: marked reference but moved")
            }
            Disposition::Accepted | Disposition::Defect => {
                assert_ne!(
                    actual, reference,
                    "{id}: recorded divergence now matches reference"
                );
                assert_eq!(
                    actual,
                    entry.native.to_actual(),
                    "{id}: recorded as a {:?} divergence, but the native output has since moved\nrecorded {:?}\nactual   {:?}",
                    entry.disposition,
                    entry.native,
                    actual
                );
            }
        }
        assert_eq!(
            actual,
            entry.native.to_actual(),
            "{id}: ledger tuple differs from native output"
        );
    }
}

#[test]
fn ledger_covers_fixture_one_to_one() {
    let corpus = fixture();
    let fixture_cases: BTreeSet<&str> = corpus["cases"]
        .as_array()
        .expect("fixture cases")
        .iter()
        .map(|case| case["case"].as_str().expect("case id"))
        .collect();
    let ledger_cases: BTreeSet<&str> = LEDGER.iter().map(|entry| entry.case).collect();
    assert_eq!(fixture_cases.len(), 79);
    assert_eq!(fixture_cases, ledger_cases);
}

#[test]
fn outstanding_defects_are_declared() {
    let defects: Vec<&LedgerEntry> = LEDGER
        .iter()
        .filter(|entry| entry.disposition == Disposition::Defect)
        .collect();
    for entry in &defects {
        assert!(
            entry.reason.len() > 40,
            "{}: defect reason must describe an owner-visible consequence",
            entry.case
        );
    }
    assert!(defects.is_empty(), "outstanding defects: {defects:?}");
}

#[test]
fn compile_outcomes_are_distinct() {
    assert!(matches!(
        compile_query("", reference_date()).outcome,
        CompileOutcome::NoInput
    ));
    assert!(matches!(
        compile_query("   ", reference_date()).outcome,
        CompileOutcome::NoInput
    ));
    assert!(matches!(
        compile_query("yesterday", reference_date()).outcome,
        CompileOutcome::FiltersOnly
    ));
    assert!(matches!(
        compile_query("standup", reference_date()).outcome,
        CompileOutcome::Compiled { .. }
    ));
    assert!(matches!(
        compile_query("📅📆", reference_date()).outcome,
        CompileOutcome::NoTokenizableTerm
    ));
}

#[test]
fn normalization_forms_compile_to_equivalent_fts_queries() {
    assert_eq!(
        expression(&compile_query("café", reference_date()).outcome),
        expression(&compile_query("cafe\u{301}", reference_date()).outcome)
    );
}

#[test]
fn malformed_operators_become_fts_literals() {
    let connection = test_db();
    for query in ["AND", "NOT alpha"] {
        let compiled = compile_query(query, reference_date());
        let expression = expression(&compiled.outcome);
        let _ = match_count(&connection, expression);
    }
}

#[test]
fn predicate_date_precedence_and_normalization_match_python() {
    let compiled = compile_query("yesterday", reference_date());
    let exact = QueryPredicate::new(
        compiled.outcome.clone(),
        &compiled.temporal,
        PredicateInput {
            day: Some("20260101".into()),
            day_from: Some("20260102".into()),
            day_to: Some("20260103".into()),
            facet: Some("WORK".into()),
            agent: Some("Event".into()),
            stream: Some("raw".into()),
            time_bucket: Some("morning".into()),
        },
    );
    assert_eq!(
        exact.effective_date,
        EffectiveDateConstraint::Exact("20260101".into())
    );
    assert_eq!(exact.facet.as_deref(), Some("work"));
    assert_eq!(exact.agent.as_deref(), Some("event"));
    let caller_range = QueryPredicate::new(
        compiled.outcome.clone(),
        &compiled.temporal,
        PredicateInput {
            day_from: Some("20260102".into()),
            ..PredicateInput::default()
        },
    );
    assert_eq!(
        caller_range.effective_date,
        EffectiveDateConstraint::Range {
            day_from: Some("20260102".into()),
            day_to: None
        }
    );
    let temporal_range = QueryPredicate::new(
        compiled.outcome,
        &compiled.temporal,
        PredicateInput::default(),
    );
    assert_eq!(
        temporal_range.effective_date,
        EffectiveDateConstraint::Range {
            day_from: Some("20260805".into()),
            day_to: Some("20260805".into())
        }
    );
}

#[test]
fn all_compiled_corpus_expressions_are_valid_without_near() {
    let connection = test_db();
    for case in fixture()["cases"].as_array().expect("fixture cases") {
        let id = case["case"].as_str().expect("case id");
        let compiled = compile_query(case["input"].as_str().expect("input"), reference_date());
        if let CompileOutcome::Compiled { expression } = compiled.outcome {
            assert!(!expression.contains("NEAR"), "{id}: D12 forbids NEAR");
            match_count(&connection, &expression);
        }
    }
}

#[test]
fn non_ascii_differential_retrieval_preserves_indexable_terms() {
    let excluded = ["cjk_subrun", "emoji"];
    let fixture = fixture();
    for case in fixture["cases"].as_array().expect("fixture cases") {
        let id = case["case"].as_str().expect("case id");
        let input = case["input"].as_str().expect("input");
        if !input.is_ascii() && !excluded.contains(&id) {
            let compiled = compile_query(input, reference_date());
            let expression = expression(&compiled.outcome);
            let connection = test_db();
            connection
                .execute("INSERT INTO chunks(content) VALUES (?)", [input])
                .expect("insert input");
            assert_eq!(
                match_count(&connection, expression),
                1,
                "{id}: native expression retrieves exact input"
            );
            let reference = case["compiled"]["expression"]
                .as_str()
                .expect("reference expression");
            let old_count = if reference.is_empty() {
                0
            } else {
                match_count(&connection, reference)
            };
            if id != "nfd_decomposed" {
                assert_eq!(
                    old_count, 0,
                    "{id}: destroyed reference expression must not retrieve input"
                );
            }
        }
    }
}

#[test]
fn mixed_emoji_query_retains_its_indexable_word() {
    let connection = test_db();
    connection
        .execute("INSERT INTO chunks(content) VALUES (?)", ["meeting 📅"])
        .expect("insert mixed emoji input");
    let compiled = compile_query("meeting 📅", reference_date());
    assert_eq!(expression(&compiled.outcome), "\"meeting\"");
    assert_eq!(match_count(&connection, expression(&compiled.outcome)), 1);
}

#[test]
fn or_and_interior_quotes_remain_valid_fts() {
    let connection = test_db();
    connection
        .execute("INSERT INTO chunks(content) VALUES ('foo')", [])
        .expect("insert foo");
    connection
        .execute("INSERT INTO chunks(content) VALUES ('bar')", [])
        .expect("insert bar");
    let or_compiled = compile_query("foo OR bar", reference_date());
    let or_expression = expression(&or_compiled.outcome);
    assert_eq!(match_count(&connection, or_expression), 2);
    let quote_compiled = compile_query("foo\"bar", reference_date());
    let quote_expression = expression(&quote_compiled.outcome);
    assert!(quote_expression.contains("\"\""));
    let _ = match_count(&connection, quote_expression);
}

fn test_db() -> Connection {
    let connection = Connection::open_in_memory().expect("in-memory SQLite");
    connection
        .execute_batch(CREATE_CHUNKS)
        .expect("exact production chunks DDL");
    connection
}

fn match_count(connection: &Connection, expression: &str) -> usize {
    let count: i64 = connection
        .query_row(
            "SELECT count(*) FROM chunks WHERE chunks MATCH ?",
            params![expression],
            |row| row.get(0),
        )
        .expect("MATCH query succeeds");
    usize::try_from(count).expect("count is non-negative")
}
