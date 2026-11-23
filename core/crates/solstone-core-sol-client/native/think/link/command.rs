// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::collections::BTreeMap;
use std::fs::{self, DirBuilder, File, OpenOptions};
use std::io::{self, Write};
#[cfg(unix)]
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::SystemTime;

use jiff::Timestamp;
use serde_json::{Map, Value};
use solstone_core_cli_copy::{display_for, render_for};
use spl_core::pairlink::{PairLinkError, ParsedPairLink};

use crate::command::{CommandContext, CommandOutput};
use crate::json_format::{json_compact_ascii, json_pretty_ascii};
use crate::resident::ResidentCommand;
use crate::seam::{
    LinkJoinCredential, LinkJoinDirectRequest, LinkJoinPairTarget, LinkJoinPairingError,
    LinkJoinPairingErrorKind, LinkJoinRelayControlEndpoint, LinkJoinRelayErrorKind,
    LinkJoinRelayRequest, LinkJoinTerminal, LinkJournalMetadata, LinkServeBundle,
    LinkServeCarrierPolicy, LinkServeEndpoint, LinkServeError, LinkServeErrorKind,
    LinkServeRequest, LinkServeRuntimeRecord, LinkServeStatusSnapshot, LinkServeTransportErrorKind,
};

// `link confirm` can end a link that was already sending, so these say nothing about
// what reached that journal.
const UNVERIFIED_KEY: &str = "unverified_ca_accepted";
const DEFAULT_CLIENT_LABEL: &str = "linked-system";
const DEFAULT_SERVE_PORT: u16 = 5015;
const PAIR_LINK_PREFIX: &str = "https://go.solstone.app/p#";
const LOCAL_ENDPOINTS_MAX_BYTES: usize = 16 * 1024;
static BUNDLE_STAGING_SEQUENCE: AtomicU64 = AtomicU64::new(0);
const BUNDLE_FILES: &[&str] = &[
    "private.pem",
    "cert.pem",
    "chain.pem",
    "home_attestation.jwt",
    "peer.json",
];

#[must_use]
pub fn link_join(ctx: CommandContext<'_>) -> CommandOutput {
    let locale = ctx.human_locale_with_options(&["--home", "--code", "--as", "--label", "--mark"]);
    let parsed = match parse_args(locale, ctx.args) {
        Ok(parsed) => parsed,
        Err(error) => return argparse_error(locale, error),
    };
    if parsed.help {
        return CommandOutput::success(render_for(locale, "journal.cli.sol.link.join.help", &[]));
    }
    if parsed.code.is_none() {
        return argparse_error(
            locale,
            render_for(locale, "journal.cli.sol.link.join.required.code", &[]),
        );
    }
    if let Some(unknown) = parsed.unknown {
        return argparse_error(
            locale,
            display_for(
                locale,
                "journal.cli.sol.link.argument.unrecognized",
                [("unknown", &unknown)],
            ),
        );
    }

    let as_role = parsed.as_role.unwrap_or_default();
    if !matches!(as_role.as_str(), "" | "phone" | "observer") {
        return CommandOutput::failure(
            render_for(locale, "journal.cli.sol.link.join.invalid.role", &[]),
            2,
        );
    }

    let label = match parsed.label {
        Some(label) => {
            if let Some(error) = label_error(locale, &label) {
                return CommandOutput::failure(format!("{error}\n"), 2);
            }
            label
        }
        None => hostname_client_label(ctx.env),
    };

    let Some(seam) = ctx.link_pairing else {
        return CommandOutput::failure(
            render_for(locale, "journal.cli.sol.link.join.pairing.unavailable", &[]),
            1,
        );
    };

    if parsed.mark_repeated {
        return CommandOutput::failure(
            render_for(locale, "journal.cli.sol.link.mark.usage", &[]),
            2,
        );
    }
    if let Some(mark) = &parsed.mark
        && mark_words(mark).len() != 2
    {
        return CommandOutput::failure(
            render_for(locale, "journal.cli.sol.link.mark.usage", &[]),
            2,
        );
    }

    let pair_request = match parse_pair_request(
        locale,
        parsed
            .code
            .as_deref()
            .expect("code presence checked")
            .trim(),
        parsed.home.as_deref(),
    ) {
        Ok(pair_request) => pair_request,
        Err(error) => return CommandOutput::failure(format!("{error}\n"), 1),
    };

    let additional_fields = Map::new();
    let bundle_dir = match observer_bundle_dir(locale, &label, ctx.env) {
        Ok(bundle_dir) => {
            if path_lexists(&bundle_dir) {
                return CommandOutput::failure(
                    format!("{}\n", existing_path_message(locale, &bundle_dir)),
                    1,
                );
            }
            bundle_dir
        }
        Err(error) => return CommandOutput::failure(format!("{error}\n"), 1),
    };

    // Before the request goes out, so a refusal here doesn't spend the link.
    let terminal = if parsed.mark.is_some() {
        None
    } else {
        let Some(terminal) = seam.open_terminal() else {
            return CommandOutput::failure(
                render_for(locale, "journal.cli.sol.link.mark.terminal.required", &[]),
                1,
            );
        };
        Some(terminal)
    };

    let credential = match pair_request {
        PairRequest::Direct(request) => {
            let request = LinkJoinDirectRequest {
                targets: request.targets,
                nonce_hex: request.nonce_hex,
                ca_fp_prefix: request.ca_fp_prefix,
                device_label: label.clone(),
                additional_fields,
            };
            seam.pair_direct(request)
        }
        PairRequest::Relay(request) => {
            let request = LinkJoinRelayRequest {
                relay_origin: request.relay_origin,
                secret: request.secret,
                ca_fp_spki: request.ca_fp_spki,
                device_label: label.clone(),
                additional_fields,
            };
            seam.pair_relay(request)
        }
    };
    let credential = match credential {
        Ok(credential) => credential,
        Err(error) => {
            return CommandOutput::failure(format!("{}\n", pairing_error_text(locale, error)), 1);
        }
    };

    if let Err(error) = validate_credential(locale, &credential) {
        return CommandOutput::failure(format!("{error}\n"), 1);
    }
    let local_endpoints = match normalized_local_endpoints(locale, &credential.local_endpoints) {
        Ok(value) => value,
        Err(error) => return CommandOutput::failure(format!("{error}\n"), 1),
    };
    if json_compact_ascii(&local_endpoints).len() > LOCAL_ENDPOINTS_MAX_BYTES {
        return CommandOutput::failure(
            render_for(locale, "journal.cli.sol.link.join.response.large", &[]),
            1,
        );
    }

    // Nothing is written until the owner has confirmed the journal's mark.
    let confirmed = match terminal {
        Some(mut terminal) => {
            match answer_at_terminal(locale, terminal.as_mut(), &credential.instance_id) {
                MarkAnswer::Yes => Ok(()),
                MarkAnswer::No => Err(CommandOutput::failure(
                    render_for(locale, "journal.cli.sol.link.mark.join.mismatch", &[]),
                    1,
                )),
                MarkAnswer::Cancelled | MarkAnswer::WalkedAway => Err(CommandOutput::failure(
                    render_for(locale, "journal.cli.sol.link.mark.join.cancel", &[]),
                    1,
                )),
            }
        }
        None => answer_from_words(
            locale,
            parsed.mark.as_deref().expect("mark or terminal present"),
            &credential.instance_id,
        ),
    };
    if let Err(refused) = confirmed {
        seam.retire(&credential);
        return refused;
    }

    let chain_pem = join_chain(&credential.ca_chain_pem);
    let peer_json = peer_json(&label, now_utc(ctx.clock), &credential, local_endpoints);
    let mut files = BTreeMap::new();
    files.insert(
        "private.pem".to_string(),
        credential.client_key_pem.as_bytes().to_vec(),
    );
    files.insert(
        "cert.pem".to_string(),
        credential.client_cert_pem.as_bytes().to_vec(),
    );
    files.insert("chain.pem".to_string(), chain_pem.into_bytes());
    files.insert(
        "home_attestation.jwt".to_string(),
        credential
            .home_attestation
            .as_ref()
            .expect("home_attestation checked")
            .as_bytes()
            .to_vec(),
    );
    let store = crate::link_credentials::LinkCredentialStore::new(bundle_dir.clone(), &label);
    let _lock = match store.acquire_lock_for(locale) {
        Ok(guard) => guard,
        Err(err) => {
            return CommandOutput::failure(
                display_for(
                    locale,
                    "journal.cli.sol.link.join.lock.error",
                    [("err", &err.message_for(locale))],
                ),
                1,
            );
        }
    };
    if path_lexists(&bundle_dir) {
        return CommandOutput::failure(
            format!("{}\n", spent_existing_path_message(locale, &bundle_dir)),
            1,
        );
    }
    if let (Some(origin), Some(token), Some(expires_at)) = (
        &credential.relay_origin,
        &credential.relay_device_token,
        credential.relay_device_token_expires_at,
    ) {
        let cert_sha256 = format!(
            "sha256:{}",
            spl_core::ca::sha256_hex(credential.client_cert_pem.as_bytes())
        );
        let record = crate::link_credentials::RelayAccessRecord {
            state: crate::link_credentials::RelayAccessState::Ready,
            relay_origin: Some(origin.clone()),
            device_token: Some(token.clone()),
            expires_at: Some(expires_at),
            access_generation: 1,
            identity: crate::link_credentials::PairingIdentity {
                cert_sha256,
                instance_id: credential.instance_id.clone(),
                ca_fingerprint: credential.ca_fingerprint.clone(),
            },
        };
        if let Ok(bytes) = serde_json::to_vec_pretty(&record) {
            files.insert("relay_access.json".to_string(), bytes);
        }
    }
    files.insert("peer.json".to_string(), peer_json.into_bytes());

    if let Err(error) = publish_bundle_atomic(locale, &bundle_dir, &files) {
        let message = if error.kind() == io::ErrorKind::AlreadyExists {
            spent_existing_path_message(locale, &bundle_dir)
        } else {
            error.to_string()
        };
        return CommandOutput::failure(format!("{message}\n"), 1);
    }
    let _ = fs::remove_file(bundle_dir.join("journal_metadata.json"));
    let _ = fs::remove_file(bundle_dir.join("serve_runtime.json"));

    CommandOutput::success(display_for(
        locale,
        "journal.cli.sol.link.join.paired",
        [("path", &bundle_dir.display())],
    ))
}

enum MarkAnswer {
    Yes,
    No,
    /// Cancelled at the couldn't-verify question.
    Cancelled,
    /// Nobody left at the terminal to answer.
    WalkedAway,
}

/// Ask on the owner's terminal.
fn answer_at_terminal(locale: &str, terminal: &mut dyn LinkJoinTerminal, jid: &str) -> MarkAnswer {
    let spec = crate::mark_text::mark_from_jid(jid)
        .ok()
        .map(|mark| mark.to_render_spec());
    let ask = if let Some(spec) = &spec {
        let spoken = format_spoken_mark(spec);
        terminal.write_text(&display_for(
            locale,
            "journal.cli.sol.link.mark.prompt.identified",
            [("spoken", &spoken)],
        ));
        render_for(locale, "journal.cli.sol.link.mark.ask.identified", &[])
    } else {
        terminal.write_text(&render_for(
            locale,
            "journal.cli.sol.link.mark.prompt.unavailable",
            &[],
        ));
        render_for(locale, "journal.cli.sol.link.mark.ask.unavailable", &[])
    };
    let (go_ahead, stop): (&[&str], &[&str]) = if spec.is_some() {
        (&["yes", "y"], &["no", "n"])
    } else {
        (&["continue"], &["cancel"])
    };
    loop {
        let Some(line) = terminal.read_answer() else {
            return MarkAnswer::WalkedAway;
        };
        let answer = line.trim().to_ascii_lowercase();
        if go_ahead.contains(&answer.as_str()) {
            return MarkAnswer::Yes;
        }
        if stop.contains(&answer.as_str()) {
            return if spec.is_some() {
                MarkAnswer::No
            } else {
                MarkAnswer::Cancelled
            };
        }
        terminal.write_text(&format!("{ask}\n"));
    }
}

/// Compare the scripted `--mark` words, already checked to be two, to the mark.
fn answer_from_words(locale: &str, words: &str, jid: &str) -> Result<(), CommandOutput> {
    let Ok(mark) = crate::mark_text::mark_from_jid(jid) else {
        return Err(CommandOutput::failure(
            render_for(locale, "journal.cli.sol.link.mark.join.unverifiable", &[]),
            1,
        ));
    };
    let spec = mark.to_render_spec();
    let given = mark_words(words);
    if given.len() == 2
        && given[0].eq_ignore_ascii_case(&spec.words[0])
        && given[1].eq_ignore_ascii_case(&spec.words[1])
    {
        return Ok(());
    }
    Err(CommandOutput::failure(
        render_for(locale, "journal.cli.sol.link.mark.join.wrong", &[]),
        1,
    ))
}

fn mark_words(value: &str) -> Vec<&str> {
    value
        .split(|ch: char| ch.is_ascii_whitespace() || ch == '\u{b7}')
        .filter(|piece| !piece.is_empty())
        .collect()
}

/// A link waits for the owner if its stored id isn't the CA it pins, or if no such
/// CA can be read and the owner hasn't said to go on without one.
fn bundle_is_held(selection: &ServeBundleSelection, ca_jid: Option<&str>) -> bool {
    match ca_jid {
        Some(jid) => jid != selection.bundle.instance_id,
        None => !unverified_link_accepted(&selection.bundle_dir),
    }
}

fn unverified_link_accepted(bundle_dir: &Path) -> bool {
    fs::read_to_string(bundle_dir.join("peer.json"))
        .ok()
        .and_then(|text| serde_json::from_str::<Value>(&text).ok())
        .is_some_and(|peer| peer.get(UNVERIFIED_KEY) == Some(&Value::Bool(true)))
}

fn held_lines(locale: &str, label: &str) -> String {
    display_for(locale, "journal.cli.sol.link.held", [("label", &label)])
}

#[must_use]
pub fn link_confirm(ctx: CommandContext<'_>) -> CommandOutput {
    let locale = ctx.human_locale_with_options(&["--label", "--mark"]);
    let parsed = match parse_confirm_args(locale, ctx.args) {
        Ok(parsed) => parsed,
        Err(error) => return confirm_usage_error(locale, &error),
    };
    if parsed.help {
        return CommandOutput::success(render_for(
            locale,
            "journal.cli.sol.link.confirm.help",
            &[],
        ));
    }
    if let Some(unknown) = parsed.unknown {
        return confirm_usage_error(
            locale,
            &display_for(
                locale,
                "journal.cli.sol.link.argument.unrecognized",
                [("unknown", &unknown)],
            ),
        );
    }
    if parsed.mark_repeated
        || parsed
            .mark
            .as_deref()
            .is_some_and(|mark| mark_words(mark).len() != 2)
    {
        return CommandOutput::failure(
            render_for(locale, "journal.cli.sol.link.mark.usage", &[]),
            2,
        );
    }
    let Some(seam) = ctx.link_pairing else {
        return CommandOutput::failure(
            render_for(locale, "journal.cli.sol.link.join.pairing.unavailable", &[]),
            1,
        );
    };
    let selection = match resolve_serve_bundle(locale, parsed.label.as_deref(), ctx.env) {
        Ok(selection) => selection,
        Err(error) => {
            return CommandOutput::failure(
                display_for(
                    locale,
                    "journal.cli.sol.link.confirm.error",
                    [("error", &error)],
                ),
                1,
            );
        }
    };
    let ca_jid = seam.pinned_ca_jid(&selection.bundle.ca_chain_pem);
    if !bundle_is_held(&selection, ca_jid.as_deref()) {
        return CommandOutput::success(render_for(
            locale,
            "journal.cli.sol.link.confirm.done",
            &[],
        ));
    }

    let answer = if let Some(mark) = parsed.mark.as_deref() {
        let Some(jid) = ca_jid.as_deref() else {
            return CommandOutput::failure(
                render_for(locale, "journal.cli.sol.link.mark.confirm.unavailable", &[]),
                5,
            );
        };
        answer_from_words(locale, mark, jid).map_err(|_| {
            retire_bundle(seam, &selection);
            CommandOutput::failure(
                render_for(locale, "journal.cli.sol.link.mark.confirm.wrong", &[]),
                1,
            )
        })
    } else {
        let Some(mut terminal) = seam.open_terminal() else {
            return CommandOutput::failure(
                render_for(locale, "journal.cli.sol.link.mark.confirm.terminal", &[]),
                1,
            );
        };
        match answer_at_terminal(
            locale,
            terminal.as_mut(),
            ca_jid.as_deref().unwrap_or_default(),
        ) {
            MarkAnswer::Yes => Ok(()),
            MarkAnswer::No => {
                retire_bundle(seam, &selection);
                Err(CommandOutput::failure(
                    render_for(locale, "journal.cli.sol.link.mark.confirm.mismatch", &[]),
                    1,
                ))
            }
            MarkAnswer::Cancelled => {
                retire_bundle(seam, &selection);
                Err(CommandOutput::failure(
                    render_for(locale, "journal.cli.sol.link.mark.confirm.cancel", &[]),
                    1,
                ))
            }
            MarkAnswer::WalkedAway => {
                return CommandOutput::failure(held_lines(locale, &selection.label), 5);
            }
        }
    };
    if let Err(refused) = answer {
        return match fs::remove_dir_all(&selection.bundle_dir) {
            Ok(()) => refused,
            Err(error) => CommandOutput::failure(format!("{error}\n"), 1),
        };
    }
    match store_link_answer(&selection.bundle_dir, ca_jid.as_deref()) {
        Ok(()) => CommandOutput::success(render_for(
            locale,
            "journal.cli.sol.link.confirm.success",
            &[],
        )),
        Err(error) => CommandOutput::failure(format!("{error}\n"), 1),
    }
}

/// Best effort, like a refused join: the journal's row for this device goes.
fn retire_bundle(seam: &dyn crate::seam::LinkJoinPairingSeam, selection: &ServeBundleSelection) {
    let bundle = &selection.bundle;
    let relay = match &bundle.relay_access {
        Some(crate::link_credentials::StoreLoadOutcome::Ready(record)) => Some(record),
        _ => None,
    };
    let ca_fp_prefix = bundle
        .ca_chain_pem
        .first()
        .and_then(|pem| crate::link_credentials::pem_cert_der(pem))
        .map(|der| spl_core::ca::sha256(&der)[..16].to_vec())
        .unwrap_or_default();
    seam.retire(&LinkJoinCredential {
        client_key_pem: bundle.private_key_pem.clone(),
        client_cert_pem: bundle.client_cert_pem.clone(),
        ca_chain_pem: bundle.ca_chain_pem.clone(),
        ca_fingerprint: String::new(),
        instance_id: bundle.instance_id.clone(),
        home_label: bundle.home_label.clone(),
        home_attestation: Some(bundle.home_attestation.clone()),
        local_endpoints: bundle.local_endpoints.clone(),
        relay_origin: relay.and_then(|record| record.relay_origin.clone()),
        relay_device_token: relay.and_then(|record| record.device_token.clone()),
        relay_device_token_expires_at: relay.and_then(|record| record.expires_at),
        ca_fp_prefix,
        endpoints: bundle.endpoints.clone(),
    });
}

/// The owner went ahead: the id the link stores becomes the CA's, or, with no CA
/// to read, the link notes that they went on without one.
fn store_link_answer(bundle_dir: &Path, ca_jid: Option<&str>) -> io::Result<()> {
    let Some(jid) = ca_jid else {
        return rewrite_bundle_json(bundle_dir, "peer.json", |peer| {
            if let Some(fields) = peer.as_object_mut() {
                fields.insert(UNVERIFIED_KEY.to_owned(), Value::Bool(true));
            }
        });
    };
    rewrite_bundle_json(bundle_dir, "peer.json", |peer| {
        if let Some(slot) = peer.pointer_mut("/instance_id") {
            *slot = Value::String(jid.to_owned());
        }
    })?;
    rewrite_bundle_json(bundle_dir, "relay_access.json", |record| {
        if let Some(slot) = record.pointer_mut("/identity/instance_id") {
            *slot = Value::String(jid.to_owned());
        }
    })
}

fn rewrite_bundle_json(
    bundle_dir: &Path,
    name: &str,
    edit: impl FnOnce(&mut Value),
) -> io::Result<()> {
    let path = bundle_dir.join(name);
    let text = match fs::read_to_string(&path) {
        Ok(text) => text,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error),
    };
    let mut value: Value = serde_json::from_str(&text).map_err(io::Error::other)?;
    edit(&mut value);
    let staged = bundle_dir.join(format!(".{name}.confirm"));
    write_private_file(&staged, json_pretty_ascii(&value).as_bytes())?;
    fs::rename(&staged, &path)
}

