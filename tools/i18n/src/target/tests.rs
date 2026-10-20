// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

#[cfg(test)]
#[allow(clippy::module_inception)]
mod tests {
    use crate::catalog::LoadedCatalog;
    use crate::locale::LocaleRegistry;
    use crate::target::cases::TestCase;
    use crate::target::model::{Target, enum_variant_resource_key};
    use crate::target::oracle::eval_fluent_category;
    use crate::target::publish::publish;
    use crate::target::readers::{
        read_android_xml, read_gettext_po, read_webextension_json, read_xcstrings,
    };
    use crate::vocab::Channel;
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::process::Command;

    fn find_fluent_bundle_rlib() -> Option<PathBuf> {
        let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
        let candidates = [
            manifest_dir.join("target/debug/deps"),
            manifest_dir
                .parent()
                .unwrap()
                .parent()
                .unwrap()
                .join("target/debug/deps"),
        ];
        for dir in &candidates {
            if let Ok(entries) = fs::read_dir(dir) {
                for entry in entries.flatten() {
                    let name = entry.file_name().to_string_lossy().to_string();
                    if name.starts_with("libfluent_bundle-") && name.ends_with(".rlib") {
                        return Some(entry.path());
                    }
                }
            }
        }
        None
    }

    fn build_combo_bundle(catalog: &LoadedCatalog) -> crate::target::model::TargetBundle {
        let mut msg_map = std::collections::HashMap::new();
        msg_map.insert(
            "en-x-synth".to_string(),
            crate::admit::ParsedMessage {
                id: "synth.test.combo".to_string(),
                branches: vec![
                    crate::model::Branch {
                        key: Some("box".to_string()),
                        default: true,
                        pattern: vec![
                            crate::model::PatternElement {
                                kind: crate::model::ElementKind::Text,
                                value: "box ".to_string(),
                            },
                            crate::model::PatternElement {
                                kind: crate::model::ElementKind::Arg,
                                value: "name".to_string(),
                            },
                        ],
                    },
                    crate::model::Branch {
                        key: Some("bag".to_string()),
                        default: false,
                        pattern: vec![
                            crate::model::PatternElement {
                                kind: crate::model::ElementKind::Text,
                                value: "bag ".to_string(),
                            },
                            crate::model::PatternElement {
                                kind: crate::model::ElementKind::Arg,
                                value: "name".to_string(),
                            },
                        ],
                    },
                ],
                selector_var: Some("kind".to_string()),
            },
        );

        let combo_msg = crate::target::model::TargetMessage {
            id: "synth.test.combo".to_string(),
            resource_key: "mcombo".to_string(),
            meta: crate::metadata::MessageMetadata {
                description: "Combo test".to_string(),
                context: "Test".to_string(),
                surfaces: vec!["test".to_string()],
                bundles: vec!["combo-app".to_string()],
                arguments: vec![
                    crate::model::ArgumentSpec {
                        name: "kind".to_string(),
                        arg_type: crate::vocab::ScalarType::String,
                        numeric_style: None,
                        min: None,
                        max: None,
                    },
                    crate::model::ArgumentSpec {
                        name: "name".to_string(),
                        arg_type: crate::vocab::ScalarType::String,
                        numeric_style: None,
                        min: None,
                        max: None,
                    },
                ],
                format_intent: crate::vocab::FormatIntent::Enum,
                selector: Some(crate::model::SelectorSpec {
                    name: "kind".to_string(),
                    selector_type: "enum".to_string(),
                    enum_values: vec!["box".to_string(), "bag".to_string()],
                    enum_default: Some("box".to_string()),
                }),
                protected_slots: vec![],
                source_revision: "".to_string(),
                max_length: 100,
                risk: crate::vocab::Risk::Routine,
                auto_merge: false,
                approval: None,
                webextension: false,
            },
            translations: msg_map,
        };

        let loc_entry = catalog
            .registry
            .locales
            .iter()
            .find(|l| l.tag == "en-x-synth")
            .unwrap()
            .clone();

        crate::target::model::TargetBundle {
            id: "combo-app".to_string(),
            locales: vec![loc_entry],
            messages: vec![combo_msg],
        }
    }

