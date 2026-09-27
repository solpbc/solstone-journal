// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Authorization endpoint for journal-hosted MCP OAuth.

use std::net::IpAddr;

use tokio::sync::watch;

#[cfg(all(test, not(feature = "full-tests")))]
use super::cimd::CimdAttemptIo;
#[cfg(all(test, not(feature = "full-tests")))]
use super::dcr::resolve_or_register_cimd_client_with_io;
use super::dcr::{CimdRegistrationError, resolve_or_register_cimd_client};
use super::rate_limit::PairingFailureRecord;
use super::redirect::{RedirectHost, parse_redirect_uri, redirect_uri_is_allowed};
use super::store::{IssuedAuthorization, OAuthStoreError, RegisteredClient};
use super::urlparse::{query_value_encode, validate_cimd_url};
use super::{
    AUTHORIZE_MAX_BODY_BYTES, MAX_CLIENT_ID_BYTES, MAX_CODE_BYTES, MAX_STATE_BYTES, OAuthRuntime,
    parse_urlencoded_pairs, reject_non_identity_encoding,
};
use crate::http1::{HttpRequest, HttpResponse};
use crate::permissions::{ReadPermission, ReadScope, resolve_permission_facet_names};
#[cfg(all(test, not(feature = "full-tests")))]
use crate::tokens::RandomSource;

