// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use crate::canonicalize::{
    CanonicalSink, CappedVecSink, write_array_end, write_array_start, write_integer,
    write_object_end, write_object_key, write_object_start, write_quoted_code_points,
    write_separator,
};
use crate::{BodyEnvelope, BodyInteger, EnvelopeError, EnvelopeErrorCode, EnvelopeErrorField};

const MAX_ENVELOPE_BYTES: usize = 1_048_576;

/// Encodes a checked body envelope as canonical JSONL bytes.
pub fn encode_body_envelope(envelope: &BodyEnvelope) -> Result<Vec<u8>, EnvelopeError> {
    let mut buffer = Vec::with_capacity(MAX_ENVELOPE_BYTES);
    {
        let mut sink = CappedVecSink::new(&mut buffer, MAX_ENVELOPE_BYTES);
        write_envelope(&mut sink, envelope).map_err(|_| overflow(envelope))?;
    }
    Ok(buffer)
}

fn overflow(envelope: &BodyEnvelope) -> EnvelopeError {
    EnvelopeError::new(
        Some(envelope.bundle_id().clone()),
        EnvelopeErrorCode::InputTooLarge,
        EnvelopeErrorField::Envelope,
        None,
    )
}

fn write_envelope<S: CanonicalSink>(sink: &mut S, envelope: &BodyEnvelope) -> Result<(), S::Error> {
    write_object_start(sink)?;
    write_key(sink, "bundle_id")?;
    write_ascii_string(sink, envelope.bundle_id().as_str())?;
    write_separator(sink)?;
    write_key(sink, "days")?;
    write_days(sink, envelope.days())?;
    write_separator(sink)?;
    write_key(sink, "ledger")?;
    write_ledger(sink, envelope)?;
    write_separator(sink)?;
    write_key(sink, "raw_retention")?;
    write_ascii_string(sink, envelope.raw_retention().as_str())?;
    write_separator(sink)?;
    write_key(sink, "row_count")?;
    write_u64(sink, envelope.row_count())?;
    write_separator(sink)?;
    write_key(sink, "schema")?;
    write_ascii_string(sink, envelope.schema())?;
    write_separator(sink)?;
    write_key(sink, "shards")?;
    write_shards(sink, envelope)?;
    write_separator(sink)?;
    write_key(sink, "source_family")?;
    write_ascii_string(sink, envelope.source_family().as_str())?;
    write_separator(sink)?;
    write_key(sink, "source_hash")?;
    write_ascii_string(sink, envelope.source_hash().as_str())?;
    write_separator(sink)?;
    write_key(sink, "summary_plan")?;
    match envelope.summary_plan() {
        Some(plan) => {
            write_object_start(sink)?;
            write_key(sink, "days")?;
            write_days(sink, plan.days())?;
            write_separator(sink)?;
            write_key(sink, "schema")?;
            write_ascii_string(sink, plan.schema())?;
            write_object_end(sink)?;
        }
        None => sink.write_bytes(b"null")?,
    }
    write_object_end(sink)?;
    sink.write_bytes(b"\n")
}

fn write_days<S: CanonicalSink>(sink: &mut S, days: &[crate::BodyDay]) -> Result<(), S::Error> {
    write_array_start(sink)?;
    for (index, day) in days.iter().enumerate() {
        if index > 0 {
            write_separator(sink)?;
        }
        write_ascii_string(sink, day.as_str())?;
    }
    write_array_end(sink)
}

fn write_ledger<S: CanonicalSink>(sink: &mut S, envelope: &BodyEnvelope) -> Result<(), S::Error> {
    let ledger = envelope.ledger();
    write_object_start(sink)?;
    write_key(sink, "bytes")?;
    write_u64(sink, ledger.bytes())?;
    write_separator(sink)?;
    write_key(sink, "events")?;
    write_u64(sink, ledger.events())?;
    write_separator(sink)?;
    write_key(sink, "path")?;
    write_ascii_string(sink, ledger.path())?;
    write_separator(sink)?;
    write_key(sink, "sha256")?;
    write_ascii_string(sink, ledger.sha256().as_str())?;
    write_object_end(sink)
}