    #[test]
    fn test_rust_generated_api_compilation_and_execution() {
        let cat_dir = Path::new("fixtures/targets");
        let catalog =
            LoadedCatalog::load_and_validate(cat_dir, Channel::Development, None).unwrap();
        let out_dir = Path::new("/var/tmp/test_rust_api_out");
        let _ = fs::remove_dir_all(out_dir);
        publish(
            &catalog,
            Channel::Development,
            None,
            &[Target::Rust],
            out_dir,
        )
        .unwrap();

        // Also generate combo bundle to test enum kind followed by string name
        let combo_bundle = build_combo_bundle(&catalog);
        crate::target::rust::generate_rust_target(&combo_bundle, out_dir).unwrap();

        let rlib = find_fluent_bundle_rlib().expect("fluent_bundle rlib must exist in deps");
        let bundle_dir = out_dir.join("rust/target-operator");

        let runner_src = r#"
mod api;
use api::*;

fn main() {
    // 1. Reversed args
    let r1 = render_dotted("en-x-synth", "synth.notes.span", &[("count", Value::Cardinal(10000))]).unwrap();
    assert_eq!(r1, "tr span other \u{2068}10000\u{2069}");

    // 2. Boolean
    let r2 = render_dotted("en-x-synth", "synth.notes.gate", &[("open", Value::Boolean(true))]).unwrap();
    assert_eq!(r2, "tr gate \u{2068}true\u{2069}");

    // 3. Enum kind selects on kind
    let r3 = render_dotted("en-x-synth", "synth.notes.choice", &[("kind", Value::String("bag"))]).unwrap();
    assert_eq!(r3, "kind-bag");

    // 4. MessageId
    let r4 = render("en-x-synth", MessageId::m73796e74682e6e6f7465732e6c6162656c, &[("name", Value::String("Alice"))]).unwrap();
    assert_eq!(r4, "tr label \u{2068}Alice\u{2069}");

    // 5. Unknown dotted id
    let err_dotted = render_dotted("en-x-synth", "unknown.dotted.id", &[]).unwrap_err();
    assert_eq!(err_dotted, RenderError::UnknownMessage);

    // 6. Wrong type
    let err_type = render_dotted("en-x-synth", "synth.notes.label", &[("name", Value::Integer(42))]).unwrap_err();
    assert_eq!(err_type, RenderError::InvalidArgumentType("name".to_string()));

    // 7. Out of range
    let err_range = render_dotted("en-x-synth", "synth.notes.group", &[("n", Value::Integer(9999999999999999))]).unwrap_err();
    assert_eq!(err_range, RenderError::OutOfRange("n".to_string()));

    // 8. Global bounds when min/max are absent
    let ok_bound = render_dotted("en-x-synth", "synth.notes.wide", &[("n", Value::Integer(9007199254740991))]).unwrap();
    assert!(ok_bound.contains("9007199254740991"));
    let err_bound = render_dotted("en-x-synth", "synth.notes.wide", &[("n", Value::Integer(9007199254740992))]).unwrap_err();
    assert_eq!(err_bound, RenderError::OutOfRange("n".to_string()));

    // 9. {x} isolated by U+2068 and U+2069
    assert!(r1.contains('\u{2068}'));
    assert!(r1.contains('\u{2069}'));
}
"#;

        let runner_rs = bundle_dir.join("main.rs");
        fs::write(&runner_rs, runner_src.as_bytes()).unwrap();

        let bin_path = bundle_dir.join("test_bin");
        let deps_dir = rlib.parent().unwrap();
        let compile_status = Command::new("rustc")
            .arg("--edition=2024")
            .arg("-L")
            .arg(deps_dir)
            .arg("--extern")
            .arg(format!("fluent_bundle={}", rlib.display()))
            .arg("-o")
            .arg(&bin_path)
            .arg(&runner_rs)
            .status()
            .expect("Failed to invoke rustc");
        assert!(compile_status.success(), "rustc compilation failed");

        let run_status = Command::new(&bin_path)
            .status()
            .expect("Failed to execute test binary");
        assert!(run_status.success(), "test binary execution failed");

        // Compile and run combo bundle test
        let combo_dir = out_dir.join("rust/combo-app");
        let combo_runner_src = r#"
mod api;
use api::*;

fn main() {
    // Enum kind followed by string name (selects on kind)
    let r1 = render_dotted("en-x-synth", "synth.test.combo", &[("kind", Value::String("bag")), ("name", Value::String("Alice"))]).unwrap();
    assert_eq!(r1, "bag \u{2068}Alice\u{2069}");

    // Reversed args: string name followed by enum kind
    let r2 = render_dotted("en-x-synth", "synth.test.combo", &[("name", Value::String("Alice")), ("kind", Value::String("bag"))]).unwrap();
    assert_eq!(r2, "bag \u{2068}Alice\u{2069}");
}
"#;
        let combo_runner_rs = combo_dir.join("main.rs");
        fs::write(&combo_runner_rs, combo_runner_src.as_bytes()).unwrap();
        let combo_bin_path = combo_dir.join("test_bin");
        let compile_combo = Command::new("rustc")
            .arg("--edition=2024")
            .arg("-L")
            .arg(deps_dir)
            .arg("--extern")
            .arg(format!("fluent_bundle={}", rlib.display()))
            .arg("-o")
            .arg(&combo_bin_path)
            .arg(&combo_runner_rs)
            .status()
            .expect("Failed to invoke rustc for combo");
        assert!(
            compile_combo.success(),
            "rustc compilation failed for combo"
        );

        let run_combo = Command::new(&combo_bin_path)
            .status()
            .expect("Failed to execute combo test binary");
        assert!(run_combo.success(), "combo test binary execution failed");

        let _ = fs::remove_dir_all(out_dir);
    }

    #[test]
    fn test_variant_keys_not_history() {
        let parent = "mchoice";
        let k_box_cap = enum_variant_resource_key(parent, "Box");
        let k_box_low = enum_variant_resource_key(parent, "box");
        let k_in_bag_hyphen = enum_variant_resource_key(parent, "in-bag");
        let k_in_bag_under = enum_variant_resource_key(parent, "in_bag");

        let mut set = std::collections::HashSet::new();
        set.insert(k_box_cap.clone());
        set.insert(k_box_low.clone());
        set.insert(k_in_bag_hyphen.clone());
        set.insert(k_in_bag_under.clone());
        assert_eq!(set.len(), 4, "Four variant keys must be distinct");

        let history_content = fs::read_to_string("fixtures/targets/history.json").unwrap();
        assert!(!history_content.contains(&k_box_cap));
        assert!(!history_content.contains(&k_box_low));
        assert!(!history_content.contains(&k_in_bag_hyphen));
        assert!(!history_content.contains(&k_in_bag_under));
    }

    #[test]
    fn test_direct_fluent_bundle_read() {
        let cat_dir = Path::new("fixtures/targets");
        let catalog =
            LoadedCatalog::load_and_validate(cat_dir, Channel::Development, None).unwrap();
        let out_dir = Path::new("/var/tmp/test_fluent_read_out");
        let _ = fs::remove_dir_all(out_dir);
        publish(
            &catalog,
            Channel::Development,
            None,
            &[Target::Rust],
            out_dir,
        )
        .unwrap();

        let ftl_path = out_dir.join("rust/target-operator/en-x-synth.ftl");
        let content = fs::read_to_string(&ftl_path).unwrap();
        let res = fluent_bundle::FluentResource::try_new(content).expect("FTL must parse");
        let lang = "en".parse().unwrap();
        let mut bundle = fluent_bundle::FluentBundle::new(vec![lang]);
        bundle
            .add_resource(res)
            .expect("Resource must add to bundle");

        let msg = bundle
            .get_message("m73796e74682e6e6f7465732e616e63686f72")
            .expect("anchor msg");
        let pattern = msg.value().expect("msg value");
        let mut errs = vec![];
        let val = bundle.format_pattern(pattern, None, &mut errs);
        assert!(errs.is_empty());
        assert_eq!(val, "tr-synth-anchor");

        let _ = fs::remove_dir_all(out_dir);
    }

