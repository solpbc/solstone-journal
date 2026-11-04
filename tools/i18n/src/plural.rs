// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! The cardinal plural families, and which families each language may use.
//!
//! Every generated reader selects a branch from this table, so a family's rule
//! is one source: the same expression tokens run here, in the Rust and ESM
//! output and in the gettext header. The pinned `intl_pluralrules` carries CLDR
//! 37, and hosts carry whatever ICU they ship, so neither decides a category.
//! Apple and Android select with their own platform rules.
//!
//! Rules cover integers, the only quantities the portable profile admits, on
//! the magnitude of the count. The language table is checked against
//! `fixtures/plural-goldens/cldr.json`, which comes from a CLDR reader rather
//! than from this file.

// A rule's condition is emitted as text into Rust, JavaScript and gettext, so
// it stays in the form all three read.
#![allow(clippy::manual_range_contains)]

use crate::fault::Fault;

/// The string argument that carries a message's selected category in target FTL.
pub const CATEGORY_ARGUMENT: &str = "i18nCategory";

pub struct CardinalRule {
    pub category: &'static str,
    pub condition: &'static str,
    applies: fn(u64) -> bool,
}

pub struct CardinalFamily {
    pub id: &'static str,
    /// Tried in order. Their index is the gettext plural form.
    pub rules: &'static [CardinalRule],
    /// The category of every other integer, and the last gettext form.
    pub otherwise: &'static str,
}

macro_rules! rule {
    ($category:literal, |$n:ident| $($condition:tt)+) => {
        CardinalRule {
            category: $category,
            condition: stringify!($($condition)+),
            applies: |$n| $($condition)+,
        }
    };
}

pub const FAMILIES: &[CardinalFamily] = &[
    CardinalFamily {
        id: "en",
        rules: &[rule!("one", |n| n == 1)],
        otherwise: "other",
    },
    CardinalFamily {
        id: "ar",
        rules: &[
            rule!("zero", |n| n == 0),
            rule!("one", |n| n == 1),
            rule!("two", |n| n == 2),
            rule!("few", |n| n % 100 >= 3 && n % 100 <= 10),
            rule!("many", |n| n % 100 >= 11),
        ],
        otherwise: "other",
    },
    CardinalFamily {
        id: "other",
        rules: &[],
        otherwise: "other",
    },
    CardinalFamily {
        id: "one-zero",
        rules: &[rule!("one", |n| n == 0 || n == 1)],
        otherwise: "other",
    },
    CardinalFamily {
        id: "one-many",
        rules: &[
            rule!("one", |n| n == 1),
            rule!("many", |n| n != 0 && n % 1000000 == 0),
        ],
        otherwise: "other",
    },
    CardinalFamily {
        id: "one-zero-many",
        rules: &[
            rule!("one", |n| n == 0 || n == 1),
            rule!("many", |n| n != 0 && n % 1000000 == 0),
        ],
        otherwise: "other",
    },
    CardinalFamily {
        id: "east-slavic",
        rules: &[
            rule!("one", |n| n % 10 == 1 && n % 100 != 11),
            rule!("few", |n| n % 10 >= 2
                && n % 10 <= 4
                && (n % 100 < 12 || n % 100 > 14)),
        ],
        otherwise: "many",
    },
    CardinalFamily {
        id: "pl",
        rules: &[
            rule!("one", |n| n == 1),
            rule!("few", |n| n % 10 >= 2
                && n % 10 <= 4
                && (n % 100 < 12 || n % 100 > 14)),
        ],
        otherwise: "many",
    },
    CardinalFamily {
        id: "cs",
        rules: &[rule!("one", |n| n == 1), rule!("few", |n| n >= 2 && n <= 4)],
        otherwise: "other",
    },
    CardinalFamily {
        id: "ro",
        rules: &[
            rule!("one", |n| n == 1),
            rule!("few", |n| n == 0 || (n % 100 >= 1 && n % 100 <= 19)),
        ],
        otherwise: "other",
    },
    CardinalFamily {
        id: "he",
        rules: &[rule!("one", |n| n == 1), rule!("two", |n| n == 2)],
        otherwise: "other",
    },
    CardinalFamily {
        id: "fil",
        rules: &[rule!("one", |n| n % 10 != 4 && n % 10 != 6 && n % 10 != 9)],
        otherwise: "other",
    },
];

