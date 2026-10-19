// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use crate::admit::{
    ParsedMessage, dotted_to_fluent_id, fluent_to_dotted_id, parse_ftl_resource,
    validate_admitted_message,
};
use crate::catalog::LoadedCatalog;
use crate::cli::update_history;
use crate::coverage::{CoverageCell, CoverageReport};
use crate::ir::build_ir;
use crate::locale::LocaleRegistry;
use crate::metadata::MessageMetadata;
use crate::model::{
    ArgumentSpec, Branch, ElementKind, PatternElement, ProtectedSlotSpec, SelectorSpec,
};
use crate::pseudo::{generate_pseudo_ftl, transform_ar_xb, transform_en_xa};
use crate::scalar::{
    CARDINAL_MAX, CARDINAL_MIN, INTEGER_MAX, INTEGER_MIN, check_scalar_value,
    validate_numeric_bounds,
};
use crate::vocab::{Channel, FormatIntent, NumericStyle, ProtectedSlotKind, ScalarType};
use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

static TEST_TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);

fn unique_temp_dir(prefix: &str) -> PathBuf {
    let pid = std::process::id();
    let count = TEST_TEMP_COUNTER.fetch_add(1, Ordering::Relaxed);
    let p = Path::new("/var/tmp").join(format!("i18n-test-{}-{}-{}", prefix, pid, count));
    let _ = fs::remove_dir_all(&p);
    fs::create_dir_all(&p).unwrap();
    p
}

fn copy_dir_recursive(src: &Path, dst: &Path) {
    fs::create_dir_all(dst).unwrap();
    for entry in fs::read_dir(src).unwrap().flatten() {
        let path = entry.path();
        let target = dst.join(entry.file_name());
        if path.is_dir() {
            copy_dir_recursive(&path, &target);
        } else {
            fs::copy(&path, &target).unwrap();
        }
    }
}

fn fixture_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("fixtures")
        .join("synth")
}

#[test]
fn test_fixture_compiles_cleanly() {
    let fix = fixture_dir();
    let res = crate::compile(&fix, Channel::Production, None);
    assert!(res.is_ok(), "Expected fixture to compile: {:?}", res.err());
}

#[test]
fn test_fixture_coverage_denominators() {
    let fix = fixture_dir();
    let cat = LoadedCatalog::load_and_validate(&fix, Channel::Production, None).unwrap();

    let mut bundle_locales = HashMap::new();
    bundle_locales.insert("synth-app".to_string(), vec!["en-x-synth".to_string()]);
    let mut message_bundles = HashMap::new();
    for (id, meta) in &cat.metadata {
        message_bundles.insert(id.clone(), meta.bundles.clone());
    }

    let report = CoverageReport::compute(
        &bundle_locales,
        &message_bundles,
        &cat.source_messages,
        &cat.translations,
        &HashSet::new(),
    );

    assert_eq!(report.messages_den, 8);
    assert_eq!(report.messages_num, 8);
    assert_eq!(report.branches_den, 4);
    assert_eq!(report.branches_num, 4);
    assert_eq!(report.slots_den, 6);
    assert_eq!(report.slots_num, 6);
    assert_eq!(report.cells.len(), 8);
    for cell in &report.cells {
        assert_eq!(cell.bundle, "synth-app");
        assert_eq!(cell.locale, "en-x-synth");
    }
}

#[test]
fn test_coverage_cells_with_two_bundles_and_two_locales() {
    let mut bundle_locales = HashMap::new();
    bundle_locales.insert("bundle-a".to_string(), vec!["loc-1".to_string()]);
    bundle_locales.insert(
        "bundle-b".to_string(),
        vec!["loc-1".to_string(), "loc-2".to_string()],
    );

    let mut message_bundles = HashMap::new();
    message_bundles.insert("msg1".to_string(), vec!["bundle-a".to_string()]);
    message_bundles.insert("msg2".to_string(), vec!["bundle-b".to_string()]);
    message_bundles.insert(
        "msg3".to_string(),
        vec!["bundle-a".to_string(), "bundle-b".to_string()],
    );

    let mut source_messages = HashMap::new();
    source_messages.insert(
        "msg1".to_string(),
        ParsedMessage {
            id: "msg1".to_string(),
            branches: vec![Branch {
                key: None,
                default: true,
                pattern: vec![PatternElement {
                    kind: ElementKind::Text,
                    value: "hello".to_string(),
                }],
            }],
            selector_var: None,
        },
    );
    source_messages.insert(
        "msg2".to_string(),
        ParsedMessage {
            id: "msg2".to_string(),
            branches: vec![
                Branch {
                    key: Some("one".to_string()),
                    default: false,
                    pattern: vec![],
                },
                Branch {
                    key: Some("other".to_string()),
                    default: true,
                    pattern: vec![],
                },
            ],
            selector_var: Some("n".to_string()),
        },
    );
    source_messages.insert(
        "msg3".to_string(),
        ParsedMessage {
            id: "msg3".to_string(),
            branches: vec![Branch {
                key: None,
                default: true,
                pattern: vec![],
            }],
            selector_var: None,
        },
    );

    let mut translations = HashMap::new();
    let mut loc1_map = HashMap::new();
    loc1_map.insert("msg1".to_string(), source_messages["msg1"].clone());
    // In loc-1, msg2 adds a CLDR category [few]
    loc1_map.insert(
        "msg2".to_string(),
        ParsedMessage {
            id: "msg2".to_string(),
            branches: vec![
                Branch {
                    key: Some("one".to_string()),
                    default: false,
                    pattern: vec![],
                },
                Branch {
                    key: Some("few".to_string()),
                    default: false,
                    pattern: vec![],
                },
                Branch {
                    key: Some("other".to_string()),
                    default: true,
                    pattern: vec![],
                },
            ],
            selector_var: Some("n".to_string()),
        },
    );
    loc1_map.insert("msg3".to_string(), source_messages["msg3"].clone());
    translations.insert("loc-1".to_string(), loc1_map);

    let mut loc2_map = HashMap::new();
    loc2_map.insert("msg2".to_string(), source_messages["msg2"].clone());
    loc2_map.insert("msg3".to_string(), source_messages["msg3"].clone());
    translations.insert("loc-2".to_string(), loc2_map);

    let report = CoverageReport::compute(
        &bundle_locales,
        &message_bundles,
        &source_messages,
        &translations,
        &HashSet::new(),
    );

    let expected_cells = vec![
        CoverageCell {
            bundle: "bundle-a".to_string(),
            locale: "loc-1".to_string(),
            message_id: "msg1".to_string(),
        },
        CoverageCell {
            bundle: "bundle-a".to_string(),
            locale: "loc-1".to_string(),
            message_id: "msg3".to_string(),
        },
        CoverageCell {
            bundle: "bundle-b".to_string(),
            locale: "loc-1".to_string(),
            message_id: "msg2".to_string(),
        },
        CoverageCell {
            bundle: "bundle-b".to_string(),
            locale: "loc-1".to_string(),
            message_id: "msg3".to_string(),
        },
        CoverageCell {
            bundle: "bundle-b".to_string(),
            locale: "loc-2".to_string(),
            message_id: "msg2".to_string(),
        },
        CoverageCell {
            bundle: "bundle-b".to_string(),
            locale: "loc-2".to_string(),
            message_id: "msg3".to_string(),
        },
    ];
    assert_eq!(report.cells, expected_cells);
    assert_eq!(report.branches_den, 4);
    assert_eq!(report.branches_num, 4);
    assert!(report.branches_num <= report.branches_den);
}

#[test]
fn test_raw_dotted_fluent_rejected() {
    let raw = "synth.notes.plain = A synthetic sentence.\n";
    let res = fluent_syntax::parser::parse(raw);
    assert!(
        res.is_err(),
        "Raw dotted line should be rejected by Fluent parser"
    );
    let (resource, _errs) = res.unwrap_err();
    assert!(
        resource
            .body
            .iter()
            .any(|entry| matches!(entry, fluent_syntax::ast::Entry::Junk { .. })),
        "Expected Entry::Junk in parse result"
    );
}