    #[test]
    fn test_rust_local_collision_control() {
        let cat_dir = Path::new("fixtures/targets");
        let catalog =
            LoadedCatalog::load_and_validate(cat_dir, Channel::Development, None).unwrap();
        let out_dir = Path::new("/var/tmp/test_rust_local_collision_out");
        let _ = fs::remove_dir_all(out_dir);

        let branches = vec![
            crate::model::Branch {
                key: Some("Box".to_string()),
                default: true,
                pattern: vec![
                    crate::model::PatternElement {
                        kind: crate::model::ElementKind::Text,
                        value: "Box branch ".to_string(),
                    },
                    crate::model::PatternElement {
                        kind: crate::model::ElementKind::Arg,
                        value: "a_b".to_string(),
                    },
                ],
            },
            crate::model::Branch {
                key: Some("box".to_string()),
                default: false,
                pattern: vec![
                    crate::model::PatternElement {
                        kind: crate::model::ElementKind::Text,
                        value: "box branch ".to_string(),
                    },
                    crate::model::PatternElement {
                        kind: crate::model::ElementKind::Arg,
                        value: "a_b".to_string(),
                    },
                ],
            },
            crate::model::Branch {
                key: Some("in-bag".to_string()),
                default: false,
                pattern: vec![
                    crate::model::PatternElement {
                        kind: crate::model::ElementKind::Text,
                        value: "in-bag branch ".to_string(),
                    },
                    crate::model::PatternElement {
                        kind: crate::model::ElementKind::Arg,
                        value: "a_b".to_string(),
                    },
                ],
            },
            crate::model::Branch {
                key: Some("in_bag".to_string()),
                default: false,
                pattern: vec![
                    crate::model::PatternElement {
                        kind: crate::model::ElementKind::Text,
                        value: "in_bag branch ".to_string(),
                    },
                    crate::model::PatternElement {
                        kind: crate::model::ElementKind::Arg,
                        value: "a_b".to_string(),
                    },
                ],
            },
        ];

        let mut translations = std::collections::HashMap::new();
        translations.insert(
            "en-x-synth".to_string(),
            crate::admit::ParsedMessage {
                id: "synth.test.shadow".to_string(),
                branches,
                selector_var: Some("a-b".to_string()),
            },
        );

        let message = crate::target::model::TargetMessage {
            id: "synth.test.shadow".to_string(),
            resource_key: "mshadow".to_string(),
            meta: crate::metadata::MessageMetadata {
                description: "Shadow control".to_string(),
                context: "Test".to_string(),
                surfaces: vec!["test".to_string()],
                bundles: vec!["shadow-app".to_string()],
                arguments: vec![
                    crate::model::ArgumentSpec {
                        name: "a-b".to_string(),
                        arg_type: crate::vocab::ScalarType::String,
                        numeric_style: None,
                        min: None,
                        max: None,
                    },
                    crate::model::ArgumentSpec {
                        name: "a_b".to_string(),
                        arg_type: crate::vocab::ScalarType::String,
                        numeric_style: None,
                        min: None,
                        max: None,
                    },
                ],
                format_intent: crate::vocab::FormatIntent::Enum,
                selector: Some(crate::model::SelectorSpec {
                    name: "a-b".to_string(),
                    selector_type: "enum".to_string(),
                    enum_values: vec![
                        "Box".to_string(),
                        "box".to_string(),
                        "in-bag".to_string(),
                        "in_bag".to_string(),
                    ],
                    enum_default: Some("Box".to_string()),
                }),
                protected_slots: vec![],
                source_revision: "".to_string(),
                max_length: 100,
                risk: crate::vocab::Risk::Routine,
                auto_merge: false,
                approval: None,
                webextension: false,
            },
            translations,
        };

        let loc_entry = catalog
            .registry
            .locales
            .iter()
            .find(|l| l.tag == "en-x-synth")
            .unwrap()
            .clone();

        let bundle = crate::target::model::TargetBundle {
            id: "shadow-app".to_string(),
            locales: vec![loc_entry],
            messages: vec![message],
        };

        // Assert 4 _v{hex} ids in generated FTL and absent from history.json
        let ftl = crate::target::ftl::generate_bundle_ftl(&bundle, "en-x-synth").unwrap();
        let history_str = fs::read_to_string("fixtures/targets/history.json").unwrap();
        let k_box_cap = enum_variant_resource_key("mshadow", "Box");
        let k_box_low = enum_variant_resource_key("mshadow", "box");
        let k_in_bag_dash = enum_variant_resource_key("mshadow", "in-bag");
        let k_in_bag_under = enum_variant_resource_key("mshadow", "in_bag");

        for key in [&k_box_cap, &k_box_low, &k_in_bag_dash, &k_in_bag_under] {
            assert!(
                ftl.contains(key),
                "Generated FTL must contain variant key {}",
                key
            );
            assert!(
                !history_str.contains(key),
                "history.json must not contain variant key {}",
                key
            );
        }

        // Generate Rust API and compile with rustc
        crate::target::rust::generate_rust_target(&bundle, out_dir).unwrap();
        let app_dir = out_dir.join("rust/shadow-app");
        let rlib = find_fluent_bundle_rlib().expect("fluent_bundle rlib must exist in deps");

        let runner_src = r#"
mod api;
use api::*;

fn main() {
    let rendered = render_dotted("en-x-synth", "synth.test.shadow", &[("a-b", Value::String("box")), ("a_b", Value::String("kept"))]).unwrap();
    assert!(rendered.contains("box branch"), "Expected 'box branch', got: {}", rendered);
    assert!(rendered.contains("kept"), "Expected 'kept', got: {}", rendered);
    assert!(!rendered.contains("Box branch"), "Must not select default 'Box branch'");
}
"#;
        let runner_rs = app_dir.join("main.rs");
        fs::write(&runner_rs, runner_src.as_bytes()).unwrap();

        let bin_path = app_dir.join("test_bin");
        let deps_dir = rlib.parent().unwrap();
        let compile_status = Command::new("rustc")
            .arg("--edition=2024")
            .arg("-L")
            .arg(deps_dir)
            .arg("--extern")
            .arg(format!("fluent_bundle={}", rlib.display()))
            .arg("-o")
            .arg(&bin_path)
            .arg(&runner_rs)
            .status()
            .expect("Failed to invoke rustc");
        assert!(
            compile_status.success(),
            "rustc compilation failed for shadow control"
        );

        let run_status = Command::new(&bin_path)
            .status()
            .expect("Failed to execute shadow control binary");
        assert!(run_status.success(), "shadow control test execution failed");

        let _ = fs::remove_dir_all(out_dir);
    }

