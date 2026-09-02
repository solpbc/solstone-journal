// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Windows environment merging, ordinal ordering, and block construction.

use std::cmp::Ordering;
use std::ffi::{OsStr, OsString};

use thiserror::Error;

const NUL: u16 = 0;
const EQUALS: u16 = b'=' as u16;

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub(super) enum WindowsEnvironmentError {
    #[error("Windows environment key is empty")]
    EmptyKey,
    #[error("Windows environment key is invalid")]
    InvalidKey,
    #[error("Windows environment values cannot contain an interior NUL")]
    InteriorNul,
    #[error("Windows environment overrides collide under ordinal comparison")]
    OverrideCollision,
    #[error("Windows ordinal comparison failed")]
    OrdinalComparison,
    #[error("Windows inherited environment lookup failed: {0}")]
    InheritedEnvironment(String),
    #[cfg(windows)]
    #[error("Windows wide-string encoding failed: {0}")]
    WideEncoding(String),
}

pub(super) type WindowsOrdinalResult<T> = Result<T, WindowsEnvironmentError>;
pub(super) type WindowsEnvironmentSourceResult<T> = Result<T, WindowsEnvironmentError>;
pub(super) type WindowsWideResult<T> = Result<T, WindowsEnvironmentError>;

/// Windows's version-defined, ordinal ignore-case environment-key comparison.
pub(super) trait WindowsOrdinalCompare {
    fn compare_ignore_case(&self, left: &[u16], right: &[u16]) -> WindowsOrdinalResult<Ordering>;
}

/// A snapshot of the environment inherited by the eventual child.
pub(super) trait InheritedWindowsEnvironment {
    fn snapshot(&self) -> WindowsEnvironmentSourceResult<Vec<(OsString, OsString)>>;
}

/// An explicit helper environment starts from no inherited parent variables.
///
/// This is deliberately a source adapter, rather than a special case in the
/// environment merger, so the same duplicate and UTF-16 validation applies to
/// every child environment shape.
#[cfg(windows)]
pub(super) struct EmptyInheritedWindowsEnvironment;

#[cfg(windows)]
impl InheritedWindowsEnvironment for EmptyInheritedWindowsEnvironment {
    fn snapshot(&self) -> WindowsEnvironmentSourceResult<Vec<(OsString, OsString)>> {
        Ok(Vec::new())
    }
}

/// The sole lossless `OsString` to UTF-16 conversion boundary.
pub(super) trait WindowsWideEncoder {
    fn encode_wide(&self, value: &OsStr) -> WindowsWideResult<Vec<u16>>;
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct EnvironmentEntry {
    key: Vec<u16>,
    value: Vec<u16>,
}

/// Fully prepared child environment plus the two PATH search sources.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct WindowsEnvironmentPlan {
    pub(super) block: Vec<u16>,
    pub(super) child_path: Option<Vec<u16>>,
    pub(super) parent_path: Option<Vec<u16>>,
}

/// Merge inherited Windows environment entries with caller overrides.
///
/// Derived from Rust 1.97.1 `sys/process/windows.rs::{EnvKey, make_envp}`.
/// The explicit block and injected ordinal adapter are derived for owned, Linux-testable
/// launch preparation.
pub(super) fn prepare_environment(
    overrides: &std::collections::BTreeMap<OsString, OsString>,
    ordinal: &dyn WindowsOrdinalCompare,
    inherited: &dyn InheritedWindowsEnvironment,
    encoder: &dyn WindowsWideEncoder,
) -> Result<WindowsEnvironmentPlan, WindowsEnvironmentError> {
    let mut merged = Vec::new();
    for (key, value) in inherited.snapshot()? {
        let entry = encode_entry(&key, &value, false, encoder)?;
        upsert_entry(&mut merged, entry, ordinal, false)?;
    }
    let parent_path = find_path(&merged, ordinal)?;

    let mut caller_entries = Vec::new();
    for (key, value) in overrides {
        let entry = encode_entry(key, value, true, encoder)?;
        if caller_entries
            .iter()
            .any(|existing| keys_equal(existing, &entry, ordinal).unwrap_or(false))
        {
            // Re-run the comparison so an adapter failure is not accidentally interpreted as no
            // collision by `any` above.
            for existing in &caller_entries {
                if keys_equal(existing, &entry, ordinal)? {
                    return Err(WindowsEnvironmentError::OverrideCollision);
                }
            }
        }
        caller_entries.push(entry);
    }

    let mut child_path = None;
    for entry in caller_entries {
        if is_path_key(&entry.key, ordinal)? {
            child_path = Some(entry.value.clone());
        }
        upsert_entry(&mut merged, entry, ordinal, true)?;
    }
    let ordered = sort_entries(merged, ordinal)?;
    Ok(WindowsEnvironmentPlan {
        block: make_environment_block(&ordered),
        child_path,
        parent_path,
    })
}

