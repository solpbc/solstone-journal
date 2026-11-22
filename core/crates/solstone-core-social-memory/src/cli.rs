// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! The development-build command: sign in to the owner's atproto account
//! with the loopback client, share an activity into a space on that account,
//! keep shared copies in step, follow a space someone shared, and bring what
//! it holds into this journal.
//!
//! Compiled only into development builds. With `SOLSTONE_SOCIAL_RIG` naming
//! a rig file, every request goes to that rehearsal rig and nowhere else.

use std::ffi::OsString;
use std::io::Write;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::time::Duration;

use jiff::Timestamp;
use solstone_core_cli_copy::{Value, render_for};
use solstone_core_oauth::atproto::DpopKeyPair;
use solstone_core_oauth::atproto_client::{
    AuthorizationStart, DEFAULT_PLC_DIRECTORY, HandleDns, begin_authorization,
    complete_authorization, discover_auth_server, is_atproto_did, loopback_client_id,
    resolve_identity,
};
use solstone_core_oauth::callback::{
    CallbackError, CallbackOutcome, CallbackPolicy, LoopbackBindSpec, LoopbackHost,
    LoopbackListener, receive_loopback_callback, respond_to_callback_stream,
};
use solstone_core_oauth::registration::RegistrationMode;
use solstone_core_oauth::ssrf::{OAuthHttpClient, SsrfHttpsClient};
use solstone_core_oauth::store::{
    ClientRegistration, CredentialError, LoadResult, OAuthCredentialDoc, OAuthCredentialManager,
    OAuthTokens, ServiceId, load_credential_file, now_unix_secs, save_credential_file,
};

use crate::dns::SystemHandleDns;
use crate::record::ActivityRef;
use crate::rig::{RigConfig, RigDns, RigHttp};
use crate::sign_in_scope;
use crate::space::{Session, add_readers, create_space, space_authority};
use crate::store::{
    self, ReceiveReport, followed_spaces, receive_space, share_activity, shares, sync_shares,
};

/// The journal's stored atproto sign-in, `config/atproto-social-sign-in.json`.
const SERVICE: &str = "atproto-social";
const CALLBACK_PATH: &str = "/callback";
const SIGN_IN_WAIT: Duration = Duration::from_secs(600);

/// The environment variable naming a rehearsal rig file.
pub const RIG_ENV: &str = "SOLSTONE_SOCIAL_RIG";

const EXIT_USAGE: i32 = 64;
const EXIT_FAILURE: i32 = 1;

/// True when the arguments ask only for help, which reads no journal.
pub fn is_help(args: &[OsString]) -> bool {
    args.is_empty() || args.iter().any(|a| a == "-h" || a == "--help")
}

/// Where requests go: the owner's real hosts, or a rehearsal rig.
struct Net {
    http: Arc<dyn OAuthHttpClient>,
    dns: Box<dyn HandleDns>,
    plc: String,
}

impl Net {
    fn from_env() -> Result<Self, String> {
        match std::env::var_os(RIG_ENV) {
            Some(path) => {
                let rig = RigHttp::new(RigConfig::load(Path::new(&path))?)?;
                let plc = rig.plc().to_owned();
                Ok(Self {
                    http: Arc::new(rig),
                    dns: Box::new(RigDns),
                    plc,
                })
            }
            None => Ok(Self {
                http: Arc::new(SsrfHttpsClient::new()),
                dns: Box::new(SystemHandleDns),
                plc: DEFAULT_PLC_DIRECTORY.to_owned(),
            }),
        }
    }
}

enum Failure {
    Usage,
    /// Already reported.
    Quiet,
    SignedOut,
    Message(String),
    Detail(String),
}

impl<E: std::fmt::Display> From<E> for Failure {
    fn from(err: E) -> Self {
        Self::Detail(err.to_string())
    }
}

struct Output<'a> {
    locale: &'a str,
}