const AUTHORIZE_CSP: &str = "default-src 'none'; style-src 'self' 'unsafe-inline'; font-src 'self'; form-action 'self'; frame-ancestors 'none'";
const AUTHORIZE_CSS: &str = r#"@font-face{font-family:Comfortaa;src:url('/authorize/assets/Comfortaa-Variable.woff2') format('woff2');font-display:swap;font-weight:300 700}
:root{color-scheme:light dark;--paper:#FCF3E4;--surface:#FEFCF8;--ink:#1A1A1A;--on-ink:#FEFCF8;--muted:#6E6453;--line:#E2D7BF;--field:#6E6453;--danger:#9F2D2D;--danger-wash:#F8E9E6;--accent:#B06A1A}
@media(prefers-color-scheme:dark){:root{--paper:#221C19;--surface:#2B231C;--ink:#FCF3E4;--on-ink:#1A1A1A;--muted:#B0A699;--line:#514840;--field:#B0A699;--danger:#D7998C;--danger-wash:#3F271F;--accent:#F2A451}}
body{margin:0;background:var(--paper);color:var(--ink);font:16px/1.5 system-ui,sans-serif}main{max-width:650px;margin:40px auto;background:var(--surface);border:1px solid var(--line);border-radius:18px;padding:28px}h1{font:700 28px/1.15 Comfortaa,system-ui,sans-serif}h2{font:700 16px/1.2 Comfortaa,system-ui,sans-serif;margin-top:24px}.muted{color:var(--muted)}fieldset{border:0;padding:0;margin:8px 0}input[type=checkbox]{accent-color:var(--accent)}.check{display:block;padding:5px 0}input[type=text]{display:block;width:100%;box-sizing:border-box;font:inherit;padding:11px;border:1px solid var(--field);border-radius:8px;background:var(--surface);color:var(--ink)}button{background:var(--ink);color:var(--on-ink);border:0;border-radius:8px;padding:11px 16px;font:700 16px/1.2 Comfortaa,system-ui,sans-serif;margin-top:18px}.error{background:var(--danger-wash);border-left:4px solid var(--danger);padding:10px}@media(max-width:700px){main{margin:0;border:0;border-radius:0;padding:22px;min-height:100vh;box-sizing:border-box}}
"#;

struct ConsentSelection {
    scope: Option<&'static str>,
    facets: Vec<String>,
    categories: Vec<String>,
}

impl ConsentSelection {
    fn initial() -> Self {
        Self {
            scope: None,
            facets: Vec::new(),
            categories: vec![
                "transcripts".to_owned(),
                "entities".to_owned(),
                "facets".to_owned(),
            ],
        }
    }
}

/// Which half of the consent flow a page serves. The journal's facets appear
/// only once the requester has shown they hold the pairing code.
#[derive(Clone, Copy, PartialEq, Eq)]
enum ConsentStage {
    Code { wrong_code: bool },
    Choose { incomplete: bool },
}

fn checked(selected: bool) -> &'static str {
    if selected { " checked" } else { "" }
}

const COMFORTAA: &[u8] =
    include_bytes!("../../../solstone-core-convey-shell/assets/static/Comfortaa-Variable.woff2");

pub(crate) fn design_css() -> HttpResponse {
    HttpResponse::bytes(
        200,
        "OK",
        "text/css; charset=utf-8",
        AUTHORIZE_CSS.as_bytes().to_vec(),
    )
    .with_header(
        "Cache-Control",
        "public, max-age=604800, immutable".to_owned(),
    )
}

pub(crate) fn comfortaa() -> HttpResponse {
    HttpResponse::bytes(200, "OK", "font/woff2", COMFORTAA.to_vec()).with_header(
        "Cache-Control",
        "public, max-age=604800, immutable".to_owned(),
    )
}

/// GET `/authorize`.
pub(crate) async fn get_authorize(
    request: &HttpRequest,
    source: IpAddr,
    oauth: &OAuthRuntime,
    shutdown: &mut watch::Receiver<bool>,
) -> HttpResponse {
    if let Err(response) = require_pairing_window(oauth) {
        return response;
    }
    let Some(pairs) = parse_get_pairs(request) else {
        return local_error("authorization request could not be started");
    };
    let Some(client_id) = field(&pairs, "client_id").filter(|value| !value.is_empty()) else {
        return local_error("authorization request could not be started");
    };
    let client = match resolve_live(oauth, source, client_id, shutdown).await {
        Ok(client) => client,
        Err(_) => return local_error("authorization request could not be started"),
    };
    finish_authorize_get(request, &pairs, client, source, oauth)
}

/// GET `/authorize` with injected CIMD I/O.
#[cfg(all(test, not(feature = "full-tests")))]
pub(crate) async fn get_authorize_with_io<IO: CimdAttemptIo>(
    request: &HttpRequest,
    source: IpAddr,
    oauth: &OAuthRuntime,
    shutdown: &mut watch::Receiver<bool>,
    io: IO,
    random: &dyn RandomSource,
) -> HttpResponse {
    if let Err(response) = require_pairing_window(oauth) {
        return response;
    }
    let Some(pairs) = parse_get_pairs(request) else {
        return local_error("authorization request could not be started");
    };
    let Some(client_id) = field(&pairs, "client_id").filter(|value| !value.is_empty()) else {
        return local_error("authorization request could not be started");
    };
    let client =
        match resolve_authorize_client(oauth, source, client_id, shutdown, io, random).await {
            Ok(client) => client,
            Err(_) => return local_error("authorization request could not be started"),
        };
    finish_authorize_get(request, &pairs, client, source, oauth)
}

fn parse_get_pairs(request: &HttpRequest) -> Option<Vec<(String, String)>> {
    let pairs = parse_urlencoded_pairs(query_from_target(&request.target))?;
    if oversize_authorize_get_fields(&pairs) {
        return None;
    }
    Some(pairs)
}

/// Registration and authorization answer only while the owner has a pairing
/// code open for this door. Outside that window nothing about the request is
/// parsed, no client metadata is fetched and nothing is stored.
fn require_pairing_window(oauth: &OAuthRuntime) -> Result<(), HttpResponse> {
    match oauth.store.pairing_window_open(&oauth.binding()) {
        Ok(true) => Ok(()),
        Ok(false) => Err(pairing_window_closed()),
        Err(_) => Err(local_error("authorization is temporarily unavailable")),
    }
}

/// The page a browser gets when no pairing code is open. It says nothing
/// about the journal behind it.
fn pairing_window_closed() -> HttpResponse {
    html_status(
        403,
        "Forbidden",
        "<h1>nothing is waiting to connect here.</h1><p>to connect an agent, open your journal, go to agents › connect an agent, and make a pairing code first. then connect from your agent again.</p>",
    )
}

fn finish_authorize_get(
    request: &HttpRequest,
    pairs: &[(String, String)],
    client: RegisteredClient,
    source: IpAddr,
    oauth: &OAuthRuntime,
) -> HttpResponse {
    let Some(redirect_uri) = field(pairs, "redirect_uri").filter(|value| !value.is_empty()) else {
        return local_error("authorization request could not be started");
    };
    if !redirect_uri_is_allowed(redirect_uri, &client.redirect_uris) {
        return local_error("authorization request could not be started");
    }
    let state = field(pairs, "state");
    let issuer = match oauth.published_origin(request) {
        Ok(issuer) => issuer,
        Err(response) => return response,
    };
    let expected_resource = match &oauth.resource_origin {
        super::ResourceOrigin::Fixed(_) | super::ResourceOrigin::Byo { .. } => {
            canonical_resource(oauth)
        }
        super::ResourceOrigin::Request => format!("{issuer}/mcp"),
    };
    if field(pairs, "response_type") != Some("code")
        || field(pairs, "code_challenge_method") != Some("S256")
        || field(pairs, "code_challenge").is_none_or(|value| value.is_empty())
        || field(pairs, "resource") != Some(expected_resource.as_str())
    {
        return error_redirect(redirect_uri, "invalid_request", state);
    }
    let code_challenge = field(pairs, "code_challenge").expect("challenge present");
    let stored_resource = canonical_resource(oauth);
    match oauth.store.create_transaction_at_door(
        &oauth.binding(),
        &client.id,
        redirect_uri,
        &stored_resource,
        &issuer,
        code_challenge,
        "S256",
        state,
        &oauth.source_cohort(source),
    ) {
        Ok(transaction_id) => consent_page(
            &client,
            redirect_uri,
            &transaction_id,
            oauth,
            ConsentStage::Code { wrong_code: false },
            &ConsentSelection::initial(),
        ),
        Err(OAuthStoreError::NoActivePairing) => pairing_window_closed(),
        Err(OAuthStoreError::Quota) => {
            error_redirect(redirect_uri, "temporarily_unavailable", state)
        }
        Err(_) => error_redirect(redirect_uri, "server_error", state),
    }
}

/// POST `/authorize`.
pub(crate) fn post_authorize(
    request: &HttpRequest,
    source: IpAddr,
    oauth: &OAuthRuntime,
) -> HttpResponse {
    if let Err(response) = reject_non_identity_encoding(request) {
        return response;
    }
    if request.body.len() > AUTHORIZE_MAX_BODY_BYTES {
        return HttpResponse::error(
            413,
            "Payload Too Large",
            "authorization request exceeds its size limit",
        );
    }
    let Ok(body) = std::str::from_utf8(&request.body) else {
        return local_error("authorization is temporarily unavailable");
    };
    let Some(pairs) = parse_urlencoded_pairs(body) else {
        return local_error("authorization is temporarily unavailable");
    };
    let transaction_id = field(&pairs, "transaction_id").unwrap_or("");
    let pairing_code = field(&pairs, "pairing_code").unwrap_or("");
    if transaction_id.is_empty()
        || transaction_id.len() > MAX_CODE_BYTES
        || pairing_code.len() > MAX_CODE_BYTES
    {
        return expired_error();
    }
    if pairing_code.is_empty() {
        return post_verified_choice(&pairs, transaction_id, oauth);
    }
    let generation = match oauth.store.pairing_generation() {
        Ok(generation) => generation,
        Err(_) => return local_error("authorization is temporarily unavailable"),
    };
    oauth.pairing_limiter.prune_generation(generation);
    if oauth.pairing_limiter.is_limited(source, generation) {
        return too_many_wrong_codes();
    }
    let categories = chosen_categories(&pairs);
    if categories.is_empty() {
        return local_error("choose what this agent may see before connecting");
    }
    // Facet names are never read or resolved here: until the code verifies,
    // nothing may reveal which facets this journal has.
    let (whole_journal, selected_scope) = match field(&pairs, "scope") {
        Some("whole_journal") => (true, "whole_journal"),
        Some("facets") => (false, "facets"),
        _ => return local_error("choose what this agent may see before connecting"),
    };
    let selection = ConsentSelection {
        scope: Some(selected_scope),
        facets: Vec::new(),
        categories: categories.clone(),
    };
    let outcome = if whole_journal {
        let permission = ReadPermission {
            categories,
            scope: ReadScope::WholeJournal,
        };
        oauth
            .store
            .complete_pairing_with_permission(
                transaction_id,
                pairing_code,
                Some(permission),
                &oauth.binding(),
            )
            .map(|issued| success_redirect(&issued))
    } else {
        oauth
            .store
            .verify_pairing(transaction_id, pairing_code, &oauth.binding())
            .map(|verified_id| {
                pending_page(
                    &verified_id,
                    oauth,
                    ConsentStage::Choose { incomplete: false },
                    &selection,
                )
            })
    };
    match outcome {
        Ok(response) => response,
        Err(OAuthStoreError::PairingMismatch) => {
            if oauth.pairing_limiter.record_failure(source, generation)
                == PairingFailureRecord::JustTripped
            {
                let _ = oauth.store.lock_pairing_code();
                return too_many_wrong_codes();
            }
            match oauth
                .store
                .pending_authorization(transaction_id, &oauth.binding())
            {
                Ok(Some(pending)) => consent_page(
                    &pending.client,
                    &pending.redirect_uri,
                    transaction_id,
                    oauth,
                    ConsentStage::Code { wrong_code: true },
                    &selection,
                ),
                Ok(None) => local_error(
                    "too many wrong codes for this request. connect from your agent again.",
                ),
                Err(_) => local_error("authorization is temporarily unavailable"),
            }
        }
        Err(error) => pairing_error(error),
    }
}

/// The second half of a facet-scoped consent: the pairing code already
/// verified, and the requester now names the facets.
fn post_verified_choice(
    pairs: &[(String, String)],
    transaction_id: &str,
    oauth: &OAuthRuntime,
) -> HttpResponse {
    let categories = chosen_categories(pairs);
    let names: Vec<String> = pairs
        .iter()
        .filter(|(key, _)| key == "facet")
        .map(|(_, value)| value.clone())
        .collect();
    let scope = field(pairs, "scope");
    let selection = ConsentSelection {
        scope: match scope {
            Some("whole_journal") => Some("whole_journal"),
            Some("facets") => Some("facets"),
            _ => None,
        },
        facets: names.clone(),
        categories: categories.clone(),
    };
    let incomplete = categories.is_empty()
        || selection.scope.is_none()
        || (selection.scope == Some("facets") && names.is_empty());
    if incomplete {
        return pending_page(
            transaction_id,
            oauth,
            ConsentStage::Choose { incomplete: true },
            &selection,
        );
    }
    let scope = if selection.scope == Some("whole_journal") {
        ReadScope::WholeJournal
    } else {
        match resolve_permission_facet_names(&oauth.journal_root, &names) {
            Ok(ids) => ReadScope::Facets { ids },
            Err(_) => {
                return local_error(
                    "the chosen facets could not be verified; return to your journal and try again",
                );
            }
        }
    };
    let permission = ReadPermission { categories, scope };
    match oauth
        .store
        .complete_verified_pairing(transaction_id, permission, &oauth.binding())
    {
        Ok(issued) => success_redirect(&issued),
        Err(error) => pairing_error(error),
    }
}

fn chosen_categories(pairs: &[(String, String)]) -> Vec<String> {
    pairs
        .iter()
        .filter(|(key, value)| {
            key == "category" && matches!(value.as_str(), "transcripts" | "entities" | "facets")
        })
        .map(|(_, value)| value.clone())
        .collect()
}

fn pending_page(
    transaction_id: &str,
    oauth: &OAuthRuntime,
    stage: ConsentStage,
    selection: &ConsentSelection,
) -> HttpResponse {
    match oauth
        .store
        .pending_authorization(transaction_id, &oauth.binding())
    {
        Ok(Some(pending))
            if pending.pairing_verified == matches!(stage, ConsentStage::Choose { .. }) =>
        {
            consent_page(
                &pending.client,
                &pending.redirect_uri,
                transaction_id,
                oauth,
                stage,
                selection,
            )
        }
        Ok(_) => expired_error(),
        Err(_) => local_error("authorization is temporarily unavailable"),
    }
}

/// The code is locked after too many wrong guesses; only a new one helps.
fn too_many_wrong_codes() -> HttpResponse {
    html_status(
        429,
        "Too Many Requests",
        "<p>too many wrong codes were entered. make a new code in the agents app, then connect from your agent again.</p>",
    )
}

fn expired_error() -> HttpResponse {
    local_error(
        "this request has expired or is no longer valid. connect from your agent again, with a new code from the agents app if yours is no longer open.",
    )
}

fn pairing_error(error: OAuthStoreError) -> HttpResponse {
    match error {
        OAuthStoreError::PairingLocked => local_error(
            "this pairing code no longer works. make a new one in the agents app, then connect from your agent again.",
        ),
        OAuthStoreError::TransactionNotFound
        | OAuthStoreError::TransactionExpired
        | OAuthStoreError::TransactionExhausted => expired_error(),
        OAuthStoreError::NoActivePairing => local_error(
            "there's no open pairing code. make one in the agents app, then connect from your agent again.",
        ),
        _ => local_error("authorization is temporarily unavailable"),
    }
}

#[cfg(all(test, not(feature = "full-tests")))]
async fn resolve_authorize_client<IO: CimdAttemptIo>(
    oauth: &OAuthRuntime,
    source: IpAddr,
    client_id: &str,
    shutdown: &mut watch::Receiver<bool>,
    io: IO,
    random: &dyn RandomSource,
) -> Result<RegisteredClient, CimdRegistrationError> {
    if validate_cimd_url(client_id).is_ok() {
        resolve_or_register_cimd_client_with_io(
            oauth, source, client_id, None, shutdown, io, random,
        )
        .await
        .map(|resolved| resolved.client)
    } else {
        match oauth.lookup_client_by_cimd_url(client_id) {
            Ok(Some(client)) => Ok(client),
            Ok(None) => Err(CimdRegistrationError::Fetch),
            Err(error) => Err(CimdRegistrationError::Store(error)),
        }
    }
}

async fn resolve_live(
    oauth: &OAuthRuntime,
    source: IpAddr,
    client_id: &str,
    shutdown: &mut watch::Receiver<bool>,
) -> Result<RegisteredClient, CimdRegistrationError> {
    if validate_cimd_url(client_id).is_ok() {
        resolve_or_register_cimd_client(oauth, source, client_id, None, shutdown)
            .await
            .map(|resolved| resolved.client)
    } else {
        match oauth.lookup_client_by_cimd_url(client_id) {
            Ok(Some(client)) => Ok(client),
            Ok(None) => Err(CimdRegistrationError::Fetch),
            Err(error) => Err(CimdRegistrationError::Store(error)),
        }
    }
}

fn oversize_authorize_get_fields(pairs: &[(String, String)]) -> bool {
    for (name, value) in pairs {
        let cap = match name.as_str() {
            "client_id" | "redirect_uri" | "resource" => MAX_CLIENT_ID_BYTES,
            "state" => MAX_STATE_BYTES,
            "code_challenge" | "transaction_id" | "pairing_code" => MAX_CODE_BYTES,
            _ => continue,
        };
        if value.len() > cap {
            return true;
        }
    }
    false
}

fn field<'a>(pairs: &'a [(String, String)], name: &str) -> Option<&'a str> {
    pairs
        .iter()
        .find(|(key, _)| key == name)
        .map(|(_, value)| value.as_str())
}

