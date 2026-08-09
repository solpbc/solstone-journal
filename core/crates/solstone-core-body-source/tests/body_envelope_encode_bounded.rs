// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use solstone_core_body_source::{
    BodyDay, BodyDigest, BodyEnvelope, BodyMonth, BodyRawRetention, BodySourceFamily,
    BodySourceHash, BundleId, EnvelopeErrorCode, EnvelopeErrorField, EnvelopeLedger, EnvelopeShard,
    encode_body_envelope,
};

const MAX_ENVELOPE_BYTES: usize = 1_048_576;
const MAX_PEAK_BYTES: u64 = 2_200_000;
const MAX_SMALL_PEAK_BYTES: u64 = 65_536;

#[test]
fn terminal_framing_accepts_a_canonical_object_one_byte_below_the_cap() {
    let envelope = boundary_envelope(MAX_ENVELOPE_BYTES - 1);
    let encoded = encode_body_envelope(&envelope).expect("boundary envelope encodes");
    assert_eq!(canonical_len(&envelope), MAX_ENVELOPE_BYTES - 1);
    assert_eq!(encoded.len(), MAX_ENVELOPE_BYTES);
    assert_eq!(encoded.last(), Some(&b'\n'));
}

#[test]
fn terminal_framing_refuses_a_canonical_object_at_the_cap() {
    let envelope = boundary_envelope(MAX_ENVELOPE_BYTES);
    assert_eq!(canonical_len(&envelope), MAX_ENVELOPE_BYTES);
    assert_overflow(encode_body_envelope(&envelope));
}

#[test]
fn hundred_thousand_distinct_days_refuse_with_bounded_encoder_peak() {
    let envelope = oura_envelope(100_000, None);
    assert_overflow(encode_body_envelope(&envelope));
    let info = allocation_counter::measure(|| {
        drop(encode_body_envelope(&envelope));
    });
    assert!(
        info.bytes_max <= MAX_PEAK_BYTES,
        "peak was {} bytes",
        info.bytes_max
    );
}

#[test]
fn small_envelope_does_not_preallocate_the_full_limit() {
    let envelope = oura_envelope(1, None);
    let info = allocation_counter::measure(|| {
        assert!(encode_body_envelope(&envelope).is_ok());
    });
    assert!(
        info.bytes_max <= MAX_SMALL_PEAK_BYTES,
        "small-envelope peak was {} bytes",
        info.bytes_max
    );
}

fn boundary_envelope(target: usize) -> BodyEnvelope {
    let count = 65_000;
    let baseline = oura_envelope(count, None);
    let baseline_len = canonical_len(&baseline);
    assert!(baseline_len <= target);
    let mut remaining = target - baseline_len;
    let padding = baseline
        .shards()
        .iter()
        .map(|shard| {
            let digits = shard.bytes().to_string().len();
            let added = remaining.min(20 - digits);
            remaining -= added;
            if added == 0 {
                shard.bytes()
            } else {
                10_u64.pow((digits + added - 1) as u32)
            }
        })
        .collect::<Vec<_>>();
    assert_eq!(
        remaining, 0,
        "boundary padding capacity must cover the target"
    );
    let envelope = oura_envelope(count, Some(&padding));
    assert_eq!(canonical_len(&envelope), target);
    envelope
}

fn oura_envelope(count: usize, shard_bytes: Option<&[u64]>) -> BodyEnvelope {
    let bundle = BundleId::from_bytes(b"body-00000000000000000000000000").unwrap();
    let digest = digest('a');
    let (days, month_rows) = generated_days(count);
    let shards = month_rows
        .into_iter()
        .enumerate()
        .map(|(index, (month, rows))| {
            let bytes = shard_bytes.map_or(rows, |values| values[index]);
            EnvelopeShard::new(&bundle, index as u64, month, bytes, rows, digest.clone()).unwrap()
        })
        .collect();
    let rows = u64::try_from(count).unwrap();
    BodyEnvelope::new(
        bundle.clone(),
        BodySourceFamily::OuraApi,
        BodySourceHash::from_bytes_for_family(
            b"bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
            &BodySourceFamily::OuraApi,
        )
        .unwrap(),
        BodyRawRetention::RetainParsed,
        rows,
        days,
        shards,
        EnvelopeLedger::new(&bundle, rows, rows, digest).unwrap(),
        None,
    )
    .unwrap()
}

