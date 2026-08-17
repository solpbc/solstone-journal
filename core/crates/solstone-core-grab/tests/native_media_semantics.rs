// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::fs;
use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};
use solstone_core_grab::test_hooks::{RgbFrame, decode_frames};
use tempfile::tempdir;

const DISTINCT_MKV_SHA256: &str =
    "f4e8d1aa0cee50288f8b97808dced883ad3c9af01c779f4b9a066f908f328caa";
const NULL_PTS_H264_SHA256: &str =
    "94d9948f2789b8b5543c27fd5c2a836a2b3df30541143e7f5d537ed39d923792";
const DISTINCT_ID3_RGB24_SHA256: &str =
    "8174dfacc8f1c7fddffb2f6fb93070e2a6c41064e52477370ea79695067ec2f4";
const DISTINCT_ID1_RGB24_SHA256: &str =
    "0b33dbcdd6754a86355a3230801ca90de867fd57e700029038e33e9cfd9aa341";

fn corpus_path(file: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures/grab_corpus")
        .join(file)
}

fn sha256_hex(bytes: &[u8]) -> String {
    hex_encode(&Sha256::digest(bytes))
}

fn hex_encode(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn rgb24_sha256(frame: &RgbFrame) -> String {
    sha256_hex(&frame.pixels)
}

#[test]
fn fixture_bytes_match_the_authored_input_hashes() {
    let distinct = fs::read(corpus_path("distinct.mkv")).expect("read distinct.mkv");
    let null_pts = fs::read(corpus_path("null-pts.h264")).expect("read null-pts.h264");
    assert_eq!(sha256_hex(&distinct), DISTINCT_MKV_SHA256);
    assert_eq!(sha256_hex(&null_pts), NULL_PTS_H264_SHA256);
}

#[test]
fn requested_ids_use_pts_based_linear_identity_and_preserve_order() {
    let distinct = fs::read(corpus_path("distinct.mkv")).expect("read distinct.mkv");
    assert_eq!(sha256_hex(&distinct), DISTINCT_MKV_SHA256);

    let frames = decode_frames(&corpus_path("distinct.mkv"), &[3, 1, 7]).unwrap();
    assert_eq!(frames.len(), 3);
    assert_eq!(
        rgb24_sha256(frames[0].as_ref().expect("id 3 present")),
        DISTINCT_ID3_RGB24_SHA256
    );
    assert_eq!(
        rgb24_sha256(frames[1].as_ref().expect("id 1 present")),
        DISTINCT_ID1_RGB24_SHA256
    );
    assert!(frames[2].is_none(), "id 7 is past the three-frame video");
}

#[test]
fn pts_null_and_broken_media_have_the_expected_failure_surface() {
    let null_pts = fs::read(corpus_path("null-pts.h264")).expect("read null-pts.h264");
    assert_eq!(sha256_hex(&null_pts), NULL_PTS_H264_SHA256);

    assert_eq!(
        decode_frames(&corpus_path("null-pts.h264"), &[1, 2]).unwrap(),
        vec![None, None]
    );

    let temp = tempdir().unwrap();
    let invalid = temp.path().join("invalid.bin");
    fs::write(&invalid, b"not media").unwrap();
    assert!(decode_frames(&invalid, &[1]).is_err());

    let truncated = temp.path().join("truncated.h264");
    fs::write(&truncated, &null_pts[..null_pts.len() / 2]).unwrap();
    let result = decode_frames(&truncated, &[3]);
    assert!(
        result
            .as_ref()
            .map(|frames| frames.first().is_none_or(Option::is_none))
            .unwrap_or(true),
        "truncated annex-B must error or return a missing slot, never a partial image"
    );
}
