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

use chrono::{DateTime, Utc};
use serde_json::{Map, Value};
use spl_core::pairlink::{PairLinkError, ParsedPairLink};

use crate::command::{CommandContext, CommandOutput};
use crate::json_format::{json_compact_ascii, json_pretty_ascii};
use crate::resident::ResidentCommand;
use crate::seam::{
    LinkJoinCredential, LinkJoinDirectRequest, LinkJoinPairTarget, LinkJoinPairingError,
    LinkJoinPairingErrorKind, LinkJoinRelayControlEndpoint, LinkJoinRelayErrorKind,
    LinkJoinRelayRequest, LinkJournalMetadata, LinkServeBundle, LinkServeCarrierPolicy,
    LinkServeEndpoint, LinkServeError, LinkServeErrorKind, LinkServeRequest,
    LinkServeRuntimeRecord, LinkServeStatusSnapshot, LinkServeTransportErrorKind,
};

const HELP: &str = "usage: solstone link join [-h] [--home HOME] --code CODE [--as AS_ROLE]\n                     [--label LABEL]\n\noptions:\n  -h, --help     show this help message and exit\n  --home HOME    Receiver base URL\n  --code CODE    pair-link URL\n  --as AS_ROLE   Optional tag to join as\n  --label LABEL  Local credentials label (defaults to this machine's hostname)\n";
const USAGE: &str = "usage: solstone link join [-h] [--home HOME] --code CODE [--as AS_ROLE]\n                     [--label LABEL]\n";
const SERVE_HELP: &str = "usage: solstone link serve [-h] [--label LABEL] [--port PORT]\n                      [--relay-url RELAY_URL] [--direct | --relay-only]\n\noptions:\n  -h, --help            show this help message and exit\n  --label LABEL         Link bundle label\n  --port PORT           Loopback port to serve on (default: 5015)\n  --relay-url RELAY_URL\n                        Override the relay URL\n  --direct              Direct only: reach the journal through a direct\n                        connection, never the relay. Use when the home is\n                        reachable directly (same network/VPN) to avoid relay\n                        dependency.\n  --relay-only          Relay only: reach the journal through the relay,\n                        never a direct connection. Use when direct connections\n                        must not be attempted, even if the home is reachable\n                        locally.\n";
const SERVE_USAGE: &str = "usage: solstone link serve [-h] [--label LABEL] [--port PORT]\n                      [--relay-url RELAY_URL] [--direct | --relay-only]\n";
const STATUS_HELP: &str = "usage: solstone link status [-h] [--label LABEL]\n\nShow link status and observed remote journal version.\n\noptions:\n  -h, --help     show this help message and exit\n  --label LABEL  Link bundle label (defaults to the only paired link)\n";
const STATUS_USAGE: &str = "usage: solstone link status [-h] [--label LABEL]\n";
const DEFAULT_CLIENT_LABEL: &str = "linked-system";
const DEFAULT_SERVE_PORT: u16 = 5015;
const DEFAULT_RELAY_URL: &str = "https://link.solstone.app";
const PAIR_LINK_PREFIX: &str = "https://go.solstone.app/p#";
const LOCAL_ENDPOINTS_MAX_BYTES: usize = 16 * 1024;
static BUNDLE_STAGING_SEQUENCE: AtomicU64 = AtomicU64::new(0);
const PEER_JOIN_MOVED: &str =
    "Peer joins require journal-device authority and are not available through this command.\n";
const BUNDLE_FILES: &[&str] = &[
    "private.pem",
    "cert.pem",
    "chain.pem",
    "home_attestation.jwt",
    "peer.json",
];