    #[test]
    fn test_node_api_mjs() {
        let cat_dir = Path::new("fixtures/targets");
        let catalog =
            LoadedCatalog::load_and_validate(cat_dir, Channel::Development, None).unwrap();
        let out_dir = Path::new("/var/tmp/test_node_api_out");
        let _ = fs::remove_dir_all(out_dir);
        publish(
            &catalog,
            Channel::Development,
            None,
            &[Target::Esm],
            out_dir,
        )
        .unwrap();

        // Verify private-use tag with locale-formatted faults locale-formatted at generate time
        let loc_synth = catalog
            .registry
            .locales
            .iter()
            .find(|l| l.tag == "en-x-synth")
            .unwrap()
            .clone();

        let formatted_bundle_private = crate::target::model::TargetBundle {
            id: "target-formatted-private".to_string(),
            locales: vec![loc_synth],
            messages: vec![crate::target::model::TargetMessage {
                id: "synth.test.formatted".to_string(),
                resource_key: "mformatted".to_string(),
                meta: crate::metadata::MessageMetadata {
                    description: "Formatted test".to_string(),
                    context: "Test".to_string(),
                    surfaces: vec!["test".to_string()],
                    bundles: vec!["target-formatted-private".to_string()],
                    arguments: vec![crate::model::ArgumentSpec {
                        name: "n".to_string(),
                        arg_type: crate::vocab::ScalarType::Integer,
                        numeric_style: Some(crate::vocab::NumericStyle::LocaleFormatted),
                        min: None,
                        max: None,
                    }],
                    format_intent: crate::vocab::FormatIntent::Text,
                    selector: None,
                    protected_slots: vec![],
                    source_revision: "".to_string(),
                    max_length: 100,
                    risk: crate::vocab::Risk::Routine,
                    auto_merge: false,
                    approval: None,
                    webextension: false,
                },
                translations: {
                    let mut map = std::collections::HashMap::new();
                    map.insert(
                        "en-x-synth".to_string(),
                        crate::admit::ParsedMessage {
                            id: "synth.test.formatted".to_string(),
                            branches: vec![crate::model::Branch {
                                key: None,
                                default: true,
                                pattern: vec![
                                    crate::model::PatternElement {
                                        kind: crate::model::ElementKind::Text,
                                        value: "formatted ".to_string(),
                                    },
                                    crate::model::PatternElement {
                                        kind: crate::model::ElementKind::Arg,
                                        value: "n".to_string(),
                                    },
                                ],
                            }],
                            selector_var: None,
                        },
                    );
                    map
                },
            }],
        };

        let esm_private_err = crate::target::esm::generate_esm_target(
            &formatted_bundle_private,
            out_dir,
            Path::new("vendor/fluent-js"),
        )
        .unwrap_err();
        assert!(
            esm_private_err
                .iter()
                .any(|f| f.rule == "locale-formatted" && f.locale == "en-x-synth")
        );
        assert!(!out_dir.join("esm/target-formatted-private").exists());

        // Construct standard formatting bundle with en-IN and en
        let loc_en_in = catalog
            .registry
            .locales
            .iter()
            .find(|l| l.tag == "en-IN")
            .unwrap()
            .clone();
        let loc_en = catalog
            .registry
            .locales
            .iter()
            .find(|l| l.tag == "en")
            .unwrap()
            .clone();

        let formatted_bundle = crate::target::model::TargetBundle {
            id: "target-formatted".to_string(),
            locales: vec![loc_en_in, loc_en],
            messages: vec![crate::target::model::TargetMessage {
                id: "synth.test.formatted".to_string(),
                resource_key: "mformatted".to_string(),
                meta: crate::metadata::MessageMetadata {
                    description: "Formatted test".to_string(),
                    context: "Test".to_string(),
                    surfaces: vec!["test".to_string()],
                    bundles: vec!["target-formatted".to_string()],
                    arguments: vec![crate::model::ArgumentSpec {
                        name: "n".to_string(),
                        arg_type: crate::vocab::ScalarType::Integer,
                        numeric_style: Some(crate::vocab::NumericStyle::LocaleFormatted),
                        min: None,
                        max: None,
                    }],
                    format_intent: crate::vocab::FormatIntent::Text,
                    selector: None,
                    protected_slots: vec![],
                    source_revision: "".to_string(),
                    max_length: 100,
                    risk: crate::vocab::Risk::Routine,
                    auto_merge: false,
                    approval: None,
                    webextension: false,
                },
                translations: {
                    let mut map = std::collections::HashMap::new();
                    let parsed = crate::admit::ParsedMessage {
                        id: "synth.test.formatted".to_string(),
                        branches: vec![crate::model::Branch {
                            key: None,
                            default: true,
                            pattern: vec![
                                crate::model::PatternElement {
                                    kind: crate::model::ElementKind::Text,
                                    value: "formatted ".to_string(),
                                },
                                crate::model::PatternElement {
                                    kind: crate::model::ElementKind::Arg,
                                    value: "n".to_string(),
                                },
                            ],
                        }],
                        selector_var: None,
                    };
                    map.insert("en-IN".to_string(), parsed.clone());
                    map.insert("en".to_string(), parsed);
                    map
                },
            }],
        };

        // Assert generate_rust_target returns locale-formatted
        let rust_err =
            crate::target::rust::generate_rust_target(&formatted_bundle, out_dir).unwrap_err();
        assert!(
            rust_err
                .iter()
                .any(|f| f.rule == "locale-formatted" && f.message_id == "synth.test.formatted")
        );
        assert!(!out_dir.join("rust/target-formatted").exists());

        crate::target::esm::generate_esm_target(
            &formatted_bundle,
            out_dir,
            Path::new("vendor/fluent-js"),
        )
        .unwrap();

        // Check no http:// or https:// in generated ESM vendor files
        let esm_vendor = out_dir.join("esm/target-operator/vendor");
        if esm_vendor.exists() {
            for entry in fs::read_dir(&esm_vendor).unwrap().flatten() {
                if entry.path().is_file() {
                    let content = fs::read_to_string(entry.path()).unwrap();
                    for line in content.lines() {
                        assert!(
                            !line.contains("http://") && !line.contains("https://"),
                            "Found URL in {:?}",
                            entry.path()
                        );
                    }
                }
            }
        }

        // Check generated api.mjs contains no node: import and no useIsolating: false
        let api_files = [
            out_dir.join("esm/target-operator/api.mjs"),
            out_dir.join("esm/target-plural/api.mjs"),
            out_dir.join("esm/target-formatted/api.mjs"),
        ];
        for f in &api_files {
            let code = fs::read_to_string(f).unwrap();
            assert!(!code.contains("node:"), "File {:?} contains node:", f);
            assert!(
                !code.contains("useIsolating: false"),
                "File {:?} contains useIsolating: false",
                f
            );
        }

        let script_path = out_dir.join("test_runner.mjs");
        let script_code = r#"
import { render as renderOp } from "./esm/target-operator/api.mjs";
import { render as renderPlural } from "./esm/target-plural/api.mjs";
import { render as renderFormatted } from "./esm/target-formatted/api.mjs";
import { CATALOGS } from "./esm/target-plural/catalogs.mjs";
import { FluentBundle, FluentResource } from "./esm/vendor/fluent-bundle/index.js";
import assert from "node:assert";

// 1. Reversed name renders with U+2068 and U+2069 around the value
const r1 = renderOp("en-x-synth", "synth.notes.label", [["name", "Alice"]]);
assert.strictEqual(r1, "tr label \u2068Alice\u2069");

// 2. Ungrouped 10000 contains the digits 10000 and no grouping character
const rGroup = renderOp("en-x-synth", "synth.notes.group", [["n", 10000]]);
assert(rGroup.includes("10000"), "Ungrouped 10000 must contain 10000");
assert(!rGroup.includes(","), "Ungrouped 10000 must have no grouping comma");
assert(!rGroup.includes("."), "Ungrouped 10000 must have no grouping dot");

// 3. A constructed locale-formatted integer 10000000 in en-IN and en
const rIn = renderFormatted("en-IN", "synth.test.formatted", [["n", 10000000]]);
assert(rIn.includes("1,00,00,000"), `en-IN must format 10000000 as 1,00,00,000, got ${rIn}`);

const rEn = renderFormatted("en", "synth.test.formatted", [["n", 10000000]]);
assert(rEn.includes("10,000,000"), `en must format 10000000 as 10,000,000, got ${rEn}`);

// 4. Arabic count 3 on synth.notes.forms contains tr-other (omitted few falls through to that locale's other)
const rAr3 = renderPlural("ar", "synth.notes.forms", [["count", 3]]);
assert(rAr3.includes("tr-other"), "Arabic count 3 should fall through to tr-other");

// 5. Arabic -2147483648 contains the digits -2147483648. Compare that render to a direct FluentBundle.formatPattern of the same embedded FTL. Do not assert it equals Rust other or tr-other.
const rArMin = renderPlural("ar", "synth.notes.forms", [["count", -2147483648]]);
assert(rArMin.includes("-2147483648"), "Arabic min i32 must contain digits -2147483648");

const directResource = new FluentResource(CATALOGS["ar"]);
const directBundle = new FluentBundle("ar");
directBundle.addResource(directResource);
const directMsg = directBundle.getMessage("m73796e74682e6e6f7465732e666f726d73");
const directVal = directBundle.formatPattern(directMsg.value, { count: -2147483648 });
assert.strictEqual(rArMin, directVal, "Arabic min i32 render must match direct FluentBundle.formatPattern");

// 6. Unmatched enum unknown selects kind-box
const rDef = renderOp("en-x-synth", "synth.notes.choice", [["kind", "unknown"]]);
assert.strictEqual(rDef, "kind-box");

// 7. 1.5 for an integer throws
assert.throws(() => {
    renderOp("en-x-synth", "synth.notes.group", [["n", 1.5]]);
});

// Also assert NaN, Infinity, and out-of-range bounds throw
assert.throws(() => {
    renderOp("en-x-synth", "synth.notes.group", [["n", NaN]]);
});
assert.throws(() => {
    renderOp("en-x-synth", "synth.notes.group", [["n", Infinity]]);
});

// 8. Generated api.mjs contains no node: import and no useIsolating: false (verified above)
"#;
        fs::write(&script_path, script_code.as_bytes()).unwrap();

        let status = Command::new("node")
            .arg(&script_path)
            .status()
            .expect("Failed to invoke node");
        assert!(status.success(), "Node test runner failed");

        let _ = fs::remove_dir_all(out_dir);
    }

