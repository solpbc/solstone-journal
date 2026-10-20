// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use crate::fault::Fault;
use fluent_bundle::{FluentArgs, FluentBundle, FluentResource, FluentValue};
use std::cell::RefCell;

const SELECT_FTL: &str = "\
sel = { $count ->
    [zero] zero
    [one] one
    [two] two
    [few] few
    [many] many
   *[other] other
}
";

const NUMBER_FTL: &str = "num = { NUMBER($val) }\n";

struct OracleBundles {
    en: FluentBundle<FluentResource>,
    ar: FluentBundle<FluentResource>,
}

impl OracleBundles {
    fn new() -> Self {
        let res_en =
            FluentResource::try_new(SELECT_FTL.to_string()).expect("valid select FTL for oracle");
        let res_num_en =
            FluentResource::try_new(NUMBER_FTL.to_string()).expect("valid num FTL for oracle");
        let lang_en = "en".parse().expect("valid en langid");
        let mut bundle_en = FluentBundle::new(vec![lang_en]);
        let _ = bundle_en.add_builtins();
        bundle_en
            .add_resource(res_en)
            .expect("resource adds to en bundle");
        bundle_en
            .add_resource(res_num_en)
            .expect("num resource adds to en bundle");

        let res_ar =
            FluentResource::try_new(SELECT_FTL.to_string()).expect("valid select FTL for oracle");
        let res_num_ar =
            FluentResource::try_new(NUMBER_FTL.to_string()).expect("valid num FTL for oracle");
        let lang_ar = "ar".parse().expect("valid ar langid");
        let mut bundle_ar = FluentBundle::new(vec![lang_ar]);
        let _ = bundle_ar.add_builtins();
        bundle_ar
            .add_resource(res_ar)
            .expect("resource adds to ar bundle");
        bundle_ar
            .add_resource(res_num_ar)
            .expect("num resource adds to ar bundle");

        OracleBundles {
            en: bundle_en,
            ar: bundle_ar,
        }
    }
}

thread_local! {
    static BUNDLES: RefCell<Option<OracleBundles>> = const { RefCell::new(None) };
}

// Note: Arabic in intl_pluralrules 7.0.2 compares the integer operand to 3..=10 and 11..=99 with no % 100.
// That is the pinned fluent-bundle 0.16.0 result (e.g. 101 is other, not one).
// Fluent JS native plurals disagree on large Arabic values (e.g. i32::MIN is many in JS native plurals
// but other in pinned fluent-bundle 0.16.0).
// The JS adapter must use this category function, not @fluent/bundle's select, to choose the branch,
// and then format that branch with Fluent JS.
pub fn eval_fluent_category(cardinal: &str, count: i64) -> Result<&'static str, Fault> {
    BUNDLES.with(|cell| {
        let mut opt = cell.borrow_mut();
        if opt.is_none() {
            *opt = Some(OracleBundles::new());
        }
        let bundles = opt.as_ref().unwrap();
        let bundle = match cardinal {
            "en" => &bundles.en,
            "ar" => &bundles.ar,
            _ => return Err(Fault::catalog("plural-rule")),
        };

        let msg = bundle
            .get_message("sel")
            .ok_or_else(|| Fault::catalog("plural-rule"))?;
        let pattern = msg.value().ok_or_else(|| Fault::catalog("plural-rule"))?;

        let mut args = FluentArgs::new();
        args.set("count", FluentValue::from(count));
        let mut errors = vec![];
        let result = bundle.format_pattern(pattern, Some(&args), &mut errors);
        if !errors.is_empty() {
            return Err(Fault::catalog("plural-rule"));
        }

        match result.as_ref() {
            "zero" => Ok("zero"),
            "one" => Ok("one"),
            "two" => Ok("two"),
            "few" => Ok("few"),
            "many" => Ok("many"),
            "other" => Ok("other"),
            _ => Err(Fault::catalog("plural-rule")),
        }
    })
}

