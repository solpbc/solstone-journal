// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::fmt;
use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use chrono::{Local, TimeZone};
use serde_json::{Map, Value, json};
use solstone_core_journal_io::cortex_use::{
    CortexNamespaceAuthority, CortexRecoveryDisposition, CortexUseCandidateRead,
    CortexUseDestinationCheck, CortexUseFatal, CortexUseFileIdentity, CortexUseRefusal,
    CortexUseRefusalCounts, admit_active_use, build_recovery_catalog, census_cortex_namespace,
    check_cortex_use_destination, complete_active_use, create_or_admit_cortex_namespace,
    parse_cortex_lifecycle_name, read_cortex_use_request, recover_active_use,
    talent_directory_name,
};
use solstone_core_journal_io::journal_root::{JournalEntryKind, JournalRoot};

// The recovery census walks every entry under `talents/`, cumulatively, to find
// orphaned in-progress uses. A long-running journal accumulates a large body of
// ordinary completed talent output in that same tree, so this bound has to clear
// real historical corpus size, not just the in-flight working set. Measured on a
// journal in continuous use for over a year: ~600k entries total, with several
// single talent directories alone past 65k. The prior 64Ki bound made every
// startup census fail. This is a generous multiple of that measurement, not an
// unbounded value: it still catches a genuinely pathological (e.g. runaway or
// corrupted) namespace rather than let recovery spin unbounded.
const MAXIMUM_RECOVERY_ENTRIES: usize = 4 * 1024 * 1024;

pub struct CortexStore {
    journal: PathBuf,
    talents: PathBuf,
    authority: CortexNamespaceAuthority,
}

impl fmt::Debug for CortexStore {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("CortexStore")
            .field("journal", &self.journal)
            .field("talents", &self.talents)
            .finish()
    }
}

impl CortexStore {
    pub fn new(journal: PathBuf) -> io::Result<Self> {
        let root = JournalRoot::open(&journal).map_err(io::Error::other)?;
        let authority = create_or_admit_cortex_namespace(root).map_err(io::Error::other)?;
        let talents = journal.join("talents");
        Ok(Self {
            journal,
            talents,
            authority,
        })
    }

    pub(crate) fn active_path(&self, name: &str, use_id: &str) -> PathBuf {
        self.talents
            .join(talent_directory_name(name))
            .join(format!("{use_id}_active.jsonl"))
    }

    pub fn claim(
        &self,
        name: &str,
        use_id: &str,
        request: &Map<String, Value>,
    ) -> io::Result<Option<(PathBuf, CortexUseFileIdentity)>> {
        let first_row = serde_json::to_vec(&Value::Object(request.clone()))?;
        match admit_active_use(&self.authority, name, use_id, &first_row) {
            Ok(admitted) => Ok(Some((self.active_path(name, use_id), admitted.identity()))),
            Err(error) if error.is_already_claimed() => Ok(None),
            Err(error) => Err(io::Error::other(error.to_string())),
        }
    }

