// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc
use crate::context::CheckContext;
use crate::vocabulary::Platform;
use serde_json::Value;
use solstone_core_callosum::{
    CallosumConnectionPhase, CallosumReceiveEvent, CallosumSocketConnection,
};
use solstone_core_system::process::{
    InstanceVerdict, ProcessInstance, ProcessInstanceSource, SystemProcessInstanceSource,
};
use std::cell::RefCell;
use std::marker::PhantomData;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::time::{Duration, Instant};

const STOP_CLEANUP_BOUND: Duration = Duration::from_millis(50);

struct SupervisorSample {
    journal: PathBuf,
    endpoint: PathBuf,
    generation: ProcessInstance,
    received: Instant,
    value: Value,
}

#[derive(Default)]
struct RunSamples {
    supervisor: Option<SupervisorSample>,
}

thread_local! {
    static RUN_SAMPLES: RefCell<Option<RunSamples>> = const { RefCell::new(None) };
}

/// One doctor call's successful supervisor sample, never a remembered failure.
/// Nested calls get their own scope; different threads cannot join a scope.
pub(crate) struct SupervisorSampleScope {
    previous: Option<RunSamples>,
    _same_thread: PhantomData<Rc<()>>,
}

pub(crate) fn share_for_run() -> SupervisorSampleScope {
    SupervisorSampleScope {
        previous: RUN_SAMPLES.with(|slot| slot.replace(Some(RunSamples::default()))),
        _same_thread: PhantomData,
    }
}

impl Drop for SupervisorSampleScope {
    fn drop(&mut self) {
        RUN_SAMPLES.with(|slot| slot.replace(self.previous.take()));
    }
}

fn live_generation(context: &CheckContext) -> Option<ProcessInstance> {
    if context.platform != Platform::Windows {
        return None;
    }
    let bytes = std::fs::read(
        context
            .journal_path
            .join("health/supervisor.process_instance"),
    )
    .ok()?;
    let instance = serde_json::from_slice(&bytes).ok()?;
    matches!(
        SystemProcessInstanceSource.observe(&instance),
        InstanceVerdict::SameLive { .. }
    )
    .then_some(instance)
}

fn shared_sample(context: &CheckContext, generation: ProcessInstance) -> Option<Value> {
    RUN_SAMPLES.with(|slot| {
        let scope = slot.borrow();
        let sample = scope.as_ref()?.supervisor.as_ref()?;
        (sample.journal == context.journal_path
            && sample.endpoint == context.callosum_socket_path
            && sample.generation == generation
            && sample.received.elapsed() < context.service_status_timeout)
            .then(|| sample.value.clone())
    })
}

fn clear_sample() {
    RUN_SAMPLES.with(|slot| {
        if let Some(scope) = slot.borrow_mut().as_mut() {
            scope.supervisor = None;
        }
    });
}

fn is_fresh_status_timestamp(timestamp_ms: i64, watermark_ms: i64, now_ms: i64) -> bool {
    timestamp_ms > watermark_ms && timestamp_ms <= now_ms
}

/// Why a status probe has no status.
///
/// ⛔ These are not interchangeable.  Only [`Self::NoSocket`] establishes that
/// nothing is running; the rest mean the probe could not reach a conclusion,
/// and a probe that cannot reach a subsystem must not report that subsystem
/// dead.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Unavailable {
    /// Nothing is listening to answer: no callosum socket exists, or on
    /// Windows, no live resident owns the named pipe.
    NoSocket,
    /// Windows only: the resident's recorded process could not be checked,
    /// so whether anything is listening is unknown.
    Unverifiable,
    /// The probe could not construct its own async runtime.
    ProbeRuntime,
    /// A connection was made and no status frame arrived within the budget.
    Timeout,
    /// The status connection closed before a status frame arrived.
    Transport,
}

impl Unavailable {
    /// Short cause, for a diagnostic line.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::NoSocket => "nothing is listening",
            Self::Unverifiable => "can't tell whether anything is listening",
            Self::ProbeRuntime => "nothing answered",
            Self::Timeout => "took too long to answer",
            Self::Transport => "the connection dropped",
        }
    }
}

/// Is there anything to connect to?
///
/// On Unix the socket file is the listener's own artifact. ⛔ On Windows it is
/// not: the resident serves a named pipe and never creates
/// `health/callosum.sock`, so testing that path reports "nothing listening" on
/// every healthy install. The Windows equivalent is the resident itself -- the
/// process instance it records at boot, alive and the same process -- which is
/// the evidence `journal service status` already reads for readiness.
fn endpoint_absent(context: &CheckContext) -> Option<Unavailable> {
    match context.platform {
        Platform::Windows => match resident_process(&context.journal_path) {
            InstanceVerdict::SameLive { .. } => None,
            InstanceVerdict::NotSameOrExited => Some(Unavailable::NoSocket),
            InstanceVerdict::Unverifiable => Some(Unavailable::Unverifiable),
        },
        Platform::Linux | Platform::Darwin => {
            (!context.callosum_socket_path.exists()).then_some(Unavailable::NoSocket)
        }
    }
}

