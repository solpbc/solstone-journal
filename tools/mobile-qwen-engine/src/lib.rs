// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! App-owned, joined lifetime for the separately linked native Qwen engine.

use std::path::{Path, PathBuf};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RuntimeIdentity {
    pub abi: u32,
    pub source_revision: String,
    pub runtime_identity: String,
}

// Deliberately not Debug: credentials never belong in diagnostics.
pub struct Config {
    pub identity: RuntimeIdentity,
    pub generation: u64,
    pub model_generation: String,
    pub model_path: PathBuf,
    pub projector_path: PathBuf,
    pub bearer_token: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum State {
    Created,
    Loading,
    Ready,
    Stopping,
    Stopped,
    Failed,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Snapshot {
    pub generation: u64,
    pub state: State,
    pub port: Option<std::num::NonZeroU16>,
    pub engine_result: i32,
    pub error: String,
}

pub fn runtime_identity() -> Result<RuntimeIdentity, String> {
    #[cfg(journal_mobile_qwen_native)]
    {
        native::identity()
    }
    #[cfg(not(journal_mobile_qwen_native))]
    {
        Err("the bundled local engine is not linked into this app".into())
    }
}

/// Register only the library linked into this app. This does not load a model
/// or choose a provider for the owner.
pub fn register_bundled_runtime(manifest: &Path, image: &Path) -> Result<(), String> {
    let native = runtime_identity()?;
    verify_package(manifest, image, &native)?;
    let identity = solstone_core_local::embedded::RuntimeIdentity {
        artifact_key: solstone_core_local::install::pins::platform_key(),
        abi: native.abi,
        source_revision: native.source_revision,
        runtime_identity: native.runtime_identity,
    };
    identity.validate()?;
    solstone_core_local::embedded::register(std::sync::Arc::new(Factory { identity }))
}

fn verify_package(manifest: &Path, image: &Path, identity: &RuntimeIdentity) -> Result<(), String> {
    use sha2::{Digest, Sha256};
    use std::io::Read;

    let manifest_file = std::fs::File::open(manifest)
        .map_err(|error| format!("could not open the local runtime package record: {error}"))?;
    if manifest_file
        .metadata()
        .map_err(|error| error.to_string())?
        .len()
        > 64 * 1024
    {
        return Err("the local runtime package record is too large".into());
    }
    let record: serde_json::Value = serde_json::from_reader(manifest_file)
        .map_err(|error| format!("could not read the local runtime package record: {error}"))?;
    if record["schema"] != "solstone.mobile-qwen-package.v1"
        || record["runtime_identity"].as_str() != Some(identity.runtime_identity.as_str())
    {
        return Err("the local runtime package record does not match the loaded engine".into());
    }
    let expected = record["image_sha256"]
        .as_str()
        .filter(|digest| digest.len() == 64 && digest.bytes().all(|b| b.is_ascii_hexdigit()))
        .ok_or("the local runtime package has no valid image digest")?;
    let mut image = std::fs::File::open(image)
        .map_err(|error| format!("could not open the packaged local runtime: {error}"))?;
    if !image
        .metadata()
        .map_err(|error| error.to_string())?
        .is_file()
    {
        return Err("the packaged local runtime is not a regular file".into());
    }
    let mut digest = Sha256::new();
    let mut buffer = [0; 64 * 1024];
    loop {
        let count = image
            .read(&mut buffer)
            .map_err(|error| format!("could not verify the packaged local runtime: {error}"))?;
        if count == 0 {
            break;
        }
        digest.update(&buffer[..count]);
    }
    let observed: String = digest
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    if observed != expected {
        return Err("the packaged local runtime did not pass verification".into());
    }
    Ok(())
}

struct Factory {
    identity: solstone_core_local::embedded::RuntimeIdentity,
}

impl solstone_core_local::embedded::Factory for Factory {
    fn identity(&self) -> solstone_core_local::embedded::RuntimeIdentity {
        self.identity.clone()
    }

    fn start(
        &self,
        launch: solstone_core_local::embedded::Launch,
    ) -> Result<Box<dyn solstone_core_local::embedded::Engine>, String> {
        if launch.identity != self.identity {
            return Err("the local engine package changed before launch".into());
        }
        let engine = Engine::start(Config {
            identity: RuntimeIdentity {
                abi: self.identity.abi,
                source_revision: self.identity.source_revision.clone(),
                runtime_identity: self.identity.runtime_identity.clone(),
            },
            generation: launch.generation,
            model_generation: launch.model_generation,
            model_path: launch.model_path,
            projector_path: launch.projector_path,
            bearer_token: launch.bearer_token,
        })?;
        Ok(Box::new(engine))
    }
}

fn provider_snapshot(snapshot: Snapshot) -> solstone_core_local::embedded::Snapshot {
    use solstone_core_local::embedded::Phase;
    solstone_core_local::embedded::Snapshot {
        generation: snapshot.generation,
        phase: match snapshot.state {
            State::Created => Phase::Created,
            State::Loading => Phase::Loading,
            State::Ready => Phase::Ready,
            State::Stopping => Phase::Stopping,
            State::Stopped => Phase::Stopped,
            State::Failed => Phase::Failed,
        },
        port: snapshot.port,
        engine_result: snapshot.engine_result,
        error: snapshot.error,
    }
}

impl solstone_core_local::embedded::Engine for Engine {
    fn snapshot(&self) -> Result<solstone_core_local::embedded::Snapshot, String> {
        self.snapshot().map(provider_snapshot)
    }

    fn request_stop(&self) {
        self.request_stop();
    }

    fn is_finished(&self) -> bool {
        self.is_finished()
    }

    fn stop_and_join(&mut self) -> Result<solstone_core_local::embedded::Snapshot, String> {
        self.stop_and_join().map(provider_snapshot)
    }
}

/// The caller keeps the verified model-generation use lease until this owner
/// has joined. Dropping the owner also requests Stop and joins; it never frees
/// a native handle while its worker can still use it.
pub struct Engine {
    #[cfg(journal_mobile_qwen_native)]
    native: native::Engine,
}

impl Engine {
    pub fn start(config: Config) -> Result<Self, String> {
        #[cfg(journal_mobile_qwen_native)]
        {
            Ok(Self {
                native: native::Engine::start(config)?,
            })
        }
        #[cfg(not(journal_mobile_qwen_native))]
        {
            let _ = config;
            Err("the bundled local engine is not linked into this app".into())
        }
    }

    pub fn snapshot(&self) -> Result<Snapshot, String> {
        #[cfg(journal_mobile_qwen_native)]
        {
            self.native.snapshot()
        }
        #[cfg(not(journal_mobile_qwen_native))]
        {
            Err("the bundled local engine is not linked into this app".into())
        }
    }

    pub fn request_stop(&self) {
        #[cfg(journal_mobile_qwen_native)]
        self.native.request_stop();
    }

    pub fn is_finished(&self) -> bool {
        #[cfg(journal_mobile_qwen_native)]
        {
            self.native.is_finished()
        }
        #[cfg(not(journal_mobile_qwen_native))]
        {
            true
        }
    }

    /// Blocking retirement; call from the app's processing worker, not its UI.
    pub fn stop_and_join(&mut self) -> Result<Snapshot, String> {
        #[cfg(journal_mobile_qwen_native)]
        {
            self.native.stop_and_join()
        }
        #[cfg(not(journal_mobile_qwen_native))]
        {
            Err("the bundled local engine is not linked into this app".into())
        }
    }
}

#[cfg(journal_mobile_qwen_native)]
mod native {
    use super::{Config, RuntimeIdentity, Snapshot, State};
    use std::ffi::{CStr, CString, c_char};
    use std::ptr::NonNull;
    use std::sync::Arc;
    use std::thread::{self, JoinHandle};

    const ABI: u32 = 1;
    const ERROR_CAPACITY: usize = 512;

    #[repr(C)]
    struct Handle {
        _private: [u8; 0],
    }

    #[repr(C)]
    struct WireConfig {
        abi_version: u32,
        struct_size: u32,
        generation: u64,
        expected_runtime_identity: *const c_char,
        expected_source_revision: *const c_char,
        model_generation: *const c_char,
        model_path: *const c_char,
        projector_path: *const c_char,
        bearer_token: *const c_char,
    }

    #[repr(C)]
    struct WireSnapshot {
        abi_version: u32,
        struct_size: u32,
        generation: u64,
        state: u32,
        bound_port: u32,
        engine_result: i32,
        error: [c_char; ERROR_CAPACITY],
    }

    unsafe extern "C" {
        fn journal_mobile_qwen_abi_version() -> u32;
        fn journal_mobile_qwen_source_revision() -> *const c_char;
        fn journal_mobile_qwen_runtime_identity() -> *const c_char;
        fn journal_mobile_qwen_create_v1(
            config: *const WireConfig,
            out: *mut *mut Handle,
            error: *mut c_char,
            error_capacity: usize,
        ) -> i32;
        fn journal_mobile_qwen_run_v1(handle: *mut Handle) -> i32;
        fn journal_mobile_qwen_request_stop_v1(handle: *mut Handle) -> i32;
        fn journal_mobile_qwen_get_snapshot_v1(
            handle: *mut Handle,
            snapshot: *mut WireSnapshot,
        ) -> i32;
        fn journal_mobile_qwen_destroy_v1(handle: *mut Handle) -> i32;
    }

    fn exported_text(pointer: *const c_char) -> Result<String, String> {
        if pointer.is_null() {
            return Err("the bundled engine identity is unavailable".into());
        }
        // The linked ABI exports immutable, NUL-terminated static strings.
        let text = unsafe { CStr::from_ptr(pointer) }
            .to_str()
            .map_err(|_| "the bundled engine identity is invalid")?;
        if text.is_empty() {
            return Err("the bundled engine identity is empty".into());
        }
        Ok(text.to_owned())
    }

    pub(super) fn identity() -> Result<RuntimeIdentity, String> {
        // All three functions read immutable linked-library identity.
        let abi = unsafe { journal_mobile_qwen_abi_version() };
        if abi != ABI {
            return Err("the bundled engine ABI is incompatible".into());
        }
        Ok(RuntimeIdentity {
            abi,
            source_revision: exported_text(unsafe { journal_mobile_qwen_source_revision() })?,
            runtime_identity: exported_text(unsafe { journal_mobile_qwen_runtime_identity() })?,
        })
    }

    fn error_text(bytes: &[c_char]) -> String {
        let bytes: Vec<u8> = bytes.iter().map(|value| *value as u8).collect();
        let end = bytes
            .iter()
            .position(|value| *value == 0)
            .unwrap_or(bytes.len());
        String::from_utf8_lossy(&bytes[..end]).into_owned()
    }

    struct OwnedHandle {
        pointer: NonNull<Handle>,
        generation: u64,
    }

    // The native ABI permits run, snapshot and request_stop concurrently. The
    // final Arc is released only after run has returned and its worker joined.
    unsafe impl Send for OwnedHandle {}
    unsafe impl Sync for OwnedHandle {}

    impl Drop for OwnedHandle {
        fn drop(&mut self) {
            // No native caller can retain this pointer after the final Arc.
            let result = unsafe { journal_mobile_qwen_destroy_v1(self.pointer.as_ptr()) };
            debug_assert_eq!(result, 0, "joined engine handle must be destroyable");
        }
    }

    pub(super) struct Engine {
        handle: Arc<OwnedHandle>,
        worker: Option<JoinHandle<i32>>,
    }

    impl Engine {
        pub(super) fn start(config: Config) -> Result<Self, String> {
            if config.identity != identity()? {
                return Err("the bundled engine changed since setup was checked".into());
            }
            let strings = [
                config.identity.runtime_identity,
                config.identity.source_revision,
                config.model_generation,
                config
                    .model_path
                    .to_str()
                    .ok_or("invalid model path")?
                    .to_owned(),
                config
                    .projector_path
                    .to_str()
                    .ok_or("invalid projector path")?
                    .to_owned(),
                config.bearer_token,
            ]
            .into_iter()
            .map(|value| CString::new(value).map_err(|_| "invalid local engine configuration"))
            .collect::<Result<Vec<_>, _>>()?;
            let wire = WireConfig {
                abi_version: ABI,
                struct_size: std::mem::size_of::<WireConfig>() as u32,
                generation: config.generation,
                expected_runtime_identity: strings[0].as_ptr(),
                expected_source_revision: strings[1].as_ptr(),
                model_generation: strings[2].as_ptr(),
                model_path: strings[3].as_ptr(),
                projector_path: strings[4].as_ptr(),
                bearer_token: strings[5].as_ptr(),
            };
            let mut pointer = std::ptr::null_mut();
            let mut error = [0; ERROR_CAPACITY];
            // create copies every string before returning and starts no worker.
            let result = unsafe {
                journal_mobile_qwen_create_v1(&wire, &mut pointer, error.as_mut_ptr(), error.len())
            };
            if result != 0 {
                return Err(format!(
                    "could not create the local engine: {}",
                    error_text(&error)
                ));
            }
            let handle = Arc::new(OwnedHandle {
                pointer: NonNull::new(pointer).ok_or("local engine returned no handle")?,
                generation: config.generation,
            });
            let run = handle.clone();
            let worker = thread::Builder::new()
                .name("local-thinking".into())
                .spawn(move || unsafe { journal_mobile_qwen_run_v1(run.pointer.as_ptr()) })
                .map_err(|error| format!("could not start the local engine worker: {error}"))?;
            Ok(Self {
                handle,
                worker: Some(worker),
            })
        }

        pub(super) fn snapshot(&self) -> Result<Snapshot, String> {
            let mut wire = WireSnapshot {
                abi_version: ABI,
                struct_size: std::mem::size_of::<WireSnapshot>() as u32,
                generation: 0,
                state: 0,
                bound_port: 0,
                engine_result: 0,
                error: [0; ERROR_CAPACITY],
            };
            let result = unsafe {
                journal_mobile_qwen_get_snapshot_v1(self.handle.pointer.as_ptr(), &mut wire)
            };
            if result != 0 || wire.generation != self.handle.generation {
                return Err("could not read the owned local engine state".into());
            }
            let state = match wire.state {
                0 => State::Created,
                1 => State::Loading,
                2 => State::Ready,
                3 => State::Stopping,
                4 => State::Stopped,
                5 => State::Failed,
                _ => return Err("the local engine returned an unknown state".into()),
            };
            let port = if state == State::Ready {
                Some(
                    std::num::NonZeroU16::new(
                        u16::try_from(wire.bound_port).map_err(|_| "invalid local engine port")?,
                    )
                    .ok_or("local engine readiness has no bound port")?,
                )
            } else {
                None
            };
            Ok(Snapshot {
                generation: wire.generation,
                state,
                port,
                engine_result: wire.engine_result,
                error: error_text(&wire.error),
            })
        }

        pub(super) fn request_stop(&self) {
            // Native Stop is thread-safe, idempotent and latched during load.
            unsafe {
                journal_mobile_qwen_request_stop_v1(self.handle.pointer.as_ptr());
            }
        }

        pub(super) fn is_finished(&self) -> bool {
            self.worker.as_ref().is_none_or(JoinHandle::is_finished)
        }

        pub(super) fn stop_and_join(&mut self) -> Result<Snapshot, String> {
            self.request_stop();
            if let Some(worker) = self.worker.take() {
                worker
                    .join()
                    .map_err(|_| "the local engine worker panicked")?;
            }
            self.snapshot()
        }
    }

    impl Drop for Engine {
        fn drop(&mut self) {
            self.request_stop();
            if let Some(worker) = self.worker.take() {
                let _ = worker.join();
            }
        }
    }
}

#[cfg(all(test, feature = "full-tests"))]
mod tests {
    use super::*;
    use sha2::{Digest, Sha256};

    #[test]
    fn package_record_refuses_different_image_bytes_and_library_identity() {
        let directory = tempfile::tempdir().unwrap();
        let image = directory.path().join("image");
        let manifest = directory.path().join("manifest.json");
        let bytes = b"independent native-image fixture";
        std::fs::write(&image, bytes).unwrap();
        let digest: String = Sha256::digest(bytes)
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect();
        let identity = RuntimeIdentity {
            abi: 1,
            source_revision: "1".repeat(40),
            runtime_identity: format!("sha256:{}", "2".repeat(64)),
        };
        std::fs::write(
            &manifest,
            serde_json::to_vec(&serde_json::json!({
                "schema": "solstone.mobile-qwen-package.v1",
                "runtime_identity": identity.runtime_identity,
                "image_sha256": digest,
            }))
            .unwrap(),
        )
        .unwrap();
        assert!(verify_package(&manifest, &image, &identity).is_ok());
        let other = RuntimeIdentity {
            runtime_identity: "different image".into(),
            ..identity.clone()
        };
        assert!(verify_package(&manifest, &image, &other).is_err());
        let mut changed = *bytes;
        changed[0] ^= 1;
        std::fs::write(&image, changed).unwrap();
        assert!(verify_package(&manifest, &image, &identity).is_err());
    }

    #[cfg(journal_mobile_qwen_native)]
    #[test]
    fn native_worker_stop_and_failed_loads_release_the_only_handle() {
        let directory = tempfile::tempdir().unwrap();
        let identity = runtime_identity().unwrap();
        let model = directory.path().join("invalid.gguf");
        let projector = directory.path().join("invalid-projector.gguf");
        std::fs::write(&model, b"invalid model fixture").unwrap();
        std::fs::write(&projector, b"invalid projector fixture").unwrap();
        for generation in 1..=4 {
            let mut engine = Engine::start(Config {
                identity: identity.clone(),
                generation,
                model_generation: "missing-model-fixture".into(),
                model_path: model.clone(),
                projector_path: projector.clone(),
                bearer_token: "a".repeat(64),
            })
            .unwrap();
            if generation % 2 == 0 {
                let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
                while !engine.is_finished() {
                    assert!(
                        std::time::Instant::now() < deadline,
                        "invalid model load must finish"
                    );
                    std::thread::sleep(std::time::Duration::from_millis(5));
                }
            } else {
                engine.request_stop();
            }
            let snapshot = engine.stop_and_join().unwrap();
            assert_eq!(snapshot.generation, generation);
            assert!(matches!(snapshot.state, State::Stopped | State::Failed));
            assert!(snapshot.port.is_none());
            assert!(engine.is_finished());
            // Next iteration must create a new sole native handle. Retaining
            // the previous native generation would cause create to return BUSY.
            drop(engine);
        }
    }
}
