// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! The standalone SPL home-service runtime.
//!
//! The service-facing I/O adapters are intentionally kept out of this first
//! layer. Buffering, admission, and health vocabulary are pure so every
//! transport implementation shares the same owner-visible behaviour.

mod admission;
mod callosum;
mod health;
mod link_state_files;
mod loopback_pipe;
mod posture_gate;
mod reconnect_backoff;
mod relay_client;
mod relay_control;
mod relay_health;
mod relay_status_failure;
mod relay_websocket;
mod service;
mod service_process;
mod service_shutdown;
mod service_transition;
mod tunnel_route;
mod ws_buffer;
mod ws_sink;

pub use admission::RelayAdmissionGate;
pub use callosum::{CallosumEmit, LoggingEmit, Verbosity};
pub use health::{
    LINK_HEALTH_EVENT, OFFLINE_TUNNEL_REASONS, REASON_HOME_MISSING_MOBILE,
    REASON_LOCAL_PRIVATE_LISTENER_UNREACHABLE, REASON_RELAY_ADMISSION_SATURATED,
    REASON_RELAY_TUNNEL_REJECTED, REASON_RELAY_TUNNEL_UNREACHABLE, REASON_SERVICE_TOKEN_REJECTED,
};
pub use link_state_files::{
    LinkServiceToken, LinkServiceTokenRead, LinkState, LinkStateRead, load_link_service_token,
    load_link_state,
};
pub use loopback_pipe::{
    TCP_TO_WS_READ_MAX, TunnelPipeError, TunnelPipeProgress, pipe_loopback, pipe_tunnel,
};
pub use posture_gate::{
    PostureGate, PostureInput, RelayBlocked, RelayDecision, RelayPermit, ServiceToken, TokenInput,
};
pub use reconnect_backoff::{
    INITIAL_RECONNECT_BASE, MAX_RECONNECT_BASE, ReconnectBackoffError, ReconnectSchedule,
    schedule_reconnect,
};
pub(crate) use relay_client::{
    LISTEN_ACK_STABILITY_WINDOW, LISTEN_PING_ACK_TIMEOUT, LISTEN_PING_INTERVAL,
};
pub use relay_client::{
    LoopbackConnect, LoopbackDialer, LoopbackStream, RelayClient, RelayClientConfig, RelayError,
};
pub use relay_control::{
    ListenControl, bearer_authorization_value, parse_listen_control, relay_tunnel_url,
    websocket_endpoint,
};
pub use relay_health::{RelayHealth, RelayHealthState, RelayTunnelFailure};
pub use relay_status_failure::{RelayTunnelFailureSignal, classify_relay_tunnel_failure};
pub use relay_websocket::{
    RelayWebSocket, RelayWebSocketError, RelayWebSocketReader, RelayWebSocketWriter,
};
pub use service::{
    POSTURE_POLL_INTERVAL, RelayRunTask, RelayServiceToken, ServiceDeps, ServiceError, ServicePoll,
    StartedRelay, run_service,
};
pub use service_process::{NativeServiceError, run_native_service};
pub use service_shutdown::{RelayStop, ServiceShutdownError, stop_relay_run};
pub use service_transition::{
    PostureObservation, ServiceAction, ServiceLifecycle, TokenObservation, transition,
};
pub use tunnel_route::{TunnelRoute, route_tunnel_prefix};
pub use ws_buffer::{BufferedWsReader, WsBufferError, WsByteSource, WsClosed};
pub use ws_sink::WsByteSink;