#[test]
fn test_pseudo_ftl_parses_cleanly_with_fluent() {
    let fix = fixture_dir();
    let cat = LoadedCatalog::load_and_validate(&fix, Channel::Production, None).unwrap();

    let en_xa = generate_pseudo_ftl(&cat.source_messages, transform_en_xa);
    let ar_xb = generate_pseudo_ftl(&cat.source_messages, transform_ar_xb);

    let res_xa = fluent_syntax::parser::parse(en_xa.as_str());
    assert!(
        res_xa.is_ok(),
        "en-XA should parse as Ok: {:?}",
        res_xa.err()
    );
    let res_resource_xa = res_xa.unwrap();
    assert!(
        !res_resource_xa
            .body
            .iter()
            .any(|entry| matches!(entry, fluent_syntax::ast::Entry::Junk { .. })),
        "en-XA should contain no Entry::Junk"
    );

    let res_xb = fluent_syntax::parser::parse(ar_xb.as_str());
    assert!(
        res_xb.is_ok(),
        "ar-XB should parse as Ok: {:?}",
        res_xb.err()
    );
    let res_resource_xb = res_xb.unwrap();
    assert!(
        !res_resource_xb
            .body
            .iter()
            .any(|entry| matches!(entry, fluent_syntax::ast::Entry::Junk { .. })),
        "ar-XB should contain no Entry::Junk"
    );

    for id in cat.source_messages.keys() {
        let fluent_id = dotted_to_fluent_id(id);
        assert!(
            fluent_id
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
        );
        assert_eq!(&fluent_to_dotted_id(&fluent_id), id);
    }
}

#[test]
fn test_admission_rejection_classes() {
    struct Case {
        ftl: &'static str,
        expected_rule: &'static str,
    }

    let cases = vec![
        Case {
            ftl: "-brand = Solstone\nmsg = Hello\n",
            expected_rule: "term",
        },
        Case {
            ftl: "msg = { other_msg }\n",
            expected_rule: "message-reference",
        },
        Case {
            ftl: "msg = { -brand }\n",
            expected_rule: "rich-text",
        },
        Case {
            ftl: "msg = { { \"nested\" } }\n",
            expected_rule: "rich-text",
        },
        Case {
            ftl: "msg = { NUMBER($val) }\n",
            expected_rule: "function-call",
        },
        Case {
            ftl: "msg = { DATETIME($d, style: \"short\") }\n",
            expected_rule: "function-call",
        },
        Case {
            ftl: "msg = { CALLEE($val) }\n",
            expected_rule: "function-call",
        },
        Case {
            ftl: "msg = Hello\n    .attr = Value\n",
            expected_rule: "attribute",
        },
        Case {
            ftl: "msg = { \"literal\" }\n",
            expected_rule: "literal-placeable",
        },
        Case {
            ftl: "msg = { 42 }\n",
            expected_rule: "literal-placeable",
        },
        Case {
            ftl: "msg = { $count ->\n    [1] one\n   *[other] other\n}\n",
            expected_rule: "exact-number-branch",
        },
        Case {
            ftl: "msg = { $var1 ->\n    [a] { $var2 ->\n        *[x] deep\n    }\n   *[other] other\n}\n",
            expected_rule: "nested-selector",
        },
        Case {
            ftl: "msg = { $var1 ->\n   *[x] one\n} and { $var2 ->\n   *[y] two\n}\n",
            expected_rule: "multiple-selectors",
        },
        Case {
            ftl: "msg = prefix { $var ->\n   *[other] other\n}\n",
            expected_rule: "selector-shape",
        },
        Case {
            ftl: "g@rb@ge = text\n",
            expected_rule: "junk",
        },
        Case {
            ftl: "msg = First\nmsg = Duplicate\n",
            expected_rule: "duplicate-id",
        },
        Case {
            ftl: "msg = { $sel ->\n    [a] one\n    [a] duplicate\n   *[other] def\n}\n",
            expected_rule: "duplicate-branch",
        },
    ];

    for case in cases {
        let res = parse_ftl_resource(case.ftl, "en");
        assert!(
            res.is_err(),
            "Expected error for FTL:\n{}\nGot ok",
            case.ftl
        );
        let faults = res.unwrap_err();
        let expected_msg_id = if case.expected_rule == "junk" || case.expected_rule == "term" {
            "<unparsed>"
        } else {
            "msg"
        };
        assert!(
            faults.iter().any(|f| f.rule == case.expected_rule
                && f.message_id == expected_msg_id
                && f.locale == "en"),
            "Expected fault (rule={}, msg_id={}, locale=en) in faults: {:?} for FTL:\n{}",
            case.expected_rule,
            expected_msg_id,
            faults,
            case.ftl
        );
    }
}

#[test]
fn test_admitted_branch_rules() {
    let mut meta = MessageMetadata {
        description: "d".to_string(),
        context: "c".to_string(),
        bundles: vec!["b".to_string()],
        arguments: vec![ArgumentSpec {
            name: "count".to_string(),
            arg_type: ScalarType::Cardinal,
            numeric_style: Some(NumericStyle::LocaleFormatted),
            min: None,
            max: None,
        }],
        format_intent: FormatIntent::Cardinal,
        max_length: 100,
        source_revision: "a".repeat(64),
        risk: crate::vocab::Risk::Routine,
        auto_merge: false,
        approval: None,
        protected_slots: vec![],
        selector: Some(SelectorSpec {
            name: "count".to_string(),
            selector_type: "cardinal".to_string(),
            enum_values: vec![],
            enum_default: None,
        }),
    };

    // 1. Cardinal default when other is not default
    let mut msg = ParsedMessage {
        id: "test.count".to_string(),
        branches: vec![
            Branch {
                key: Some("one".to_string()),
                default: true,
                pattern: vec![],
            },
            Branch {
                key: Some("other".to_string()),
                default: false,
                pattern: vec![],
            },
        ],
        selector_var: Some("count".to_string()),
    };
    let res = validate_admitted_message(&mut msg, &meta, "en", true, None);
    assert_eq!(res.unwrap_err()[0].rule, "cardinal-default");

    // 2. Enum default mismatch
    meta.format_intent = FormatIntent::Enum;
    meta.selector = Some(SelectorSpec {
        name: "kind".to_string(),
        selector_type: "string".to_string(),
        enum_values: vec!["a".to_string(), "b".to_string()],
        enum_default: Some("a".to_string()),
    });
    meta.arguments = vec![ArgumentSpec {
        name: "kind".to_string(),
        arg_type: ScalarType::String,
        numeric_style: None,
        min: None,
        max: None,
    }];
    let mut enum_msg = ParsedMessage {
        id: "test.enum".to_string(),
        branches: vec![
            Branch {
                key: Some("a".to_string()),
                default: false,
                pattern: vec![],
            },
            Branch {
                key: Some("b".to_string()),
                default: true,
                pattern: vec![],
            },
        ],
        selector_var: Some("kind".to_string()),
    };
    let res = validate_admitted_message(&mut enum_msg, &meta, "en", true, None);
    assert_eq!(res.unwrap_err()[0].rule, "enum-default");

    // 3. Argument parity for unknown placeholder
    let plain_meta = MessageMetadata {
        description: "d".to_string(),
        context: "c".to_string(),
        bundles: vec!["b".to_string()],
        arguments: vec![],
        format_intent: FormatIntent::Text,
        max_length: 100,
        source_revision: "a".repeat(64),
        risk: crate::vocab::Risk::Routine,
        auto_merge: false,
        approval: None,
        protected_slots: vec![],
        selector: None,
    };
    let mut unknown_arg_msg = ParsedMessage {
        id: "test.plain".to_string(),
        branches: vec![Branch {
            key: None,
            default: true,
            pattern: vec![PatternElement {
                kind: ElementKind::Arg,
                value: "unknown".to_string(),
            }],
        }],
        selector_var: None,
    };
    let res = validate_admitted_message(&mut unknown_arg_msg, &plain_meta, "en", true, None);
    assert!(res.unwrap_err().iter().any(|f| f.rule == "argument-parity"));

    // 4. Selector identity when selector variable changes
    let mut wrong_sel_msg = ParsedMessage {
        id: "test.enum".to_string(),
        branches: vec![
            Branch {
                key: Some("a".to_string()),
                default: true,
                pattern: vec![],
            },
            Branch {
                key: Some("b".to_string()),
                default: false,
                pattern: vec![],
            },
        ],
        selector_var: Some("different_var".to_string()),
    };
    let res = validate_admitted_message(&mut wrong_sel_msg, &meta, "en", true, None);
    assert!(
        res.unwrap_err()
            .iter()
            .any(|f| f.rule == "selector-identity")
    );

    // 5. Extra enum branch is enum-branch-set
    let mut extra_enum_msg = ParsedMessage {
        id: "test.enum".to_string(),
        branches: vec![
            Branch {
                key: Some("a".to_string()),
                default: true,
                pattern: vec![],
            },
            Branch {
                key: Some("b".to_string()),
                default: false,
                pattern: vec![],
            },
            Branch {
                key: Some("c".to_string()),
                default: false,
                pattern: vec![],
            },
        ],
        selector_var: Some("kind".to_string()),
    };
    let res = validate_admitted_message(&mut extra_enum_msg, &meta, "en", true, None);
    assert!(res.unwrap_err().iter().any(|f| f.rule == "enum-branch-set"));
}

