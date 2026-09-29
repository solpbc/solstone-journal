// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::sync::Arc;

use chrono::{DateTime, FixedOffset, Local};

/// Injectable clock: the current instant together with the local offset.
///
/// Day strings and local wall-time forms come from the local reading
/// (`naive_local()`); epoch milliseconds and UTC timestamps come from the
/// instant, so neither is shifted by the local offset.
#[derive(Clone)]
pub struct Clock(Arc<dyn Fn() -> DateTime<FixedOffset> + Send + Sync>);

impl Clock {
    pub fn local() -> Self {
        Self(Arc::new(|| Local::now().fixed_offset()))
    }

    pub fn new(now: impl Fn() -> DateTime<FixedOffset> + Send + Sync + 'static) -> Self {
        Self(Arc::new(now))
    }

    pub fn now(&self) -> DateTime<FixedOffset> {
        (self.0)()
    }
}