/// The families each language may declare. A language with two names two CLDR
/// releases: Vietnamese is other-only through CLDR 48 and takes `one` at 0 and
/// 1 in CLDR main, and the release every reader pins is an open product policy.
/// The first family is the one the goldens hold it to.
pub const LANGUAGES: &[(&str, &[&str])] = &[
    ("am", &["one-zero"]),
    ("ar", &["ar"]),
    ("az", &["en"]),
    ("bn", &["one-zero"]),
    ("cs", &["cs"]),
    ("de", &["en"]),
    ("el", &["en"]),
    ("en", &["en"]),
    ("es", &["one-many"]),
    ("fa", &["one-zero"]),
    ("fil", &["fil"]),
    ("fr", &["one-zero-many"]),
    ("he", &["he"]),
    ("hi", &["one-zero"]),
    ("hu", &["en"]),
    ("hy", &["one-zero"]),
    ("id", &["other"]),
    ("it", &["one-many"]),
    ("ja", &["other"]),
    ("ka", &["en"]),
    ("kk", &["en"]),
    ("km", &["other"]),
    ("ko", &["other"]),
    ("mr", &["en"]),
    ("ms", &["other"]),
    ("my", &["other"]),
    ("nl", &["en"]),
    ("pa", &["one-zero"]),
    ("pl", &["pl"]),
    ("pt", &["one-zero-many"]),
    ("pt-BR", &["one-zero-many"]),
    ("pt-PT", &["one-many"]),
    ("ro", &["ro"]),
    ("ru", &["east-slavic"]),
    ("sv", &["en"]),
    ("sw", &["en"]),
    ("ta", &["en"]),
    ("te", &["en"]),
    ("th", &["other"]),
    ("tr", &["en"]),
    ("uk", &["east-slavic"]),
    ("ur", &["en"]),
    ("vi", &["other", "one-zero"]),
    ("yo", &["other"]),
    ("zh", &["other"]),
];

impl CardinalFamily {
    pub fn category(&self, count: i64) -> &'static str {
        let n = count.unsigned_abs();
        self.rules
            .iter()
            .find(|rule| (rule.applies)(n))
            .map_or(self.otherwise, |rule| rule.category)
    }

    /// Every category this family can select, in gettext form order.
    pub fn categories(&self) -> Vec<&'static str> {
        let mut categories: Vec<_> = self.rules.iter().map(|rule| rule.category).collect();
        categories.push(self.otherwise);
        categories
    }

    /// A branch key that can show: one the family selects, or the required default.
    pub fn can_show(&self, key: &str) -> bool {
        key == "other" || self.categories().contains(&key)
    }

    /// The selection as a Rust expression over `n`.
    pub fn rust_expression(&self) -> String {
        let mut out = String::new();
        for rule in self.rules {
            out.push_str(&format!(
                "if {} {{ \"{}\" }} else ",
                rule.condition, rule.category
            ));
        }
        if self.rules.is_empty() {
            format!("\"{}\"", self.otherwise)
        } else {
            format!("{out}{{ \"{}\" }}", self.otherwise)
        }
    }

    /// The selection as a JavaScript expression over `n`.
    pub fn javascript_expression(&self) -> String {
        let mut out = String::new();
        for rule in self.rules {
            out.push_str(&format!("({}) ? \"{}\" : ", rule.condition, rule.category));
        }
        format!("{out}\"{}\"", self.otherwise)
    }

    pub fn gettext_plural_forms(&self) -> String {
        let mut formula = String::new();
        for (index, rule) in self.rules.iter().enumerate() {
            formula.push_str(&format!("{} ? {} : ", rule.condition, index));
        }
        format!(
            "nplurals={}; plural=({}{});",
            self.rules.len() + 1,
            formula,
            self.rules.len()
        )
    }
}

pub fn family(id: &str) -> Option<&'static CardinalFamily> {
    FAMILIES.iter().find(|family| family.id == id)
}

/// The category an optional family selects. A locale with no family has only
/// the required default.
pub fn category(cardinal: Option<&str>, count: i64) -> Result<&'static str, Fault> {
    match cardinal {
        None => Ok("other"),
        Some(id) => family(id)
            .map(|family| family.category(count))
            .ok_or_else(|| Fault::catalog("plural-rule")),
    }
}

