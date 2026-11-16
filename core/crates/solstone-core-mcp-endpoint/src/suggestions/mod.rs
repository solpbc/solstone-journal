// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

pub mod owner;
pub mod store;
pub mod submit;

#[cfg(all(test, feature = "full-tests"))]
mod tests;

pub const SUGGESTION_SCHEMA: u32 = 1;
pub const SUGGESTION_RESULT_SCHEMA: u32 = 1;
pub const SETTINGS_SCHEMA: u32 = 1;
pub const PENDING_AGENT_CAP: usize = 20;
pub const DAILY_AGENT_CAP: usize = 50;

#[cfg(not(test))]
pub fn suggestions_ready() -> bool {
    crate::AGENT_SUGGESTIONS_READY
}

#[cfg(not(test))]
pub fn force_suggestion_receipt_failure() -> bool {
    false
}

#[cfg(not(test))]
pub fn force_suggestion_ask_failure() -> bool {
    false
}

#[cfg(test)]
thread_local! {
    static TEST_SUGGESTIONS_READY: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
    static FORCE_SUGGESTION_RECEIPT_FAILURE: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
    static FORCE_SUGGESTION_ASK_FAILURE: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

#[cfg(test)]
pub fn suggestions_ready() -> bool {
    TEST_SUGGESTIONS_READY.with(std::cell::Cell::get)
}

#[cfg(test)]
pub fn set_test_suggestions_ready(ready: bool) {
    TEST_SUGGESTIONS_READY.with(|c| c.set(ready));
}

#[cfg(test)]
pub fn force_suggestion_receipt_failure() -> bool {
    FORCE_SUGGESTION_RECEIPT_FAILURE.with(std::cell::Cell::get)
}

#[cfg(test)]
pub fn set_force_suggestion_receipt_failure(fail: bool) {
    FORCE_SUGGESTION_RECEIPT_FAILURE.with(|c| c.set(fail));
}

#[cfg(test)]
pub fn force_suggestion_ask_failure() -> bool {
    FORCE_SUGGESTION_ASK_FAILURE.with(std::cell::Cell::get)
}

#[cfg(test)]
pub fn set_force_suggestion_ask_failure(fail: bool) {
    FORCE_SUGGESTION_ASK_FAILURE.with(|c| c.set(fail));
}

#[cfg(all(test, feature = "full-tests"))]
thread_local! {
    static BEFORE_ACTIVITY_APPLY: std::cell::RefCell<Option<Box<dyn FnOnce()>>> = const {std::cell::RefCell::new(None)};
}

#[cfg(all(test, feature = "full-tests"))]
pub(crate) fn before_activity_apply(callback: impl FnOnce() + 'static) {
    BEFORE_ACTIVITY_APPLY.with(|hook| *hook.borrow_mut() = Some(Box::new(callback)));
}

#[cfg(all(test, feature = "full-tests"))]
pub(crate) fn run_before_activity_apply() {
    BEFORE_ACTIVITY_APPLY.with(|hook| {
        if let Some(callback) = hook.borrow_mut().take() {
            callback();
        }
    });
}
