// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use crate::denylist::DEFAULT_READ_CALL_BUDGET;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ReadBudget {
    cap: i64,
    count: i64,
}
impl ReadBudget {
    pub const fn new(cap: i64) -> Self {
        Self { cap, count: 0 }
    }
    pub const fn with_default_cap() -> Self {
        Self::new(DEFAULT_READ_CALL_BUDGET)
    }
    pub fn charge(&mut self) -> bool {
        if self.count >= self.cap {
            false
        } else {
            self.count += 1;
            true
        }
    }
    pub const fn count(&self) -> i64 {
        self.count
    }
    pub const fn cap(&self) -> i64 {
        self.cap
    }
}