    /// Append only to the active inode. This intentionally does not create a
    /// missing file: late monitor output must not resurrect a completed use.
    pub(crate) fn append_active(
        &self,
        path: &Path,
        event: &Map<String, Value>,
    ) -> io::Result<bool> {
        let line = serde_json::to_vec(&Value::Object(event.clone()))?;
        match OpenOptions::new().append(true).open(path) {
            Ok(mut file) => {
                file.write_all(&line)?;
                file.write_all(b"\n")?;
                Ok(true)
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(false),
            Err(error) => Err(error),
        }
    }

    pub(crate) fn recover(&self) -> Result<RecoveryReport, CortexUseFatal> {
        let (catalog, mut refusals) = self.inventory_recovery_catalog()?;
        let authority = admit_recovery_namespace(&self.journal)?;
        for talent in catalog.talents() {
            let Some(talent_name) = talent.name().to_str() else {
                refusals.record(CortexUseRefusal::TalentDirectoryRefused);
                continue;
            };
            let talent_directory = self.talents.join(talent_name);
            for candidate in talent.candidates() {
                match candidate.disposition() {
                    CortexRecoveryDisposition::Completed => {}
                    CortexRecoveryDisposition::Collision => match candidate.unresolved_reason() {
                        Some(reason) => refusals.record(reason),
                        None if parse_cortex_lifecycle_name(candidate.leaf())
                            .active()
                            .is_some() =>
                        {
                            recover_collision_without_reason(
                                &talent_directory,
                                candidate.leaf(),
                                &mut refusals,
                            );
                        }
                        None => {}
                    },
                    CortexRecoveryDisposition::Active => {
                        recover_active_candidate(
                            self,
                            &authority,
                            talent_name,
                            &talent_directory,
                            candidate.leaf(),
                            &mut refusals,
                        );
                    }
                }
            }
        }
        Ok(RecoveryReport { refusals })
    }

    pub(crate) fn complete(
        &self,
        use_id: &str,
        name: &str,
        identity: CortexUseFileIdentity,
        request: Option<&Map<String, Value>>,
    ) -> bool {
        if let Err(error) = complete_active_use(&self.authority, name, use_id, identity) {
            eprintln!("cortex: failed to complete talent file {use_id}: {error}");
            return false;
        }
        let completed = self
            .active_path(name, use_id)
            .with_file_name(format!("{use_id}.jsonl"));
        if let Some(request) = request {
            self.append_day_index(use_id, request, &completed);
        }
        true
    }

    pub(crate) fn has_finish(&self, active: &Path) -> bool {
        let Ok(text) = fs::read_to_string(active) else {
            return false;
        };
        text.lines().any(|line| {
            let Ok(event) = serde_json::from_str::<Value>(line) else {
                return false;
            };
            event.get("event") == Some(&Value::String("finish".into()))
                || (event.get("event") == Some(&Value::String("error".into()))
                    && event
                        .get("terminal")
                        .and_then(Value::as_bool)
                        .unwrap_or(true))
        })
    }

    pub(crate) fn append_day_index(
        &self,
        use_id: &str,
        request: &Map<String, Value>,
        completed: &Path,
    ) {
        let day = request
            .get("day")
            .and_then(Value::as_str)
            .filter(|day| is_day_key(day))
            .map(str::to_owned)
            .unwrap_or_else(|| day_from_use_id(use_id));
        if !is_day_key(&day) {
            return;
        }
        // No sender populates a request `ts`: the dispatcher stamps `ts` onto each
        // relayed event, not onto the request. Defaulting to 0 made every day-index
        // row render at the epoch and, because `runtime` treats a zero start as
        // unknown, left `runtime_seconds` null. The use id is epoch milliseconds --
        // `day_from_use_id` above already relies on that -- so derive from it.
        let start_ts = request
            .get("ts")
            .and_then(Value::as_i64)
            .filter(|timestamp| *timestamp != 0)
            .unwrap_or_else(|| ts_from_use_id(use_id));
        let mut thinking_count = 0_u64;
        let mut tool_count = 0_u64;
        let mut degraded = Value::Null;
        let mut error_message = Value::Null;
        let mut reason_code = Value::Null;
        let mut model = Value::Null;
        let mut runtime_seconds = Value::Null;
        let mut status = "completed";
        if let Ok(text) = fs::read_to_string(completed) {
            for line in text.lines() {
                let Ok(event) = serde_json::from_str::<Value>(line) else {
                    continue;
                };
                match event.get("event").and_then(Value::as_str) {
                    Some("thinking") => thinking_count += 1,
                    Some("tool_start") => tool_count += 1,
                    Some("start") => model = event.get("model").cloned().unwrap_or(Value::Null),
                    Some("finish") => {
                        status = "completed";
                        degraded = event.get("degraded").cloned().unwrap_or(Value::Null);
                        runtime_seconds =
                            runtime(start_ts, event.get("ts").and_then(Value::as_i64));
                    }
                    Some("error") => {
                        status = "error";
                        error_message = event
                            .get("error")
                            .and_then(Value::as_str)
                            .map(|text| Value::String(text.chars().take(200).collect()))
                            .unwrap_or(Value::Null);
                        reason_code = event.get("reason_code").cloned().unwrap_or(Value::Null);
                        runtime_seconds =
                            runtime(start_ts, event.get("ts").and_then(Value::as_i64));
                    }
                    _ => {}
                }
            }
        }
        let summary = json!({
            "use_id": use_id,
            "name": request.get("name").cloned().unwrap_or(Value::Null),
            "day": day,
            "facet": request.get("facet").cloned().unwrap_or(Value::Null),
            "ts": start_ts,
            "status": status,
            "runtime_seconds": runtime_seconds,
            "provider": request.get("provider").cloned().unwrap_or(Value::Null),
            "model": model,
            "schedule": request.get("schedule").cloned().unwrap_or(Value::Null),
            "thinking_count": thinking_count,
            "tool_count": tool_count,
            "error_message": if status == "error" { error_message } else { Value::Null },
            "reason_code": if status == "error" { reason_code } else { Value::Null },
            "degraded": degraded,
            "output_file": summarize_output_file(&self.journal.join(&day), &self.journal, request)
                .map(Value::String)
                .unwrap_or(Value::Null),
            "prompt": request.get("prompt").cloned().unwrap_or_else(|| Value::String(String::new())),
        });
        let path = self.talents.join(format!("{day}.jsonl"));
        if let Ok(mut file) = OpenOptions::new().create(true).append(true).open(path) {
            let _ = writeln!(file, "{summary}");
        }
    }

    #[cfg(test)]
    pub(crate) fn talents(&self) -> &Path {
        &self.talents
    }
    pub(crate) fn journal(&self) -> &Path {
        &self.journal
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub(crate) struct RecoveryReport {
    refusals: CortexUseRefusalCounts,
}

impl RecoveryReport {
    pub(crate) fn refusals(&self) -> &CortexUseRefusalCounts {
        &self.refusals
    }
}

#[cfg(test)]
thread_local! {
    static RECOVERY_DESTINATION_IO_FAULT: std::cell::Cell<bool> = const {
        std::cell::Cell::new(false)
    };
}

#[cfg(test)]
fn recovery_destination_check_checkpoint() -> Result<(), CortexUseRefusal> {
    RECOVERY_DESTINATION_IO_FAULT.with(|fault| {
        (!fault.replace(false))
            .then_some(())
            .ok_or(CortexUseRefusal::DestinationIo)
    })
}

#[cfg(not(test))]
fn recovery_destination_check_checkpoint() -> Result<(), CortexUseRefusal> {
    Ok(())
}

#[cfg(test)]
fn run_with_recovery_destination_io_fault<T>(operation: impl FnOnce() -> T) -> (T, bool) {
    RECOVERY_DESTINATION_IO_FAULT.with(|fault| {
        assert!(
            !fault.replace(true),
            "recovery destination fault is already active"
        );
    });
    let result = operation();
    let consumed = RECOVERY_DESTINATION_IO_FAULT.with(|fault| !fault.replace(false));
    (result, consumed)
}

#[cfg(test)]
thread_local! {
    static RECOVERY_REQUEST_MAP_IO_FAULT: std::cell::Cell<bool> = const {
        std::cell::Cell::new(false)
    };
}

#[cfg(test)]
fn recovery_request_map_check_checkpoint() -> Result<(), CortexUseRefusal> {
    RECOVERY_REQUEST_MAP_IO_FAULT.with(|fault| {
        (!fault.replace(false))
            .then_some(())
            .ok_or(CortexUseRefusal::CandidateIo)
    })
}

#[cfg(not(test))]
fn recovery_request_map_check_checkpoint() -> Result<(), CortexUseRefusal> {
    Ok(())
}

#[cfg(test)]
fn run_with_recovery_request_map_io_fault<T>(operation: impl FnOnce() -> T) -> (T, bool) {
    RECOVERY_REQUEST_MAP_IO_FAULT.with(|fault| {
        assert!(
            !fault.replace(true),
            "recovery request map fault is already active"
        );
    });
    let result = operation();
    let consumed = RECOVERY_REQUEST_MAP_IO_FAULT.with(|fault| !fault.replace(false));
    (result, consumed)
}

fn admit_recovery_namespace(journal: &Path) -> Result<CortexNamespaceAuthority, CortexUseFatal> {
    let root = JournalRoot::open(journal).map_err(|_| CortexUseFatal::RootInspectionFailed)?;
    create_or_admit_cortex_namespace(root).map_err(|_| CortexUseFatal::RootInspectionFailed)
}

impl CortexStore {
    fn inventory_recovery_catalog(
        &self,
    ) -> Result<
        (
            solstone_core_journal_io::cortex_use::CortexRecoveryCatalog,
            CortexUseRefusalCounts,
        ),
        CortexUseFatal,
    > {
        let authority = admit_recovery_namespace(&self.journal)?;
        let census = census_cortex_namespace(authority, MAXIMUM_RECOVERY_ENTRIES)
            .map_err(|_| CortexUseFatal::RootInspectionFailed)?;
        let mut refusals = CortexUseRefusalCounts::default();
        for _ in 0..census.refused_talent_count() {
            refusals.record(CortexUseRefusal::TalentDirectoryRefused);
        }
        for talent in census.talents() {
            for leaf in talent.entries() {
                if leaf.kind() != JournalEntryKind::RegularFile
                    && leaf.projections().active().is_some()
                {
                    refusals.record(CortexUseRefusal::CandidateNonregular);
                }
            }
        }
        let catalog =
            build_recovery_catalog(&census).map_err(|_| CortexUseFatal::RootInspectionFailed)?;
        Ok((catalog, refusals))
    }
}

fn read_active_request_map(path: &Path) -> Option<Map<String, Value>> {
    let text = fs::read_to_string(path).ok()?;
    let line = text.lines().next()?;
    match serde_json::from_str::<Value>(line) {
        Ok(Value::Object(map)) => Some(map),
        _ => None,
    }
}

fn recover_active_candidate(
    store: &CortexStore,
    authority: &CortexNamespaceAuthority,
    talent_name: &str,
    talent_directory: &Path,
    leaf: &std::ffi::OsStr,
    refusals: &mut CortexUseRefusalCounts,
) {
    let request = match read_cortex_use_request(talent_directory, leaf) {
        CortexUseCandidateRead::Accepted(request) => request,
        CortexUseCandidateRead::Refused(refusal) => {
            refusals.record(refusal);
            return;
        }
    };
    let destination = match recovery_destination_check_checkpoint() {
        Ok(()) => check_cortex_use_destination(talent_directory, &request),
        Err(refusal) => CortexUseDestinationCheck::Refused(refusal),
    };
    match destination {
        CortexUseDestinationCheck::Vacant => {}
        CortexUseDestinationCheck::Refused(refusal) => {
            refusals.record(refusal);
            return;
        }
    }
    let active_path = talent_directory.join(leaf);
    if recovery_request_map_check_checkpoint().is_err() {
        refusals.record(CortexUseRefusal::CandidateIo);
        return;
    }
    let Some(request_map) = read_active_request_map(&active_path) else {
        refusals.record(CortexUseRefusal::CandidateIo);
        return;
    };
    let mut error = synthesized_error(
        &request.use_id,
        "Recovered: Cortex restarted while talent was running",
    );
    error.insert(
        "reason_code".into(),
        Value::String("cortex_restart_recovered".into()),
    );
    match store.append_active(&active_path, &error) {
        Ok(true) => {}
        Ok(false) | Err(_) => {
            refusals.record(CortexUseRefusal::CandidateIo);
            return;
        }
    }
    match recover_active_use(authority, talent_name, &request.use_id) {
        Ok(()) => {
            let completed_path = active_path.with_file_name(format!("{}.jsonl", request.use_id));
            store.append_day_index(&request.use_id, &request_map, &completed_path);
        }
        Err(error) if error.is_already_claimed() => {
            refusals.record(CortexUseRefusal::DestinationOccupied);
        }
        Err(_) => {
            refusals.record(CortexUseRefusal::CandidateIo);
        }
    }
}

fn recover_collision_without_reason(
    talent_directory: &Path,
    leaf: &std::ffi::OsStr,
    refusals: &mut CortexUseRefusalCounts,
) {
    match read_cortex_use_request(talent_directory, leaf) {
        CortexUseCandidateRead::Accepted(request) => {
            match check_cortex_use_destination(talent_directory, &request) {
                CortexUseDestinationCheck::Refused(refusal) => refusals.record(refusal),
                CortexUseDestinationCheck::Vacant => {
                    refusals.record(CortexUseRefusal::InvalidRequest);
                }
            }
        }
        CortexUseCandidateRead::Refused(_) => {
            refusals.record(CortexUseRefusal::InvalidRequest);
        }
    }
}

pub(crate) fn synthesized_error(use_id: &str, error: impl Into<String>) -> Map<String, Value> {
    Map::from_iter([
        ("event".into(), Value::String("error".into())),
        ("ts".into(), Value::from(now_ms())),
        ("use_id".into(), Value::String(use_id.to_owned())),
        ("error".into(), Value::String(error.into())),
    ])
}

pub(crate) fn now_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

fn runtime(start: i64, end: Option<i64>) -> Value {
    match (start, end) {
        (start, Some(end)) if start != 0 && end != 0 => {
            Value::from(((end - start) as f64 / 1000.0 * 10.0).round() / 10.0)
        }
        _ => Value::Null,
    }
}

/// Epoch milliseconds encoded in a use id, or 0 when it is not a numeric id.
fn ts_from_use_id(use_id: &str) -> i64 {
    use_id.parse::<i64>().unwrap_or(0)
}

fn day_from_use_id(use_id: &str) -> String {
    use_id
        .parse::<i64>()
        .ok()
        .and_then(|milliseconds| Local.timestamp_millis_opt(milliseconds).single())
        .map(|time| time.format("%Y%m%d").to_string())
        .unwrap_or_default()
}

fn summarize_output_file(
    day_dir: &Path,
    journal_root: &Path,
    request: &Map<String, Value>,
) -> Option<String> {
    let output_path = request
        .get("output_path")
        .and_then(Value::as_str)
        .map(PathBuf::from)
        .or_else(|| derived_output_path(day_dir, request))?;
    if !output_path.exists() {
        return None;
    }
    output_path
        .strip_prefix(day_dir)
        .or_else(|_| output_path.strip_prefix(journal_root))
        .ok()
        .map(|path| path.display().to_string())
}

fn derived_output_path(day_dir: &Path, request: &Map<String, Value>) -> Option<PathBuf> {
    let output = request.get("output")?;
    let name = request.get("name").and_then(Value::as_str)?;
    let name = match name.split_once(':') {
        Some((app, name)) => format!("_{app}_{name}"),
        None => name.to_owned(),
    };
    let extension = if output == "json" { "json" } else { "md" };
    let file = format!("{name}.{extension}");
    let segment = request
        .get("segment")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty());
    let facet = request
        .get("facet")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty());
    let stream = request
        .get("env")
        .and_then(Value::as_object)
        .and_then(|env| env.get("SOL_STREAM"))
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty());
    let output_dir = match segment {
        Some(segment) => {
            let segment_dir = match stream {
                Some(stream) => day_dir.join(stream).join(segment),
                None => day_dir.join(segment),
            };
            segment_dir.join("talents")
        }
        None => day_dir.join("talents"),
    };
    Some(match facet {
        Some(facet) => output_dir.join(facet).join(file),
        None => output_dir.join(file),
    })
}

