// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use axum::body::Body;
use axum::http::StatusCode;
use axum::response::Response;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum OwnerReadRole {
    SpeakersIndex,
    SpeakersGrid,
    SpeakersQuality,
    SpeakersOwnerStatus,
    SpeakersDiscoveryCache,
    SpeakersDiscoveryPresence,
    SpeakersDiscoveryResolveStatement,
    SpeakersPeopleSearch,
    SpeakersServeAudio,
    SpeakersKnown,
    SpeakersSegmentSpeakers,
    SpeakersReview,
    SpeakersMonthStats,
    SpeakersSegments,
    SpeakersSegmentsCli,
    SpeakersReviewCli,
    SpeakersStatus,
    SpeakersSuggest,
    SpeakersKeepSeparate,
    SpeakersDismissals,
    SpeakersIdentifyOperations,
    SpeakersIdentifyOperation,
    TranscriptsRoot,
    TranscriptsIndex,
    TranscriptsMonthStats,
    TranscriptsRanges,
    TranscriptsSegments,
    TranscriptsDay,
    TranscriptsRead,
    TranscriptsSegment,
    TranscriptsServeFile,
    StatsData,
    StatsUsage,
    StatsIndex,
    StatsMonthStats,
    HealthState,
    HealthLog,
    HealthInfo,
    HealthSummary,
    HealthRange,
    HealthPipeline,
    HomePulse,
    HomeBriefing,
    BackupStatus,
    BackupOffloadStatus,
}