fn query_from_target(target: &str) -> &str {
    match target.split_once('?') {
        Some((_, query)) => query
            .split_once('#')
            .map(|(query, _)| query)
            .unwrap_or(query),
        None => "",
    }
}

fn canonical_resource(oauth: &OAuthRuntime) -> String {
    oauth.binding().canonical().to_owned()
}

fn html_escape(value: &str) -> String {
    let mut escaped = String::with_capacity(value.len());
    for char in value.chars() {
        match char {
            '&' => escaped.push_str("&amp;"),
            '<' => escaped.push_str("&lt;"),
            '>' => escaped.push_str("&gt;"),
            '"' => escaped.push_str("&quot;"),
            '\'' => escaped.push_str("&#39;"),
            other => escaped.push(other),
        }
    }
    escaped
}

fn html_status(status: u16, reason: &'static str, body: &str) -> HttpResponse {
    html_status_with_csp(status, reason, body, AUTHORIZE_CSP.to_owned())
}

/// The consent page's policy also admits the origin its form finally lands on.
///
/// A successful consent answers the form POST with a redirect to the client's
/// callback, which is on another origin. Browsers apply `form-action` to every
/// hop of that redirect, so a policy of `'self'` alone makes the browser refuse
/// the redirect and the sign-in never completes. The callback origin comes from
/// the closed redirect grammar, so it can never widen the policy past that one
/// admitted origin.
fn consent_csp(callback_origin: Option<&str>) -> String {
    match callback_origin {
        Some(origin) => AUTHORIZE_CSP.replace(
            "form-action 'self'",
            &format!("form-action 'self' {origin}"),
        ),
        None => AUTHORIZE_CSP.to_owned(),
    }
}

fn html_status_with_csp(
    status: u16,
    reason: &'static str,
    body: &str,
    csp: String,
) -> HttpResponse {
    HttpResponse::html(
        status,
        reason,
        format!(
            "<!DOCTYPE html><html><head><meta name=\"viewport\" content=\"width=device-width, initial-scale=1\"><link rel=\"stylesheet\" href=\"/authorize/assets/design.css\"></head><body><main>{body}</main></body></html>"
        ),
    )
    .with_header("Cache-Control", "no-store".to_owned())
    .with_header("Content-Security-Policy", csp)
    .with_header("X-Frame-Options", "DENY".to_owned())
}

fn local_error(message: &str) -> HttpResponse {
    html_status(
        400,
        "Bad Request",
        &format!("<p>{}</p>", html_escape(message)),
    )
}

fn consent_page(
    client: &RegisteredClient,
    redirect_uri: &str,
    transaction_id: &str,
    oauth: &OAuthRuntime,
    stage: ConsentStage,
    selection: &ConsentSelection,
) -> HttpResponse {
    let parsed = parse_redirect_uri(redirect_uri).ok();
    let host = parsed
        .as_ref()
        .map(redirect_host_label)
        .unwrap_or_else(|| "unknown".to_owned());
    let callback_origin = parsed.as_ref().map(redirect_origin);
    let client_label = client
        .client_name
        .as_deref()
        .unwrap_or(client.client_id.as_str());
    consent_page_named(
        client_label,
        &host,
        callback_origin.as_deref(),
        transaction_id,
        oauth,
        stage,
        selection,
    )
}

fn consent_page_named(
    client_label: &str,
    return_host: &str,
    callback_origin: Option<&str>,
    transaction_id: &str,
    oauth: &OAuthRuntime,
    stage: ConsentStage,
    selection: &ConsentSelection,
) -> HttpResponse {
    let client = html_escape(client_label);
    let transaction = html_escape(transaction_id);
    let facets_hint = match stage {
        ConsentStage::Code { .. } => "you'll choose them once your code matches. ",
        ConsentStage::Choose { .. } => "",
    };
    let choices = format!(
        r#"<fieldset><label class="check"><input type="radio" name="scope" value="whole_journal"{whole_checked}> <strong>your whole journal</strong><br><span class="muted">everything in your journal now, and anything added later, including facets you create later.</span></label><label class="check"><input type="radio" name="scope" value="facets"{facets_checked}> <strong>only the facets you choose</strong><br><span class="muted">{facets_hint}just the facets you pick, now and as they grow. facets you create later are not included.</span></label></fieldset>{facet_list}<fieldset><legend>what kinds of material</legend><label class="check"><input type="checkbox" name="category" value="transcripts"{transcripts_checked}> <strong>transcripts</strong><br><span class="muted">what was said in your recordings and imports, as text. never the audio or the screen frames themselves.</span></label><label class="check"><input type="checkbox" name="category" value="entities"{entities_checked}> <strong>entities</strong><br><span class="muted">the people, places and projects your journal knows, and what it has noted about them.</span></label><label class="check"><input type="checkbox" name="category" value="facets"{category_facets_checked}> <strong>facets</strong><br><span class="muted">the shape of your journal: facet names and descriptions, and the activities, events and summaries filed in them.</span></label></fieldset>"#,
        whole_checked = checked(selection.scope == Some("whole_journal")),
        facets_checked = checked(selection.scope == Some("facets")),
        facet_list = match stage {
            ConsentStage::Code { .. } => String::new(),
            ConsentStage::Choose { .. } => facet_list(oauth, selection),
        },
        transcripts_checked = checked(
            selection
                .categories
                .iter()
                .any(|value| value == "transcripts")
        ),
        entities_checked = checked(selection.categories.iter().any(|value| value == "entities")),
        category_facets_checked =
            checked(selection.categories.iter().any(|value| value == "facets")),
    );
    let body = match stage {
        ConsentStage::Code { wrong_code } => {
            let wrong = if wrong_code {
                "<p class=\"error\" role=\"alert\">that code didn't match. enter it again, or make a new one in the agents app, then connect from your agent again.</p>"
            } else {
                ""
            };
            format!(
                r#"<h1>{client} wants to connect to your journal.</h1><p>when you're done, it returns to <strong>{host}</strong>. it can read within what you choose, and can't add, change or delete anything.</p><form method="post" action="/authorize"><input type="hidden" name="transaction_id" value="{transaction}"><h2>what {client} may see</h2>{choices}<h2>pairing code</h2>{wrong}<label>enter the code shown in your journal, under agents › connect an agent<input type="text" name="pairing_code" autocomplete="one-time-code" spellcheck="false" required></label><p class="muted">being at your journal to read the code is what proves it's you. no password, no sign-in.</p><button type="submit">connect {client}</button><p class="muted">not you, or not expecting this? close this tab. nothing has been connected, and this code stays unused.</p></form>"#,
                host = html_escape(return_host),
            )
        }
        ConsentStage::Choose { incomplete } => {
            let notice = if incomplete {
                "<p class=\"error\" role=\"alert\">choose your whole journal or at least one facet, and at least one kind of material.</p>"
            } else {
                ""
            };
            format!(
                r#"<h1>choose what {client} may see.</h1><p>your code matched. {client} can read only within what you choose, and can't add, change or delete anything.</p><form method="post" action="/authorize"><input type="hidden" name="transaction_id" value="{transaction}">{notice}{choices}<button type="submit">connect {client}</button><p class="muted">changed your mind? close this tab. nothing is connected, and this code won't work again.</p></form>"#,
            )
        }
    };
    html_status_with_csp(200, "OK", &body, consent_csp(callback_origin))
}

/// The journal's facets as choices. Only a requester whose pairing code has
/// verified is ever shown this.
fn facet_list(oauth: &OAuthRuntime, selection: &ConsentSelection) -> String {
    let facets =
        solstone_core_facets::list_declared_facet_names(&oauth.journal_root).unwrap_or_default();
    if facets.is_empty() {
        return "<p class=\"muted\">your journal has no facets yet.</p>".to_owned();
    }
    let options = facets
        .into_iter()
        .map(|facet| {
            let declaration = solstone_core_facets::read_facet_declaration(&oauth.journal_root, &facet)
                .ok()
                .flatten();
            let title = declaration
                .as_ref()
                .map(|value| value.title.as_str())
                .filter(|value| !value.is_empty())
                .unwrap_or(&facet);
            let color = declaration
                .as_ref()
                .map(|value| value.color.as_str())
                .filter(|value| !value.is_empty())
                .unwrap_or("#76746b");
            let facet_checked = checked(selection.facets.contains(&facet));
            format!("<label class=\"check\"><input type=\"checkbox\" name=\"facet\" value=\"{}\"{facet_checked}> <span style=\"color:{}\">●</span> {}</label>", html_escape(&facet), html_escape(color), html_escape(title))
        })
        .collect::<String>();
    format!("<fieldset><legend>facets</legend>{options}</fieldset>")
}

