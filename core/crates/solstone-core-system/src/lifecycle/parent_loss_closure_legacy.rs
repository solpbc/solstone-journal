// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Bounded semantic comparison of an immutable legacy ledger snapshot.
use super::*;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
enum Boundary {
    FirstOrEnd,
    AfterValue,
    Finished,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
struct Cursor {
    offset: usize,
    boundary: Boundary,
}

#[derive(Serialize, Deserialize)]
struct Segment {
    schema: u32,
    key: String,
    seq: usize,
    expected_span: Option<usize>,
    expected_digest: Option<String>,
    expected_row_start: usize,
    expected_rows: usize,
    before: Cursor,
    after: Cursor,
    actual_rows: usize,
    raw_digest: String,
    equal: bool,
}

#[derive(Serialize, Deserialize)]
struct ComparisonCommit {
    schema: u32,
    key: String,
    segments: Vec<String>,
}

fn syntax() -> ParentLossLedgerError {
    <serde_json::Error as serde::de::Error>::custom("invalid legacy ledger array boundary").into()
}

fn skip_space(bytes: &[u8], mut offset: usize) -> usize {
    while bytes
        .get(offset)
        .is_some_and(|b| matches!(b, b' ' | b'\n' | b'\r' | b'\t'))
    {
        offset += 1;
    }
    offset
}

impl Cursor {
    fn next(&mut self, bytes: &[u8]) -> Result<Option<serde_json::Value>, ParentLossLedgerError> {
        if self.boundary == Boundary::Finished {
            return Ok(None);
        }
        let mut offset = skip_space(bytes, self.offset);
        if bytes.get(offset) == Some(&b']') {
            offset = skip_space(bytes, offset + 1);
            if offset != bytes.len() {
                return Err(syntax());
            }
            self.offset = offset;
            self.boundary = Boundary::Finished;
            return Ok(None);
        }
        if self.boundary == Boundary::AfterValue {
            if bytes.get(offset) != Some(&b',') {
                return Err(syntax());
            }
            offset = skip_space(bytes, offset + 1);
            if bytes.get(offset) == Some(&b']') {
                return Err(syntax());
            }
        }
        let mut values =
            serde_json::Deserializer::from_slice(&bytes[offset..]).into_iter::<serde_json::Value>();
        let value = values.next().ok_or_else(syntax)??;
        self.offset = offset + values.byte_offset();
        self.boundary = Boundary::AfterValue;
        #[cfg(test)]
        LEGACY_DECODED_ROWS.with(|count| count.set(count.get() + 1));
        Ok(Some(value))
    }
}

fn payload_path(dir: &Path, seq: usize) -> std::path::PathBuf {
    dir.join(format!("legacy-comparison-{seq:06}.json"))
}

fn preserve_invalid<T>(dir: &Path, payload: &Path) -> Result<T, ParentLossLedgerError> {
    if payload.exists() {
        set_aside(payload)?;
    }
    let commit = dir.join("legacy-comparison-progress.json");
    if commit.exists() {
        set_aside(&commit)?;
    }
    Err(unverifiable_admission())
}

pub(super) fn compare(
    spools: &Spools,
    existing: &[u8],
    deadline: Instant,
) -> Result<Option<bool>, ParentLossLedgerError> {
    let first = skip_space(existing, 0);
    if existing.get(first) != Some(&b'[') {
        let _: serde_json::Value = serde_json::from_slice(existing)?;
        return Ok(Some(false));
    }
    let key = digest_bytes(&serde_json::to_vec(&(
        &spools.commit,
        digest_bytes(existing),
        existing.len(),
    ))?);
    let commit_path = spools.dir.join("legacy-comparison-progress.json");
    let retained = read_json_durable_validated::<ComparisonCommit>(
        ArtifactId::ParentLossClosureRecovery,
        &commit_path,
        |p| {
            if p.schema != 1 || p.key != key {
                return Err("invalid legacy comparison commit".to_owned());
            }
            Ok(())
        },
    )?;
    let mut commit = match retained {
        DurableRead::Present(p) => p,
        _ => ComparisonCommit {
            schema: 1,
            key: key.clone(),
            segments: Vec::new(),
        },
    };
    let indices: Vec<_> = spools
        .commit
        .spans
        .iter()
        .enumerate()
        .filter_map(|(index, span)| (span.sealed_rows > 0).then_some(index))
        .collect();
    let mut file = fs::File::open(spool_paths(&spools.dir)[1].clone())?;
    if file.metadata()?.len() != spools.commit.sealed_end {
        reset(&spools.dir)?;
        return Err(unverifiable_admission());
    }
    let mut cursor = Cursor {
        offset: first + 1,
        boundary: Boundary::FirstOrEnd,
    };
    let mut equal = true;
    let mut expected_row_start = 0;
    let mut seq = 0;
    loop {
        if Instant::now() >= deadline {
            return Ok(None);
        }
        let expected_span = indices.get(seq).copied();
        let expected = if let Some(index) = expected_span {
            spools.read_span(&mut file, true, &spools.commit.spans[index])?
        } else {
            Vec::new()
        };
        let expected_rows = expected_span.map_or(0, |i| spools.commit.spans[i].sealed_rows);
        let expected_digest = expected_span.map(|i| spools.commit.spans[i].sealed_digest.clone());
        for (ordinal, row) in expected
            .split(|b| *b == b'\n')
            .filter(|r| !r.is_empty())
            .enumerate()
        {
            let id = spools
                .sealed_ids
                .get(expected_row_start + ordinal)
                .ok_or_else(unverifiable_admission)?;
            let prefix = format!("{{\"launch_id\":{},", serde_json::to_string(id)?);
            if !row.starts_with(prefix.as_bytes()) {
                return Err(unverifiable_admission());
            }
        }
        let path = payload_path(&spools.dir, seq);
        let segment = if let Some(digest) = commit.segments.get(seq) {
            let bytes = match fs::read(&path) {
                Ok(bytes) => bytes,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                    return preserve_invalid(&spools.dir, &path);
                }
                Err(error) => return Err(error.into()),
            };
            if digest_bytes(&bytes) != *digest {
                return preserve_invalid(&spools.dir, &path);
            }
            let p: Segment = serde_json::from_slice(&bytes)?;
            let limit = if expected_span.is_some() {
                expected_rows
            } else {
                RECOVERY_BATCH_LIMIT
            };
            if p.schema != 1
                || p.key != key
                || p.seq != seq
                || p.expected_span != expected_span
                || p.expected_digest != expected_digest
                || p.expected_row_start != expected_row_start
                || p.expected_rows != expected_rows
                || p.before != cursor
                || p.after.offset < cursor.offset
                || p.after.offset > existing.len()
                || p.after.boundary == Boundary::FirstOrEnd
                || p.actual_rows > limit
                || (p.after.boundary == Boundary::AfterValue && p.actual_rows != limit)
                || (p.before.boundary == Boundary::Finished
                    && (p.after != p.before || p.actual_rows != 0))
                || (p.equal && p.actual_rows != expected_rows)
                || (p.after.boundary == Boundary::Finished && p.after.offset != existing.len())
                || digest_bytes(&existing[cursor.offset..p.after.offset]) != p.raw_digest
            {
                return preserve_invalid(&spools.dir, &path);
            }
            p
        } else {
            let before = cursor.clone();
            let mut local_equal = true;
            let mut actual_rows = 0;
            if expected_span.is_some() {
                for row in expected.split(|b| *b == b'\n').filter(|r| !r.is_empty()) {
                    let expected: serde_json::Value = serde_json::from_slice(row)?;
                    #[cfg(test)]
                    LEGACY_DECODED_ROWS.with(|count| count.set(count.get() + 1));
                    if let Some(actual) = cursor.next(existing)? {
                        actual_rows += 1;
                        local_equal &= actual == expected;
                    } else {
                        local_equal = false;
                    }
                }
            } else {
                for _ in 0..RECOVERY_BATCH_LIMIT {
                    if cursor.next(existing)?.is_none() {
                        break;
                    }
                    actual_rows += 1;
                    local_equal = false;
                }
            }
            let raw_digest = digest_bytes(&existing[before.offset..cursor.offset]);
            let p = Segment {
                schema: 1,
                key: key.clone(),
                seq,
                expected_span,
                expected_digest,
                expected_row_start,
                expected_rows,
                before,
                after: cursor.clone(),
                actual_rows,
                raw_digest,
                equal: local_equal,
            };
            // Only an independently committed exact payload may carry a prior
            // comparison result. A payload or cursor cannot authorize itself.
            if path.exists() {
                set_aside(&path)?;
            }
            let bytes = serde_json::to_vec(&p)?;
            atomic_replace(&path, &bytes, AtomicWriteOptions { mode: Some(0o600) })?;
            #[cfg(test)]
            if LEGACY_PAYLOAD_WRITE_FAULT.with(|fault| fault.get()) {
                return Ok(None);
            }
            // Compute the expectation from this writer's immutable value, never
            // from a retained mutable payload reread.
            commit.segments.push(digest_bytes(&bytes));
            write_json(&commit_path, &commit, json_options())?;
            #[cfg(test)]
            if LEGACY_STOP_AFTER_SEGMENT.with(|remaining| {
                let n = remaining.get();
                remaining.set(n.saturating_sub(1));
                n == 1
            }) {
                return Ok(None);
            }
            p
        };
        cursor = segment.after;
        equal &= segment.equal;
        expected_row_start += expected_rows;
        seq += 1;
        if seq >= indices.len() && cursor.boundary == Boundary::Finished {
            if seq != commit.segments.len() || expected_row_start != spools.sealed_ids.len() {
                return preserve_invalid(&spools.dir, &path);
            }
            return Ok(Some(equal));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    // The comparison makes durable writes that can take seconds on a busy disk,
    // so this only bounds a hang.
    fn deadline() -> Instant {
        Instant::now() + Duration::from_secs(60)
    }

    fn rows(values: &[serde_json::Value]) -> Vec<String> {
        values
            .iter()
            .enumerate()
            .map(|(i, value)| format!("{{\"launch_id\":\"row-{i}\",\"value\":{value}}}"))
            .collect()
    }

    fn array(values: &[serde_json::Value]) -> Vec<u8> {
        format!("[{}]", rows(values).join(",")).into_bytes()
    }

    fn spools(dir: &Path, values: &[serde_json::Value]) -> Spools {
        let mut body = Vec::new();
        let mut spans = Vec::new();
        for (i, row) in rows(values).into_iter().enumerate() {
            let start = body.len() as u64;
            let bytes = format!("{row}\n").into_bytes();
            body.extend_from_slice(&bytes);
            spans.push(Span {
                first_row: i,
                last_row: i + 1,
                admission_start: 0,
                admission_end: 0,
                admission_digest: digest_bytes(&[]),
                sealed_start: start,
                sealed_end: body.len() as u64,
                sealed_digest: digest_bytes(&bytes),
                sealed_rows: 1,
            });
        }
        fs::write(spool_paths(dir)[1].clone(), &body).unwrap();
        let ids = (0..values.len())
            .map(|i| format!("row-{i}"))
            .collect::<Vec<_>>();
        Spools {
            dir: dir.to_owned(),
            commit: MergeCommit {
                schema: 1,
                generation: 1,
                sealed_phase: ParentLossPhase::Sealed,
                input_digest: "independent-input".to_owned(),
                positions: vec![values.len()],
                rows: values.len(),
                admission_end: 0,
                sealed_end: body.len() as u64,
                spans,
            },
            all_ids: ids.clone(),
            sealed_ids: ids,
        }
    }

    fn finish_with_real_resumes(
        spools: &Spools,
        bytes: &[u8],
    ) -> Result<bool, ParentLossLedgerError> {
        loop {
            LEGACY_STOP_AFTER_SEGMENT.with(|n| n.set(1));
            let outcome = compare(spools, bytes, deadline());
            LEGACY_STOP_AFTER_SEGMENT.with(|n| n.set(0));
            if let Some(equal) = outcome? {
                return Ok(equal);
            }
        }
    }

    #[test]
    fn committed_comparison_resumes_without_decoding_its_completed_values() {
        let dir = TempDir::new().unwrap();
        let values = [serde_json::json!(1), serde_json::json!(2)];
        let spools = spools(dir.path(), &values);
        let bytes = array(&values);
        LEGACY_STOP_AFTER_SEGMENT.with(|n| n.set(1));
        assert_eq!(compare(&spools, &bytes, deadline()).unwrap(), None);
        LEGACY_DECODED_ROWS.with(|n| n.set(0));
        LEGACY_STOP_AFTER_SEGMENT.with(|n| n.set(1));
        assert_eq!(compare(&spools, &bytes, deadline()).unwrap(), None);
        LEGACY_STOP_AFTER_SEGMENT.with(|n| n.set(0));
        assert_eq!(LEGACY_DECODED_ROWS.with(|n| n.get()), 2);
        let commit: ComparisonCommit = serde_json::from_slice(
            &fs::read(dir.path().join("legacy-comparison-progress.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(commit.segments.len(), 2, "next actual segment must commit");
        assert!(finish_with_real_resumes(&spools, &bytes).unwrap());
    }

    #[test]
    fn persisted_comparison_preserves_the_whole_value_oracle() {
        let values = [
            "null",
            "{}",
            "{\"a\":null}",
            "{\"a\":1,\"a\":2}",
            "{\"a\":2}",
            "1",
            "1.0",
            "-0",
            "0",
            "-0.0",
            "0.0",
            "[1,null]",
            "{\"nested\":{\"x\":1,\"y\":null}}",
            "{\"nested\":{\"y\":null,\"x\":1}}",
            "{\"unknown\":true}",
        ];
        for expected in &values {
            for actual in &values {
                let expected = [
                    serde_json::from_str::<serde_json::Value>(expected).unwrap(),
                    serde_json::json!("equal-suffix"),
                ];
                let dir = TempDir::new().unwrap();
                let spools = spools(dir.path(), &expected);
                let bytes = format!("\t[\r{{\"launch_id\":\"row-0\",\"value\":{actual}}}\n,{{\"launch_id\":\"row-1\",\"value\":\"equal-suffix\"}} ]\r\n").into_bytes();
                let old_expected: serde_json::Value =
                    serde_json::from_slice(&array(&expected)).unwrap();
                let old_actual: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
                assert_eq!(
                    finish_with_real_resumes(&spools, &bytes).unwrap(),
                    old_actual == old_expected
                );
            }
        }
        for (expected, actual) in [
            (Vec::new(), Vec::new()),
            (
                Vec::new(),
                vec![serde_json::json!(1); RECOVERY_BATCH_LIMIT + 1],
            ),
            (vec![serde_json::json!(1), serde_json::json!(2)], Vec::new()),
            (
                vec![serde_json::json!(1)],
                vec![serde_json::json!(1), serde_json::json!(2)],
            ),
        ] {
            let dir = TempDir::new().unwrap();
            let spools = spools(dir.path(), &expected);
            let bytes = array(&actual);
            assert_eq!(
                finish_with_real_resumes(&spools, &bytes).unwrap(),
                expected == actual
            );
        }
    }

    #[test]
    fn primitive_extra_rows_and_non_array_roots_keep_value_parser_semantics() {
        for bytes in [
            "[]",
            "[null,1,-0,1.0,true,\"text\",[1],{\"x\":1,\"x\":2}]",
            "null",
            "{}",
            "true",
            "0",
            "\"text\"",
        ] {
            let dir = TempDir::new().unwrap();
            let spools = spools(dir.path(), &[]);
            let old: serde_json::Value = serde_json::from_str(bytes).unwrap();
            assert_eq!(
                finish_with_real_resumes(&spools, bytes.as_bytes()).unwrap(),
                old == serde_json::json!([])
            );
        }
        for bytes in [
            "[1,]",
            "[1 2]",
            "[1,,2]",
            "[1",
            "[] trailing",
            "[]\u{000b}",
            "[1e999]",
        ] {
            let dir = TempDir::new().unwrap();
            let spools = spools(dir.path(), &[]);
            assert!(
                finish_with_real_resumes(&spools, bytes.as_bytes()).is_err(),
                "{bytes}"
            );
        }
    }

    #[test]
    fn corrupt_payload_boundaries_counts_and_results_cannot_authorize_a_skipped_value() {
        for field in [
            "offset",
            "expected_rows",
            "actual_rows",
            "expected_row_start",
            "expected_span",
            "boundary",
            "equal",
        ] {
            let dir = TempDir::new().unwrap();
            let values = [serde_json::json!(1), serde_json::json!(2)];
            let spools = spools(dir.path(), &values);
            let bytes = array(&[serde_json::json!(9), serde_json::json!(2)]);
            LEGACY_STOP_AFTER_SEGMENT.with(|n| n.set(1));
            assert_eq!(compare(&spools, &bytes, deadline()).unwrap(), None);
            LEGACY_STOP_AFTER_SEGMENT.with(|n| n.set(0));
            let path = payload_path(dir.path(), 0);
            let mut payload: serde_json::Value =
                serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
            match field {
                "offset" => payload["after"]["offset"] = bytes.len().into(),
                "boundary" => payload["after"]["boundary"] = "Finished".into(),
                "equal" => payload["equal"] = true.into(),
                _ => payload[field] = 99.into(),
            }
            fs::write(&path, serde_json::to_vec(&payload).unwrap()).unwrap();
            assert!(compare(&spools, &bytes, deadline()).is_err(), "{field}");
            assert!(!finish_with_real_resumes(&spools, &bytes).unwrap());
        }
    }

    #[test]
    fn changed_snapshot_merge_or_spool_cannot_reuse_a_comparison() {
        for change in 0..3 {
            let dir = TempDir::new().unwrap();
            let values = [serde_json::json!(1), serde_json::json!(2)];
            let mut spools = spools(dir.path(), &values);
            let mut bytes = array(&values);
            LEGACY_STOP_AFTER_SEGMENT.with(|n| n.set(1));
            assert_eq!(compare(&spools, &bytes, deadline()).unwrap(), None);
            LEGACY_STOP_AFTER_SEGMENT.with(|n| n.set(0));
            match change {
                0 => bytes = array(&[serde_json::json!(9), serde_json::json!(2)]),
                1 => spools.commit.input_digest.push_str("-changed"),
                _ => {
                    let path = spool_paths(dir.path())[1].clone();
                    fs::write(path, b"changed expected UID").unwrap();
                    assert!(compare(&spools, &bytes, deadline()).is_err());
                    continue;
                }
            }
            LEGACY_DECODED_ROWS.with(|n| n.set(0));
            LEGACY_STOP_AFTER_SEGMENT.with(|n| n.set(1));
            assert_eq!(compare(&spools, &bytes, deadline()).unwrap(), None);
            LEGACY_STOP_AFTER_SEGMENT.with(|n| n.set(0));
            assert_eq!(LEGACY_DECODED_ROWS.with(|n| n.get()), 2);
            assert_eq!(
                finish_with_real_resumes(&spools, &bytes).unwrap(),
                change == 1
            );
        }
    }

    #[test]
    fn an_uncommitted_comparison_payload_is_replayed_before_it_can_authorize_equality() {
        let dir = TempDir::new().unwrap();
        let values = [serde_json::json!(1), serde_json::json!(2)];
        let spools = spools(dir.path(), &values);
        let bytes = array(&values);
        LEGACY_STOP_AFTER_SEGMENT.with(|n| n.set(1));
        assert_eq!(compare(&spools, &bytes, deadline()).unwrap(), None);
        LEGACY_STOP_AFTER_SEGMENT.with(|n| n.set(0));
        let path = dir.path().join("legacy-comparison-progress.json");
        let committed = fs::read(&path).unwrap();
        LEGACY_PAYLOAD_WRITE_FAULT.with(|n| n.set(true));
        assert_eq!(compare(&spools, &bytes, deadline()).unwrap(), None);
        LEGACY_PAYLOAD_WRITE_FAULT.with(|n| n.set(false));
        assert_eq!(fs::read(&path).unwrap(), committed);
        LEGACY_DECODED_ROWS.with(|n| n.set(0));
        assert!(finish_with_real_resumes(&spools, &bytes).unwrap());
        assert_eq!(LEGACY_DECODED_ROWS.with(|n| n.get()), 2);
    }

    #[test]
    fn comparison_hints_and_missing_reordered_or_extra_proofs_do_not_erase_a_mismatch() {
        for damage in 0..5 {
            let dir = TempDir::new().unwrap();
            let values = [serde_json::json!(1), serde_json::json!(2)];
            let spools = spools(dir.path(), &values);
            let bytes = array(&[serde_json::json!(9), serde_json::json!(2)]);
            for _ in 0..2 {
                LEGACY_STOP_AFTER_SEGMENT.with(|n| n.set(1));
                assert_eq!(compare(&spools, &bytes, deadline()).unwrap(), None);
            }
            LEGACY_STOP_AFTER_SEGMENT.with(|n| n.set(0));
            let path = dir.path().join("legacy-comparison-progress.json");
            let mut commit: serde_json::Value =
                serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
            match damage {
                0 => {
                    commit["cursor"] = bytes.len().into();
                    commit["equal"] = true.into();
                    commit["complete"] = true.into();
                }
                1 => {
                    commit["segments"].as_array_mut().unwrap().pop();
                }
                2 => commit["segments"].as_array_mut().unwrap().swap(0, 1),
                3 => {
                    let first = commit["segments"][0].clone();
                    commit["segments"].as_array_mut().unwrap().push(first);
                }
                _ => {
                    fs::remove_file(payload_path(dir.path(), 0)).unwrap();
                }
            }
            fs::write(&path, serde_json::to_vec(&commit).unwrap()).unwrap();
            let result = compare(&spools, &bytes, deadline());
            if damage >= 2 {
                assert!(result.is_err());
            } else {
                assert_eq!(result.unwrap(), Some(false));
            }
            assert!(!finish_with_real_resumes(&spools, &bytes).unwrap());
        }
    }
}
