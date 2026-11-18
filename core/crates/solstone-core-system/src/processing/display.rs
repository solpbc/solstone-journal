// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::path::Path;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use super::DisplayPowersaveReading;

const DEBOUNCE: Duration = Duration::from_secs(120);
const MAX_SAMPLE_GAP: Duration = Duration::from_secs(10);

#[derive(Default)]
struct Monitor {
    sampled: Option<Instant>,
    asleep_since: Option<Instant>,
    reading: Option<DisplayPowersaveReading>,
    published: Option<(Instant, DisplayPowersaveReading)>,
}

impl Monitor {
    fn update(&mut self, state: Option<bool>, now: Instant) -> DisplayPowersaveReading {
        if self
            .sampled
            .is_some_and(|last| now.saturating_duration_since(last) > MAX_SAMPLE_GAP)
        {
            self.asleep_since = None;
        }
        self.sampled = Some(now);
        if state != Some(true) {
            self.asleep_since = None;
        }
        let debounced = state == Some(true)
            && now.saturating_duration_since(*self.asleep_since.get_or_insert(now)) >= DEBOUNCE;
        let reading = DisplayPowersaveReading {
            available: state.is_some(),
            asleep: state == Some(true),
            debounced,
        };
        self.reading = Some(reading);
        reading
    }
}

static MONITOR: Mutex<Monitor> = Mutex::new(Monitor {
    sampled: None,
    asleep_since: None,
    reading: None,
    published: None,
});

pub(super) fn observe_display() -> DisplayPowersaveReading {
    let mut monitor = MONITOR.lock().expect("display monitor");
    let now = Instant::now();
    if monitor
        .sampled
        .is_some_and(|last| now.saturating_duration_since(last) < Duration::from_secs(1))
    {
        return monitor
            .reading
            .unwrap_or(DisplayPowersaveReading::UNAVAILABLE);
    }
    monitor.update(native_state(), now)
}

/// The supervisor publishes the debounce it actually uses. Health never starts
/// a separate debounce clock, and a stopped supervisor's snapshot expires.
pub fn publish_display_snapshot(journal: &Path) {
    let reading = observe_display();
    let now = Instant::now();
    {
        let mut monitor = MONITOR.lock().expect("display monitor");
        if monitor.published.is_some_and(|(last, previous)| {
            previous == reading && now.saturating_duration_since(last) < Duration::from_secs(15)
        }) {
            return;
        }
        monitor.published = Some((now, reading));
    }
    let write = || -> Result<(), String> {
        std::fs::create_dir_all(journal.join("health")).map_err(|e| e.to_string())?;
        solstone_core_journal_io::write_json(
            journal.join("health/processing-display.json"),
            &serde_json::json!({"sampled_at": jiff::Timestamp::now().as_millisecond(), "available": reading.available, "asleep": reading.asleep, "debounced": reading.debounced}),
            solstone_core_journal_io::JsonWriteOptions::default(),
        ).map_err(|e| e.to_string())
    };
    if let Err(error) = write() {
        log::warn!("could not publish display state: {error}");
    }
}

pub fn read_display_snapshot(journal: &Path) -> DisplayPowersaveReading {
    let read = || -> Option<DisplayPowersaveReading> {
        let value: serde_json::Value = serde_json::from_slice(
            &std::fs::read(journal.join("health/processing-display.json")).ok()?,
        )
        .ok()?;
        let age = jiff::Timestamp::now()
            .as_millisecond()
            .checked_sub(value.get("sampled_at")?.as_i64()?)?;
        if !(0..=30_000).contains(&age) {
            return None;
        }
        Some(DisplayPowersaveReading {
            available: value.get("available")?.as_bool()?,
            asleep: value.get("asleep")?.as_bool()?,
            debounced: value.get("debounced")?.as_bool()?,
        })
    };
    read().unwrap_or(DisplayPowersaveReading::UNAVAILABLE)
}

#[cfg(target_os = "linux")]
fn native_state() -> Option<bool> {
    linux_state(Path::new("/sys/class/drm"))
}