fn encode_entry(
    key: &OsStr,
    value: &OsStr,
    caller_override: bool,
    encoder: &dyn WindowsWideEncoder,
) -> Result<EnvironmentEntry, WindowsEnvironmentError> {
    if key.as_encoded_bytes().contains(&0) || value.as_encoded_bytes().contains(&0) {
        return Err(WindowsEnvironmentError::InteriorNul);
    }
    let key = encoder.encode_wide(key)?;
    let value = encoder.encode_wide(value)?;
    if key.contains(&NUL) || value.contains(&NUL) {
        return Err(WindowsEnvironmentError::InteriorNul);
    }
    validate_key(&key, caller_override)?;
    Ok(EnvironmentEntry { key, value })
}

fn validate_key(key: &[u16], caller_override: bool) -> Result<(), WindowsEnvironmentError> {
    if key.is_empty() {
        return Err(WindowsEnvironmentError::EmptyKey);
    }
    if key[0] == EQUALS {
        if caller_override || !is_inherited_pseudo_key(key) {
            return Err(WindowsEnvironmentError::InvalidKey);
        }
        return Ok(());
    }
    if key.contains(&EQUALS) {
        return Err(WindowsEnvironmentError::InvalidKey);
    }
    Ok(())
}

fn is_inherited_pseudo_key(key: &[u16]) -> bool {
    // `std::env::vars_os` preserves every Windows environment entry whose key starts with `=`,
    // including drive-current-directory keys (`=C:`) and cmd.exe keys such as `=ExitCode`.
    // They are inherited OS state, not a caller-injection seam; caller overrides remain rejected.
    matches!(key, [EQUALS, rest @ ..] if !rest.is_empty() && !rest.contains(&EQUALS))
}

fn keys_equal(
    left: &EnvironmentEntry,
    right: &EnvironmentEntry,
    ordinal: &dyn WindowsOrdinalCompare,
) -> Result<bool, WindowsEnvironmentError> {
    Ok(ordinal.compare_ignore_case(&left.key, &right.key)? == Ordering::Equal)
}

fn upsert_entry(
    entries: &mut Vec<EnvironmentEntry>,
    entry: EnvironmentEntry,
    ordinal: &dyn WindowsOrdinalCompare,
    caller_wins: bool,
) -> Result<(), WindowsEnvironmentError> {
    if let Some(index) = entries.iter().position(|existing| {
        ordinal
            .compare_ignore_case(&existing.key, &entry.key)
            .map(|ordering| ordering == Ordering::Equal)
            .unwrap_or(false)
    }) {
        // Repeat fallibly so a native zero/unrecognized result never gets collapsed into a
        // non-match merely because `position` cannot return Result.
        if ordinal.compare_ignore_case(&entries[index].key, &entry.key)? == Ordering::Equal
            && caller_wins
        {
            entries[index] = entry;
        }
        return Ok(());
    }
    entries.push(entry);
    Ok(())
}

