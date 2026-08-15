// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use chrono::{Local, NaiveDateTime};
use std::sync::Arc;

/// Injectable local wall clock: Python uses `date.today()`, not UTC.
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
