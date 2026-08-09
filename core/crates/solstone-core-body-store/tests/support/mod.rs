// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

#![allow(dead_code)]

use std::path::{Path, PathBuf};

use serde_json::Value;
use sha2::{Digest, Sha256};
use solstone_core_body_source::{
    BodyDigest, BodyEnvelope, BodyLedgerEvent, BodyString, BodyValue, BundleId, Coordinate,
    PresentationRow, ValidatedBodyRowEvent, canonicalize, decode_body_envelope,
    decode_body_ledger_event, parse, project, validate_body_row_event,
};

pub struct Observation {
    envelope: BodyEnvelope,
    event: BodyLedgerEvent,
    row_frame: Vec<u8>,
}

impl Observation {
    pub fn validate(&self) -> ValidatedBodyRowEvent {
        validate_body_row_event(&self.envelope, &self.row_frame, &self.event)
            .expect("test observation validates")
    }
}

pub fn native_bundle_fixture_path() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../../core/fixtures/body_source_native_bundle_v1.json")
}

pub fn native_bundle_fixture() -> Value {
    serde_json::from_str(
        &std::fs::read_to_string(native_bundle_fixture_path()).expect("fixture should read"),
    )
    .expect("fixture should parse")
}

pub fn fixture_observation(name: &str) -> Observation {
    let case = fixture_case(name);
    let envelope = decode_body_envelope(
        case["expected_envelope_jsonl"]
            .as_str()
            .expect("fixture envelope")
            .as_bytes(),
    )
    .expect("fixture envelope decodes");
    let event = decode_body_ledger_event(
        case["expected_ledger_jsonl"]
            .as_str()
            .expect("fixture ledger")
            .as_bytes(),
        &envelope,
        1,
    )
    .expect("fixture event decodes");
    Observation {
        envelope,
        event,
        row_frame: case["expected_normalized_jsonl"]
            .as_str()
            .expect("fixture row")
            .as_bytes()
            .to_vec(),
    }
}

pub fn observation(
    case_name: &str,
    bundle: &str,
    dedupe_key: &str,
    value_hash: &str,
    configure: impl FnOnce(&mut BodyValue, &BodyEnvelope),
) -> Observation {
    let mut envelope = fixture_envelope(case_name);
    let replacement_bundle = BundleId::from_bytes(bundle.as_bytes()).expect("test bundle is valid");
    envelope = BodyEnvelope::new(
        replacement_bundle,
        envelope.source_family(),
        envelope.source_hash().clone(),
        envelope.raw_retention(),
        envelope.row_count(),
        envelope.days().to_vec(),
        envelope.shards().to_vec(),
        envelope.ledger().clone(),
        envelope.summary_plan().cloned(),
    )
    .expect("replacement bundle envelope is valid");

    let mut row = fixture_row(case_name);
    set_text(
        &mut row,
        "import_id",
        envelope.bundle_id().as_str().bytes().map(u32::from),
    );
    set_text(
        &mut row,
        "normalized_ref",
        format!(
            "imports/{}/{}#L1",
            envelope.bundle_id().as_str(),
            envelope.shards()[0].path()
        )
        .bytes()
        .map(u32::from),
    );
    set_text(
        &mut row,
        "raw_ref",
        format!("imports/{}/raw/source", envelope.bundle_id().as_str())
            .bytes()
            .map(u32::from),
    );
    set_text(&mut row, "dedupe_key", dedupe_key.bytes().map(u32::from));
    configure(&mut row, &envelope);

    let row_json = canonicalize(&row).expect("test row canonicalizes");
    let row_frame = format!("{row_json}\n").into_bytes();
    let value = parse(row_json.as_bytes()).expect("canonical row parses");
    let coordinate = Coordinate::new(
        envelope.bundle_id().as_str(),
        envelope.shards()[0].path(),
        1,
    );
    let presentation = PresentationRow::new(&value, &coordinate).expect("test row presents");
    let candidate = project(&presentation, coordinate).expect("test row projects");
    let event = BodyLedgerEvent::new(
        &envelope,
        1,
        0,
        1,
        sha256_body_digest(&row_frame),
        digest(value_hash),
        &candidate,
    )
    .expect("test event binds");

    Observation {
        envelope,
        event,
        row_frame,
    }
}

pub fn set_text(value: &mut BodyValue, field: &str, code_points: impl IntoIterator<Item = u32>) {
    let BodyValue::Object(object) = value else {
        panic!("test row is an object");
    };
    object.insert(
        body_string(field),
        BodyValue::String(
            BodyString::from_code_points(code_points.into_iter().collect())
                .expect("test code points are in range"),
        ),
    );
}

pub fn set_null(value: &mut BodyValue, field: &str) {
    let BodyValue::Object(object) = value else {
        panic!("test row is an object");
    };
    object.insert(body_string(field), BodyValue::Null);
}

pub fn text(value: &str) -> Vec<u32> {
    value.chars().map(u32::from).collect()
}

fn fixture_case(name: &str) -> Value {
    native_bundle_fixture()["cases"]
        .as_array()
        .expect("fixture cases")
        .iter()
        .find(|case| case["name"].as_str() == Some(name))
        .expect("fixture case exists")
        .clone()
}

fn fixture_envelope(name: &str) -> BodyEnvelope {
    let case = fixture_case(name);
    decode_body_envelope(
        case["expected_envelope_jsonl"]
            .as_str()
            .expect("fixture envelope")
            .as_bytes(),
    )
    .expect("fixture envelope decodes")
}

fn fixture_row(name: &str) -> BodyValue {
    let case = fixture_case(name);
    parse(
        case["expected_normalized_jsonl"]
            .as_str()
            .expect("fixture row")
            .as_bytes(),
    )
    .expect("fixture row parses")
}

fn body_string(value: &str) -> BodyString {
    BodyString::from_code_points(value.bytes().map(u32::from).collect())
        .expect("ASCII field name is valid")
}

fn digest(value: &str) -> BodyDigest {
    BodyDigest::from_bytes(value.as_bytes()).expect("test digest is valid")
}

fn sha256_body_digest(bytes: &[u8]) -> BodyDigest {
    digest(&format!("sha256:{:x}", Sha256::digest(bytes)))
}
