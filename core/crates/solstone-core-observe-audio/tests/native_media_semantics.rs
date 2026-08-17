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

    let image_only = temporary_path("image-only").with_extension("bmp");
    fs::write(&image_only, one_pixel_bmp()).expect("write image-only input");
    assert!(matches!(
        decode_f32_mono(&image_only),
        Err(AudioError::NoAudioStream { .. })
    ));
    fs::remove_file(image_only).expect("remove image-only input");

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

fn one_pixel_bmp() -> [u8; 58] {
    let mut bmp = [0_u8; 58];
    bmp[..2].copy_from_slice(b"BM");
    bmp[2..6].copy_from_slice(&58_u32.to_le_bytes());
    bmp[10..14].copy_from_slice(&54_u32.to_le_bytes());
    bmp[14..18].copy_from_slice(&40_u32.to_le_bytes());
    bmp[18..22].copy_from_slice(&1_i32.to_le_bytes());
    bmp[22..26].copy_from_slice(&1_i32.to_le_bytes());
    bmp[26..28].copy_from_slice(&1_u16.to_le_bytes());
    bmp[28..30].copy_from_slice(&24_u16.to_le_bytes());
    bmp[34..38].copy_from_slice(&4_u32.to_le_bytes());
    bmp
}
