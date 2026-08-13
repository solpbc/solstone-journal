// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

#[cfg(target_os = "macos")]
use std::process::Command;

use serde_json::{Map, Value};

const MIB: u64 = 1024 * 1024;
const GIB: u64 = 1024 * MIB;

#[derive(Clone, Copy, Debug, Default)]
pub struct ThrottleState {
    pub throttled: bool,
    pub count: u32,
    pub floor_mib: Option<u64>,
    pub available_mib: Option<u64>,
}

pub trait MemoryProbe: Send + Sync {
    fn available_bytes(&self) -> Option<u64>;
    fn total_bytes(&self) -> Option<u64>;
    fn unified_memory(&self) -> bool;
}

#[derive(Default)]
pub struct SystemMemoryProbe;
impl MemoryProbe for SystemMemoryProbe {
    fn available_bytes(&self) -> Option<u64> {
        #[cfg(target_os = "linux")]
        {
            let text = std::fs::read_to_string("/proc/meminfo").ok()?;
            return meminfo(&text, "MemAvailable");
        }
        #[cfg(target_os = "macos")]
        {
            return command_text("/usr/bin/vm_stat", &[]).and_then(|text| macos_available(&text));
        }
        #[allow(unreachable_code)]
        None
    }
    fn total_bytes(&self) -> Option<u64> {
        #[cfg(target_os = "linux")]
        {
            let text = std::fs::read_to_string("/proc/meminfo").ok()?;
            return meminfo(&text, "MemTotal");
        }
        #[cfg(target_os = "macos")]
        {
            return command_text("/usr/sbin/sysctl", &["-n", "hw.memsize"])
                .and_then(|text| macos_total(&text));
        }
        #[allow(unreachable_code)]
        None
    }
    fn unified_memory(&self) -> bool {
        std::env::consts::OS == "macos" && std::env::consts::ARCH == "aarch64"
    }
}

#[cfg(target_os = "macos")]
fn command_text(program: &str, args: &[&str]) -> Option<String> {
    Command::new(program)
        .args(args)
        .output()
        .ok()
        .filter(|output| output.status.success())
        .and_then(|output| String::from_utf8(output.stdout).ok())
}

#[cfg(any(target_os = "macos", test))]
fn macos_total(text: &str) -> Option<u64> {
    text.trim().parse().ok()
}

#[cfg(any(target_os = "macos", test))]
fn macos_available(text: &str) -> Option<u64> {
    let page_size = text
        .split("page size of ")
        .nth(1)?
        .split_whitespace()
        .next()?
        .parse::<u64>()
        .ok()?;
    let pages = text
        .lines()
        .filter(|line| line.starts_with("Pages free") || line.starts_with("Pages inactive"))
        .try_fold(0_u64, |total, line| {
            let value = line
                .split_once(':')?
                .1
                .trim()
                .trim_end_matches('.')
                .parse::<u64>()
                .ok()?;
            total.checked_add(value)
        })?;
    pages.checked_mul(page_size)
}
#[cfg(target_os = "linux")]
fn meminfo(text: &str, wanted: &str) -> Option<u64> {
    let value = text.lines().find_map(|line| {
        let (key, rest) = line.split_once(':')?;
        (key == wanted)
            .then(|| rest.split_whitespace().next()?.parse::<u64>().ok())
            .flatten()
    })?;
    value.checked_mul(1024)
}

