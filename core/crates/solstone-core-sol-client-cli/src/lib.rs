// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use solstone_core_sol_client::aggregate;
use solstone_core_sol_client::command::{CommandContext, CommandOutput};
use solstone_core_sol_client::resident::ResidentHandler;
use solstone_core_sol_client::seam::{
    BuildIdentityProvider, ClientItemIdProvider, Clock, FileProvider, HttpTransport,
    LinkJoinPairingSeam, LinkServeRunner, LinkStatusProbe, NotificationSink,
};
use std::collections::BTreeMap;
use std::ffi::{OsStr, OsString};

pub mod help;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    Migrated { path: Vec<OsString> },
    Import { args: Vec<OsString> },
    Status { args: Vec<OsString> },
    MovedStub { name: OsString },
    Unsupported { args: Vec<OsString> },
}

pub struct DispatchSeams<'a> {
    pub transport: &'a dyn HttpTransport,
    pub clock: Option<&'a dyn Clock>,
    pub files: Option<&'a dyn FileProvider>,
    pub build_identity: Option<&'a dyn BuildIdentityProvider>,
    pub client_item_ids: Option<&'a dyn ClientItemIdProvider>,
    pub notification_sink: Option<&'a dyn NotificationSink>,
}

pub struct LinkDispatchSeams<'a> {
    pub transport: &'a dyn HttpTransport,
    pub clock: Option<&'a dyn Clock>,
    pub files: Option<&'a dyn FileProvider>,
    pub link_pairing: Option<&'a dyn LinkJoinPairingSeam>,
    pub link_serve: Option<&'a dyn LinkServeRunner>,
    pub link_status_probe: Option<&'a dyn LinkStatusProbe>,
}

pub enum LinkDispatch {
    Buffered(CommandOutput),
    Resident {
        handler: ResidentHandler,
        args: Vec<String>,
    },
}

#[must_use]
pub fn evaluate_args(args: &[OsString]) -> Outcome {
    match args {
        [command, rest @ ..] if command == OsStr::new("call") => evaluate_call(rest),
        [command, rest @ ..] if command == OsStr::new("import") => {
            match_generated_surface_path("sol-import", &[String::from("import")]).map_or_else(
                || Outcome::Unsupported {
                    args: args.to_vec(),
                },
                |_entry| Outcome::Import {
                    args: rest.to_vec(),
                },
            )
        }
        [command, rest @ ..] if command == OsStr::new("status") => {
            match_generated_surface_path("sol-status", &[String::from("status")]).map_or_else(
                || Outcome::Unsupported {
                    args: args.to_vec(),
                },
                |_entry| Outcome::Status {
                    args: rest.to_vec(),
                },
            )
        }
        _ => Outcome::Unsupported {
            args: args.to_vec(),
        },
    }
}

#[must_use]
pub fn dispatch_sol_import_with_seams(
    args: &[String],
    env: &BTreeMap<String, String>,
    stdin: &str,
    today: &str,
    seams: DispatchSeams<'_>,
) -> CommandOutput {
    let Some((_, handler)) = match_generated_surface_path("sol-import", &[String::from("import")])
    else {
        return CommandOutput::failure("unsupported command.\n", 64);
    };
    handler(CommandContext {
        args,
        env,
        stdin,
        today,
        transport: seams.transport,
        clock: seams.clock,
        files: seams.files,
        build_identity: seams.build_identity,
        client_item_ids: seams.client_item_ids,
        notification_sink: None,
        link_pairing: None,
        link_serve: None,
        link_status_probe: None,
    })
}

#[must_use]
pub fn dispatch_sol_status_with_seams(
    args: &[String],
    env: &BTreeMap<String, String>,
    stdin: &str,
    today: &str,
    seams: DispatchSeams<'_>,
) -> CommandOutput {
    let Some((_, handler)) = match_generated_surface_path("sol-status", &[String::from("status")])
    else {
        return CommandOutput::failure("unsupported command.\n", 64);
    };
    handler(CommandContext {
        args,
        env,
        stdin,
        today,
        transport: seams.transport,
        clock: seams.clock,
        files: seams.files,
        build_identity: seams.build_identity,
        client_item_ids: seams.client_item_ids,
        notification_sink: None,
        link_pairing: None,
        link_serve: None,
        link_status_probe: None,
    })
}