pub fn link_serve(ctx: CommandContext<'_>) -> Result<ResidentCommand<'_>, CommandOutput> {
    let locale = ctx.human_locale_with_options(&["--label", "--port", "--relay-url"]);
    let parsed = match parse_serve_args(locale, ctx.args) {
        Ok(parsed) => parsed,
        Err(error) => return Err(argparse_serve_error(locale, error)),
    };
    if parsed.help {
        return Err(CommandOutput::success(render_for(
            locale,
            "journal.cli.sol.link.serve.help",
            &[],
        )));
    }
    if let Some(unknown) = parsed.unknown {
        return Err(argparse_serve_error(
            locale,
            display_for(
                locale,
                "journal.cli.sol.link.argument.unrecognized",
                [("unknown", &unknown)],
            ),
        ));
    }
    if parsed.direct && parsed.relay_only {
        return Err(argparse_serve_error(
            locale,
            render_for(locale, "journal.cli.sol.link.serve.policy.conflict", &[]),
        ));
    }
    let port = match parsed.port {
        Some(port) => port,
        None => DEFAULT_SERVE_PORT,
    };
    let selection = match resolve_serve_bundle(locale, parsed.label.as_deref(), ctx.env) {
        Ok(selection) => selection,
        Err(error) => return Err(CommandOutput::failure(format!("{error}\n"), 1)),
    };
    let ca_jid = pinned_ca_jid(&ctx, &selection.bundle.ca_chain_pem);
    if bundle_is_held(&selection, ca_jid.as_deref()) {
        return Err(CommandOutput::failure(
            held_lines(locale, &selection.label),
            5,
        ));
    }
    let policy = if parsed.direct {
        LinkServeCarrierPolicy::Direct
    } else if parsed.relay_only {
        LinkServeCarrierPolicy::RelayOnly
    } else {
        LinkServeCarrierPolicy::RelayPermitted
    };
    let explicit_relay_url = parsed
        .relay_url
        .as_deref()
        .filter(|v| !v.trim().is_empty())
        .or_else(|| {
            ctx.env
                .get("SOL_LINK_RELAY_URL")
                .map(String::as_str)
                .filter(|v| !v.trim().is_empty())
        });
    let relay_origin = if parsed.direct {
        None
    } else {
        explicit_relay_url.map(|explicit| {
            crate::link_credentials::parse_relay_origin(explicit)
                .unwrap_or_else(|_| explicit.trim().trim_end_matches('/').to_string())
        })
    };
    let Some(runner) = ctx.link_serve else {
        return Err(CommandOutput::failure(
            render_for(locale, "journal.cli.sol.link.serve.unavailable", &[]),
            1,
        ));
    };
    // The mark comes from the CA this link pins. With none to read, it names none.
    let words = match ca_jid.as_deref().map(mark_words_from_jid) {
        Some(Ok(words)) if !words.is_empty() => Some(words),
        Some(_) => {
            return Err(CommandOutput::failure(
                render_for(locale, "journal.cli.sol.link.serve.mark.unnamed", &[]),
                1,
            ));
        }
        None => None,
    };
    let request = LinkServeRequest {
        label: selection.label.clone(),
        port,
        policy,
        relay_origin,
        bundle: selection.bundle,
        bundle_dir: selection.bundle_dir,
    };
    let session = match runner.start(request) {
        Ok(session) => session,
        Err(error) => {
            return Err(CommandOutput::failure(
                format!("{}\n", serve_error_text(locale, error)),
                1,
            ));
        }
    };
    let startup = match (policy, words.as_deref()) {
        (LinkServeCarrierPolicy::Direct, Some(mark)) => display_for(
            locale,
            "journal.cli.sol.link.serve.forward.direct",
            [("port", &session.bound_port()), ("mark", &mark)],
        ),
        (LinkServeCarrierPolicy::Direct, None) => display_for(
            locale,
            "journal.cli.sol.link.serve.forward.direct.unnamed",
            [("port", &session.bound_port())],
        ),
        (LinkServeCarrierPolicy::RelayPermitted, Some(mark)) => display_for(
            locale,
            "journal.cli.sol.link.serve.forward.either",
            [("port", &session.bound_port()), ("mark", &mark)],
        ),
        (LinkServeCarrierPolicy::RelayPermitted, None) => display_for(
            locale,
            "journal.cli.sol.link.serve.forward.either.unnamed",
            [("port", &session.bound_port())],
        ),
        (LinkServeCarrierPolicy::RelayOnly, Some(mark)) => display_for(
            locale,
            "journal.cli.sol.link.serve.forward.relay",
            [("port", &session.bound_port()), ("mark", &mark)],
        ),
        (LinkServeCarrierPolicy::RelayOnly, None) => display_for(
            locale,
            "journal.cli.sol.link.serve.forward.relay.unnamed",
            [("port", &session.bound_port())],
        ),
    };
    Ok(ResidentCommand::new(startup, move |shutdown| {
        eprintln!(
            "{}",
            render_for(locale, "journal.cli.sol.link.serve.warning", &[])
        );
        match session.serve(shutdown) {
            Ok(()) => CommandOutput::success(""),
            Err(error) => {
                CommandOutput::failure(format!("{}\n", serve_error_text(locale, error)), 1)
            }
        }
    }))
}

pub fn link_status(ctx: CommandContext<'_>) -> CommandOutput {
    let locale = ctx.human_locale_with_options(&["--label"]);
    let parsed = match parse_status_args(locale, ctx.args) {
        Ok(parsed) => parsed,
        Err(error) => return argparse_status_error(locale, error),
    };
    if parsed.help {
        return CommandOutput::success(render_for(locale, "journal.cli.sol.link.status.help", &[]));
    }
    if let Some(unknown) = parsed.unknown {
        return argparse_status_error(
            locale,
            display_for(
                locale,
                "journal.cli.sol.link.argument.unrecognized",
                [("unknown", &unknown)],
            ),
        );
    }

    let selection = match resolve_serve_bundle(locale, parsed.label.as_deref(), ctx.env) {
        Ok(selection) => selection,
        Err(error) => {
            return CommandOutput::failure(
                display_for(
                    locale,
                    "journal.cli.sol.link.status.error",
                    [("error", &error)],
                ),
                1,
            );
        }
    };

    // Derived from the selected bundle's own certificate chain, never from a
    // cache: a cache can predate a deleted-and-recreated bundle (including a
    // same-identity re-pair that keeps the same CA), so it must never be the
    // source of truth for what identity is expected here.
    let expected_ca_fp_prefix = ca_fp_prefix_hex(&selection.bundle.ca_chain_pem);

    let mut live_status: Option<LinkServeStatusSnapshot> = None;
    let runtime_path = selection.bundle_dir.join("serve_runtime.json");
    if let (Ok(content), Some(probe)) = (fs::read_to_string(&runtime_path), ctx.link_status_probe) {
        live_status = serde_json::from_str::<LinkServeRuntimeRecord>(&content)
            .ok()
            .and_then(|rec| probe.probe(rec.port).ok())
            .filter(|resp| resp.status == 200)
            .and_then(|resp| serde_json::from_slice::<LinkServeStatusSnapshot>(&resp.body).ok())
            .filter(|snap| {
                snap.instance_id == selection.bundle.instance_id
                    && snap.paired_at == selection.bundle.paired_at
                    && expected_ca_fp_prefix.as_deref() == Some(snap.ca_fp_prefix.as_str())
            });
    }

    let mut output = display_for(
        locale,
        "journal.cli.sol.link.status.label",
        [("label", &selection.label)],
    );
    match &live_status {
        Some(snap) => output.push_str(&display_for(
            locale,
            "journal.cli.sol.link.status.live",
            [("state", &snap.state)],
        )),
        None => output.push_str(&render_for(
            locale,
            "journal.cli.sol.link.status.stopped",
            &[],
        )),
    }
    let version = live_status
        .as_ref()
        .and_then(|snap| {
            snap.journal_version
                .as_ref()
                .map(|raw| (sanitize_display_version(raw), snap.journal_version_fresh))
        })
        .or_else(|| {
            read_cached_version_fallback(&selection, expected_ca_fp_prefix.as_deref())
                .map(|version| (version, false))
        });
    output.push_str(&match version {
        Some((version, true)) => display_for(
            locale,
            "journal.cli.sol.link.status.version.fresh",
            [("version", &version)],
        ),
        Some((version, false)) => display_for(
            locale,
            "journal.cli.sol.link.status.version.last",
            [("version", &version)],
        ),
        None => render_for(locale, "journal.cli.sol.link.status.version.unknown", &[]),
    });
    if let Some(snap) = &live_status {
        if snap.persist_uncertain {
            output.push_str(&render_for(
                locale,
                "journal.cli.sol.link.status.persist.warning",
                &[],
            ));
        }
        if snap.state == "unpaired" {
            output.push_str(&unpaired_note(locale, &selection.bundle_dir));
        }
    }
    let ca_jid = pinned_ca_jid(&ctx, &selection.bundle.ca_chain_pem);
    if bundle_is_held(&selection, ca_jid.as_deref()) {
        output.push_str(&held_lines(locale, &selection.label));
    }
    if let Some(snap) = &live_status {
        for sighting in &snap.unknown_journals {
            output.push_str(&format_sighting_block(
                locale,
                sighting,
                ca_jid.as_deref().unwrap_or_default(),
            ));
        }
    }
    CommandOutput::success(output)
}

/// The journal id of the CA a stored chain pins, checked the way `serve` checks it.
fn pinned_ca_jid(ctx: &CommandContext<'_>, ca_chain_pem: &[String]) -> Option<String> {
    match ctx.link_serve {
        Some(runner) => runner.pinned_ca_jid(ca_chain_pem),
        None => crate::seam::head_ca_jid(ca_chain_pem),
    }
}

fn mark_words_from_jid(jid: &str) -> Result<String, spl_core::mark::MarkError> {
    crate::mark_text::mark_words_from_jid(jid)
}

fn format_spoken_mark(spec: &spl_core::mark::MarkRenderSpec) -> String {
    crate::mark_text::spoken_mark(spec)
}

fn format_sighting_block(
    locale: &str,
    sighting: &crate::seam::UnknownJournalSighting,
    local_instance_id: &str,
) -> String {
    let header = match &sighting.address {
        Some(addr) => display_for(
            locale,
            "journal.cli.sol.link.sighting.direct",
            [("addr", &addr)],
        ),
        None => render_for(locale, "journal.cli.sol.link.sighting.relay", &[]),
    };
    let what_answered = match &sighting.jid {
        Some(jid) => match crate::mark_text::mark_from_jid(jid) {
            Ok(mark) => {
                let spoken = format_spoken_mark(&mark.to_render_spec());
                display_for(
                    locale,
                    "journal.cli.sol.link.sighting.claimed",
                    [("spoken", &spoken)],
                )
            }
            Err(_) => render_for(locale, "journal.cli.sol.link.sighting.claim.unshown", &[]),
        },
        None => render_for(locale, "journal.cli.sol.link.sighting.no.identity", &[]),
    };
    let your_journal = match crate::mark_text::mark_from_jid(local_instance_id) {
        Ok(mark) => {
            let spoken = format_spoken_mark(&mark.to_render_spec());
            display_for(
                locale,
                "journal.cli.sol.link.sighting.journal",
                [("spoken", &spoken)],
            )
        }
        Err(_) => render_for(locale, "journal.cli.client.network.mark.unavailable", &[]),
    };
    let footer = match (&sighting.jid, &sighting.address) {
        (None, _) => String::new(),
        (Some(_), Some(_)) => render_for(locale, "journal.cli.sol.link.sighting.reset", &[]),
        (Some(_), None) => render_for(locale, "journal.cli.sol.link.sighting.certainty", &[]),
    };
    format!("\n{header}\n\n{what_answered}\n{your_journal}{footer}\n")
}

fn unpaired_note(locale: &str, bundle_dir: &Path) -> String {
    display_for(
        locale,
        "journal.cli.sol.link.unpaired.instructions",
        [("path", &bundle_dir.display())],
    )
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
struct ParsedArgs {
    home: Option<String>,
    code: Option<String>,
    as_role: Option<String>,
    label: Option<String>,
    mark: Option<String>,
    mark_repeated: bool,
    help: bool,
    unknown: Option<String>,
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
struct ParsedServeArgs {
    label: Option<String>,
    port: Option<u16>,
    relay_url: Option<String>,
    direct: bool,
    relay_only: bool,
    help: bool,
    unknown: Option<String>,
}

struct ServeBundleSelection {
    label: String,
    bundle: LinkServeBundle,
    bundle_dir: PathBuf,
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
struct ParsedStatusArgs {
    help: bool,
    label: Option<String>,
    unknown: Option<String>,
}

fn parse_status_args(locale: &str, args: &[String]) -> Result<ParsedStatusArgs, String> {
    let mut parsed = ParsedStatusArgs::default();
    let mut index = 0;
    while index < args.len() {
        let token = &args[index];
        if token == "-h" || token == "--help" {
            parsed.help = true;
            return Ok(parsed);
        } else if let Some(value) = token.strip_prefix("--label=") {
            parsed.label = Some(value.to_string());
        } else if token == "--label" {
            index += 1;
            parsed.label = Some(take_value(locale, args, index, "--label")?.to_string());
        } else if parsed.unknown.is_none() {
            parsed.unknown = Some(token.clone());
        }
        index += 1;
    }
    Ok(parsed)
}

fn argparse_status_error(locale: &str, error: String) -> CommandOutput {
    CommandOutput::failure(
        display_for(
            locale,
            "journal.cli.sol.link.status.usage.error",
            [("error", &error)],
        ),
        2,
    )
}

fn sanitize_display_version(raw: &str) -> String {
    raw.chars()
        .filter(|c| !c.is_control() && *c != '\x1b')
        .collect()
}

fn read_cached_version_fallback(
    selection: &ServeBundleSelection,
    expected_ca_fp_prefix: Option<&str>,
) -> Option<String> {
    let meta_path = selection.bundle_dir.join("journal_metadata.json");
    fs::read_to_string(&meta_path)
        .ok()
        .and_then(|content| serde_json::from_str::<LinkJournalMetadata>(&content).ok())
        .filter(|meta| {
            meta.instance_id == selection.bundle.instance_id
                && meta.paired_at == selection.bundle.paired_at
                && expected_ca_fp_prefix.is_some_and(|ca| meta.ca_fp_prefix == ca)
                && !meta.journal_version.is_empty()
        })
        .map(|meta| sanitize_display_version(&meta.journal_version))
}

/// Computes the same CA-fingerprint prefix pinned at pairing time (see
/// `solstone-core-sol-link`'s `ca_fp_prefix`: SHA-256 of the first
/// certificate's DER, hex-encoded, truncated to the pinned 16-byte prefix),
/// applied here to the selected bundle's own chain rather than to any cached
/// or peer-reported value.
fn ca_fp_prefix_hex(ca_chain_pem: &[String]) -> Option<String> {
    let der = crate::link_credentials::pem_cert_der(ca_chain_pem.first()?)?;
    Some(spl_core::ca::sha256_hex(&der)[..32].to_string())
}

fn parse_args(locale: &str, args: &[String]) -> Result<ParsedArgs, String> {
    let mut parsed = ParsedArgs::default();
    let mut index = 0;
    while index < args.len() {
        let token = &args[index];
        if token == "-h" || token == "--help" {
            parsed.help = true;
        } else if let Some(value) = token.strip_prefix("--home=") {
            parsed.home = Some(value.to_string());
        } else if token == "--home" {
            index += 1;
            parsed.home = Some(take_value(locale, args, index, "--home")?.to_string());
        } else if let Some(value) = token.strip_prefix("--code=") {
            parsed.code = Some(value.to_string());
        } else if token == "--code" {
            index += 1;
            parsed.code = Some(take_value(locale, args, index, "--code")?.to_string());
        } else if let Some(value) = token.strip_prefix("--as=") {
            parsed.as_role = Some(value.to_string());
        } else if token == "--as" {
            index += 1;
            parsed.as_role = Some(take_value(locale, args, index, "--as")?.to_string());
        } else if let Some(value) = token.strip_prefix("--label=") {
            parsed.label = Some(value.to_string());
        } else if token == "--label" {
            index += 1;
            parsed.label = Some(take_value(locale, args, index, "--label")?.to_string());
        } else if let Some(value) = token.strip_prefix("--mark=") {
            parsed.mark_repeated |= parsed.mark.is_some();
            parsed.mark = Some(value.to_string());
        } else if token == "--mark" {
            index += 1;
            parsed.mark_repeated |= parsed.mark.is_some();
            // A missing or flag-shaped value gets the mark's own usage line.
            parsed.mark = Some(
                args.get(index)
                    .filter(|value| !value.starts_with('-'))
                    .cloned()
                    .unwrap_or_default(),
            );
        } else if parsed.unknown.is_none() {
            parsed.unknown = Some(token.clone());
        }
        index += 1;
    }
    Ok(parsed)
}

fn parse_confirm_args(locale: &str, args: &[String]) -> Result<ParsedArgs, String> {
    let mut parsed = ParsedArgs::default();
    let mut index = 0;
    while index < args.len() {
        let token = &args[index];
        if token == "-h" || token == "--help" {
            parsed.help = true;
        } else if let Some(value) = token.strip_prefix("--label=") {
            parsed.label = Some(value.to_string());
        } else if token == "--label" {
            index += 1;
            parsed.label = Some(take_value(locale, args, index, "--label")?.to_string());
        } else if let Some(value) = token.strip_prefix("--mark=") {
            parsed.mark_repeated |= parsed.mark.is_some();
            parsed.mark = Some(value.to_string());
        } else if token == "--mark" {
            index += 1;
            parsed.mark_repeated |= parsed.mark.is_some();
            parsed.mark = Some(
                args.get(index)
                    .filter(|value| !value.starts_with('-'))
                    .cloned()
                    .unwrap_or_default(),
            );
        } else if parsed.unknown.is_none() {
            parsed.unknown = Some(token.clone());
        }
        index += 1;
    }
    Ok(parsed)
}

fn confirm_usage_error(locale: &str, error: &str) -> CommandOutput {
    CommandOutput::failure(
        display_for(
            locale,
            "journal.cli.sol.link.confirm.usage.error",
            [("error", &error)],
        ),
        2,
    )
}

fn parse_serve_args(locale: &str, args: &[String]) -> Result<ParsedServeArgs, String> {
    let mut parsed = ParsedServeArgs::default();
    let mut index = 0;
    while index < args.len() {
        let token = &args[index];
        if token == "-h" || token == "--help" {
            parsed.help = true;
        } else if let Some(value) = token.strip_prefix("--label=") {
            parsed.label = Some(value.to_string());
        } else if token == "--label" {
            index += 1;
            parsed.label = Some(take_value(locale, args, index, "--label")?.to_string());
        } else if let Some(value) = token.strip_prefix("--port=") {
            parsed.port = Some(parse_serve_port(locale, value)?);
        } else if token == "--port" {
            index += 1;
            parsed.port = Some(parse_serve_port(
                locale,
                take_raw_value(locale, args, index, "--port")?,
            )?);
        } else if let Some(value) = token.strip_prefix("--relay-url=") {
            parsed.relay_url = Some(value.to_string());
        } else if token == "--relay-url" {
            index += 1;
            parsed.relay_url = Some(take_value(locale, args, index, "--relay-url")?.to_string());
        } else if token == "--direct" {
            parsed.direct = true;
        } else if token == "--relay-only" {
            parsed.relay_only = true;
        } else if parsed.unknown.is_none() {
            parsed.unknown = Some(token.clone());
        }
        index += 1;
    }
    Ok(parsed)
}

fn parse_serve_port(locale: &str, value: &str) -> Result<u16, String> {
    let parsed = value.parse::<i64>().map_err(|_| {
        display_for(
            locale,
            "journal.cli.sol.link.serve.port.invalid",
            [("value", &value)],
        )
    })?;
    match parsed {
        0 => Ok(0),
        1..=65535 => u16::try_from(parsed)
            .map_err(|_| render_for(locale, "journal.cli.sol.link.serve.port.range", &[])),
        _ => Err(render_for(
            locale,
            "journal.cli.sol.link.serve.port.range",
            &[],
        )),
    }
}

fn take_value<'a>(
    locale: &str,
    args: &'a [String],
    index: usize,
    option: &str,
) -> Result<&'a str, String> {
    let Some(value) = args.get(index).map(String::as_str) else {
        return Err(display_for(
            locale,
            "journal.cli.sol.link.argument.expected",
            [("option", &option)],
        ));
    };
    if value.starts_with('-') {
        return Err(display_for(
            locale,
            "journal.cli.sol.link.argument.expected",
            [("option", &option)],
        ));
    }
    Ok(value)
}

fn take_raw_value<'a>(
    locale: &str,
    args: &'a [String],
    index: usize,
    option: &str,
) -> Result<&'a str, String> {
    let Some(value) = args.get(index).map(String::as_str) else {
        return Err(display_for(
            locale,
            "journal.cli.sol.link.argument.expected",
            [("option", &option)],
        ));
    };
    Ok(value)
}

fn argparse_serve_error(locale: &str, error: String) -> CommandOutput {
    CommandOutput::failure(
        display_for(
            locale,
            "journal.cli.sol.link.serve.usage.error",
            [("error", &error)],
        ),
        2,
    )
}

fn resolve_serve_bundle(
    locale: &str,
    label: Option<&str>,
    env: &BTreeMap<String, String>,
) -> Result<ServeBundleSelection, String> {
    if let Some(label) = label {
        let bundle_dir = observer_bundle_dir(locale, label, env)?;
        let bundle = load_serve_bundle(locale, &bundle_dir).map_err(|error| {
            display_for(
                locale,
                "journal.cli.sol.link.bundle.invalid",
                [
                    ("label", &label),
                    ("path", &bundle_dir.display()),
                    ("error", &error),
                ],
            )
        })?;
        return Ok(ServeBundleSelection {
            label: label.to_string(),
            bundle,
            bundle_dir,
        });
    }

    let root = observer_spl_root(locale, env)?;
    let mut bundles = BTreeMap::new();
    if root.is_dir() {
        for entry in fs::read_dir(&root).map_err(|error| error.to_string())? {
            let entry = entry.map_err(|error| error.to_string())?;
            let path = entry.path();
            if !path.is_dir() {
                continue;
            }
            let label = entry.file_name().to_string_lossy().to_string();
            if let Ok(bundle) = load_serve_bundle(locale, &path) {
                bundles.insert(label, (bundle, path));
            }
        }
    }
    if bundles.is_empty() {
        return Err(display_for(
            locale,
            "journal.cli.sol.link.bundle.none",
            [("path", &root.display())],
        ));
    }
    if bundles.len() > 1 {
        let labels = bundles.keys().cloned().collect::<Vec<_>>().join(", ");
        return Err(display_for(
            locale,
            "journal.cli.sol.link.bundle.multiple",
            [("labels", &labels)],
        ));
    }
    let (label, (bundle, bundle_dir)) = bundles.into_iter().next().expect("bundle count checked");
    Ok(ServeBundleSelection {
        label,
        bundle,
        bundle_dir,
    })
}

fn observer_spl_root(locale: &str, env: &BTreeMap<String, String>) -> Result<PathBuf, String> {
    let base = env
        .get("XDG_CONFIG_HOME")
        .filter(|value| !value.is_empty())
        .map_or_else(
            || {
                env.get("HOME")
                    .filter(|value| !value.is_empty())
                    // A Windows console sets USERPROFILE, not HOME.
                    .or_else(|| {
                        env.get("USERPROFILE")
                            .filter(|value| cfg!(windows) && !value.is_empty())
                    })
                    .map(|home| PathBuf::from(home).join(".config"))
            },
            |xdg| Some(PathBuf::from(xdg)),
        );
    let Some(base) = base else {
        return Err(render_for(
            locale,
            "journal.cli.sol.link.bundle.home.missing",
            &[],
        ));
    };
    Ok(base.join("solstone-observer").join("spl"))
}

