// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::collections::BTreeMap;

use plist::{Dictionary, Value};

const SERVICE_LABEL: &str = "org.solpbc.solstone";
const SERVICE_FILE_DESCRIPTOR_LIMIT: u32 = 4096;

/// Render the launchd plist for the Solstone supervisor.
pub fn render_launchd_plist(
    env: &BTreeMap<String, String>,
    launcher_path: &str,
    port: &str,
) -> Vec<u8> {
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
    bytes
}

#[cfg(test)]
mod tests {
    use std::collections::{BTreeMap, BTreeSet};

    use plist::Value;

    use super::render_launchd_plist;

    #[test]
    fn renders_the_complete_launchd_semantic_model() {
        let environment = BTreeMap::from([
            ("HOME".to_owned(), "/home/sol".to_owned()),
            ("PATH".to_owned(), "/usr/bin:/bin".to_owned()),
            ("PYTHONUNBUFFERED".to_owned(), "1".to_owned()),
        ]);
        let bytes = render_launchd_plist(&environment, "/home/sol/.local/bin/journal", "5015");
        let plist = Value::from_reader_xml(bytes.as_slice()).expect("rendered plist parses");
        let dictionary = plist.as_dictionary().expect("plist dictionary");

        assert_eq!(
            dictionary
                .keys()
                .map(String::as_str)
                .collect::<BTreeSet<_>>(),
            BTreeSet::from([
                "EnvironmentVariables",
                "KeepAlive",
                "Label",
                "ProgramArguments",
                "RunAtLoad",
                "SoftResourceLimits",
            ])
        );
        assert_eq!(dictionary["Label"].as_string(), Some("org.solpbc.solstone"));
        assert_eq!(
            dictionary["ProgramArguments"].as_array(),
            Some(&vec![
                Value::String("/home/sol/.local/bin/journal".to_owned()),
                Value::String("start".to_owned()),
                Value::String("5015".to_owned()),
            ])
        );
        assert_eq!(
            dictionary["EnvironmentVariables"].as_dictionary(),
            Some(
                &environment
                    .iter()
                    .map(|(key, value)| (key.clone(), Value::String(value.clone())))
                    .collect()
            )
        );
        assert!(!dictionary.contains_key("StandardOutPath"));
        assert!(!dictionary.contains_key("StandardErrorPath"));
        assert_eq!(dictionary["RunAtLoad"].as_boolean(), Some(true));
        assert_eq!(
            dictionary["KeepAlive"]
                .as_dictionary()
                .expect("keep-alive dictionary")["SuccessfulExit"],
            Value::Boolean(false)
        );
        assert_eq!(
            dictionary["SoftResourceLimits"]
                .as_dictionary()
                .expect("resource-limit dictionary")["NumberOfFiles"],
            Value::Integer(4096_i64.into())
        );
    }
}