impl OwnerReadRole {
    pub const ALL: &'static [Self] = &[
        Self::SpeakersIndex,
        Self::SpeakersGrid,
        Self::SpeakersQuality,
        Self::SpeakersOwnerStatus,
        Self::SpeakersDiscoveryCache,
        Self::SpeakersDiscoveryPresence,
        Self::SpeakersDiscoveryResolveStatement,
        Self::SpeakersPeopleSearch,
        Self::SpeakersServeAudio,
        Self::SpeakersKnown,
        Self::SpeakersSegmentSpeakers,
        Self::SpeakersReview,
        Self::SpeakersMonthStats,
        Self::SpeakersSegments,
        Self::SpeakersSegmentsCli,
        Self::SpeakersReviewCli,
        Self::SpeakersStatus,
        Self::SpeakersSuggest,
        Self::SpeakersKeepSeparate,
        Self::SpeakersDismissals,
        Self::SpeakersIdentifyOperations,
        Self::SpeakersIdentifyOperation,
        Self::TranscriptsRoot,
        Self::TranscriptsIndex,
        Self::TranscriptsMonthStats,
        Self::TranscriptsRanges,
        Self::TranscriptsSegments,
        Self::TranscriptsDay,
        Self::TranscriptsRead,
        Self::TranscriptsSegment,
        Self::TranscriptsServeFile,
        Self::StatsData,
        Self::StatsUsage,
        Self::StatsIndex,
        Self::StatsMonthStats,
        Self::HealthState,
        Self::HealthLog,
        Self::HealthInfo,
        Self::HealthSummary,
        Self::HealthRange,
        Self::HealthPipeline,
        Self::HomePulse,
        Self::HomeBriefing,
        Self::BackupStatus,
        Self::BackupOffloadStatus,
    ];

    pub fn probe_uri(&self) -> &'static str {
        match self {
            Self::SpeakersIndex => "/app/speakers/api/index",
            Self::SpeakersGrid => "/app/speakers/api/grid",
            Self::SpeakersQuality => "/app/speakers/api/quality",
            Self::SpeakersOwnerStatus => "/app/speakers/api/owner/status",
            Self::SpeakersDiscoveryCache => "/app/speakers/api/discovery/cache",
            Self::SpeakersDiscoveryPresence => "/app/speakers/api/discovery/cluster/1/presence",
            Self::SpeakersDiscoveryResolveStatement => {
                "/app/speakers/api/discovery/resolve-statement?voice_day=20260901&voice_stream=default&voice_segment_key=000000_000100&voice_source=mic&voice_sentence_id=1"
            }
            Self::SpeakersPeopleSearch => "/app/speakers/api/people/search?q=a",
            Self::SpeakersServeAudio => "/app/speakers/api/serve_audio/20260901/audio.wav",
            Self::SpeakersKnown => "/app/speakers/api/speakers/known",
            Self::SpeakersSegmentSpeakers => {
                "/app/speakers/api/speakers/20260901/default/000000_000100"
            }
            Self::SpeakersReview => "/app/speakers/api/review/20260901/default/000000_000100/mic",
            Self::SpeakersMonthStats => "/app/speakers/api/stats/202609",
            Self::SpeakersSegments => "/app/speakers/api/segments/20260901",
            Self::SpeakersSegmentsCli => "/app/speakers/api/segments-cli/20260901",
            Self::SpeakersReviewCli => {
                "/app/speakers/api/review-cli/20260901/default/000000_000100/mic"
            }
            Self::SpeakersStatus => "/app/speakers/api/status",
            Self::SpeakersSuggest => "/app/speakers/api/suggest",
            Self::SpeakersKeepSeparate => "/app/speakers/api/name-variants/keep-separate",
            Self::SpeakersDismissals => "/app/speakers/api/discovery/dismissals",
            Self::SpeakersIdentifyOperations => "/app/speakers/api/discovery/identify/operations",
            Self::SpeakersIdentifyOperation => "/app/speakers/api/discovery/identify/operations/1",
            Self::TranscriptsRoot => "/app/transcripts/",
            Self::TranscriptsIndex => "/app/transcripts/api/index",
            Self::TranscriptsMonthStats => "/app/transcripts/api/stats/202609",
            Self::TranscriptsRanges => "/app/transcripts/api/ranges/20260901",
            Self::TranscriptsSegments => "/app/transcripts/api/segments/20260901",
            Self::TranscriptsDay => "/app/transcripts/api/day/20260901",
            Self::TranscriptsRead => "/app/transcripts/api/read/20260901",
            Self::TranscriptsSegment => {
                "/app/transcripts/api/segment/20260901/default/000000_000100"
            }
            Self::TranscriptsServeFile => "/app/transcripts/api/serve_file/20260901/mic.jsonl",
            Self::StatsData => "/app/stats/api/stats",
            Self::StatsUsage => "/app/stats/api/usage",
            Self::StatsIndex => "/app/stats/api/index",
            Self::StatsMonthStats => "/app/stats/api/stats/202609",
            Self::HealthState => "/app/health/api/state",
            Self::HealthLog => "/app/health/api/log?path=chronicle/20260901/health/think.log",
            Self::HealthInfo => "/app/health/api/info",
            Self::HealthSummary => "/api/health/summary",
            Self::HealthRange => "/api/health/range",
            Self::HealthPipeline => "/api/health/pipeline?day=20260901",
            Self::HomePulse => "/app/home/api/pulse",
            Self::HomeBriefing => "/app/home/api/briefing",
            Self::BackupStatus => "/app/backup/status",
            Self::BackupOffloadStatus => "/app/backup/offload/status",
        }
    }
}

pub async fn spawn_blocking_response<F>(role: OwnerReadRole, f: F) -> Response
where
    F: FnOnce() -> Response + Send + 'static,
{
    match tokio::task::spawn_blocking(move || {
        enter_expensive_work(role);
        f()
    })
    .await
    {
        Ok(response) => response,
        Err(_) => join_failed(),
    }
}

pub fn join_failed() -> Response {
    Response::builder()
        .status(StatusCode::INTERNAL_SERVER_ERROR)
        .body(Body::empty())
        .expect("empty 500 response")
}

#[cfg(not(feature = "test-hooks"))]
#[inline(always)]
fn enter_expensive_work(_role: OwnerReadRole) {}

