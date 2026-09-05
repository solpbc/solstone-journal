// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use crate::seam::{
    BuildIdentityProvider, ClientItemIdProvider, Clock, FileProvider, HttpTransport,
    LinkJoinPairingSeam, LinkServeRunner, LinkStatusProbe, NotificationSink,
};
use std::collections::BTreeMap;

#[derive(Clone, Copy)]
pub struct CommandContext<'a> {
    pub args: &'a [String],
    pub env: &'a BTreeMap<String, String>,
    pub stdin: &'a str,
    pub today: &'a str,
    pub transport: &'a dyn HttpTransport,
    pub clock: Option<&'a dyn Clock>,
    pub files: Option<&'a dyn FileProvider>,
    pub build_identity: Option<&'a dyn BuildIdentityProvider>,
    pub client_item_ids: Option<&'a dyn ClientItemIdProvider>,
    pub notification_sink: Option<&'a dyn NotificationSink>,
    pub link_pairing: Option<&'a dyn LinkJoinPairingSeam>,
    pub link_serve: Option<&'a dyn LinkServeRunner>,
    pub link_status_probe: Option<&'a dyn LinkStatusProbe>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandOutput {
    pub stdout: String,
    pub stderr: String,
    pub exit: i32,
}

impl CommandOutput {
    #[must_use]
    pub fn success(stdout: impl Into<String>) -> Self {
        Self {
            stdout: stdout.into(),
            stderr: String::new(),
            exit: 0,
        }
    }

    #[must_use]
    pub fn failure(stderr: impl Into<String>, exit: i32) -> Self {
        Self {
            stdout: String::new(),
            stderr: stderr.into(),
            exit,
        }
    }
}
