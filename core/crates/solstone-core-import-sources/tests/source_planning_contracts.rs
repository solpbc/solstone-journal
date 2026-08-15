// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

// Each preserved leaf owns its existing `support` module.
#![allow(clippy::duplicate_mod)]

#[path = "detect.rs"]
mod detect;
#[path = "plan_contract.rs"]
mod plan_contract;
#[path = "plan_utc.rs"]
mod plan_utc;
#[path = "registry_fixture_contract.rs"]
mod registry_fixture_contract;
#[path = "routing_order.rs"]
mod routing_order;
#[path = "stub_table.rs"]
mod stub_table;