#[must_use]
pub fn link_join(ctx: CommandContext<'_>) -> CommandOutput {
    let parsed = match parse_args(ctx.args) {
        Ok(parsed) => parsed,
        Err(error) => return argparse_error(error),
    };
    if parsed.help {
        return CommandOutput::success(HELP);
    }
    if parsed.code.is_none() {
        return argparse_error("the following arguments are required: --code".to_string());
    }
    if let Some(unknown) = parsed.unknown {
        return argparse_error(format!("unrecognized arguments: {unknown}"));
    }

    let as_role = parsed.as_role.unwrap_or_default();
    if as_role == "peer" {
        return CommandOutput::failure(PEER_JOIN_MOVED, 2);
    }
    if !matches!(as_role.as_str(), "" | "phone" | "observer") {
        return CommandOutput::failure("invalid role; expected one of: phone, observer\n", 2);
    }

    let label = match parsed.label {
        Some(label) => {
            if let Some(error) = label_error(&label) {
                return CommandOutput::failure(format!("{error}\n"), 2);
            }
            label
        }
        None => hostname_client_label(ctx.env),
    };

    let Some(seam) = ctx.link_pairing else {
        return CommandOutput::failure("Link pairing seam is unavailable.\n", 1);
    };

    let pair_request = match parse_pair_request(
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
    let bundle_dir = match observer_bundle_dir(&label, ctx.env) {
        Ok(bundle_dir) => {
            if path_lexists(&bundle_dir) {
                return CommandOutput::failure(
                    format!("{}\n", existing_path_message(&bundle_dir)),
                    1,
                );
            }
            bundle_dir
        }
        Err(error) => return CommandOutput::failure(format!("{error}\n"), 1),
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
        Err(error) => return CommandOutput::failure(format!("{}\n", pairing_error_text(error)), 1),
    };

    if let Err(error) = validate_credential(&credential) {
        return CommandOutput::failure(format!("{error}\n"), 1);
    }
    let local_endpoints = match normalized_local_endpoints(&credential.local_endpoints) {
        Ok(value) => value,
        Err(error) => return CommandOutput::failure(format!("{error}\n"), 1),
    };
    if json_compact_ascii(&local_endpoints).len() > LOCAL_ENDPOINTS_MAX_BYTES {
        return CommandOutput::failure("Pair response local_endpoints is too large.\n", 1);
    }

    let chain_pem = join_chain(&credential.ca_chain_pem);
    let peer_json = peer_json(
        &label,
        now_utc(ctx.clock),
        &credential,
        local_endpoints,
        false,
    );
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
    files.insert("peer.json".to_string(), peer_json.into_bytes());

    if let Err(error) = publish_bundle_atomic(&bundle_dir, &files) {
        let message = if error.kind() == io::ErrorKind::AlreadyExists {
            spent_existing_path_message(&bundle_dir)
        } else {
            error.to_string()
        };
        return CommandOutput::failure(format!("{message}\n"), 1);
    }
    let _ = fs::remove_file(bundle_dir.join("journal_metadata.json"));
    let _ = fs::remove_file(bundle_dir.join("serve_runtime.json"));

    CommandOutput::success(format!(
        "Linked {label}.\nCredentials: {}\n",
        bundle_dir.display()
    ))
}

pub fn link_serve(ctx: CommandContext<'_>) -> Result<ResidentCommand<'_>, CommandOutput> {
    let parsed = match parse_serve_args(ctx.args) {
        Ok(parsed) => parsed,
        Err(error) => return Err(argparse_serve_error(error)),
    };
    if parsed.help {
        return Err(CommandOutput::success(SERVE_HELP));
    }
    if let Some(unknown) = parsed.unknown {
        return Err(argparse_serve_error(format!(
            "unrecognized arguments: {unknown}"
        )));
    }
    if parsed.direct && parsed.relay_only {
        return Err(argparse_serve_error(
            "argument --direct: not allowed with argument --relay-only".to_string(),
        ));
    }
    let port = match parsed.port {
        Some(port) => port,
        None => DEFAULT_SERVE_PORT,
    };
    let selection = match resolve_serve_bundle(parsed.label.as_deref(), ctx.env) {
        Ok(selection) => selection,
        Err(error) => return Err(CommandOutput::failure(format!("{error}\n"), 1)),
    };
    let policy = if parsed.direct {
        LinkServeCarrierPolicy::Direct
    } else if parsed.relay_only {
        LinkServeCarrierPolicy::RelayOnly
    } else {
        LinkServeCarrierPolicy::RelayPermitted
    };
    let relay_origin = if parsed.direct {
        None
    } else {
        Some(resolve_serve_relay_url(
            parsed.relay_url.as_deref(),
            ctx.env,
        ))
    };
    let Some(runner) = ctx.link_serve else {
        return Err(CommandOutput::failure(
            "Link serve seam is unavailable.\n",
            1,
        ));
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
                format!("{}\n", serve_error_text(error)),
                1,
            ));
        }
    };
    let startup = match policy {
        LinkServeCarrierPolicy::Direct => format!(
            "forwarding 127.0.0.1:{} -> home {} via direct connection\n",
            session.bound_port(),
            selection.label
        ),
        LinkServeCarrierPolicy::RelayPermitted => format!(
            "forwarding 127.0.0.1:{} -> home {} via direct or relay\n",
            session.bound_port(),
            selection.label
        ),
        LinkServeCarrierPolicy::RelayOnly => format!(
            "forwarding 127.0.0.1:{} -> home {} via relay only\n",
            session.bound_port(),
            selection.label
        ),
    };
    Ok(ResidentCommand::new(
        startup,
        move |shutdown| match session.serve(shutdown) {
            Ok(()) => CommandOutput::success(""),
            Err(error) => CommandOutput::failure(format!("{}\n", serve_error_text(error)), 1),
        },
    ))
}

pub fn link_status(ctx: CommandContext<'_>) -> CommandOutput {
    let parsed = match parse_status_args(ctx.args) {
        Ok(parsed) => parsed,
        Err(error) => return argparse_status_error(error),
    };
    if parsed.help {
        return CommandOutput::success(STATUS_HELP);
    }
    if let Some(unknown) = parsed.unknown {
        return argparse_status_error(format!("unrecognized arguments: {unknown}"));
    }

    let selection = match resolve_serve_bundle(parsed.label.as_deref(), ctx.env) {
        Ok(selection) => selection,
        Err(error) => {
            return CommandOutput::failure(format!("solstone link status: error: {error}\n"), 1);
        }
    };

    let mut live_status: Option<LinkServeStatusSnapshot> = None;
    let runtime_path = selection.bundle_dir.join("serve_runtime.json");
    let persisted_meta = fs::read_to_string(selection.bundle_dir.join("journal_metadata.json"))
        .ok()
        .and_then(|content| serde_json::from_str::<LinkJournalMetadata>(&content).ok());
    if let (Ok(content), Some(probe)) = (fs::read_to_string(&runtime_path), ctx.link_status_probe) {
        live_status = serde_json::from_str::<LinkServeRuntimeRecord>(&content)
            .ok()
            .and_then(|rec| probe.probe(rec.port).ok())
            .filter(|resp| resp.status == 200)
            .and_then(|resp| serde_json::from_slice::<LinkServeStatusSnapshot>(&resp.body).ok())
            .filter(|snap| {
                snap.instance_id == selection.bundle.instance_id
                    && persisted_meta
                        .as_ref()
                        .is_none_or(|meta| snap.ca_fp_prefix == meta.ca_fp_prefix)
            });
    }

    let (state_str, version_str) = if let Some(snap) = live_status {
        let ver = if let Some(raw_ver) = snap.journal_version {
            let clean_ver = sanitize_display_version(&raw_ver);
            if snap.journal_version_fresh {
                clean_ver
            } else {
                format!("{clean_ver} (last known)")
            }
        } else {
            read_cached_version_fallback(&selection.bundle_dir, &selection.bundle.instance_id)
        };
        (snap.state, ver)
    } else {
        let ver =
            read_cached_version_fallback(&selection.bundle_dir, &selection.bundle.instance_id);
        ("stopped".to_string(), ver)
    };

    CommandOutput::success(format!(
        "Label: {}\nStatus: {}\nJournal version: {}\n",
        selection.label, state_str, version_str
    ))
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
struct ParsedArgs {
    home: Option<String>,
    code: Option<String>,
    as_role: Option<String>,
    label: Option<String>,
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

fn parse_status_args(args: &[String]) -> Result<ParsedStatusArgs, String> {
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
            parsed.label = Some(take_value(args, index, "--label")?.to_string());
        } else if parsed.unknown.is_none() {
            parsed.unknown = Some(token.clone());
        }
        index += 1;
    }
    Ok(parsed)
}

