// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

use solstone_core_observe_audio::{AudioError, audio_to_wav_bytes, decode_f32_mono};

fn temporary_path(name: &str) -> PathBuf {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    std::env::temp_dir().join(format!(
        "solstone-observe-audio-{name}-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ))
}

#[test]
fn decode_errors_never_use_an_empty_buffer_sentinel() {
    let empty = temporary_path("empty");
    fs::write(&empty, []).expect("write empty input");
    assert!(matches!(
        decode_f32_mono(&empty),
        Err(AudioError::EmptyInput { .. })
    ));
    fs::remove_file(empty).expect("remove empty input");

    let corrupt = temporary_path("corrupt");
    fs::write(&corrupt, b"not media").expect("write corrupt input");
    assert!(matches!(
        decode_f32_mono(&corrupt),
        Err(AudioError::CorruptInput { .. })
    ));
    fs::remove_file(corrupt).expect("remove corrupt input");

    let video_only = PathBuf::from(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../fixtures/grab_corpus/distinct.mkv"
    ));
    assert!(matches!(
        decode_f32_mono(&video_only),
        Err(AudioError::NoAudioStream { .. })
    ));

    let empty_wav = temporary_path("empty-wav").with_extension("wav");
    fs::write(
        &empty_wav,
        audio_to_wav_bytes(&[], 16_000).expect("WAV bytes"),
    )
    .expect("write empty WAV");
    assert!(matches!(
        decode_f32_mono(&empty_wav),
        Err(AudioError::NoDecodedAudio { .. })
    ));
    fs::remove_file(empty_wav).expect("remove empty WAV");
}
