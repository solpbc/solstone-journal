// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! The removal surface is closed, and stays closed as the crate grows.
//!
//! This crate becomes the only one permitted to remove the owner's media, so the
//! surface that can do it must be one named file rather than a habit. Three
//! properties, asserted over the crate's own source text:
//!
//! 1. no module names a removal or rename primitive -- later waves carve out
//!    exactly one door module, and until then the answer is none;
//! 2. every module declared in `lib.rs` is registered below, so a module added
//!    without registering it cannot become an unguarded removal surface;
//! 3. no module silences `#[must_use]` on an outcome.
//!
//! ⚠ **This test lives in `tests/` rather than in `lib.rs` deliberately.** Its own
//! banned-name literals are source text; inside the crate they would either match
//! themselves or force excluding the file that holds them -- and that file is
//! `lib.rs`, which later waves fill with the verbs. `let _ = remove_file(..)`
//! inside a verb, silencing a failed removal into a claimed one, is the highest
//! value instance of assertion 3, so the file holding the verbs is the last one
//! that may be exempt from it.

/// Every module of the crate, by the name it is declared with in `lib.rs`.
///
/// ⛔ Adding a `mod` to `lib.rs` without adding it here is a test failure. The
/// sibling `segment` crate tolerates a hand-maintained list because its surface
/// is frozen; this crate gains a module in each of the next several waves.
/// The one module permitted to reach a removal primitive.
///
/// ⛔ Per FILE and covering both removal and rename, not per primitive: a later
/// wave adds the staging rename to this same file, and a carve-out written as
/// removal-only would force that wave to edit this test — the crate's structural
/// guarantee — to land.
const DOOR: &str = "door";

const SOURCES: &[(&str, &str)] = &[
    ("content", include_str!("../src/content.rs")),
    ("door", include_str!("../src/door.rs")),
    ("eligibility", include_str!("../src/eligibility.rs")),
    ("receipt", include_str!("../src/receipt.rs")),
    ("tombstone", include_str!("../src/tombstone.rs")),
];

const LIB: &str = include_str!("../src/lib.rs");

/// Primitives that remove or rename a directory entry.
///
/// ⚠ Matched in **call-shaped** form, not as bare words. A bare `"rename"` also
/// matches `#[serde(rename_all = ..)]`, which is an ordinary attribute and not a
/// filesystem operation — the substring-scan hazard this crate's sibling has, found
/// here by the test failing on its own crate. The forms below are how a primitive
/// is actually reached: qualified through `fs::`, or called as a journal-io wrapper.
const REMOVAL_PRIMITIVES: &[&str] = &[
    "fs::remove_file",
    "fs::remove_dir",
    "fs::remove_dir_all",
    "fs::rename",
    "remove_file(",
    "remove_dir_all(",
    "rename_within(",
];

/// Forms that silence `#[must_use]`.
///
/// ⛔ Bare `let _<ident>` is **not** here, and must not be added: `-D
/// unused-variables` requires `let _guard = ..` for an RAII lock guard, which a
/// later wave holds across a removal. Banning it would tell the implementer to
/// break one requirement or the other.
const MUST_USE_BYPASSES: &[&str] = &["let _ = ", "let _=", "_ = "];

/// `drop(x)` also silences `#[must_use]`, but it must be matched as a STATEMENT.
///
/// ⚠ A bare `"drop("` substring also matches `fn drop(&mut self)` in a `Drop`
/// impl, which is ordinary code — the same substring-scan hazard a bare `"rename"`
/// has against `#[serde(rename_all = ..)]`. Both were found by this test failing
/// on its own crate.
fn discards_by_dropping(source: &str) -> Option<usize> {
    source
        .lines()
        .position(|line| line.trim_start().starts_with("drop("))
}

/// The modules `lib.rs` declares, as `mod name;` outside any `cfg(test)` block.
fn declared_modules() -> Vec<String> {
    LIB.lines()
        .map(str::trim)
        .filter_map(|line| {
            let rest = line
                .strip_prefix("pub mod ")
                .or_else(|| line.strip_prefix("mod "))?;
            let name = rest.strip_suffix(';')?;
            (!name.is_empty() && name.chars().all(|c| c.is_alphanumeric() || c == '_'))
                .then(|| name.to_owned())
        })
        .collect()
}

/// The guard must have looked at something.
///
/// Without this, every assertion below is a loop over a possibly-empty list and a
/// green result is indistinguishable from a guard that inspected nothing.
#[test]
fn the_scan_covers_every_declared_module() {
    let declared = declared_modules();
    assert!(
        !declared.is_empty(),
        "parsed no modules from lib.rs -- the parse is broken, not the crate"
    );
    assert!(
        !SOURCES.is_empty(),
        "SOURCES is empty; the scan would be vacuous"
    );

    let registered: Vec<&str> = SOURCES.iter().map(|(name, _)| *name).collect();
    for name in &declared {
        assert!(
            registered.contains(&name.as_str()),
            "module `{name}` is declared in lib.rs but not registered in SOURCES, \
             so nothing scans it for a removal surface"
        );
    }
    assert_eq!(
        declared.len(),
        SOURCES.len(),
        "SOURCES ({registered:?}) and lib.rs ({declared:?}) disagree about the module set"
    );
}

#[test]
fn no_module_names_a_removal_primitive() {
    for (name, source) in SOURCES {
        if *name == DOOR {
            continue;
        }
        for primitive in REMOVAL_PRIMITIVES {
            assert!(
                !source.contains(primitive),
                "module `{name}` names the removal primitive `{primitive}`. \
                 Removal belongs in `{DOOR}` and nowhere else."
            );
        }
    }
    // lib.rs is scanned too: it is where the verbs will live.
    for primitive in REMOVAL_PRIMITIVES {
        assert!(
            !LIB.contains(primitive),
            "lib.rs names the removal primitive `{primitive}`"
        );
    }
}

#[test]
fn no_module_silences_must_use() {
    for (name, source) in SOURCES.iter().chain(std::iter::once(&("lib", LIB))) {
        for bypass in MUST_USE_BYPASSES {
            assert!(
                !source.contains(bypass),
                "module `{name}` contains `{bypass}`, which silences #[must_use] \
                 on an Outcome -- a failed removal would become a claimed one"
            );
        }
        assert!(
            discards_by_dropping(source).is_none(),
            "module `{name}` discards a value with a bare `drop(..)` statement, \
             which silences #[must_use] on an Outcome"
        );
    }
}