fn argparse_status_error(error: String) -> CommandOutput {
    CommandOutput::failure(
        format!("{STATUS_USAGE}solstone link status: error: {error}\n"),
        2,
    )
}

fn sanitize_display_version(raw: &str) -> String {
    raw.chars()
        .filter(|c| !c.is_control() && *c != '\x1b')
        .collect()
}

fn read_cached_version_fallback(bundle_dir: &Path, expected_instance_id: &str) -> String {
    let meta_path = bundle_dir.join("journal_metadata.json");
    let cached = fs::read_to_string(&meta_path)
        .ok()
        .and_then(|content| serde_json::from_str::<LinkJournalMetadata>(&content).ok())
        .filter(|meta| meta.instance_id == expected_instance_id && !meta.journal_version.is_empty())
        .map(|meta| {
            let clean = sanitize_display_version(&meta.journal_version);
            format!("{clean} (last known)")
        });
    cached.unwrap_or_else(|| "unknown".to_string())
}

fn parse_args(args: &[String]) -> Result<ParsedArgs, String> {
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
            parsed.home = Some(take_value(args, index, "--home")?.to_string());
        } else if let Some(value) = token.strip_prefix("--code=") {
            parsed.code = Some(value.to_string());
        } else if token == "--code" {
            index += 1;
            parsed.code = Some(take_value(args, index, "--code")?.to_string());
        } else if let Some(value) = token.strip_prefix("--as=") {
            parsed.as_role = Some(value.to_string());
        } else if token == "--as" {
            index += 1;
            parsed.as_role = Some(take_value(args, index, "--as")?.to_string());
        } else if let Some(value) = token.strip_prefix("--label=") {
            parsed.label = Some(value.to_string());
        } else if token == "--label" {
            index += 1;
            parsed.label = Some(take_value(args, index, "--label")?.to_string());
        } else if parsed.unknown.is_none() {
            parsed.unknown = Some(token.clone());
        }
        index += 1;
    }
    Ok(parsed)
}

fn parse_serve_args(args: &[String]) -> Result<ParsedServeArgs, String> {
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
            parsed.label = Some(take_value(args, index, "--label")?.to_string());
        } else if let Some(value) = token.strip_prefix("--port=") {
            parsed.port = Some(parse_serve_port(value)?);
        } else if token == "--port" {
            index += 1;
            parsed.port = Some(parse_serve_port(take_raw_value(args, index, "--port")?)?);
        } else if let Some(value) = token.strip_prefix("--relay-url=") {
            parsed.relay_url = Some(value.to_string());
        } else if token == "--relay-url" {
            index += 1;
            parsed.relay_url = Some(take_value(args, index, "--relay-url")?.to_string());
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

fn parse_serve_port(value: &str) -> Result<u16, String> {
    let parsed = value
        .parse::<i64>()
        .map_err(|_| format!("argument --port: invalid int value: '{value}'"))?;
    match parsed {
        0 => Ok(0),
        1..=65535 => {
            u16::try_from(parsed).map_err(|_| "--port must be between 1 and 65535".to_string())
        }
        _ => Err("--port must be between 1 and 65535".to_string()),
    }
}

fn take_value<'a>(args: &'a [String], index: usize, option: &str) -> Result<&'a str, String> {
    let Some(value) = args.get(index).map(String::as_str) else {
        return Err(format!("argument {option}: expected one argument"));
    };
    if value.starts_with('-') {
        return Err(format!("argument {option}: expected one argument"));
    }
    Ok(value)
}

fn take_raw_value<'a>(args: &'a [String], index: usize, option: &str) -> Result<&'a str, String> {
    let Some(value) = args.get(index).map(String::as_str) else {
        return Err(format!("argument {option}: expected one argument"));
    };
    Ok(value)
}

fn argparse_serve_error(error: String) -> CommandOutput {
    CommandOutput::failure(
        format!("{SERVE_USAGE}solstone link serve: error: {error}\n"),
        2,
    )
}

fn resolve_serve_bundle(
    label: Option<&str>,
    env: &BTreeMap<String, String>,
) -> Result<ServeBundleSelection, String> {
    if let Some(label) = label {
        let bundle_dir = observer_bundle_dir(label, env)?;
        let bundle = load_serve_bundle(&bundle_dir).map_err(|error| {
            format!(
                "invalid link bundle for label '{label}' at {}: {error}. Run `solstone link join` to pair this device.",
                bundle_dir.display()
            )
        })?;
        return Ok(ServeBundleSelection {
            label: label.to_string(),
            bundle,
            bundle_dir,
        });
    }

    let root = observer_spl_root(env)?;
    let mut bundles = BTreeMap::new();
    if root.is_dir() {
        for entry in fs::read_dir(&root).map_err(|error| error.to_string())? {
            let entry = entry.map_err(|error| error.to_string())?;
            let path = entry.path();
            if !path.is_dir() {
                continue;
            }
            let label = entry.file_name().to_string_lossy().to_string();
            if let Ok(bundle) = load_serve_bundle(&path) {
                bundles.insert(label, (bundle, path));
            }
        }
    }
    if bundles.is_empty() {
        return Err(format!(
            "no observer link bundles found under {}. Run `solstone link join` to pair this device.",
            root.display()
        ));
    }
    if bundles.len() > 1 {
        let labels = bundles.keys().cloned().collect::<Vec<_>>().join(", ");
        return Err(format!(
            "multiple observer link bundles found: {labels}. Pass --label to choose one."
        ));
    }
    let (label, (bundle, bundle_dir)) = bundles.into_iter().next().expect("bundle count checked");
    Ok(ServeBundleSelection {
        label,
        bundle,
        bundle_dir,
    })
}