pub(crate) fn resident_process(journal: &Path) -> InstanceVerdict {
    solstone_core_system::lifecycle::recorded_supervisor_verdict(journal)
}

pub fn fetch(context: &CheckContext) -> Result<Value, Unavailable> {
    if let Some(cause) = endpoint_absent(context) {
        clear_sample();
        return Err(cause);
    }
    // Windows has an exact native resident generation to bind reuse to. Every
    // check still observes it alive; a restart or a long run forces a new probe.
    let generation = RUN_SAMPLES
        .with(|slot| slot.borrow().is_some())
        .then(|| live_generation(context))
        .flatten();
    if let Some(generation) = generation
        && let Some(status) = shared_sample(context, generation)
    {
        return Ok(status);
    }
    clear_sample();
    let Ok(runtime) = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    else {
        return Err(Unavailable::ProbeRuntime);
    };
    let mut received = None;
    let status = runtime.block_on(async {
        let mut connection =
            CallosumSocketConnection::new(&context.callosum_socket_path, serde_json::Map::new());
        connection.start();
        let status = match tokio::time::timeout(context.service_status_timeout, async {
            loop {
                let Some(message) = connection.next_message().await else {
                    return Err(Unavailable::Transport);
                };
                if message.tract == "supervisor" && message.event == "status" {
                    received = Some(Instant::now());
                    return Ok(Value::Object(message.extra));
                }
            }
        })
        .await
        {
            Ok(result) => result,
            Err(_elapsed) => Err(Unavailable::Timeout),
        };
        // `stop` signals shutdown before it awaits the connection task. A doctor
        // probe has no outbound work to drain, so an unresponsive peer must not
        // extend the caller's status budget by the wire client's longer join bound.
        let _ = tokio::time::timeout(STOP_CLEANUP_BOUND, connection.stop()).await;
        status
    });
    if let (Ok(value), Some(generation), Some(received)) = (&status, generation, received)
        && live_generation(context) == Some(generation)
    {
        RUN_SAMPLES.with(|slot| {
            if let Some(scope) = slot.borrow_mut().as_mut() {
                scope.supervisor = Some(SupervisorSample {
                    journal: context.journal_path.clone(),
                    endpoint: context.callosum_socket_path.clone(),
                    generation,
                    received,
                    value: value.clone(),
                });
            }
        });
    }
    status
}

/// Fetch a fresh native Sense status beacon from the current Callosum stream.
///
/// Status events are periodic and Callosum can deliver a frame that was queued
/// just before this client joined. Requiring a server timestamp strictly after
/// the connection's `Connected` watermark prevents such a queued old beacon
/// from making the live check look healthy.
pub fn fetch_observe_status(context: &CheckContext) -> Result<Value, Unavailable> {
    if let Some(cause) = endpoint_absent(context) {
        return Err(cause);
    }
    let Ok(runtime) = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    else {
        return Err(Unavailable::ProbeRuntime);
    };
    runtime.block_on(async {
        let mut connection =
            CallosumSocketConnection::new(&context.callosum_socket_path, serde_json::Map::new());
        connection.start();
        let status = match tokio::time::timeout(context.service_status_timeout, async {
            let mut connected_watermark_ms = None;
            loop {
                match connection.next_event().await {
                    Some(CallosumReceiveEvent::Continuity {
                        phase: CallosumConnectionPhase::Connected,
                        ..
                    }) => {
                        connected_watermark_ms = Some(chrono::Utc::now().timestamp_millis());
                    }
                    Some(CallosumReceiveEvent::Envelope { envelope, .. }) => {
                        let Some(watermark_ms) = connected_watermark_ms else {
                            continue;
                        };
                        if envelope.tract != "observe" || envelope.event != "status" {
                            continue;
                        }
                        let now_ms = chrono::Utc::now().timestamp_millis();
                        let Some(timestamp_ms) = envelope.ts else {
                            continue;
                        };
                        if !is_fresh_status_timestamp(timestamp_ms, watermark_ms, now_ms) {
                            continue;
                        }
                        if envelope.extra.get("name").and_then(Value::as_str)
                            == Some("native.observe")
                        {
                            return Ok(Value::Object(envelope.extra));
                        }
                    }
                    Some(CallosumReceiveEvent::Continuity { .. }) => {}
                    None => return Err(Unavailable::Transport),
                }
            }
        })
        .await
        {
            Ok(result) => result,
            Err(_elapsed) => Err(Unavailable::Timeout),
        };
        let _ = tokio::time::timeout(STOP_CLEANUP_BOUND, connection.stop()).await;
        status
    })
}

#[cfg(test)]
mod tests {
    use super::is_fresh_status_timestamp;

    #[test]
    fn observe_status_must_be_strictly_newer_than_connection_and_not_from_the_future() {
        assert!(!is_fresh_status_timestamp(9, 10, 12));
        assert!(!is_fresh_status_timestamp(10, 10, 12));
        assert!(is_fresh_status_timestamp(11, 10, 12));
        assert!(!is_fresh_status_timestamp(13, 10, 12));
    }
}

#[cfg(all(test, feature = "full-tests"))]
#[path = "service_status_tests.rs"]
mod shared_tests;
