// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

#![allow(clippy::disallowed_methods, clippy::disallowed_types)]

mod common;

use std::fs;

use common::{TempDir, generated_decode_corpus, python, read_f32le};
use solstone_core_observe_audio::{
    AudioReduction, SpeechSegment, VadResult, audio_to_wav_bytes, compute_loud_speech_windows,
    compute_nonspeech_rms, decode_f32_mono, get_nonspeech_segments, reduce_audio,
    write_f32le_exclusive,
};

const PYTHON_DECODE: &str = r#"
import sys
import numpy as np
from pathlib import Path
from solstone.observe.utils import load_audio
np.asarray(load_audio(Path(sys.argv[1])), dtype='<f4').tofile(sys.argv[2])
"#;

const PYTHON_STREAM_ZERO: &str = r#"
import av
import numpy as np
import sys
from pathlib import Path
path = Path(sys.argv[1])
with av.open(str(path)) as container:
    stream = list(container.streams.audio)[0]
    resampler = av.audio.resampler.AudioResampler(format='flt', layout='mono', rate=16000)
    chunks = []
    for frame in container.decode(stream):
        chunks.extend(out.to_ndarray() for out in resampler.resample(frame))
    chunks.extend(out.to_ndarray() for out in resampler.resample(None))
np.concatenate(chunks, axis=1).flatten().astype('<f4').tofile(sys.argv[2])
"#;

#[test]
fn decode_matches_python_across_generated_corpus() {
    let temp = TempDir::new("decode-differential");
    for (index, fixture) in generated_decode_corpus(&temp).into_iter().enumerate() {
        let expected_path = temp.path().join(format!("python-{index}.f32le"));
        python(PYTHON_DECODE, &[&fixture, &expected_path]);
        let expected = read_f32le(&expected_path);
        let actual = decode_f32_mono(&fixture).expect("Rust decode");
        assert_eq!(
            actual.len(),
            expected.len(),
            "sample count mismatch for {fixture:?}"
        );
        let max_abs_difference = actual
            .iter()
            .zip(&expected)
            .map(|(actual, expected)| (actual - expected).abs())
            .fold(0.0_f32, f32::max);
        assert!(
            max_abs_difference <= 1e-6,
            "decode differs by {max_abs_difference} for {fixture:?}"
        );
    }
}

#[test]
fn m4a_is_the_zero_padded_multi_stream_mean_not_stream_zero() {
    let temp = TempDir::new("m4a-mean");
    let fixture = common::fixture_m4a_multi_track();
    let mixed_path = temp.path().join("mixed.f32le");
    let stream_zero_path = temp.path().join("stream-zero.f32le");
    python(PYTHON_DECODE, &[&fixture, &mixed_path]);
    python(PYTHON_STREAM_ZERO, &[&fixture, &stream_zero_path]);
    let mixed = read_f32le(&mixed_path);
    let stream_zero = read_f32le(&stream_zero_path);
    let reference_difference = mixed
        .iter()
        .zip(&stream_zero)
        .map(|(mixed, stream_zero)| (mixed - stream_zero).abs())
        .fold(0.0_f32, f32::max);
    assert!(
        reference_difference > 0.25,
        "fixture must distinguish stream-zero decoding"
    );

    let actual = decode_f32_mono(&fixture).expect("Rust M4A decode");
    let max_abs_difference = actual
        .iter()
        .zip(&mixed)
        .map(|(actual, expected)| (actual - expected).abs())
        .fold(0.0_f32, f32::max);
    assert_eq!(actual.len(), mixed.len());
    assert!(
        max_abs_difference <= 1e-6,
        "Rust M4A decode differs by {max_abs_difference}"
    );
}

#[test]
fn wav_and_f32le_writers_are_byte_identical_to_python() {
    let temp = TempDir::new("wire-differential");
    let samples: [f32; 8] = [
        -1.25,
        -1.0,
        -1.5 / 32_768.0,
        -0.5 / 32_768.0,
        0.5 / 32_768.0,
        1.5 / 32_768.0,
        1.0,
        1.25,
    ];
    let samples_path = temp.path().join("samples.f32le");
    let bytes: Vec<u8> = samples
        .iter()
        .flat_map(|sample| sample.to_le_bytes())
        .collect();
    fs::write(&samples_path, bytes).expect("write input samples");
    let expected_wav = temp.path().join("python.wav");
    let expected_sidecar = temp.path().join("python.f32le");
    python(
        r#"
import numpy as np
import sys
from pathlib import Path
from solstone.observe.transcribe._audio_wire import audio_to_wav_bytes
from solstone.observe.transcribe.speakers_analyze_adapter import _write_f32le
samples = np.fromfile(sys.argv[1], dtype='<f4')
Path(sys.argv[2]).write_bytes(audio_to_wav_bytes(samples, 16000))
_write_f32le(Path(sys.argv[3]), samples)
"#,
        &[&samples_path, &expected_wav, &expected_sidecar],
    );
    assert_eq!(
        audio_to_wav_bytes(&samples, 16_000).expect("Rust WAV"),
        fs::read(expected_wav).expect("Python WAV")
    );
    let actual_sidecar = temp.path().join("rust.f32le");
    write_f32le_exclusive(&actual_sidecar, &samples).expect("Rust sidecar");
    assert_eq!(
        fs::read(actual_sidecar).expect("Rust sidecar bytes"),
        fs::read(expected_sidecar).expect("Python sidecar bytes")
    );
}