    #[test]
    fn test_own_readers_and_webextension() {
        let cat_dir = Path::new("fixtures/targets");
        let catalog =
            LoadedCatalog::load_and_validate(cat_dir, Channel::Development, None).unwrap();
        let out_dir = Path::new("/var/tmp/test_own_readers_out");
        let _ = fs::remove_dir_all(out_dir);
        let all_targets = [
            Target::Rust,
            Target::Esm,
            Target::Apple,
            Target::Android,
            Target::Webextension,
            Target::Gettext,
        ];
        publish(&catalog, Channel::Development, None, &all_targets, out_dir).unwrap();

        let cases_path = out_dir.join("cases.json");
        let cases_str = fs::read_to_string(&cases_path).unwrap();
        let cases_json: serde_json::Value = serde_json::from_str(&cases_str).unwrap();
        let cases_arr = cases_json["cases"].as_array().unwrap();

        let apple_dir = out_dir.join("apple");
        let android_dir = out_dir.join("android");
        let webext_dir = out_dir.join("webextension");
        let gettext_dir = out_dir.join("gettext");

        for c in cases_arr {
            let case = TestCase {
                target: c["target"].as_str().unwrap().to_string(),
                bundle: c["bundle"].as_str().unwrap().to_string(),
                locale: c["locale"].as_str().unwrap().to_string(),
                folder: c["folder"].as_str().map(|s| s.to_string()),
                message_id: c["message_id"].as_str().unwrap().to_string(),
                resource_key: c["resource_key"].as_str().unwrap().to_string(),
                adapter_call: c["adapter_call"].as_str().unwrap().to_string(),
                inputs: c["inputs"].as_object().unwrap().clone(),
                expected: c["expected"].as_str().unwrap().to_string(),
                own_reader: c["own_reader"].as_str().unwrap().to_string(),
                platform_reader: c["platform_reader"].as_str().unwrap().to_string(),
            };

            assert_eq!(case.platform_reader, "pending");

            match case.target.as_str() {
                "apple" => {
                    let actual = read_xcstrings(&apple_dir, &case).unwrap();
                    assert_eq!(
                        actual, case.expected,
                        "Failed apple case: {:?}",
                        case.adapter_call
                    );
                }
                "android" => {
                    let actual = read_android_xml(&android_dir, &case).unwrap();
                    assert_eq!(
                        actual, case.expected,
                        "Failed android case: {:?}",
                        case.adapter_call
                    );
                }
                "webextension" => {
                    assert_eq!(case.folder.as_deref(), Some("en"));
                    let actual = read_webextension_json(&webext_dir, &case).unwrap();
                    assert_eq!(
                        actual, case.expected,
                        "Failed webextension case: {:?}",
                        case.adapter_call
                    );
                }
                "gettext" => {
                    let actual = read_gettext_po(&gettext_dir, &case).unwrap();
                    assert_eq!(
                        actual, case.expected,
                        "Failed gettext case: {:?}",
                        case.adapter_call
                    );
                }
                _ => {}
            }
        }

        // WebExtension inspection:
        let messages_json_path =
            out_dir.join("webextension/target-chrome/_locales/en/messages.json");
        let msg_content = fs::read_to_string(&messages_json_path).unwrap();
        let msg_obj: serde_json::Value = serde_json::from_str(&msg_content).unwrap();
        let chrome_entry = &msg_obj["m73796e74682e6e6f7465732e6368726f6d65"];
        let raw_message = chrome_entry["message"].as_str().unwrap();
        assert!(!raw_message.contains("$$NAME$$"));
        assert!(raw_message.contains("$var_name$"));
        let ph_content = chrome_entry["placeholders"]["var_name"]["content"]
            .as_str()
            .unwrap();
        assert_eq!(ph_content, "$$NAME$$");

        // Android qualifiers inspection:
        assert!(
            out_dir
                .join("android/target-operator/values-en-rXA/strings.xml")
                .exists()
        );
        assert!(
            out_dir
                .join("android/target-operator/values-en-rXB/strings.xml")
                .exists()
        );
        assert!(
            out_dir
                .join("android/target-operator/values-en/strings.xml")
                .exists()
        );
        assert!(
            out_dir
                .join("android/target-plural/values-ar/strings.xml")
                .exists()
        );

        for entry in fs::read_dir(out_dir.join("android/target-operator"))
            .unwrap()
            .flatten()
        {
            let name = entry.file_name().to_string_lossy().to_string();
            assert!(
                !name.contains("b+"),
                "Android must not emit b+ folder: {}",
                name
            );
        }

        // Lone % storage inspection:
        let po_content =
            fs::read_to_string(out_dir.join("gettext/target-operator/en-x-synth.po")).unwrap();
        assert!(po_content.contains("msgctxt \"synth.notes.percent\""));
        assert!(po_content.contains("msgstr \"%%\""));
        assert!(!po_content.contains("msgstr \"%\"\n"));

        let xcstrings_content =
            fs::read_to_string(out_dir.join("apple/target-operator.xcstrings")).unwrap();
        let xc_json: serde_json::Value = serde_json::from_str(&xcstrings_content).unwrap();
        let percent_res_key = crate::history::compute_resource_key("synth.notes.percent");
        assert_eq!(
            xc_json["strings"][&percent_res_key]["localizations"]["en-x-synth"]["stringUnit"]["value"],
            "%%"
        );

        let android_xml_content =
            fs::read_to_string(out_dir.join("android/target-operator/values-en-rXA/strings.xml"))
                .unwrap();
        assert!(
            android_xml_content
                .contains(&format!("<string name=\"{}\">%%</string>", percent_res_key))
        );
        assert!(
            !android_xml_content
                .contains(&format!("<string name=\"{}\">%</string>", percent_res_key))
        );

        let _ = fs::remove_dir_all(out_dir);
    }