#[test]
fn test_protected_slot_rules() {
    // 1. slot-arg-disjoint from metadata
    let json = r#"{"description":"d","context":"c","bundles":["b"],"arguments":[{"name":"overlap","type":"string"}],"protected_slots":[{"name":"overlap","kind":"command","value":"val"}],"format_intent":"text","max_length":10,"source_revision":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa","risk":"routine"}"#;
    assert_eq!(
        MessageMetadata::parse("m", json).unwrap_err().rule,
        "slot-arg-disjoint"
    );

    let meta = MessageMetadata {
        description: "d".to_string(),
        context: "c".to_string(),
        bundles: vec!["b".to_string()],
        arguments: vec![ArgumentSpec {
            name: "count".to_string(),
            arg_type: ScalarType::Cardinal,
            numeric_style: Some(NumericStyle::LocaleFormatted),
            min: None,
            max: None,
        }],
        format_intent: FormatIntent::Cardinal,
        max_length: 100,
        source_revision: "a".repeat(64),
        risk: crate::vocab::Risk::Routine,
        auto_merge: false,
        approval: None,
        protected_slots: vec![ProtectedSlotSpec {
            name: "slot1".to_string(),
            kind: ProtectedSlotKind::Command,
            value: "cmd".to_string(),
        }],
        selector: Some(SelectorSpec {
            name: "count".to_string(),
            selector_type: "cardinal".to_string(),
            enum_values: vec![],
            enum_default: None,
        }),
    };

    let src = ParsedMessage {
        id: "test.msg".to_string(),
        branches: vec![
            Branch {
                key: Some("one".to_string()),
                default: false,
                pattern: vec![PatternElement {
                    kind: ElementKind::Slot,
                    value: "slot1".to_string(),
                }],
            },
            Branch {
                key: Some("other".to_string()),
                default: true,
                pattern: vec![PatternElement {
                    kind: ElementKind::Slot,
                    value: "slot1".to_string(),
                }],
            },
        ],
        selector_var: Some("count".to_string()),
    };

    // 2. Dropped slot in translation -> protected-slot-mismatch
    let mut tr_dropped = ParsedMessage {
        id: "test.msg".to_string(),
        branches: vec![
            Branch {
                key: Some("one".to_string()),
                default: false,
                pattern: vec![],
            },
            Branch {
                key: Some("other".to_string()),
                default: true,
                pattern: vec![PatternElement {
                    kind: ElementKind::Slot,
                    value: "slot1".to_string(),
                }],
            },
        ],
        selector_var: Some("count".to_string()),
    };
    let res = validate_admitted_message(&mut tr_dropped, &meta, "en-x-synth", false, Some(&src));
    assert!(
        res.unwrap_err()
            .iter()
            .any(|f| f.rule == "protected-slot-mismatch")
    );

    // 3. Duplicated slot in translation -> protected-slot-mismatch
    let mut tr_dup = ParsedMessage {
        id: "test.msg".to_string(),
        branches: vec![
            Branch {
                key: Some("one".to_string()),
                default: false,
                pattern: vec![
                    PatternElement {
                        kind: ElementKind::Slot,
                        value: "slot1".to_string(),
                    },
                    PatternElement {
                        kind: ElementKind::Slot,
                        value: "slot1".to_string(),
                    },
                ],
            },
            Branch {
                key: Some("other".to_string()),
                default: true,
                pattern: vec![PatternElement {
                    kind: ElementKind::Slot,
                    value: "slot1".to_string(),
                }],
            },
        ],
        selector_var: Some("count".to_string()),
    };
    let res = validate_admitted_message(&mut tr_dup, &meta, "en-x-synth", false, Some(&src));
    assert!(
        res.unwrap_err()
            .iter()
            .any(|f| f.rule == "protected-slot-mismatch")
    );

    // 4. Added cardinal branch whose slot multiset differs from source other -> protected-slot-mismatch
    let mut tr_added_mismatch = ParsedMessage {
        id: "test.msg".to_string(),
        branches: vec![
            Branch {
                key: Some("one".to_string()),
                default: false,
                pattern: vec![PatternElement {
                    kind: ElementKind::Slot,
                    value: "slot1".to_string(),
                }],
            },
            Branch {
                key: Some("few".to_string()),
                default: false,
                pattern: vec![],
            },
            Branch {
                key: Some("other".to_string()),
                default: true,
                pattern: vec![PatternElement {
                    kind: ElementKind::Slot,
                    value: "slot1".to_string(),
                }],
            },
        ],
        selector_var: Some("count".to_string()),
    };
    let res = validate_admitted_message(
        &mut tr_added_mismatch,
        &meta,
        "en-x-synth",
        false,
        Some(&src),
    );
    assert!(
        res.unwrap_err()
            .iter()
            .any(|f| f.rule == "protected-slot-mismatch")
    );

    // 5. Added cardinal branch that matches source other is accepted
    let mut tr_added_ok = ParsedMessage {
        id: "test.msg".to_string(),
        branches: vec![
            Branch {
                key: Some("one".to_string()),
                default: false,
                pattern: vec![PatternElement {
                    kind: ElementKind::Slot,
                    value: "slot1".to_string(),
                }],
            },
            Branch {
                key: Some("few".to_string()),
                default: false,
                pattern: vec![PatternElement {
                    kind: ElementKind::Slot,
                    value: "slot1".to_string(),
                }],
            },
            Branch {
                key: Some("other".to_string()),
                default: true,
                pattern: vec![PatternElement {
                    kind: ElementKind::Slot,
                    value: "slot1".to_string(),
                }],
            },
        ],
        selector_var: Some("count".to_string()),
    };
    let res = validate_admitted_message(&mut tr_added_ok, &meta, "en-x-synth", false, Some(&src));
    assert!(
        res.is_ok(),
        "Added matching cardinal branch should be accepted: {:?}",
        res.err()
    );
}

