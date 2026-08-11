// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::{fs::File, path::PathBuf};

use crate::{
    context::CheckContext,
    vocabulary::{Check, RunnerResult, Status, make_result},
};

const REPAIR: &str = "run journal service uninstall, then run journal service install separately to reinstall a headless background service";

pub fn run(context: &CheckContext, check: Check) -> RunnerResult {
    let path = context
        .home_dir
        .join("Library/LaunchAgents/org.solpbc.solstone.plist");
    if !path.exists() {
        return Ok(make_result(
            check,
            Status::Skip,
            "launchd plist absent",
            None::<String>,
        ));
    }
    let value = match File::open(&path)
        .and_then(|file| plist::Value::from_reader(file).map_err(std::io::Error::other))
    {
        Ok(value) => value,
        Err(error) => {
            return Ok(make_result(
                check,
                Status::Fail,
                format!(
                    "could not parse plist: {}: {error}",
                    std::any::type_name_of_val(&error)
                        .rsplit("::")
                        .next()
                        .unwrap_or("Error")
                ),
                Some(REPAIR),
            ));
        }
    };
    let executable = value
        .as_dictionary()
        .and_then(|dict| dict.get("ProgramArguments"))
        .and_then(plist::Value::as_array)
        .and_then(|arguments| arguments.first())
        .and_then(plist::Value::as_string);
    let Some(executable) = executable else {
        return Ok(make_result(
            check,
            Status::Fail,
            "plist is missing ProgramArguments[0]",
            Some(REPAIR),
        ));
    };
    let executable = PathBuf::from(executable);
    if !executable.exists() {
        return Ok(make_result(
            check,
            Status::Fail,
            format!(
                "plist points to missing executable: {}",
                executable.display()
            ),
            Some(REPAIR),
        ));
    }
    Ok(make_result(
        check,
        Status::Ok,
        format!("launchd plist target exists ({})", executable.display()),
        None::<String>,
    ))
}
