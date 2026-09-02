// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! SPL pairing entry calls. No retry invariant: SPL owns the one-request commit
//! rule; this adapter calls each SPL entry once and adds no retry wrapper.

use std::sync::Arc;

use solstone_core_sol_client::seam::{LinkJoinDirectRequest, LinkJoinRelayRequest};
use spl_core::pairlink::{Endpoint, RelayPairLink};
use spl_transport::credential::Credential;
use spl_transport::pairing::{self, DirectPairingSeam};
use spl_transport::{TransportError, relay_pairing};

pub(crate) async fn direct(
    request: &LinkJoinDirectRequest,
    seam: Arc<dyn DirectPairingSeam>,
) -> Result<Credential, TransportError> {
    let endpoints = request
        .targets
        .iter()
        .map(|target| Endpoint {
            host: target.host.clone(),
            port: target.port,
        })
        .collect::<Vec<_>>();
    pairing::pair_with_seam(
        &endpoints,
        &request.nonce_hex,
        &request.ca_fp_prefix,
        &request.device_label,
        seam,
        &request.additional_fields,
    )
    .await
}

pub(crate) async fn relay(request: &LinkJoinRelayRequest) -> Result<Credential, TransportError> {
    let link = RelayPairLink {
        s: secret_array(&request.secret)?,
        ca_fp_spki: request.ca_fp_spki.clone(),
        relay_origin: request.relay_origin.clone(),
    };
    relay_pairing::pair_over_relay(&link, &request.device_label, &request.additional_fields).await
}

fn secret_array(secret: &[u8]) -> Result<[u8; 8], TransportError> {
    secret
        .try_into()
        .map_err(|_| TransportError::PairLink("relay secret length".to_string()))
}

#[cfg(all(test, not(feature = "full-tests")))]
mod tests {
    #[test]
    fn spl_entry_module_stays_one_shot() {
        let source = include_str!("pairing_entry.rs");
        assert_eq!(source.matches(concat!("pair", "_with_seam(")).count(), 1);
        assert_eq!(source.matches(concat!("pair", "_over_relay(")).count(), 1);
        assert!(!source.contains(concat!("fo", "r ")));
        assert!(!source.contains(concat!("wh", "ile ")));
        assert!(!source.contains(concat!("lo", "op ")));
    }
}
