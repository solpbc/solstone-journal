// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

pub const REASON_HOME_MISSING_MOBILE: &str = "home_missing_mobile";
pub const REASON_SERVICE_TOKEN_REJECTED: &str = "service_token_rejected";
pub const REASON_RELAY_TUNNEL_REJECTED: &str = "relay_tunnel_rejected";
pub const REASON_RELAY_TUNNEL_UNREACHABLE: &str = "relay_tunnel_unreachable";
pub const REASON_LOCAL_PRIVATE_LISTENER_UNREACHABLE: &str = "local_private_listener_unreachable";
pub const REASON_RELAY_ADMISSION_SATURATED: &str = "relay_admission_saturated";

pub const OFFLINE_TUNNEL_REASONS: [&str; 2] = [
    REASON_SERVICE_TOKEN_REJECTED,
    REASON_LOCAL_PRIVATE_LISTENER_UNREACHABLE,
];
pub const LINK_HEALTH_EVENT: &str = "health";
