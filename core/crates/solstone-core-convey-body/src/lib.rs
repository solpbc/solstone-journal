// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! The Body app's read routes, over the journal's body windows.

mod audit;
mod day;
mod fmt;
mod listing;
mod old_lines;
mod presentation;
mod query;
mod router;
mod rows;
mod status;
mod strava;
mod window;

#[cfg(test)]
mod app_tests;
#[cfg(test)]
mod corpus_test;

pub use router::api_router;

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::{Path, PathBuf};

    #[test]
    fn only_convey_shell_depends_on_this_library() {
        let core = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let manifest = fs::read_to_string(core.join("Cargo.toml")).unwrap();
        let members = manifest
            .lines()
            .skip_while(|line| line.trim() != "members = [")
            .skip(1)
            .take_while(|line| line.trim() != "]")
            .filter_map(|line| line.split('"').nth(1))
            .map(PathBuf::from)
            .collect::<Vec<_>>();

        let shell_member = Path::new("crates/solstone-core-convey-shell");
        for member in members {
            let member_manifest =
                fs::read_to_string(core.join(&member).join("Cargo.toml")).unwrap();
            for table in [
                "[dependencies]",
                "[dev-dependencies]",
                "[build-dependencies]",
            ] {
                let Some(table_start) = member_manifest.find(table) else {
                    continue;
                };
                let table_body = &member_manifest[table_start + table.len()..];
                let table_body = table_body.split("\n[").next().unwrap_or(table_body);
                let names_body = table_body
                    .lines()
                    .any(|line| line.trim_start().starts_with("solstone-core-convey-body"));
                if member == shell_member && table == "[dependencies]" {
                    assert!(names_body, "shell depends on solstone-core-convey-body");
                } else {
                    assert!(
                        !names_body,
                        "{} names solstone-core-convey-body in {table}",
                        member.display()
                    );
                }
            }
        }
    }

    #[test]
    fn body_manifest_has_no_ipc_dependencies() {
        let manifest =
            fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("Cargo.toml")).unwrap();
        for forbidden in ["callosum", "socket", "ipc"] {
            assert!(
                !manifest.contains(forbidden),
                "Body trends must not add {forbidden} to Cargo.toml"
            );
        }
    }
}
