// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Bounded inventory and checked-read limits shared by journal source backends.

/// Immutable limits for a recursive source inventory and its checked reads.
///
/// Every limit is inclusive: a value equal to its corresponding maximum is
/// accepted, while the next observed value refuses the complete operation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct InventoryBudget {
    maximum_entries: usize,
    maximum_depth: usize,
    maximum_member_utf8_bytes: usize,
    maximum_relative_path_utf16_bytes: usize,
    maximum_checked_read_bytes: usize,
}

impl InventoryBudget {
    /// Construct a complete source-inventory budget.
    #[must_use]
    pub const fn new(
        maximum_entries: usize,
        maximum_depth: usize,
        maximum_member_utf8_bytes: usize,
        maximum_relative_path_utf16_bytes: usize,
        maximum_checked_read_bytes: usize,
    ) -> Self {
        Self {
            maximum_entries,
            maximum_depth,
            maximum_member_utf8_bytes,
            maximum_relative_path_utf16_bytes,
            maximum_checked_read_bytes,
        }
    }

    /// Maximum directory entries observed before policy filtering.
    #[must_use]
    pub const fn maximum_entries(self) -> usize {
        self.maximum_entries
    }

    /// Maximum recursive depth, with the admitted root at depth zero.
    #[must_use]
    pub const fn maximum_depth(self) -> usize {
        self.maximum_depth
    }

    /// Maximum UTF-8 byte length for one complete archive member name.
    #[must_use]
    pub const fn maximum_member_utf8_bytes(self) -> usize {
        self.maximum_member_utf8_bytes
    }

    /// Maximum UTF-16 byte length for one native relative path.
    #[must_use]
    pub const fn maximum_relative_path_utf16_bytes(self) -> usize {
        self.maximum_relative_path_utf16_bytes
    }

    /// Maximum cumulative bytes returned by one checked-read session.
    #[must_use]
    pub const fn maximum_checked_read_bytes(self) -> usize {
        self.maximum_checked_read_bytes
    }
}

/// The budget dimension that refused an all-or-nothing source operation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum InventoryBudgetLimit {
    Entries,
    Depth,
    MemberUtf8Bytes,
    RelativePathUtf16Bytes,
    CheckedReadBytes,
}

#[cfg(windows)]
pub(crate) struct InventoryUsage {
    entries: usize,
}

#[cfg(windows)]
impl InventoryUsage {
    pub(crate) const fn new() -> Self {
        Self { entries: 0 }
    }

    pub(crate) fn observe_entry(
        &mut self,
        budget: InventoryBudget,
    ) -> Result<(), InventoryBudgetLimit> {
        self.entries = self.entries.saturating_add(1);
        (self.entries <= budget.maximum_entries)
            .then_some(())
            .ok_or(InventoryBudgetLimit::Entries)
    }

    pub(crate) fn check_depth(
        budget: InventoryBudget,
        depth: usize,
    ) -> Result<(), InventoryBudgetLimit> {
        (depth <= budget.maximum_depth)
            .then_some(())
            .ok_or(InventoryBudgetLimit::Depth)
    }

    pub(crate) fn check_member(
        budget: InventoryBudget,
        member: &str,
    ) -> Result<(), InventoryBudgetLimit> {
        (member.len() <= budget.maximum_member_utf8_bytes)
            .then_some(())
            .ok_or(InventoryBudgetLimit::MemberUtf8Bytes)
    }

    pub(crate) fn check_relative_path(
        budget: InventoryBudget,
        relative_path: &std::path::Path,
    ) -> Result<(), InventoryBudgetLimit> {
        #[cfg(windows)]
        let bytes = std::os::windows::ffi::OsStrExt::encode_wide(relative_path.as_os_str())
            .count()
            .saturating_mul(std::mem::size_of::<u16>());
        #[cfg(not(windows))]
        let bytes = relative_path.as_os_str().len();
        (bytes <= budget.maximum_relative_path_utf16_bytes)
            .then_some(())
            .ok_or(InventoryBudgetLimit::RelativePathUtf16Bytes)
    }
}

#[cfg(windows)]
pub(crate) struct CheckedReadUsage {
    bytes: usize,
}

#[cfg(windows)]
impl CheckedReadUsage {
    pub(crate) const fn new() -> Self {
        Self { bytes: 0 }
    }

    pub(crate) fn check_reserve(
        &self,
        budget: InventoryBudget,
        bytes: usize,
    ) -> Result<(), InventoryBudgetLimit> {
        let total = self
            .bytes
            .checked_add(bytes)
            .ok_or(InventoryBudgetLimit::CheckedReadBytes)?;
        (total <= budget.maximum_checked_read_bytes)
            .then_some(())
            .ok_or(InventoryBudgetLimit::CheckedReadBytes)
    }

    pub(crate) fn commit(&mut self, bytes: usize) {
        self.bytes = self
            .bytes
            .checked_add(bytes)
            .expect("checked-read byte reservation was verified before commit");
    }
}

#[cfg(all(test, windows))]
mod tests {
    use super::{CheckedReadUsage, InventoryBudget, InventoryBudgetLimit, InventoryUsage};

    const BUDGET: InventoryBudget = InventoryBudget::new(1, 2, 3, 6, 5);

    #[test]
    fn each_inventory_limit_is_inclusive_and_independent() {
        let mut usage = InventoryUsage::new();
        usage.observe_entry(BUDGET).unwrap();
        assert_eq!(
            usage.observe_entry(BUDGET),
            Err(InventoryBudgetLimit::Entries)
        );
        InventoryUsage::check_depth(BUDGET, 2).unwrap();
        assert_eq!(
            InventoryUsage::check_depth(BUDGET, 3),
            Err(InventoryBudgetLimit::Depth)
        );
        InventoryUsage::check_member(BUDGET, "abc").unwrap();
        assert_eq!(
            InventoryUsage::check_member(BUDGET, "abcd"),
            Err(InventoryBudgetLimit::MemberUtf8Bytes)
        );
        InventoryUsage::check_relative_path(BUDGET, std::path::Path::new("abc")).unwrap();
        assert_eq!(
            InventoryUsage::check_relative_path(BUDGET, std::path::Path::new("abcdefg")),
            Err(InventoryBudgetLimit::RelativePathUtf16Bytes)
        );
    }

    #[test]
    fn checked_read_budget_is_session_local_and_all_or_nothing() {
        let mut usage = CheckedReadUsage::new();
        usage.check_reserve(BUDGET, 5).unwrap();
        usage.commit(5);
        assert_eq!(
            usage.check_reserve(BUDGET, 1),
            Err(InventoryBudgetLimit::CheckedReadBytes)
        );
        let mut second_session = CheckedReadUsage::new();
        second_session.check_reserve(BUDGET, 5).unwrap();
        second_session.commit(5);
    }
}