fn is_day_key(day: &str) -> bool {
    day.len() == 8 && day.bytes().all(|byte| byte.is_ascii_digit())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(unix)]
    use std::os::unix::fs::PermissionsExt;

    #[cfg(unix)]
    use solstone_core_journal_io::cortex_use::{
        CortexCensusPrimitive, CortexUseOperation, CortexUseReadPrimitive,
        format_cortex_use_summary, run_with_cortex_census_fault, run_with_cortex_use_read_fault,
    };
    use tempfile::tempdir;

    fn request() -> Map<String, Value> {
        serde_json::from_value(
            json!({"name":"conversation","day":"19700101","ts":1000,"prompt":"p"}),
        )
        .unwrap()
    }

    fn recovery_request(use_id: &str) -> Map<String, Value> {
        recovery_request_named("conversation", use_id)
    }

    fn recovery_request_named(name: &str, use_id: &str) -> Map<String, Value> {
        let mut request = request();
        request.insert("name".into(), Value::String(name.into()));
        request.insert("use_id".into(), Value::String(use_id.into()));
        request
    }

    fn claim_recovery_candidate(store: &CortexStore, name: &str, use_id: &str) -> PathBuf {
        let request = recovery_request_named(name, use_id);
        store.claim(name, use_id, &request).unwrap().unwrap().0
    }

    fn complete_with_request(store: &CortexStore, use_id: &str, request: &Map<String, Value>) {
        let name = request["name"].as_str().expect("name");
        let (_, identity) = store.claim(name, use_id, request).unwrap().unwrap();
        store.complete(use_id, name, identity, Some(request));
    }

    fn day_rows(store: &CortexStore, day: &str) -> Vec<Value> {
        fs::read_to_string(store.talents().join(format!("{day}.jsonl")))
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect()
    }

    /// No sender puts a `ts` on the request, so the day index used to stamp every
    /// row at the epoch and report a null runtime. The use id is epoch milliseconds.
    #[test]
    fn day_index_row_dates_from_the_use_id_when_the_request_has_no_ts() {
        let directory = tempdir().unwrap();
        let store = CortexStore::new(directory.path().to_path_buf()).unwrap();
        let mut request = request();
        request.remove("ts");
        let use_id = "1788248640729";
        let (active, identity) = store
            .claim("conversation", use_id, &request)
            .unwrap()
            .unwrap();
        store
            .append_active(
                &active,
                &serde_json::from_value(json!({"event":"start","ts":1788248640729_i64})).unwrap(),
            )
            .unwrap();
        store
            .append_active(
                &active,
                &serde_json::from_value(json!({"event":"finish","ts":1788248643229_i64})).unwrap(),
            )
            .unwrap();
        store.complete(use_id, "conversation", identity, Some(&request));

        let rows = day_rows(&store, "19700101");
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0]["ts"], json!(1788248640729_i64));
        assert_eq!(rows[0]["runtime_seconds"], json!(2.5));
    }

    #[test]
    fn append_without_create_drops_late_event_and_invalid_recovery_evidence_is_preserved() {
        let directory = tempdir().unwrap();
        let store = CortexStore::new(directory.path().to_path_buf()).unwrap();
        let (active, identity) = store
            .claim("conversation", "one", &request())
            .unwrap()
            .unwrap();
        store.complete("one", "conversation", identity, Some(&request()));
        assert!(
            !store
                .append_active(&active, &synthesized_error("one", "late"))
                .unwrap()
        );
        assert!(!active.exists());
        OpenOptions::new()
            .create(true)
            .append(true)
            .open(&active)
            .unwrap();
        let report = store.recover().unwrap();
        let completed = active.with_file_name("one.jsonl");
        assert_eq!(report.refusals().get(CortexUseRefusal::InvalidRequest), 1);
        assert!(!active.exists() || fs::read(&active).unwrap().is_empty());
        assert!(
            !fs::read_to_string(completed)
                .unwrap()
                .contains("Recovered: Cortex restarted while talent was running")
        );
    }

    #[test]
    fn day_index_terminal_status_is_last_wins() {
        let directory = tempdir().unwrap();
        let store = CortexStore::new(directory.path().to_path_buf()).unwrap();
        let (active, identity) = store
            .claim("conversation", "one", &request())
            .unwrap()
            .unwrap();
        store
            .append_active(
                &active,
                &serde_json::from_value(
                    json!({"event":"error","terminal":false,"error":"not terminal","ts":1100}),
                )
                .unwrap(),
            )
            .unwrap();
        store
            .append_active(
                &active,
                &serde_json::from_value(json!({"event":"finish","ts":1200})).unwrap(),
            )
            .unwrap();
        store.complete("one", "conversation", identity, Some(&request()));
        let index = fs::read_to_string(store.talents().join("19700101.jsonl")).unwrap();
        let row: Value = serde_json::from_str(index.trim()).unwrap();
        assert_eq!(row["status"], "completed");
        assert!(row["error_message"].is_null());
        assert!(row["reason_code"].is_null());
    }

    #[test]
    fn duplicate_claim_leaves_request_file_byte_identical() {
        let directory = tempdir().unwrap();
        let store = CortexStore::new(directory.path().to_path_buf()).unwrap();
        let request = recovery_request("one");
        let active = store
            .claim("conversation", "one", &request)
            .unwrap()
            .unwrap()
            .0;
        let before = fs::read(&active).unwrap();
        assert!(
            store
                .claim("conversation", "one", &request)
                .unwrap()
                .is_none()
        );
        assert_eq!(fs::read(active).unwrap(), before);
    }

    #[test]
    fn recovery_creates_day_index_without_touching_unrelated_talent_leaves() {
        let directory = tempdir().unwrap();
        let store = CortexStore::new(directory.path().to_path_buf()).unwrap();
        let request = recovery_request("one");
        let active = store
            .claim("conversation", "one", &request)
            .unwrap()
            .unwrap()
            .0;
        let unrelated = store.talents().join("chat.log");
        fs::write(&unrelated, "preserve this unrelated leaf").unwrap();
        let report = store.recover().unwrap();
        assert!(active.with_file_name("one.jsonl").exists());
        let rows = day_rows(&store, "19700101");
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0]["use_id"], "one");
        assert_eq!(
            fs::read_to_string(unrelated).unwrap(),
            "preserve this unrelated leaf"
        );
        assert!(
            solstone_core_journal_io::cortex_use::format_cortex_use_summary(
                solstone_core_journal_io::cortex_use::CortexUseOperation::Recovery,
                report.refusals(),
            )
            .is_none()
        );
    }

    #[test]
    fn completion_without_request_only_renames() {
        let directory = tempdir().unwrap();
        let store = CortexStore::new(directory.path().to_path_buf()).unwrap();
        let (active, identity) = store
            .claim("conversation", "one", &request())
            .unwrap()
            .unwrap();
        store.complete("one", "conversation", identity, None);
        assert!(active.with_file_name("one.jsonl").exists());
        assert!(!store.talents().join("19700101.jsonl").exists());
    }

    #[test]
    fn unreadable_completed_log_fabricates_completed_summary() {
        let directory = tempdir().unwrap();
        let store = CortexStore::new(directory.path().to_path_buf()).unwrap();
        let unreadable = directory.path().join("directory-not-log");
        fs::create_dir(&unreadable).unwrap();
        store.append_day_index("one", &request(), &unreadable);
        let row: Value = serde_json::from_str(
            &fs::read_to_string(store.talents().join("19700101.jsonl")).unwrap(),
        )
        .unwrap();
        assert_eq!(row["status"], "completed");
        assert!(row["runtime_seconds"].is_null());
        assert_eq!(row["thinking_count"], 0);
        assert!(row["model"].is_null());
    }

    #[test]
    fn day_index_has_exact_shape_and_declared_null_temporaries() {
        let directory = tempdir().unwrap();
        let store = CortexStore::new(directory.path().to_path_buf()).unwrap();
        let mut request = request();
        request.insert("model".into(), Value::String("request-model".into()));
        let (active, identity) = store
            .claim("conversation", "one", &request)
            .unwrap()
            .unwrap();
        for event in [
            json!({"event":"start","model":"priced-model","provider":"provider","ts":1100}),
            json!({"event":"finish","usage":{"input_tokens":2},"ts":2300}),
        ] {
            store
                .append_active(&active, &serde_json::from_value(event).unwrap())
                .unwrap();
        }
        store.complete("one", "conversation", identity, Some(&request));
        let row: Value = serde_json::from_str(
            &fs::read_to_string(store.talents().join("19700101.jsonl")).unwrap(),
        )
        .unwrap();
        let expected = [
            "use_id",
            "name",
            "day",
            "facet",
            "ts",
            "status",
            "runtime_seconds",
            "provider",
            "model",
            "schedule",
            "thinking_count",
            "tool_count",
            "error_message",
            "reason_code",
            "degraded",
            "output_file",
            "prompt",
        ];
        assert_eq!(
            row.as_object()
                .unwrap()
                .keys()
                .map(String::as_str)
                .collect::<Vec<_>>(),
            expected
        );
        assert!(row.get("cost").is_none());
        assert!(row["output_file"].is_null());
        assert_eq!(row["model"], "priced-model");
        assert_eq!(row["runtime_seconds"], 1.3);
    }

    #[test]
    fn day_index_summarizes_daily_plain_and_facet_output_files() {
        let directory = tempdir().unwrap();
        let store = CortexStore::new(directory.path().to_path_buf()).unwrap();
        let day_dir = directory.path().join("19700101");
        fs::create_dir_all(day_dir.join("talents/work")).unwrap();
        fs::write(day_dir.join("talents/plain.md"), "plain").unwrap();
        fs::write(day_dir.join("talents/work/_app_facet.json"), "{}").unwrap();
        let plain = serde_json::from_value(json!({
            "name":"plain", "day":"19700101", "output":"md"
        }))
        .unwrap();
        let facet = serde_json::from_value(json!({
            "name":"app:facet", "day":"19700101", "output":"json", "facet":"work"
        }))
        .unwrap();
        complete_with_request(&store, "one", &plain);
        complete_with_request(&store, "two", &facet);
        let rows = day_rows(&store, "19700101");
        assert_eq!(rows[0]["output_file"], "talents/plain.md");
        assert_eq!(rows[1]["output_file"], "talents/work/_app_facet.json");
    }

    #[test]
    fn day_index_summarizes_segment_and_segment_facet_output_files() {
        let directory = tempdir().unwrap();
        let store = CortexStore::new(directory.path().to_path_buf()).unwrap();
        let day_dir = directory.path().join("19700101");
        fs::create_dir_all(day_dir.join("segment/talents")).unwrap();
        fs::create_dir_all(day_dir.join("focus/other/talents/work")).unwrap();
        fs::write(day_dir.join("segment/talents/plain.md"), "plain").unwrap();
        fs::write(
            day_dir.join("focus/other/talents/work/_app_facet.json"),
            "{}",
        )
        .unwrap();
        let segment = serde_json::from_value(json!({
            "name":"plain", "day":"19700101", "output":"md", "segment":"segment"
        }))
        .unwrap();
        let facet = serde_json::from_value(json!({
            "name":"app:facet", "day":"19700101", "output":"json", "segment":"other",
            "facet":"work", "env":{"SOL_STREAM":"focus"}
        }))
        .unwrap();
        complete_with_request(&store, "one", &segment);
        complete_with_request(&store, "two", &facet);
        let rows = day_rows(&store, "19700101");
        assert_eq!(rows[0]["output_file"], "segment/talents/plain.md");
        assert_eq!(
            rows[1]["output_file"],
            "focus/other/talents/work/_app_facet.json"
        );
    }

    #[test]
    fn day_index_summarizes_output_path_override_in_day_and_journal_root() {
        let directory = tempdir().unwrap();
        let store = CortexStore::new(directory.path().to_path_buf()).unwrap();
        let day_dir = directory.path().join("19700101");
        let day_override = day_dir.join("custom.md");
        let root_override = directory.path().join("shared/output.json");
        fs::create_dir_all(root_override.parent().unwrap()).unwrap();
        fs::create_dir_all(&day_dir).unwrap();
        fs::write(&day_override, "day").unwrap();
        fs::write(&root_override, "root").unwrap();
        let in_day = serde_json::from_value(json!({
            "name":"one", "day":"19700101", "output_path":day_override
        }))
        .unwrap();
        let in_root = serde_json::from_value(json!({
            "name":"two", "day":"19700101", "output_path":root_override
        }))
        .unwrap();
        complete_with_request(&store, "one", &in_day);
        complete_with_request(&store, "two", &in_root);
        let rows = day_rows(&store, "19700101");
        assert_eq!(rows[0]["output_file"], "custom.md");
        assert_eq!(rows[1]["output_file"], "shared/output.json");
    }

    #[test]
    fn day_index_output_file_is_null_when_output_is_missing() {
        let directory = tempdir().unwrap();
        let store = CortexStore::new(directory.path().to_path_buf()).unwrap();
        let request = serde_json::from_value(json!({
            "name":"plain", "day":"19700101", "output":"md"
        }))
        .unwrap();
        complete_with_request(&store, "one", &request);
        assert!(day_rows(&store, "19700101")[0]["output_file"].is_null());
    }

    #[test]
    fn recovery_requires_a_valid_request_and_does_not_resurrect_late_output() {
        let directory = tempdir().unwrap();
        let store = CortexStore::new(directory.path().to_path_buf()).unwrap();
        let active = store
            .claim("conversation", "one", &request())
            .unwrap()
            .unwrap()
            .0;
        fs::remove_file(&active).unwrap();
        assert!(
            !store
                .append_active(&active, &synthesized_error("one", "late"))
                .unwrap()
        );
        let recovery_request = recovery_request("one");
        store
            .claim("conversation", "one", &recovery_request)
            .unwrap()
            .unwrap();
        store.recover().unwrap();
        assert!(active.with_file_name("one.jsonl").exists());
    }

    #[test]
    fn completion_rename_failure_is_silent_and_success_is_observable() {
        let directory = tempdir().unwrap();
        let store = CortexStore::new(directory.path().to_path_buf()).unwrap();
        let (claimed, identity) = store
            .claim("conversation", "missing", &request())
            .unwrap()
            .unwrap();
        fs::remove_file(&claimed).unwrap();
        store.complete("missing", "conversation", identity, Some(&request()));
        assert!(!store.talents().join("19700101.jsonl").exists());
        let (active, identity) = store
            .claim("conversation", "one", &request())
            .unwrap()
            .unwrap();
        store.complete("one", "conversation", identity, Some(&request()));
        assert!(active.with_file_name("one.jsonl").exists());
    }

    #[test]
    fn missing_finish_scan_and_no_create_append_do_not_resurrect_active_file() {
        let directory = tempdir().unwrap();
        let store = CortexStore::new(directory.path().to_path_buf()).unwrap();
        let active = store.active_path("conversation", "one");
        assert!(!store.has_finish(&active));
        assert!(
            !store
                .append_active(
                    &active,
                    &synthesized_error("one", "Talent exited with code 1 without finish event")
                )
                .unwrap()
        );
        assert!(!active.exists());
    }

    #[test]
    fn day_index_uses_start_provenance_and_request_timestamp() {
        let directory = tempdir().unwrap();
        let store = CortexStore::new(directory.path().to_path_buf()).unwrap();
        let mut request = request();
        request.insert("model".into(), Value::String("request-model".into()));
        let (active, identity) = store
            .claim("conversation", "one", &request)
            .unwrap()
            .unwrap();
        store
            .append_active(
                &active,
                &serde_json::from_value(json!({"event":"start","model":"start-model","provider":"start-provider","ts":9000})).unwrap(),
            )
            .unwrap();
        store
            .append_active(
                &active,
                &serde_json::from_value(json!({"event":"finish","ts":2356})).unwrap(),
            )
            .unwrap();
        let mut updated = request;
        updated.insert("provider".into(), Value::String("start-provider".into()));
        store.complete("one", "conversation", identity, Some(&updated));
        let row: Value = serde_json::from_str(
            &fs::read_to_string(store.talents().join("19700101.jsonl")).unwrap(),
        )
        .unwrap();
        assert_eq!(row["provider"], "start-provider");
        assert_eq!(row["model"], "start-model");
        assert_eq!(row["runtime_seconds"], 1.4);
    }

    #[test]
    fn recovery_rerun_overwrites_recovered_log_and_duplicates_day_index() {
        let directory = tempdir().unwrap();
        let store = CortexStore::new(directory.path().to_path_buf()).unwrap();
        let request = recovery_request("one");
        let first = store
            .claim("conversation", "one", &request)
            .unwrap()
            .unwrap()
            .0;
        let second_request = recovery_request("two");
        let _second = store
            .claim("conversation", "two", &second_request)
            .unwrap()
            .unwrap();
        store.recover().unwrap();
        let recovered = first.with_file_name("one.jsonl");
        let recovered_text = fs::read_to_string(&recovered).unwrap();
        fs::remove_file(&recovered).unwrap();
        let (rerun, identity) = store
            .claim("conversation", "one", &request)
            .unwrap()
            .unwrap();
        store
            .append_active(
                &rerun,
                &serde_json::from_value(json!({"event":"finish","ts":1200})).unwrap(),
            )
            .unwrap();
        store.complete("one", "conversation", identity, Some(&request));
        let rerun_text = fs::read_to_string(&recovered).unwrap();
        assert_ne!(rerun_text, recovered_text);
        let rows = fs::read_to_string(store.talents().join("19700101.jsonl")).unwrap();
        assert_eq!(
            rows.lines()
                .filter(|line| line.contains("\"use_id\":\"one\""))
                .count(),
            2
        );
        let two: Vec<_> = day_rows(&store, "19700101")
            .into_iter()
            .filter(|row| row["use_id"] == "two")
            .collect();
        assert_eq!(two.len(), 1);
        assert_eq!(two[0]["status"], "error");
        assert_eq!(
            two[0]["error_message"],
            "Recovered: Cortex restarted while talent was running"
        );
    }

    #[test]
    fn recovery_day_index_records_synthesized_error_status() {
        let directory = tempdir().unwrap();
        let store = CortexStore::new(directory.path().to_path_buf()).unwrap();
        claim_recovery_candidate(&store, "conversation", "one");
        store.recover().unwrap();
        let rows = day_rows(&store, "19700101");
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0]["use_id"], "one");
        assert_eq!(rows[0]["name"], "conversation");
        assert_eq!(rows[0]["day"], "19700101");
        assert_eq!(rows[0]["status"], "error");
        assert_eq!(
            rows[0]["error_message"],
            "Recovered: Cortex restarted while talent was running"
        );
    }

    #[test]
    fn recovery_refused_occupied_destination_does_not_write_day_index() {
        let directory = tempdir().unwrap();
        let store = CortexStore::new(directory.path().to_path_buf()).unwrap();
        let occupied = claim_recovery_candidate(&store, "conversation", "occupied");
        fs::write(
            occupied.with_file_name("occupied.jsonl"),
            b"existing completed record\n",
        )
        .unwrap();
        let report = store.recover().unwrap();
        assert_eq!(
            report.refusals().get(CortexUseRefusal::DestinationOccupied),
            1
        );
        assert!(!store.talents().join("19700101.jsonl").exists());
    }

    #[test]
    fn recovery_writes_day_index_rows_per_talent_and_day() {
        let directory = tempdir().unwrap();
        let store = CortexStore::new(directory.path().to_path_buf()).unwrap();
        claim_recovery_candidate(&store, "conversation", "one");
        claim_recovery_candidate(&store, "other", "two");
        let mut third = recovery_request_named("conversation", "three");
        third.insert("day".into(), Value::String("19700102".into()));
        store
            .claim("conversation", "three", &third)
            .unwrap()
            .unwrap();
        store.recover().unwrap();
        let day1 = day_rows(&store, "19700101");
        assert_eq!(day1.len(), 2);
        assert!(day1.iter().any(|row| row["use_id"] == "one"));
        assert!(day1.iter().any(|row| row["use_id"] == "two"));
        let day2 = day_rows(&store, "19700102");
        assert_eq!(day2.len(), 1);
        assert_eq!(day2[0]["use_id"], "three");
    }

    #[test]
    fn recovery_counts_injected_request_map_io_and_preserves_active_bytes() {
        let directory = tempdir().unwrap();
        let store = CortexStore::new(directory.path().to_path_buf()).unwrap();
        let active = claim_recovery_candidate(&store, "conversation", "one");
        let before = fs::read(&active).unwrap();
        let (report, injected) =
            run_with_recovery_request_map_io_fault(|| store.recover().unwrap());
        assert!(injected);
        assert_eq!(report.refusals().get(CortexUseRefusal::CandidateIo), 1);
        assert_eq!(fs::read(&active).unwrap(), before);
        assert!(!active.with_file_name("one.jsonl").exists());
        assert!(!store.talents().join("19700101.jsonl").exists());
    }

    #[test]
    fn recovery_terminalizes_only_admitted_candidates_and_preserves_refused_evidence() {
        let directory = tempdir().unwrap();
        let store = CortexStore::new(directory.path().to_path_buf()).unwrap();
        let genuine = store
            .claim("conversation", "genuine", &recovery_request("genuine"))
            .unwrap()
            .unwrap()
            .0;
        let malformed = store.active_path("conversation", "malformed");
        fs::write(&malformed, b"not-json\n").unwrap();
        let wrong_directory = store.active_path("other", "wrong-directory");
        fs::create_dir_all(wrong_directory.parent().unwrap()).unwrap();
        fs::write(
            &wrong_directory,
            b"{\"name\":\"conversation\",\"use_id\":\"wrong-directory\"}\n",
        )
        .unwrap();
        let occupied = store
            .claim("conversation", "occupied", &recovery_request("occupied"))
            .unwrap()
            .unwrap()
            .0;
        let occupied_completed = occupied.with_file_name("occupied.jsonl");
        fs::write(&occupied_completed, b"existing completed record\n").unwrap();
        let malformed_before = fs::read(&malformed).unwrap();
        let wrong_directory_before = fs::read(&wrong_directory).unwrap();
        let occupied_before = fs::read(&occupied).unwrap();

        let report = store.recover().unwrap();

        assert!(genuine.with_file_name("genuine.jsonl").exists());
        assert_eq!(fs::read(&malformed).unwrap(), malformed_before);
        assert_eq!(fs::read(&wrong_directory).unwrap(), wrong_directory_before);
        assert_eq!(fs::read(&occupied).unwrap(), occupied_before);
        assert_eq!(
            fs::read(&occupied_completed).unwrap(),
            b"existing completed record\n"
        );
        assert_eq!(report.refusals().get(CortexUseRefusal::InvalidRequest), 2);
        assert_eq!(
            report.refusals().get(CortexUseRefusal::DestinationOccupied),
            1
        );
    }

    #[test]
    fn recovery_counts_nonregular_active_leaf_and_recovers_another_candidate() {
        let directory = tempdir().unwrap();
        let store = CortexStore::new(directory.path().to_path_buf()).unwrap();
        let genuine = claim_recovery_candidate(&store, "genuine", "one");
        let nonregular = store.active_path("nonregular", "directory");
        fs::create_dir_all(nonregular.parent().unwrap()).unwrap();
        fs::create_dir(&nonregular).unwrap();

        let report = store.recover().unwrap();

        assert!(genuine.with_file_name("one.jsonl").exists());
        assert!(fs::metadata(&nonregular).unwrap().is_dir());
        assert_eq!(
            report.refusals().get(CortexUseRefusal::CandidateNonregular),
            1
        );
    }

    #[cfg(unix)]
    #[test]
    fn recovery_counts_injected_candidate_io_and_recovers_another_candidate() {
        let directory = tempdir().unwrap();
        let store = CortexStore::new(directory.path().to_path_buf()).unwrap();
        let first = claim_recovery_candidate(&store, "conversation", "first");
        let second = claim_recovery_candidate(&store, "conversation", "second");
        let first_before = fs::read(&first).unwrap();
        let second_before = fs::read(&second).unwrap();

        let (report, injected) =
            run_with_cortex_use_read_fault(CortexUseReadPrimitive::FirstRowRead, 1, || {
                store.recover().unwrap()
            });

        assert!(injected);
        assert_eq!(report.refusals().get(CortexUseRefusal::CandidateIo), 1);
        let recovered = usize::from(first.with_file_name("first.jsonl").exists())
            + usize::from(second.with_file_name("second.jsonl").exists());
        assert_eq!(recovered, 1);
        if first.exists() {
            assert_eq!(fs::read(&first).unwrap(), first_before);
        } else {
            assert_eq!(fs::read(&second).unwrap(), second_before);
        }
    }

    #[cfg(unix)]
    #[test]
    fn recovery_counts_injected_candidate_identity_change_and_recovers_another_candidate() {
        let directory = tempdir().unwrap();
        let store = CortexStore::new(directory.path().to_path_buf()).unwrap();
        let first = claim_recovery_candidate(&store, "conversation", "first");
        let second = claim_recovery_candidate(&store, "conversation", "second");
        let first_before = fs::read(&first).unwrap();
        let second_before = fs::read(&second).unwrap();

        let (report, injected) =
            run_with_cortex_use_read_fault(CortexUseReadPrimitive::FinalNameObserve, 1, || {
                store.recover().unwrap()
            });

        assert!(injected);
        assert_eq!(
            report
                .refusals()
                .get(CortexUseRefusal::CandidateIdentityChanged),
            1
        );
        let recovered = usize::from(first.with_file_name("first.jsonl").exists())
            + usize::from(second.with_file_name("second.jsonl").exists());
        assert_eq!(recovered, 1);
        if first.exists() {
            assert_eq!(fs::read(&first).unwrap(), first_before);
        } else {
            assert_eq!(fs::read(&second).unwrap(), second_before);
        }
    }

    #[cfg(unix)]
    #[test]
    fn recovery_counts_refused_talent_directory_and_recovers_another_candidate() {
        let directory = tempdir().unwrap();
        let store = CortexStore::new(directory.path().to_path_buf()).unwrap();
        let genuine = claim_recovery_candidate(&store, "genuine", "one");
        let refused = claim_recovery_candidate(&store, "refused", "blocked");
        let refused_before = fs::read(&refused).unwrap();
        let refused_directory = refused.parent().unwrap();
        let original_permissions = fs::metadata(refused_directory).unwrap().permissions();
        fs::set_permissions(refused_directory, std::fs::Permissions::from_mode(0o0)).unwrap();
        let report = store.recover();
        fs::set_permissions(refused_directory, original_permissions).unwrap();
        let report = report.unwrap();

        assert!(genuine.with_file_name("one.jsonl").exists());
        assert_eq!(fs::read(&refused).unwrap(), refused_before);
        assert_eq!(
            report
                .refusals()
                .get(CortexUseRefusal::TalentDirectoryRefused),
            1
        );
    }

    #[test]
    fn recovery_counts_destination_io_fault_and_recovers_another_candidate() {
        let directory = tempdir().unwrap();
        let store = CortexStore::new(directory.path().to_path_buf()).unwrap();
        let first = claim_recovery_candidate(&store, "conversation", "first");
        let second = claim_recovery_candidate(&store, "conversation", "second");
        let first_before = fs::read(&first).unwrap();
        let second_before = fs::read(&second).unwrap();

        let (report, injected) =
            run_with_recovery_destination_io_fault(|| store.recover().unwrap());

        assert!(injected);
        assert_eq!(report.refusals().get(CortexUseRefusal::DestinationIo), 1);
        let recovered = usize::from(first.with_file_name("first.jsonl").exists())
            + usize::from(second.with_file_name("second.jsonl").exists());
        assert_eq!(recovered, 1);
        if first.exists() {
            assert_eq!(fs::read(&first).unwrap(), first_before);
        } else {
            assert_eq!(fs::read(&second).unwrap(), second_before);
        }
    }

    #[cfg(unix)]
    #[test]
    fn recovery_aggregates_every_refusal_class_in_fixed_diagnostic_order() {
        let directory = tempdir().unwrap();
        let store = CortexStore::new(directory.path().to_path_buf()).unwrap();

        let invalid = store.active_path("invalid", "missing-newline");
        fs::create_dir_all(invalid.parent().unwrap()).unwrap();
        fs::write(
            &invalid,
            b"{\"name\":\"invalid\",\"use_id\":\"missing-newline\"}",
        )
        .unwrap();

        let nonregular = store.active_path("nonregular", "directory");
        fs::create_dir_all(nonregular.parent().unwrap()).unwrap();
        fs::create_dir(&nonregular).unwrap();

        let candidate_io = claim_recovery_candidate(&store, "candidate-io", "io");
        let candidate_io_permissions = fs::metadata(&candidate_io).unwrap().permissions();
        fs::set_permissions(&candidate_io, std::fs::Permissions::from_mode(0o0)).unwrap();

        let occupied = claim_recovery_candidate(&store, "occupied", "taken");
        fs::write(
            occupied.with_file_name("taken.jsonl"),
            b"already completed\n",
        )
        .unwrap();
        let destination = claim_recovery_candidate(&store, "destination", "io");
        let genuine_one = claim_recovery_candidate(&store, "genuine-one", "one");
        let genuine_two = claim_recovery_candidate(&store, "genuine-two", "two");

        let refused = claim_recovery_candidate(&store, "refused", "blocked");
        let refused_directory = refused.parent().unwrap();
        let refused_permissions = fs::metadata(refused_directory).unwrap().permissions();
        fs::set_permissions(refused_directory, std::fs::Permissions::from_mode(0o0)).unwrap();

        let ((report, destination_injected), identity_injected) =
            run_with_cortex_use_read_fault(CortexUseReadPrimitive::FinalNameObserve, 1, || {
                run_with_recovery_destination_io_fault(|| store.recover())
            });
        fs::set_permissions(&candidate_io, candidate_io_permissions).unwrap();
        fs::set_permissions(refused_directory, refused_permissions).unwrap();
        let report = report.unwrap();

        assert!(identity_injected);
        assert!(destination_injected);
        for refusal in [
            CortexUseRefusal::InvalidRequest,
            CortexUseRefusal::CandidateNonregular,
            CortexUseRefusal::CandidateIo,
            CortexUseRefusal::CandidateIdentityChanged,
            CortexUseRefusal::DestinationOccupied,
            CortexUseRefusal::DestinationIo,
            CortexUseRefusal::TalentDirectoryRefused,
        ] {
            assert_eq!(report.refusals().get(refusal), 1, "{refusal:?}");
        }
        assert!(
            genuine_one.with_file_name("one.jsonl").exists()
                || genuine_two.with_file_name("two.jsonl").exists()
        );
        assert_eq!(
            format_cortex_use_summary(CortexUseOperation::Recovery, report.refusals()),
            Some(
                "cortex_recovery invalid_request=1 candidate_nonregular=1 candidate_io=1 candidate_identity_changed=1 destination_occupied=1 destination_io=1 talent_directory_refused=1".into()
            )
        );
        assert!(invalid.exists());
        assert!(fs::metadata(&nonregular).unwrap().is_dir());
        assert!(candidate_io.exists());
        assert!(destination.exists() || destination.with_file_name("io.jsonl").exists());
    }

    #[cfg(unix)]
    #[test]
    fn recovery_refuses_symlinked_talents_root_and_does_not_traverse_talent_symlinks() {
        let directory = tempdir().unwrap();
        let store = CortexStore::new(directory.path().to_path_buf()).unwrap();
        let target = directory.path().join("target");
        fs::create_dir(&target).unwrap();
        let linked_talent = target.join("conversation");
        fs::create_dir(&linked_talent).unwrap();
        fs::write(
            linked_talent.join("one_active.jsonl"),
            b"{\"name\":\"conversation\",\"use_id\":\"one\"}\n",
        )
        .unwrap();
        std::os::unix::fs::symlink(&linked_talent, store.talents().join("linked")).unwrap();
        store.recover().unwrap();
        assert!(linked_talent.join("one_active.jsonl").exists());

        fs::remove_file(store.talents().join("linked")).unwrap();
        fs::remove_dir(store.talents()).unwrap();
        std::os::unix::fs::symlink(&target, store.talents()).unwrap();
        assert_eq!(store.recover(), Err(CortexUseFatal::RootInspectionFailed));
    }

    #[test]
    fn active_path_uses_the_shared_talent_directory_projection() {
        let directory = tempdir().unwrap();
        let store = CortexStore::new(directory.path().to_path_buf()).unwrap();
        assert!(
            store
                .active_path("app:name", "one")
                .ends_with("app--name/one_active.jsonl")
        );
        let escaped = store.active_path("foo/../../outside", "one");
        assert!(escaped.starts_with(store.talents()));
    }

    #[test]
    fn day_index_ignores_a_path_shaped_day() {
        let directory = tempdir().unwrap();
        let store = CortexStore::new(directory.path().to_path_buf()).unwrap();
        let mut request = request();
        request.insert("day".into(), Value::String("../outside".into()));
        store.append_day_index("1000", &request, Path::new("/missing"));
        assert!(!directory.path().join("outside.jsonl").exists());
        assert!(!store.talents().join("../outside.jsonl").exists());
    }

    #[test]
    fn claim_returns_none_when_the_completed_use_already_exists() {
        let directory = tempdir().unwrap();
        let store = CortexStore::new(directory.path().to_path_buf()).unwrap();
        complete_with_request(&store, "one", &recovery_request("one"));
        assert!(
            store
                .claim("conversation", "one", &recovery_request("one"))
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn complete_refuses_a_stale_identity_after_the_active_row_changes() {
        let directory = tempdir().unwrap();
        let store = CortexStore::new(directory.path().to_path_buf()).unwrap();
        let request = recovery_request("one");
        let (active, identity) = store
            .claim("conversation", "one", &request)
            .unwrap()
            .unwrap();
        let mut bytes = fs::read(&active).unwrap();
        bytes[0] ^= 1;
        fs::write(&active, &bytes).unwrap();
        store.complete("one", "conversation", identity, Some(&request));
        assert!(active.exists());
        assert!(!active.with_file_name("one.jsonl").exists());
        assert!(!store.talents().join("19700101.jsonl").exists());
    }

    #[test]
    fn recovery_classifies_completed_content_and_terminalizes_a_sibling_active() {
        let directory = tempdir().unwrap();
        let store = CortexStore::new(directory.path().to_path_buf()).unwrap();
        let genuine = claim_recovery_candidate(&store, "conversation", "beta");
        let completed_named_active = store.active_path("conversation", "gamma");
        fs::write(
            &completed_named_active,
            b"{\"name\":\"conversation\",\"use_id\":\"gamma_active\"}\n",
        )
        .unwrap();
        let before = fs::read(&completed_named_active).unwrap();
        store.recover().unwrap();
        assert!(genuine.with_file_name("beta.jsonl").exists());
        assert_eq!(fs::read(&completed_named_active).unwrap(), before);
    }

    #[test]
    fn recovery_completes_active_candidates_in_two_talent_directories() {
        let directory = tempdir().unwrap();
        let store = CortexStore::new(directory.path().to_path_buf()).unwrap();
        let first = claim_recovery_candidate(&store, "conversation", "one");
        let second = claim_recovery_candidate(&store, "other", "two");
        store.recover().unwrap();
        assert!(first.with_file_name("one.jsonl").exists());
        assert!(second.with_file_name("two.jsonl").exists());
    }

    #[cfg(unix)]
    #[test]
    fn recovery_aborts_on_identity_changed_at_talent_list() {
        let directory = tempdir().unwrap();
        let store = CortexStore::new(directory.path().to_path_buf()).unwrap();
        let _ = claim_recovery_candidate(&store, "conversation", "one");
        let (result, consumed) =
            run_with_cortex_census_fault(CortexCensusPrimitive::PostLeafEnumeration, 1, || {
                store.recover()
            });
        assert!(consumed);
        assert_eq!(result, Err(CortexUseFatal::RootInspectionFailed));
    }

    #[cfg(unix)]
    #[test]
    fn recovery_collision_on_completed_hypothesis_io_recovers_a_sibling() {
        let directory = tempdir().unwrap();
        let store = CortexStore::new(directory.path().to_path_buf()).unwrap();
        let genuine = claim_recovery_candidate(&store, "conversation", "aaa");
        let refused = store.active_path("conversation", "zzz");
        fs::write(
            &refused,
            b"{\"name\":\"conversation\",\"use_id\":\"nope\"}\n",
        )
        .unwrap();
        let before = fs::read(&refused).unwrap();
        let (report, consumed) =
            run_with_cortex_use_read_fault(CortexUseReadPrimitive::InitialNameObserve, 3, || {
                store.recover().unwrap()
            });
        assert!(consumed);
        assert_eq!(report.refusals().get(CortexUseRefusal::CandidateIo), 1);
        assert!(genuine.with_file_name("aaa.jsonl").exists());
        assert_eq!(fs::read(&refused).unwrap(), before);
    }

    #[test]
    fn recovery_round_trips_an_escaped_talent_name() {
        let directory = tempdir().unwrap();
        let store = CortexStore::new(directory.path().to_path_buf()).unwrap();
        let active = claim_recovery_candidate(&store, "app:name", "one");
        assert!(active.ends_with("app--name/one_active.jsonl"));
        store.recover().unwrap();
        assert!(active.with_file_name("one.jsonl").exists());
    }
}
