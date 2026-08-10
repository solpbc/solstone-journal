// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

/// The one-time notification emitted when the `sol` call budget is exceeded.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BudgetExhaustedEvent {
    pub tool: &'static str,
    pub budget: i64,
    pub count: i64,
}

/// Per-run `sol` budget. This intentionally differs from `ReadBudget`:
/// `sol` increments before testing `count > cap`, matching the provider loop.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SolCallBudget {
    cap: i64,
    count: i64,
    exhaustion_emitted: bool,
}

impl SolCallBudget {
    pub const fn new(cap: i64) -> Self {
        Self {
            cap,
            count: 0,
            exhaustion_emitted: false,
        }
    }

    /// Charge one allowed call, returning the first exhaustion notification.
    pub fn charge(&mut self) -> Option<BudgetExhaustedEvent> {
        self.count += 1;
        if self.count <= self.cap || self.exhaustion_emitted {
            return None;
        }
        self.exhaustion_emitted = true;
        Some(BudgetExhaustedEvent {
            tool: "sol",
            budget: self.cap,
            count: self.count,
        })
    }

    pub const fn exhausted(&self) -> bool {
        self.count > self.cap
    }

    pub const fn count(&self) -> i64 {
        self.count
    }

    pub const fn cap(&self) -> i64 {
        self.cap
    }
}
