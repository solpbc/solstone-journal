// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::sync::Arc;

use chrono::{DateTime, Utc};

#[derive(Clone)]
pub struct Clock(Arc<dyn Fn() -> DateTime<Utc> + Send + Sync>);

impl Clock {
    pub fn system() -> Self {
        Self(Arc::new(Utc::now))
    }

    pub fn fixed(now: DateTime<Utc>) -> Self {
        Self(Arc::new(move || now))
    }

    pub fn now(&self) -> DateTime<Utc> {
        (self.0)()
    }
}