#[derive(Clone)]
pub struct Admission {
    probe: Arc<dyn MemoryProbe>,
    state: Arc<Mutex<ThrottleState>>,
}
impl Admission {
    pub fn new(probe: Arc<dyn MemoryProbe>) -> Self {
        Self {
            probe,
            state: Arc::new(Mutex::new(ThrottleState::default())),
        }
    }
    pub fn state(&self) -> ThrottleState {
        *self.state.lock().expect("throttle lock")
    }
    pub fn floor_bytes(&self, config: &Map<String, Value>) -> u64 {
        if let Some(value) = config
            .get("memory")
            .and_then(Value::as_object)
            .and_then(|v| v.get("floor_mib"))
            .and_then(Value::as_u64)
        {
            return value.saturating_mul(MIB);
        }
        if !self.probe.unified_memory() {
            return 0;
        }
        let stt_floor = if std::env::consts::OS == "macos" {
            2 * GIB
        } else {
            4 * GIB
        };
        let auto = self
            .probe
            .total_bytes()
            .map(|v| (v as f64 * 0.06) as u64)
            .unwrap_or(stt_floor + GIB);
        auto.clamp(stt_floor + GIB, 12 * GIB)
    }
    pub fn wait<F, S, E>(
        &self,
        _stage: &str,
        config: &Map<String, Value>,
        should_stop: S,
        on_start: F,
        on_end: E,
    ) -> bool
    where
        F: FnOnce(u64, u64),
        S: Fn() -> bool,
        E: FnOnce(f64),
    {
        let floor = self.floor_bytes(config);
        if floor == 0 {
            return !should_stop();
        }
        let started = Instant::now();
        let mut began = false;
        let mut on_start = Some(on_start);
        let mut on_end = Some(on_end);
        loop {
            if should_stop() {
                if began {
                    on_end.take().expect("end")(started.elapsed().as_secs_f64());
                    self.finish();
                }
                return false;
            }
            match self.probe.available_bytes() {
                None => {
                    if began {
                        on_end.take().expect("end")(started.elapsed().as_secs_f64());
                        self.finish();
                    }
                    return true;
                }
                Some(available) if available >= floor => {
                    if began {
                        on_end.take().expect("end")(started.elapsed().as_secs_f64());
                        self.finish();
                    }
                    return true;
                }
                Some(available) => {
                    if !began {
                        began = true;
                        let mut s = self.state.lock().expect("throttle lock");
                        s.throttled = true;
                        s.count += 1;
                        s.floor_mib = Some(floor / MIB);
                        s.available_mib = Some(available / MIB);
                        drop(s);
                        on_start.take().expect("start")(available, floor);
                    } else {
                        self.state.lock().expect("throttle lock").available_mib =
                            Some(available / MIB);
                    }
                    std::thread::sleep(Duration::from_secs(1));
                }
            }
        }
    }
    fn finish(&self) {
        let mut s = self.state.lock().expect("throttle lock");
        s.count = s.count.saturating_sub(1);
        if s.count == 0 {
            *s = ThrottleState::default();
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicBool, Ordering};
    struct Probe;
    impl MemoryProbe for Probe {
        fn available_bytes(&self) -> Option<u64> {
            Some(1)
        }
        fn total_bytes(&self) -> Option<u64> {
            Some(100 * GIB)
        }
        fn unified_memory(&self) -> bool {
            true
        }
    }
    #[test]
    fn explicit_floor_is_used() {
        let a = Admission::new(Arc::new(Probe));
        assert_eq!(
            a.floor_bytes(
                &serde_json::json!({"memory":{"floor_mib":7}})
                    .as_object()
                    .unwrap()
                    .clone()
            ),
            7 * MIB
        );
    }

    #[test]
    fn throttle_emits_one_start_and_completion_when_stop_arrives() {
        let admission = Admission::new(Arc::new(Probe));
        let stop = Arc::new(AtomicBool::new(false));
        let started = Arc::new(AtomicBool::new(false));
        let completed = Arc::new(AtomicBool::new(false));
        let stopped = Arc::clone(&stop);
        let start_seen = Arc::clone(&started);
        let completed_seen = Arc::clone(&completed);
        let admitted = admission.wait(
            "describe",
            &serde_json::json!({"memory":{"floor_mib":7}})
                .as_object()
                .unwrap()
                .clone(),
            || stop.load(Ordering::SeqCst),
            move |available, floor| {
                assert_eq!(available, 1);
                assert_eq!(floor, 7 * MIB);
                start_seen.store(true, Ordering::SeqCst);
                stopped.store(true, Ordering::SeqCst);
            },
            move |waited| {
                assert!(waited >= 1.0);
                completed_seen.store(true, Ordering::SeqCst);
            },
        );
        assert!(!admitted);
        assert!(started.load(Ordering::SeqCst));
        assert!(completed.load(Ordering::SeqCst));
        assert!(!admission.state().throttled);
    }

    #[test]
    fn macos_vm_stat_memory_arithmetic_is_correct() {
        let vm_stat = "Mach Virtual Memory Statistics: (page size of 4096 bytes)\nPages free:                               10.\nPages inactive:                           20.\n";
        assert_eq!(macos_total("34359738368\n"), Some(34_359_738_368));
        assert_eq!(macos_available(vm_stat), Some(30 * 4096));
    }
}
