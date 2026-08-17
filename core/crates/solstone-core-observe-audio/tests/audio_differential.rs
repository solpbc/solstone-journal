// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

#![allow(clippy::disallowed_methods, clippy::disallowed_types)]

mod common;

use common::{TempDir, generated_decode_corpus, python, read_f32le};
use solstone_core_observe_audio::decode_f32_mono;

const PYTHON_DECODE: &str = r#"
import sys
import numpy as np
from pathlib import Path
from solstone.observe.utils import load_audio
np.asarray(load_audio(Path(sys.argv[1])), dtype='<f4').tofile(sys.argv[2])
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