#[test]
fn reduction_and_timestamp_restoration_match_python_cases() {
    let cases = [
        (4.0, vec![]),
        (4.0, vec![(0.0, 1.0), (3.0, 4.0)]),
        (4.0, vec![(0.0, 1.0), (2.999, 4.0)]),
        (7.0, vec![(0.0, 1.0), (5.0, 6.0)]),
        (9.0, vec![(3.0, 4.0), (5.0, 6.0)]),
    ];
    let temp = TempDir::new("vad-differential");
    let oracle_path = temp.path().join("oracle.txt");
    python(
        r#"
import sys
from pathlib import Path
import numpy as np
from solstone.observe.vad import VadResult, reduce_audio
cases = [
    (4.0, []),
    (4.0, [(0.0, 1.0), (3.0, 4.0)]),
    (4.0, [(0.0, 1.0), (2.999, 4.0)]),
    (7.0, [(0.0, 1.0), (5.0, 6.0)]),
    (9.0, [(3.0, 4.0), (5.0, 6.0)]),
]
lines = []
for duration, speech in cases:
    audio = np.arange(int(duration * 16000), dtype=np.float32)
    reduced, reduction = reduce_audio(audio, VadResult(duration, 0.0, bool(speech), speech))
    if reduced is None:
        lines.append('0')
    else:
        fields = ['1', str(len(reduced)), str(len(reduction.segments)), repr(reduction.reduced_duration)]
        fields.extend(repr(value) for segment in reduction.segments for value in (
            segment.original_start, segment.original_end, segment.reduced_start, segment.reduced_end))
        lines.append(','.join(fields))
mapping = __import__('solstone.observe.vad', fromlist=['AudioReduction']).AudioReduction(
    segments=[
        __import__('solstone.observe.vad', fromlist=['SpeechSegment']).SpeechSegment(3.0, 4.0, 1.0, 2.0),
        __import__('solstone.observe.vad', fromlist=['SpeechSegment']).SpeechSegment(8.0, 9.0, 4.0, 5.0),
    ], original_duration=10.0, reduced_duration=6.0)
lines.append(','.join(repr(mapping.restore_timestamp(value)) for value in (0.5, 1.5, 3.0, 5.5)))
Path(sys.argv[1]).write_text('\n'.join(lines))
"#,
        &[&oracle_path],
    );
    let oracle = fs::read_to_string(oracle_path).expect("read Python reduction oracle");
    let mut lines = oracle.lines();
    for (duration_s, speech_segments) in cases {
        let expected = lines.next().expect("Python case result");
        let audio: Vec<f32> = (0..(duration_s * 16_000.0) as usize)
            .map(|sample| sample as f32)
            .collect();
        let vad = VadResult {
            duration_s,
            speech_duration_s: 0.0,
            has_speech: !speech_segments.is_empty(),
            speech_segments,
            noisy_rms: None,
            noisy_s: 0.0,
            loud_windows: 0,
            speech_loud_windows: 0,
        };
        match reduce_audio(&audio, &vad) {
            None => assert_eq!(expected, "0"),
            Some((reduced, mapping)) => {
                let fields: Vec<&str> = expected.split(',').collect();
                assert_eq!(fields[0], "1");
                assert_eq!(
                    reduced.len(),
                    fields[1].parse::<usize>().expect("Python reduced length")
                );
                assert_eq!(
                    mapping.segments.len(),
                    fields[2].parse::<usize>().expect("Python segment count")
                );
                assert!(
                    (mapping.reduced_duration_s
                        - fields[3].parse::<f64>().expect("Python duration"))
                    .abs()
                        <= 1e-12
                );
                for (actual, expected) in mapping.segments.iter().zip(fields[4..].chunks_exact(4)) {
                    assert_eq!(
                        actual.original_start_s,
                        expected[0].parse::<f64>().expect("Python original start")
                    );
                    assert_eq!(
                        actual.original_end_s,
                        expected[1].parse::<f64>().expect("Python original end")
                    );
                    assert_eq!(
                        actual.reduced_start_s,
                        expected[2].parse::<f64>().expect("Python reduced start")
                    );
                    assert_eq!(
                        actual.reduced_end_s,
                        expected[3].parse::<f64>().expect("Python reduced end")
                    );
                }
            }
        }
    }
    let expected_restore: Vec<f64> = lines
        .next()
        .expect("Python timestamp mapping")
        .split(',')
        .map(|value| value.parse().expect("Python timestamp"))
        .collect();
    let mapping = AudioReduction {
        segments: vec![
            SpeechSegment {
                original_start_s: 3.0,
                original_end_s: 4.0,
                reduced_start_s: 1.0,
                reduced_end_s: 2.0,
            },
            SpeechSegment {
                original_start_s: 8.0,
                original_end_s: 9.0,
                reduced_start_s: 4.0,
                reduced_end_s: 5.0,
            },
        ],
        original_duration_s: 10.0,
        reduced_duration_s: 6.0,
    };
    for (time, expected) in [0.5, 1.5, 3.0, 5.5].into_iter().zip(expected_restore) {
        assert_eq!(mapping.restore_timestamp(time), expected);
    }
}