fn find_path(
    entries: &[EnvironmentEntry],
    ordinal: &dyn WindowsOrdinalCompare,
) -> Result<Option<Vec<u16>>, WindowsEnvironmentError> {
    for entry in entries {
        if is_path_key(&entry.key, ordinal)? {
            return Ok(Some(entry.value.clone()));
        }
    }
    Ok(None)
}

fn is_path_key(
    key: &[u16],
    ordinal: &dyn WindowsOrdinalCompare,
) -> Result<bool, WindowsEnvironmentError> {
    Ok(
        ordinal.compare_ignore_case(key, &"PATH".encode_utf16().collect::<Vec<_>>())?
            == Ordering::Equal,
    )
}

fn sort_entries(
    entries: Vec<EnvironmentEntry>,
    ordinal: &dyn WindowsOrdinalCompare,
) -> Result<Vec<EnvironmentEntry>, WindowsEnvironmentError> {
    let mut ordered: Vec<EnvironmentEntry> = Vec::with_capacity(entries.len());
    for entry in entries {
        let mut index = ordered.len();
        for (candidate_index, existing) in ordered.iter().enumerate() {
            match ordinal.compare_ignore_case(&entry.key, &existing.key)? {
                Ordering::Less => {
                    index = candidate_index;
                    break;
                }
                Ordering::Equal => {
                    return Err(WindowsEnvironmentError::OrdinalComparison);
                }
                Ordering::Greater => {}
            }
        }
        ordered.insert(index, entry);
    }
    Ok(ordered)
}

fn make_environment_block(entries: &[EnvironmentEntry]) -> Vec<u16> {
    let mut block = Vec::new();
    for entry in entries {
        block.extend_from_slice(&entry.key);
        block.push(EQUALS);
        block.extend_from_slice(&entry.value);
        block.push(NUL);
    }
    block
}

fn ascii_upper(unit: u16) -> u16 {
    if (b'a' as u16..=b'z' as u16).contains(&unit) {
        unit - (b'a' as u16 - b'A' as u16)
    } else {
        unit
    }
}

#[cfg(windows)]
pub(super) struct SystemWindowsOrdinalCompare;

#[cfg(windows)]
impl WindowsOrdinalCompare for SystemWindowsOrdinalCompare {
    fn compare_ignore_case(&self, left: &[u16], right: &[u16]) -> WindowsOrdinalResult<Ordering> {
        #[allow(unsafe_code)]
        // SAFETY: both slices are valid UTF-16 storage for the supplied explicit lengths and the
        // synchronous API does not retain either pointer after it returns.
        let result = unsafe {
            windows_sys::Win32::Globalization::CompareStringOrdinal(
                left.as_ptr(),
                left.len() as i32,
                right.as_ptr(),
                right.len() as i32,
                1,
            )
        };
        match result {
            1 => Ok(Ordering::Less),
            2 => Ok(Ordering::Equal),
            3 => Ok(Ordering::Greater),
            _ => Err(WindowsEnvironmentError::OrdinalComparison),
        }
    }
}

#[cfg(windows)]
pub(super) struct SystemInheritedWindowsEnvironment;

#[cfg(windows)]
impl InheritedWindowsEnvironment for SystemInheritedWindowsEnvironment {
    fn snapshot(&self) -> WindowsEnvironmentSourceResult<Vec<(OsString, OsString)>> {
        Ok(std::env::vars_os().collect())
    }
}

#[cfg(windows)]
pub(super) struct SystemWindowsWideEncoder;

#[cfg(windows)]
impl WindowsWideEncoder for SystemWindowsWideEncoder {
    fn encode_wide(&self, value: &OsStr) -> WindowsWideResult<Vec<u16>> {
        use std::os::windows::ffi::OsStrExt;

        Ok(value.encode_wide().collect())
    }
}

#[cfg(test)]
mod tests {
    use std::cell::{Cell, RefCell};
    use std::collections::{BTreeMap, BTreeSet};

    use super::*;

    struct FakeOrdinalCompare {
        failures: RefCell<BTreeSet<(Vec<u16>, Vec<u16>)>>,
        calls: RefCell<Vec<(Vec<u16>, Vec<u16>)>>,
    }