fn observer_spl_root(env: &BTreeMap<String, String>) -> Result<PathBuf, String> {
    let base = env
        .get("XDG_CONFIG_HOME")
        .filter(|value| !value.is_empty())
        .map_or_else(
            || {
                env.get("HOME")
                    .filter(|value| !value.is_empty())
                    .map(|home| PathBuf::from(home).join(".config"))
            },
            |xdg| Some(PathBuf::from(xdg)),
        );
    let Some(base) = base else {
        return Err("Could not resolve home directory for observer credentials.".to_string());
    };
    Ok(base.join("solstone-observer").join("spl"))
}

fn load_serve_bundle(bundle_dir: &Path) -> Result<LinkServeBundle, String> {
    if !bundle_dir.is_dir() {
        return Err(format!("PL bundle not found: {}", bundle_dir.display()));
    }
    let missing = BUNDLE_FILES
        .iter()
        .filter(|name| !bundle_dir.join(name).exists())
        .map(|name| bundle_dir.join(name).display().to_string())
        .collect::<Vec<_>>();
    if !missing.is_empty() {
        return Err(format!("missing PL bundle file: {}", missing.join(", ")));
    }
    let private_key_pem = read_bundle_text(bundle_dir, "private.pem")?;
    let client_cert_pem = read_bundle_text(bundle_dir, "cert.pem")?;
    let chain_pem = read_bundle_text(bundle_dir, "chain.pem")?;
    let ca_chain_pem = split_pem_certificates(&chain_pem)?;
    let home_attestation = read_bundle_text(bundle_dir, "home_attestation.jwt")?;
    let peer_text = read_bundle_text(bundle_dir, "peer.json")?;
    let peer: Value = serde_json::from_str(&peer_text)
        .map_err(|error| format!("invalid peer.json in {}: {error}", bundle_dir.display()))?;
    let local_endpoints = match peer.get("local_endpoints") {
        Some(Value::Null) | None => Value::Array(Vec::new()),
        Some(Value::Array(_)) => peer
            .get("local_endpoints")
            .cloned()
            .expect("local_endpoints presence checked"),
        Some(_) => return Err("peer.json local_endpoints must be a list".to_string()),
    };
    let endpoints = serve_endpoints_from_value(&local_endpoints)?;
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
    })
}

fn read_bundle_text(bundle_dir: &Path, name: &str) -> Result<String, String> {
    fs::read_to_string(bundle_dir.join(name)).map_err(|error| error.to_string())
}

fn split_pem_certificates(chain_pem: &str) -> Result<Vec<String>, String> {
    const BEGIN: &str = "-----BEGIN CERTIFICATE-----";
    const END: &str = "-----END CERTIFICATE-----";
    let mut certs = Vec::new();
    let mut rest = chain_pem;
    while let Some(begin) = rest.find(BEGIN) {
        let after_begin = &rest[begin..];
        let Some(end) = after_begin.find(END) else {
            return Err("chain.pem contains an incomplete certificate".to_string());
        };
        let end_index = end + END.len();
        let mut cert = after_begin[..end_index].to_string();
        cert.push('\n');
        certs.push(cert);
        rest = &after_begin[end_index..];
    }
    if certs.is_empty() {
        return Err("chain.pem contains no certificates".to_string());
    }
    Ok(certs)
}

fn serve_endpoints_from_value(value: &Value) -> Result<Vec<LinkServeEndpoint>, String> {
    let Value::Array(items) = value else {
        return Err("peer.json local_endpoints must be a list".to_string());
    };
    let mut endpoints = Vec::new();
    for item in items {
        let Value::Object(map) = item else {
            return Err("peer.json local_endpoints entries must be objects".to_string());
        };
        let host = map
            .get("ip")
            .or_else(|| map.get("host"))
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim()
            .to_string();
        if host.is_empty() {
            return Err("LAN endpoint missing ip".to_string());
        }
        let port = match map.get("port") {
            None | Some(Value::Null) => 7657,
            Some(Value::Number(number)) => number
                .as_u64()
                .and_then(|value| u16::try_from(value).ok())
                .ok_or_else(|| "LAN endpoint has invalid port".to_string())?,
            Some(Value::String(value)) => value
                .parse::<u16>()
                .map_err(|_| "LAN endpoint has invalid port".to_string())?,
            Some(_) => return Err("LAN endpoint has invalid port".to_string()),
        };
        endpoints.push(LinkServeEndpoint { host, port });
    }
    Ok(endpoints)
}

fn resolve_serve_relay_url(value: Option<&str>, env: &BTreeMap<String, String>) -> String {
    if let Some(value) = value.filter(|value| !value.trim().is_empty()) {
        return value.trim().trim_end_matches('/').to_string();
    }
    if let Some(value) = env
        .get("SOL_LINK_RELAY_URL")
        .filter(|value| !value.trim().is_empty())
    {
        return value.trim().trim_end_matches('/').to_string();
    }
    DEFAULT_RELAY_URL.to_string()
}