#[test]
fn test_scalar_bounds_and_boolean() {
    assert!(
        validate_numeric_bounds(
            ScalarType::Cardinal,
            Some(CARDINAL_MIN as f64),
            Some(CARDINAL_MAX as f64)
        )
        .is_ok()
    );
    assert_eq!(
        validate_numeric_bounds(ScalarType::Cardinal, Some((CARDINAL_MIN - 1) as f64), None)
            .unwrap_err(),
        "numeric-range"
    );
    assert_eq!(
        validate_numeric_bounds(ScalarType::Cardinal, None, Some((CARDINAL_MAX + 1) as f64))
            .unwrap_err(),
        "numeric-range"
    );

    assert!(
        validate_numeric_bounds(
            ScalarType::Integer,
            Some(INTEGER_MIN as f64),
            Some(INTEGER_MAX as f64)
        )
        .is_ok()
    );
    assert_eq!(
        validate_numeric_bounds(ScalarType::Integer, Some((INTEGER_MIN - 1) as f64), None)
            .unwrap_err(),
        "numeric-range"
    );
    assert_eq!(
        validate_numeric_bounds(ScalarType::Integer, None, Some((INTEGER_MAX + 1) as f64))
            .unwrap_err(),
        "numeric-range"
    );

    assert_eq!(
        validate_numeric_bounds(ScalarType::Integer, Some(f64::NAN), None).unwrap_err(),
        "numeric-range"
    );
    assert_eq!(
        validate_numeric_bounds(ScalarType::Integer, Some(f64::INFINITY), None).unwrap_err(),
        "numeric-range"
    );
    assert_eq!(
        validate_numeric_bounds(ScalarType::Integer, Some(10.0), Some(5.0)).unwrap_err(),
        "numeric-range"
    );

    // Boolean is not-a-number
    let val_bool = serde_json::Value::Bool(true);
    assert_eq!(
        check_scalar_value(ScalarType::Cardinal, &val_bool).unwrap_err(),
        "not-a-number"
    );
    assert_eq!(
        check_scalar_value(ScalarType::Integer, &val_bool).unwrap_err(),
        "not-a-number"
    );
    assert_eq!(
        validate_numeric_bounds(ScalarType::Boolean, Some(0.0), Some(1.0)).unwrap_err(),
        "not-a-number"
    );

    let json = r#"{"description":"d","context":"c","bundles":["b"],"arguments":[{"name":"n","type":"cardinal","numeric_style":"ungrouped","min":true}],"format_intent":"text","max_length":10,"source_revision":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa","risk":"routine"}"#;
    assert_eq!(
        MessageMetadata::parse("m", json).unwrap_err().rule,
        "not-a-number"
    );
}

#[test]
fn test_metadata_validations() {
    let json = r#"{"description":"d","context":"c","bundles":["b"],"arguments":[],"format_intent":"text","max_length":10,"source_revision":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa","risk":"routine","extra":1}"#;
    assert_eq!(
        MessageMetadata::parse("m", json).unwrap_err().rule,
        "unknown-field"
    );

    let json = r#"{"description":"d","context":"","bundles":["b"],"arguments":[],"format_intent":"text","max_length":10,"source_revision":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa","risk":"routine"}"#;
    assert_eq!(
        MessageMetadata::parse("m", json).unwrap_err().rule,
        "empty-context"
    );

    let json = r#"{"description":"d","context":"c","bundles":[],"arguments":[],"format_intent":"text","max_length":10,"source_revision":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa","risk":"routine"}"#;
    assert_eq!(
        MessageMetadata::parse("m", json).unwrap_err().rule,
        "empty-membership"
    );

    let json = r#"{"description":"d","context":"c","bundles":["b"],"arguments":[],"format_intent":"text","max_length":10,"source_revision":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa","risk":"routine","approval":true}"#;
    assert_eq!(
        MessageMetadata::parse("m", json).unwrap_err().rule,
        "approval-shape"
    );

    let json = r#"{"description":"d","context":"c","bundles":["b"],"arguments":[],"format_intent":"text","max_length":10,"source_revision":"badhash","risk":"routine"}"#;
    assert_eq!(
        MessageMetadata::parse("m", json).unwrap_err().rule,
        "hash-syntax"
    );

    let json = r#"{"description":"d","context":"c","bundles":["b"],"arguments":[],"format_intent":"text","max_length":0,"source_revision":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa","risk":"routine"}"#;
    assert_eq!(
        MessageMetadata::parse("m", json).unwrap_err().rule,
        "max-length"
    );

    for high_risk in &["payment", "legal", "statutory", "consent"] {
        let json = format!(
            r#"{{"description":"d","context":"c","bundles":["b"],"arguments":[],"format_intent":"text","max_length":10,"source_revision":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa","risk":"{}","auto_merge":true}}"#,
            high_risk
        );
        assert_eq!(
            MessageMetadata::parse("m", &json).unwrap_err().rule,
            "auto-merge-ineligible",
            "High risk {} should fail auto-merge-ineligible",
            high_risk
        );
    }
}

#[test]
fn test_locale_channel_eligibility() {
    let reg = LocaleRegistry::embedded().unwrap();
    let bundles = vec!["synth-app".to_string()];

    assert!(
        reg.get_eligible_locales(Channel::Production, &bundles, Some("en-x-synth"))
            .is_ok()
    );

    for loc in &["en", "en-x-draft", "en-x-test", "en-XA", "ar-XB"] {
        assert_eq!(
            reg.get_eligible_locales(Channel::Production, &bundles, Some(loc))
                .unwrap_err()
                .rule,
            "locale-channel",
            "Locale {} should be rejected on production",
            loc
        );
    }
}

#[test]
fn test_duplicate_namespace() {
    let json = r#"{
        "locales": [],
        "namespaces": [
            {"namespace": "test.ns", "owner": "owner1"},
            {"namespace": "test.ns", "owner": "owner2"}
        ],
        "bundles": []
    }"#;
    let res = LocaleRegistry::load(json);
    assert_eq!(res.unwrap_err().rule, "duplicate-namespace");
}

#[test]
fn test_two_emits_are_byte_identical() {
    let fix = fixture_dir();
    let temp_dir = unique_temp_dir("emit");
    let temp1 = temp_dir.join("emit1.json");
    let temp2 = temp_dir.join("emit2.json");

    crate::emit(&fix, Channel::Production, None, &temp1).unwrap();
    crate::emit(&fix, Channel::Production, None, &temp2).unwrap();

    let bytes1 = fs::read(&temp1).unwrap();
    let bytes2 = fs::read(&temp2).unwrap();

    assert_eq!(bytes1, bytes2);
    let _ = fs::remove_dir_all(&temp_dir);
}

#[test]
fn test_ir_roundtrip_preserves_slots() {
    let fix = fixture_dir();
    let temp_dir = unique_temp_dir("slots");
    let temp = temp_dir.join("emit_slots.json");

    crate::emit(&fix, Channel::Production, None, &temp).unwrap();
    let content = fs::read_to_string(&temp).unwrap();
    let ir: serde_json::Value = serde_json::from_str(&content).unwrap();

    let cmd_msg = ir["messages"]
        .as_array()
        .unwrap()
        .iter()
        .find(|m| m["id"] == "synth.notes.command")
        .unwrap();

    let slots = cmd_msg["protected_slots"].as_array().unwrap();
    let cmd_slot = slots.iter().find(|s| s["name"] == "cmd").unwrap();
    assert_eq!(cmd_slot["value"], "  --{%}\" ");

    let gap_slot = slots.iter().find(|s| s["name"] == "gap").unwrap();
    assert_eq!(gap_slot["value"], " \n ");

    let _ = fs::remove_dir_all(&temp_dir);
}