fn generated_days(count: usize) -> (Vec<BodyDay>, Vec<(BodyMonth, u64)>) {
    let mut year = 2000_u16;
    let mut month = 1_u8;
    let mut day = 1_u8;
    let mut days = Vec::with_capacity(count);
    for _ in 0..count {
        days.push(BodyDay::from_bytes(format!("{year:04}{month:02}{day:02}").as_bytes()).unwrap());
        advance_day(&mut year, &mut month, &mut day);
    }
    let mut month_rows = Vec::new();
    for day in &days {
        match month_rows.last_mut() {
            Some((month, rows)) if *month == day.month() => *rows += 1,
            _ => month_rows.push((day.month(), 1)),
        }
    }
    (days, month_rows)
}

fn canonical_len(envelope: &BodyEnvelope) -> usize {
    let fields = [
        field_len("bundle_id", quoted_len(envelope.bundle_id().as_str())),
        field_len("days", days_len(envelope.days())),
        field_len("ledger", ledger_len(envelope)),
        field_len(
            "raw_retention",
            quoted_len(envelope.raw_retention().as_str()),
        ),
        field_len("row_count", digits_len(envelope.row_count())),
        field_len("schema", quoted_len(envelope.schema())),
        field_len("shards", shards_len(envelope)),
        field_len(
            "source_family",
            quoted_len(envelope.source_family().as_str()),
        ),
        field_len("source_hash", quoted_len(envelope.source_hash().as_str())),
        field_len("summary_plan", 4),
    ];
    2 + fields.into_iter().sum::<usize>() + fields.len() - 1
}

fn days_len(days: &[BodyDay]) -> usize {
    2 + days
        .iter()
        .map(|day| quoted_len(day.as_str()))
        .sum::<usize>()
        + days.len().saturating_sub(1)
}

fn ledger_len(envelope: &BodyEnvelope) -> usize {
    let ledger = envelope.ledger();
    object_len([
        field_len("bytes", digits_len(ledger.bytes())),
        field_len("events", digits_len(ledger.events())),
        field_len("path", quoted_len(ledger.path())),
        field_len("sha256", quoted_len(ledger.sha256().as_str())),
    ])
}

fn shards_len(envelope: &BodyEnvelope) -> usize {
    2 + envelope
        .shards()
        .iter()
        .map(|shard| {
            object_len([
                field_len("bytes", digits_len(shard.bytes())),
                field_len("path", quoted_len(shard.path())),
                field_len("rows", digits_len(shard.rows())),
                field_len("sha256", quoted_len(shard.sha256().as_str())),
            ])
        })
        .sum::<usize>()
        + envelope.shards().len().saturating_sub(1)
}

fn object_len<const N: usize>(fields: [usize; N]) -> usize {
    2 + fields.into_iter().sum::<usize>() + N.saturating_sub(1)
}

fn field_len(key: &str, value: usize) -> usize {
    quoted_len(key) + 1 + value
}

fn quoted_len(value: &str) -> usize {
    value.len() + 2
}

fn digits_len(value: u64) -> usize {
    value.to_string().len()
}

fn assert_overflow(result: Result<Vec<u8>, solstone_core_body_source::EnvelopeError>) {
    let error = result.expect_err("oversized envelope must refuse");
    assert_eq!(error.code(), EnvelopeErrorCode::InputTooLarge);
    assert_eq!(error.field(), EnvelopeErrorField::Envelope);
    assert_eq!(error.index(), None);
}

fn digest(character: char) -> BodyDigest {
    BodyDigest::from_bytes(format!("sha256:{}", character.to_string().repeat(64)).as_bytes())
        .unwrap()
}

fn advance_day(year: &mut u16, month: &mut u8, day: &mut u8) {
    if *day < days_in_month(*year, *month) {
        *day += 1;
    } else {
        *day = 1;
        if *month == 12 {
            *month = 1;
            *year += 1;
        } else {
            *month += 1;
        }
    }
}

fn days_in_month(year: u16, month: u8) -> u8 {
    match month {
        2 if year.is_multiple_of(4) && (!year.is_multiple_of(100) || year.is_multiple_of(400)) => {
            29
        }
        2 => 28,
        4 | 6 | 9 | 11 => 30,
        _ => 31,
    }
}