    impl FakeOrdinalCompare {
        fn new() -> Self {
            Self {
                failures: RefCell::new(BTreeSet::new()),
                calls: RefCell::new(Vec::new()),
            }
        }
    }

    impl WindowsOrdinalCompare for FakeOrdinalCompare {
        fn compare_ignore_case(
            &self,
            left: &[u16],
            right: &[u16],
        ) -> WindowsOrdinalResult<Ordering> {
            self.calls
                .borrow_mut()
                .push((left.to_vec(), right.to_vec()));
            if self
                .failures
                .borrow()
                .contains(&(left.to_vec(), right.to_vec()))
            {
                return Err(WindowsEnvironmentError::OrdinalComparison);
            }
            Ok(ascii_upper(left).cmp(&ascii_upper(right)))
        }
    }

    struct FakeInheritedWindowsEnvironment {
        entries: Vec<(OsString, OsString)>,
        fail: Cell<bool>,
    }

    impl InheritedWindowsEnvironment for FakeInheritedWindowsEnvironment {
        fn snapshot(&self) -> WindowsEnvironmentSourceResult<Vec<(OsString, OsString)>> {
            if self.fail.get() {
                Err(WindowsEnvironmentError::InheritedEnvironment(
                    "configured failure".to_owned(),
                ))
            } else {
                Ok(self.entries.clone())
            }
        }
    }

    struct FakeWideEncoder {
        overrides: RefCell<BTreeMap<OsString, Vec<u16>>>,
        calls: RefCell<Vec<OsString>>,
    }

    impl FakeWideEncoder {
        fn new() -> Self {
            Self {
                overrides: RefCell::new(BTreeMap::new()),
                calls: RefCell::new(Vec::new()),
            }
        }

        fn map(&self, value: &str, encoded: Vec<u16>) {
            self.overrides
                .borrow_mut()
                .insert(OsString::from(value), encoded);
        }
    }

    impl WindowsWideEncoder for FakeWideEncoder {
        fn encode_wide(&self, value: &OsStr) -> WindowsWideResult<Vec<u16>> {
            self.calls.borrow_mut().push(value.to_owned());
            if let Some(encoded) = self.overrides.borrow().get(value) {
                return Ok(encoded.clone());
            }
            Ok(value
                .as_encoded_bytes()
                .iter()
                .map(|byte| u16::from(*byte))
                .collect())
        }
    }

    fn ascii_upper(value: &[u16]) -> Vec<u16> {
        value.iter().map(|unit| super::ascii_upper(*unit)).collect()
    }

    fn block_entries(block: &[u16]) -> Vec<Vec<u16>> {
        block
            .split(|unit| *unit == NUL)
            .filter(|entry| !entry.is_empty())
            .map(ToOwned::to_owned)
            .collect()
    }

    fn setup() -> (
        FakeOrdinalCompare,
        FakeInheritedWindowsEnvironment,
        FakeWideEncoder,
    ) {
        (
            FakeOrdinalCompare::new(),
            FakeInheritedWindowsEnvironment {
                entries: Vec::new(),
                fail: Cell::new(false),
            },
            FakeWideEncoder::new(),
        )
    }

    #[test]
    fn inherited_environment_merges_ordinally_and_preserves_empty_values_and_pseudo_keys() {
        let (ordinal, mut inherited, encoder) = setup();
        inherited.entries = vec![
            (OsString::from("=C:"), OsString::from(r"C:\work")),
            (OsString::from("=ExitCode"), OsString::from("00000000")),
            (OsString::from("zeta"), OsString::new()),
            (OsString::from("Alpha"), OsString::from("one")),
        ];
        let mut overrides = BTreeMap::new();
        overrides.insert(OsString::from("beta"), OsString::from("two"));
        let plan = prepare_environment(&overrides, &ordinal, &inherited, &encoder).unwrap();
        assert_eq!(
            block_entries(&plan.block),
            vec![
                "=C:=C:\\work".encode_utf16().collect::<Vec<_>>(),
                "=ExitCode=00000000".encode_utf16().collect::<Vec<_>>(),
                "Alpha=one".encode_utf16().collect::<Vec<_>>(),
                "beta=two".encode_utf16().collect::<Vec<_>>(),
                "zeta=".encode_utf16().collect::<Vec<_>>(),
            ]
        );
        assert_eq!(plan.block.last(), Some(&NUL));
    }

