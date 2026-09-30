// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use chrono::{DateTime, Utc};
use std::sync::Arc;

#[derive(Clone)]
pub struct Clock(Arc<dyn Fn() -> DateTime<Utc> + Send + Sync>);
impl Clock {
    pub fn real() -> Self {
        Self(Arc::new(Utc::now))
    }
    pub fn new(now: impl Fn() -> DateTime<Utc> + Send + Sync + 'static) -> Self {
        Self(Arc::new(now))
    }
    pub fn now(&self) -> DateTime<Utc> {
        (self.0)()
    }
}
