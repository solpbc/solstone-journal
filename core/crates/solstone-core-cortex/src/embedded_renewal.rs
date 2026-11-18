// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::{
    path::PathBuf,
    sync::{Arc, mpsc},
};

use solstone_core_callosum::CallosumEnvelope;

use crate::{
    renewal::{RenewalHandle, RenewalService, RenewalWorkerStart},
    state::Outbound,
};

/// The existing Cortex prerequisite policy without subprocess talent workers.
/// The app must reflect registration on Callosum before constructing this owner.
pub struct EmbeddedSppRenewal {
    handle: RenewalHandle,
    service: RenewalService,
    outbound: mpsc::Receiver<Outbound>,
}

impl EmbeddedSppRenewal {
    pub fn start(journal: PathBuf) -> Self {
        let (sender, outbound) = mpsc::channel();
        let handle = RenewalHandle::production(journal, sender, Arc::new(jiff::Timestamp::now));
        if handle.startup_refresh_needed() {
            handle.startup_refresh();
        }
        let mut service = RenewalService::new(RenewalWorkerStart::production(handle.clone()));
        service.start_worker_once();
        Self {
            handle,
            service,
            outbound,
        }
    }

    pub fn try_next(&self) -> Option<CallosumEnvelope> {
        self.outbound
            .try_recv()
            .ok()
            .map(|outbound| CallosumEnvelope {
                tract: outbound.tract.into(),
                event: outbound.event,
                ts: None,
                extra: outbound.fields,
            })
    }

    pub fn observe(&self, envelope: &CallosumEnvelope) {
        if envelope.tract == "supervisor" {
            self.handle
                .handle_supervisor(&envelope.event, &envelope.extra);
        }
    }

    pub fn stop(&mut self) {
        self.service.stop();
    }
}

impl Drop for EmbeddedSppRenewal {
    fn drop(&mut self) {
        self.stop();
    }
}