    #[test]
    fn test_anchors() {
        let cat_dir = Path::new("fixtures/targets");
        let catalog =
            LoadedCatalog::load_and_validate(cat_dir, Channel::Development, None).unwrap();
        let out_dir = Path::new("/var/tmp/test_anchors_out");
        let _ = fs::remove_dir_all(out_dir);
        publish(
            &catalog,
            Channel::Development,
            None,
            &[Target::Rust],
            out_dir,
        )
        .unwrap();

        let cases_path = out_dir.join("cases.json");
        let cases_str = fs::read_to_string(&cases_path).unwrap();
        let cases_json: serde_json::Value = serde_json::from_str(&cases_str).unwrap();
        let cases_arr = cases_json["cases"].as_array().unwrap();

        let mut found_percent = false;
        let mut found_10000 = false;
        let mut found_min_i32 = false;
        let mut found_ar_3_tr_other = false;
        let mut found_kind_box = false;
        let mut found_shell_slots = false;

        for c in cases_arr {
            let msg_id = c["message_id"].as_str().unwrap();
            let exp = c["expected"].as_str().unwrap();
            let loc = c["locale"].as_str().unwrap();

            if msg_id == "synth.notes.percent" && exp == "%" {
                found_percent = true;
            }
            if msg_id == "synth.notes.group" && exp == "tr group 10000" {
                found_10000 = true;
            }
            if msg_id == "synth.notes.span" && exp.contains("-2147483648") {
                found_min_i32 = true;
            }
            if msg_id == "synth.notes.forms"
                && loc == "ar"
                && c["inputs"]["count"] == 3
                && exp == "tr-other 3"
            {
                found_ar_3_tr_other = true;
            }
            if msg_id == "synth.notes.choice" && exp == "kind-box" {
                found_kind_box = true;
            }
            if msg_id == "synth.notes.shell"
                && exp.contains(" \n\t ")
                && exp.contains(" --flag={val}% ")
                && exp.contains("--opt")
                && exp.contains("TOKEN")
                && exp.contains("https://example.invalid/shell")
                && exp.contains("MARK")
            {
                found_shell_slots = true;
            }
        }

        assert!(found_percent, "Anchor %");
        assert!(found_10000, "Anchor 10000");
        assert!(found_min_i32, "Anchor -2147483648");
        assert!(
            found_ar_3_tr_other,
            "Anchor tr-other for Arabic few-count 3"
        );
        assert!(found_kind_box, "Anchor kind-box");
        assert!(found_shell_slots, "Anchor protected slots");

        let _ = fs::remove_dir_all(out_dir);
    }

