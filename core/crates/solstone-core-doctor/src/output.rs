// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc
use crate::vocabulary::*;
use chrono::{SecondsFormat, Utc};
use serde_json::json;
use std::io::Write;
pub fn emit_json(results: &[CheckResult]) {
    println!(
        "{}",
        json!({"checks":results,"summary":summary_counts(results)})
    );
}
pub fn emit_text(results: &[CheckResult], verbose: bool) {
    let mut stdout = std::io::stdout().lock();
    emit_text_to(&mut stdout, results, verbose).expect("write doctor text output");
}

pub fn emit_text_to(
    writer: &mut impl Write,
    results: &[CheckResult],
    verbose: bool,
) -> std::io::Result<()> {
    for result in results
        .iter()
        .filter(|r| verbose || matches!(r.status, Status::Fail | Status::Warn))
    {
        writeln!(
            writer,
            "  {} {} — {}",
            status_label(result),
            result.name,
            result.detail
        )?;
        if let Some(fix) = &result.fix {
            writeln!(writer, "    → {fix}")?;
        }
    }
    let s = summary_counts(results);
    writeln!(
        writer,
        "doctor: {} checks, {} failed, {} warnings, {} skipped, {} errors",
        s["total"], s["failed"], s["warnings"], s["skipped"], s["errors"]
    )?;
    Ok(())
}
pub fn summary_status(results: &[CheckResult]) -> &'static str {
    if results_failed(results) {
        "failed"
    } else if results.iter().any(|r| {
        r.status == Status::Warn || (r.severity == Severity::Advisory && r.status == Status::Fail)
    }) {
        "warning"
    } else {
        "ok"
    }
}
pub fn emit_jsonl(results: &[CheckResult], started_at: &str, duration_ms: u128, port: u16) {
    let mut out = std::io::stdout().lock();
    emit_jsonl_to(&mut out, results, started_at, duration_ms, port);
}

pub fn emit_jsonl_to(
    writer: &mut impl Write,
    results: &[CheckResult],
    started_at: &str,
    duration_ms: u128,
    port: u16,
) {
    let now = || Utc::now().to_rfc3339_opts(SecondsFormat::Secs, true);
    let _ = writeln!(
        writer,
        "{}",
        json!({"event":"doctor.started","ts":now(),"started_at":started_at,"version":env!("CARGO_PKG_VERSION"),"port":port})
    );
    for r in results {
        let status = match r.status {
            Status::Ok => "ok",
            Status::Warn => "warning",
            Status::Fail => "failed",
            Status::Skip => "skipped",
        };
        let mut event = json!({"event":"check.completed","ts":now(),"name":r.name,"severity":r.severity,"status":status,"detail":r.detail,"fix":r.fix.clone().unwrap_or_default(),"execution_error":r.execution_error});
        if let Some(facts) = &r.observer_delivery {
            event.as_object_mut().expect("object").insert(
                "observer_delivery".to_owned(),
                serde_json::to_value(facts).expect("observer delivery facts serialize"),
            );
        }
        let _ = writeln!(writer, "{event}");
    }
    let _ = writeln!(
        writer,
        "{}",
        json!({"event":"doctor.completed","ts":now(),"started_at":started_at,"status":summary_status(results),"duration_ms":duration_ms,"summary":summary_counts(results)})
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vocabulary::{Check, Platform, Severity, Status, make_result};

    #[test]
    fn emit_text_to_bytes_match_former_println_lines() {
        let check = Check {
            name: "observer_delivery_stall",
            severity: Severity::Advisory,
            platforms: &[Platform::Linux],
        };
        let warn = make_result(check, Status::Warn, "detail here", Some("do this"));
        let skip = make_result(check, Status::Skip, "skipped", None::<String>);
        let mut buf = Vec::new();
        emit_text_to(&mut buf, &[warn.clone(), skip], false).unwrap();
        let expected = format!(
            "  {} {} — {}\n    → {}\ndoctor: {} checks, {} failed, {} warnings, {} skipped, {} errors\n",
            status_label(&warn),
            warn.name,
            warn.detail,
            warn.fix.as_deref().unwrap(),
            2,
            0,
            1,
            1,
            0,
        );
        assert_eq!(String::from_utf8(buf).unwrap(), expected);
    }

    #[test]
    fn emit_text_to_propagates_writer_failure() {
        struct FailingWriter;
        impl Write for FailingWriter {
            fn write(&mut self, _buf: &[u8]) -> std::io::Result<usize> {
                Err(std::io::Error::other("injected"))
            }

            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }

        let check = Check {
            name: "observer_delivery_stall",
            severity: Severity::Advisory,
            platforms: &[Platform::Linux],
        };
        let result = make_result(check, Status::Warn, "detail", None::<String>);
        assert!(emit_text_to(&mut FailingWriter, &[result], false).is_err());
    }
}