#[test]
fn test_distinct_history_mutation_rules() {
    let fix = fixture_dir();

    // Helper to run update_history on a mutated copy and assert rule and byte preservation
    let assert_history_fault = |mutate: fn(&Path), expected_rule: &str, expected_msg_id: &str| {
        let temp_cat = unique_temp_dir("hist_mut");
        copy_dir_recursive(&fix, &temp_cat);
        let hist_file = temp_cat.join("history.json");

        mutate(&temp_cat);
        let orig_bytes = fs::read(&hist_file).unwrap();

        let res = update_history(&temp_cat);
        assert!(
            res.is_err(),
            "Expected fault {} for msg {}, got ok",
            expected_rule,
            expected_msg_id
        );
        let faults = res.unwrap_err();
        assert!(
            faults
                .iter()
                .any(|f| f.rule == expected_rule && f.message_id == expected_msg_id),
            "Expected fault (rule={}, msg_id={}) in faults: {:?}",
            expected_rule,
            expected_msg_id,
            faults
        );

        let current_bytes = fs::read(&hist_file).unwrap();
        assert_eq!(
            current_bytes, orig_bytes,
            "history.json must not be modified on fault"
        );
        let _ = fs::remove_dir_all(&temp_cat);
    };

    // 1. signature-mutation: change arg name or type
    assert_history_fault(
        |p| {
            let meta_p = p.join("meta").join("synth.notes.count.json");
            let s = fs::read_to_string(&meta_p).unwrap();
            let mut val: serde_json::Value = serde_json::from_str(&s).unwrap();
            val["arguments"][0]["name"] = serde_json::json!("renamed");
            fs::write(&meta_p, serde_json::to_string(&val).unwrap()).unwrap();
        },
        "signature-mutation",
        "synth.notes.count",
    );

    // 2. numeric-range: change min/max only
    assert_history_fault(
        |p| {
            let meta_p = p.join("meta").join("synth.notes.count.json");
            let s = fs::read_to_string(&meta_p).unwrap();
            let mut val: serde_json::Value = serde_json::from_str(&s).unwrap();
            val["arguments"][0]["min"] = serde_json::json!(10);
            fs::write(&meta_p, serde_json::to_string(&val).unwrap()).unwrap();
        },
        "numeric-range",
        "synth.notes.count",
    );

    // 3. format-semantic: format_intent or numeric_style change
    assert_history_fault(
        |p| {
            let meta_p = p.join("meta").join("synth.notes.plain.json");
            let s = fs::read_to_string(&meta_p).unwrap();
            let mut val: serde_json::Value = serde_json::from_str(&s).unwrap();
            val["format_intent"] = serde_json::json!("enum");
            val["selector"] = serde_json::json!({
                "name": "kind",
                "type": "string",
                "enum_values": ["a", "b"],
                "enum_default": "a"
            });
            fs::write(&meta_p, serde_json::to_string(&val).unwrap()).unwrap();
        },
        "format-semantic",
        "synth.notes.plain",
    );

    // 4. selector-mutation: selector name or type change
    assert_history_fault(
        |p| {
            let meta_p = p.join("meta").join("synth.notes.kind.json");
            let s = fs::read_to_string(&meta_p).unwrap();
            let mut val: serde_json::Value = serde_json::from_str(&s).unwrap();
            val["selector"]["name"] = serde_json::json!("renamed_selector");
            fs::write(&meta_p, serde_json::to_string(&val).unwrap()).unwrap();
        },
        "selector-mutation",
        "synth.notes.kind",
    );

    // 5. enum-mutation: enum values or default change
    assert_history_fault(
        |p| {
            let meta_p = p.join("meta").join("synth.notes.kind.json");
            let s = fs::read_to_string(&meta_p).unwrap();
            let mut val: serde_json::Value = serde_json::from_str(&s).unwrap();
            val["selector"]["enum_values"] = serde_json::json!(["box", "bag", "added"]);
            fs::write(&meta_p, serde_json::to_string(&val).unwrap()).unwrap();
        },
        "enum-mutation",
        "synth.notes.kind",
    );

    // 6. protected-slot-mutation: protected slot name, kind, or value change
    assert_history_fault(
        |p| {
            let meta_p = p.join("meta").join("synth.notes.command.json");
            let s = fs::read_to_string(&meta_p).unwrap();
            let mut val: serde_json::Value = serde_json::from_str(&s).unwrap();
            val["protected_slots"][0]["value"] = serde_json::json!("mutated_slot_value");
            fs::write(&meta_p, serde_json::to_string(&val).unwrap()).unwrap();
        },
        "protected-slot-mutation",
        "synth.notes.command",
    );

    // 7. owner-conflict: owner differs from registry
    assert_history_fault(
        |p| {
            let hist_p = p.join("history.json");
            let s = fs::read_to_string(&hist_p).unwrap();
            let mut val: serde_json::Value = serde_json::from_str(&s).unwrap();
            val["records"][0]["owner"] = serde_json::json!("wrong_owner");
            fs::write(&hist_p, serde_json::to_string(&val).unwrap()).unwrap();
        },
        "owner-conflict",
        "synth.notes.command",
    );

    // 8. resource-mismatch: stored resource is not lowercase hex of id
    assert_history_fault(
        |p| {
            let hist_p = p.join("history.json");
            let s = fs::read_to_string(&hist_p).unwrap();
            let mut val: serde_json::Value = serde_json::from_str(&s).unwrap();
            val["records"][0]["resource"] = serde_json::json!("bad_resource_hex");
            fs::write(&hist_p, serde_json::to_string(&val).unwrap()).unwrap();
        },
        "resource-mismatch",
        "synth.notes.command",
    );

    // 9. tombstone-reuse: source id is tombstoned in history
    assert_history_fault(
        |p| {
            let hist_p = p.join("history.json");
            let s = fs::read_to_string(&hist_p).unwrap();
            let mut val: serde_json::Value = serde_json::from_str(&s).unwrap();
            let plain = val["records"]
                .as_array_mut()
                .unwrap()
                .iter_mut()
                .find(|r| r["id"] == "synth.notes.plain")
                .unwrap();
            plain["tombstone"] = serde_json::json!(true);
            fs::write(&hist_p, serde_json::to_string(&val).unwrap()).unwrap();
        },
        "tombstone-reuse",
        "synth.notes.plain",
    );

    // 10. id-reuse: two records share an id
    assert_history_fault(
        |p| {
            let hist_p = p.join("history.json");
            let s = fs::read_to_string(&hist_p).unwrap();
            let mut val: serde_json::Value = serde_json::from_str(&s).unwrap();
            let clone_rec = val["records"][0].clone();
            val["records"].as_array_mut().unwrap().push(clone_rec);
            fs::write(&hist_p, serde_json::to_string(&val).unwrap()).unwrap();
        },
        "id-reuse",
        "synth.notes.command",
    );
}

#[test]
fn test_real_tombstone_reuse_on_compile() {
    let fix = fixture_dir();
    let temp_cat = unique_temp_dir("tomb_compile");
    copy_dir_recursive(&fix, &temp_cat);

    let hist_file = temp_cat.join("history.json");
    let content = fs::read_to_string(&hist_file).unwrap();
    let mut reg: serde_json::Value = serde_json::from_str(&content).unwrap();
    let plain = reg["records"]
        .as_array_mut()
        .unwrap()
        .iter_mut()
        .find(|r| r["id"] == "synth.notes.plain")
        .unwrap();
    plain["tombstone"] = serde_json::json!(true);
    fs::write(&hist_file, serde_json::to_string(&reg).unwrap()).unwrap();

    let res = crate::compile(&temp_cat, Channel::Production, None);
    assert!(res.is_err());
    let faults = res.unwrap_err();
    assert!(
        faults
            .iter()
            .any(|f| f.rule == "tombstone-reuse" && f.message_id == "synth.notes.plain")
    );
    let _ = fs::remove_dir_all(&temp_cat);
}

#[test]
fn test_history_missing() {
    let fix = fixture_dir();
    let temp_cat = unique_temp_dir("hist_miss");
    copy_dir_recursive(&fix, &temp_cat);

    let hist_file = temp_cat.join("history.json");
    fs::remove_file(&hist_file).unwrap();

    let res = crate::compile(&temp_cat, Channel::Production, None);
    assert!(res.is_err());
    let faults = res.unwrap_err();
    assert!(faults.iter().any(|f| f.rule == "history-missing"));
    assert!(
        !hist_file.exists(),
        "history-missing must not create history.json"
    );

    let _ = fs::remove_dir_all(&temp_cat);
}

#[test]
fn test_reorder_independence() {
    let fix = fixture_dir();
    let cat1 = LoadedCatalog::load_and_validate(&fix, Channel::Production, None).unwrap();
    let eligible = vec!["en-x-synth".to_string()];

    let ir1 = build_ir(
        Channel::Production,
        &cat1.config.source_locale,
        &eligible,
        &cat1.source_messages,
        &cat1.metadata,
        &cat1.translations,
        &cat1.registry.get_namespace_owners(),
    );

    let mut modified_meta = cat1.metadata.clone();
    for meta in modified_meta.values_mut() {
        meta.arguments.reverse();
    }

    let ir2 = build_ir(
        Channel::Production,
        &cat1.config.source_locale,
        &eligible,
        &cat1.source_messages,
        &modified_meta,
        &cat1.translations,
        &cat1.registry.get_namespace_owners(),
    );

    let bytes1 = serde_json::to_string(&ir1).unwrap();
    let bytes2 = serde_json::to_string(&ir2).unwrap();
    assert_eq!(
        bytes1, bytes2,
        "Reordering metadata arguments must not change IR"
    );
}