/// The families a locale tag's language may declare, most specific tag first.
pub fn language_families(tag: &str) -> Option<&'static [&'static str]> {
    let lookup = |key: &str| {
        LANGUAGES
            .iter()
            .find(|(language, _)| *language == key)
            .map(|(_, families)| *families)
    };
    lookup(tag).or_else(|| lookup(tag.split('-').next().unwrap_or(tag)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::locale::LocaleRegistry;
    use serde_json::Value;

    struct Goldens {
        probes: Vec<i64>,
        categories: Vec<(String, Vec<String>)>,
    }

    fn goldens() -> Goldens {
        let value: Value =
            serde_json::from_str(include_str!("../fixtures/plural-goldens/cldr.json")).unwrap();
        Goldens {
            probes: value["probes"]
                .as_array()
                .unwrap()
                .iter()
                .map(|probe| probe.as_i64().unwrap())
                .collect(),
            categories: value["categories"]
                .as_object()
                .unwrap()
                .iter()
                .map(|(language, categories)| {
                    (
                        language.clone(),
                        categories
                            .as_str()
                            .unwrap()
                            .split(' ')
                            .map(str::to_owned)
                            .collect(),
                    )
                })
                .collect(),
        }
    }

    fn selects_like(family: &CardinalFamily, goldens: &Goldens, expected: &[String]) -> bool {
        goldens
            .probes
            .iter()
            .zip(expected)
            .all(|(probe, category)| family.category(*probe) == category)
    }

    fn registry_with(tag: &str, cardinal: &str) -> Result<LocaleRegistry, Fault> {
        LocaleRegistry::load(&format!(
            r#"{{"locales":[{{"tag":"{tag}","tier":"release","purpose":"ordinary",
            "development":"released","production":"released","description":"test",
            "cardinal":"{cardinal}"}}],"namespaces":[],"bundles":[]}}"#
        ))
    }

    #[test]
    fn every_language_selects_what_cldr_selects() {
        let goldens = goldens();
        assert_eq!(goldens.categories.len(), LANGUAGES.len());
        for (language, expected) in &goldens.categories {
            let families = language_families(language)
                .unwrap_or_else(|| panic!("{language} has no plural family"));
            let family = family(families[0]).unwrap();
            for (probe, category) in goldens.probes.iter().zip(expected) {
                assert_eq!(
                    family.category(*probe),
                    category,
                    "{language} under {} at {probe}",
                    family.id
                );
                assert_eq!(family.category(-*probe), category, "{language} at -{probe}");
            }
        }
    }

    #[test]
    fn a_language_cannot_borrow_a_family_that_selects_wrongly() {
        let goldens = goldens();
        let mut refused = 0;
        for (language, expected) in &goldens.categories {
            for candidate in FAMILIES {
                let declared = registry_with(language, candidate.id);
                if selects_like(candidate, &goldens, expected) {
                    assert!(declared.is_ok(), "{language} refused {}", candidate.id);
                } else if !language_families(language).unwrap().contains(&candidate.id) {
                    assert_eq!(
                        declared.unwrap_err().rule,
                        "plural-family",
                        "{language} borrowed {}",
                        candidate.id
                    );
                    refused += 1;
                }
            }
        }
        // The compiled-in registry holds the same line for its own rows.
        assert!(refused > 300, "{refused}");
        assert!(LocaleRegistry::embedded().is_ok());
    }

    #[test]
    fn vietnamese_may_declare_either_cldr_reading() {
        assert!(registry_with("vi", "other").is_ok());
        let main = registry_with("vi", "one-zero").unwrap();
        assert_eq!(main.locales[0].cardinal.as_deref(), Some("one-zero"));
        let family = family("one-zero").unwrap();
        assert_eq!(
            [0, 1, 2].map(|n| family.category(n)),
            ["one", "one", "other"]
        );
    }

    #[test]
    fn a_language_with_no_row_and_an_unknown_family_refuse() {
        assert_eq!(registry_with("zz", "en").unwrap_err().rule, "plural-family");
        assert_eq!(registry_with("ru", "fr").unwrap_err().rule, "plural-rule");
        assert_eq!(
            registry_with("zh-Hans", "en").unwrap_err().rule,
            "plural-family"
        );
        assert!(registry_with("zh-Hans", "other").is_ok());
    }

    #[test]
    fn private_use_and_synthetic_tags_may_use_any_family() {
        assert!(registry_with("en-x-plural", "ar").is_ok());
        assert!(registry_with("en-x-slavic", "east-slavic").is_ok());
    }

    #[test]
    fn gettext_forms_follow_the_rule_order() {
        assert_eq!(
            family("en").unwrap().gettext_plural_forms(),
            "nplurals=2; plural=(n == 1 ? 0 : 1);"
        );
        assert_eq!(
            family("other").unwrap().gettext_plural_forms(),
            "nplurals=1; plural=(0);"
        );
        assert_eq!(
            family("east-slavic").unwrap().gettext_plural_forms(),
            "nplurals=3; plural=(n % 10 == 1 && n % 100 != 11 ? 0 : n % 10 >= 2 && n % 10 <= 4 && (n % 100 < 12 || n % 100 > 14) ? 1 : 2);"
        );
    }
}
