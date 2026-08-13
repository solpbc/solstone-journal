// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::collections::VecDeque;

use serde_json::json;
use solstone_core_top::{
    FrameSample, PlainTopStyle, ProcessBirth, ProcessIdentity, ProcessObserver, ProcessSample,
    ReductionSample, TopState, cleanup_processes, render_frame,
};

struct Samples(VecDeque<ProcessSample>);

impl ProcessObserver for Samples {
    fn sample(&mut self, _pid: u32, _monotonic_seconds: f64) -> ProcessSample {
        self.0.pop_front().expect("scheduled process sample")
    }
}

fn live(pid: u32, birth: u64, rss_bytes: u64, cpu_percent: f64) -> ProcessSample {
    ProcessSample::Live {
        identity: ProcessIdentity {
            pid,
            birth: ProcessBirth::LinuxStartTicks(birth),
        },
        rss_bytes,
        cpu_percent,
    }
}

fn sample(wall_seconds: f64, monotonic_seconds: f64) -> ReductionSample {
    ReductionSample {
        wall_seconds,
        monotonic_seconds,
        wall_datetime: json!({"datetime":"fixture"}),
    }
}

#[test]
fn monotonic_time_controls_ghost_runtime_and_observe_hysteresis() {
    let mut state = TopState::default();
    state.running_tasks.insert(
        "gone".to_owned(),
        json!({"ref":"gone","name":"gone","pid":91}),
    );
    let mut observer = Samples(VecDeque::from([ProcessSample::Missing]));
    cleanup_processes(&mut state, &sample(10_000.0, 0.0), &mut observer);
    assert!(state.finished_tasks.contains_key("gone"));
    cleanup_processes(&mut state, &sample(-1_000_000.0, 5.0), &mut observer);
    assert!(state.finished_tasks.contains_key("gone"));
    cleanup_processes(&mut state, &sample(9_999_999.0, 5.1), &mut observer);
    assert!(!state.finished_tasks.contains_key("gone"));

    state.observe_status = [
        ("mode".to_owned(), json!("idle")),
        (
            "screencast".to_owned(),
            json!({"window_elapsed_seconds": 1}),
        ),
    ]
    .into();
    state.displayed_mode = "screencast".to_owned();
    state.last_active_ts = 10.0;
    let before = render_frame(
        &state,
        FrameSample {
            wall_seconds: -9_999.0,
            monotonic_seconds: 19.999,
        },
        120,
        &PlainTopStyle,
    );
    let boundary = render_frame(
        &state,
        FrameSample {
            wall_seconds: 9_999_999.0,
            monotonic_seconds: 20.0,
        },
        120,
        &PlainTopStyle,
    );
    assert!(before.contains("[LIVE]"));
    assert!(boundary.contains("[IDLE]"));
}

#[test]
fn birth_identity_resets_pid_cache_and_denied_hides_metrics() {
    let mut state = TopState::default();
    state.running_tasks.insert(
        "task".to_owned(),
        json!({"ref":"task","name":"task","pid":44}),
    );
    let mut observer = Samples(VecDeque::from([
        live(44, 100, 10 * 1_048_576, 12.0),
        live(44, 100, 11 * 1_048_576, 27.0),
        live(44, 200, 2 * 1_048_576, 0.0),
        ProcessSample::AccessDenied,
    ]));

    cleanup_processes(&mut state, &sample(0.0, 1.0), &mut observer);
    cleanup_processes(&mut state, &sample(1_000_000.0, 2.0), &mut observer);
    assert_eq!(state.cpu_cache.get(&44), Some(&27.0));
    assert_eq!(state.memory_cache.get(&44), Some(&(11 * 1_048_576)));

    cleanup_processes(&mut state, &sample(-1_000_000.0, 3.0), &mut observer);
    assert_eq!(
        state.process_identities.get(&44),
        Some(&ProcessIdentity {
            pid: 44,
            birth: ProcessBirth::LinuxStartTicks(200),
        })
    );
    assert_eq!(state.cpu_cache.get(&44), Some(&0.0));
    assert_eq!(state.memory_cache.get(&44), Some(&(2 * 1_048_576)));

    cleanup_processes(&mut state, &sample(5_000_000.0, 4.0), &mut observer);
    assert!(!state.cpu_pids.contains(&44));
    assert!(!state.cpu_cache.contains_key(&44));
    assert!(!state.memory_cache.contains_key(&44));
    assert!(!state.process_identities.contains_key(&44));
}