/// Target adapters implement the cardinal rules on wide integers so abs(i32::MIN) cannot overflow.
pub fn adapter_cardinal_category(cardinal: &str, count: i64) -> Result<&'static str, Fault> {
    let v = if count < 0 {
        (count as i128).unsigned_abs()
    } else {
        count as u128
    };

    match cardinal {
        "en" => {
            if v == 1 {
                Ok("one")
            } else {
                Ok("other")
            }
        }
        "ar" => {
            if v == 0 {
                Ok("zero")
            } else if v == 1 {
                Ok("one")
            } else if v == 2 {
                Ok("two")
            } else if (3..=10).contains(&v) {
                Ok("few")
            } else if (11..=99).contains(&v) {
                Ok("many")
            } else {
                Ok("other")
            }
        }
        _ => Err(Fault::catalog("plural-rule")),
    }
}

pub fn verify_oracle_probes() -> Result<(), Fault> {
    const PROBES: &[i64] = &[
        -1,
        0,
        1,
        2,
        3,
        4,
        5,
        10,
        11,
        100,
        i32::MIN as i64,
        i32::MAX as i64,
        10000,
        -9007199254740991_i64,
        9007199254740991_i64,
    ];

    for &val in PROBES {
        let cat_en = eval_fluent_category("en", val)?;
        let cat_ar = eval_fluent_category("ar", val)?;

        let expected_en = if val == 1 || val == -1 {
            "one"
        } else {
            "other"
        };
        if cat_en != expected_en {
            return Err(Fault::catalog("plural-rule"));
        }

        let expected_ar = match val {
            -1 => "one",
            0 => "zero",
            1 => "one",
            2 => "two",
            3..=10 => "few",
            11 => "many",
            _ => "other",
        };
        if cat_ar != expected_ar {
            return Err(Fault::catalog("plural-rule"));
        }
    }

    Ok(())
}

pub fn format_integer_ungrouped(val: i64) -> String {
    format!("{}", val)
}

pub fn format_fluent_number(cardinal: &str, val: i64) -> Result<String, Fault> {
    BUNDLES.with(|cell| {
        let mut opt = cell.borrow_mut();
        if opt.is_none() {
            *opt = Some(OracleBundles::new());
        }
        let bundles = opt.as_ref().unwrap();
        let bundle = match cardinal {
            "en" => &bundles.en,
            "ar" => &bundles.ar,
            _ => return Err(Fault::catalog("plural-rule")),
        };

        let msg = bundle
            .get_message("num")
            .ok_or_else(|| Fault::catalog("plural-rule"))?;
        let pattern = msg.value().ok_or_else(|| Fault::catalog("plural-rule"))?;

        let mut args = FluentArgs::new();
        args.set("val", FluentValue::from(val));
        let mut errors = vec![];
        let result = bundle.format_pattern(pattern, Some(&args), &mut errors);
        if !errors.is_empty() {
            return Err(Fault::catalog("plural-rule"));
        }
        let clean = result.replace(['\u{2068}', '\u{2069}', '\u{2066}', '\u{2067}'], "");
        Ok(clean)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_oracle_probe_verification() {
        assert!(verify_oracle_probes().is_ok());
    }

    #[test]
    fn test_format_integer_ungrouped() {
        assert_eq!(format_integer_ungrouped(10000), "10000");
        assert_eq!(format_integer_ungrouped(-1), "-1");
        assert_eq!(format_integer_ungrouped(i32::MIN as i64), "-2147483648");
        assert_eq!(
            format_integer_ungrouped(9007199254740991_i64),
            "9007199254740991"
        );
    }

    #[test]
    fn test_cardinal_category_grid() {
        let mut probes: Vec<i64> = (-5..=120).collect();
        probes.extend_from_slice(&[
            199,
            200,
            -100,
            i32::MIN as i64,
            i32::MIN as i64 + 1,
            i32::MAX as i64 - 1,
            i32::MAX as i64,
        ]);

        for val in probes {
            let cat_en_adapter = adapter_cardinal_category("en", val).unwrap();
            let cat_en_fluent = eval_fluent_category("en", val).unwrap();
            assert_eq!(cat_en_adapter, cat_en_fluent, "mismatch for en on {}", val);

            let cat_ar_adapter = adapter_cardinal_category("ar", val).unwrap();
            let cat_ar_fluent = eval_fluent_category("ar", val).unwrap();
            assert_eq!(cat_ar_adapter, cat_ar_fluent, "mismatch for ar on {}", val);

            if val == 101 {
                assert_eq!(cat_ar_adapter, "other");
                assert_eq!(cat_ar_fluent, "other");
            }
        }
    }
}