#[cfg(feature = "test-hooks")]
pub fn enter_expensive_work(role: OwnerReadRole) {
    hooks::enter(role);
}

#[cfg(feature = "test-hooks")]
pub mod test_hooks {
    use super::*;

    pub fn hold(role: OwnerReadRole) {
        hooks::hold(role);
    }

    pub fn panic(role: OwnerReadRole) {
        hooks::panic(role);
    }

    pub fn entered(role: OwnerReadRole) -> usize {
        hooks::entered(role)
    }

    pub fn wait_entered(role: OwnerReadRole, n: usize) {
        hooks::wait_entered(role, n);
    }

    pub fn release(role: OwnerReadRole) {
        hooks::release(role);
    }

    pub fn release_all() {
        hooks::release_all();
    }

    pub fn reset() {
        hooks::reset();
    }
}

#[cfg(feature = "test-hooks")]
mod hooks {
    use std::collections::HashMap;
    use std::sync::{Condvar, Mutex, OnceLock};

    use super::OwnerReadRole;

    #[derive(Default)]
    struct State {
        entered: HashMap<OwnerReadRole, usize>,
        held: HashMap<OwnerReadRole, bool>,
        panics: HashMap<OwnerReadRole, bool>,
    }

    struct Coordinator {
        state: Mutex<State>,
        cvar: Condvar,
    }

    fn coordinator() -> &'static Coordinator {
        static COORD: OnceLock<Coordinator> = OnceLock::new();
        COORD.get_or_init(|| Coordinator {
            state: Mutex::new(State::default()),
            cvar: Condvar::new(),
        })
    }

    pub(crate) fn enter(role: OwnerReadRole) {
        let coord = coordinator();
        let mut guard = coord.state.lock().expect("hook lock");
        *guard.entered.entry(role).or_insert(0) += 1;
        coord.cvar.notify_all();
        let should_panic = guard.panics.get(&role).copied().unwrap_or(false);
        if should_panic {
            drop(guard);
            coord.cvar.notify_all();
            std::panic::panic_any("expensive_work_panic");
        }
        while guard.held.get(&role).copied().unwrap_or(false) {
            guard = coord.cvar.wait(guard).expect("hook wait");
        }
    }

    pub(crate) fn hold(role: OwnerReadRole) {
        let coord = coordinator();
        let mut guard = coord.state.lock().expect("hook lock");
        guard.held.insert(role, true);
    }

    pub(crate) fn panic(role: OwnerReadRole) {
        let coord = coordinator();
        let mut guard = coord.state.lock().expect("hook lock");
        guard.panics.insert(role, true);
    }

    pub(crate) fn entered(role: OwnerReadRole) -> usize {
        let coord = coordinator();
        let guard = coord.state.lock().expect("hook lock");
        guard.entered.get(&role).copied().unwrap_or(0)
    }

    pub(crate) fn wait_entered(role: OwnerReadRole, n: usize) {
        let coord = coordinator();
        let mut guard = coord.state.lock().expect("hook lock");
        while guard.entered.get(&role).copied().unwrap_or(0) < n {
            guard = coord.cvar.wait(guard).expect("hook wait");
        }
    }

    pub(crate) fn release(role: OwnerReadRole) {
        let coord = coordinator();
        let mut guard = coord.state.lock().expect("hook lock");
        guard.held.insert(role, false);
        coord.cvar.notify_all();
    }

    pub(crate) fn release_all() {
        let coord = coordinator();
        let mut guard = coord.state.lock().expect("hook lock");
        guard.held.clear();
        coord.cvar.notify_all();
    }

    pub(crate) fn reset() {
        let coord = coordinator();
        let mut guard = coord.state.lock().expect("hook lock");
        guard.entered.clear();
        guard.held.clear();
        guard.panics.clear();
        coord.cvar.notify_all();
    }
}
