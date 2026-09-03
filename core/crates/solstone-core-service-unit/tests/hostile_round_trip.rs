// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::collections::BTreeMap;

use solstone_core_service_unit::{render_launchd_plist, render_systemd_unit};

mod support;

fn environment(home: &str) -> BTreeMap<String, String> {
    BTreeMap::from([
        ("HOME".to_owned(), home.to_owned()),
        ("PATH".to_owned(), "/usr/bin:/bin".to_owned()),
        ("PYTHONUNBUFFERED".to_owned(), "1".to_owned()),
    ])
}

#[test]
fn hostile_printable_values_round_trip_through_independent_parsers() {
    let hostile = "space 'single' \"double\" \\ $ ${NAME} % %% `tick` café";
    let env = environment(hostile);
    let launcher = format!("/opt/{hostile}/journal");
    let port = format!("5{hostile}");
    let plist = render_launchd_plist(&env, &launcher, &port);
    let unit = render_systemd_unit(&env, &launcher, &port);
    let parsed_plist = support::parse_plist(&plist);
    let parsed_unit = support::parse_unit(&unit);
    let dictionary = parsed_plist.as_dictionary().expect("plist dictionary");
    let arguments = dictionary["ProgramArguments"]
        .as_array()
        .expect("arguments");
    assert_eq!(arguments[0].as_string(), Some(launcher.as_str()));
    assert_eq!(arguments[2].as_string(), Some(port.as_str()));
    assert_eq!(
        parsed_unit.exec_start,
        vec![launcher, "start".to_owned(), port]
    );
    assert_eq!(parsed_unit.environment, env);
}

#[test]
fn control_characters_in_non_journal_fields_stay_within_one_directive_line() {
    let control = "line\ncontrol\u{1}";
    let env = environment(control);
    let launcher = format!("/opt/{control}/journal");
    let unit = render_systemd_unit(&env, &launcher, "5015");
    let baseline = render_systemd_unit(&environment("/home/sol"), "/opt/journal", "5015");
    let exec_line = unit
        .lines()
        .find(|line| line.starts_with("ExecStart="))
        .expect("ExecStart");
    let home_line = unit
        .lines()
        .find(|line| line.starts_with("Environment=\"HOME="))
        .expect("HOME");
    assert!(exec_line.contains("\\n") && exec_line.contains("\\x01"));
    assert!(home_line.contains("\\n") && home_line.contains("\\x01"));
    assert_eq!(unit.lines().count(), baseline.lines().count());
    let parsed = support::parse_unit(&unit);
    assert_eq!(parsed.exec_start[0], launcher);
    assert_eq!(parsed.environment["HOME"], control);
}

#[test]
fn rendered_units_have_no_journal_owned_output_directives() {
    let environment = environment("/home/sol");
    let systemd = render_systemd_unit(&environment, "/opt/journal", "5015");
    assert!(!systemd.contains("StandardOutput="));
    assert!(!systemd.contains("StandardError="));
    let plist = render_launchd_plist(&environment, "/opt/journal", "5015");
    let dictionary = plist::Value::from_reader_xml(plist.as_slice())
        .unwrap()
        .into_dictionary()
        .unwrap();
    assert!(!dictionary.contains_key("StandardOutPath"));
    assert!(!dictionary.contains_key("StandardErrorPath"));
}
