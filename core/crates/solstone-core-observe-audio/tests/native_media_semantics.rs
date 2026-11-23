// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

use solstone_core_observe_audio::{AudioError, UnitDecoder, audio_to_wav_bytes, decode_f32_mono};

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

    // 28-byte header-only M4A stub (ftyp only, no moov or mdat payload)
    // classifies as EmptyInput on both whole-file and streaming decode paths.
    const WATCH_STUB: &[u8] = &[
        0x00, 0x00, 0x00, 0x1c, b'f', b't', b'y', b'p', b'M', b'4', b'A', b' ', 0x00, 0x00, 0x00,
        0x00, b'M', b'4', b'A', b' ', b'm', b'p', b'4', b'2', b'i', b's', b'o', b'm',
    ];
    let header_stub = temporary_path("header-stub").with_extension("m4a");
    fs::write(&header_stub, WATCH_STUB).expect("write header-only m4a stub");
    assert!(matches!(
        decode_f32_mono(&header_stub),
        Err(AudioError::EmptyInput { .. })
    ));
    assert!(matches!(
        UnitDecoder::open(&header_stub),
        Err(AudioError::EmptyInput { .. })
    ));
    fs::remove_file(header_stub).expect("remove header-only m4a stub");

    // Truncated M4A carrying media bytes inside mdat must remain CorruptInput.
    let mut truncated = WATCH_STUB.to_vec();
    truncated.extend_from_slice(&[0, 0, 0, 0, b'm', b'd', b'a', b't', 0xde, 0xad, 0xbe, 0xef]);
    let truncated_m4a = temporary_path("truncated-mdat").with_extension("m4a");
    fs::write(&truncated_m4a, &truncated).expect("write truncated m4a");
    assert!(matches!(
        decode_f32_mono(&truncated_m4a),
        Err(AudioError::CorruptInput { .. })
    ));
    assert!(matches!(
        UnitDecoder::open(&truncated_m4a),
        Err(AudioError::CorruptInput { .. })
    ));
    fs::remove_file(truncated_m4a).expect("remove truncated m4a");

    // Arbitrary non-FLAC bytes with a .flac suffix must remain CorruptInput
    // even when FFmpeg's flac demuxer opens them with 0 channels / 0 Hz.
    let garbage_flac = temporary_path("garbage-flac").with_extension("flac");
    fs::write(&garbage_flac, b"invalid audio container").expect("write garbage flac");
    assert!(matches!(
        decode_f32_mono(&garbage_flac),
        Err(AudioError::CorruptInput { .. })
    ));
    let mut unit_decoder = match UnitDecoder::open(&garbage_flac) {
        Err(AudioError::CorruptInput { .. }) => None,
        Ok(decoder) => Some(decoder),
        Err(other) => panic!("unexpected error opening garbage flac: {other:?}"),
    };
    if let Some(decoder) = unit_decoder.as_mut() {
        assert!(matches!(
            decoder.pull(1024),
            Err(AudioError::CorruptInput { .. })
        ));
    }
    fs::remove_file(garbage_flac).expect("remove garbage flac");
}