#[must_use]
pub fn dispatch_sol_link_with_seams(
    args: &[String],
    env: &BTreeMap<String, String>,
    stdin: &str,
    today: &str,
    seams: LinkDispatchSeams<'_>,
) -> LinkDispatch {
    let Some((path, remaining)) = link_lookup_path(args) else {
        return LinkDispatch::Buffered(CommandOutput::failure("unsupported command.\n", 64));
    };
    if let Some((_, handler)) = match_generated_resident_surface_path("sol-link", &path) {
        return LinkDispatch::Resident {
            handler,
            args: remaining.to_vec(),
        };
    }
    let Some((_, handler)) = match_generated_surface_path("sol-link", &path) else {
        return LinkDispatch::Buffered(CommandOutput::failure("unsupported command.\n", 64));
    };
    LinkDispatch::Buffered(handler(CommandContext {
        args: remaining,
        env,
        stdin,
        today,
        transport: seams.transport,
        clock: seams.clock,
        files: seams.files,
        build_identity: None,
        client_item_ids: None,
        notification_sink: None,
        link_pairing: seams.link_pairing,
        link_serve: seams.link_serve,
        link_status_probe: seams.link_status_probe,
    }))
}

fn evaluate_call(args: &[OsString]) -> Outcome {
    let Some((entry, len)) = match_generated_path(args) else {
        return Outcome::Unsupported {
            args: args.to_vec(),
        };
    };
    match entry.entry_type {
        "http" | "local" => Outcome::Migrated {
            path: args[..len].to_vec(),
        },
        "moved-stub" => Outcome::MovedStub {
            name: args[0].clone(),
        },
        _ => Outcome::Unsupported {
            args: args.to_vec(),
        },
    }
}

#[must_use]
pub fn dispatch_sol_call(
    args: &[String],
    env: &BTreeMap<String, String>,
    stdin: &str,
    today: &str,
    transport: &dyn HttpTransport,
) -> CommandOutput {
    dispatch_sol_call_with_seams(
        args,
        env,
        stdin,
        today,
        DispatchSeams {
            transport,
            clock: None,
            files: None,
            build_identity: None,
            client_item_ids: None,
            notification_sink: None,
        },
    )
}

#[must_use]
pub fn dispatch_sol_call_with_seams(
    args: &[String],
    env: &BTreeMap<String, String>,
    stdin: &str,
    today: &str,
    seams: DispatchSeams<'_>,
) -> CommandOutput {
    let Some((_, handler, len)) = match_generated_str_path(args) else {
        return CommandOutput::failure("unsupported command.\n", 64);
    };
    let remaining = args[len..].to_vec();
    handler(CommandContext {
        args: &remaining,
        env,
        stdin,
        today,
        transport: seams.transport,
        clock: seams.clock,
        files: seams.files,
        build_identity: seams.build_identity,
        client_item_ids: seams.client_item_ids,
        notification_sink: None,
        link_pairing: None,
        link_serve: None,
        link_status_probe: None,
    })
}

#[must_use]
pub fn resolve_sol_call_leaf(args: &[String]) -> Option<&'static aggregate::InventoryEntry> {
    match_generated_str_path(args).map(|(entry, _handler, _len)| entry)
}

#[must_use]
pub fn resolve_surface_leaf(
    surface: &str,
    args: &[String],
) -> Option<&'static aggregate::InventoryEntry> {
    if surface == "sol-call" {
        return resolve_sol_call_leaf(args);
    }
    match_generated_surface_path(surface, args)
        .map(|(entry, _handler)| entry)
        .or_else(|| {
            match_generated_resident_surface_path(surface, args).map(|(entry, _handler)| entry)
        })
}

fn match_generated_path(args: &[OsString]) -> Option<(&'static aggregate::InventoryEntry, usize)> {
    let utf8 = args
        .iter()
        .map(|arg| arg.to_str().map(str::to_string))
        .collect::<Option<Vec<_>>>()?;
    match_generated_str_path(&utf8).map(|(entry, _handler, len)| (entry, len))
}

fn match_generated_str_path(
    args: &[String],
) -> Option<(
    &'static aggregate::InventoryEntry,
    aggregate::Handler,
    usize,
)> {
    let max_len = aggregate::entries()
        .iter()
        .map(|entry| entry.path.len())
        .max()
        .unwrap_or(0);
    for len in (1..=args.len().min(max_len)).rev() {
        let path = args[..len].iter().map(String::as_str).collect::<Vec<_>>();
        if let Some((entry, handler)) = aggregate::handler_for(&path) {
            if entry.surface != "sol-call" {
                continue;
            }
            return Some((entry, handler, len));
        }
    }
    None
}

fn match_generated_surface_path(
    surface: &str,
    args: &[String],
) -> Option<(&'static aggregate::InventoryEntry, aggregate::Handler)> {
    let path = args.iter().map(String::as_str).collect::<Vec<_>>();
    aggregate::handler_for_surface(surface, &path)
}