#[test]
fn test_bundle_export_and_import_contracts() {
    let fix = fixture_dir();
    let temp_export = unique_temp_dir("export");
    let _ = fs::remove_dir_all(&temp_export);
    crate::bundle::export_bundle(&fix, &temp_export).unwrap();

    let temp_dest = unique_temp_dir("dest");
    let canary = temp_dest.join("canary.txt");
    fs::write(&canary, b"initial bytes").unwrap();

    // 1. Valid bundle into existing destination returns destination-exists, canary unchanged
    let res = crate::bundle::import_bundle(&temp_export, &temp_dest);
    assert_eq!(res.unwrap_err().rule, "destination-exists");
    assert_eq!(fs::read(&canary).unwrap(), b"initial bytes");

    // 2. Tampered member returns import-refused even if dest exists, canary unchanged
    let temp_tampered = unique_temp_dir("tampered");
    copy_dir_recursive(&temp_export, &temp_tampered);
    fs::write(temp_tampered.join("source").join("en.ftl"), b"tampered").unwrap();
    let res = crate::bundle::import_bundle(&temp_tampered, &temp_dest);
    assert_eq!(res.unwrap_err().rule, "import-refused");
    assert_eq!(fs::read(&canary).unwrap(), b"initial bytes");

    // 3. Absolute path in lock returns import-refused
    let temp_abs = unique_temp_dir("abs");
    copy_dir_recursive(&temp_export, &temp_abs);
    let lock_path = temp_abs.join("import-lock.json");
    let lock_str = fs::read_to_string(&lock_path).unwrap();
    let abs_lock_str = lock_str.replace("catalog.json", "/catalog.json");
    fs::write(&lock_path, abs_lock_str).unwrap();
    let res = crate::bundle::import_bundle(&temp_abs, &temp_dest);
    assert_eq!(res.unwrap_err().rule, "import-refused");
    assert_eq!(fs::read(&canary).unwrap(), b"initial bytes");

    // 4. Dot-dot path in lock returns import-refused
    let temp_dotdot = unique_temp_dir("dotdot");
    copy_dir_recursive(&temp_export, &temp_dotdot);
    let lock_path = temp_dotdot.join("import-lock.json");
    let lock_str = fs::read_to_string(&lock_path).unwrap();
    let dd_lock_str = lock_str.replace("catalog.json", "../catalog.json");
    fs::write(&lock_path, dd_lock_str).unwrap();
    let res = crate::bundle::import_bundle(&temp_dotdot, &temp_dest);
    assert_eq!(res.unwrap_err().rule, "import-refused");
    assert_eq!(fs::read(&canary).unwrap(), b"initial bytes");

    // 5. Symlink in bundle returns import-refused
    #[cfg(unix)]
    {
        let temp_sym = unique_temp_dir("symlink");
        copy_dir_recursive(&temp_export, &temp_sym);
        let sym_path = temp_sym.join("symlink.txt");
        let _ = std::os::unix::fs::symlink(temp_sym.join("catalog.json"), &sym_path);
        let res = crate::bundle::import_bundle(&temp_sym, &temp_dest);
        assert_eq!(res.unwrap_err().rule, "import-refused");
        assert_eq!(fs::read(&canary).unwrap(), b"initial bytes");
        let _ = fs::remove_dir_all(&temp_sym);
    }

    let _ = fs::remove_dir_all(&temp_export);
    let _ = fs::remove_dir_all(&temp_dest);
    let _ = fs::remove_dir_all(&temp_tampered);
    let _ = fs::remove_dir_all(&temp_abs);
    let _ = fs::remove_dir_all(&temp_dotdot);
}

#[test]
fn test_third_party_notices_match_lock() {
    let lock_path = Path::new(env!("CARGO_MANIFEST_DIR")).join("Cargo.lock");
    let lock_content = fs::read_to_string(lock_path).unwrap();
    let notices_path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("licenses")
        .join("NOTICES.md");
    let notices_content = fs::read_to_string(notices_path).unwrap();

    let mut current_name = None;
    for line in lock_content.lines() {
        if line.starts_with("name = ") {
            let name = line.trim_start_matches("name = ").trim_matches('"');
            current_name = Some(name.to_string());
        } else if line.starts_with("version = ")
            && let Some(name) = current_name.take()
            && name != "i18n-catalog"
        {
            let version = line.trim_start_matches("version = ").trim_matches('"');
            let needle = format!("{} {}", name, version);
            assert!(
                notices_content.contains(&needle),
                "NOTICES.md missing package entry: {}",
                needle
            );
        }
    }
}

#[test]
fn test_stale_lock_detection() {
    let fix = fixture_dir();
    let temp_cat = unique_temp_dir("stale_cat");
    copy_dir_recursive(&fix, &temp_cat);

    let src_path = temp_cat.join("source").join("en.ftl");
    let src_content = fs::read_to_string(&src_path).unwrap();
    let modified = src_content.replace("A synthetic sentence.", "A modified sentence.");
    fs::write(&src_path, modified).unwrap();

    let res = crate::compile(&temp_cat, Channel::Production, None);
    assert!(res.is_err());
    let faults = res.unwrap_err();
    assert!(
        faults
            .iter()
            .any(|f| f.rule == "stale" && f.message_id == "synth.notes.plain")
    );

    let _ = fs::remove_dir_all(&temp_cat);
}

#[test]
fn test_missing_catalog_and_memberships() {
    let non_existent = Path::new("/var/tmp/does_not_exist_cat_12345");
    let res = crate::compile(non_existent, Channel::Production, None);
    assert!(res.is_err());
    assert_eq!(res.unwrap_err()[0].rule, "missing-catalog");

    let reg = LocaleRegistry::embedded().unwrap();
    assert_eq!(
        reg.get_eligible_locales(Channel::Production, &[], None)
            .unwrap_err()
            .rule,
        "empty-membership"
    );
    assert_eq!(
        reg.get_eligible_locales(Channel::Production, &["unknown-bundle".to_string()], None)
            .unwrap_err()
            .rule,
        "undeclared-membership"
    );
}

#[test]
fn test_resource_key_fluent_parse() {
    let key = crate::history::compute_resource_key("synth.notes.plain");
    assert!(key.starts_with('m'));
    let ftl = format!("{} = ok\n", key);
    let res = fluent_syntax::parser::parse(ftl.as_str());
    assert!(
        res.is_ok(),
        "Resource key FTL should parse Ok: {:?}",
        res.err()
    );
    let resource = res.unwrap();
    assert!(
        !resource
            .body
            .iter()
            .any(|entry| matches!(entry, fluent_syntax::ast::Entry::Junk { .. })),
        "Resource key FTL should contain no Entry::Junk"
    );
}

#[test]
fn test_encoded_alias_fault() {
    let ftl = "synth__DOT__notes__DOT__plain = Hi.\n";
    let res = parse_ftl_resource(ftl, "en");
    assert!(res.is_err());
    let faults = res.unwrap_err();
    assert!(
        faults.iter().any(|f| f.rule == "encoded-alias"
            && f.message_id == "synth__DOT__notes__DOT__plain"
            && f.locale == "en"),
        "Expected encoded-alias fault, got {:?}",
        faults
    );
}

#[test]
fn test_markup_rules() {
    // <b>rich markup</b> faults
    let res1 = parse_ftl_resource("msg = <b>rich markup</b>\n", "en");
    assert!(res1.is_err());
    assert!(res1.unwrap_err().iter().any(|f| f.rule == "rich-text"));

    // a < b stays text
    let res2 = parse_ftl_resource("msg = a < b\n", "en");
    assert!(res2.is_ok());

    // Protected-slot value with <b> round-trips
    let meta = MessageMetadata {
        description: "d".to_string(),
        context: "c".to_string(),
        bundles: vec!["b".to_string()],
        arguments: vec![],
        format_intent: FormatIntent::Text,
        max_length: 100,
        source_revision: "a".repeat(64),
        risk: crate::vocab::Risk::Routine,
        auto_merge: false,
        approval: None,
        protected_slots: vec![ProtectedSlotSpec {
            name: "markup".to_string(),
            kind: crate::vocab::ProtectedSlotKind::Whitespace,
            value: "<b>".to_string(),
        }],
        selector: None,
    };
    let mut msg = ParsedMessage {
        id: "msg".to_string(),
        branches: vec![Branch {
            key: None,
            default: true,
            pattern: vec![
                PatternElement {
                    kind: ElementKind::Text,
                    value: "Hello ".to_string(),
                },
                PatternElement {
                    kind: ElementKind::Arg,
                    value: "markup".to_string(),
                },
            ],
        }],
        selector_var: None,
    };
    let validate_res = validate_admitted_message(&mut msg, &meta, "en", true, None);
    assert!(validate_res.is_ok());
    assert_eq!(msg.branches[0].pattern[1].kind, ElementKind::Slot);
}

