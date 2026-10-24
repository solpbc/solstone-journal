// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Narrow owning adapter for the pinned Parakeet C API, used by mobile hosts.

use serde_json::Value;
use sha2::{Digest, Sha256};
use std::{fs::File, io::Read, path::Path};

use solstone_core_transcribe::EMBEDDED_AUDIO_MAX_SAMPLES;
pub use solstone_core_transcribe::{
    EmbeddedAsr, EmbeddedModelInfo, discover_audio_files, process_embedded,
};

#[cfg(feature = "native")]
mod ffi;

/// A locally loaded, integrity-checked model. The native context stays on the
/// caller's worker thread and is freed when this owner is dropped.
pub struct Parakeet {
    #[cfg(feature = "native")]
    context: ffi::Context,
}

fn verify_model(path: &Path) -> Result<(), String> {
    let pin = solstone_core_assets::catalog()
        .iter()
        .find(|a| a.unit == "parakeet-model")
        .ok_or("Parakeet model pin absent")?;
    let mut input = File::open(path).map_err(|e| format!("open Parakeet model: {e}"))?;
    if input.metadata().map_err(|e| e.to_string())?.len() != pin.size_bytes {
        return Err("Parakeet model size differs from pinned model".into());
    }
    let mut hash = Sha256::new();
    let mut buffer = [0u8; 64 * 1024];
    loop {
        let n = input
            .read(&mut buffer)
            .map_err(|e| format!("read Parakeet model: {e}"))?;
        if n == 0 {
            break;
        }
        hash.update(&buffer[..n]);
    }
    if format!("{:x}", hash.finalize()) != pin.sha256 {
        return Err("Parakeet model digest differs from pinned model".into());
    }
    Ok(())
}

impl Parakeet {
    /// Observe the outstanding native call without recording samples or output.
    /// `true` brackets only the C API invocation, after input/model admission;
    /// `false` is delivered before copying or decoding its returned JSON.
    pub fn transcribe_observed(
        &self,
        samples: &[f32],
        native_call: &dyn Fn(bool),
    ) -> Result<Value, String> {
        if samples.is_empty()
            || samples.len() > EMBEDDED_AUDIO_MAX_SAMPLES
            || samples.iter().any(|v| !v.is_finite())
        {
            return Err("Parakeet requires finite mono 16 kHz samples, at most 120 seconds".into());
        }
        #[cfg(feature = "native")]
        {
            normalize(self.context.transcribe(samples, native_call)?)
        }
        #[cfg(not(feature = "native"))]
        {
            let _ = native_call;
            Err("native Parakeet support was not compiled".into())
        }
    }

    /// Identity of the maintained recognizer, before allocating its context.
    pub fn pinned_model_info() -> EmbeddedModelInfo {
        EmbeddedModelInfo {
            backend: "parakeet-cpp".into(),
            model: "tdt-0.6b-v3-q8_0".into(),
            device: if cfg!(target_os = "ios") {
                "auto"
            } else {
                "cpu"
            }
            .into(),
            compute_type: "q8_0".into(),
        }
    }

    /// Verify and load the maintained model. Scratch is app-private and shared
    /// with the production VAD/speaker pipeline; ASR itself accepts PCM directly.
    pub fn new(model: &Path, scratch: &Path) -> Result<Self, String> {
        verify_model(model)?;
        std::fs::create_dir_all(scratch).map_err(|e| format!("create audio scratch: {e}"))?;
        #[cfg(feature = "native")]
        {
            Ok(Self {
                context: ffi::Context::load(model)?,
            })
        }
        #[cfg(not(feature = "native"))]
        {
            Err("native Parakeet support was not compiled".into())
        }
    }
}

impl EmbeddedAsr for Parakeet {
    fn transcribe(&self, samples: &[f32]) -> Result<Value, String> {
        self.transcribe_observed(samples, &|_| {})
    }

    fn model_info(&self) -> EmbeddedModelInfo {
        Self::pinned_model_info()
    }
}

#[cfg(any(feature = "native", test))]
fn observe_native_call<T>(observer: &dyn Fn(bool), call: impl FnOnce() -> T) -> T {
    struct Active<'a>(&'a dyn Fn(bool));
    impl Drop for Active<'_> {
        fn drop(&mut self) {
            (self.0)(false);
        }
    }
    observer(true);
    let _active = Active(observer);
    call()
}

#[cfg(any(feature = "native", test))]
fn normalize(mut batch: Value) -> Result<Value, String> {
    let clips = batch
        .as_array_mut()
        .ok_or("Parakeet response must be a clip array")?;
    if clips.len() != 1 {
        return Err("Parakeet response must have exactly one clip".into());
    }
    let mut clip = clips.remove(0);
    for word in clip
        .get_mut("words")
        .and_then(Value::as_array_mut)
        .ok_or("Parakeet response lacks words")?
    {
        let object = word
            .as_object_mut()
            .ok_or("Parakeet word must be an object")?;
        let text = object.remove("w").ok_or("Parakeet word lacks w")?;
        object.insert("word".into(), text);
    }
    Ok(clip)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn observation_is_active_only_during_the_call_and_clears_on_unwind() {
        use std::cell::{Cell, RefCell};
        let active = Cell::new(false);
        let events = RefCell::new(Vec::new());
        let observe = |value| {
            active.set(value);
            events.borrow_mut().push(value);
        };
        let result: Result<(), &str> = observe_native_call(&observe, || {
            assert!(active.get());
            Err("native failure")
        });
        assert_eq!(result, Err("native failure"));
        assert!(!active.get());
        assert_eq!(*events.borrow(), vec![true, false]);
        let panic = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            observe_native_call(&observe, || panic!("native adapter panic"));
        }));
        assert!(panic.is_err());
        assert!(!active.get());
        assert_eq!(*events.borrow(), vec![true, false, true, false]);
    }

    #[cfg(not(feature = "native"))]
    #[test]
    fn refused_input_or_uncompiled_native_never_claims_an_active_call() {
        let asr = Parakeet {};
        let observe = |_| panic!("no C API invocation was admitted");
        for samples in [&[][..], &[f32::NAN][..], &[0.0][..]] {
            assert!(asr.transcribe_observed(samples, &observe).is_err());
        }
    }
    #[test]
    fn native_word_contract_is_normalized_without_losing_timings() {
        let value = normalize(serde_json::json!([{"text":"Hello","words":[{"w":"Hello","start":0.1,"end":0.8,"conf":0.9}]}])).unwrap();
        assert_eq!(
            value["words"][0],
            serde_json::json!({"word":"Hello","start":0.1,"end":0.8,"conf":0.9})
        );
        assert!(normalize(serde_json::json!([])).is_err());
        assert!(normalize(serde_json::json!([{"text":"bad"}])).is_err());
    }
}