fn match_generated_resident_surface_path(
    surface: &str,
    args: &[String],
) -> Option<(&'static aggregate::InventoryEntry, ResidentHandler)> {
    let path = args.iter().map(String::as_str).collect::<Vec<_>>();
    aggregate::resident_handler_for_surface(surface, &path)
}

fn link_lookup_path(args: &[String]) -> Option<(Vec<String>, &[String])> {
    match args {
        [command, verb, rest @ ..] if command == "link" => {
            Some((vec![String::from("link"), verb.clone()], rest))
        }
        [verb, rest @ ..] => Some((vec![String::from("link"), verb.clone()], rest)),
        [] => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use solstone_core_sol_client::seam::{
        ScriptedHttpTransport, ScriptedLinkJoinPairingSeam, ScriptedLinkServeRunner,
    };

    fn args(values: &[&str]) -> Vec<OsString> {
        values.iter().map(OsString::from).collect()
    }

    fn string_args(values: &[&str]) -> Vec<String> {
        values.iter().map(|value| (*value).to_string()).collect()
    }

    #[test]
    fn routes_named_builtins_to_generated_authority() {
        assert_eq!(
            evaluate_args(&args(&["call", "identity"])),
            Outcome::MovedStub {
                name: OsString::from("identity")
            }
        );
    }

    #[test]
    fn classifies_call_leaf_as_migrated_shell() {
        assert_eq!(
            evaluate_args(&args(&["call", "activities", "list"])),
            Outcome::Migrated {
                path: args(&["activities", "list"])
            }
        );
    }

    #[test]
    fn routes_top_level_import_to_import_shell() {
        assert_eq!(
            evaluate_args(&args(&["import", "sample.txt"])),
            Outcome::Import {
                args: args(&["sample.txt"])
            }
        );
    }

    #[test]
    fn routes_top_level_status_to_status_shell() {
        assert_eq!(
            evaluate_args(&args(&["status"])),
            Outcome::Status { args: vec![] }
        );
    }

    #[test]
    fn sol_link_dispatch_resolves_join_and_serve_from_full_or_trimmed_argv() {
        let env = BTreeMap::new();
        let transport = ScriptedHttpTransport::new(vec![]);
        let pairing = ScriptedLinkJoinPairingSeam::new(vec![]);
        let serve = ScriptedLinkServeRunner::new(vec![]);
        let seams = || LinkDispatchSeams {
            transport: &transport,
            clock: None,
            files: None,
            link_pairing: Some(&pairing),
            link_serve: Some(&serve),
            link_status_probe: None,
        };

        let join_args = string_args(&["link", "join"]);
        match dispatch_sol_link_with_seams(&join_args, &env, "", "20260726", seams()) {
            LinkDispatch::Buffered(output) => {
                assert_eq!(output.exit, 2);
                assert!(
                    output
                        .stderr
                        .contains("the following arguments are required: --code")
                );
            }
            LinkDispatch::Resident { .. } => panic!("link join must stay buffered"),
        }

        let status_args = string_args(&["link", "status", "--help"]);
        match dispatch_sol_link_with_seams(&status_args, &env, "", "20260726", seams()) {
            LinkDispatch::Buffered(output) => {
                assert_eq!(output.exit, 0);
                assert!(output.stdout.contains("usage: solstone link status"));
            }
            LinkDispatch::Resident { .. } => panic!("link status must stay buffered"),
        }

        let full_serve_args = string_args(&["link", "serve", "--help"]);
        match dispatch_sol_link_with_seams(&full_serve_args, &env, "", "20260726", seams()) {
            LinkDispatch::Resident { args, .. } => assert_eq!(args, string_args(&["--help"])),
            LinkDispatch::Buffered(_) => panic!("link serve must resolve as resident"),
        }

        let trimmed_serve_args = string_args(&["serve", "--help"]);
        match dispatch_sol_link_with_seams(&trimmed_serve_args, &env, "", "20260726", seams()) {
            LinkDispatch::Resident { args, .. } => assert_eq!(args, string_args(&["--help"])),
            LinkDispatch::Buffered(_) => panic!("trimmed link serve must resolve as resident"),
        }
    }

    #[test]
    fn classifies_unported_call_as_unsupported_without_spawn_path() {
        assert_eq!(
            evaluate_args(&args(&["call", "transcripts", "list"])),
            Outcome::Unsupported {
                args: args(&["transcripts", "list"])
            }
        );
    }
}