fn load_serve_bundle(locale: &str, bundle_dir: &Path) -> Result<LinkServeBundle, String> {
    if !bundle_dir.is_dir() {
        return Err(display_for(
            locale,
            "journal.cli.sol.link.bundle.path.missing",
            [("path", &bundle_dir.display())],
        ));
    }
    let missing = BUNDLE_FILES
        .iter()
        .filter(|name| !bundle_dir.join(name).exists())
        .map(|name| bundle_dir.join(name).display().to_string())
        .collect::<Vec<_>>();
    if !missing.is_empty() {
        return Err(display_for(
            locale,
            "journal.cli.sol.link.bundle.files.missing",
            [("paths", &missing.join(", "))],
        ));
    }
    let private_key_pem = read_bundle_text(bundle_dir, "private.pem")?;
    let client_cert_pem = read_bundle_text(bundle_dir, "cert.pem")?;
    let chain_pem = read_bundle_text(bundle_dir, "chain.pem")?;
    let ca_chain_pem = split_pem_certificates(locale, &chain_pem)?;
    let home_attestation = read_bundle_text(bundle_dir, "home_attestation.jwt")?;
    let peer_text = read_bundle_text(bundle_dir, "peer.json")?;
    let peer: Value = serde_json::from_str(&peer_text).map_err(|error| {
        display_for(
            locale,
            "journal.cli.sol.link.bundle.peer.invalid",
            [("path", &bundle_dir.display()), ("error", &error)],
        )
    })?;
    let local_endpoints = match peer.get("local_endpoints") {
        Some(Value::Null) | None => Value::Array(Vec::new()),
        Some(Value::Array(_)) => peer
            .get("local_endpoints")
            .cloned()
            .expect("local_endpoints presence checked"),
        Some(_) => {
            return Err(render_for(
                locale,
                "journal.cli.sol.link.bundle.endpoints.list",
                &[],
            ));
        }
    };
    let endpoints = serve_endpoints_from_value(locale, &local_endpoints)?;
    let label = bundle_dir
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or_default();
    let store = crate::link_credentials::LinkCredentialStore::new(bundle_dir.to_path_buf(), label);
    let relay_access = Some(store.load_access());
    Ok(LinkServeBundle {
        private_key_pem,
        client_cert_pem,
        ca_chain_pem,
        home_attestation,
        instance_id: peer
            .get("instance_id")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
        home_label: peer
            .get("home_label")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
        paired_at: peer
            .get("paired_at")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
        endpoints,
        local_endpoints,
        relay_access,
    })
}

fn read_bundle_text(bundle_dir: &Path, name: &str) -> Result<String, String> {
    fs::read_to_string(bundle_dir.join(name)).map_err(|error| error.to_string())
}

fn split_pem_certificates(locale: &str, chain_pem: &str) -> Result<Vec<String>, String> {
    const BEGIN: &str = "-----BEGIN CERTIFICATE-----";
    const END: &str = "-----END CERTIFICATE-----";
    let mut certs = Vec::new();
    let mut rest = chain_pem;
    while let Some(begin) = rest.find(BEGIN) {
        let after_begin = &rest[begin..];
        let Some(end) = after_begin.find(END) else {
            return Err(render_for(
                locale,
                "journal.cli.sol.link.bundle.chain.incomplete",
                &[],
            ));
        };
        let end_index = end + END.len();
        let mut cert = after_begin[..end_index].to_string();
        cert.push('\n');
        certs.push(cert);
        rest = &after_begin[end_index..];
    }
    if certs.is_empty() {
        return Err(render_for(
            locale,
            "journal.cli.sol.link.bundle.chain.empty",
            &[],
        ));
    }
    Ok(certs)
}

fn serve_endpoints_from_value(
    locale: &str,
    value: &Value,
) -> Result<Vec<LinkServeEndpoint>, String> {
    let Value::Array(items) = value else {
        return Err(render_for(
            locale,
            "journal.cli.sol.link.bundle.endpoints.list",
            &[],
        ));
    };
    let mut endpoints = Vec::new();
    for item in items {
        let Value::Object(map) = item else {
            return Err(render_for(
                locale,
                "journal.cli.sol.link.bundle.endpoints.objects",
                &[],
            ));
        };
        let host = map
            .get("ip")
            .or_else(|| map.get("host"))
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim()
            .to_string();
        if host.is_empty() {
            return Err(render_for(
                locale,
                "journal.cli.sol.link.bundle.endpoint.ip",
                &[],
            ));
        }
        let port = match map.get("port") {
            None | Some(Value::Null) => 7657,
            Some(Value::Number(number)) => number
                .as_u64()
                .and_then(|value| u16::try_from(value).ok())
                .ok_or_else(|| {
                    render_for(locale, "journal.cli.sol.link.bundle.endpoint.port", &[])
                })?,
            Some(Value::String(value)) => value.parse::<u16>().map_err(|_| {
                render_for(locale, "journal.cli.sol.link.bundle.endpoint.port", &[])
            })?,
            Some(_) => {
                return Err(render_for(
                    locale,
                    "journal.cli.sol.link.bundle.endpoint.port",
                    &[],
                ));
            }
        };
        endpoints.push(LinkServeEndpoint { host, port });
    }
    Ok(endpoints)
}

fn argparse_error(locale: &str, error: String) -> CommandOutput {
    CommandOutput::failure(
        display_for(
            locale,
            "journal.cli.sol.link.join.usage.error",
            [("error", &error)],
        ),
        2,
    )
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum PairRequest {
    Direct(DirectPairRequest),
    Relay(RelayPairRequest),
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct DirectPairRequest {
    targets: Vec<LinkJoinPairTarget>,
    nonce_hex: String,
    ca_fp_prefix: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct RelayPairRequest {
    relay_origin: String,
    secret: Vec<u8>,
    ca_fp_spki: Vec<u8>,
}

fn parse_pair_request(locale: &str, code: &str, home: Option<&str>) -> Result<PairRequest, String> {
    if !code.starts_with(PAIR_LINK_PREFIX) {
        return Err(render_for(
            locale,
            "journal.cli.sol.link.join.code.invalid",
            &[],
        ));
    }
    match spl_core::pairlink::parse(code) {
        Ok(ParsedPairLink::Direct(link)) => {
            let targets = if let Some(home) = home {
                vec![parse_home_target(locale, home)?]
            } else {
                link.candidates
                    .into_iter()
                    .map(|endpoint| LinkJoinPairTarget {
                        host: endpoint.host,
                        port: endpoint.port,
                    })
                    .collect()
            };
            Ok(PairRequest::Direct(DirectPairRequest {
                targets,
                nonce_hex: link.nonce_hex,
                ca_fp_prefix: link.ca_fp_prefix,
            }))
        }
        Ok(ParsedPairLink::Relay(link)) => Ok(PairRequest::Relay(RelayPairRequest {
            relay_origin: link.relay_origin,
            secret: link.s.to_vec(),
            ca_fp_spki: link.ca_fp_spki,
        })),
        Err(PairLinkError::DisallowedDirectIpv4 { address: _ }) => Err(render_for(
            locale,
            "journal.cli.sol.link.join.code.nonlocal",
            &[],
        )),
        Err(
            PairLinkError::MissingFragment
            | PairLinkError::Crockford(_)
            | PairLinkError::UnsupportedVersion(_)
            | PairLinkError::UnsupportedAddressType(_)
            | PairLinkError::UnknownCaFpTag(_)
            | PairLinkError::BadRelayOrigin
            | PairLinkError::Truncated { .. }
            | PairLinkError::LengthMismatch { .. }
            | PairLinkError::InvalidCandidateCount { .. },
        ) => Err(malformed_pair_link_message(locale)),
    }
}

fn malformed_pair_link_message(locale: &str) -> String {
    render_for(locale, "journal.cli.sol.link.join.code.malformed", &[])
}

fn parse_home_target(locale: &str, home: &str) -> Result<LinkJoinPairTarget, String> {
    let Some((_, rest)) = home.split_once("://") else {
        return Err(render_for(
            locale,
            "journal.cli.sol.link.join.target.host",
            &[],
        ));
    };
    let authority = rest
        .split(['/', '?', '#'])
        .next()
        .unwrap_or_default()
        .rsplit('@')
        .next()
        .unwrap_or_default();
    if authority.is_empty() {
        return Err(render_for(
            locale,
            "journal.cli.sol.link.join.target.host",
            &[],
        ));
    }
    if let Some(after_bracket) = authority
        .strip_prefix('[')
        .and_then(|value| value.split_once(']'))
    {
        let host = after_bracket.0;
        if host.is_empty() {
            return Err(render_for(
                locale,
                "journal.cli.sol.link.join.target.host",
                &[],
            ));
        }
        let port_text = after_bracket
            .1
            .strip_prefix(':')
            .ok_or_else(|| render_for(locale, "journal.cli.sol.link.join.target.port", &[]))?;
        let port = parse_explicit_port(locale, port_text)?;
        return Ok(LinkJoinPairTarget {
            host: host.to_string(),
            port,
        });
    }
    let Some((host, port_text)) = authority.rsplit_once(':') else {
        return Err(render_for(
            locale,
            "journal.cli.sol.link.join.target.port",
            &[],
        ));
    };
    if host.is_empty() {
        return Err(render_for(
            locale,
            "journal.cli.sol.link.join.target.host",
            &[],
        ));
    }
    let port = parse_explicit_port(locale, port_text)?;
    Ok(LinkJoinPairTarget {
        host: host.to_string(),
        port,
    })
}

fn parse_explicit_port(locale: &str, value: &str) -> Result<u16, String> {
    value
        .parse::<u16>()
        .map_err(|_| render_for(locale, "journal.cli.sol.link.join.target.port", &[]))
}

fn label_error(locale: &str, label: &str) -> Option<String> {
    if label.is_empty() {
        return Some(render_for(locale, "journal.cli.sol.link.label.empty", &[]));
    }
    if label.chars().count() > 80 {
        return Some(render_for(locale, "journal.cli.sol.link.label.length", &[]));
    }
    if label.contains('/') || label.contains('\\') {
        return Some(render_for(
            locale,
            "journal.cli.sol.link.label.separator",
            &[],
        ));
    }
    if label.contains("..") {
        return Some(render_for(
            locale,
            "journal.cli.sol.link.label.traversal",
            &[],
        ));
    }
    if label.starts_with('.') {
        return Some(render_for(locale, "journal.cli.sol.link.label.start", &[]));
    }
    if !label
        .chars()
        .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '_' | '.' | '-'))
    {
        return Some(render_for(
            locale,
            "journal.cli.sol.link.label.character",
            &[],
        ));
    }
    None
}

fn sanitize_client_label(raw: &str) -> String {
    if !raw
        .chars()
        .any(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '_' | '.' | '-'))
    {
        return String::new();
    }
    let mut label = String::new();
    let mut dot_run = 0usize;
    for ch in raw.chars() {
        let mapped = if ch.is_ascii_alphanumeric() || matches!(ch, '_' | '.' | '-') {
            ch
        } else {
            '-'
        };
        if mapped == '.' {
            dot_run += 1;
            if dot_run == 2 {
                label.pop();
                label.push('-');
            } else if dot_run < 2 {
                label.push(mapped);
            }
        } else {
            dot_run = 0;
            label.push(mapped);
        }
    }
    let trimmed = label
        .trim_start_matches('.')
        .chars()
        .take(80)
        .collect::<String>();
    if trimmed.is_empty() || label_error("en-US", &trimmed).is_some() {
        String::new()
    } else {
        trimmed
    }
}

fn hostname_client_label(env: &BTreeMap<String, String>) -> String {
    env.get("HOSTNAME")
        .or_else(|| env.get("COMPUTERNAME"))
        .map_or_else(String::new, |value| sanitize_client_label(value))
        .if_empty(DEFAULT_CLIENT_LABEL)
}

trait IfEmpty {
    fn if_empty(self, fallback: &str) -> String;
}

impl IfEmpty for String {
    fn if_empty(self, fallback: &str) -> String {
        if self.is_empty() {
            fallback.to_string()
        } else {
            self
        }
    }
}

fn observer_bundle_dir(
    locale: &str,
    label: &str,
    env: &BTreeMap<String, String>,
) -> Result<PathBuf, String> {
    Ok(observer_spl_root(locale, env)?.join(label))
}

fn validate_credential(locale: &str, credential: &LinkJoinCredential) -> Result<(), String> {
    if credential.client_key_pem.is_empty() {
        return Err(render_for(locale, "journal.cli.sol.link.response.key", &[]));
    }
    if credential.client_cert_pem.is_empty() {
        return Err(render_for(
            locale,
            "journal.cli.sol.link.response.cert",
            &[],
        ));
    }
    if credential.ca_chain_pem.is_empty() || credential.ca_chain_pem.iter().any(String::is_empty) {
        return Err(render_for(
            locale,
            "journal.cli.sol.link.response.chain",
            &[],
        ));
    }
    if credential.instance_id.is_empty() {
        return Err(render_for(
            locale,
            "journal.cli.sol.link.response.instance",
            &[],
        ));
    }
    if credential
        .home_attestation
        .as_deref()
        .unwrap_or_default()
        .is_empty()
    {
        return Err(render_for(
            locale,
            "journal.cli.sol.link.response.attestation",
            &[],
        ));
    }
    Ok(())
}

fn normalized_local_endpoints(locale: &str, value: &Value) -> Result<Value, String> {
    match value {
        Value::Null => Ok(Value::Array(Vec::new())),
        Value::Array(_) => Ok(value.clone()),
        Value::Bool(_) | Value::Number(_) | Value::String(_) | Value::Object(_) => Err(render_for(
            locale,
            "journal.cli.sol.link.response.endpoints",
            &[],
        )),
    }
}

fn join_chain(ca_chain_pem: &[String]) -> String {
    ca_chain_pem
        .iter()
        .map(|cert| {
            if cert.ends_with('\n') {
                cert.clone()
            } else {
                format!("{cert}\n")
            }
        })
        .collect()
}

fn peer_json(
    label: &str,
    paired_at: String,
    credential: &LinkJoinCredential,
    local_endpoints: Value,
) -> String {
    let mut peer = Map::new();
    peer.insert("label".to_string(), Value::String(label.to_string()));
    peer.insert("paired_at".to_string(), Value::String(paired_at));
    peer.insert(
        "instance_id".to_string(),
        Value::String(credential.instance_id.clone()),
    );
    peer.insert(
        "home_label".to_string(),
        Value::String(credential.home_label.clone()),
    );
    peer.insert(
        "fingerprint".to_string(),
        Value::String(credential.ca_fingerprint.clone()),
    );
    peer.insert("local_endpoints".to_string(), local_endpoints);
    peer.insert("role".to_string(), Value::String(String::new()));
    format!("{}\n", json_pretty_ascii(&Value::Object(peer)))
}

fn now_utc(clock: Option<&dyn crate::seam::Clock>) -> String {
    let now = clock.map_or_else(SystemTime::now, |clock| clock.now());
    let datetime = Timestamp::try_from(now).expect("the clock is within the supported range");
    datetime.strftime("%Y-%m-%dT%H:%M:%SZ").to_string()
}

fn pairing_error_text(locale: &str, error: LinkJoinPairingError) -> String {
    let kind = error.kind;
    match kind {
        LinkJoinPairingErrorKind::Rejected { status } => display_for(
            locale,
            "journal.cli.sol.link.pair.rejected",
            [("status", &status)],
        ),
        LinkJoinPairingErrorKind::Relay(LinkJoinRelayErrorKind::PairWindowClosed) => {
            render_for(locale, "journal.cli.sol.link.pair.closed", &[])
        }
        LinkJoinPairingErrorKind::PairResponseMissingHomeAttestation => {
            render_for(locale, "journal.cli.sol.link.response.attestation", &[])
        }
        LinkJoinPairingErrorKind::NoEndpoint => {
            render_for(locale, "journal.cli.sol.link.pair.listener", &[])
        }
        LinkJoinPairingErrorKind::RelayControlRejected { endpoint, status } => display_for(
            locale,
            "journal.cli.sol.link.pair.enrollment",
            [
                ("endpoint", &relay_control_endpoint_code(endpoint)),
                ("status", &status),
            ],
        ),
        kind @ (LinkJoinPairingErrorKind::Io
        | LinkJoinPairingErrorKind::Tls
        | LinkJoinPairingErrorKind::TlsAccessDenied
        | LinkJoinPairingErrorKind::TlsCertificateUnknown
        | LinkJoinPairingErrorKind::Crypto
        | LinkJoinPairingErrorKind::Mux
        | LinkJoinPairingErrorKind::Http
        | LinkJoinPairingErrorKind::Json
        | LinkJoinPairingErrorKind::PairLink
        | LinkJoinPairingErrorKind::Pairing
        | LinkJoinPairingErrorKind::Relay(
            LinkJoinRelayErrorKind::HomeOffline
            | LinkJoinRelayErrorKind::Unauthorized
            | LinkJoinRelayErrorKind::Unpaid
            | LinkJoinRelayErrorKind::UnknownInstance
            | LinkJoinRelayErrorKind::Overflow
            | LinkJoinRelayErrorKind::Abnormal
            | LinkJoinRelayErrorKind::UpgradeRejected
            | LinkJoinRelayErrorKind::Stalled,
        )
        | LinkJoinPairingErrorKind::NotPaired
        | LinkJoinPairingErrorKind::LocalOffset
        | LinkJoinPairingErrorKind::RuntimeUnavailable) => display_for(
            locale,
            "journal.cli.sol.link.pair.failed",
            [("code", &transport_error_code(kind))],
        ),
    }
}

fn serve_error_text(locale: &str, error: LinkServeError) -> String {
    match error.kind {
        LinkServeErrorKind::InvalidBundle => render_for(
            locale,
            "journal.cli.sol.link.serve.credentials.invalid",
            &[],
        ),
        LinkServeErrorKind::Bind { port, addr_in_use } => {
            if addr_in_use {
                display_for(
                    locale,
                    "journal.cli.sol.link.serve.bind.in.use",
                    [("port", &port)],
                )
            } else {
                display_for(
                    locale,
                    "journal.cli.sol.link.serve.bind.failed",
                    [("port", &port)],
                )
            }
        }
        LinkServeErrorKind::RuntimeUnavailable => render_for(
            locale,
            "journal.cli.sol.link.serve.runtime.unavailable",
            &[],
        ),
        LinkServeErrorKind::BridgeCapability => {
            render_for(locale, "journal.cli.sol.link.serve.bridge.failed", &[])
        }
        LinkServeErrorKind::Transport(kind) => serve_transport_error_text(locale, kind),
    }
}

fn serve_transport_error_text(locale: &str, kind: LinkServeTransportErrorKind) -> String {
    match kind {
        LinkServeTransportErrorKind::Io => {
            render_for(locale, "journal.cli.sol.link.transport.io", &[])
        }
        LinkServeTransportErrorKind::Tls
        | LinkServeTransportErrorKind::TlsAccessDenied
        | LinkServeTransportErrorKind::TlsCertificateUnknown => {
            render_for(locale, "journal.cli.sol.link.transport.tls", &[])
        }
        LinkServeTransportErrorKind::Crypto => {
            render_for(locale, "journal.cli.sol.link.transport.credential", &[])
        }
        LinkServeTransportErrorKind::Mux => {
            render_for(locale, "journal.cli.sol.link.transport.framing", &[])
        }
        LinkServeTransportErrorKind::Http => {
            render_for(locale, "journal.cli.sol.link.transport.http", &[])
        }
        LinkServeTransportErrorKind::Json => {
            render_for(locale, "journal.cli.sol.link.transport.json", &[])
        }
        LinkServeTransportErrorKind::PairLink => {
            render_for(locale, "journal.cli.sol.link.transport.pair.data", &[])
        }
        LinkServeTransportErrorKind::Pairing => {
            render_for(locale, "journal.cli.sol.link.transport.enrollment", &[])
        }
        LinkServeTransportErrorKind::Rejected { status } => display_for(
            locale,
            "journal.cli.sol.link.transport.rejected",
            [("status", &status)],
        ),
        LinkServeTransportErrorKind::Relay(error) => serve_relay_error_text(locale, error),
        LinkServeTransportErrorKind::RelayControlRejected { endpoint, status } => match endpoint {
            crate::seam::LinkServeRelayControlEndpoint::EnrollDevice => display_for(
                locale,
                "journal.cli.sol.link.transport.enroll.rejected",
                [("status", &status)],
            ),
            crate::seam::LinkServeRelayControlEndpoint::TokenRefresh => display_for(
                locale,
                "journal.cli.sol.link.transport.refresh.rejected",
                [("status", &status)],
            ),
        },
        LinkServeTransportErrorKind::NoEndpoint => {
            render_for(locale, "journal.cli.sol.link.transport.no.endpoint", &[])
        }
        LinkServeTransportErrorKind::NotPaired => {
            render_for(locale, "journal.cli.sol.link.transport.not.paired", &[])
        }
        LinkServeTransportErrorKind::LocalOffset => {
            render_for(locale, "journal.cli.sol.link.transport.offset", &[])
        }
    }
}

fn serve_relay_error_text(locale: &str, error: crate::seam::LinkServeRelayErrorKind) -> String {
    match error {
        crate::seam::LinkServeRelayErrorKind::HomeOffline => {
            render_for(locale, "journal.cli.sol.link.relay.offline", &[])
        }
        crate::seam::LinkServeRelayErrorKind::Unauthorized => {
            render_for(locale, "journal.cli.sol.link.relay.unauthorized", &[])
        }
        crate::seam::LinkServeRelayErrorKind::Unpaid => {
            render_for(locale, "journal.cli.sol.link.relay.unpaid", &[])
        }
        crate::seam::LinkServeRelayErrorKind::UnknownInstance => {
            render_for(locale, "journal.cli.sol.link.relay.unknown", &[])
        }
        crate::seam::LinkServeRelayErrorKind::PairWindowClosed => {
            render_for(locale, "journal.cli.sol.link.relay.closed", &[])
        }
        crate::seam::LinkServeRelayErrorKind::Overflow => {
            render_for(locale, "journal.cli.sol.link.relay.overflow", &[])
        }
        crate::seam::LinkServeRelayErrorKind::Abnormal => {
            render_for(locale, "journal.cli.sol.link.relay.abnormal", &[])
        }
        crate::seam::LinkServeRelayErrorKind::UpgradeRejected => {
            render_for(locale, "journal.cli.sol.link.relay.upgrade", &[])
        }
        crate::seam::LinkServeRelayErrorKind::Stalled => {
            render_for(locale, "journal.cli.sol.link.relay.stalled", &[])
        }
    }
}

fn relay_control_endpoint_code(endpoint: LinkJoinRelayControlEndpoint) -> &'static str {
    match endpoint {
        LinkJoinRelayControlEndpoint::EnrollDevice => "relay-control-enroll-device",
        LinkJoinRelayControlEndpoint::TokenRefresh => "relay-control-token-refresh",
    }
}