fn argparse_error(error: String) -> CommandOutput {
    CommandOutput::failure(format!("{USAGE}solstone link join: error: {error}\n"), 2)
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

fn parse_pair_request(code: &str, home: Option<&str>) -> Result<PairRequest, String> {
    if !code.starts_with(PAIR_LINK_PREFIX) {
        return Err(format!(
            "Pair code did not match an accepted form. Use a pair-link like {PAIR_LINK_PREFIX}... from 'solstone call link pair'."
        ));
    }
    match spl_core::pairlink::parse(code) {
        Ok(ParsedPairLink::Direct(link)) => {
            let targets = if let Some(home) = home {
                vec![parse_home_target(home)?]
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
        Err(PairLinkError::DisallowedDirectIpv4 { address: _ }) => Err(
            "Pair-link points at an address outside the local network this joiner will dial."
                .to_string(),
        ),
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
        ) => Err(malformed_pair_link_message()),
    }
}

fn malformed_pair_link_message() -> String {
    format!(
        "Malformed pair-link. Use the full {PAIR_LINK_PREFIX}... value from the pairing output."
    )
}

fn parse_home_target(home: &str) -> Result<LinkJoinPairTarget, String> {
    let Some((_, rest)) = home.split_once("://") else {
        return Err("Pair-link target missing host.".to_string());
    };
    let authority = rest
        .split(['/', '?', '#'])
        .next()
        .unwrap_or_default()
        .rsplit('@')
        .next()
        .unwrap_or_default();
    if authority.is_empty() {
        return Err("Pair-link target missing host.".to_string());
    }
    if let Some(after_bracket) = authority
        .strip_prefix('[')
        .and_then(|value| value.split_once(']'))
    {
        let host = after_bracket.0;
        if host.is_empty() {
            return Err("Pair-link target missing host.".to_string());
        }
        let port_text = after_bracket
            .1
            .strip_prefix(':')
            .ok_or_else(|| "Pair-link target missing explicit port.".to_string())?;
        let port = parse_explicit_port(port_text)?;
        return Ok(LinkJoinPairTarget {
            host: host.to_string(),
            port,
        });
    }
    let Some((host, port_text)) = authority.rsplit_once(':') else {
        return Err("Pair-link target missing explicit port.".to_string());
    };
    if host.is_empty() {
        return Err("Pair-link target missing host.".to_string());
    }
    let port = parse_explicit_port(port_text)?;
    Ok(LinkJoinPairTarget {
        host: host.to_string(),
        port,
    })
}

fn parse_explicit_port(value: &str) -> Result<u16, String> {
    value
        .parse::<u16>()
        .map_err(|_| "Pair-link target missing explicit port.".to_string())
}

fn label_error(label: &str) -> Option<&'static str> {
    if label.is_empty() {
        return Some("--label must not be empty");
    }
    if label.chars().count() > 80 {
        return Some("--label must be 80 characters or fewer");
    }
    if label.contains('/') || label.contains('\\') {
        return Some("--label must not contain path separators");
    }
    if label.contains("..") {
        return Some("--label must not contain '..'");
    }
    if label.starts_with('.') {
        return Some("--label must not start with '.'");
    }
    if !label
        .chars()
        .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '_' | '.' | '-'))
    {
        return Some("--label may contain only letters, numbers, '-', '_', and '.'");
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
    if trimmed.is_empty() || label_error(&trimmed).is_some() {
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

fn observer_bundle_dir(label: &str, env: &BTreeMap<String, String>) -> Result<PathBuf, String> {
    Ok(observer_spl_root(env)?.join(label))
}

fn validate_credential(credential: &LinkJoinCredential) -> Result<(), &'static str> {
    if credential.client_key_pem.is_empty() {
        return Err("Pair response missing generated client key");
    }
    if credential.client_cert_pem.is_empty() {
        return Err("Pair response missing client_cert");
    }
    if credential.ca_chain_pem.is_empty() || credential.ca_chain_pem.iter().any(String::is_empty) {
        return Err("Pair response missing ca_chain");
    }
    if credential.instance_id.is_empty() {
        return Err("Pair response missing instance_id");
    }
    if credential
        .home_attestation
        .as_deref()
        .unwrap_or_default()
        .is_empty()
    {
        return Err("Pair response missing home_attestation");
    }
    Ok(())
}

fn normalized_local_endpoints(value: &Value) -> Result<Value, &'static str> {
    match value {
        Value::Null => Ok(Value::Array(Vec::new())),
        Value::Array(_) => Ok(value.clone()),
        Value::Bool(_) | Value::Number(_) | Value::String(_) | Value::Object(_) => {
            Err("Pair response local_endpoints must be an array.")
        }
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
    is_peer: bool,
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
    peer.insert(
        "role".to_string(),
        Value::String(if is_peer { "peer" } else { "" }.to_string()),
    );
    format!("{}\n", json_pretty_ascii(&Value::Object(peer)))
}

fn now_utc(clock: Option<&dyn crate::seam::Clock>) -> String {
    let now = clock.map_or_else(SystemTime::now, |clock| clock.now());
    let datetime: DateTime<Utc> = now.into();
    datetime.format("%Y-%m-%dT%H:%M:%SZ").to_string()
}

fn pairing_error_text(error: LinkJoinPairingError) -> String {
    let kind = error.kind;
    match kind {
        LinkJoinPairingErrorKind::Rejected { status } => format!(
            "Pairing failed (HTTP {status}): the pairing window is closed or the code was already used."
        ),
        LinkJoinPairingErrorKind::Relay(LinkJoinRelayErrorKind::PairWindowClosed) => {
            "Pairing failed: the pairing window is closed or expired. Generate a new pair code and retry."
                .to_string()
        }
        LinkJoinPairingErrorKind::PairResponseMissingHomeAttestation => {
            "Pair response missing home_attestation".to_string()
        }
        LinkJoinPairingErrorKind::NoEndpoint => {
            "Could not connect to the pairing listener.".to_string()
        }
        LinkJoinPairingErrorKind::RelayControlRejected { endpoint, status } => {
            format!(
                "Relay rejected device enrollment ({} HTTP {status}).",
                relay_control_endpoint_code(endpoint)
            )
        }
        kind @ (LinkJoinPairingErrorKind::Io
        | LinkJoinPairingErrorKind::Tls
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
        | LinkJoinPairingErrorKind::RuntimeUnavailable) => format!(
            "Pairing failed ({}). Generate a new pair code and retry.",
            transport_error_code(kind)
        ),
    }
}

fn serve_error_text(error: LinkServeError) -> String {
    match error.kind {
        LinkServeErrorKind::InvalidBundle => {
            "Link credentials are invalid. Run solstone link join before solstone link serve.".to_string()
        }
        LinkServeErrorKind::Bind { port, addr_in_use } => {
            if addr_in_use {
                format!(
                    "cannot bind 127.0.0.1:{port}: address already in use. Another `solstone link serve` or Convey may already be using that port."
                )
            } else {
                format!("cannot bind 127.0.0.1:{port}: bind failed")
            }
        }
        LinkServeErrorKind::RuntimeUnavailable => {
            "Native link runtime is unavailable. Reinstall solstone-core and retry.".to_string()
        }
        LinkServeErrorKind::BridgeCapability => {
            "Native link bridge setup failed before serving. Retry after reinstalling solstone-core."
                .to_string()
        }
        LinkServeErrorKind::Transport(kind) => serve_transport_error_text(kind),
    }
}

fn serve_transport_error_text(kind: LinkServeTransportErrorKind) -> String {
    match kind {
        LinkServeTransportErrorKind::Io => {
            "Link transport I/O failed while serving. Check that the journal is reachable on LAN/VPN or relay, then retry.".to_string()
        }
        LinkServeTransportErrorKind::Tls => {
            "Secure link handshake failed. Re-run solstone link join if the journal certificate or pairing changed.".to_string()
        }
        LinkServeTransportErrorKind::Crypto => {
            "Link credential material is invalid. Re-run solstone link join for this observer.".to_string()
        }
        LinkServeTransportErrorKind::Mux => {
            "SPL stream framing failed while serving. Retry; re-pair if it continues.".to_string()
        }
        LinkServeTransportErrorKind::Http => {
            "The journal response over the link could not be parsed. Update both peers or retry.".to_string()
        }
        LinkServeTransportErrorKind::Json => {
            "Relay or bridge JSON could not be parsed. Check the relay URL and retry.".to_string()
        }
        LinkServeTransportErrorKind::PairLink => {
            "Stored pairing data is invalid. Re-run solstone link join for this observer.".to_string()
        }
        LinkServeTransportErrorKind::Pairing => {
            "Link credential or relay enrollment failed. Re-run solstone link join if retrying does not fix it.".to_string()
        }
        LinkServeTransportErrorKind::Rejected { status } => {
            format!("The paired journal rejected the link request with HTTP {status}.")
        }
        LinkServeTransportErrorKind::Relay(error) => serve_relay_error_text(error).to_string(),
        LinkServeTransportErrorKind::RelayControlRejected { endpoint, status } => {
            match endpoint {
                crate::seam::LinkServeRelayControlEndpoint::EnrollDevice => format!(
                    "Relay enrollment was rejected with HTTP {status}. Re-run solstone link join if the bundle attestation is stale."
                ),
                crate::seam::LinkServeRelayControlEndpoint::TokenRefresh => format!(
                    "Relay token refresh was rejected with HTTP {status}. Re-run solstone link join for this observer."
                ),
            }
        }
        LinkServeTransportErrorKind::NoEndpoint => {
            "No journal endpoint is available. Re-run solstone link join or pass --relay-url unless using --direct intentionally.".to_string()
        }
        LinkServeTransportErrorKind::NotPaired => {
            "Link credentials are missing. Run solstone link join before solstone link serve.".to_string()
        }
        LinkServeTransportErrorKind::LocalOffset => {
            "Local offset lookup failed. Check the system clock and retry.".to_string()
        }
    }
}

fn serve_relay_error_text(error: crate::seam::LinkServeRelayErrorKind) -> &'static str {
    match error {
        crate::seam::LinkServeRelayErrorKind::HomeOffline => {
            "The relay reports the home journal is offline. Start the journal or use --direct on LAN/VPN."
        }
        crate::seam::LinkServeRelayErrorKind::Unauthorized => {
            "The relay rejected this observer token. Re-run solstone link join for this observer."
        }
        crate::seam::LinkServeRelayErrorKind::Unpaid => {
            "The relay account is not available. Check relay service/account status or use --direct."
        }
        crate::seam::LinkServeRelayErrorKind::UnknownInstance => {
            "The relay does not know this journal instance. Re-run solstone link join."
        }
        crate::seam::LinkServeRelayErrorKind::PairWindowClosed => {
            "The relay pairing window is closed. Re-run solstone link join from a fresh code."
        }
        crate::seam::LinkServeRelayErrorKind::Overflow => {
            "The relay is temporarily overloaded. Retry or use --direct on LAN/VPN."
        }
        crate::seam::LinkServeRelayErrorKind::Abnormal => {
            "The relay connection closed abnormally. Retry or use --direct on LAN/VPN."
        }
        crate::seam::LinkServeRelayErrorKind::UpgradeRejected => {
            "The relay rejected the WebSocket upgrade. Check --relay-url and retry."
        }
        crate::seam::LinkServeRelayErrorKind::Stalled => {
            "The relay connection stalled. Retry or use --direct on LAN/VPN."
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

fn existing_path_message(path: &Path) -> String {
    format!(
        "Credentials path already exists: {}. Remove it and rerun if re-pairing.",
        path.display()
    )
}

fn spent_existing_path_message(path: &Path) -> String {
    format!(
        "Credentials path already exists: {}. The pairing code is now spent; generate a new one and rerun after removing it.",
        path.display()
    )
}

fn publish_bundle_atomic(bundle_dir: &Path, files: &BTreeMap<String, Vec<u8>>) -> io::Result<()> {
    publish_bundle_atomic_with_writer(bundle_dir, files, write_private_file)
}

fn publish_bundle_atomic_with_writer<W>(
    bundle_dir: &Path,
    files: &BTreeMap<String, Vec<u8>>,
    write_file: W,
) -> io::Result<()>
where
    W: Fn(&Path, &[u8]) -> io::Result<()>,
{
    if files.len() != BUNDLE_FILES.len()
        || BUNDLE_FILES.iter().any(|name| !files.contains_key(*name))
    {
        return Err(io::Error::other("credential bundle file set is incomplete"));
    }
    let parent = bundle_dir.parent().unwrap_or_else(|| Path::new("."));
    fs::create_dir_all(parent)?;
    if path_lexists(bundle_dir) {
        return Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            existing_path_message(bundle_dir),
        ));
    }

    let staging = PrivateStagingDir::create(parent, bundle_dir)?;
    for (name, content) in files {
        write_file(&staging.path.join(name), content)?;
    }
    File::open(&staging.path)?.sync_all()?;
    publish_staging_dir(&staging.path, bundle_dir)?;
    staging.disarm();
    if let Ok(directory) = File::open(parent) {
        let _ = directory.sync_all();
    }
    Ok(())
}

#[cfg(any(target_vendor = "apple", target_os = "linux"))]
fn publish_staging_dir(staging: &Path, destination: &Path) -> io::Result<()> {
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
fn publish_staging_dir(staging: &Path, destination: &Path) -> io::Result<()> {
    if path_lexists(destination) {
        return Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            existing_path_message(destination),
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
            let mut builder = DirBuilder::new();
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
    use std::cell::Cell;
    use std::collections::BTreeMap;
    use std::fs;
    use std::path::{Path, PathBuf};
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
            stdin: "",
            today: "20260726",
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
            stdin: "",
            today: "20260726",
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
            relay_device_token: None,
            relay_device_token_expires_at: None,
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
        const CERT: &str = "-----BEGIN CERTIFICATE-----\nAA==\n-----END CERTIFICATE-----\n";
        let bundle_dir = config.join("solstone-observer").join("spl").join(label);
        fs::create_dir_all(&bundle_dir).expect("serve bundle dir");
        fs::write(bundle_dir.join("private.pem"), "PRIVATE\n").expect("private key");
        fs::write(bundle_dir.join("cert.pem"), CERT).expect("client cert");
        fs::write(bundle_dir.join("chain.pem"), CERT).expect("chain");
        fs::write(bundle_dir.join("home_attestation.jwt"), "attestation.jwt").expect("attestation");
        fs::write(
            bundle_dir.join("peer.json"),
            json!({
                "instance_id": "home-instance",
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
            instance_id: "home-instance".to_string(),
            home_label: "Home".to_string(),
            paired_at: "2026-07-26T00:00:00Z".to_string(),
            endpoints: serve_endpoints_from_value(&local_endpoints).expect("serve endpoints"),
            local_endpoints,
        }
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
        publish_bundle_atomic(&bundle, &bundle_files()).expect("publish bundle");

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
    fn help_is_python_byte_exact() {
        let env = BTreeMap::new();
        let root = temp_dir("help-root");
        let seam = ScriptedLinkJoinPairingSeam::new(vec![]);
        let clock = FakeClock::at_unix(0);
        let output = run(&["--help"], &env, &root, &seam, &clock);
        assert_eq!(output, CommandOutput::success(HELP));
        assert_eq!(HELP.len(), 354);
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
        assert_eq!(
            ambiguous.stderr,
            "multiple observer link bundles found: alpha, beta. Pass --label to choose one.\n"
        );
        assert_eq!(ambiguous.exit, 1);
        runner.assert_done();

        let explicit_runner = ScriptedLinkServeRunner::new(vec![ExpectedLinkServeCall {
            expected: expected_serve_request(
                &config,
                "beta",
                5016,
                LinkServeCarrierPolicy::RelayPermitted,
                Some(DEFAULT_RELAY_URL),
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
        assert_eq!(
            explicit.startup(),
            "forwarding 127.0.0.1:5016 -> home beta via direct or relay\n"
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
                Some(DEFAULT_RELAY_URL),
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
            "forwarding 127.0.0.1:5015 -> home alpha via direct or relay\n"
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
                Some(DEFAULT_RELAY_URL),
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
        assert_eq!(
            resident.startup(),
            "forwarding 127.0.0.1:54321 -> home alpha via direct or relay\n"
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

        assert_eq!(
            resident.startup(),
            "forwarding 127.0.0.1:6001 -> home direct via direct connection\n"
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
                Some(DEFAULT_RELAY_URL),
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

        assert_eq!(
            resident.startup(),
            "forwarding 127.0.0.1:6002 -> home relay-only via relay only\n"
        );
        let recorded = runner.recorded();
        assert_eq!(recorded.len(), 1);
        assert_eq!(
            recorded[0].request.policy,
            LinkServeCarrierPolicy::RelayOnly
        );
        assert_eq!(
            recorded[0].request.relay_origin,
            Some(DEFAULT_RELAY_URL.to_string())
        );
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

        assert_eq!(
            resident.startup(),
            "forwarding 127.0.0.1:6003 -> home relay-only via relay only\n"
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
                Some(DEFAULT_RELAY_URL),
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
                Some(DEFAULT_RELAY_URL),
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
            let text = serve_transport_error_text(kind);
            assert_no_secret_substrings(&text);
        }
    }

    #[test]
    fn missing_code_exits_like_argparse() {
        let env = BTreeMap::new();
        let root = temp_dir("missing-code-root");
        let seam = ScriptedLinkJoinPairingSeam::new(vec![]);
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
        let seam = ScriptedLinkJoinPairingSeam::new(vec![]);
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
        let seam = ScriptedLinkJoinPairingSeam::new(vec![]);
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
            format!("{}\n", existing_path_message(&existing))
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
        let seam = ScriptedLinkJoinPairingSeam::new(vec![]);
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
            format!("{}\n", existing_path_message(&existing))
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
        let seam = ScriptedLinkJoinPairingSeam::new(vec![ExpectedLinkJoinPairingCall::Direct {
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
            format!("Linked laptop.\nCredentials: {}\n", bundle.display())
        );
        assert_eq!(output.exit, 0);
        assert_eq!(
            fs::read_to_string(bundle.join("peer.json")).expect("peer json"),
            OBSERVER_PEER_JSON
        );
        let entries = fs::read_dir(bundle.parent().expect("bundle parent"))
            .expect("bundle parent")
            .collect::<Result<Vec<_>, _>>()
            .expect("bundle entries");
        assert_eq!(entries.len(), 1);
        assert_bundle_files_exist(&bundle);
        assert!(!path_lexists(&root.join("peers")));
        seam.assert_done();
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

        let result =
            publish_bundle_atomic_with_writer(&bundle, &bundle_files(), |path, content| {
                if writes.get() == 1 {
                    return Err(io::Error::other("injected mid-write failure"));
                }
                writes.set(writes.get() + 1);
                write_private_file(path, content)
            });

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

        let result =
            publish_bundle_atomic_with_writer(&bundle, &bundle_files(), |path, content| {
                if !created.replace(true) {
                    fs::create_dir(&bundle)?;
                }
                write_private_file(path, content)
            });

        assert_eq!(
            result.expect_err("racing destination must win").kind(),
            io::ErrorKind::AlreadyExists
        );
        assert!(bundle.is_dir());
        assert_eq!(fs::read_dir(&bundle).expect("winner directory").count(), 0);
        assert_no_dot_residue(parent);
    }

    #[test]
    fn peer_missing_state_fails_without_pairing() {
        let temp = temp_dir("peer-state");
        let config = temp.join("config");
        let env = base_env(&config, &temp.join("home"));
        let root = temp.join("journal");
        let seam = ScriptedLinkJoinPairingSeam::new(vec![]);
        let clock = FakeClock::at_unix(0);

        let output = run(
            &["--code", &direct_pair_link(), "--as", "peer"],
            &env,
            &root,
            &seam,
            &clock,
        );

        assert_eq!(output.stderr, PEER_JOIN_MOVED);
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
        let first_seam =
            ScriptedLinkJoinPairingSeam::new(vec![ExpectedLinkJoinPairingCall::Direct {
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

        let second_seam = ScriptedLinkJoinPairingSeam::new(vec![]);
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
        let seam = ScriptedLinkJoinPairingSeam::new(vec![ExpectedLinkJoinPairingCall::Direct {
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
        let seam = ScriptedLinkJoinPairingSeam::new(vec![ExpectedLinkJoinPairingCall::Direct {
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
        let seam = ScriptedLinkJoinPairingSeam::new(vec![ExpectedLinkJoinPairingCall::Direct {
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
                true
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
                false
            ),
            NESTED_ENDPOINTS_JSON
        );
    }

    #[test]
    fn pairing_error_text_covers_every_kind_without_secret_leaks() {
        let mut kinds = vec![
            LinkJoinPairingErrorKind::Io,
            LinkJoinPairingErrorKind::Tls,
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
            let text = pairing_error_text(LinkJoinPairingError::new(kind.clone()));
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
            parse_home_target("https://").expect_err("missing host"),
            "Pair-link target missing host."
        );
        assert_eq!(
            parse_home_target("https://home.local").expect_err("missing port"),
            "Pair-link target missing explicit port."
        );
        assert_eq!(
            parse_home_target("https://home.local:7657/path?ignored=true").expect("target"),
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
            stdin: "",
            today: "20260726",
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
        assert!(
            output
                .stderr
                .starts_with("solstone link status: error: no observer link bundles found under ")
        );
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
        assert_eq!(
            output.stderr,
            "solstone link status: error: multiple observer link bundles found: alpha, beta. Pass --label to choose one.\n"
        );
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
            instance_id: "home-instance".to_string(),
            ca_fp_prefix: "abcd".to_string(),
            journal_version: "2026.07.26".to_string(),
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
            ca_fp_prefix: "abcd".to_string(),
            journal_version: "2026.07.26".to_string(),
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
            instance_id: "home-instance".to_string(),
            ca_fp_prefix: "abcd".to_string(),
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
            instance_id: "home-instance".to_string(),
            ca_fp_prefix: "abcd".to_string(),
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
            instance_id: "home-instance".to_string(),
            ca_fp_prefix: "abcd".to_string(),
            journal_version: "2026.07.26".to_string(),
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
            ca_fp_prefix: "abcd".to_string(),
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
            instance_id: "home-instance".to_string(),
            ca_fp_prefix: "expected-ca-fp".to_string(),
            journal_version: "2026.07.26".to_string(),
            observed_at: 1234.0,
        };
        fs::write(
            bundle_dir.join("journal_metadata.json"),
            serde_json::to_vec(&meta).expect("serialize"),
        )
        .expect("write");

        // Live probe returns matching instance_id but different ca_fp_prefix
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
            ca_fp_prefix: "different-ca-fp".to_string(),
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
            instance_id: "home-instance".to_string(),
            ca_fp_prefix: "abcd".to_string(),
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

        let seam = ScriptedLinkJoinPairingSeam::new(vec![ExpectedLinkJoinPairingCall::Direct {
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
}
