// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use super::*;
use std::cell::RefCell;
use std::io::Write;
use std::path::PathBuf;
use std::sync::{Arc, mpsc};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

thread_local! {
    static BEFORE_WAIT: RefCell<Option<Box<dyn FnOnce()>>> = RefCell::new(None);
}

pub(super) fn before_serving_wait(operation: &str) {
    // Only this thread's actual creator request can consume its entry gate.
    // Stop, status and ordinary Start are never parked by this test observer.
    if operation == "creator-recovery" {
        BEFORE_WAIT.with(|slot| {
            if let Some(hook) = slot.borrow_mut().take() {
                hook();
            }
        });
    }
}

struct Temporary(PathBuf);
impl Temporary {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "creator-native-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir(&path).unwrap();
        fs::create_dir(path.join("journal")).unwrap();
        Self(path)
    }
    fn journal(&self) -> PathBuf {
        self.0.join("journal")
    }
    fn command(&self, id: &str, apply: bool) -> String {
        let root = jio::JournalRoot::open(&self.journal()).unwrap();
        let generation = THINKING_GENERATION.load(Ordering::SeqCst);
        let mut request = json!({"op":"creator-recovery","admission_generation":generation,
            "request_id":id,"root_observation":root.identity().observation_id(),"action":"observe"});
        if apply {
            // Producer fixture creates exclusion; observation itself may not.
            drop(jio::activity::ActivityLease::try_acquire(Arc::new(root)).unwrap());
            let observed_id = format!("{id}-observation");
            let observed = serving(&self.journal(), &self.command(&observed_id, false)).unwrap();
            assert!(observed["refusal"].is_null(), "{observed}");
            request["action"] = json!("apply");
            request["observation_request_id"] = json!(observed_id);
            request["observation_epoch"] = json!(generation);
            request["confirmation"] = observed["confirmation"].clone();
            request["confirmation_revision"] = observed["confirmation_revision"].clone();
        }
        let command = request.to_string();
        let mut file = fs::File::create(self.0.join(format!("request-{id}.json"))).unwrap();
        file.write_all(command.as_bytes()).unwrap();
        file.sync_all().unwrap();
        fs::File::open(&self.0).unwrap().sync_all().unwrap();
        command
    }
}
impl Drop for Temporary {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

struct Release(Option<mpsc::Sender<()>>);
impl Drop for Release {
    fn drop(&mut self) {
        if let Some(sender) = self.0.take() {
            let _ = sender.send(());
        }
    }
}

#[test]
fn stopped_native_creator_entry_keeps_epoch_identity_and_lost_reply_custody() {
    request_serving_stop();
    assert!(SERVING.lock().unwrap().is_none());
    assert_eq!(retirement::admission(), retirement::Outcome::Stopped);

    // Exact wrong-root data can refuse but never create activity authority.
    let f = Temporary::new();
    let command = f.command("wrong-root", false);
    let mut wrong: Value = serde_json::from_str(&command).unwrap();
    wrong["root_observation"] = json!("foreign-original-root");
    assert!(serving(&f.journal(), &wrong.to_string()).is_err());
    assert!(!f.journal().join("backup-activity.lock").exists());
    let mut missing: Value = serde_json::from_str(&command).unwrap();
    missing.as_object_mut().unwrap().remove("request_id");
    assert!(serving(&f.journal(), &missing.to_string()).is_err());
    assert!(!f.journal().join("backup-activity.lock").exists());

    // Capture the original ticket, then block the actual native entry before
    // its SERVING wait. Stop must invalidate it before any proof/cleanup work.
    let stage = f
        .journal()
        .join(".tmp-offload-00000000000000000000000000000001");
    fs::create_dir(&stage).unwrap();
    let command = f.command("preworker-stop", false);
    let (entered, at_entry) = mpsc::channel();
    let (release, held) = mpsc::channel();
    let mut release = Release(Some(release));
    let journal = f.journal();
    let worker = std::thread::spawn(move || {
        BEFORE_WAIT.with(|slot| {
            *slot.borrow_mut() = Some(Box::new(move || {
                entered.send(()).unwrap();
                held.recv_timeout(Duration::from_secs(10)).unwrap();
            }))
        });
        serving(&journal, &command)
    });
    at_entry.recv_timeout(Duration::from_secs(10)).unwrap();
    request_serving_stop();
    release.0.take().unwrap().send(()).unwrap();
    assert!(worker.join().unwrap().is_err());
    assert!(stage.exists());
    assert!(!f.journal().join("backup-activity.lock").exists());

    // The request identity is durable before dispatch. Losing the receiving
    // endpoint never repeats Native.run or releases an in-flight native frame.
    let command = f.command("lost-reply", true);
    let expected: Value = serde_json::from_str(&command).unwrap();
    let persisted = fs::read(f.0.join("request-lost-reply.json")).unwrap();
    assert_eq!(persisted, command.as_bytes());
    let (reply, receiver) = mpsc::channel::<Value>();
    drop(receiver);
    let (entered, at_effect) = mpsc::channel();
    let (release, held) = mpsc::channel();
    let mut release = Release(Some(release));
    let journal = f.journal();
    let native = std::thread::spawn(move || {
        let (result, used) = jio::media::run_with_media_hook(
            jio::media::MediaPrimitive::ParentSync,
            move || {
                entered.send(()).unwrap();
                held.recv_timeout(Duration::from_secs(10)).unwrap();
                Ok(())
            },
            || serving(&journal, &command),
        );
        assert!(used);
        let result = result.unwrap();
        let delivery_lost = reply.send(result.clone()).is_err();
        (result, delivery_lost)
    });
    at_effect.recv_timeout(Duration::from_secs(10)).unwrap();
    assert!(!stage.exists());
    let original = Arc::new(jio::JournalRoot::open(&f.journal()).unwrap());
    assert!(jio::activity::ActivityLease::try_acquire(Arc::clone(&original)).is_err());
    assert!(SERVING.try_lock().is_err());
    let epoch = THINKING_GENERATION.load(Ordering::SeqCst);
    let (stopped, stop_result) = mpsc::channel();
    let journal = f.journal();
    let stop = std::thread::spawn(move || {
        let result = serving(&journal, "{\"op\":\"stop\"}");
        stopped.send(result).unwrap();
    });
    let deadline = Instant::now() + Duration::from_secs(5);
    while THINKING_GENERATION.load(Ordering::SeqCst) == epoch {
        assert!(
            Instant::now() < deadline,
            "actual Stop did not deliver cancellation"
        );
        std::thread::sleep(Duration::from_millis(1));
    }
    // Stop has actually executed its cancellation prelude. A returned Stop
    // while the native effect frame is held is a failure, not an observation.
    let before_release = stop_result.recv_timeout(Duration::from_millis(100));
    let premature = before_release.is_ok();
    release.0.take().unwrap().send(()).unwrap();
    let (report, lost) = native.join().unwrap();
    assert!(lost);
    let stopped = match before_release {
        Ok(value) => value,
        Err(mpsc::RecvTimeoutError::Timeout) => {
            stop_result.recv_timeout(Duration::from_secs(10)).unwrap()
        }
        Err(error) => panic!("Stop receiver failed: {error}"),
    }
    .unwrap();
    stop.join().unwrap();
    assert!(
        !premature,
        "actual Stop returned while the native cleanup frame was held"
    );
    assert_eq!(stopped["stopped"], true);
    assert_eq!(report["request_id"], expected["request_id"]);
    assert_eq!(report["root_observation"], expected["root_observation"]);
    assert_eq!(report["remote_outcome"], "unknown");
    assert_eq!(report["replay_authorized"], false);
    assert_eq!(
        report["stages"][0]["facts"]["directory_absence"],
        "Confirmed"
    );
    assert!(jio::activity::ActivityLease::try_acquire(original).is_ok());
    assert_eq!(
        fs::read(f.0.join("request-lost-reply.json")).unwrap(),
        persisted
    );
    assert!(!f.journal().join("config").exists());
    assert!(!f.journal().join("health").exists());
    eprintln!(
        "creator_native_receiving request_persisted=true creator_invocations=1 reply_lost=true stop_waited_for_frame=true remote_outcome=unknown replay=false"
    );
}