    #[test]
    fn fake_wide_encoder_preserves_a_lone_surrogate_unit() {
        let (ordinal, inherited, encoder) = setup();
        encoder.map("surrogate-value", vec![0xd800]);
        let mut overrides = BTreeMap::new();
        overrides.insert(
            OsString::from("SURROGATE"),
            OsString::from("surrogate-value"),
        );
        let plan = prepare_environment(&overrides, &ordinal, &inherited, &encoder).unwrap();
        assert!(plan.block.windows(2).any(|units| units == [EQUALS, 0xd800]));
    }

    #[test]
    fn zero_entries_produce_an_empty_plan_for_the_owned_block_wrapper() {
        let (ordinal, inherited, encoder) = setup();
        let plan = prepare_environment(&BTreeMap::new(), &ordinal, &inherited, &encoder).unwrap();
        assert!(plan.block.is_empty());
    }

    #[test]
    fn invalid_override_keys_and_nuls_fail() {
        let (ordinal, inherited, encoder) = setup();
        for (key, value, expected) in [
            ("", "value", WindowsEnvironmentError::EmptyKey),
            ("=C:", "value", WindowsEnvironmentError::InvalidKey),
            ("=ExitCode", "value", WindowsEnvironmentError::InvalidKey),
            ("bad=key", "value", WindowsEnvironmentError::InvalidKey),
            ("nul\0key", "value", WindowsEnvironmentError::InteriorNul),
            ("key", "nul\0value", WindowsEnvironmentError::InteriorNul),
        ] {
            let mut overrides = BTreeMap::new();
            overrides.insert(OsString::from(key), OsString::from(value));
            assert_eq!(
                prepare_environment(&overrides, &ordinal, &inherited, &encoder).unwrap_err(),
                expected
            );
        }
    }

    #[test]
    fn ordinal_collisions_and_failures_are_hard_errors() {
        let (ordinal, inherited, encoder) = setup();
        let mut overrides = BTreeMap::new();
        overrides.insert(OsString::from("Path"), OsString::from("one"));
        overrides.insert(OsString::from("PATH"), OsString::from("two"));
        assert_eq!(
            prepare_environment(&overrides, &ordinal, &inherited, &encoder).unwrap_err(),
            WindowsEnvironmentError::OverrideCollision
        );

        let (ordinal, inherited, encoder) = setup();
        ordinal.failures.borrow_mut().insert((
            "PATH".encode_utf16().collect(),
            "PATH".encode_utf16().collect(),
        ));
        let mut overrides = BTreeMap::new();
        overrides.insert(OsString::from("PATH"), OsString::from("one"));
        assert_eq!(
            prepare_environment(&overrides, &ordinal, &inherited, &encoder).unwrap_err(),
            WindowsEnvironmentError::OrdinalComparison
        );
    }

    #[test]
    fn caller_path_spelling_and_value_win_over_inherited_path() {
        let (ordinal, mut inherited, encoder) = setup();
        inherited.entries = vec![(OsString::from("Path"), OsString::from("parent"))];
        let mut overrides = BTreeMap::new();
        overrides.insert(OsString::from("PATH"), OsString::from("child"));
        let plan = prepare_environment(&overrides, &ordinal, &inherited, &encoder).unwrap();
        assert_eq!(plan.child_path, Some("child".encode_utf16().collect()));
        assert_eq!(plan.parent_path, Some("parent".encode_utf16().collect()));
        assert_eq!(
            block_entries(&plan.block),
            vec!["PATH=child".encode_utf16().collect::<Vec<_>>()]
        );
    }
}