fn transport_error_code(kind: LinkJoinPairingErrorKind) -> &'static str {
    match kind {
        LinkJoinPairingErrorKind::Io => "io",
        LinkJoinPairingErrorKind::Tls => "tls",
        LinkJoinPairingErrorKind::TlsAccessDenied => "tls-access-denied",
        LinkJoinPairingErrorKind::TlsCertificateUnknown => "tls-certificate-unknown",
        LinkJoinPairingErrorKind::Crypto => "crypto",
        LinkJoinPairingErrorKind::Mux => "mux",
        LinkJoinPairingErrorKind::Http => "http",
        LinkJoinPairingErrorKind::Json => "json",
        LinkJoinPairingErrorKind::PairLink => "pair-link",
        LinkJoinPairingErrorKind::Pairing => "pairing",
        LinkJoinPairingErrorKind::PairResponseMissingHomeAttestation => {
            "pair-response-missing-home-attestation"
        }
        LinkJoinPairingErrorKind::Rejected { status: _ } => "rejected",
        LinkJoinPairingErrorKind::Relay(LinkJoinRelayErrorKind::HomeOffline) => {
            "relay-home-offline"
        }
        LinkJoinPairingErrorKind::Relay(LinkJoinRelayErrorKind::Unauthorized) => {
            "relay-unauthorized"
        }
        LinkJoinPairingErrorKind::Relay(LinkJoinRelayErrorKind::Unpaid) => "relay-unpaid",
        LinkJoinPairingErrorKind::Relay(LinkJoinRelayErrorKind::UnknownInstance) => {
            "relay-unknown-instance"
        }
        LinkJoinPairingErrorKind::Relay(LinkJoinRelayErrorKind::PairWindowClosed) => {
            "relay-pair-window-closed"
        }
        LinkJoinPairingErrorKind::Relay(LinkJoinRelayErrorKind::Overflow) => "relay-overflow",
        LinkJoinPairingErrorKind::Relay(LinkJoinRelayErrorKind::Abnormal) => "relay-abnormal",
        LinkJoinPairingErrorKind::Relay(LinkJoinRelayErrorKind::UpgradeRejected) => {
            "relay-upgrade-rejected"
        }
        LinkJoinPairingErrorKind::Relay(LinkJoinRelayErrorKind::Stalled) => "relay-stalled",
        LinkJoinPairingErrorKind::RelayControlRejected {
            endpoint,
            status: _,
        } => relay_control_endpoint_code(endpoint),
        LinkJoinPairingErrorKind::NoEndpoint => "no-endpoint",
        LinkJoinPairingErrorKind::NotPaired => "not-paired",
        LinkJoinPairingErrorKind::LocalOffset => "local-offset",
        LinkJoinPairingErrorKind::RuntimeUnavailable => "runtime-unavailable",
    }
}

fn path_lexists(path: &Path) -> bool {
    fs::symlink_metadata(path).is_ok()
}

fn existing_path_message(locale: &str, path: &Path) -> String {
    display_for(
        locale,
        "journal.cli.sol.link.bundle.exists",
        [("path", &path.display())],
    )
}

fn spent_existing_path_message(locale: &str, path: &Path) -> String {
    display_for(
        locale,
        "journal.cli.sol.link.bundle.exists.spent",
        [("path", &path.display())],
    )
}

fn publish_bundle_atomic(
    locale: &str,
    bundle_dir: &Path,
    files: &BTreeMap<String, Vec<u8>>,
) -> io::Result<()> {
    publish_bundle_atomic_with_writer(locale, bundle_dir, files, write_private_file)
}

fn publish_bundle_atomic_with_writer<W>(
    locale: &str,
    bundle_dir: &Path,
    files: &BTreeMap<String, Vec<u8>>,
    write_file: W,
) -> io::Result<()>
where
    W: Fn(&Path, &[u8]) -> io::Result<()>,
{
    if BUNDLE_FILES.iter().any(|name| !files.contains_key(*name)) {
        return Err(io::Error::other(render_for(
            locale,
            "journal.cli.sol.link.bundle.files.incomplete",
            &[],
        )));
    }
    let parent = bundle_dir.parent().unwrap_or_else(|| Path::new("."));
    fs::create_dir_all(parent)?;
    if path_lexists(bundle_dir) {
        return Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            existing_path_message(locale, bundle_dir),
        ));
    }

    let staging = PrivateStagingDir::create(parent, bundle_dir)?;
    for (name, content) in files {
        write_file(&staging.path.join(name), content)?;
    }
    crate::link_credentials::sync_directory(&staging.path)?;
    publish_staging_dir(locale, &staging.path, bundle_dir)?;
    staging.disarm();
    if let Ok(directory) = File::open(parent) {
        let _ = directory.sync_all();
    }
    Ok(())
}

#[cfg(any(target_vendor = "apple", target_os = "linux"))]
fn publish_staging_dir(_locale: &str, staging: &Path, destination: &Path) -> io::Result<()> {
    rustix::fs::renameat_with(
        rustix::fs::CWD,
        staging,
        rustix::fs::CWD,
        destination,
        rustix::fs::RenameFlags::NOREPLACE,
    )?;
    Ok(())
}

#[cfg(not(any(target_vendor = "apple", target_os = "linux")))]
fn publish_staging_dir(locale: &str, staging: &Path, destination: &Path) -> io::Result<()> {
    if path_lexists(destination) {
        return Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            existing_path_message(locale, destination),
        ));
    }
    fs::rename(staging, destination)
}

fn write_private_file(path: &Path, content: &[u8]) -> io::Result<()> {
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    options.mode(0o600);
    let mut file = options.open(path)?;
    file.write_all(content)?;
    file.sync_all()
}

struct PrivateStagingDir {
    path: PathBuf,
    armed: bool,
}

impl PrivateStagingDir {
    fn create(parent: &Path, destination: &Path) -> io::Result<Self> {
        let stem = destination
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("credentials");
        for _ in 0..100 {
            let sequence = BUNDLE_STAGING_SEQUENCE.fetch_add(1, Ordering::Relaxed);
            let path = parent.join(format!(
                ".{stem}.staging.{}_{}.tmp",
                std::process::id(),
                sequence
            ));
            #[cfg(unix)]
            let mut builder = DirBuilder::new();
            #[cfg(not(unix))]
            let builder = DirBuilder::new();
            #[cfg(unix)]
            builder.mode(0o700);
            match builder.create(&path) {
                Ok(()) => return Ok(Self { path, armed: true }),
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
                Err(error) => return Err(error),
            }
        }
        Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            "could not allocate credential staging directory",
        ))
    }

    fn disarm(mut self) {
        self.armed = false;
    }
}

impl Drop for PrivateStagingDir {
    fn drop(&mut self) {
        if self.armed {
            let _ = fs::remove_dir_all(&self.path);
        }
    }
}