#[test]
fn nonspeech_analysis_and_vad_predicates_match_python() {
    let audio: Vec<f32> = (0..12).map(|sample| sample as f32 / 20.0).collect();
    let speech = [(0.5, 1.0), (2.0, 2.5)];
    let temp = TempDir::new("nonspeech-differential");
    let audio_path = temp.path().join("audio.f32le");
    let oracle_path = temp.path().join("oracle.txt");
    fs::write(
        &audio_path,
        audio
            .iter()
            .flat_map(|sample| sample.to_le_bytes())
            .collect::<Vec<_>>(),
    )
    .expect("write oracle audio");
    python(
        r#"
import numpy as np
import sys
from pathlib import Path
from solstone.observe.vad import VadResult, compute_loud_speech_windows, compute_nonspeech_rms, get_nonspeech_segments
audio = np.fromfile(sys.argv[1], dtype='<f4')
speech = [(0.5, 1.0), (2.0, 2.5)]
segments = get_nonspeech_segments(speech, 3.0)
rms, duration = compute_nonspeech_rms(audio, speech, 4, 0.5)
loud, speech_loud = compute_loud_speech_windows(audio, speech, 4, 0.5, 0.01)
vad = VadResult(3.0, 1.0, True, speech, noisy_rms=0.01, loud_windows=3, speech_loud_windows=2)
Path(sys.argv[2]).write_text('|'.join(f'{start},{end}' for start, end in segments) + ';' + repr(float(rms)) + ';' + repr(duration) + ';' + f'{loud},{speech_loud}' + ';' + repr(vad.is_noisy(0.01)) + ';' + repr(vad.loud_speech_ratio))
"#,
        &[&audio_path, &oracle_path],
    );
    let oracle = fs::read_to_string(oracle_path).expect("read Python nonspeech oracle");
    let fields: Vec<&str> = oracle.split(';').collect();
    let expected_segments: Vec<(f64, f64)> = fields[0]
        .split('|')
        .map(|segment| {
            let bounds: Vec<f64> = segment
                .split(',')
                .map(|value| value.parse().expect("Python bound"))
                .collect();
            (bounds[0], bounds[1])
        })
        .collect();
    assert_eq!(get_nonspeech_segments(&speech, 3.0), expected_segments);
    let (rms, duration) = compute_nonspeech_rms(&audio, &speech, 4, 0.5);
    assert!((rms.expect("Rust RMS") - fields[1].parse::<f64>().expect("Python RMS")).abs() <= 1e-6);
    assert!((duration - fields[2].parse::<f64>().expect("Python duration")).abs() <= 1e-6);
    assert!(compute_nonspeech_rms(&audio, &speech, 4, 1.0).0.is_some());
    let expected_loud: Vec<usize> = fields[3]
        .split(',')
        .map(|value| value.parse().expect("Python loud count"))
        .collect();
    assert_eq!(
        compute_loud_speech_windows(&audio, &speech, 4, 0.5, 0.01),
        (expected_loud[0], expected_loud[1])
    );
    let vad = VadResult {
        duration_s: 3.0,
        speech_duration_s: 1.0,
        has_speech: true,
        speech_segments: speech.to_vec(),
        noisy_rms: Some(0.01),
        noisy_s: 0.0,
        loud_windows: 3,
        speech_loud_windows: 2,
    };
    assert_eq!(vad.is_noisy(0.01), fields[4] == "True");
    assert_eq!(
        vad.loud_speech_ratio().expect("Rust ratio"),
        fields[5].parse::<f64>().expect("Python ratio")
    );
}