fn redirect_host_label(parsed: &super::redirect::ParsedRedirectUri) -> String {
    let host = match parsed.host {
        RedirectHost::Localhost => "localhost",
        RedirectHost::V4Loopback => "127.0.0.1",
        RedirectHost::Claude => "claude.ai",
    };
    match parsed.port {
        Some(port) => format!("{host}:{port}"),
        None => host.to_owned(),
    }
}

fn redirect_origin(parsed: &super::redirect::ParsedRedirectUri) -> String {
    let scheme = match parsed.scheme {
        super::redirect::RedirectScheme::Http => "http",
        super::redirect::RedirectScheme::Https => "https",
    };
    format!("{scheme}://{}", redirect_host_label(parsed))
}

fn error_redirect(redirect_uri: &str, error: &str, state: Option<&str>) -> HttpResponse {
    let mut params = vec![("error", error)];
    if let Some(state) = state {
        params.push(("state", state));
    }
    redirect_to(redirect_uri, &params)
}

fn success_redirect(issued: &IssuedAuthorization) -> HttpResponse {
    let mut params = vec![
        ("code", issued.code.as_str()),
        ("iss", issued.issuer.as_str()),
    ];
    if let Some(state) = issued.state.as_deref() {
        params.push(("state", state));
    }
    redirect_to(&issued.redirect_uri, &params)
}

fn redirect_to(base: &str, params: &[(&str, &str)]) -> HttpResponse {
    let mut location = base.to_owned();
    let mut separator = if base.contains('?') { '&' } else { '?' };
    for (name, value) in params {
        location.push(separator);
        separator = '&';
        location.push_str(&query_value_encode(name));
        location.push('=');
        location.push_str(&query_value_encode(value));
    }
    HttpResponse::empty(302, "Found")
        .with_header("Location", location)
        .with_header("Cache-Control", "no-store".to_owned())
}

#[cfg(all(test, not(feature = "full-tests")))]
mod tests {
    use std::io;
    use std::net::{IpAddr, Ipv4Addr, SocketAddr};
    use std::pin::Pin;
    use std::task::{Context, Poll};

    use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
    use sha2::{Digest, Sha256};
    use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
    use tokio::sync::watch;

    use super::{comfortaa, design_css, get_authorize_with_io, post_authorize};
    use crate::http1::{HttpMethod, HttpRequest, HttpResponse};
    use crate::oauth::cimd::CimdAttemptIo;
    use crate::oauth::rate_limit::PairingFailureRecord;
    use crate::oauth::urlparse::query_value_encode;
    use crate::oauth::{AUTHORIZE_MAX_BODY_BYTES, OAuthRuntime};
    use crate::tokens::SystemRandomSource;

    const CIMD_URL: &str = "https://client.example/cimd.json";
    const SOURCE: IpAddr = IpAddr::V4(Ipv4Addr::new(198, 51, 100, 10));
    const ORIGIN: &str = "https://mcp.test";
    const REDIRECT: &str = "http://127.0.0.1/callback";

    fn journal_root() -> tempfile::TempDir {
        tempfile::Builder::new()
            .prefix("solstone-mcp-authorize-")
            .tempdir_in(crate::test_scratch())
            .unwrap()
    }

    /// A solstone.me runtime whose owner has a pairing code open, the only
    /// time registration and authorization answer at all.
    fn runtime(journal: &tempfile::TempDir) -> OAuthRuntime {
        let oauth = closed_runtime(journal);
        oauth
            .store
            .generate_pairing_code_with_door("relay")
            .unwrap();
        oauth
    }

    /// A solstone.me runtime with no pairing code open.
    fn closed_runtime(journal: &tempfile::TempDir) -> OAuthRuntime {
        OAuthRuntime::new(journal.path(), ORIGIN.to_owned())
    }

    fn pkce_challenge() -> String {
        URL_SAFE_NO_PAD.encode(Sha256::digest(b"pkce-verifier"))
    }