#[cfg(test)]
mod tests {
    const SIGHTING_RELAY_FOOTER: &str = "this is less certain than a direct connection.";
    const SIGHTING_RESET_FOOTER: &str =
        "if you recently reset your journal, pair this device again.";
    const MARK_UNVERIFIABLE: &str = "this computer couldn't work out the journal's mark, so the words you gave can't be matched. this computer isn't paired, and nothing it has kept went to that journal through this link. get a fresh pairing link from your journal and run solstone link join in a terminal to decide for yourself.";
    const MARK_MISMATCH: &str = "the mark words you gave don't match the journal's mark, so this computer isn't paired, and nothing it has kept went to that journal through this link. check the words, get a fresh pairing link from your journal and run solstone link join again.";
    const MARK_USAGE: &str =
        "give the two words of your journal's mark, like --mark \"bramble quokka\".";
    const JOIN_NO_TERMINAL: &str = "solstone link join can't ask you about your journal's mark here. run it in a terminal, or give the mark's two words with --mark \"word word\".";
    const CANCEL: &str = "pairing cancelled. nothing this computer has kept went to that journal through this link. get a fresh pairing link from your journal and run solstone link join again when you're ready.";
    const MISMATCH_BODY: &str = "you said this mark doesn't match the one your journal shows, so this computer isn't paired, and nothing it has kept went to that journal through this link. you may have pasted the wrong link, or something isn't right. get a fresh pairing link from your journal and run solstone link join again, or email support@solstone.app and we'll help.";
    const NOT_PAIRED: &str = "not paired.";
    const PAIRED: &str = "paired.";
    const ASK_UNAVAILABLE: &str = "type continue to pair anyway, or cancel to stop:";
    const COULDNT_VERIFY: &str = "couldn't verify.";
    const ASK_IDENTIFIED: &str = "does this match your journal? type yes or no:";
    const MARK_UNAVAILABLE: &str = "  your journal's mark: unavailable right now";
    const STATUS_USAGE: &str = "usage: solstone link status [-h] [--label LABEL]\n";
    const STATUS_HELP: &str = "usage: solstone link status [-h] [--label LABEL]\n\nShow link status and observed remote journal version.\n\noptions:\n  -h, --help     show this help message and exit\n  --label LABEL  Link bundle label (defaults to the only paired link)\n";
    const SERVE_USAGE: &str = "usage: solstone link serve [-h] [--label LABEL] [--port PORT]\n                      [--relay-url RELAY_URL] [--direct | --relay-only]\n";
    const SERVE_HELP: &str = "usage: solstone link serve [-h] [--label LABEL] [--port PORT]\n                      [--relay-url RELAY_URL] [--direct | --relay-only]\n\noptions:\n  -h, --help            show this help message and exit\n  --label LABEL         Link bundle label\n  --port PORT           Loopback port to serve on (default: 5015)\n  --relay-url RELAY_URL\n                        Override the relay URL\n  --direct              Direct only: reach the journal through a direct\n                        connection, never the relay. use when your journal is\n                        reachable directly (same network/vpn) to avoid relay\n                        dependency.\n  --relay-only          Relay only: reach the journal through the relay,\n                        never a direct connection. use when direct connections\n                        must not be attempted, even if your journal is reachable\n                        locally.\n";
    const USAGE: &str = "usage: solstone link join [-h] [--home HOME] --code CODE [--as AS_ROLE]\n                     [--label LABEL] [--mark MARK]\n";
    const HELP: &str = "usage: solstone link join [-h] [--home HOME] --code CODE [--as AS_ROLE]\n                     [--label LABEL] [--mark MARK]\n\noptions:\n  -h, --help     show this help message and exit\n  --home HOME    Receiver base URL\n  --code CODE    pair-link URL\n  --as AS_ROLE   Optional tag to join as\n  --label LABEL  Local credentials label (defaults to this machine's hostname)\n  --mark MARK    the two words of the mark your journal's network app shows.\n                 needed when there's no terminal to ask you on.\n";
    use std::cell::Cell;
    use std::collections::BTreeMap;
    use std::fs;
    use std::path::{Path, PathBuf};
    #[cfg(unix)]
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicU64, Ordering};

    use crate::resident::ShutdownSignal;
    use crate::seam::{
        ExpectedLinkJoinPairingCall, ExpectedLinkServeCall, ExpectedLinkServeSession, FakeClock,
        LinkServeRelayControlEndpoint, LinkServeRelayErrorKind, ScriptedHttpTransport,
        ScriptedLinkJoinPairingSeam, ScriptedLinkServeRunner,
    };
    use serde_json::json;
    use spl_core::crockford;

    use super::*;

    const OBSERVER_PEER_JSON: &str =
        include_str!("../../../../../fixtures/native-sol/link-join/observer_ascii_peer.json");
    const PEER_NON_ASCII_JSON: &str =
        include_str!("../../../../../fixtures/native-sol/link-join/peer_non_ascii_peer.json");
    const NESTED_ENDPOINTS_JSON: &str =
        include_str!("../../../../../fixtures/native-sol/link-join/nested_endpoints_peer.json");
    // A P-256 self-signed CA, so a bundle holding it has a readable pin. Its journal
    // id is 32eb4572-27d1-8392-baa2-6e4ea0b1c953.
    const SERVE_BUNDLE_CERT: &str = "-----BEGIN CERTIFICATE-----\nMIIBizCCATGgAwIBAgIUaAtxcLWxD/p1oqh/9Aocs3XqohIwCgYIKoZIzj0EAwIw\nGjEYMBYGA1UEAwwPam91cm5hbCB0ZXN0IENBMCAXDTI2MTEwMzIwMzA1N1oYDzIx\nMjYxMDEwMjAzMDU3WjAaMRgwFgYDVQQDDA9qb3VybmFsIHRlc3QgQ0EwWTATBgcq\nhkjOPQIBBggqhkjOPQMBBwNCAASjAUrtvHhXImO9uNkYvS23BVGgrfiDjKIIX6dy\n9ypRVytQTOr22rJTMz1Ho3UL3CCPgI3vE++74Td1ksv4VEtFo1MwUTAdBgNVHQ4E\nFgQUtHl+/hoa8sO3oNNBye16XSzrmIswHwYDVR0jBBgwFoAUtHl+/hoa8sO3oNNB\nye16XSzrmIswDwYDVR0TAQH/BAUwAwEB/zAKBggqhkjOPQQDAgNIADBFAiEAt5nD\nKg4ulCZR5pf3n3o6UrGnMyeC7zSwY9nxImwYzYwCIAIRhpaTkFcdo8xh+WZbnRf3\nKsepBLM5i/VJXfBt8/L/\n-----END CERTIFICATE-----\n";

    static TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);
    #[cfg(unix)]
    static UMASK_MUTEX: Mutex<()> = Mutex::new(());
    #[cfg(unix)]
    struct UmaskGuard {
        previous: nix::sys::stat::Mode,
    }

    struct ImmediateShutdown;

    impl ShutdownSignal for ImmediateShutdown {
        fn wait(&self) {}
    }

    #[cfg(unix)]
    impl UmaskGuard {
        fn set(mask: nix::libc::mode_t) -> Self {
            let previous = nix::sys::stat::umask(nix::sys::stat::Mode::from_bits_truncate(mask));
            Self { previous }
        }
    }

    #[cfg(unix)]
    impl Drop for UmaskGuard {
        fn drop(&mut self) {
            let _ = nix::sys::stat::umask(self.previous);
        }
    }

    fn string_args(values: &[&str]) -> Vec<String> {
        values.iter().map(|value| (*value).to_string()).collect()
    }

    fn direct_pair_link() -> String {
        let mut blob = vec![0x04, 0x01, 192, 168, 1, 10];
        blob.extend_from_slice(&7657u16.to_be_bytes());
        blob.extend_from_slice(&[0x11; 16]);
        blob.extend_from_slice(&[0x22; 16]);
        format!("{PAIR_LINK_PREFIX}{}", crockford::encode(&blob))
    }

    fn relay_pair_link() -> String {
        let mut blob = vec![0x06];
        blob.extend_from_slice(&[0x33; 8]);
        blob.push(0x01);
        blob.extend_from_slice(&[0x44; 16]);
        blob.push(0);
        format!("{PAIR_LINK_PREFIX}{}", crockford::encode(&blob))
    }

    fn temp_dir(name: &str) -> PathBuf {
        let id = TEMP_COUNTER.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "solstone-link-join-test-{}-{id}-{name}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&path);
        fs::create_dir_all(&path).expect("temp dir");
        path
    }

    fn base_env(config: &Path, home: &Path) -> BTreeMap<String, String> {
        BTreeMap::from([
            ("XDG_CONFIG_HOME".to_string(), config.display().to_string()),
            ("HOME".to_string(), home.display().to_string()),
            ("HOSTNAME".to_string(), "Test Host".to_string()),
        ])
    }

    fn run(
        args: &[&str],
        env: &BTreeMap<String, String>,
        _journal_root: &Path,
        seam: &ScriptedLinkJoinPairingSeam,
        clock: &FakeClock,
    ) -> CommandOutput {
        let argv = string_args(args);
        let transport = ScriptedHttpTransport::new(vec![]);
        link_join(CommandContext {
            args: &argv,
            env,
            stdin: "".into(),
            transport: &transport,
            clock: Some(clock),
            files: None,
            build_identity: None,
            client_item_ids: None,
            notification_sink: None,
            link_pairing: Some(seam),
            link_serve: None,
            link_status_probe: None,
        })
    }

    fn run_serve<'a>(
        args: &[&str],
        env: &'a BTreeMap<String, String>,
        runner: &'a ScriptedLinkServeRunner,
    ) -> Result<ResidentCommand<'a>, CommandOutput> {
        let argv = Box::leak(string_args(args).into_boxed_slice());
        let transport = Box::leak(Box::new(ScriptedHttpTransport::new(vec![])));
        link_serve(CommandContext {
            args: argv,
            env,
            stdin: "".into(),
            transport,
            clock: None,
            files: None,
            build_identity: None,
            client_item_ids: None,
            notification_sink: None,
            link_pairing: None,
            link_serve: Some(runner),
            link_status_probe: None,
        })
    }

    /// The owner goes ahead on whichever question the credential raises; these
    /// fixtures' id derives no mark, so the question is `continue`.
    fn answering_seam(calls: Vec<ExpectedLinkJoinPairingCall>) -> ScriptedLinkJoinPairingSeam {
        ScriptedLinkJoinPairingSeam::new(calls).with_terminal(&["continue"])
    }

    fn credential(local_endpoints: Value) -> LinkJoinCredential {
        LinkJoinCredential {
            client_key_pem: "PRIVATE\n".to_string(),
            client_cert_pem: "CERT\n".to_string(),
            ca_chain_pem: vec!["CA".to_string()],
            ca_fingerprint: "sha256:abc".to_string(),
            instance_id: "receiver-instance".to_string(),
            home_label: "Home".to_string(),
            home_attestation: Some("header.payload.signature".to_string()),
            local_endpoints,
            relay_origin: None,
            relay_device_token: None,
            relay_device_token_expires_at: None,
            ca_fp_prefix: vec![0x22; 16],
            endpoints: Vec::new(),
        }
    }

    fn bundle_files() -> BTreeMap<String, Vec<u8>> {
        BTreeMap::from([
            ("private.pem".to_string(), b"PRIVATE\n".to_vec()),
            ("cert.pem".to_string(), b"CERT\n".to_vec()),
            ("chain.pem".to_string(), b"CA\n".to_vec()),
            (
                "home_attestation.jwt".to_string(),
                b"header.payload.signature".to_vec(),
            ),
            (
                "peer.json".to_string(),
                OBSERVER_PEER_JSON.as_bytes().to_vec(),
            ),
        ])
    }

    fn serve_bundle(config: &Path, label: &str, local_endpoints: Value) -> LinkServeBundle {
        const CERT: &str = SERVE_BUNDLE_CERT;
        let bundle_dir = config.join("solstone-observer").join("spl").join(label);
        fs::create_dir_all(&bundle_dir).expect("serve bundle dir");
        fs::write(bundle_dir.join("private.pem"), "PRIVATE\n").expect("private key");
        fs::write(bundle_dir.join("cert.pem"), CERT).expect("client cert");
        fs::write(bundle_dir.join("chain.pem"), CERT).expect("chain");
        fs::write(bundle_dir.join("home_attestation.jwt"), "attestation.jwt").expect("attestation");
        fs::write(
            bundle_dir.join("peer.json"),
            json!({
                "instance_id": "32eb4572-27d1-8392-baa2-6e4ea0b1c953",
                "home_label": "Home",
                "paired_at": "2026-07-26T00:00:00Z",
                "local_endpoints": local_endpoints.clone(),
            })
            .to_string(),
        )
        .expect("peer json");
        LinkServeBundle {
            private_key_pem: "PRIVATE\n".to_string(),
            client_cert_pem: CERT.to_string(),
            ca_chain_pem: vec![CERT.to_string()],
            home_attestation: "attestation.jwt".to_string(),
            instance_id: "32eb4572-27d1-8392-baa2-6e4ea0b1c953".to_string(),
            home_label: "Home".to_string(),
            paired_at: "2026-07-26T00:00:00Z".to_string(),
            endpoints: serve_endpoints_from_value("en-US", &local_endpoints)
                .expect("serve endpoints"),
            local_endpoints,
            relay_access: Some(crate::link_credentials::StoreLoadOutcome::Absent),
        }
    }

    fn serve_bundle_with_instance_id(
        config: &Path,
        label: &str,
        local_endpoints: Value,
        instance_id: &str,
    ) -> LinkServeBundle {
        const CERT: &str = SERVE_BUNDLE_CERT;
        let bundle_dir = config.join("solstone-observer").join("spl").join(label);
        fs::create_dir_all(&bundle_dir).expect("serve bundle dir");
        fs::write(bundle_dir.join("private.pem"), "PRIVATE\n").expect("private key");
        fs::write(bundle_dir.join("cert.pem"), CERT).expect("client cert");
        fs::write(bundle_dir.join("chain.pem"), CERT).expect("chain");
        fs::write(bundle_dir.join("home_attestation.jwt"), "attestation.jwt").expect("attestation");
        fs::write(
            bundle_dir.join("peer.json"),
            json!({
                "instance_id": instance_id,
                "home_label": "Home",
                "paired_at": "2026-07-26T00:00:00Z",
                "local_endpoints": local_endpoints.clone(),
            })
            .to_string(),
        )
        .expect("peer json");
        LinkServeBundle {
            private_key_pem: "PRIVATE\n".to_string(),
            client_cert_pem: CERT.to_string(),
            ca_chain_pem: vec![CERT.to_string()],
            home_attestation: "attestation.jwt".to_string(),
            instance_id: instance_id.to_string(),
            home_label: "Home".to_string(),
            paired_at: "2026-07-26T00:00:00Z".to_string(),
            endpoints: serve_endpoints_from_value("en-US", &local_endpoints)
                .expect("serve endpoints"),
            local_endpoints,
            relay_access: Some(crate::link_credentials::StoreLoadOutcome::Absent),
        }
    }

    fn serve_bundle_ca_fp_prefix() -> String {
        ca_fp_prefix_hex(&[SERVE_BUNDLE_CERT.to_string()]).expect("test cert ca fp prefix")
    }

    fn expected_serve_request(
        config: &Path,
        label: &str,
        port: u16,
        policy: LinkServeCarrierPolicy,
        relay_origin: Option<&str>,
        bundle: LinkServeBundle,
    ) -> LinkServeRequest {
        LinkServeRequest {
            label: label.to_string(),
            port,
            policy,
            relay_origin: relay_origin.map(str::to_string),
            bundle,
            bundle_dir: config.join("solstone-observer").join("spl").join(label),
        }
    }

    fn assert_bundle_files_exist(bundle: &Path) {
        for name in BUNDLE_FILES {
            assert!(bundle.join(name).is_file(), "{name}");
        }
    }

    fn bundle_hashes(bundle: &Path) -> BTreeMap<String, String> {
        BUNDLE_FILES
            .iter()
            .map(|name| {
                let bytes = fs::read(bundle.join(name)).expect("bundle file bytes");
                ((*name).to_string(), spl_core::ca::sha256_hex(&bytes))
            })
            .collect()
    }

    fn assert_no_dot_residue(parent: &Path) {
        if !parent.exists() {
            return;
        }
        let residues = fs::read_dir(parent)
            .expect("parent entries")
            .map(|entry| entry.expect("parent entry").file_name())
            .filter(|name| name.to_string_lossy().starts_with('.'))
            .collect::<Vec<_>>();
        assert!(residues.is_empty(), "staging residue: {residues:?}");
    }

    fn secret_substrings() -> &'static [&'static str] {
        &[
            "raw peer body secret",
            "00112233445566778899aabbccddeeff",
            "BEGIN CERTIFICATE REQUEST",
            "sha256:secretfingerprint",
            "https://go.solstone.app/p#SECRETFRAGMENT",
        ]
    }

    fn assert_no_secret_substrings(text: &str) {
        for secret in secret_substrings() {
            assert!(
                !text.contains(secret),
                "secret substring {secret:?} leaked in {text:?}"
            );
        }
    }

    #[cfg(unix)]
    fn mode(path: &Path) -> u32 {
        use std::os::unix::fs::PermissionsExt;

        fs::metadata(path).expect("metadata").permissions().mode() & 0o777
    }

    #[cfg(unix)]
    fn assert_bundle_permissions_under_current_umask() {
        let temp = temp_dir("permissions");
        let bundle = temp.join("bundle");
        publish_bundle_atomic("en-US", &bundle, &bundle_files()).expect("publish bundle");

        assert_eq!(mode(&bundle), 0o700);
        for name in BUNDLE_FILES {
            assert_eq!(mode(&bundle.join(name)), 0o600, "{name}");
        }
    }

    fn expected_direct_request(label: &str) -> LinkJoinDirectRequest {
        LinkJoinDirectRequest {
            targets: vec![LinkJoinPairTarget {
                host: "192.168.1.10".to_string(),
                port: 7657,
            }],
            nonce_hex: "11111111111111111111111111111111".to_string(),
            ca_fp_prefix: vec![0x22; 16],
            device_label: label.to_string(),
            additional_fields: Map::new(),
        }
    }

    #[test]
    fn help_prints_every_join_option() {
        let env = BTreeMap::new();
        let root = temp_dir("help-root");
        let seam = answering_seam(vec![]);
        let clock = FakeClock::at_unix(0);
        let output = run(&["--help"], &env, &root, &seam, &clock);
        assert_eq!(output, CommandOutput::success(HELP));
        assert_eq!(HELP.len(), 509);
        seam.assert_done();
    }

    #[test]
    fn serve_help_is_python_byte_exact() {
        let env = BTreeMap::new();
        let runner = ScriptedLinkServeRunner::new(vec![]);
        let output = match run_serve(&["--help"], &env, &runner) {
            Err(output) => output,
            Ok(_) => panic!("help must not enter resident serve"),
        };
        assert_eq!(output, CommandOutput::success(SERVE_HELP));
        runner.assert_done();
    }

    #[test]
    fn serve_argv_errors_exit_two_before_starting() {
        let cases = [
            (
                vec!["--unknown"],
                format!(
                    "{SERVE_USAGE}solstone link serve: error: unrecognized arguments: --unknown\n"
                ),
            ),
            (
                vec!["--label"],
                format!(
                    "{SERVE_USAGE}solstone link serve: error: argument --label: expected one argument\n"
                ),
            ),
            (
                vec!["--port", "abc"],
                format!(
                    "{SERVE_USAGE}solstone link serve: error: argument --port: invalid int value: 'abc'\n"
                ),
            ),
            (
                vec!["--port", "-1"],
                format!(
                    "{SERVE_USAGE}solstone link serve: error: --port must be between 1 and 65535\n"
                ),
            ),
            (
                vec!["--port", "65536"],
                format!(
                    "{SERVE_USAGE}solstone link serve: error: --port must be between 1 and 65535\n"
                ),
            ),
            (
                vec!["--direct", "--relay-only"],
                format!(
                    "{SERVE_USAGE}solstone link serve: error: argument --direct: not allowed with argument --relay-only\n"
                ),
            ),
            (
                vec!["--relay-only", "--direct"],
                format!(
                    "{SERVE_USAGE}solstone link serve: error: argument --direct: not allowed with argument --relay-only\n"
                ),
            ),
        ];
        for (args, expected_stderr) in cases {
            let env = BTreeMap::new();
            let runner = ScriptedLinkServeRunner::new(vec![]);
            let output = match run_serve(&args, &env, &runner) {
                Err(output) => output,
                Ok(_) => panic!("argv error must not enter resident serve"),
            };
            assert_eq!(output.stdout, "");
            assert_eq!(output.stderr, expected_stderr);
            assert_eq!(output.exit, 2);
            runner.assert_done();
        }
    }

    #[test]
    fn serve_bundle_resolution_names_sorted_labels_and_supports_single_default() {
        let temp = temp_dir("serve-bundles");
        let config = temp.join("config");
        let env = base_env(&config, &temp.join("home"));
        let alpha = serve_bundle(
            &config,
            "alpha",
            json!([{"ip": "192.168.1.10", "port": 7657}]),
        );
        let beta = serve_bundle(
            &config,
            "beta",
            json!([{"ip": "192.168.1.10", "port": 7657}]),
        );
        let runner = ScriptedLinkServeRunner::new(vec![]);
        let ambiguous = match run_serve(&[], &env, &runner) {
            Err(output) => output,
            Ok(_) => panic!("ambiguous bundle must not enter resident serve"),
        };
        let alpha_offset = ambiguous.stderr.find("alpha").expect("must name alpha");
        let beta_offset = ambiguous.stderr.find("beta").expect("must name beta");
        assert!(alpha_offset < beta_offset);
        assert_eq!(ambiguous.exit, 1);
        runner.assert_done();

        let explicit_runner = ScriptedLinkServeRunner::new(vec![ExpectedLinkServeCall {
            expected: expected_serve_request(
                &config,
                "beta",
                5016,
                LinkServeCarrierPolicy::RelayPermitted,
                None,
                beta,
            ),
            result: Ok(ExpectedLinkServeSession {
                bound_port: 5016,
                serve_result: Ok(()),
            }),
        }]);
        let explicit = match run_serve(
            &["--label", "beta", "--port", "5016"],
            &env,
            &explicit_runner,
        ) {
            Ok(resident) => resident,
            Err(output) => panic!("explicit bundle failed before resident: {output:?}"),
        };
        let words = mark_words_from_jid("32eb4572-27d1-8392-baa2-6e4ea0b1c953").unwrap();
        assert_eq!(
            explicit.startup(),
            format!("forwarding 127.0.0.1:5016 -> {words} via direct or relay\n")
        );
        assert_eq!(
            explicit.serve(&ImmediateShutdown),
            CommandOutput::success("")
        );
        explicit_runner.assert_done();

        fs::remove_dir_all(config.join("solstone-observer").join("spl").join("beta"))
            .expect("remove beta");
        let default_runner = ScriptedLinkServeRunner::new(vec![ExpectedLinkServeCall {
            expected: expected_serve_request(
                &config,
                "alpha",
                DEFAULT_SERVE_PORT,
                LinkServeCarrierPolicy::RelayPermitted,
                None,
                alpha,
            ),
            result: Ok(ExpectedLinkServeSession {
                bound_port: DEFAULT_SERVE_PORT,
                serve_result: Ok(()),
            }),
        }]);
        let defaulted = match run_serve(&[], &env, &default_runner) {
            Ok(resident) => resident,
            Err(output) => panic!("single bundle failed before resident: {output:?}"),
        };
        assert_eq!(
            defaulted.startup(),
            format!("forwarding 127.0.0.1:5015 -> {words} via direct or relay\n")
        );
        default_runner.assert_done();
    }

    #[test]
    fn serve_port_zero_requests_ephemeral_and_prints_bound_port() {
        let temp = temp_dir("serve-port-zero");
        let config = temp.join("config");
        let env = base_env(&config, &temp.join("home"));
        let bundle = serve_bundle(
            &config,
            "alpha",
            json!([{"ip": "192.168.1.10", "port": 7657}]),
        );
        let runner = ScriptedLinkServeRunner::new(vec![ExpectedLinkServeCall {
            expected: expected_serve_request(
                &config,
                "alpha",
                0,
                LinkServeCarrierPolicy::RelayPermitted,
                None,
                bundle,
            ),
            result: Ok(ExpectedLinkServeSession {
                bound_port: 54321,
                serve_result: Ok(()),
            }),
        }]);
        let resident = match run_serve(&["--port", "0"], &env, &runner) {
            Ok(resident) => resident,
            Err(output) => panic!("port 0 must enter resident serve: {output:?}"),
        };
        let words = mark_words_from_jid("32eb4572-27d1-8392-baa2-6e4ea0b1c953").unwrap();
        assert_eq!(
            resident.startup(),
            format!("forwarding 127.0.0.1:54321 -> {words} via direct or relay\n")
        );
        runner.assert_done();
    }

    #[test]
    fn serve_direct_omits_relay_even_with_poisoned_relay_inputs() {
        let temp = temp_dir("serve-direct");
        let config = temp.join("config");
        let mut env = base_env(&config, &temp.join("home"));
        env.insert(
            "SOL_LINK_RELAY_URL".to_string(),
            "https://poisoned.invalid".to_string(),
        );
        let bundle = serve_bundle(
            &config,
            "direct",
            json!([{"ip": "192.168.1.10", "port": 7657}]),
        );
        let runner = ScriptedLinkServeRunner::new(vec![ExpectedLinkServeCall {
            expected: expected_serve_request(
                &config,
                "direct",
                6001,
                LinkServeCarrierPolicy::Direct,
                None,
                bundle,
            ),
            result: Ok(ExpectedLinkServeSession {
                bound_port: 6001,
                serve_result: Ok(()),
            }),
        }]);

        let resident = match run_serve(
            &[
                "--label",
                "direct",
                "--port",
                "6001",
                "--relay-url",
                "https://also-poisoned.invalid",
                "--direct",
            ],
            &env,
            &runner,
        ) {
            Ok(resident) => resident,
            Err(output) => panic!("direct serve failed before resident: {output:?}"),
        };

        let words = mark_words_from_jid("32eb4572-27d1-8392-baa2-6e4ea0b1c953").unwrap();
        assert_eq!(
            resident.startup(),
            format!("forwarding 127.0.0.1:6001 -> {words} via direct connection\n")
        );
        assert_eq!(runner.recorded()[0].request.relay_origin, None);
        runner.assert_done();
    }

    #[test]
    fn serve_relay_only_flag_produces_relay_only_policy_and_strips_no_bundle_data() {
        let temp = temp_dir("serve-relay-only");
        let config = temp.join("config");
        let env = base_env(&config, &temp.join("home"));
        let bundle = serve_bundle(
            &config,
            "relay-only",
            json!([{"ip": "192.168.1.10", "port": 7657}]),
        );
        assert!(!bundle.endpoints.is_empty());
        let runner = ScriptedLinkServeRunner::new(vec![ExpectedLinkServeCall {
            expected: expected_serve_request(
                &config,
                "relay-only",
                6002,
                LinkServeCarrierPolicy::RelayOnly,
                None,
                bundle,
            ),
            result: Ok(ExpectedLinkServeSession {
                bound_port: 6002,
                serve_result: Ok(()),
            }),
        }]);

        let resident = match run_serve(
            &["--label", "relay-only", "--port", "6002", "--relay-only"],
            &env,
            &runner,
        ) {
            Ok(resident) => resident,
            Err(output) => panic!("relay-only serve failed before resident: {output:?}"),
        };

        let words = mark_words_from_jid("32eb4572-27d1-8392-baa2-6e4ea0b1c953").unwrap();
        assert_eq!(
            resident.startup(),
            format!("forwarding 127.0.0.1:6002 -> {words} via relay only\n")
        );
        let recorded = runner.recorded();
        assert_eq!(recorded.len(), 1);
        assert_eq!(
            recorded[0].request.policy,
            LinkServeCarrierPolicy::RelayOnly
        );
        assert_eq!(recorded[0].request.relay_origin, None);
        assert!(!recorded[0].request.bundle.endpoints.is_empty());
        runner.assert_done();
    }

    #[test]
    fn serve_relay_only_flag_uses_explicit_relay_url() {
        let temp = temp_dir("serve-relay-only-override");
        let config = temp.join("config");
        let env = base_env(&config, &temp.join("home"));
        let relay_origin = "https://custom-relay.example";
        let bundle = serve_bundle(
            &config,
            "relay-only",
            json!([{"ip": "192.168.1.10", "port": 7657}]),
        );
        let runner = ScriptedLinkServeRunner::new(vec![ExpectedLinkServeCall {
            expected: expected_serve_request(
                &config,
                "relay-only",
                6003,
                LinkServeCarrierPolicy::RelayOnly,
                Some(relay_origin),
                bundle,
            ),
            result: Ok(ExpectedLinkServeSession {
                bound_port: 6003,
                serve_result: Ok(()),
            }),
        }]);

        let resident = match run_serve(
            &[
                "--label",
                "relay-only",
                "--port",
                "6003",
                "--relay-url",
                relay_origin,
                "--relay-only",
            ],
            &env,
            &runner,
        ) {
            Ok(resident) => resident,
            Err(output) => panic!("relay-only serve failed before resident: {output:?}"),
        };

        let words = mark_words_from_jid("32eb4572-27d1-8392-baa2-6e4ea0b1c953").unwrap();
        assert_eq!(
            resident.startup(),
            format!("forwarding 127.0.0.1:6003 -> {words} via relay only\n")
        );
        assert_eq!(
            runner.recorded()[0].request.relay_origin,
            Some(relay_origin.to_string())
        );
        runner.assert_done();
    }

    #[test]
    fn serve_non_argv_failures_exit_one() {
        let temp = temp_dir("serve-failures");
        let config = temp.join("config");
        let env = base_env(&config, &temp.join("home"));
        let bundle = serve_bundle(
            &config,
            "laptop",
            json!([{"ip": "192.168.1.10", "port": 7657}]),
        );
        let bind_runner = ScriptedLinkServeRunner::new(vec![ExpectedLinkServeCall {
            expected: expected_serve_request(
                &config,
                "laptop",
                DEFAULT_SERVE_PORT,
                LinkServeCarrierPolicy::RelayPermitted,
                None,
                bundle.clone(),
            ),
            result: Err(LinkServeError::new(LinkServeErrorKind::Bind {
                port: DEFAULT_SERVE_PORT,
                addr_in_use: true,
            })),
        }]);
        let bind = match run_serve(&["--label", "laptop"], &env, &bind_runner) {
            Err(output) => output,
            Ok(_) => panic!("bind failure must not return resident"),
        };
        assert_eq!(bind.exit, 1);
        assert_eq!(
            bind.stderr,
            "cannot bind 127.0.0.1:5015: address already in use. Another `solstone link serve` or Convey may already be using that port.\n"
        );
        bind_runner.assert_done();

        let enroll_runner = ScriptedLinkServeRunner::new(vec![ExpectedLinkServeCall {
            expected: expected_serve_request(
                &config,
                "laptop",
                DEFAULT_SERVE_PORT,
                LinkServeCarrierPolicy::RelayPermitted,
                None,
                bundle,
            ),
            result: Err(LinkServeError::new(LinkServeErrorKind::Transport(
                LinkServeTransportErrorKind::RelayControlRejected {
                    endpoint: LinkServeRelayControlEndpoint::EnrollDevice,
                    status: 401,
                },
            ))),
        }]);
        let enrollment = match run_serve(&["--label", "laptop"], &env, &enroll_runner) {
            Err(output) => output,
            Ok(_) => panic!("enrollment failure must not return resident"),
        };
        assert_eq!(enrollment.exit, 1);
        assert_eq!(
            enrollment.stderr,
            "Relay enrollment was rejected with HTTP 401. Re-run solstone link join if the bundle attestation is stale.\n"
        );
        enroll_runner.assert_done();
    }

    #[test]
    fn serve_transport_error_text_covers_every_variant_without_secret_leaks() {
        let kinds = [
            LinkServeTransportErrorKind::Io,
            LinkServeTransportErrorKind::Tls,
            LinkServeTransportErrorKind::TlsAccessDenied,
            LinkServeTransportErrorKind::TlsCertificateUnknown,
            LinkServeTransportErrorKind::Crypto,
            LinkServeTransportErrorKind::Mux,
            LinkServeTransportErrorKind::Http,
            LinkServeTransportErrorKind::Json,
            LinkServeTransportErrorKind::PairLink,
            LinkServeTransportErrorKind::Pairing,
            LinkServeTransportErrorKind::Rejected { status: 403 },
            LinkServeTransportErrorKind::Relay(LinkServeRelayErrorKind::HomeOffline),
            LinkServeTransportErrorKind::Relay(LinkServeRelayErrorKind::Unauthorized),
            LinkServeTransportErrorKind::Relay(LinkServeRelayErrorKind::Unpaid),
            LinkServeTransportErrorKind::Relay(LinkServeRelayErrorKind::UnknownInstance),
            LinkServeTransportErrorKind::Relay(LinkServeRelayErrorKind::PairWindowClosed),
            LinkServeTransportErrorKind::Relay(LinkServeRelayErrorKind::Overflow),
            LinkServeTransportErrorKind::Relay(LinkServeRelayErrorKind::Abnormal),
            LinkServeTransportErrorKind::Relay(LinkServeRelayErrorKind::UpgradeRejected),
            LinkServeTransportErrorKind::Relay(LinkServeRelayErrorKind::Stalled),
            LinkServeTransportErrorKind::RelayControlRejected {
                endpoint: LinkServeRelayControlEndpoint::EnrollDevice,
                status: 401,
            },
            LinkServeTransportErrorKind::RelayControlRejected {
                endpoint: LinkServeRelayControlEndpoint::TokenRefresh,
                status: 401,
            },
            LinkServeTransportErrorKind::NoEndpoint,
            LinkServeTransportErrorKind::NotPaired,
            LinkServeTransportErrorKind::LocalOffset,
        ];
        for kind in kinds {
            let text = serve_transport_error_text("en-US", kind);
            assert_no_secret_substrings(&text);
        }
    }

    #[test]
    fn missing_code_exits_like_argparse() {
        let env = BTreeMap::new();
        let root = temp_dir("missing-code-root");
        let seam = answering_seam(vec![]);
        let clock = FakeClock::at_unix(0);
        let output = run(&[], &env, &root, &seam, &clock);
        assert_eq!(
            output.stderr,
            format!(
                "{USAGE}solstone link join: error: the following arguments are required: --code\n"
            )
        );
        assert_eq!(output.exit, 2);
        seam.assert_done();
    }

    #[test]
    fn invalid_role_and_label_fail_before_pairing() {
        let env = BTreeMap::new();
        let root = temp_dir("invalid-root");
        let seam = answering_seam(vec![]);
        let clock = FakeClock::at_unix(0);
        let role = run(
            &["--code", &direct_pair_link(), "--as", "bad"],
            &env,
            &root,
            &seam,
            &clock,
        );
        assert_eq!(
            role.stderr,
            "invalid role; expected one of: phone, observer\n"
        );
        assert_eq!(role.exit, 2);
        let label = run(
            &["--code", &direct_pair_link(), "--label", "bad..name"],
            &env,
            &root,
            &seam,
            &clock,
        );
        assert_eq!(label.stderr, "--label must not contain '..'\n");
        assert_eq!(label.exit, 2);
        seam.assert_done();
    }

    #[test]
    fn observer_existing_path_is_checked_before_direct_pairing() {
        let temp = temp_dir("observer-precheck");
        let config = temp.join("config");
        let env = base_env(&config, &temp.join("home"));
        let root = temp.join("journal");
        let existing = config.join("solstone-observer").join("spl").join("laptop");
        fs::create_dir_all(&existing).expect("existing bundle");
        let seam = answering_seam(vec![]);
        let clock = FakeClock::at_unix(0);

        let output = run(
            &["--code", &direct_pair_link(), "--label", "laptop"],
            &env,
            &root,
            &seam,
            &clock,
        );

        assert_eq!(
            output.stderr,
            format!("{}\n", existing_path_message("en-US", &existing))
        );
        assert_eq!(output.exit, 1);
        seam.assert_done();
    }

    #[test]
    fn observer_existing_path_is_checked_before_relay_pairing() {
        let temp = temp_dir("observer-relay-precheck");
        let config = temp.join("config");
        let env = base_env(&config, &temp.join("home"));
        let root = temp.join("journal");
        let existing = config.join("solstone-observer").join("spl").join("laptop");
        fs::create_dir_all(&existing).expect("existing bundle");
        let seam = answering_seam(vec![]);
        let clock = FakeClock::at_unix(0);

        let output = run(
            &["--code", &relay_pair_link(), "--label", "laptop"],
            &env,
            &root,
            &seam,
            &clock,
        );

        assert_eq!(
            output.stderr,
            format!("{}\n", existing_path_message("en-US", &existing))
        );
        assert_eq!(output.exit, 1);
        seam.assert_done();
    }

    #[test]
    fn observer_success_writes_one_bundle_with_python_peer_json_bytes() {
        let temp = temp_dir("observer-success");
        let config = temp.join("config");
        let env = base_env(&config, &temp.join("home"));
        let root = temp.join("journal");
        let expected = expected_direct_request("laptop");
        let seam = answering_seam(vec![ExpectedLinkJoinPairingCall::Direct {
            expected,
            result: Ok(credential(Value::Null)),
        }]);
        let clock = FakeClock::at_unix(0);

        let output = run(
            &["--code", &direct_pair_link(), "--label", "laptop"],
            &env,
            &root,
            &seam,
            &clock,
        );

        let bundle = config.join("solstone-observer").join("spl").join("laptop");
        assert_eq!(
            output.stdout,
            format!("{PAIRED}\ncredentials: {}\n", bundle.display())
        );
        assert_eq!(output.exit, 0);
        assert_eq!(
            fs::read_to_string(bundle.join("peer.json")).expect("peer json"),
            OBSERVER_PEER_JSON
        );
        let entries = fs::read_dir(bundle.parent().expect("bundle parent"))
            .expect("bundle parent")
            .collect::<Result<Vec<_>, _>>()
            .expect("bundle entries")
            .into_iter()
            .filter(|entry| !entry.file_name().to_string_lossy().starts_with('.'))
            .collect::<Vec<_>>();
        assert_eq!(entries.len(), 1);
        assert_bundle_files_exist(&bundle);
        seam.assert_done();
    }

    const GATE_JID: &str = "32eb4572-27d1-8392-baa2-6e4ea0b1c953";

    struct GateRun {
        output: CommandOutput,
        bundle: PathBuf,
    }

    fn gate_run(seam: &ScriptedLinkJoinPairingSeam, extra: &[&str]) -> GateRun {
        let temp = temp_dir("join-gate");
        let config = temp.join("config");
        let env = base_env(&config, &temp.join("home"));
        let clock = FakeClock::at_unix(0);
        let code = direct_pair_link();
        let mut args = vec!["--code", code.as_str(), "--label", "laptop"];
        args.extend_from_slice(extra);
        let output = run(&args, &env, &temp.join("journal"), seam, &clock);
        GateRun {
            output,
            bundle: config.join("solstone-observer").join("spl").join("laptop"),
        }
    }

    fn pairing_calls(instance_id: &str) -> Vec<ExpectedLinkJoinPairingCall> {
        let mut paired = credential(Value::Null);
        paired.instance_id = instance_id.to_string();
        vec![ExpectedLinkJoinPairingCall::Direct {
            expected: expected_direct_request("laptop"),
            result: Ok(paired),
        }]
    }

    fn gate_words() -> Vec<String> {
        crate::mark_text::mark_from_jid(GATE_JID)
            .expect("mark")
            .to_render_spec()
            .words
            .iter()
            .map(ToString::to_string)
            .collect()
    }

    #[test]
    fn yes_at_the_terminal_shows_the_mark_then_writes_the_bundle() {
        let seam = ScriptedLinkJoinPairingSeam::new(pairing_calls(GATE_JID))
            .with_terminal(&["maybe", "", " YES "]);
        let run = gate_run(&seam, &[]);
        assert_eq!(run.output.exit, 0);
        assert_eq!(
            run.output.stdout,
            format!("{PAIRED}\ncredentials: {}\n", run.bundle.display())
        );
        assert_bundle_files_exist(&run.bundle);
        let shown = seam.terminal_shown();
        let spec = crate::mark_text::mark_from_jid(GATE_JID)
            .expect("mark")
            .to_render_spec();
        assert!(shown.contains(&format_spoken_mark(&spec)));
        assert_eq!(shown.matches(ASK_IDENTIFIED).count(), 3);
        assert!(seam.retired().is_empty());
        seam.assert_done();
    }

    #[test]
    fn no_at_the_terminal_writes_nothing_and_retires_the_pairing() {
        for answer in ["no", "n"] {
            let seam =
                ScriptedLinkJoinPairingSeam::new(pairing_calls(GATE_JID)).with_terminal(&[answer]);
            let run = gate_run(&seam, &[]);
            assert_eq!(run.output.exit, 1);
            assert_eq!(
                run.output.stderr,
                format!("{NOT_PAIRED}\n{MISMATCH_BODY}\n")
            );
            assert!(!path_lexists(&run.bundle));
            assert_eq!(seam.retired(), vec![GATE_JID.to_string()]);
            seam.assert_done();
        }
    }

    #[test]
    fn walking_away_from_the_terminal_writes_nothing() {
        let seam = ScriptedLinkJoinPairingSeam::new(pairing_calls(GATE_JID)).with_terminal(&[]);
        let run = gate_run(&seam, &[]);
        assert_eq!(run.output.exit, 1);
        assert_eq!(run.output.stderr, format!("{CANCEL}\n"));
        assert!(!path_lexists(&run.bundle));
        assert_eq!(seam.retired(), vec![GATE_JID.to_string()]);
    }

    #[test]
    fn a_mark_that_cannot_be_worked_out_takes_whole_word_answers() {
        let seam = ScriptedLinkJoinPairingSeam::new(pairing_calls("receiver-instance"))
            .with_terminal(&["yes", "c", "cancel"]);
        let run = gate_run(&seam, &[]);
        assert_eq!(run.output.exit, 1);
        assert_eq!(run.output.stderr, format!("{CANCEL}\n"));
        assert!(!path_lexists(&run.bundle));
        let shown = seam.terminal_shown();
        assert!(shown.contains(MARK_UNAVAILABLE));
        assert_eq!(shown.matches(ASK_UNAVAILABLE).count(), 3);
    }

    #[test]
    fn no_terminal_and_no_mark_refuses_before_the_link_is_spent() {
        let seam = ScriptedLinkJoinPairingSeam::new(vec![]);
        let run = gate_run(&seam, &[]);
        assert_eq!(run.output.exit, 1);
        assert_eq!(run.output.stderr, format!("{JOIN_NO_TERMINAL}\n"));
        assert!(!path_lexists(&run.bundle));
        assert!(seam.recorded().is_empty());
    }

    #[test]
    fn mark_words_that_match_write_the_bundle_without_a_terminal() {
        let words = gate_words();
        for given in [
            format!("{} {}", words[0], words[1]),
            format!("{}\u{b7}{}", words[0].to_uppercase(), words[1]),
        ] {
            let seam = ScriptedLinkJoinPairingSeam::new(pairing_calls(GATE_JID));
            let run = gate_run(&seam, &["--mark", &given]);
            assert_eq!(run.output.exit, 0, "{}", run.output.stderr);
            assert_bundle_files_exist(&run.bundle);
            assert!(seam.retired().is_empty());
            seam.assert_done();
        }
    }

    #[test]
    fn wrong_mark_words_write_nothing_and_retire_the_pairing() {
        let seam = ScriptedLinkJoinPairingSeam::new(pairing_calls(GATE_JID));
        let run = gate_run(&seam, &["--mark", "wrong words"]);
        assert_eq!(run.output.exit, 1);
        assert_eq!(
            run.output.stderr,
            format!("{NOT_PAIRED}\n{MARK_MISMATCH}\n")
        );
        assert!(!path_lexists(&run.bundle));
        assert_eq!(seam.retired(), vec![GATE_JID.to_string()]);
    }

    #[test]
    fn mark_words_cannot_vouch_for_a_mark_that_cannot_be_worked_out() {
        let seam = ScriptedLinkJoinPairingSeam::new(pairing_calls("receiver-instance"));
        let run = gate_run(&seam, &["--mark", "any words"]);
        assert_eq!(run.output.exit, 1);
        assert_eq!(
            run.output.stderr,
            format!("{COULDNT_VERIFY}\n{MARK_UNVERIFIABLE}\n")
        );
        assert!(!path_lexists(&run.bundle));
        assert_eq!(seam.retired(), vec!["receiver-instance".to_string()]);
    }

    #[test]
    fn malformed_mark_is_a_usage_error_before_the_link_is_spent() {
        for extra in [
            &["--mark", "one"][..],
            &["--mark", "three whole words"][..],
            &["--mark"][..],
            &["--mark", "a b", "--mark", "c d"][..],
        ] {
            let seam = ScriptedLinkJoinPairingSeam::new(vec![]);
            let run = gate_run(&seam, extra);
            assert_eq!(run.output.exit, 2);
            assert_eq!(run.output.stderr, format!("{MARK_USAGE}\n"));
            assert!(seam.recorded().is_empty());
        }
    }

    #[cfg(unix)]
    #[test]
    fn bundle_permissions_are_explicit_under_permissive_umask() {
        let _lock = UMASK_MUTEX.lock().expect("umask lock");
        let _guard = UmaskGuard::set(0o000);
        assert_bundle_permissions_under_current_umask();
    }

    #[test]
    fn mid_write_failure_leaves_no_final_or_staging_residue() {
        let temp = temp_dir("mid-write");
        let bundle = temp.join("spl").join("laptop");
        let parent = bundle.parent().expect("bundle parent");
        let writes = Cell::new(0usize);

        let result = publish_bundle_atomic_with_writer(
            "en-US",
            &bundle,
            &bundle_files(),
            |path, content| {
                if writes.get() == 1 {
                    return Err(io::Error::other("injected mid-write failure"));
                }
                writes.set(writes.get() + 1);
                write_private_file(path, content)
            },
        );

        assert_eq!(writes.get(), 1);
        assert!(result.is_err());
        assert!(!path_lexists(&bundle));
        assert_no_dot_residue(parent);
    }

    #[test]
    fn publication_never_replaces_a_racing_empty_destination() {
        let temp = temp_dir("publish-race");
        let bundle = temp.join("spl").join("laptop");
        let parent = bundle.parent().expect("bundle parent");
        let created = Cell::new(false);

        let result = publish_bundle_atomic_with_writer(
            "en-US",
            &bundle,
            &bundle_files(),
            |path, content| {
                if !created.replace(true) {
                    fs::create_dir(&bundle)?;
                }
                write_private_file(path, content)
            },
        );

        assert_eq!(
            result.expect_err("racing destination must win").kind(),
            io::ErrorKind::AlreadyExists
        );
        assert!(bundle.is_dir());
        assert_eq!(fs::read_dir(&bundle).expect("winner directory").count(), 0);
        assert_no_dot_residue(parent);
    }

    #[test]
    fn retired_peer_role_is_rejected_without_pairing() {
        let temp = temp_dir("peer-state");
        let config = temp.join("config");
        let env = base_env(&config, &temp.join("home"));
        let root = temp.join("journal");
        let seam = answering_seam(vec![]);
        let clock = FakeClock::at_unix(0);

        let output = run(
            &["--code", &direct_pair_link(), "--as", "peer"],
            &env,
            &root,
            &seam,
            &clock,
        );

        assert_eq!(
            output.stderr,
            "invalid role; expected one of: phone, observer\n"
        );
        assert_eq!(output.exit, 2);
        seam.assert_done();
    }

    #[test]
    fn existing_bundle_refuses_without_mutating_any_file() {
        let temp = temp_dir("refuse-without-mutation");
        let config = temp.join("config");
        let env = base_env(&config, &temp.join("home"));
        let root = temp.join("journal");
        let bundle = config.join("solstone-observer").join("spl").join("laptop");
        let first_seam = answering_seam(vec![ExpectedLinkJoinPairingCall::Direct {
            expected: expected_direct_request("laptop"),
            result: Ok(credential(Value::Null)),
        }]);
        let first_clock = FakeClock::at_unix(0);
        let first = run(
            &["--code", &direct_pair_link(), "--label", "laptop"],
            &env,
            &root,
            &first_seam,
            &first_clock,
        );
        assert_eq!(first.exit, 0);
        first_seam.assert_done();
        let before = bundle_hashes(&bundle);

        let second_seam = answering_seam(vec![]);
        let second_clock = FakeClock::at_unix(60);
        let second = run(
            &["--code", &direct_pair_link(), "--label", "laptop"],
            &env,
            &root,
            &second_seam,
            &second_clock,
        );

        assert_ne!(second.exit, 0);
        assert_eq!(bundle_hashes(&bundle), before);
        second_seam.assert_done();
    }

    #[test]
    fn local_endpoints_shape_errors_fail_before_writing() {
        let temp = temp_dir("endpoint-shape");
        let config = temp.join("config");
        let env = base_env(&config, &temp.join("home"));
        let root = temp.join("journal");
        let bundle = config.join("solstone-observer").join("spl").join("laptop");
        let seam = answering_seam(vec![ExpectedLinkJoinPairingCall::Direct {
            expected: expected_direct_request("laptop"),
            result: Ok(credential(json!({"ip": "10.0.0.2"}))),
        }]);
        let clock = FakeClock::at_unix(0);

        let output = run(
            &["--code", &direct_pair_link(), "--label", "laptop"],
            &env,
            &root,
            &seam,
            &clock,
        );

        assert_eq!(
            output.stderr,
            "Pair response local_endpoints must be an array.\n"
        );
        assert_eq!(output.exit, 1);
        assert!(!path_lexists(&bundle));
        seam.assert_done();
    }

    #[test]
    fn local_endpoints_size_errors_fail_before_writing() {
        let temp = temp_dir("endpoint-size");
        let config = temp.join("config");
        let env = base_env(&config, &temp.join("home"));
        let root = temp.join("journal");
        let bundle = config.join("solstone-observer").join("spl").join("laptop");
        let endpoints = Value::Array(vec![Value::String("x".repeat(LOCAL_ENDPOINTS_MAX_BYTES))]);
        let seam = answering_seam(vec![ExpectedLinkJoinPairingCall::Direct {
            expected: expected_direct_request("laptop"),
            result: Ok(credential(endpoints)),
        }]);
        let clock = FakeClock::at_unix(0);

        let output = run(
            &["--code", &direct_pair_link(), "--label", "laptop"],
            &env,
            &root,
            &seam,
            &clock,
        );

        assert_eq!(
            output.stderr,
            "Pair response local_endpoints is too large.\n"
        );
        assert_eq!(output.exit, 1);
        assert!(!path_lexists(&bundle));
        seam.assert_done();
    }

    #[test]
    fn missing_home_attestation_fails_before_writing() {
        let temp = temp_dir("attestation");
        let config = temp.join("config");
        let env = base_env(&config, &temp.join("home"));
        let root = temp.join("journal");
        let bundle = config.join("solstone-observer").join("spl").join("laptop");
        let mut returned = credential(Value::Array(Vec::new()));
        returned.home_attestation = None;
        let seam = answering_seam(vec![ExpectedLinkJoinPairingCall::Direct {
            expected: expected_direct_request("laptop"),
            result: Ok(returned),
        }]);
        let clock = FakeClock::at_unix(0);

        let output = run(
            &["--code", &direct_pair_link(), "--label", "laptop"],
            &env,
            &root,
            &seam,
            &clock,
        );

        assert_eq!(output.stderr, "Pair response missing home_attestation\n");
        assert_eq!(output.exit, 1);
        assert!(!path_lexists(&bundle));
        seam.assert_done();
    }

    #[test]
    fn peer_json_byte_oracles_cover_non_ascii_and_nested_endpoint_order() {
        let mut non_ascii = credential(json!([
            {"endpoint": "réseau-local", "port": 7657, "scope": "lan"}
        ]));
        non_ascii.home_label = "Hôme".to_string();
        assert_eq!(
            peer_json(
                "café",
                "1970-01-01T00:00:00Z".to_string(),
                &non_ascii,
                non_ascii.local_endpoints.clone(),
            ),
            PEER_NON_ASCII_JSON
        );

        let nested = credential(json!([
            {
                "ip": "10.0.0.2",
                "port": 7657,
                "scope": "lan",
                "meta": {"first": "one", "second": ["two", {"third": "three"}]}
            }
        ]));
        assert_eq!(
            peer_json(
                "laptop",
                "1970-01-01T00:00:00Z".to_string(),
                &nested,
                nested.local_endpoints.clone(),
            ),
            NESTED_ENDPOINTS_JSON
        );
    }

    #[test]
    fn pairing_error_text_covers_every_kind_without_secret_leaks() {
        let mut kinds = vec![
            LinkJoinPairingErrorKind::Io,
            LinkJoinPairingErrorKind::Tls,
            LinkJoinPairingErrorKind::TlsAccessDenied,
            LinkJoinPairingErrorKind::TlsCertificateUnknown,
            LinkJoinPairingErrorKind::Crypto,
            LinkJoinPairingErrorKind::Mux,
            LinkJoinPairingErrorKind::Http,
            LinkJoinPairingErrorKind::Json,
            LinkJoinPairingErrorKind::PairLink,
            LinkJoinPairingErrorKind::Pairing,
            LinkJoinPairingErrorKind::PairResponseMissingHomeAttestation,
            LinkJoinPairingErrorKind::Rejected { status: 403 },
            LinkJoinPairingErrorKind::RelayControlRejected {
                endpoint: LinkJoinRelayControlEndpoint::EnrollDevice,
                status: 403,
            },
            LinkJoinPairingErrorKind::RelayControlRejected {
                endpoint: LinkJoinRelayControlEndpoint::TokenRefresh,
                status: 403,
            },
            LinkJoinPairingErrorKind::NoEndpoint,
            LinkJoinPairingErrorKind::NotPaired,
            LinkJoinPairingErrorKind::LocalOffset,
            LinkJoinPairingErrorKind::RuntimeUnavailable,
        ];
        kinds.extend(
            [
                LinkJoinRelayErrorKind::HomeOffline,
                LinkJoinRelayErrorKind::Unauthorized,
                LinkJoinRelayErrorKind::Unpaid,
                LinkJoinRelayErrorKind::UnknownInstance,
                LinkJoinRelayErrorKind::PairWindowClosed,
                LinkJoinRelayErrorKind::Overflow,
                LinkJoinRelayErrorKind::Abnormal,
                LinkJoinRelayErrorKind::UpgradeRejected,
                LinkJoinRelayErrorKind::Stalled,
            ]
            .into_iter()
            .map(LinkJoinPairingErrorKind::Relay),
        );

        for kind in kinds {
            let text = pairing_error_text("en-US", LinkJoinPairingError::new(kind.clone()));
            assert!(!text.trim().is_empty(), "{kind:?}");
            assert_no_secret_substrings(&text);
            let code = transport_error_code(kind);
            assert!(!code.trim().is_empty());
            assert_no_secret_substrings(code);
        }
    }

    #[test]
    fn home_override_requires_host_and_explicit_port() {
        assert_eq!(
            parse_home_target("en-US", "https://").expect_err("missing host"),
            "Pair-link target missing host."
        );
        assert_eq!(
            parse_home_target("en-US", "https://home.local").expect_err("missing port"),
            "Pair-link target missing explicit port."
        );
        assert_eq!(
            parse_home_target("en-US", "https://home.local:7657/path?ignored=true")
                .expect("target"),
            LinkJoinPairTarget {
                host: "home.local".to_string(),
                port: 7657,
            }
        );
    }

    fn run_status(
        args: &[&str],
        env: &BTreeMap<String, String>,
        probe: Option<&dyn crate::seam::LinkStatusProbe>,
    ) -> CommandOutput {
        let argv = Box::leak(string_args(args).into_boxed_slice());
        let transport = Box::leak(Box::new(ScriptedHttpTransport::new(vec![])));
        link_status(CommandContext {
            args: argv,
            env,
            stdin: "".into(),
            transport,
            clock: None,
            files: None,
            build_identity: None,
            client_item_ids: None,
            notification_sink: None,
            link_pairing: None,
            link_serve: None,
            link_status_probe: probe,
        })
    }

    struct UnreadablePinRunner;

    impl crate::seam::LinkServeRunner for UnreadablePinRunner {
        fn start(
            &self,
            _request: crate::seam::LinkServeRequest,
        ) -> Result<Box<dyn crate::seam::LinkServeSession>, crate::seam::LinkServeError> {
            unreachable!("status never starts a session")
        }

        fn pinned_ca_jid(&self, _ca_chain_pem: &[String]) -> Option<String> {
            None
        }
    }

    // Falsified by status reading the chain's head itself: a certificate that copies a key
    // reads as the journal there while serve and confirm refuse it.
    #[test]
    fn status_holds_a_pairing_serve_would_hold() {
        let temp = temp_dir("status-same-check-as-serve");
        let config = temp.join("config");
        let env = base_env(&config, &temp.join("home"));
        serve_bundle(&config, "alpha", json!([]));
        let argv = Box::leak(string_args(&["--label", "alpha"]).into_boxed_slice());
        let transport = Box::leak(Box::new(ScriptedHttpTransport::new(vec![])));
        let output = link_status(CommandContext {
            args: argv,
            env: &env,
            stdin: "".into(),
            transport,
            clock: None,
            files: None,
            build_identity: None,
            client_item_ids: None,
            notification_sink: None,
            link_pairing: None,
            link_serve: Some(&UnreadablePinRunner),
            link_status_probe: None,
        });
        assert_eq!(output.exit, 0);
        assert!(output.stdout.contains(&held_lines("en-US", "alpha")));
    }

    #[test]
    fn status_help_and_usage_errors() {
        let temp = temp_dir("status-help");
        let config = temp.join("config");
        let env = base_env(&config, &temp.join("home"));

        let help = run_status(&["--help"], &env, None);
        assert_eq!(help.exit, 0);
        assert_eq!(help.stdout, STATUS_HELP);

        let help_short = run_status(&["-h"], &env, None);
        assert_eq!(help_short.exit, 0);
        assert_eq!(help_short.stdout, STATUS_HELP);

        let unknown = run_status(&["--unknown"], &env, None);
        assert_eq!(unknown.exit, 2);
        assert_eq!(
            unknown.stderr,
            format!(
                "{STATUS_USAGE}solstone link status: error: unrecognized arguments: --unknown\n"
            )
        );

        let missing_label = run_status(&["--label"], &env, None);
        assert_eq!(missing_label.exit, 2);
        assert_eq!(
            missing_label.stderr,
            format!(
                "{STATUS_USAGE}solstone link status: error: argument --label: expected one argument\n"
            )
        );
    }

    #[test]
    fn status_no_bundles_fails_with_clean_error() {
        let temp = temp_dir("status-no-bundles");
        let config = temp.join("config");
        let env = base_env(&config, &temp.join("home"));

        let output = run_status(&[], &env, None);
        assert_eq!(output.exit, 1);
        let root = config.join("solstone-observer").join("spl");
        assert!(output.stderr.contains(&root.display().to_string()));
    }

    #[test]
    fn status_multiple_bundles_without_label_fails() {
        let temp = temp_dir("status-multi-bundles");
        let config = temp.join("config");
        let env = base_env(&config, &temp.join("home"));
        serve_bundle(&config, "alpha", json!([]));
        serve_bundle(&config, "beta", json!([]));

        let output = run_status(&[], &env, None);
        assert_eq!(output.exit, 1);
        let alpha_offset = output.stderr.find("alpha").expect("must name alpha");
        let beta_offset = output.stderr.find("beta").expect("must name beta");
        assert!(alpha_offset < beta_offset);
    }

    #[test]
    fn status_stopped_no_metadata() {
        let temp = temp_dir("status-stopped-no-meta");
        let config = temp.join("config");
        let env = base_env(&config, &temp.join("home"));
        serve_bundle(&config, "alpha", json!([]));

        let output = run_status(&[], &env, None);
        assert_eq!(output.exit, 0);
        assert_eq!(
            output.stdout,
            "Label: alpha\nStatus: stopped\nJournal version: unknown\n"
        );
    }

    #[test]
    fn status_stopped_with_cached_metadata() {
        let temp = temp_dir("status-stopped-cached");
        let config = temp.join("config");
        let env = base_env(&config, &temp.join("home"));
        serve_bundle(&config, "alpha", json!([]));

        let bundle_dir = config.join("solstone-observer").join("spl").join("alpha");
        let meta = LinkJournalMetadata {
            instance_id: "32eb4572-27d1-8392-baa2-6e4ea0b1c953".to_string(),
            ca_fp_prefix: serve_bundle_ca_fp_prefix(),
            paired_at: "2026-07-26T00:00:00Z".to_string(),
            journal_version: "2026.07.26".to_string(),
            journal_name: None,
            observed_at: 1234.0,
        };
        fs::write(
            bundle_dir.join("journal_metadata.json"),
            serde_json::to_vec(&meta).expect("serialize"),
        )
        .expect("write");

        let output = run_status(&[], &env, None);
        assert_eq!(output.exit, 0);
        assert_eq!(
            output.stdout,
            "Label: alpha\nStatus: stopped\nJournal version: 2026.07.26 (last known)\n"
        );
    }

    #[test]
    fn status_stopped_with_mismatched_cached_metadata() {
        let temp = temp_dir("status-stopped-mismatch");
        let config = temp.join("config");
        let env = base_env(&config, &temp.join("home"));
        serve_bundle(&config, "alpha", json!([]));

        let bundle_dir = config.join("solstone-observer").join("spl").join("alpha");
        let meta = LinkJournalMetadata {
            instance_id: "other-instance".to_string(),
            ca_fp_prefix: serve_bundle_ca_fp_prefix(),
            paired_at: "2026-07-26T00:00:00Z".to_string(),
            journal_version: "2026.07.26".to_string(),
            journal_name: None,
            observed_at: 1234.0,
        };
        fs::write(
            bundle_dir.join("journal_metadata.json"),
            serde_json::to_vec(&meta).expect("serialize"),
        )
        .expect("write");

        let output = run_status(&[], &env, None);
        assert_eq!(output.exit, 0);
        assert_eq!(
            output.stdout,
            "Label: alpha\nStatus: stopped\nJournal version: unknown\n"
        );
    }

    #[test]
    fn status_running_fresh_version() {
        let temp = temp_dir("status-running-fresh");
        let config = temp.join("config");
        let env = base_env(&config, &temp.join("home"));
        serve_bundle(&config, "alpha", json!([]));

        let bundle_dir = config.join("solstone-observer").join("spl").join("alpha");
        let runtime = LinkServeRuntimeRecord { port: 5015 };
        fs::write(
            bundle_dir.join("serve_runtime.json"),
            serde_json::to_vec(&runtime).expect("serialize"),
        )
        .expect("write");

        let snapshot = LinkServeStatusSnapshot {
            state: "connected".to_string(),
            health: "ok".to_string(),
            manager_alive: true,
            active_requests: 0,
            reconnect_count: 0,
            last_connected_at: Some(100.0),
            connected_age_seconds: Some(10.0),
            last_failure: None,
            next_retry_at: None,
            journal_version: Some("2026.07.26".to_string()),
            journal_version_fresh: true,
            instance_id: "32eb4572-27d1-8392-baa2-6e4ea0b1c953".to_string(),
            ca_fp_prefix: serve_bundle_ca_fp_prefix(),
            paired_at: "2026-07-26T00:00:00Z".to_string(),
            persist_uncertain: false,
            unknown_journals: Vec::new(),
        };
        let response = crate::seam::HttpResponse {
            status: 200,
            headers: vec![],
            body: serde_json::to_vec(&snapshot).expect("serialize"),
            policy: crate::seam::TimeoutPolicy::Api,
        };
        let probe = crate::seam::ScriptedLinkStatusProbe::new(vec![(5015, Ok(response))]);

        let output = run_status(&[], &env, Some(&probe));
        assert_eq!(output.exit, 0);
        assert_eq!(
            output.stdout,
            "Label: alpha\nStatus: connected\nJournal version: 2026.07.26\n"
        );
        assert_eq!(probe.recorded(), vec![5015]);
    }

    #[test]
    fn status_running_stale_version_in_snapshot() {
        let temp = temp_dir("status-running-stale");
        let config = temp.join("config");
        let env = base_env(&config, &temp.join("home"));
        serve_bundle(&config, "alpha", json!([]));

        let bundle_dir = config.join("solstone-observer").join("spl").join("alpha");
        let runtime = LinkServeRuntimeRecord { port: 5015 };
        fs::write(
            bundle_dir.join("serve_runtime.json"),
            serde_json::to_vec(&runtime).expect("serialize"),
        )
        .expect("write");

        let snapshot = LinkServeStatusSnapshot {
            state: "disconnected".to_string(),
            health: "offline".to_string(),
            manager_alive: true,
            active_requests: 0,
            reconnect_count: 1,
            last_connected_at: None,
            connected_age_seconds: None,
            last_failure: None,
            next_retry_at: None,
            journal_version: Some("2026.07.26".to_string()),
            journal_version_fresh: false,
            instance_id: "32eb4572-27d1-8392-baa2-6e4ea0b1c953".to_string(),
            ca_fp_prefix: serve_bundle_ca_fp_prefix(),
            paired_at: "2026-07-26T00:00:00Z".to_string(),
            persist_uncertain: false,
            unknown_journals: Vec::new(),
        };
        let response = crate::seam::HttpResponse {
            status: 200,
            headers: vec![],
            body: serde_json::to_vec(&snapshot).expect("serialize"),
            policy: crate::seam::TimeoutPolicy::Api,
        };
        let probe = crate::seam::ScriptedLinkStatusProbe::new(vec![(5015, Ok(response))]);

        let output = run_status(&[], &env, Some(&probe));
        assert_eq!(output.exit, 0);
        assert_eq!(
            output.stdout,
            "Label: alpha\nStatus: disconnected\nJournal version: 2026.07.26 (last known)\n"
        );
        assert_eq!(probe.recorded(), vec![5015]);
    }

    // Falsified by printing the unpaired state without its guidance: the owner learns the link
    // stopped but not what to do about it.
    #[test]
    fn status_running_unpaired_says_how_to_pair_again() {
        let temp = temp_dir("status-running-unpaired");
        let config = temp.join("config");
        let env = base_env(&config, &temp.join("home"));
        serve_bundle(&config, "alpha", json!([]));

        let bundle_dir = config.join("solstone-observer").join("spl").join("alpha");
        let runtime = LinkServeRuntimeRecord { port: 5015 };
        fs::write(
            bundle_dir.join("serve_runtime.json"),
            serde_json::to_vec(&runtime).expect("serialize"),
        )
        .expect("write");

        let snapshot = LinkServeStatusSnapshot {
            state: "unpaired".to_string(),
            health: "unhealthy".to_string(),
            manager_alive: true,
            active_requests: 0,
            reconnect_count: 1,
            last_connected_at: None,
            connected_age_seconds: None,
            last_failure: None,
            next_retry_at: None,
            journal_version: None,
            journal_version_fresh: false,
            instance_id: "32eb4572-27d1-8392-baa2-6e4ea0b1c953".to_string(),
            ca_fp_prefix: serve_bundle_ca_fp_prefix(),
            paired_at: "2026-07-26T00:00:00Z".to_string(),
            persist_uncertain: false,
            unknown_journals: Vec::new(),
        };
        let response = crate::seam::HttpResponse {
            status: 200,
            headers: vec![],
            body: serde_json::to_vec(&snapshot).expect("serialize"),
            policy: crate::seam::TimeoutPolicy::Api,
        };
        let probe = crate::seam::ScriptedLinkStatusProbe::new(vec![(5015, Ok(response))]);

        let output = run_status(&[], &env, Some(&probe));
        assert_eq!(output.exit, 0);
        assert!(
            output.stdout.contains("Status: unpaired\n"),
            "{}",
            output.stdout
        );
        assert!(
            output
                .stdout
                .ends_with(&unpaired_note("en-US", &bundle_dir)),
            "{}",
            output.stdout
        );
        assert!(
            output.stdout.contains(&bundle_dir.display().to_string()),
            "{}",
            output.stdout
        );
    }

    #[test]
    fn status_running_probe_failed_falls_back_to_metadata() {
        let temp = temp_dir("status-probe-failed");
        let config = temp.join("config");
        let env = base_env(&config, &temp.join("home"));
        serve_bundle(&config, "alpha", json!([]));

        let bundle_dir = config.join("solstone-observer").join("spl").join("alpha");
        let runtime = LinkServeRuntimeRecord { port: 5015 };
        fs::write(
            bundle_dir.join("serve_runtime.json"),
            serde_json::to_vec(&runtime).expect("serialize"),
        )
        .expect("write");

        let meta = LinkJournalMetadata {
            instance_id: "32eb4572-27d1-8392-baa2-6e4ea0b1c953".to_string(),
            ca_fp_prefix: serve_bundle_ca_fp_prefix(),
            paired_at: "2026-07-26T00:00:00Z".to_string(),
            journal_version: "2026.07.26".to_string(),
            journal_name: None,
            observed_at: 1234.0,
        };
        fs::write(
            bundle_dir.join("journal_metadata.json"),
            serde_json::to_vec(&meta).expect("serialize"),
        )
        .expect("write");

        let probe = crate::seam::ScriptedLinkStatusProbe::new(vec![(
            5015,
            Err(crate::error::ClientError::unreachable(Some(
                "connection refused".to_string(),
            ))),
        )]);

        let output = run_status(&[], &env, Some(&probe));
        assert_eq!(output.exit, 0);
        assert_eq!(
            output.stdout,
            "Label: alpha\nStatus: stopped\nJournal version: 2026.07.26 (last known)\n"
        );
        assert_eq!(probe.recorded(), vec![5015]);
    }

    #[test]
    fn status_running_cross_pairing_mismatch_falls_back() {
        let temp = temp_dir("status-cross-pairing");
        let config = temp.join("config");
        let env = base_env(&config, &temp.join("home"));
        serve_bundle(&config, "alpha", json!([]));

        let bundle_dir = config.join("solstone-observer").join("spl").join("alpha");
        let runtime = LinkServeRuntimeRecord { port: 5015 };
        fs::write(
            bundle_dir.join("serve_runtime.json"),
            serde_json::to_vec(&runtime).expect("serialize"),
        )
        .expect("write");

        let snapshot = LinkServeStatusSnapshot {
            state: "connected".to_string(),
            health: "ok".to_string(),
            manager_alive: true,
            active_requests: 0,
            reconnect_count: 0,
            last_connected_at: Some(100.0),
            connected_age_seconds: Some(10.0),
            last_failure: None,
            next_retry_at: None,
            journal_version: Some("2026.07.26".to_string()),
            journal_version_fresh: true,
            instance_id: "other-rogue-instance".to_string(),
            ca_fp_prefix: serve_bundle_ca_fp_prefix(),
            paired_at: "2026-07-26T00:00:00Z".to_string(),
            persist_uncertain: false,
            unknown_journals: Vec::new(),
        };
        let response = crate::seam::HttpResponse {
            status: 200,
            headers: vec![],
            body: serde_json::to_vec(&snapshot).expect("serialize"),
            policy: crate::seam::TimeoutPolicy::Api,
        };
        let probe = crate::seam::ScriptedLinkStatusProbe::new(vec![(5015, Ok(response))]);

        let output = run_status(&[], &env, Some(&probe));
        assert_eq!(output.exit, 0);
        assert_eq!(
            output.stdout,
            "Label: alpha\nStatus: stopped\nJournal version: unknown\n"
        );
        assert_eq!(probe.recorded(), vec![5015]);
    }

    #[test]
    fn status_running_cross_pairing_ca_fp_mismatch_falls_back() {
        let temp = temp_dir("status-cross-pairing-ca-fp");
        let config = temp.join("config");
        let env = base_env(&config, &temp.join("home"));
        serve_bundle(&config, "alpha", json!([]));

        let bundle_dir = config.join("solstone-observer").join("spl").join("alpha");
        let runtime = LinkServeRuntimeRecord { port: 5015 };
        fs::write(
            bundle_dir.join("serve_runtime.json"),
            serde_json::to_vec(&runtime).expect("serialize"),
        )
        .expect("write");

        let meta = LinkJournalMetadata {
            instance_id: "32eb4572-27d1-8392-baa2-6e4ea0b1c953".to_string(),
            ca_fp_prefix: serve_bundle_ca_fp_prefix(),
            paired_at: "2026-07-26T00:00:00Z".to_string(),
            journal_version: "2026.07.26".to_string(),
            journal_name: None,
            observed_at: 1234.0,
        };
        fs::write(
            bundle_dir.join("journal_metadata.json"),
            serde_json::to_vec(&meta).expect("serialize"),
        )
        .expect("write");

        // Live probe returns matching instance_id and paired_at but a
        // ca_fp_prefix that does not match the bundle's actual chain — must
        // be rejected even though a persisted cache exists with the correct
        // value.
        let snapshot = LinkServeStatusSnapshot {
            state: "connected".to_string(),
            health: "ok".to_string(),
            manager_alive: true,
            active_requests: 0,
            reconnect_count: 0,
            last_connected_at: Some(100.0),
            connected_age_seconds: Some(10.0),
            last_failure: None,
            next_retry_at: None,
            journal_version: Some("2026.07.26".to_string()),
            journal_version_fresh: true,
            instance_id: "32eb4572-27d1-8392-baa2-6e4ea0b1c953".to_string(),
            ca_fp_prefix: "different-ca-fp".to_string(),
            paired_at: "2026-07-26T00:00:00Z".to_string(),
            persist_uncertain: false,
            unknown_journals: Vec::new(),
        };
        let response = crate::seam::HttpResponse {
            status: 200,
            headers: vec![],
            body: serde_json::to_vec(&snapshot).expect("serialize"),
            policy: crate::seam::TimeoutPolicy::Api,
        };
        let probe = crate::seam::ScriptedLinkStatusProbe::new(vec![(5015, Ok(response))]);

        let output = run_status(&[], &env, Some(&probe));
        assert_eq!(output.exit, 0);
        // Rejected live snapshot, fell back to persisted metadata
        assert_eq!(
            output.stdout,
            "Label: alpha\nStatus: stopped\nJournal version: 2026.07.26 (last known)\n"
        );
        assert_eq!(probe.recorded(), vec![5015]);
    }

    #[test]
    fn status_running_ca_fp_mismatch_with_no_cached_metadata_is_rejected() {
        // No journal_metadata.json exists at all here. The identity check
        // must still be enforced against the bundle's own certificate chain
        // rather than being skipped for lack of anything cached to compare
        // against.
        let temp = temp_dir("status-ca-mismatch-no-cache");
        let config = temp.join("config");
        let env = base_env(&config, &temp.join("home"));
        serve_bundle(&config, "alpha", json!([]));

        let bundle_dir = config.join("solstone-observer").join("spl").join("alpha");
        let runtime = LinkServeRuntimeRecord { port: 5015 };
        fs::write(
            bundle_dir.join("serve_runtime.json"),
            serde_json::to_vec(&runtime).expect("serialize"),
        )
        .expect("write");

        let snapshot = LinkServeStatusSnapshot {
            state: "connected".to_string(),
            health: "ok".to_string(),
            manager_alive: true,
            active_requests: 0,
            reconnect_count: 0,
            last_connected_at: Some(100.0),
            connected_age_seconds: Some(10.0),
            last_failure: None,
            next_retry_at: None,
            journal_version: Some("2026.07.26".to_string()),
            journal_version_fresh: true,
            instance_id: "32eb4572-27d1-8392-baa2-6e4ea0b1c953".to_string(),
            ca_fp_prefix: "rogue-ca-fp".to_string(),
            paired_at: "2026-07-26T00:00:00Z".to_string(),
            persist_uncertain: false,
            unknown_journals: Vec::new(),
        };
        let response = crate::seam::HttpResponse {
            status: 200,
            headers: vec![],
            body: serde_json::to_vec(&snapshot).expect("serialize"),
            policy: crate::seam::TimeoutPolicy::Api,
        };
        let probe = crate::seam::ScriptedLinkStatusProbe::new(vec![(5015, Ok(response))]);

        let output = run_status(&[], &env, Some(&probe));
        assert_eq!(output.exit, 0);
        assert_eq!(
            output.stdout,
            "Label: alpha\nStatus: stopped\nJournal version: unknown\n"
        );
        assert_eq!(probe.recorded(), vec![5015]);
    }

    #[test]
    fn status_running_stale_paired_at_after_repair_is_rejected() {
        // Same instance_id and same CA (a same-identity re-pair does not
        // necessarily rotate the CA) but a paired_at from before the bundle
        // was replaced. An orphaned serve process for the old pairing must
        // not be trusted just because instance_id and ca_fp_prefix survived
        // the re-pair unchanged.
        let temp = temp_dir("status-stale-paired-at");
        let config = temp.join("config");
        let env = base_env(&config, &temp.join("home"));
        serve_bundle(&config, "alpha", json!([]));

        let bundle_dir = config.join("solstone-observer").join("spl").join("alpha");
        let runtime = LinkServeRuntimeRecord { port: 5015 };
        fs::write(
            bundle_dir.join("serve_runtime.json"),
            serde_json::to_vec(&runtime).expect("serialize"),
        )
        .expect("write");

        let meta = LinkJournalMetadata {
            instance_id: "32eb4572-27d1-8392-baa2-6e4ea0b1c953".to_string(),
            ca_fp_prefix: serve_bundle_ca_fp_prefix(),
            paired_at: "stale-paired-at-before-repair".to_string(),
            journal_version: "2026.06.01".to_string(),
            journal_name: None,
            observed_at: 1000.0,
        };
        fs::write(
            bundle_dir.join("journal_metadata.json"),
            serde_json::to_vec(&meta).expect("serialize"),
        )
        .expect("write");

        let snapshot = LinkServeStatusSnapshot {
            state: "connected".to_string(),
            health: "ok".to_string(),
            manager_alive: true,
            active_requests: 0,
            reconnect_count: 0,
            last_connected_at: Some(100.0),
            connected_age_seconds: Some(10.0),
            last_failure: None,
            next_retry_at: None,
            journal_version: Some("2026.06.01".to_string()),
            journal_version_fresh: true,
            instance_id: "32eb4572-27d1-8392-baa2-6e4ea0b1c953".to_string(),
            ca_fp_prefix: serve_bundle_ca_fp_prefix(),
            paired_at: "stale-paired-at-before-repair".to_string(),
            persist_uncertain: false,
            unknown_journals: Vec::new(),
        };
        let response = crate::seam::HttpResponse {
            status: 200,
            headers: vec![],
            body: serde_json::to_vec(&snapshot).expect("serialize"),
            policy: crate::seam::TimeoutPolicy::Api,
        };
        let probe = crate::seam::ScriptedLinkStatusProbe::new(vec![(5015, Ok(response))]);

        let output = run_status(&[], &env, Some(&probe));
        assert_eq!(output.exit, 0);
        // Both the live snapshot and the cache predate the re-pair recorded
        // in the bundle's own peer.json (paired_at "2026-07-26T00:00:00Z"),
        // so neither is trusted.
        assert_eq!(
            output.stdout,
            "Label: alpha\nStatus: stopped\nJournal version: unknown\n"
        );
        assert_eq!(probe.recorded(), vec![5015]);
    }

    #[test]
    fn status_live_probe_rendered_output_contains_no_raw_control_or_escape_bytes() {
        let temp = temp_dir("status-live-sanitize");
        let config = temp.join("config");
        let env = base_env(&config, &temp.join("home"));
        serve_bundle(&config, "alpha", json!([]));

        let bundle_dir = config.join("solstone-observer").join("spl").join("alpha");
        let runtime = LinkServeRuntimeRecord { port: 5015 };
        fs::write(
            bundle_dir.join("serve_runtime.json"),
            serde_json::to_vec(&runtime).expect("serialize"),
        )
        .expect("write");

        let snapshot = LinkServeStatusSnapshot {
            state: "connected".to_string(),
            health: "ok".to_string(),
            manager_alive: true,
            active_requests: 0,
            reconnect_count: 0,
            last_connected_at: Some(100.0),
            connected_age_seconds: Some(10.0),
            last_failure: None,
            next_retry_at: None,
            journal_version: Some("\x1b[31m2026.07.26\x1b[0m\x00\x07".to_string()),
            journal_version_fresh: true,
            instance_id: "32eb4572-27d1-8392-baa2-6e4ea0b1c953".to_string(),
            ca_fp_prefix: serve_bundle_ca_fp_prefix(),
            paired_at: "2026-07-26T00:00:00Z".to_string(),
            persist_uncertain: false,
            unknown_journals: Vec::new(),
        };
        let response = crate::seam::HttpResponse {
            status: 200,
            headers: vec![],
            body: serde_json::to_vec(&snapshot).expect("serialize"),
            policy: crate::seam::TimeoutPolicy::Api,
        };
        let probe = crate::seam::ScriptedLinkStatusProbe::new(vec![(5015, Ok(response))]);

        let output = run_status(&[], &env, Some(&probe));
        assert_eq!(output.exit, 0);
        assert_eq!(
            output.stdout,
            "Label: alpha\nStatus: connected\nJournal version: [31m2026.07.26[0m\n"
        );
        // Ensure no raw ESC or control characters exist anywhere in stdout except newline
        for byte in output.stdout.bytes() {
            assert!(
                byte == b'\n' || (byte >= 0x20 && byte != 0x7F && byte != 0x1b),
                "unexpected byte {:#x} in output stdout",
                byte
            );
        }
    }

    #[test]
    fn status_sanitizes_control_and_escape_chars() {
        assert_eq!(
            sanitize_display_version("\x1b[31m2026.07.26\x1b[0m\r\n"),
            "[31m2026.07.26[0m"
        );
        assert_eq!(sanitize_display_version("2026.07.26\0\x07"), "2026.07.26");
    }

    #[test]
    fn link_join_cleans_metadata_and_runtime() {
        let temp = temp_dir("join-cleanup");
        let config = temp.join("config");
        let env = base_env(&config, &temp.join("home"));
        let root = temp.join("journal");
        let bundle_dir = config.join("solstone-observer").join("spl").join("laptop");

        let seam = answering_seam(vec![ExpectedLinkJoinPairingCall::Direct {
            expected: expected_direct_request("laptop"),
            result: Ok(credential(Value::Array(Vec::new()))),
        }]);
        let clock = FakeClock::at_unix(0);

        let output = run(
            &["--code", &direct_pair_link(), "--label", "laptop"],
            &env,
            &root,
            &seam,
            &clock,
        );
        assert_eq!(output.exit, 0);
        assert!(!bundle_dir.join("journal_metadata.json").exists());
        assert!(!bundle_dir.join("serve_runtime.json").exists());
        seam.assert_done();
    }

    #[test]
    fn status_reports_sidecar_persist_uncertainty_warning() {
        let temp = temp_dir("status-persist-uncertain");
        let config = temp.join("config");
        let env = base_env(&config, &temp.join("home"));
        serve_bundle(&config, "alpha", json!([]));

        let bundle_dir = config.join("solstone-observer").join("spl").join("alpha");
        let runtime = LinkServeRuntimeRecord { port: 5015 };
        fs::write(
            bundle_dir.join("serve_runtime.json"),
            serde_json::to_vec(&runtime).expect("serialize"),
        )
        .expect("write");

        let snapshot = LinkServeStatusSnapshot {
            state: "connected".to_string(),
            health: "ok".to_string(),
            manager_alive: true,
            active_requests: 0,
            reconnect_count: 0,
            last_connected_at: Some(100.0),
            connected_age_seconds: Some(10.0),
            last_failure: None,
            next_retry_at: None,
            journal_version: Some("2026.07.26".to_string()),
            journal_version_fresh: true,
            instance_id: "32eb4572-27d1-8392-baa2-6e4ea0b1c953".to_string(),
            ca_fp_prefix: serve_bundle_ca_fp_prefix(),
            paired_at: "2026-07-26T00:00:00Z".to_string(),
            persist_uncertain: true,
            unknown_journals: Vec::new(),
        };
        let response = crate::seam::HttpResponse {
            status: 200,
            headers: vec![],
            body: serde_json::to_vec(&snapshot).expect("serialize"),
            policy: crate::seam::TimeoutPolicy::Api,
        };
        let probe = crate::seam::ScriptedLinkStatusProbe::new(vec![(5015, Ok(response))]);

        let output = run_status(&[], &env, Some(&probe));
        assert_eq!(output.exit, 0);
        assert!(
            output
                .stdout
                .contains("Warning: connection changes may not have been saved.")
        );
        assert_eq!(probe.recorded(), vec![5015]);
    }

    #[test]
    fn join_with_pairing_token_persists_relay_access_and_load_serve_bundle_returns_ready() {
        let temp = temp_dir("join-relay-access");
        let config = temp.join("config");
        let env = base_env(&config, &temp.join("home"));
        let root = temp.join("journal");
        let bundle = config.join("solstone-observer").join("spl").join("laptop");
        let mut cred = credential(json!([{"ip": "10.0.0.2", "port": 7657}]));
        cred.client_cert_pem = SERVE_BUNDLE_CERT.to_string();
        cred.ca_chain_pem = vec![SERVE_BUNDLE_CERT.to_string()];
        let der = crate::link_credentials::pem_cert_der(SERVE_BUNDLE_CERT).unwrap();
        cred.ca_fingerprint = format!("sha256:{}", spl_core::ca::sha256_hex(&der));
        cred.relay_origin = Some("https://link.solstone.app".to_string());
        cred.relay_device_token = Some("mock_token_abc".to_string());
        cred.relay_device_token_expires_at = Some(2000000000);

        let seam = answering_seam(vec![ExpectedLinkJoinPairingCall::Direct {
            expected: expected_direct_request("laptop"),
            result: Ok(cred),
        }]);
        let clock = FakeClock::at_unix(0);

        let output = run(
            &["--code", &direct_pair_link(), "--label", "laptop"],
            &env,
            &root,
            &seam,
            &clock,
        );
        assert_eq!(output.exit, 0);
        assert!(bundle.join("relay_access.json").is_file());

        let loaded = load_serve_bundle("en-US", &bundle).expect("load serve bundle");
        match loaded.relay_access {
            Some(crate::link_credentials::StoreLoadOutcome::Ready(record)) => {
                assert_eq!(
                    record.relay_origin.as_deref(),
                    Some("https://link.solstone.app")
                );
                assert_eq!(record.device_token.as_deref(), Some("mock_token_abc"));
                assert_eq!(record.expires_at, Some(2000000000));
            }
            other => panic!("expected Ready outcome, got {other:?}"),
        }
    }

    #[test]
    fn old_bundle_without_relay_access_still_loads_lan() {
        let temp = temp_dir("old-lan-bundle");
        let config = temp.join("config");
        let bundle_dir = config.join("solstone-observer").join("spl").join("laptop");
        let _ = serve_bundle(
            &config,
            "laptop",
            json!([{"ip": "192.168.1.50", "port": 7657}]),
        );

        // Ensure relay_access.json does not exist
        let _ = fs::remove_file(bundle_dir.join("relay_access.json"));

        let loaded = load_serve_bundle("en-US", &bundle_dir).expect("load old bundle");
        assert_eq!(
            loaded.relay_access,
            Some(crate::link_credentials::StoreLoadOutcome::Absent)
        );
        assert_eq!(loaded.endpoints.len(), 1);
        assert_eq!(loaded.endpoints[0].host, "192.168.1.50");
        assert_eq!(loaded.endpoints[0].port, 7657);
    }

    #[test]
    fn status_running_unknown_journal_address_and_jid_prints_template() {
        let temp = temp_dir("status-unknown-addr-jid");
        let config = temp.join("config");
        let env = base_env(&config, &temp.join("home"));
        let local_id = "32eb4572-27d1-8392-baa2-6e4ea0b1c953";
        serve_bundle_with_instance_id(&config, "alpha", json!([]), local_id);

        let bundle_dir = config.join("solstone-observer").join("spl").join("alpha");
        let runtime = LinkServeRuntimeRecord { port: 5015 };
        fs::write(
            bundle_dir.join("serve_runtime.json"),
            serde_json::to_vec(&runtime).expect("serialize"),
        )
        .expect("write");

        let other_jid = "01957597-2a5c-7d92-8ec2-e0f3408a0d9b";
        let other_mark = crate::mark_text::mark_from_jid(other_jid).expect("other mark");
        let other_spoken = format_spoken_mark(&other_mark.to_render_spec());
        let local_mark = crate::mark_text::mark_from_jid(local_id).expect("local mark");
        let local_spoken = format_spoken_mark(&local_mark.to_render_spec());

        let snapshot = LinkServeStatusSnapshot {
            state: "connected".to_string(),
            health: "ok".to_string(),
            manager_alive: true,
            active_requests: 0,
            reconnect_count: 0,
            last_connected_at: Some(100.0),
            connected_age_seconds: Some(10.0),
            last_failure: None,
            next_retry_at: None,
            journal_version: Some("2026.07.26".to_string()),
            journal_version_fresh: true,
            instance_id: local_id.to_string(),
            ca_fp_prefix: serve_bundle_ca_fp_prefix(),
            paired_at: "2026-07-26T00:00:00Z".to_string(),
            persist_uncertain: false,
            unknown_journals: vec![crate::seam::UnknownJournalSighting {
                address: Some("192.168.1.50:7657".to_string()),
                jid: Some(other_jid.to_string()),
            }],
        };
        let response = crate::seam::HttpResponse {
            status: 200,
            headers: vec![],
            body: serde_json::to_vec(&snapshot).expect("serialize"),
            policy: crate::seam::TimeoutPolicy::Api,
        };
        let probe = crate::seam::ScriptedLinkStatusProbe::new(vec![(5015, Ok(response))]);

        let output = run_status(&[], &env, Some(&probe));
        assert_eq!(output.exit, 0);
        let expected = format!(
            "Label: alpha\nStatus: connected\nJournal version: 2026.07.26\n\nunknown journal seen at 192.168.1.50:7657:\n\nwhat answered:  {other_spoken} (claimed, not verified)\nyour journal:   {local_spoken}\n\n{SIGHTING_RESET_FOOTER}\n"
        );
        assert_eq!(output.stdout, expected);
    }

    #[test]
    fn status_running_unknown_journal_relay_prints_template() {
        let temp = temp_dir("status-unknown-relay");
        let config = temp.join("config");
        let env = base_env(&config, &temp.join("home"));
        let local_id = "32eb4572-27d1-8392-baa2-6e4ea0b1c953";
        serve_bundle_with_instance_id(&config, "alpha", json!([]), local_id);

        let bundle_dir = config.join("solstone-observer").join("spl").join("alpha");
        let runtime = LinkServeRuntimeRecord { port: 5015 };
        fs::write(
            bundle_dir.join("serve_runtime.json"),
            serde_json::to_vec(&runtime).expect("serialize"),
        )
        .expect("write");

        let other_jid = "01957597-2a5c-7d92-8ec2-e0f3408a0d9b";
        let other_mark = crate::mark_text::mark_from_jid(other_jid).expect("other mark");
        let other_spoken = format_spoken_mark(&other_mark.to_render_spec());
        let local_mark = crate::mark_text::mark_from_jid(local_id).expect("local mark");
        let local_spoken = format_spoken_mark(&local_mark.to_render_spec());

        let snapshot = LinkServeStatusSnapshot {
            state: "connected".to_string(),
            health: "ok".to_string(),
            manager_alive: true,
            active_requests: 0,
            reconnect_count: 0,
            last_connected_at: Some(100.0),
            connected_age_seconds: Some(10.0),
            last_failure: None,
            next_retry_at: None,
            journal_version: Some("2026.07.26".to_string()),
            journal_version_fresh: true,
            instance_id: local_id.to_string(),
            ca_fp_prefix: serve_bundle_ca_fp_prefix(),
            paired_at: "2026-07-26T00:00:00Z".to_string(),
            persist_uncertain: false,
            unknown_journals: vec![crate::seam::UnknownJournalSighting {
                address: None,
                jid: Some(other_jid.to_string()),
            }],
        };
        let response = crate::seam::HttpResponse {
            status: 200,
            headers: vec![],
            body: serde_json::to_vec(&snapshot).expect("serialize"),
            policy: crate::seam::TimeoutPolicy::Api,
        };
        let probe = crate::seam::ScriptedLinkStatusProbe::new(vec![(5015, Ok(response))]);

        let output = run_status(&[], &env, Some(&probe));
        assert_eq!(output.exit, 0);
        let expected = format!(
            "Label: alpha\nStatus: connected\nJournal version: 2026.07.26\n\nunknown journal seen through the relay:\n\nwhat answered:  {other_spoken} (claimed, not verified)\nyour journal:   {local_spoken}\n\n{SIGHTING_RELAY_FOOTER}\n"
        );
        assert_eq!(output.stdout, expected);
    }

    #[test]
    fn status_running_unknown_journal_no_jid_prints_no_identity() {
        let temp = temp_dir("status-unknown-no-jid");
        let config = temp.join("config");
        let env = base_env(&config, &temp.join("home"));
        let local_id = "32eb4572-27d1-8392-baa2-6e4ea0b1c953";
        serve_bundle_with_instance_id(&config, "alpha", json!([]), local_id);

        let bundle_dir = config.join("solstone-observer").join("spl").join("alpha");
        let runtime = LinkServeRuntimeRecord { port: 5015 };
        fs::write(
            bundle_dir.join("serve_runtime.json"),
            serde_json::to_vec(&runtime).expect("serialize"),
        )
        .expect("write");

        let local_mark = crate::mark_text::mark_from_jid(local_id).expect("local mark");
        let local_spoken = format_spoken_mark(&local_mark.to_render_spec());

        let snapshot = LinkServeStatusSnapshot {
            state: "connected".to_string(),
            health: "ok".to_string(),
            manager_alive: true,
            active_requests: 0,
            reconnect_count: 0,
            last_connected_at: Some(100.0),
            connected_age_seconds: Some(10.0),
            last_failure: None,
            next_retry_at: None,
            journal_version: Some("2026.07.26".to_string()),
            journal_version_fresh: true,
            instance_id: local_id.to_string(),
            ca_fp_prefix: serve_bundle_ca_fp_prefix(),
            paired_at: "2026-07-26T00:00:00Z".to_string(),
            persist_uncertain: false,
            unknown_journals: vec![crate::seam::UnknownJournalSighting {
                address: Some("192.168.1.50:7657".to_string()),
                jid: None,
            }],
        };
        let response = crate::seam::HttpResponse {
            status: 200,
            headers: vec![],
            body: serde_json::to_vec(&snapshot).expect("serialize"),
            policy: crate::seam::TimeoutPolicy::Api,
        };
        let probe = crate::seam::ScriptedLinkStatusProbe::new(vec![(5015, Ok(response))]);

        let output = run_status(&[], &env, Some(&probe));
        assert_eq!(output.exit, 0);
        let expected = format!(
            "Label: alpha\nStatus: connected\nJournal version: 2026.07.26\n\nunknown journal seen at 192.168.1.50:7657:\n\nwhat answered:  no identity presented\nyour journal:   {local_spoken}\n"
        );
        assert_eq!(output.stdout, expected);
        assert!(!output.stdout.contains("(claimed, not verified)"));
        assert!(!output.stdout.contains(SIGHTING_RESET_FOOTER));
    }

    #[test]
    fn status_running_unknown_journal_invalid_jid_prints_fallback_copy() {
        let temp = temp_dir("status-unknown-invalid-jid");
        let config = temp.join("config");
        let env = base_env(&config, &temp.join("home"));
        let local_id = "32eb4572-27d1-8392-baa2-6e4ea0b1c953";
        serve_bundle_with_instance_id(&config, "alpha", json!([]), local_id);

        let bundle_dir = config.join("solstone-observer").join("spl").join("alpha");
        let runtime = LinkServeRuntimeRecord { port: 5015 };
        fs::write(
            bundle_dir.join("serve_runtime.json"),
            serde_json::to_vec(&runtime).expect("serialize"),
        )
        .expect("write");

        let local_mark = crate::mark_text::mark_from_jid(local_id).expect("local mark");
        let local_spoken = format_spoken_mark(&local_mark.to_render_spec());

        let snapshot = LinkServeStatusSnapshot {
            state: "connected".to_string(),
            health: "ok".to_string(),
            manager_alive: true,
            active_requests: 0,
            reconnect_count: 0,
            last_connected_at: Some(100.0),
            connected_age_seconds: Some(10.0),
            last_failure: None,
            next_retry_at: None,
            journal_version: Some("2026.07.26".to_string()),
            journal_version_fresh: true,
            instance_id: local_id.to_string(),
            ca_fp_prefix: serve_bundle_ca_fp_prefix(),
            paired_at: "2026-07-26T00:00:00Z".to_string(),
            persist_uncertain: false,
            unknown_journals: vec![crate::seam::UnknownJournalSighting {
                address: Some("192.168.1.50:7657".to_string()),
                jid: Some("not-a-valid-uuid".to_string()),
            }],
        };
        let response = crate::seam::HttpResponse {
            status: 200,
            headers: vec![],
            body: serde_json::to_vec(&snapshot).expect("serialize"),
            policy: crate::seam::TimeoutPolicy::Api,
        };
        let probe = crate::seam::ScriptedLinkStatusProbe::new(vec![(5015, Ok(response))]);

        let output = run_status(&[], &env, Some(&probe));
        assert_eq!(output.exit, 0);
        let expected = format!(
            "Label: alpha\nStatus: connected\nJournal version: 2026.07.26\n\nunknown journal seen at 192.168.1.50:7657:\n\nwhat answered:  claimed identity could not be shown (claimed, not verified)\nyour journal:   {local_spoken}\n\n{SIGHTING_RESET_FOOTER}\n"
        );
        assert_eq!(output.stdout, expected);
    }

    #[test]
    fn status_running_unknown_journal_invalid_local_id_prints_fallback_copy() {
        let temp = temp_dir("status-unknown-invalid-local-id");
        let config = temp.join("config");
        let env = base_env(&config, &temp.join("home"));
        // "home-instance" is not a UUID, so mark_from_jid will fail
        serve_bundle_with_instance_id(&config, "alpha", json!([]), "home-instance");
        // No CA to read, and the owner went on without one: no mark from its id.
        let bundle_dir = config.join("solstone-observer").join("spl").join("alpha");
        fs::write(
            bundle_dir.join("chain.pem"),
            "-----BEGIN CERTIFICATE-----\nAA==\n-----END CERTIFICATE-----\n",
        )
        .expect("chain");
        let mut peer: Value =
            serde_json::from_str(&fs::read_to_string(bundle_dir.join("peer.json")).expect("peer"))
                .expect("peer json");
        peer[UNVERIFIED_KEY] = Value::Bool(true);
        fs::write(bundle_dir.join("peer.json"), peer.to_string()).expect("peer");

        let bundle_dir = config.join("solstone-observer").join("spl").join("alpha");
        let runtime = LinkServeRuntimeRecord { port: 5015 };
        fs::write(
            bundle_dir.join("serve_runtime.json"),
            serde_json::to_vec(&runtime).expect("serialize"),
        )
        .expect("write");

        let other_jid = "01957597-2a5c-7d92-8ec2-e0f3408a0d9b";
        let other_mark = crate::mark_text::mark_from_jid(other_jid).expect("other mark");
        let other_spoken = format_spoken_mark(&other_mark.to_render_spec());

        let snapshot = LinkServeStatusSnapshot {
            state: "connected".to_string(),
            health: "ok".to_string(),
            manager_alive: true,
            active_requests: 0,
            reconnect_count: 0,
            last_connected_at: Some(100.0),
            connected_age_seconds: Some(10.0),
            last_failure: None,
            next_retry_at: None,
            journal_version: Some("2026.07.26".to_string()),
            journal_version_fresh: true,
            instance_id: "home-instance".to_string(),
            ca_fp_prefix: ca_fp_prefix_hex(&[
                "-----BEGIN CERTIFICATE-----\nAA==\n-----END CERTIFICATE-----\n".to_string(),
            ])
            .expect("prefix"),
            paired_at: "2026-07-26T00:00:00Z".to_string(),
            persist_uncertain: false,
            unknown_journals: vec![crate::seam::UnknownJournalSighting {
                address: Some("192.168.1.50:7657".to_string()),
                jid: Some(other_jid.to_string()),
            }],
        };
        let response = crate::seam::HttpResponse {
            status: 200,
            headers: vec![],
            body: serde_json::to_vec(&snapshot).expect("serialize"),
            policy: crate::seam::TimeoutPolicy::Api,
        };
        let probe = crate::seam::ScriptedLinkStatusProbe::new(vec![(5015, Ok(response))]);

        let output = run_status(&[], &env, Some(&probe));
        assert_eq!(output.exit, 0);
        let expected = format!(
            "Label: alpha\nStatus: connected\nJournal version: 2026.07.26\n\nunknown journal seen at 192.168.1.50:7657:\n\nwhat answered:  {other_spoken} (claimed, not verified)\n{unavailable}\n\n{SIGHTING_RESET_FOOTER}\n",
            unavailable = crate::mark_text::MARK_UNAVAILABLE_LINE
        );
        assert_eq!(output.stdout, expected);
    }

    #[test]
    fn status_running_multiple_unknown_journals_preserves_order() {
        let temp = temp_dir("status-unknown-multi-order");
        let config = temp.join("config");
        let env = base_env(&config, &temp.join("home"));
        let local_id = "32eb4572-27d1-8392-baa2-6e4ea0b1c953";
        serve_bundle_with_instance_id(&config, "alpha", json!([]), local_id);

        let bundle_dir = config.join("solstone-observer").join("spl").join("alpha");
        let runtime = LinkServeRuntimeRecord { port: 5015 };
        fs::write(
            bundle_dir.join("serve_runtime.json"),
            serde_json::to_vec(&runtime).expect("serialize"),
        )
        .expect("write");

        let other_jid = "01957597-2a5c-7d92-8ec2-e0f3408a0d9b";
        let other_mark = crate::mark_text::mark_from_jid(other_jid).expect("other mark");
        let other_spoken = format_spoken_mark(&other_mark.to_render_spec());
        let local_mark = crate::mark_text::mark_from_jid(local_id).expect("local mark");
        let local_spoken = format_spoken_mark(&local_mark.to_render_spec());

        let snapshot = LinkServeStatusSnapshot {
            state: "connected".to_string(),
            health: "ok".to_string(),
            manager_alive: true,
            active_requests: 0,
            reconnect_count: 0,
            last_connected_at: Some(100.0),
            connected_age_seconds: Some(10.0),
            last_failure: None,
            next_retry_at: None,
            journal_version: Some("2026.07.26".to_string()),
            journal_version_fresh: true,
            instance_id: local_id.to_string(),
            ca_fp_prefix: serve_bundle_ca_fp_prefix(),
            paired_at: "2026-07-26T00:00:00Z".to_string(),
            persist_uncertain: false,
            unknown_journals: vec![
                crate::seam::UnknownJournalSighting {
                    address: Some("192.168.1.50:7657".to_string()),
                    jid: Some(other_jid.to_string()),
                },
                crate::seam::UnknownJournalSighting {
                    address: None,
                    jid: None,
                },
            ],
        };
        let response = crate::seam::HttpResponse {
            status: 200,
            headers: vec![],
            body: serde_json::to_vec(&snapshot).expect("serialize"),
            policy: crate::seam::TimeoutPolicy::Api,
        };
        let probe = crate::seam::ScriptedLinkStatusProbe::new(vec![(5015, Ok(response))]);

        let output = run_status(&[], &env, Some(&probe));
        assert_eq!(output.exit, 0);
        let block1 = format!(
            "\nunknown journal seen at 192.168.1.50:7657:\n\nwhat answered:  {other_spoken} (claimed, not verified)\nyour journal:   {local_spoken}\n\n{SIGHTING_RESET_FOOTER}\n"
        );
        let block2 = format!(
            "\nunknown journal seen through the relay:\n\nwhat answered:  no identity presented\nyour journal:   {local_spoken}\n"
        );
        let expected = format!(
            "Label: alpha\nStatus: connected\nJournal version: 2026.07.26\n{block1}{block2}"
        );
        assert_eq!(output.stdout, expected);
    }

    #[test]
    fn status_running_unknown_journal_prints_comparison_across_unpaired_and_connected_states() {
        let temp = temp_dir("status-unknown-states");
        let config = temp.join("config");
        let env = base_env(&config, &temp.join("home"));
        let local_id = "32eb4572-27d1-8392-baa2-6e4ea0b1c953";
        serve_bundle_with_instance_id(&config, "alpha", json!([]), local_id);

        let bundle_dir = config.join("solstone-observer").join("spl").join("alpha");
        let runtime = LinkServeRuntimeRecord { port: 5015 };
        fs::write(
            bundle_dir.join("serve_runtime.json"),
            serde_json::to_vec(&runtime).expect("serialize"),
        )
        .expect("write");

        let other_jid = "01957597-2a5c-7d92-8ec2-e0f3408a0d9b";
        let other_mark = crate::mark_text::mark_from_jid(other_jid).expect("other mark");
        let other_spoken = format_spoken_mark(&other_mark.to_render_spec());
        let local_mark = crate::mark_text::mark_from_jid(local_id).expect("local mark");
        let local_spoken = format_spoken_mark(&local_mark.to_render_spec());
        assert_ne!(other_spoken, local_spoken);

        let make_snapshot = |state: &str| LinkServeStatusSnapshot {
            state: state.to_string(),
            health: if state == "connected" {
                "ok".to_string()
            } else {
                "unhealthy".to_string()
            },
            manager_alive: true,
            active_requests: 0,
            reconnect_count: 0,
            last_connected_at: if state == "connected" {
                Some(100.0)
            } else {
                None
            },
            connected_age_seconds: if state == "connected" {
                Some(10.0)
            } else {
                None
            },
            last_failure: None,
            next_retry_at: None,
            journal_version: if state == "connected" {
                Some("2026.07.26".to_string())
            } else {
                None
            },
            journal_version_fresh: state == "connected",
            instance_id: local_id.to_string(),
            ca_fp_prefix: serve_bundle_ca_fp_prefix(),
            paired_at: "2026-07-26T00:00:00Z".to_string(),
            persist_uncertain: false,
            unknown_journals: vec![crate::seam::UnknownJournalSighting {
                address: Some("192.168.1.50:7657".to_string()),
                jid: Some(other_jid.to_string()),
            }],
        };

        let expected_sighting = format!(
            "unknown journal seen at 192.168.1.50:7657:\n\nwhat answered:  {other_spoken} (claimed, not verified)\nyour journal:   {local_spoken}\n\n{SIGHTING_RESET_FOOTER}\n"
        );

        // 1. Connected state
        {
            let snapshot = make_snapshot("connected");
            let response = crate::seam::HttpResponse {
                status: 200,
                headers: vec![],
                body: serde_json::to_vec(&snapshot).expect("serialize"),
                policy: crate::seam::TimeoutPolicy::Api,
            };
            let probe = crate::seam::ScriptedLinkStatusProbe::new(vec![(5015, Ok(response))]);
            let output = run_status(&[], &env, Some(&probe));
            assert_eq!(output.exit, 0);
            assert!(output.stdout.contains("Status: connected\n"));
            assert!(output.stdout.contains(&expected_sighting));
        }

        // 2. Unpaired state
        {
            let snapshot = make_snapshot("unpaired");
            let response = crate::seam::HttpResponse {
                status: 200,
                headers: vec![],
                body: serde_json::to_vec(&snapshot).expect("serialize"),
                policy: crate::seam::TimeoutPolicy::Api,
            };
            let probe = crate::seam::ScriptedLinkStatusProbe::new(vec![(5015, Ok(response))]);
            let output = run_status(&[], &env, Some(&probe));
            assert_eq!(output.exit, 0);
            assert!(output.stdout.contains("Status: unpaired\n"));
            assert!(output.stdout.contains(&expected_sighting));
            assert!(output.stdout.contains(&unpaired_note("en-US", &bundle_dir)));
        }
    }
}