    #[test]
    fn test_publish_safety_matrix_and_canary() {
        let cat_dir = Path::new("fixtures/targets");
        let catalog =
            LoadedCatalog::load_and_validate(cat_dir, Channel::Development, None).unwrap();

        // 1. Canary in unmarked directory
        let unmarked_dir = Path::new("/var/tmp/test_safety_unmarked_dir");
        let _ = fs::remove_dir_all(unmarked_dir);
        fs::create_dir_all(unmarked_dir).unwrap();
        let canary_file = unmarked_dir.join("canary.txt");
        fs::write(&canary_file, b"canary data").unwrap();

        let err = publish(
            &catalog,
            Channel::Development,
            None,
            &[Target::Rust],
            unmarked_dir,
        )
        .unwrap_err();
        assert_eq!(err[0].rule, "destination-unmarked");
        assert!(canary_file.exists(), "Canary file must survive");
        assert_eq!(fs::read(&canary_file).unwrap(), b"canary data");
        let _ = fs::remove_dir_all(unmarked_dir);

        // 2. Symlink destination
        let real_dir = Path::new("/var/tmp/test_safety_real_dir");
        let _ = fs::remove_dir_all(real_dir);
        fs::create_dir_all(real_dir).unwrap();
        let symlink_dest = Path::new("/var/tmp/test_safety_symlink_dir");
        let _ = fs::remove_file(symlink_dest);
        std::os::unix::fs::symlink(real_dir, symlink_dest).unwrap();

        let err_link = publish(
            &catalog,
            Channel::Development,
            None,
            &[Target::Rust],
            symlink_dest,
        )
        .unwrap_err();
        assert_eq!(err_link[0].rule, "destination-link");
        assert!(
            symlink_dest
                .symlink_metadata()
                .unwrap()
                .file_type()
                .is_symlink()
        );
        let _ = fs::remove_file(symlink_dest);
        let _ = fs::remove_dir_all(real_dir);

        // 3. Overlap destination
        let err_overlap_same = publish(
            &catalog,
            Channel::Development,
            None,
            &[Target::Rust],
            cat_dir,
        )
        .unwrap_err();
        assert_eq!(err_overlap_same[0].rule, "destination-overlap");
        let sub_dest = cat_dir.join("sub_out");
        let err_overlap_sub = publish(
            &catalog,
            Channel::Development,
            None,
            &[Target::Rust],
            &sub_dest,
        )
        .unwrap_err();
        assert_eq!(err_overlap_sub[0].rule, "destination-overlap");

        // 4. Marker directory replaced and production drops excluded locales
        let out_dir = Path::new("/var/tmp/test_safety_marker_replace_out");
        let _ = fs::remove_dir_all(out_dir);

        publish(
            &catalog,
            Channel::Development,
            None,
            &[Target::Rust],
            out_dir,
        )
        .unwrap();
        assert!(out_dir.join("i18n-catalog-targets").exists());
        assert!(out_dir.join("rust/target-plural/ar.ftl").exists());

        let catalog_prod =
            LoadedCatalog::load_and_validate(cat_dir, Channel::Production, None).unwrap();
        publish(
            &catalog_prod,
            Channel::Production,
            None,
            &[Target::Rust],
            out_dir,
        )
        .unwrap();
        assert!(out_dir.join("i18n-catalog-targets").exists());
        assert!(!out_dir.join("rust/target-plural").exists());

        let _ = fs::remove_dir_all(out_dir);
    }

    #[test]
    fn test_matrix_and_channel_exclusion() {
        let registry = LocaleRegistry::embedded().unwrap();
        let b_a = registry
            .bundles
            .iter()
            .find(|b| b.id == "target-a")
            .unwrap();
        assert_eq!(b_a.locales, vec!["en-x-l1", "en-x-l2"]);
        let b_b = registry
            .bundles
            .iter()
            .find(|b| b.id == "target-b")
            .unwrap();
        assert_eq!(b_b.locales, vec!["en-x-l2", "en-x-l3"]);

        let l_test = registry
            .locales
            .iter()
            .find(|l| l.tag == "en-x-test")
            .unwrap();
        assert!(l_test.is_eligible_for_channel(Channel::Development));
        assert!(!l_test.is_eligible_for_channel(Channel::Production));

        let l_ar = registry.locales.iter().find(|l| l.tag == "ar").unwrap();
        assert!(l_ar.is_eligible_for_channel(Channel::Development));
        assert!(!l_ar.is_eligible_for_channel(Channel::Production));
    }

    #[test]
    fn test_cardinal_category_grid_eval() {
        assert_eq!(eval_fluent_category("en", 1).unwrap(), "one");
        assert_eq!(eval_fluent_category("en", -1).unwrap(), "one");
        assert_eq!(eval_fluent_category("en", 0).unwrap(), "other");
        assert_eq!(eval_fluent_category("en", 2).unwrap(), "other");

        assert_eq!(eval_fluent_category("ar", 0).unwrap(), "zero");
        assert_eq!(eval_fluent_category("ar", 1).unwrap(), "one");
        assert_eq!(eval_fluent_category("ar", -1).unwrap(), "one");
        assert_eq!(eval_fluent_category("ar", 2).unwrap(), "two");
        assert_eq!(eval_fluent_category("ar", -2).unwrap(), "two");

        for n in 3..=10 {
            assert_eq!(eval_fluent_category("ar", n).unwrap(), "few");
        }
        for n in 11..=99 {
            assert_eq!(eval_fluent_category("ar", n).unwrap(), "many");
        }
        for n in [100, 101, 102, 199, 200] {
            assert_eq!(eval_fluent_category("ar", n).unwrap(), "other");
        }
        assert_eq!(
            eval_fluent_category("ar", i32::MIN as i64).unwrap(),
            "other"
        );
    }

    #[test]
    fn test_android_qualifier_validation_and_resolution() {
        use crate::locale::{LocaleEntry, validate_android_qualifier};
        use crate::target::android::resolve_android_qualifier;
        use crate::target::model::TargetBundle;

        // validate_android_qualifier
        assert!(validate_android_qualifier("en"));
        assert!(validate_android_qualifier("ar"));
        assert!(validate_android_qualifier("eng"));
        assert!(validate_android_qualifier("en-rXA"));
        assert!(validate_android_qualifier("en-rXB"));
        assert!(validate_android_qualifier("en-rUS"));
        assert!(validate_android_qualifier("es-rES"));

        assert!(!validate_android_qualifier("en-x-synth"));
        assert!(!validate_android_qualifier("b+en"));
        assert!(!validate_android_qualifier("values-en"));
        assert!(!validate_android_qualifier("en-rus"));
        assert!(!validate_android_qualifier("EN"));
        assert!(!validate_android_qualifier("en-rUSA"));
        assert!(!validate_android_qualifier(""));
        assert!(!validate_android_qualifier("en-r"));

        // LocaleRegistry::load rejects invalid present android_qualifier
        let invalid_json = r#"{
            "locales": [{
                "tag": "en-x-synth",
                "tier": "release",
                "purpose": "ordinary",
                "development": "released",
                "production": "released",
                "android_qualifier": "invalid_qualifier",
                "description": "test"
            }],
            "namespaces": [],
            "bundles": []
        }"#;
        let err = LocaleRegistry::load(invalid_json).unwrap_err();
        assert_eq!(err.rule, "locale-qualifier");

        // resolve_android_qualifier
        let reg = LocaleRegistry::embedded().unwrap();
        let synth_loc = reg.locales.iter().find(|l| l.tag == "en-x-synth").unwrap();
        assert_eq!(resolve_android_qualifier(synth_loc).unwrap(), "en-rXA");

        let chrome_loc = reg.locales.iter().find(|l| l.tag == "en-x-chrome").unwrap();
        assert_eq!(resolve_android_qualifier(chrome_loc).unwrap(), "en-rXB");

        let en_loc = reg.locales.iter().find(|l| l.tag == "en").unwrap();
        assert_eq!(resolve_android_qualifier(en_loc).unwrap(), "en");

        let ar_loc = reg.locales.iter().find(|l| l.tag == "ar").unwrap();
        assert_eq!(resolve_android_qualifier(ar_loc).unwrap(), "ar");