    fn header<'a>(response: &'a HttpResponse, name: &str) -> Option<&'a str> {
        response
            .extra_headers
            .iter()
            .find(|(candidate, _)| candidate.eq_ignore_ascii_case(name))
            .map(|(_, value)| value.as_str())
    }

    fn body_text(response: &HttpResponse) -> String {
        String::from_utf8(response.body.clone()).unwrap()
    }

    fn public_addr() -> SocketAddr {
        SocketAddr::from((Ipv4Addr::new(8, 8, 8, 8), 443))
    }

    fn cimd_http(name: &str) -> Vec<u8> {
        let body = format!(
            r#"{{"client_id":"{CIMD_URL}","redirect_uris":["{REDIRECT}"],"client_name":"{name}"}}"#
        );
        format!(
            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\r\n{body}",
            body.len()
        )
        .into_bytes()
    }

    struct FakeConnection {
        data: Vec<u8>,
        offset: usize,
    }

    impl AsyncRead for FakeConnection {
        fn poll_read(
            mut self: Pin<&mut Self>,
            _context: &mut Context<'_>,
            buf: &mut ReadBuf<'_>,
        ) -> Poll<io::Result<()>> {
            if self.offset >= self.data.len() {
                return Poll::Ready(Ok(()));
            }
            let remaining = self.data.len() - self.offset;
            let take = remaining.min(buf.remaining());
            buf.put_slice(&self.data[self.offset..self.offset + take]);
            self.offset += take;
            Poll::Ready(Ok(()))
        }
    }

    impl AsyncWrite for FakeConnection {
        fn poll_write(
            self: Pin<&mut Self>,
            _context: &mut Context<'_>,
            buf: &[u8],
        ) -> Poll<io::Result<usize>> {
            Poll::Ready(Ok(buf.len()))
        }
        fn poll_flush(self: Pin<&mut Self>, _context: &mut Context<'_>) -> Poll<io::Result<()>> {
            Poll::Ready(Ok(()))
        }
        fn poll_shutdown(self: Pin<&mut Self>, _context: &mut Context<'_>) -> Poll<io::Result<()>> {
            Poll::Ready(Ok(()))
        }
    }

    struct FakeIo {
        response: Vec<u8>,
        fail: bool,
    }

    impl FakeIo {
        fn ok(name: &str) -> Self {
            Self {
                response: cimd_http(name),
                fail: false,
            }
        }
        fn fail() -> Self {
            Self {
                response: Vec::new(),
                fail: true,
            }
        }
    }

    impl CimdAttemptIo for FakeIo {
        type Socket = ();
        type Connection = FakeConnection;

        async fn resolve(&mut self, _host: &str, _port: u16) -> io::Result<Vec<SocketAddr>> {
            Ok(vec![public_addr()])
        }
        async fn connect(&mut self, _address: SocketAddr) -> io::Result<Self::Socket> {
            Ok(())
        }
        async fn tls(
            &mut self,
            _socket: Self::Socket,
            _server_name: &str,
        ) -> io::Result<Self::Connection> {
            if self.fail {
                return Err(io::Error::other("CIMD TLS failed"));
            }
            Ok(FakeConnection {
                data: self.response.clone(),
                offset: 0,
            })
        }
    }

    fn get_request(query: &str) -> HttpRequest {
        let mut request = HttpRequest::from_test_parts(HttpMethod::Get, Vec::new(), Vec::new());
        request.target = format!("/authorize?{query}");
        request
    }

    fn post_request(body: &str) -> HttpRequest {
        HttpRequest::from_test_parts(HttpMethod::Post, Vec::new(), body.as_bytes().to_vec())
    }

    fn authorize_query(extra: &[(&str, &str)]) -> String {
        let challenge = pkce_challenge();
        let mut params = vec![
            ("client_id", CIMD_URL),
            ("redirect_uri", REDIRECT),
            ("response_type", "code"),
            ("code_challenge", challenge.as_str()),
            ("code_challenge_method", "S256"),
            ("resource", "https://mcp.test/mcp"),
        ];
        for (name, value) in extra {
            if let Some(existing) = params.iter_mut().find(|(key, _)| key == name) {
                existing.1 = value;
            } else {
                params.push((name, value));
            }
        }
        params
            .into_iter()
            .map(|(name, value)| format!("{name}={}", query_value_encode(value)))
            .collect::<Vec<_>>()
            .join("&")
    }

    async fn get_with(oauth: &OAuthRuntime, io: FakeIo, query: &str) -> HttpResponse {
        let (_tx, mut shutdown) = watch::channel(false);
        get_authorize_with_io(
            &get_request(query),
            SOURCE,
            oauth,
            &mut shutdown,
            io,
            &SystemRandomSource,
        )
        .await
    }

    fn hidden_transaction_id(body: &str) -> String {
        let marker = "name=\"transaction_id\" value=\"";
        let start = body.find(marker).expect("hidden transaction_id") + marker.len();
        let end = body[start..].find('"').expect("value terminator") + start;
        body[start..end].to_owned()
    }

    #[tokio::test(start_paused = true)]
    async fn get_happy_path_renders_consent_with_security_headers() {
        let journal = journal_root();
        let oauth = runtime(&journal);
        let response = get_with(&oauth, FakeIo::ok("fixture"), &authorize_query(&[])).await;
        assert_eq!(response.status, 200);
        assert_eq!(response.content_type, Some("text/html; charset=utf-8"));
        let body = body_text(&response);
        assert_eq!(body.matches("name=\"pairing_code\"").count(), 1);
        assert!(body.contains("/authorize/assets/design.css"));
        assert!(body.contains("<meta name=\"viewport\""));
        assert!(!body.contains("value=\"whole_journal\" checked"));
        assert!(body.contains("name=\"transaction_id\""));
        assert!(!body.contains("name=\"client_id\""));
        assert!(!body.contains("name=\"redirect_uri\""));
        assert_eq!(header(&response, "Cache-Control"), Some("no-store"));
        assert_eq!(header(&response, "X-Frame-Options"), Some("DENY"));
        let csp = header(&response, "Content-Security-Policy").unwrap();
        assert!(csp.contains("frame-ancestors 'none'"));
        // The consent form's redirect lands on the client's callback origin, and
        // browsers refuse that hop unless form-action admits it.
        assert!(
            csp.contains("form-action 'self' http://127.0.0.1;"),
            "{csp}"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn get_with_no_code_open_starts_nothing_and_fetches_nothing() {
        let journal = journal_root();
        let oauth = closed_runtime(&journal);
        // A fetch would fail and answer 400; the closed door answers before any fetch.
        let response = get_with(&oauth, FakeIo::fail(), &authorize_query(&[])).await;
        assert_eq!(response.status, 403);
        let body = body_text(&response);
        assert!(
            body.contains("nothing is waiting to connect here."),
            "{body}"
        );
        assert!(!body.contains("name=\"pairing_code\""));
        assert!(!body.contains("transaction_id"));
        assert!(
            !journal.path().join("mcp-endpoint/oauth.json").exists(),
            "a closed door stores nothing"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn only_a_live_code_for_this_door_opens_it_and_using_it_closes_it() {
        let journal = journal_root();
        let oauth = closed_runtime(&journal);
        oauth
            .store
            .generate_pairing_code_with_door("local")
            .unwrap();
        let other_door = get_with(&oauth, FakeIo::ok("fixture"), &authorize_query(&[])).await;
        assert_eq!(
            other_door.status, 403,
            "a code for this computer opens nothing here"
        );

        let pairing = oauth
            .store
            .generate_pairing_code_with_door("relay")
            .unwrap();
        let open = get_with(&oauth, FakeIo::ok("fixture"), &authorize_query(&[])).await;
        assert_eq!(open.status, 200);
        let transaction_id = hidden_transaction_id(&body_text(&open));
        let body = format!(
            "transaction_id={}&pairing_code={}&scope=whole_journal&category=transcripts",
            query_value_encode(&transaction_id),
            pairing.code
        );
        let connected = post_authorize(&post_request(&body), SOURCE, &oauth);
        assert_eq!(
            connected.status, 302,
            "the right code connects with no second step"
        );

        let after = get_with(&oauth, FakeIo::ok("fixture"), &authorize_query(&[])).await;
        assert_eq!(after.status, 403, "a used code closes the door again");
    }

    #[tokio::test(start_paused = true)]
    async fn consent_page_shows_no_mark_and_claims_no_identity() {
        let journal = journal_root();
        let oauth = runtime(&journal);
        let response = get_with(&oauth, FakeIo::ok("fixture"), &authorize_query(&[])).await;
        assert_eq!(response.status, 200);
        let body = body_text(&response);
        assert!(!body.contains("class=\"mark"), "{body}");
        assert!(!body.contains("the mark"), "{body}");
        assert!(!body.contains("this is your journal"), "{body}");
        assert!(!body_text(&design_css()).contains(".mark"));
    }

    #[test]
    fn consent_csp_admits_exactly_the_callback_origin_and_other_pages_do_not() {
        use super::super::redirect::parse_redirect_uri;
        use super::{AUTHORIZE_CSP, consent_csp, local_error, redirect_origin};
        let origin = |uri: &str| redirect_origin(&parse_redirect_uri(uri).unwrap());
        assert_eq!(
            origin("http://localhost:19876/callback"),
            "http://localhost:19876"
        );
        assert_eq!(origin("http://127.0.0.1/callback"), "http://127.0.0.1");
        assert_eq!(
            origin("https://claude.ai/api/mcp/auth_callback"),
            "https://claude.ai"
        );
        let csp = consent_csp(Some("http://localhost:19876"));
        assert!(
            csp.contains("form-action 'self' http://localhost:19876;"),
            "{csp}"
        );
        assert_eq!(csp.matches("form-action").count(), 1);
        assert_eq!(consent_csp(None), AUTHORIZE_CSP);
        let error = local_error("authorization request could not be started");
        assert_eq!(
            header(&error, "Content-Security-Policy"),
            Some(AUTHORIZE_CSP)
        );
    }

    #[test]
    fn design_assets_are_same_origin_and_self_contained() {
        let css = design_css();
        assert_eq!(css.content_type, Some("text/css; charset=utf-8"));
        let css_body = body_text(&css);
        assert!(css_body.contains("/authorize/assets/Comfortaa-Variable.woff2"));

        let font = comfortaa();
        assert_eq!(font.content_type, Some("font/woff2"));
        assert!(font.body.starts_with(b"wOF2"));
    }

    struct UnusedIo;

    impl CimdAttemptIo for UnusedIo {
        type Socket = ();
        type Connection = FakeConnection;

        async fn resolve(&mut self, _host: &str, _port: u16) -> io::Result<Vec<SocketAddr>> {
            Err(io::Error::other("known CIMD client must not fetch"))
        }
        async fn connect(&mut self, _address: SocketAddr) -> io::Result<Self::Socket> {
            Err(io::Error::other("known CIMD client must not fetch"))
        }
        async fn tls(
            &mut self,
            _socket: Self::Socket,
            _server_name: &str,
        ) -> io::Result<Self::Connection> {
            Err(io::Error::other("known CIMD client must not fetch"))
        }
    }

    #[tokio::test(start_paused = true)]
    async fn get_known_cimd_client_does_not_fetch() {
        let journal = journal_root();
        let oauth = runtime(&journal);
        let first = get_with(&oauth, FakeIo::ok("fixture"), &authorize_query(&[])).await;
        assert_eq!(first.status, 200);
        let (_tx, mut shutdown) = watch::channel(false);
        let second = get_authorize_with_io(
            &get_request(&authorize_query(&[])),
            SOURCE,
            &oauth,
            &mut shutdown,
            UnusedIo,
            &SystemRandomSource,
        )
        .await;
        assert_eq!(second.status, 200);
        assert!(body_text(&second).contains("name=\"transaction_id\""));
        assert!(header(&second, "Location").is_none());
    }

    #[tokio::test(start_paused = true)]
    async fn get_unfetchable_client_has_no_location() {
        let journal = journal_root();
        let oauth = runtime(&journal);
        let response = get_with(&oauth, FakeIo::fail(), &authorize_query(&[])).await;
        assert_eq!(response.status, 400);
        assert!(header(&response, "Location").is_none());
    }

    #[tokio::test(start_paused = true)]
    async fn get_unknown_classic_id_has_no_location() {
        let journal = journal_root();
        let oauth = runtime(&journal);
        let (_tx, mut shutdown) = watch::channel(false);
        let mut request = get_request(
            "client_id=oauth%3Adcr%3Aunknown&redirect_uri=http%3A%2F%2F127.0.0.1%2Fcallback&response_type=code&code_challenge=abc&code_challenge_method=S256&resource=https%3A%2F%2Fmcp.test%2Fmcp",
        );
        request.target = "/authorize?client_id=oauth%3Adcr%3Aunknown&redirect_uri=http%3A%2F%2F127.0.0.1%2Fcallback&response_type=code&code_challenge=abc&code_challenge_method=S256&resource=https%3A%2F%2Fmcp.test%2Fmcp".into();
        let response = get_authorize_with_io(
            &request,
            SOURCE,
            &oauth,
            &mut shutdown,
            FakeIo::fail(),
            &SystemRandomSource,
        )
        .await;
        assert_eq!(response.status, 400);
        assert!(header(&response, "Location").is_none());
    }

    #[tokio::test(start_paused = true)]
    async fn get_unregistered_redirect_has_no_location() {
        let journal = journal_root();
        let oauth = runtime(&journal);
        let query = authorize_query(&[("redirect_uri", "http://127.0.0.1/other")]);
        let response = get_with(&oauth, FakeIo::ok("fixture"), &query).await;
        assert_eq!(response.status, 400);
        assert!(header(&response, "Location").is_none());
    }

    #[tokio::test(start_paused = true)]
    async fn get_plain_pkce_redirects_with_encoded_state() {
        let journal = journal_root();
        let oauth = runtime(&journal);
        let state = "a&b=#x";
        for method in [None, Some("plain")] {
            let mut extras = vec![("state", state)];
            let query = if let Some(method) = method {
                extras.push(("code_challenge_method", method));
                authorize_query(&extras)
            } else {
                authorize_query(&extras).replace("&code_challenge_method=S256", "")
            };
            let response = get_with(&oauth, FakeIo::ok("fixture"), &query).await;
            assert_eq!(response.status, 302);
            let location = header(&response, "Location").unwrap();
            assert!(location.starts_with(REDIRECT));
            assert!(location.contains("error=invalid_request"));
            assert!(location.contains(&format!("state={}", query_value_encode(state))));
            assert!(!location.contains("state=a&b"));
            assert!(!location.contains("#x"));
            assert_eq!(header(&response, "Cache-Control"), Some("no-store"));
        }
    }

    #[tokio::test(start_paused = true)]
    async fn get_wrong_resource_redirects() {
        let journal = journal_root();
        let oauth = runtime(&journal);
        let query = authorize_query(&[("resource", "https://mcp.test/other")]);
        let response = get_with(&oauth, FakeIo::ok("fixture"), &query).await;
        assert_eq!(response.status, 302);
        let location = header(&response, "Location").unwrap();
        assert!(location.contains("error=invalid_request"));
    }

    #[tokio::test(start_paused = true)]
    async fn get_consent_escapes_client_name() {
        let journal = journal_root();
        let oauth = runtime(&journal);
        let response = get_with(
            &oauth,
            FakeIo::ok("<script>alert(1)</script>"),
            &authorize_query(&[]),
        )
        .await;
        let body = body_text(&response);
        assert!(!body.contains("<script>"));
        assert!(body.contains("&lt;script&gt;"));
    }

    #[tokio::test(start_paused = true)]
    async fn post_happy_path_issues_a_redeemable_code() {
        let journal = journal_root();
        let oauth = runtime(&journal);
        let pairing = oauth
            .store
            .generate_pairing_code_with_door("relay")
            .unwrap();
        let get = get_with(
            &oauth,
            FakeIo::ok("fixture"),
            &authorize_query(&[("state", "st&ate")]),
        )
        .await;
        let transaction_id = hidden_transaction_id(&body_text(&get));
        let response = post_authorize(
            &post_request(&format!(
                "transaction_id={}&pairing_code={}&scope=whole_journal&category=transcripts",
                query_value_encode(&transaction_id),
                query_value_encode(&pairing.code)
            )),
            SOURCE,
            &oauth,
        );
        assert_eq!(response.status, 302);
        assert_eq!(header(&response, "Cache-Control"), Some("no-store"));
        let location = header(&response, "Location").unwrap();
        assert!(location.contains(&format!("state={}", query_value_encode("st&ate"))));
        assert!(location.contains(&format!("iss={}", query_value_encode(ORIGIN))));
        let code = location
            .split(['?', '&'])
            .find_map(|piece| piece.strip_prefix("code="))
            .unwrap();
        let tokens = oauth
            .store
            .redeem_authorization_code(
                code,
                CIMD_URL,
                REDIRECT,
                "https://mcp.test/mcp",
                "pkce-verifier",
                &oauth.binding(),
            )
            .unwrap();
        assert!(!tokens.access_token.is_empty());
        let grant = oauth.store.list_grants().unwrap().pop().unwrap();
        let permission = crate::permissions::PermissionStore::open(journal.path())
            .get_permission(&format!("oauth:{}", grant.id))
            .unwrap()
            .unwrap();
        assert_eq!(permission.generation, 1);
        assert_eq!(permission.read.unwrap().categories, vec!["transcripts"]);
    }

    #[tokio::test(start_paused = true)]
    async fn post_wrong_code_retries_then_exhausts() {
        let journal = journal_root();
        let oauth = runtime(&journal);
        oauth
            .store
            .generate_pairing_code_with_door("relay")
            .unwrap();
        let get = get_with(&oauth, FakeIo::ok("fixture"), &authorize_query(&[])).await;
        let transaction_id = hidden_transaction_id(&body_text(&get));
        let body = format!(
            "transaction_id={}&pairing_code=00000000&scope=whole_journal&category=transcripts",
            query_value_encode(&transaction_id)
        );
        for _ in 0..4 {
            let response = post_authorize(&post_request(&body), SOURCE, &oauth);
            let page = body_text(&response);
            assert!(page.contains("that code didn't match"));
            assert!(page.contains(&transaction_id));
        }
        let fifth = post_authorize(&post_request(&body), SOURCE, &oauth);
        let page = body_text(&fifth);
        assert!(page.contains("too many wrong codes for this request"));
        assert!(!page.contains("name=\"transaction_id\""));
        let sixth = post_authorize(&post_request(&body), SOURCE, &oauth);
        assert!(body_text(&sixth).contains("no longer valid"));
    }

    #[tokio::test(start_paused = true)]
    async fn post_wrong_code_preserves_client_and_narrowed_selection() {
        let journal = journal_root();
        solstone_core_facets::create_facet(journal.path(), "work", "Work", "", "#123456", "", None)
            .unwrap();
        solstone_core_facets::create_facet(
            journal.path(),
            "personal",
            "Personal",
            "",
            "#654321",
            "",
            None,
        )
        .unwrap();
        let oauth = runtime(&journal);
        oauth
            .store
            .generate_pairing_code_with_door("relay")
            .unwrap();
        let get = get_with(&oauth, FakeIo::ok("fixture"), &authorize_query(&[])).await;
        let transaction_id = hidden_transaction_id(&body_text(&get));
        let response = post_authorize(
            &post_request(&format!(
                "transaction_id={}&pairing_code=00000000&scope=facets&facet=work&facet=personal&category=transcripts&category=facets",
                query_value_encode(&transaction_id)
            )),
            SOURCE,
            &oauth,
        );

        let page = body_text(&response);
        assert!(page.contains("fixture wants to connect"));
        assert!(page.contains("returns to <strong>127.0.0.1</strong>"));
        assert!(page.contains("name=\"scope\" value=\"whole_journal\""));
        assert!(!page.contains("name=\"scope\" value=\"whole_journal\" checked"));
        assert!(page.contains("name=\"scope\" value=\"facets\" checked"));
        assert!(!page.contains("name=\"facet\""));
        assert!(!page.contains("Work") && !page.contains("Personal"));
        assert!(page.contains("name=\"category\" value=\"transcripts\" checked"));
        assert!(!page.contains("name=\"category\" value=\"entities\" checked"));
        assert!(page.contains("name=\"category\" value=\"facets\" checked"));
    }

    fn create_sensitive_facet(journal: &tempfile::TempDir) {
        solstone_core_facets::create_facet(
            journal.path(),
            "therapy",
            "Therapy with Dana",
            "",
            "#a1b2c3",
            "",
            None,
        )
        .unwrap();
    }

    fn assert_no_facet_content(page: &str) {
        for marker in ["therapy", "Therapy with Dana", "#a1b2c3", "name=\"facet\""] {
            assert!(!page.contains(marker), "{marker} leaked: {page}");
        }
    }

    /// Every door reachable from beyond this computer: solstone.me, the owner's
    /// own hostname and the LAN/VPN door.
    fn public_doors(journal: &tempfile::TempDir) -> Vec<(&'static str, OAuthRuntime, String)> {
        let lan_host = "192.168.1.20:7660";
        vec![
            ("relay", runtime(journal), format!("{ORIGIN}/mcp")),
            (
                "byo",
                OAuthRuntime::new_byo(journal.path(), "journal.example", 1),
                "https://journal.example/mcp".to_owned(),
            ),
            (
                "lan",
                OAuthRuntime::new_lan_door(journal.path()),
                format!("https://{lan_host}/mcp"),
            ),
        ]
    }

    #[tokio::test(start_paused = true)]
    async fn no_door_beyond_this_computer_shows_facets_before_the_code_verifies() {
        let (_tx, mut shutdown) = watch::channel(false);
        for door in ["relay", "byo", "lan"] {
            let journal = journal_root();
            if door == "byo" {
                let config_dir = journal.path().join("config");
                std::fs::create_dir_all(&config_dir).unwrap();
                std::fs::write(
                    config_dir.join("journal.json"),
                    serde_json::to_string(&serde_json::json!({
                        "mcp_endpoint": {
                            "byo_hostname": {
                                "hostname": "journal.example",
                                "enabled": true,
                                "generation": 1
                            }
                        }
                    }))
                    .unwrap(),
                )
                .unwrap();
            }
            create_sensitive_facet(&journal);
            let (_, oauth, resource) = public_doors(&journal)
                .into_iter()
                .find(|(name, _, _)| *name == door)
                .unwrap();
            oauth.store.generate_pairing_code_with_door(door).unwrap();
            let registration = crate::oauth::dcr::register_with_io(
                &post_request(&format!(r#"{{"redirect_uris":["{REDIRECT}"]}}"#)),
                SOURCE,
                &oauth,
                &mut shutdown,
                UnusedIo,
                &SystemRandomSource,
            )
            .await;
            assert_eq!(registration.status, 201, "{door}");
            let registered: serde_json::Value = serde_json::from_slice(&registration.body).unwrap();
            let client_id = registered["client_id"].as_str().unwrap();
            assert!(client_id.starts_with("oauth:dcr:"), "{door}");

            let mut request = HttpRequest::from_test_parts(
                HttpMethod::Get,
                vec![("Host".to_owned(), "192.168.1.20:7660".to_owned())],
                Vec::new(),
            );
            request.target = format!(
                "/authorize?{}",
                authorize_query(&[("client_id", client_id), ("resource", resource.as_str())])
            );
            let get = get_authorize_with_io(
                &request,
                SOURCE,
                &oauth,
                &mut shutdown,
                UnusedIo,
                &SystemRandomSource,
            )
            .await;
            assert_eq!(get.status, 200, "{door}");
            let page = body_text(&get);
            assert!(page.contains("name=\"pairing_code\""), "{door}");
            assert_no_facet_content(&page);

            // A wrong code re-renders the page, and whether a named facet
            // exists changes nothing in the answer.
            let transaction_id = hidden_transaction_id(&page);
            let wrong_code = |facet: &str| {
                post_authorize(
                    &post_request(&format!(
                        "transaction_id={}&pairing_code=00000000&scope=facets&facet={facet}&category=transcripts",
                        query_value_encode(&transaction_id)
                    )),
                    SOURCE,
                    &oauth,
                )
            };
            let existing = wrong_code("therapy");
            let missing = wrong_code("no-such-facet");
            let page = body_text(&existing);
            assert!(page.contains("that code didn't match"), "{door}: {page}");
            assert_no_facet_content(&page);
            assert_eq!(existing.status, missing.status, "{door}");
            assert_eq!(existing.body, missing.body, "{door}");
        }
    }

    #[tokio::test(start_paused = true)]
    async fn facets_are_chosen_after_the_code_verifies() {
        let journal = journal_root();
        create_sensitive_facet(&journal);
        solstone_core_facets::create_facet(journal.path(), "work", "Work", "", "#123456", "", None)
            .unwrap();
        let oauth = runtime(&journal);
        let pairing = oauth
            .store
            .generate_pairing_code_with_door("relay")
            .unwrap();
        let get = get_with(
            &oauth,
            FakeIo::ok("fixture"),
            &authorize_query(&[("state", "st")]),
        )
        .await;
        let first_id = hidden_transaction_id(&body_text(&get));
        let verified = post_authorize(
            &post_request(&format!(
                "transaction_id={}&pairing_code={}&scope=facets&category=transcripts",
                query_value_encode(&first_id),
                query_value_encode(&pairing.code)
            )),
            SOURCE,
            &oauth,
        );
        assert_eq!(verified.status, 200);
        assert!(header(&verified, "Location").is_none());
        let page = body_text(&verified);
        assert!(page.contains("your code matched"));
        assert!(page.contains("name=\"facet\" value=\"therapy\""));
        assert!(page.contains("Therapy with Dana"));
        assert!(!page.contains("name=\"pairing_code\""));
        let verified_id = hidden_transaction_id(&page);
        assert_ne!(verified_id, first_id);

        // The identifier on the page served before the code was checked is spent.
        let stale = post_authorize(
            &post_request(&format!(
                "transaction_id={}&scope=facets&facet=therapy&category=transcripts",
                query_value_encode(&first_id)
            )),
            SOURCE,
            &oauth,
        );
        assert_eq!(stale.status, 400);
        assert!(body_text(&stale).contains("no longer valid"));

        let nothing_chosen = post_authorize(
            &post_request(&format!(
                "transaction_id={}&scope=facets&category=transcripts",
                query_value_encode(&verified_id)
            )),
            SOURCE,
            &oauth,
        );
        assert_eq!(nothing_chosen.status, 200);
        assert!(body_text(&nothing_chosen).contains("at least one facet"));

        let chosen = post_authorize(
            &post_request(&format!(
                "transaction_id={}&scope=facets&facet=work&category=transcripts",
                query_value_encode(&verified_id)
            )),
            SOURCE,
            &oauth,
        );
        assert_eq!(chosen.status, 302);
        let location = header(&chosen, "Location").unwrap();
        assert!(location.contains("state=st"));
        let code = location
            .split(['?', '&'])
            .find_map(|piece| piece.strip_prefix("code="))
            .unwrap();
        oauth
            .store
            .redeem_authorization_code(
                code,
                CIMD_URL,
                REDIRECT,
                "https://mcp.test/mcp",
                "pkce-verifier",
                &oauth.binding(),
            )
            .unwrap();
        let grant = oauth.store.list_grants().unwrap().pop().unwrap();
        let permission = crate::permissions::PermissionStore::open(journal.path())
            .get_permission(&format!("oauth:{}", grant.id))
            .unwrap()
            .unwrap()
            .read
            .unwrap();
        let work =
            crate::permissions::resolve_permission_facet_names(journal.path(), &["work".into()])
                .unwrap();
        assert_eq!(
            permission.scope,
            crate::permissions::ReadScope::Facets { ids: work }
        );
        assert_eq!(permission.categories, vec!["transcripts"]);
    }

    #[tokio::test(start_paused = true)]
    async fn choosing_facets_needs_a_verified_code() {
        let journal = journal_root();
        create_sensitive_facet(&journal);
        let oauth = runtime(&journal);
        oauth
            .store
            .generate_pairing_code_with_door("relay")
            .unwrap();
        let get = get_with(&oauth, FakeIo::ok("fixture"), &authorize_query(&[])).await;
        let transaction_id = hidden_transaction_id(&body_text(&get));
        for body in [
            "scope=facets&facet=therapy&category=transcripts",
            "scope=facets&category=transcripts",
        ] {
            let response = post_authorize(
                &post_request(&format!(
                    "transaction_id={}&{body}",
                    query_value_encode(&transaction_id)
                )),
                SOURCE,
                &oauth,
            );
            assert_eq!(response.status, 400);
            assert!(header(&response, "Location").is_none());
            let page = body_text(&response);
            assert!(page.contains("no longer valid"));
            assert_no_facet_content(&page);
        }
        assert!(oauth.store.current_pairing_code().unwrap().is_some());
    }

    #[tokio::test(start_paused = true)]
    async fn post_twenty_failures_lock_pairing_and_then_429() {
        let journal = journal_root();
        let oauth = runtime(&journal);
        oauth
            .store
            .generate_pairing_code_with_door("relay")
            .unwrap();
        let mut last = HttpResponse::error(500, "Internal Server Error", "unset");
        for _ in 0..4 {
            let get = get_with(&oauth, FakeIo::ok("fixture"), &authorize_query(&[])).await;
            let transaction_id = hidden_transaction_id(&body_text(&get));
            let body = format!(
                "transaction_id={}&pairing_code=00000000&scope=whole_journal&category=transcripts",
                query_value_encode(&transaction_id)
            );
            for _ in 0..5 {
                last = post_authorize(&post_request(&body), SOURCE, &oauth);
            }
        }
        // The twentieth wrong code locks the code, and says so rather than
        // sending the owner back to an agent that would find the door shut.
        assert_eq!(last.status, 429);
        assert!(body_text(&last).contains("make a new code in the agents app"));
        let limited = post_authorize(
            &post_request("transaction_id=x&pairing_code=00000000"),
            SOURCE,
            &oauth,
        );
        assert_eq!(limited.status, 429);
        // The locked code opens nothing: a new request is not even started.
        let closed = get_with(&oauth, FakeIo::ok("fixture"), &authorize_query(&[])).await;
        assert_eq!(closed.status, 403);
        assert!(body_text(&closed).contains("nothing is waiting to connect here."));
        let client = oauth
            .store
            .lookup_client_by_cimd_url(CIMD_URL)
            .unwrap()
            .unwrap();
        assert!(matches!(
            oauth.store.create_transaction(
                &client.id,
                REDIRECT,
                "https://mcp.test/mcp",
                ORIGIN,
                &pkce_challenge(),
                "S256",
                None,
                &SOURCE.to_string(),
            ),
            Err(crate::oauth::store::OAuthStoreError::NoActivePairing)
        ));
        assert!(
            oauth
                .store
                .current_pairing_code()
                .unwrap()
                .is_some_and(|p| p.locked)
        );
    }

    #[tokio::test(start_paused = true)]
    async fn post_ingress_bounds_do_not_touch_the_store() {
        let journal = journal_root();
        let oauth = closed_runtime(&journal);
        let mut oversize = b"transaction_id=x&pairing_code=00000000&pad=".to_vec();
        oversize.resize(AUTHORIZE_MAX_BODY_BYTES + 1, b'x');
        let too_large = post_authorize(
            &HttpRequest::from_test_parts(HttpMethod::Post, Vec::new(), oversize),
            SOURCE,
            &oauth,
        );
        assert_eq!(too_large.status, 413);
        let encoded = post_authorize(
            &HttpRequest::from_test_parts(
                HttpMethod::Post,
                vec![("Content-Encoding".to_owned(), "gzip".to_owned())],
                b"transaction_id=x&pairing_code=00000000".to_vec(),
            ),
            SOURCE,
            &oauth,
        );
        assert_eq!(encoded.status, 400);
        assert!(!journal.path().join("mcp-endpoint/oauth.json").exists());
    }

    #[tokio::test(start_paused = true)]
    async fn post_ignores_spoofed_redirect_and_state_fields() {
        let journal = journal_root();
        let oauth = runtime(&journal);
        let pairing = oauth
            .store
            .generate_pairing_code_with_door("relay")
            .unwrap();
        let get = get_with(
            &oauth,
            FakeIo::ok("fixture"),
            &authorize_query(&[("state", "bound")]),
        )
        .await;
        let transaction_id = hidden_transaction_id(&body_text(&get));
        let response = post_authorize(
            &post_request(&format!(
                "transaction_id={}&pairing_code={}&scope=whole_journal&category=transcripts&redirect_uri={}&state=spoofed",
                query_value_encode(&transaction_id),
                query_value_encode(&pairing.code),
                query_value_encode("http://127.0.0.1/evil"),
            )),
            SOURCE,
            &oauth,
        );
        assert_eq!(response.status, 302);
        let location = header(&response, "Location").unwrap();
        assert!(location.starts_with(REDIRECT));
        assert!(!location.contains("127.0.0.1/evil"));
        assert!(location.contains(&format!("state={}", query_value_encode("bound"))));
        assert!(!location.contains("state=spoofed"));
    }

    #[tokio::test(start_paused = true)]
    async fn post_prunes_stale_generation_limiter_buckets() {
        let journal = journal_root();
        let oauth = runtime(&journal);
        oauth
            .store
            .generate_pairing_code_with_door("relay")
            .unwrap();
        let old_generation = oauth.store.pairing_generation().unwrap();
        let overflow = IpAddr::V4(Ipv4Addr::from(u32::MAX));
        for index in 0..u32::MAX {
            if oauth.pairing_limiter.is_limited(overflow, old_generation) {
                break;
            }
            oauth
                .pairing_limiter
                .record_failure(IpAddr::V4(Ipv4Addr::from(index)), old_generation);
        }
        assert!(oauth.pairing_limiter.is_limited(overflow, old_generation));

        oauth
            .store
            .generate_pairing_code_with_door("relay")
            .unwrap();
        let new_generation = oauth.store.pairing_generation().unwrap();
        assert_ne!(new_generation, old_generation);

        let response = post_authorize(
            &post_request("transaction_id=x&pairing_code=00000000"),
            SOURCE,
            &oauth,
        );
        assert_eq!(response.status, 400);
        assert!(!oauth.pairing_limiter.is_limited(overflow, old_generation));
        let fresh = IpAddr::V4(Ipv4Addr::new(203, 0, 113, 9));
        assert!(!oauth.pairing_limiter.is_limited(fresh, new_generation));
    }

    #[tokio::test(start_paused = true)]
    async fn post_authorize_on_wrong_runtime_matches_unknown_transaction_without_mutating_state() {
        let journal = journal_root();
        let unbound = OAuthRuntime::new(journal.path(), "https://mcp.test".to_owned());
        let bound = OAuthRuntime::new_bound(journal.path(), "http://127.0.0.1:7659".to_owned());

        let client = unbound
            .store
            .register_client(CIMD_URL, vec![REDIRECT.to_owned()], None, "192.0.2.1")
            .unwrap();
        let pairing = unbound
            .store
            .generate_pairing_code_with_door("relay")
            .unwrap();
        let challenge = pkce_challenge();
        let tx = unbound
            .store
            .create_transaction(
                &client.id,
                REDIRECT,
                "https://mcp.test/mcp",
                "https://mcp.test",
                &challenge,
                "S256",
                None,
                "192.0.2.1",
            )
            .unwrap();

        let unknown_resp = post_authorize(
            &post_request(
                "transaction_id=unknown-id&pairing_code=00000000&scope=whole_journal&category=transcripts",
            ),
            SOURCE,
            &bound,
        );

        // 1. Right code on wrong runtime matches unknown transaction
        let resp_right = post_authorize(
            &post_request(&format!(
                "transaction_id={tx}&pairing_code={}&scope=whole_journal&category=transcripts",
                pairing.code
            )),
            SOURCE,
            &bound,
        );
        assert_eq!(resp_right.status, unknown_resp.status);
        assert_eq!(resp_right.body, unknown_resp.body);
        assert_eq!(resp_right.extra_headers, unknown_resp.extra_headers);

        // 2. Wrong code on wrong runtime matches unknown transaction
        let resp_wrong = post_authorize(
            &post_request(&format!(
                "transaction_id={tx}&pairing_code=00000000&scope=whole_journal&category=transcripts"
            )),
            SOURCE,
            &bound,
        );
        assert_eq!(resp_wrong.status, unknown_resp.status);
        assert_eq!(resp_wrong.body, unknown_resp.body);
        assert_eq!(resp_wrong.extra_headers, unknown_resp.extra_headers);

        // failure_count stays 0 and pairing is not locked on bound
        let path = journal.path().join("mcp-endpoint/oauth.json");
        let file: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        let pending = file["pending"]
            .as_array()
            .unwrap()
            .iter()
            .find(|p| p["transaction_id"] == tx)
            .unwrap();
        assert_eq!(pending["failure_count"], 0);
        assert_eq!(file["pairing"]["locked"], false);

        // record_failure 20 times on bound's limiter; 20th is JustTripped
        let generation = bound.store.pairing_generation().unwrap();
        for _ in 0..19 {
            assert_eq!(
                bound.pairing_limiter.record_failure(SOURCE, generation),
                PairingFailureRecord::Counted
            );
        }
        assert_eq!(
            bound.pairing_limiter.record_failure(SOURCE, generation),
            PairingFailureRecord::JustTripped
        );

        // Transaction completes on unbound
        let auth = unbound
            .store
            .complete_pairing(&tx, &pairing.code, &unbound.binding())
            .unwrap();

        // 3. Already-issued code on wrong runtime returns unknown transaction page
        let resp_already = post_authorize(
            &post_request(&format!(
                "transaction_id={tx}&pairing_code={}&scope=whole_journal&category=transcripts",
                pairing.code
            )),
            SOURCE,
            &bound,
        );
        assert_eq!(resp_already.status, unknown_resp.status);
        assert_eq!(resp_already.body, unknown_resp.body);
        assert_eq!(resp_already.extra_headers, unknown_resp.extra_headers);

        // Still redeems on unbound
        let tokens = unbound
            .store
            .redeem_authorization_code(
                &auth.code,
                CIMD_URL,
                REDIRECT,
                "https://mcp.test/mcp",
                "pkce-verifier",
                &unbound.binding(),
            )
            .unwrap();
        assert!(!tokens.access_token.is_empty());

        // 4. A transaction posted to the wrong runtime: matches unknown transaction page
        let unbound_p2 = unbound
            .store
            .generate_pairing_code_with_door("relay")
            .unwrap();
        let tx2 = unbound
            .store
            .create_transaction(
                &client.id,
                REDIRECT,
                "https://mcp.test/mcp",
                "https://mcp.test",
                &challenge,
                "S256",
                None,
                "192.0.2.1",
            )
            .unwrap();
        let resp_no_pairing = post_authorize(
            &post_request(&format!(
                "transaction_id={tx2}&pairing_code=00000000&scope=whole_journal&category=transcripts"
            )),
            SOURCE,
            &bound,
        );
        assert_eq!(resp_no_pairing.status, unknown_resp.status);
        assert_eq!(resp_no_pairing.body, unknown_resp.body);
        assert_eq!(resp_no_pairing.extra_headers, unknown_resp.extra_headers);

        assert!(
            unbound
                .store
                .complete_pairing(&tx2, &unbound_p2.code, &unbound.binding())
                .is_ok()
        );

        // 5. Pairing locked on wrong runtime: matches unknown transaction page, not locked page
        unbound
            .store
            .generate_pairing_code_with_door("relay")
            .unwrap();
        let tx3 = unbound
            .store
            .create_transaction(
                &client.id,
                REDIRECT,
                "https://mcp.test/mcp",
                "https://mcp.test",
                &challenge,
                "S256",
                None,
                "192.0.2.1",
            )
            .unwrap();
        let _bound_p3 = bound
            .store
            .generate_pairing_code_with_door("local")
            .unwrap();
        bound.store.lock_pairing_code().unwrap();
        assert!(bound.store.current_pairing_code().unwrap().unwrap().locked);

        let resp_locked = post_authorize(
            &post_request(&format!(
                "transaction_id={tx3}&pairing_code=00000000&scope=whole_journal&category=transcripts"
            )),
            SOURCE,
            &bound,
        );
        assert_eq!(resp_locked.status, unknown_resp.status);
        assert_eq!(resp_locked.body, unknown_resp.body);
        assert_eq!(resp_locked.extra_headers, unknown_resp.extra_headers);
        assert!(bound.store.current_pairing_code().unwrap().unwrap().locked);

        // 6. failure_count written to 5 directly in oauth.json, posted to wrong runtime -> row still present
        unbound
            .store
            .generate_pairing_code_with_door("relay")
            .unwrap();
        let tx4 = unbound
            .store
            .create_transaction(
                &client.id,
                REDIRECT,
                "https://mcp.test/mcp",
                "https://mcp.test",
                &challenge,
                "S256",
                None,
                "192.0.2.1",
            )
            .unwrap();
        let mut file: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        let p4 = file["pending"]
            .as_array_mut()
            .unwrap()
            .iter_mut()
            .find(|p| p["transaction_id"] == tx4)
            .unwrap();
        p4["failure_count"] = serde_json::json!(5);
        std::fs::write(&path, serde_json::to_vec(&file).unwrap()).unwrap();

        let _ = post_authorize(
            &post_request(&format!(
                "transaction_id={tx4}&pairing_code=00000000&scope=whole_journal&category=transcripts"
            )),
            SOURCE,
            &bound,
        );

        let file_after: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        assert!(
            file_after["pending"]
                .as_array()
                .unwrap()
                .iter()
                .any(|p| p["transaction_id"] == tx4)
        );
    }

    #[test]
    fn consent_error_responses_omit_cli_command_references() {
        let locked = super::pairing_error(super::OAuthStoreError::PairingLocked);
        let locked_body = body_text(&locked);
        assert!(!locked_body.contains("journal mcp pairing generate"));

        let no_pairing = super::pairing_error(super::OAuthStoreError::NoActivePairing);
        let no_pairing_body = body_text(&no_pairing);
        assert!(!no_pairing_body.contains("journal mcp pairing generate"));
    }
}