impl Output<'_> {
    fn text(&self, id: &str, args: &[(&str, Value<'_>)]) -> String {
        render_for(self.locale, id, args)
    }

    fn line(&self, id: &str, args: &[(&str, Value<'_>)]) {
        println!("{}", self.text(id, args));
        let _ = std::io::stdout().flush();
    }

    fn counted(
        &self,
        id: &str,
        quantity_name: &str,
        display_name: &str,
        quantity: usize,
        extra: &[(&str, Value<'_>)],
    ) {
        let shown = quantity.to_string();
        let mut args: Vec<(&str, Value<'_>)> = vec![
            (quantity_name, Value::Cardinal(quantity as i64)),
            (display_name, Value::String(&shown)),
        ];
        args.extend(extra.iter().cloned());
        self.line(id, &args);
    }
}

/// Run `solstone journal __social …` against `journal`. Returns the exit code.
pub fn run(args: &[OsString], journal: &Path, locale: &str) -> i32 {
    let out = Output { locale };
    if is_help(args) {
        out.line("journal.cli.social.usage", &[]);
        return 0;
    }
    let args: Option<Vec<String>> = args.iter().map(|a| a.to_str().map(str::to_owned)).collect();
    let result = match args {
        Some(args) => Net::from_env()
            .map_err(Failure::Detail)
            .and_then(|net| dispatch(&out, &net, journal, &args)),
        None => Err(Failure::Usage),
    };
    match result {
        Ok(()) => 0,
        Err(Failure::Usage) => {
            eprintln!("{}", out.text("journal.cli.social.usage", &[]));
            EXIT_USAGE
        }
        Err(Failure::Quiet) => EXIT_FAILURE,
        Err(Failure::SignedOut) => {
            eprintln!("{}", out.text("journal.cli.social.signed.out", &[]));
            EXIT_FAILURE
        }
        Err(Failure::Message(id)) => {
            eprintln!("{}", out.text(&id, &[]));
            EXIT_FAILURE
        }
        Err(Failure::Detail(detail)) => {
            eprintln!(
                "{}",
                out.text(
                    "journal.cli.social.failed",
                    &[("detail", Value::String(&detail))]
                )
            );
            EXIT_FAILURE
        }
    }
}

fn dispatch(out: &Output<'_>, net: &Net, journal: &Path, args: &[String]) -> Result<(), Failure> {
    let words: Vec<&str> = args.iter().map(String::as_str).collect();
    match words.as_slice() {
        ["resolve", who] => resolve(out, net, who),
        ["sign-in", who] => sign_in(out, net, journal, who),
        ["status"] => status(out, journal),
        ["share", facet, day, id, rest @ ..] => {
            let with = with_list(rest)?;
            share(out, net, journal, &activity(facet, day, id), &with)
        }
        ["sync"] => sync(out, net, journal),
        ["unshare", facet, day, id] => unshare(out, net, journal, &activity(facet, day, id)),
        ["follow", space] => follow(out, net, journal, space),
        ["receive"] => receive(out, net, journal),
        _ => Err(Failure::Usage),
    }
}

fn activity(facet: &str, day: &str, id: &str) -> ActivityRef {
    ActivityRef {
        facet: facet.to_owned(),
        day: day.to_owned(),
        id: id.to_owned(),
    }
}

fn with_list(rest: &[&str]) -> Result<Vec<String>, Failure> {
    let mut with = Vec::new();
    let mut iter = rest.iter();
    while let Some(flag) = iter.next() {
        match (*flag, iter.next()) {
            ("--with", Some(who)) => with.push((*who).to_owned()),
            _ => return Err(Failure::Usage),
        }
    }
    if with.is_empty() {
        return Err(Failure::Usage);
    }
    Ok(with)
}

fn service() -> ServiceId {
    ServiceId::new(SERVICE).expect("constant service id is valid")
}

// ---- resolve -------------------------------------------------------------

fn resolve(out: &Output<'_>, net: &Net, who: &str) -> Result<(), Failure> {
    let identity = resolve_identity(net.http.as_ref(), net.dns.as_ref(), who, &net.plc)?;
    let account = account_label(identity.handle.as_deref(), &identity.did);
    out.line(
        "journal.cli.social.resolve.done",
        &[
            ("account", Value::String(&account)),
            ("did", Value::String(&identity.did)),
            ("host", Value::String(&identity.pds)),
        ],
    );
    Ok(())
}

// ---- sign-in -------------------------------------------------------------

fn sign_in(out: &Output<'_>, net: &Net, journal: &Path, who: &str) -> Result<(), Failure> {
    let http = net.http.as_ref();
    let identity = resolve_identity(http, net.dns.as_ref(), who, &net.plc)?;
    let server = discover_auth_server(http, &identity.pds)?;
    let listener = LoopbackListener::bind(&LoopbackBindSpec::ephemeral(
        LoopbackHost::Ipv4Loopback,
        CALLBACK_PATH,
    ))?;
    let redirect_uri = listener.redirect_uri().to_owned();
    let scope = sign_in_scope();
    let client_id = loopback_client_id(&redirect_uri, &scope);
    let key = DpopKeyPair::generate()?;
    let pending = begin_authorization(
        http,
        &AuthorizationStart {
            server: &server,
            client_id: &client_id,
            redirect_uri: &redirect_uri,
            scope: &scope,
            login_hint: Some(who),
            expected_did: Some(&identity.did),
            dpop_key: &key,
            now_unix: now_unix_secs(),
        },
    )?;
    out.line(
        "journal.cli.social.signin.open",
        &[("address", Value::String(&pending.authorize_url))],
    );

    let policy = CallbackPolicy::new(&pending.state, &redirect_uri)
        .with_expected_issuer(&pending.issuer, true);
    let cancel = AtomicBool::new(false);
    let mut delivery = match receive_loopback_callback(&listener, &policy, SIGN_IN_WAIT, &cancel) {
        Ok(delivery) => delivery,
        Err(CallbackError::Timeout) => {
            return Err(Failure::Message(
                "journal.cli.social.signin.expired".to_owned(),
            ));
        }
        Err(err) => return Err(err.into()),
    };
    let callback = match delivery.outcome {
        CallbackOutcome::Accepted(callback) => callback,
        CallbackOutcome::ProviderError { error, .. } => {
            respond_to_callback_stream(Some(&mut delivery.stream), false);
            return Err(if error == "access_denied" {
                Failure::Message("journal.cli.social.signin.declined".to_owned())
            } else {
                Failure::Detail(error)
            });
        }
        CallbackOutcome::InvalidForAttempt(err) => {
            respond_to_callback_stream(Some(&mut delivery.stream), false);
            return Err(err.into());
        }
        CallbackOutcome::Ignored(_) => {
            respond_to_callback_stream(Some(&mut delivery.stream), false);
            return Err(Failure::Message(
                "journal.cli.social.signin.expired".to_owned(),
            ));
        }
    };
    let session = match complete_authorization(
        http,
        &pending,
        &callback.code,
        callback.issuer.as_deref(),
        &key,
        now_unix_secs(),
        &net.plc,
    ) {
        Ok(session) => session,
        Err(err) => {
            respond_to_callback_stream(Some(&mut delivery.stream), false);
            return Err(err.into());
        }
    };

    let service = service();
    let epoch = match load_credential_file(journal, &service) {
        LoadResult::Present(existing) => existing.epoch.saturating_add(1),
        LoadResult::Absent | LoadResult::Unreadable => 1,
    };
    let mut doc = OAuthCredentialDoc::new(&service, &session.token_endpoint);
    doc.epoch = epoch;
    doc.issuer = Some(session.issuer.clone());
    doc.resource = Some(session.pds.clone());
    doc.registration = Some(ClientRegistration {
        client_id: session.client_id.clone(),
        client_secret: None,
        mode: RegistrationMode::Static,
        client_refused: false,
    });
    doc.dpop_key_pkcs8_b64 = Some(key.to_pkcs8_b64());
    doc.dpop_nonce = session.tokens.dpop_nonce.clone();
    doc.subject = Some(session.did.clone());
    doc.account_label = identity.handle.clone();
    doc.tokens = Some(OAuthTokens {
        access_token: session.tokens.access_token.clone(),
        refresh_token: session.tokens.refresh_token.clone(),
        token_type: session.tokens.token_type.clone(),
        expires_at: session
            .tokens
            .expires_in
            .map(|secs| now_unix_secs().saturating_add(secs)),
        scopes: session.tokens.scopes.clone(),
    });
    if let Err(err) = save_credential_file(journal, &service, &doc) {
        respond_to_callback_stream(Some(&mut delivery.stream), false);
        return Err(err.into());
    }
    respond_to_callback_stream(Some(&mut delivery.stream), true);
    let account = account_label(identity.handle.as_deref(), &session.did);
    out.line(
        "journal.cli.social.signin.done",
        &[("account", Value::String(&account))],
    );
    Ok(())
}

fn account_label(handle: Option<&str>, did: &str) -> String {
    handle.map_or_else(|| did.to_owned(), ToOwned::to_owned)
}

/// The stored sign-in, refreshed if its access token is about to expire.
struct Signed {
    did: String,
    pds: String,
    access_token: String,
    dpop_key: DpopKeyPair,
}

fn signed_in(net: &Net, journal: &Path) -> Result<Signed, Failure> {
    let service = service();
    let LoadResult::Present(doc) = load_credential_file(journal, &service) else {
        return Err(Failure::SignedOut);
    };
    let (Some(did), Some(pds)) = (doc.subject.clone(), doc.resource.clone()) else {
        return Err(Failure::SignedOut);
    };
    let manager =
        OAuthCredentialManager::new(journal.to_path_buf(), service, Arc::clone(&net.http));
    let view = match manager.access_token() {
        Ok(view) => view,
        Err(CredentialError::SignInRequired) => return Err(Failure::SignedOut),
        Err(err) => return Err(err.into()),
    };
    let dpop_key = view
        .dpop_key_pkcs8_b64
        .as_deref()
        .ok_or(Failure::SignedOut)
        .and_then(|b64| DpopKeyPair::from_pkcs8_b64(b64).map_err(Failure::from))?;
    Ok(Signed {
        did,
        pds,
        access_token: view.access_token,
        dpop_key,
    })
}

impl Signed {
    fn session<'a>(&'a self, net: &'a Net) -> Session<'a> {
        Session {
            http: net.http.as_ref(),
            did: &self.did,
            pds: &self.pds,
            access_token: &self.access_token,
            dpop_key: &self.dpop_key,
            now_unix: now_unix_secs(),
            nonce: Default::default(),
        }
    }
}

// ---- status --------------------------------------------------------------

fn status(out: &Output<'_>, journal: &Path) -> Result<(), Failure> {
    match load_credential_file(journal, &service()) {
        LoadResult::Present(doc) if doc.tokens.is_some() => {
            let did = doc.subject.clone().unwrap_or_default();
            let account = account_label(doc.account_label.as_deref(), &did);
            let host = doc.resource.clone().unwrap_or_default();
            out.line(
                "journal.cli.social.status.account",
                &[
                    ("account", Value::String(&account)),
                    ("host", Value::String(&host)),
                ],
            );
        }
        _ => out.line("journal.cli.social.signed.out", &[]),
    }
    let spaces = followed_spaces(journal)?;
    let mut memories = 0;
    for space in &spaces {
        memories += store::received(journal, &space.space)?.len();
    }
    out.counted(
        "journal.cli.social.status.shares",
        "share_quantity",
        "shares",
        shares(journal)?.len(),
        &[],
    );
    out.counted(
        "journal.cli.social.status.spaces",
        "space_quantity",
        "spaces",
        spaces.len(),
        &[],
    );
    out.counted(
        "journal.cli.social.status.memories",
        "memory_quantity",
        "memories",
        memories,
        &[],
    );
    Ok(())
}

// ---- sharing -------------------------------------------------------------

fn share(
    out: &Output<'_>,
    net: &Net,
    journal: &Path,
    activity: &ActivityRef,
    with: &[String],
) -> Result<(), Failure> {
    let signed = signed_in(net, journal)?;
    let mut readers = Vec::new();
    for who in with {
        let identity = resolve_identity(net.http.as_ref(), net.dns.as_ref(), who, &net.plc)?;
        if identity.did != signed.did {
            readers.push(identity.did);
        }
    }
    if readers.is_empty() {
        return Err(Failure::Usage);
    }
    // Check the activity can be shared before creating a space for it.
    let title =
        crate::record::build_record(journal, activity, Timestamp::now(), Timestamp::now())?.title;
    let session = signed.session(net);
    let space = create_space(&session)?;
    let reader_refs: Vec<&str> = readers.iter().map(String::as_str).collect();
    add_readers(&session, &space, &reader_refs)?;
    let shared = share_activity(journal, &session, activity, &space, Timestamp::now())?;
    out.line(
        "journal.cli.social.share.done",
        &[
            ("title", Value::String(&title)),
            ("space", Value::String(&shared.space)),
        ],
    );
    Ok(())
}

fn sync(out: &Output<'_>, net: &Net, journal: &Path) -> Result<(), Failure> {
    if shares(journal)?.is_empty() {
        out.counted(
            "journal.cli.social.status.shares",
            "share_quantity",
            "shares",
            0,
            &[],
        );
        return Ok(());
    }
    let signed = signed_in(net, journal)?;
    let report = sync_shares(journal, &signed.session(net), Timestamp::now())?;
    if report.updated == 0 && report.removed == 0 {
        out.line("journal.cli.social.sync.current", &[]);
    }
    if report.updated > 0 {
        out.counted(
            "journal.cli.social.sync.updated",
            "activity_quantity",
            "activities",
            report.updated,
            &[],
        );
    }
    if report.removed > 0 {
        out.counted(
            "journal.cli.social.sync.removed",
            "activity_quantity",
            "activities",
            report.removed,
            &[],
        );
    }
    Ok(())
}

fn unshare(
    out: &Output<'_>,
    net: &Net,
    journal: &Path,
    activity: &ActivityRef,
) -> Result<(), Failure> {
    let held: Vec<_> = shares(journal)?
        .into_iter()
        .filter(|share| &share.activity == activity)
        .collect();
    if held.is_empty() {
        return Err(Failure::Message(
            "journal.cli.social.unshare.unknown".to_owned(),
        ));
    }
    let signed = signed_in(net, journal)?;
    let session = signed.session(net);
    for share in held {
        store::unshare(journal, &session, &share.space, &share.rkey)?;
    }
    out.line("journal.cli.social.unshare.done", &[]);
    Ok(())
}

// ---- receiving -----------------------------------------------------------

/// The sharer's handle, only when it verifies both ways against their DID.
fn verified_handle(net: &Net, did: &str) -> Option<String> {
    is_atproto_did(did)
        .then(|| resolve_identity(net.http.as_ref(), net.dns.as_ref(), did, &net.plc).ok())
        .flatten()
        .and_then(|identity| identity.handle)
}

fn follow(out: &Output<'_>, net: &Net, journal: &Path, space: &str) -> Result<(), Failure> {
    let authority = space_authority(space)?.to_owned();
    let signed = signed_in(net, journal)?;
    let handle = verified_handle(net, &authority);
    let report = receive_space(
        journal,
        &signed.session(net),
        space,
        handle.as_deref(),
        &net.plc,
        Timestamp::now(),
    )?;
    let sharer = account_label(handle.as_deref(), &authority);
    out.line(
        "journal.cli.social.follow.done",
        &[("sharer", Value::String(&sharer))],
    );
    report_receive(out, &sharer, &report);
    Ok(())
}

fn receive(out: &Output<'_>, net: &Net, journal: &Path) -> Result<(), Failure> {
    let spaces = followed_spaces(journal)?;
    if spaces.is_empty() {
        out.line("journal.cli.social.receive.nothing", &[]);
        return Ok(());
    }
    let signed = signed_in(net, journal)?;
    let mut failed = false;
    for followed in spaces {
        let sharer = account_label(
            followed.shared_by.handle.as_deref(),
            &followed.shared_by.did,
        );
        match receive_space(
            journal,
            &signed.session(net),
            &followed.space,
            followed.shared_by.handle.as_deref(),
            &net.plc,
            Timestamp::now(),
        ) {
            Ok(report) => report_receive(out, &sharer, &report),
            Err(err) => {
                failed = true;
                let detail = err.to_string();
                eprintln!(
                    "{}",
                    out.text(
                        "journal.cli.social.receive.failed",
                        &[
                            ("sharer", Value::String(&sharer)),
                            ("detail", Value::String(&detail)),
                        ],
                    )
                );
            }
        }
    }
    if failed { Err(Failure::Quiet) } else { Ok(()) }
}

fn report_receive(out: &Output<'_>, sharer: &str, report: &ReceiveReport) {
    let who = [("sharer", Value::String(sharer))];
    if report.added == 0 && report.updated == 0 && report.removed == 0 {
        out.line("journal.cli.social.receive.none", &who);
    }
    if report.added > 0 {
        out.counted(
            "journal.cli.social.receive.added",
            "activity_quantity",
            "activities",
            report.added,
            &who,
        );
    }
    if report.updated > 0 {
        out.counted(
            "journal.cli.social.receive.updated",
            "activity_quantity",
            "activities",
            report.updated,
            &who,
        );
    }
    if report.removed > 0 {
        out.counted(
            "journal.cli.social.receive.removed",
            "activity_quantity",
            "activities",
            report.removed,
            &who,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn words(items: &[&str]) -> Vec<OsString> {
        items.iter().map(OsString::from).collect()
    }

    #[test]
    fn help_reads_nothing() {
        assert!(is_help(&[]));
        assert!(is_help(&words(&["share", "--help"])));
        assert!(!is_help(&words(&["status"])));
    }

    #[test]
    fn a_share_needs_at_least_one_person() {
        assert!(with_list(&[]).is_err());
        assert!(with_list(&["--with"]).is_err());
        assert!(with_list(&["--to", "bob.test"]).is_err());
        assert_eq!(
            with_list(&["--with", "bob.test", "--with", "carol.test"]).ok(),
            Some(vec!["bob.test".to_owned(), "carol.test".to_owned()])
        );
    }
}