        let unresolvable = LocaleEntry {
            tag: "en-x-custom".to_string(),
            tier: crate::vocab::LocaleTier::Release,
            purpose: crate::vocab::LocalePurpose::Ordinary,
            development: crate::vocab::ReleaseState::Released,
            production: crate::vocab::ReleaseState::Released,
            description: "test".to_string(),
            cardinal: None,
            webextension_folder: None,
            android_qualifier: None,
        };
        let err_unres = resolve_android_qualifier(&unresolvable).unwrap_err();
        assert_eq!(err_unres.rule, "locale-qualifier");

        // Collision detection in generate_android_target
        let mut loc_dup1 = unresolvable.clone();
        loc_dup1.tag = "en-x-d1".to_string();
        loc_dup1.android_qualifier = Some("en-rXA".to_string());
        let mut loc_dup2 = unresolvable.clone();
        loc_dup2.tag = "en-x-d2".to_string();
        loc_dup2.android_qualifier = Some("en-rXA".to_string());

        let bundle = TargetBundle {
            id: "collision-app".to_string(),
            locales: vec![loc_dup1, loc_dup2],
            messages: vec![],
        };
        let out_dir = Path::new("/var/tmp/test_android_collision_out");
        let _ = fs::remove_dir_all(out_dir);
        let err_col =
            crate::target::android::generate_android_target(&bundle, out_dir).unwrap_err();
        assert_eq!(err_col[0].rule, "locale-qualifier");
        assert!(
            !out_dir.exists(),
            "Target directory must not be created on collision"
        );
    }

    #[test]
    fn test_apple_ungrouped_numbers_and_cardinal_contract() {
        use crate::metadata::MessageMetadata;
        use crate::model::{ElementKind, PatternElement};
        use crate::target::model::{Target, TargetMessage};
        use crate::vocab::{FormatIntent, NumericStyle, ScalarType};

        // Message with string argument 'name' then cardinal argument 'count'
        let mut meta = MessageMetadata {
            description: "Contract test".to_string(),
            context: "test".to_string(),
            surfaces: vec![],
            bundles: vec!["apple-app".to_string()],
            arguments: vec![
                crate::model::ArgumentSpec {
                    name: "name".to_string(),
                    arg_type: ScalarType::String,
                    numeric_style: None,
                    min: None,
                    max: None,
                },
                crate::model::ArgumentSpec {
                    name: "count".to_string(),
                    arg_type: ScalarType::Cardinal,
                    numeric_style: Some(NumericStyle::Ungrouped),
                    min: None,
                    max: None,
                },
            ],
            format_intent: FormatIntent::Cardinal,
            max_length: 100,
            source_revision: "rev".to_string(),
            risk: crate::vocab::Risk::Routine,
            auto_merge: false,
            approval: None,
            protected_slots: vec![],
            selector: Some(crate::model::SelectorSpec {
                name: "count".to_string(),
                selector_type: "cardinal".to_string(),
                enum_values: vec![],
                enum_default: None,
            }),
            webextension: false,
        };

        let msg = TargetMessage {
            id: "test.apple.contract".to_string(),
            resource_key: "mapplecontract".to_string(),
            meta: meta.clone(),
            translations: std::collections::HashMap::new(),
        };

        // Render template for branch: name is %1$@, count is selector so branch display is %3$@ (N=2, N+1=3)
        let branch_elements = vec![
            PatternElement {
                kind: ElementKind::Arg,
                value: "name".to_string(),
            },
            PatternElement {
                kind: ElementKind::Text,
                value: " has ".to_string(),
            },
            PatternElement {
                kind: ElementKind::Arg,
                value: "count".to_string(),
            },
            PatternElement {
                kind: ElementKind::Text,
                value: " items".to_string(),
            },
        ];

        let tmpl =
            crate::target::apple::render_apple_template(&branch_elements, &msg, true).unwrap();
        assert_eq!(tmpl, "%1$@ has %3$@ items");

        // Format adapter call: name is "Alice", count is 5 -> extra display arg "5" appended at end
        let mut inputs = serde_json::Map::new();
        inputs.insert("name".to_string(), serde_json::json!("Alice"));
        inputs.insert("count".to_string(), serde_json::json!(5));

        let call = crate::target::cases::format_adapter_call(
            Target::Apple,
            "apple-app",
            "en",
            &msg,
            &inputs,
        );
        assert_eq!(
            call,
            "String(format:locale:arguments: \"mapplecontract\", \"Alice\", 5, \"5\")"
        );

        // Test ungrouped integer in text intent
        meta.format_intent = FormatIntent::Text;
        meta.selector = None;
        meta.arguments = vec![crate::model::ArgumentSpec {
            name: "n".to_string(),
            arg_type: ScalarType::Integer,
            numeric_style: Some(NumericStyle::Ungrouped),
            min: None,
            max: None,
        }];
        let int_msg = TargetMessage {
            id: "test.apple.ungrouped".to_string(),
            resource_key: "mungrouped".to_string(),
            meta: meta.clone(),
            translations: std::collections::HashMap::new(),
        };
        let int_elements = vec![PatternElement {
            kind: ElementKind::Arg,
            value: "n".to_string(),
        }];
        let tmpl_int =
            crate::target::apple::render_apple_template(&int_elements, &int_msg, false).unwrap();
        assert_eq!(tmpl_int, "%1$@");

        let mut int_inputs = serde_json::Map::new();
        int_inputs.insert("n".to_string(), serde_json::json!(2147483648_i64));
        let int_call = crate::target::cases::format_adapter_call(
            Target::Apple,
            "apple-app",
            "en",
            &int_msg,
            &int_inputs,
        );
        assert_eq!(
            int_call,
            "String(format:locale:arguments: \"mungrouped\", \"2147483648\")"
        );

        // Test locale-formatted integer
        meta.arguments[0].numeric_style = Some(NumericStyle::LocaleFormatted);
        let loc_msg = TargetMessage {
            id: "test.apple.locale_int".to_string(),
            resource_key: "mlocaleint".to_string(),
            meta: meta.clone(),
            translations: std::collections::HashMap::new(),
        };
        let tmpl_loc =
            crate::target::apple::render_apple_template(&int_elements, &loc_msg, false).unwrap();
        assert_eq!(tmpl_loc, "%1$lld");
        let loc_call = crate::target::cases::format_adapter_call(
            Target::Apple,
            "apple-app",
            "en",
            &loc_msg,
            &int_inputs,
        );
        assert_eq!(
            loc_call,
            "String(format:locale:arguments: \"mlocaleint\", 2147483648)"
        );
    }
}