fn write_shards<S: CanonicalSink>(sink: &mut S, envelope: &BodyEnvelope) -> Result<(), S::Error> {
    write_array_start(sink)?;
    for (index, shard) in envelope.shards().iter().enumerate() {
        if index > 0 {
            write_separator(sink)?;
        }
        write_object_start(sink)?;
        write_key(sink, "bytes")?;
        write_u64(sink, shard.bytes())?;
        write_separator(sink)?;
        write_key(sink, "path")?;
        write_ascii_string(sink, shard.path())?;
        write_separator(sink)?;
        write_key(sink, "rows")?;
        write_u64(sink, shard.rows())?;
        write_separator(sink)?;
        write_key(sink, "sha256")?;
        write_ascii_string(sink, shard.sha256().as_str())?;
        write_object_end(sink)?;
    }
    write_array_end(sink)
}

fn write_key<S: CanonicalSink>(sink: &mut S, key: &str) -> Result<(), S::Error> {
    write_object_key(sink, key.bytes().map(u32::from))
}

fn write_ascii_string<S: CanonicalSink>(sink: &mut S, value: &str) -> Result<(), S::Error> {
    write_quoted_code_points(sink, value.bytes().map(u32::from))
}

fn write_u64<S: CanonicalSink>(sink: &mut S, value: u64) -> Result<(), S::Error> {
    let integer = BodyInteger::from_u64(value);
    write_integer(sink, integer.is_negative(), integer.digits())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        BodyDay, BodyDigest, BodyRawRetention, BodySourceFamily, BodySourceHash, BundleId,
        EnvelopeLedger, EnvelopeShard,
    };

    struct CountingSink {
        limit: usize,
        bytes: usize,
        days: usize,
    }

    impl CanonicalSink for CountingSink {
        type Error = ();

        fn write_bytes(&mut self, bytes: &[u8]) -> Result<(), Self::Error> {
            if bytes.len() > self.limit.saturating_sub(self.bytes) {
                return Err(());
            }
            if bytes.len() == 8 && bytes.iter().all(u8::is_ascii_digit) {
                self.days += 1;
            }
            self.bytes += bytes.len();
            Ok(())
        }
    }

    #[test]
    fn checked_writer_stops_inside_a_large_days_array_when_its_sink_fills() {
        let envelope = large_oura_envelope(100_000);
        let mut sink = CountingSink {
            limit: 1_024,
            bytes: 0,
            days: 0,
        };

        assert_eq!(write_envelope(&mut sink, &envelope), Err(()));
        assert!(sink.bytes <= sink.limit);
        assert!(sink.days < 100, "writer must stop far before all days");
    }

    fn large_oura_envelope(count: usize) -> BodyEnvelope {
        let bundle = BundleId::from_bytes(b"body-00000000000000000000000000").unwrap();
        let digest = BodyDigest::from_bytes(
            b"sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
        )
        .unwrap();
        let hash = BodySourceHash::from_bytes_for_family(
            b"bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
            &BodySourceFamily::OuraApi,
        )
        .unwrap();
        let mut year = 2000_u16;
        let mut month = 1_u8;
        let mut day = 1_u8;
        let mut days = Vec::with_capacity(count);
        for _ in 0..count {
            days.push(
                BodyDay::from_bytes(format!("{year:04}{month:02}{day:02}").as_bytes()).unwrap(),
            );
            advance_day(&mut year, &mut month, &mut day);
        }
        let mut shards = Vec::new();
        let mut index = 0_usize;
        while index < days.len() {
            let month_value = days[index].month();
            let mut rows = 1_u64;
            index += 1;
            while index < days.len() && days[index].month() == month_value {
                rows += 1;
                index += 1;
            }
            shards.push(
                EnvelopeShard::new(
                    &bundle,
                    shards.len() as u64,
                    month_value,
                    rows,
                    rows,
                    digest.clone(),
                )
                .unwrap(),
            );
        }
        let rows = u64::try_from(count).unwrap();
        BodyEnvelope::new(
            bundle.clone(),
            BodySourceFamily::OuraApi,
            hash,
            BodyRawRetention::RetainParsed,
            rows,
            days,
            shards,
            EnvelopeLedger::new(&bundle, rows, rows, digest).unwrap(),
            None,
        )
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
            2 if year.is_multiple_of(4)
                && (!year.is_multiple_of(100) || year.is_multiple_of(400)) =>
            {
                29
            }
            2 => 28,
            4 | 6 | 9 | 11 => 30,
            _ => 31,
        }
    }
}
