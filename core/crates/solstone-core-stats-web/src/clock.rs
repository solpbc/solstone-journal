// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use jiff::Timestamp;
use std::sync::Arc;

#[derive(Clone)]
pub struct Clock(Arc<dyn Fn() -> Timestamp + Send + Sync>);
impl Clock {
    pub fn real() -> Self {
        Self(Arc::new(Timestamp::now))
    }
    pub fn new(now: impl Fn() -> Timestamp + Send + Sync + 'static) -> Self {
        Self(Arc::new(now))
    }
    pub fn now(&self) -> Timestamp {
        (self.0)()
    }
}