#[cfg(any(target_os = "linux", test))]
fn linux_state(root: &Path) -> Option<bool> {
    let mut connected = 0;
    let mut asleep = true;
    for entry in std::fs::read_dir(root).ok()? {
        let entry = entry.ok()?;
        let name = entry.file_name();
        let name = name.to_str()?;
        if !name.starts_with("card") || !name.contains('-') {
            continue;
        }
        let status = std::fs::read_to_string(entry.path().join("status")).ok()?;
        match status.trim() {
            "disconnected" => continue,
            "connected" => connected += 1,
            _ => return None,
        }
        match std::fs::read_to_string(entry.path().join("dpms"))
            .ok()?
            .trim()
        {
            "On" => asleep = false,
            "Off" | "Standby" | "Suspend" => {}
            _ => return None,
        }
    }
    (connected > 0).then_some(asleep)
}

#[cfg(target_os = "macos")]
#[allow(unsafe_code)]
fn native_state() -> Option<bool> {
    #[link(name = "CoreGraphics", kind = "framework")]
    unsafe extern "C" {
        fn CGGetOnlineDisplayList(max: u32, displays: *mut u32, count: *mut u32) -> i32;
        fn CGDisplayIsAsleep(display: u32) -> u32;
    }
    let mut displays = [0u32; 32];
    let mut count = 0;
    // SAFETY: the buffer has the advertised capacity; CoreGraphics writes at
    // most that many display IDs. These functions retain no pointers.
    unsafe {
        if CGGetOnlineDisplayList(displays.len() as u32, displays.as_mut_ptr(), &mut count) != 0
            || count == 0
            || count as usize >= displays.len()
        {
            return None;
        }
        Some(
            displays[..count as usize]
                .iter()
                .all(|id| CGDisplayIsAsleep(*id) != 0),
        )
    }
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
fn native_state() -> Option<bool> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sleep_must_be_continuously_observed_and_wake_or_unknown_resets_it() {
        let mut monitor = Monitor::default();
        let start = Instant::now();
        for second in 0..=120 {
            let reading = monitor.update(Some(true), start + Duration::from_secs(second));
            assert_eq!(reading.debounced, second == 120);
        }
        let mut interrupted = Monitor::default();
        for second in 0..=120 {
            interrupted.update(Some(true), start + Duration::from_secs(second));
        }
        assert!(
            !interrupted
                .update(Some(true), start + Duration::from_secs(132))
                .debounced
        );
        assert!(
            !monitor
                .update(Some(false), start + Duration::from_secs(121))
                .debounced
        );
        assert!(
            !monitor
                .update(Some(true), start + Duration::from_secs(122))
                .debounced
        );
        assert!(
            !monitor
                .update(None, start + Duration::from_secs(123))
                .available
        );
        assert!(
            !monitor
                .update(Some(true), start + Duration::from_secs(250))
                .debounced
        );
    }

    #[test]
    fn linux_requires_every_connected_display_to_be_known_asleep() {
        let root = tempfile::tempdir().unwrap();
        let a = root.path().join("card0-DP-1");
        let b = root.path().join("card0-HDMI-A-1");
        for path in [&a, &b] {
            std::fs::create_dir(path).unwrap();
            std::fs::write(path.join("status"), "connected\n").unwrap();
            std::fs::write(path.join("dpms"), "Off\n").unwrap();
        }
        assert_eq!(linux_state(root.path()), Some(true));
        std::fs::write(b.join("dpms"), "On\n").unwrap();
        assert_eq!(linux_state(root.path()), Some(false));
        std::fs::write(b.join("status"), "disconnected\n").unwrap();
        assert_eq!(linux_state(root.path()), Some(true));
        std::fs::remove_file(a.join("dpms")).unwrap();
        assert_eq!(linux_state(root.path()), None);
        std::fs::write(a.join("status"), "unknown\n").unwrap();
        assert_eq!(linux_state(root.path()), None);
        std::fs::write(a.join("status"), "disconnected\n").unwrap();
        assert_eq!(linux_state(root.path()), None);
    }
}
