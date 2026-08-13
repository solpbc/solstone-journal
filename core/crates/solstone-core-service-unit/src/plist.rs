// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::collections::BTreeMap;

use plist::{Dictionary, Value};

use crate::{ServiceUnitError, error::validate_journal_path};

const SERVICE_LABEL: &str = "org.solpbc.solstone";
const SERVICE_FILE_DESCRIPTOR_LIMIT: u32 = 4096;

/// Render the launchd plist for the Solstone supervisor.
pub fn render_launchd_plist(
    env: &BTreeMap<String, String>,
    launcher_path: &str,
    port: &str,
    journal_path: &str,
) -> Result<Vec<u8>, ServiceUnitError> {
    validate_journal_path(journal_path)?;

    let service_log = format!("{journal_path}/health/service.log");
    let environment = env
        .iter()
        .map(|(key, value)| (key.clone(), Value::String(value.clone())))
        .collect();
    let mut keep_alive = Dictionary::new();
    keep_alive.insert("SuccessfulExit".into(), Value::Boolean(false));
    let mut resource_limits = Dictionary::new();
    resource_limits.insert(
        "NumberOfFiles".into(),
        Value::Integer(i64::from(SERVICE_FILE_DESCRIPTOR_LIMIT).into()),
    );
    let mut plist = Dictionary::new();
    plist.insert("Label".into(), Value::String(SERVICE_LABEL.into()));
    plist.insert(
        "ProgramArguments".into(),
        Value::Array(vec![
            Value::String(launcher_path.into()),
            Value::String("start".into()),
            Value::String(port.into()),
        ]),
    );
    plist.insert(
        "EnvironmentVariables".into(),
        Value::Dictionary(environment),
    );
    plist.insert("StandardOutPath".into(), Value::String(service_log.clone()));
    plist.insert("StandardErrorPath".into(), Value::String(service_log));
    plist.insert("RunAtLoad".into(), Value::Boolean(true));
    plist.insert("KeepAlive".into(), Value::Dictionary(keep_alive));
    plist.insert(
        "SoftResourceLimits".into(),
        Value::Dictionary(resource_limits),
    );

    let mut bytes = Vec::new();
    Value::Dictionary(plist)
        .to_writer_xml(&mut bytes)
        .expect("in-memory plist serializes");
    Ok(bytes)
}
