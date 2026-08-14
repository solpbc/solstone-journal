// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::sync::Arc;

use chrono::{Local, NaiveDateTime};

/// Injectable local-naive wall clock. Python routes.py:118-146 uses
/// `datetime.now()` and `date.today()`, both in local wall time.
#[derive(Clone)]
pub struct Clock(Arc<dyn Fn() -> NaiveDateTime + Send + Sync>);

impl Clock {
    pub fn local() -> Self {
        Self(Arc::new(|| Local::now().naive_local()))
    }

    pub fn new(now: impl Fn() -> NaiveDateTime + Send + Sync + 'static) -> Self {
        Self(Arc::new(now))
    }

    pub fn now(&self) -> NaiveDateTime {
        (self.0)()
    }
}