#[test]
fn test_missing_and_unknown_translations() {
    let fix = fixture_dir();
    let temp_cat = unique_temp_dir("missing_trans_cat");
    copy_dir_recursive(&fix, &temp_cat);

    // Delete one message from translations/en-x-synth.ftl -> missing-translation
    let tr_path = temp_cat.join("translations").join("en-x-synth.ftl");
    let tr_content = fs::read_to_string(&tr_path).unwrap();
    let modified_tr = tr_content.replace("synth.notes.plain = SYNTH A synthetic sentence.\n\n", "");
    assert_ne!(tr_content, modified_tr);
    fs::write(&tr_path, modified_tr).unwrap();

    let res = LoadedCatalog::load_and_validate(&temp_cat, Channel::Production, None);
    assert!(res.is_err());
    let faults = res.unwrap_err();
    assert!(
        faults.iter().any(|f| f.rule == "missing-translation"
            && f.locale == "en-x-synth"
            && f.message_id == "synth.notes.plain"),
        "Expected missing-translation fault, got {:?}",
        faults
    );

    // Add an unknown message in translation -> unknown-message
    let unknown_tr = format!("{}\nsynth.notes.unknown = SYNTH Unknown.\n", tr_content);
    fs::write(&tr_path, unknown_tr).unwrap();

    let res2 = LoadedCatalog::load_and_validate(&temp_cat, Channel::Production, None);
    assert!(res2.is_err());
    let faults2 = res2.unwrap_err();
    assert!(
        faults2.iter().any(|f| f.rule == "unknown-message"
            && f.locale == "en-x-synth"
            && f.message_id == "synth.notes.unknown"),
        "Expected unknown-message fault, got {:?}",
        faults2
    );

    let _ = fs::remove_dir_all(&temp_cat);
}

#[test]
fn test_locale_tag_path_traversal() {
    let fix = fixture_dir();
    let temp_cat = unique_temp_dir("locale_tag_cat");
    copy_dir_recursive(&fix, &temp_cat);

    let config_path = temp_cat.join("catalog.json");
    let config_str = fs::read_to_string(&config_path).unwrap();
    let mod_config = config_str.replace(
        "\"source_locale\": \"en\"",
        "\"source_locale\": \"../secret\"",
    );
    fs::write(&config_path, mod_config).unwrap();

    let res = LoadedCatalog::load_and_validate(&temp_cat, Channel::Production, None);
    assert!(res.is_err());
    let faults = res.unwrap_err();
    assert!(
        faults
            .iter()
            .any(|f| f.rule == "locale-tag" && f.locale == "../secret"),
        "Expected locale-tag fault, got {:?}",
        faults
    );

    let _ = fs::remove_dir_all(&temp_cat);
}

#[test]
fn test_unsupported_versions() {
    let fix = fixture_dir();
    let temp_cat = unique_temp_dir("version_cat");
    copy_dir_recursive(&fix, &temp_cat);

    // catalog_version: 2 -> unsupported-version
    let config_path = temp_cat.join("catalog.json");
    let config_str = fs::read_to_string(&config_path).unwrap();
    let mod_config = config_str.replace("\"catalog_version\": 1", "\"catalog_version\": 2");
    fs::write(&config_path, mod_config).unwrap();

    let res = LoadedCatalog::load_and_validate(&temp_cat, Channel::Production, None);
    assert!(res.is_err());
    assert_eq!(res.unwrap_err()[0].rule, "unsupported-version");

    let _ = fs::remove_dir_all(&temp_cat);
}

#[test]
fn test_duplicate_names_in_metadata() {
    // Conflicting argument type duplicate
    let raw_meta_arg = r#"{
        "description": "d",
        "context": "c",
        "bundles": ["synth-app"],
        "arguments": [
            { "name": "val", "type": "string" },
            { "name": "val", "type": "boolean" }
        ],
        "format_intent": "text",
        "max_length": 100,
        "source_revision": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
        "risk": "routine",
        "auto_merge": false
    }"#;
    let res1 = MessageMetadata::parse("synth.notes.test", raw_meta_arg);
    assert!(res1.is_err());
    assert_eq!(res1.unwrap_err().rule, "duplicate-argument");

    // Conflicting slot value duplicate
    let raw_meta_slot = r#"{
        "description": "d",
        "context": "c",
        "bundles": ["synth-app"],
        "arguments": [],
        "format_intent": "text",
        "max_length": 100,
        "source_revision": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
        "risk": "routine",
        "auto_merge": false,
        "protected_slots": [
            { "name": "slot1", "kind": "whitespace", "value": " " },
            { "name": "slot1", "kind": "command", "value": "ls" }
        ]
    }"#;
    let res2 = MessageMetadata::parse("synth.notes.test", raw_meta_slot);
    assert!(res2.is_err());
    assert_eq!(res2.unwrap_err().rule, "duplicate-slot");
}

#[test]
fn test_selector_type_and_intent_checks() {
    // 1. Text with selector -> selector-intent
    let raw_text_sel = r#"{
        "description": "d",
        "context": "c",
        "bundles": ["synth-app"],
        "arguments": [],
        "format_intent": "text",
        "max_length": 100,
        "source_revision": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
        "risk": "routine",
        "auto_merge": false,
        "selector": { "name": "s", "type": "string" }
    }"#;
    let res1 = MessageMetadata::parse("synth.notes.test", raw_text_sel);
    assert!(res1.is_err());
    assert_eq!(res1.unwrap_err().rule, "selector-intent");

    // 2. Cardinal with enum_values -> selector-intent
    let raw_card_ev = r#"{
        "description": "d",
        "context": "c",
        "bundles": ["synth-app"],
        "arguments": [{ "name": "count", "type": "cardinal", "numeric_style": "ungrouped" }],
        "format_intent": "cardinal",
        "max_length": 100,
        "source_revision": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
        "risk": "routine",
        "auto_merge": false,
        "selector": { "name": "count", "type": "cardinal", "enum_values": ["a"] }
    }"#;
    let res2 = MessageMetadata::parse("synth.notes.test", raw_card_ev);
    assert!(res2.is_err());
    assert_eq!(res2.unwrap_err().rule, "selector-intent");

    // 3. Enum with non-member default -> enum-default
    let raw_enum_def = r#"{
        "description": "d",
        "context": "c",
        "bundles": ["synth-app"],
        "arguments": [{ "name": "kind", "type": "string" }],
        "format_intent": "enum",
        "max_length": 100,
        "source_revision": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
        "risk": "routine",
        "auto_merge": false,
        "selector": { "name": "kind", "type": "string", "enum_values": ["a", "b"], "enum_default": "c" }
    }"#;
    let res3 = MessageMetadata::parse("synth.notes.test", raw_enum_def);
    assert!(res3.is_err());
    assert_eq!(res3.unwrap_err().rule, "enum-default");

    // 4. Present enum_values: [1] -> selector-shape
    let raw_bad_ev = r#"{
        "description": "d",
        "context": "c",
        "bundles": ["synth-app"],
        "arguments": [{ "name": "count", "type": "cardinal", "numeric_style": "ungrouped" }],
        "format_intent": "cardinal",
        "max_length": 100,
        "source_revision": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
        "risk": "routine",
        "auto_merge": false,
        "selector": { "name": "count", "type": "cardinal", "enum_values": [1] }
    }"#;
    let res4 = MessageMetadata::parse("synth.notes.test", raw_bad_ev);
    assert!(res4.is_err());
    assert_eq!(res4.unwrap_err().rule, "selector-shape");
}

