// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::sync::Arc;

use jiff::Timestamp;

#[derive(Clone)]
pub struct Clock(Arc<dyn Fn() -> Timestamp + Send + Sync>);

impl Clock {
    pub fn system() -> Self {
        Self(Arc::new(Timestamp::now))
    }

    pub fn fixed(now: Timestamp) -> Self {
        Self(Arc::new(move || now))
    }

    pub fn now(&self) -> Timestamp {
        (self.0)()
    }
}