#[test]
fn test_approval_record_validations() {
    // Missing locale -> approval-shape
    let raw_missing_loc = r#"{
        "description": "d",
        "context": "c",
        "bundles": ["synth-app"],
        "arguments": [],
        "format_intent": "text",
        "max_length": 100,
        "source_revision": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
        "risk": "routine",
        "auto_merge": true,
        "approval": {
            "source_revision": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            "translation_revision": "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"
        }
    }"#;
    let res1 = MessageMetadata::parse("synth.notes.plain", raw_missing_loc);
    assert!(res1.is_err());
    assert_eq!(res1.unwrap_err().rule, "approval-shape");

    // Array locale -> approval-shape
    let raw_array_loc = r#"{
        "description": "d",
        "context": "c",
        "bundles": ["synth-app"],
        "arguments": [],
        "format_intent": "text",
        "max_length": 100,
        "source_revision": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
        "risk": "routine",
        "auto_merge": true,
        "approval": {
            "locale": ["en-x-synth"],
            "source_revision": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            "translation_revision": "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"
        }
    }"#;
    let res2 = MessageMetadata::parse("synth.notes.plain", raw_array_loc);
    assert!(res2.is_err());
    assert_eq!(res2.unwrap_err().rule, "approval-shape");
}

#[test]
fn test_freshness_hashing_collision_resistance_and_idempotence() {
    // 1. Text element ";Arg=name" vs argument element "name"
    let msg_text = ParsedMessage {
        id: "m".to_string(),
        branches: vec![Branch {
            key: None,
            default: true,
            pattern: vec![PatternElement {
                kind: ElementKind::Text,
                value: ";Arg=name".to_string(),
            }],
        }],
        selector_var: None,
    };
    let msg_arg = ParsedMessage {
        id: "m".to_string(),
        branches: vec![Branch {
            key: None,
            default: true,
            pattern: vec![PatternElement {
                kind: ElementKind::Arg,
                value: "name".to_string(),
            }],
        }],
        selector_var: None,
    };
    let hash_text = crate::freshness::compute_source_hash(&msg_text);
    let hash_arg = crate::freshness::compute_source_hash(&msg_arg);
    assert_ne!(
        hash_text, hash_arg,
        "Text element and Arg element must hash differently"
    );

    // 2. Two lock writes of an unchanged catalog are byte-identical
    let fix = fixture_dir();
    let temp_cat = unique_temp_dir("lock_byte_identical");
    copy_dir_recursive(&fix, &temp_cat);

    let cat_str = temp_cat.to_str().unwrap().to_string();
    assert_eq!(
        crate::cli::run(vec![
            "i18n-catalog".into(),
            "lock".into(),
            cat_str.clone().into()
        ]),
        0
    );
    let lock_bytes1 = fs::read(temp_cat.join("source-lock.json")).unwrap();
    assert_eq!(
        crate::cli::run(vec!["i18n-catalog".into(), "lock".into(), cat_str.into()]),
        0
    );
    let lock_bytes2 = fs::read(temp_cat.join("source-lock.json")).unwrap();
    assert_eq!(
        lock_bytes1, lock_bytes2,
        "Two lock writes of unchanged catalog must be byte-identical"
    );

    let _ = fs::remove_dir_all(&temp_cat);
}

#[test]
fn test_freshness_control_flow() {
    let fix = fixture_dir();
    let temp_cat = unique_temp_dir("freshness_ctrl");
    copy_dir_recursive(&fix, &temp_cat);
    let cat_str = temp_cat.to_str().unwrap().to_string();

    // 1. Edit one source line
    let src_path = temp_cat.join("source").join("en.ftl");
    let src_content = fs::read_to_string(&src_path).unwrap();
    let mod_src = src_content.replace("A synthetic sentence.", "A modified sentence.");
    fs::write(&src_path, mod_src).unwrap();

    // 2. Compute new source hash for synth.notes.plain
    let parsed_src =
        crate::admit::parse_ftl_resource(&fs::read_to_string(&src_path).unwrap(), "en").unwrap();
    let plain_msg = parsed_src.messages.get("synth.notes.plain").unwrap();
    let new_src_hash = crate::freshness::compute_source_hash(plain_msg);

    // Update synth.notes.plain.json source_revision and approval source_revision, keep translation_revision unchanged
    let meta_path = temp_cat.join("meta").join("synth.notes.plain.json");
    let meta_str = fs::read_to_string(&meta_path).unwrap();
    let tr_parsed = crate::admit::parse_ftl_resource(
        &fs::read_to_string(temp_cat.join("translations").join("en-x-synth.ftl")).unwrap(),
        "en-x-synth",
    )
    .unwrap();
    let tr_plain = tr_parsed.messages.get("synth.notes.plain").unwrap();
    let tr_hash = crate::freshness::compute_source_hash(tr_plain);

    let mut meta_val: serde_json::Value = serde_json::from_str(&meta_str).unwrap();
    meta_val["source_revision"] = serde_json::Value::String(new_src_hash.clone());
    meta_val["approval"]["source_revision"] = serde_json::Value::String(new_src_hash);
    meta_val["approval"]["translation_revision"] = serde_json::Value::String(tr_hash);
    fs::write(&meta_path, serde_json::to_string_pretty(&meta_val).unwrap()).unwrap();

    // 3. lock refresh (without --acknowledge)
    assert_eq!(
        crate::cli::run(vec![
            "i18n-catalog".into(),
            "lock".into(),
            cat_str.clone().into()
        ]),
        0
    );

    // 4. production load_and_validate faults stale for en-x-synth and synth.notes.plain
    let res = LoadedCatalog::load_and_validate(&temp_cat, Channel::Production, None);
    assert!(res.is_err());
    let faults = res.unwrap_err();
    assert!(
        faults.iter().any(|f| f.rule == "stale"
            && f.locale == "en-x-synth"
            && f.message_id == "synth.notes.plain"),
        "Expected stale fault for en-x-synth and synth.notes.plain, got {:?}",
        faults
    );

    // 5. lock --acknowledge en-x-synth
    assert_eq!(
        crate::cli::run(vec![
            "i18n-catalog".into(),
            "lock".into(),
            cat_str.into(),
            "--acknowledge".into(),
            "en-x-synth".into(),
        ]),
        0
    );

    // 6. load then succeeds
    let res2 = LoadedCatalog::load_and_validate(&temp_cat, Channel::Production, None);
    assert!(
        res2.is_ok(),
        "Expected load_and_validate to succeed after acknowledge, got {:?}",
        res2.err()
    );

    let _ = fs::remove_dir_all(&temp_cat);
}

#[test]
fn test_pseudo_cli_run() {
    let fix = fixture_dir();
    let temp_out = unique_temp_dir("pseudo_out");
    let _ = fs::remove_dir_all(&temp_out); // ensure it doesn't exist yet

    let args = vec![
        "i18n-catalog".into(),
        "pseudo".into(),
        fix.to_str().unwrap().into(),
        "--out".into(),
        temp_out.to_str().unwrap().into(),
    ];
    let exit_code = crate::cli::run(args.clone());
    assert_eq!(exit_code, 0);

    // Assert both files parse Ok with no Junk
    let xa_str = fs::read_to_string(temp_out.join("en-XA.ftl")).unwrap();
    let res_xa = fluent_syntax::parser::parse(xa_str.as_str()).unwrap();
    assert!(
        !res_xa
            .body
            .iter()
            .any(|e| matches!(e, fluent_syntax::ast::Entry::Junk { .. }))
    );

    let xb_str = fs::read_to_string(temp_out.join("ar-XB.ftl")).unwrap();
    let res_xb = fluent_syntax::parser::parse(xb_str.as_str()).unwrap();
    assert!(
        !res_xb
            .body
            .iter()
            .any(|e| matches!(e, fluent_syntax::ast::Entry::Junk { .. }))
    );

    // Assert denominators
    let den_str = fs::read_to_string(temp_out.join("denominators.json")).unwrap();
    assert_eq!(
        den_str,
        "{\"messages_den\":8,\"branches_den\":4,\"slots_den\":6}\n"
    );

    // Second call onto that directory is non-zero and does not change the first tree
    let orig_xa = fs::read(temp_out.join("en-XA.ftl")).unwrap();
    let exit_code2 = crate::cli::run(args);
    assert_ne!(exit_code2, 0);
    let after_xa = fs::read(temp_out.join("en-XA.ftl")).unwrap();
    assert_eq!(orig_xa, after_xa);

    let _ = fs::remove_dir_all(&temp_out);
}
