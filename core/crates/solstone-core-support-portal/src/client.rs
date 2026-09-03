// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Durable identity state and the reference-compatible registration client.

use std::fs;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::Serialize;
use serde_json::Value;

use crate::dpop::{create_dpop_proof, json_ascii};
use crate::errors::PortalClientError;
use crate::keypair::{Keypair, save_keypair};
use crate::token::{create_access_token, sign_tos};

const DEFAULT_PORTAL_URL: &str = "https://support.solstone.app";

/// The complete response needed for TOS classification, diagnostics, and text fetches.
#[derive(Debug)]
pub(crate) struct PortalResponse {
    pub(crate) status: u16,
    pub(crate) body: String,
}

pub(crate) trait PortalTransport {
    fn request(
        &mut self,
        method: &str,
        url: &str,
        headers: &[(String, String)],
        body: RequestBody,
    ) -> Result<PortalResponse, PortalClientError>;
}

pub(crate) enum RequestBody {
    None,
    Json(String),
    Multipart(Vec<MultipartPart>),
}

struct AuthedRequestOptions<'files, 'reader> {
    json_body: Option<String>,
    params: Option<Vec<(String, String)>>,
    files: Option<&'files mut [MultipartInput<'reader>]>,
    idempotency_key: Option<String>,
}

pub(crate) trait PortalRuntime {
    fn now(&mut self) -> i64;
    fn uuid(&mut self) -> String;
    fn random_bytes(&mut self, bytes: &mut [u8]) -> Result<(), PortalClientError>;
    /// Override point for fixed test key material. Returning `Some` skips real
    /// RSA generation in `ensure_keypair`. Production runtime always returns `None`.
    fn keypair_pem(&mut self) -> Option<Vec<u8>> {
        None
    }
}

pub(crate) struct ProductionRuntime;

impl PortalRuntime for ProductionRuntime {
    fn now(&mut self) -> i64 {
        chrono::Utc::now().timestamp()
    }

    fn uuid(&mut self) -> String {
        uuid::Uuid::new_v4().to_string()
    }

    fn random_bytes(&mut self, bytes: &mut [u8]) -> Result<(), PortalClientError> {
        getrandom::fill(bytes).map_err(|error| PortalClientError::Storage {
            message: error.to_string(),
        })
    }
}

struct UreqPortalTransport {
    agent: ureq::Agent,
}

impl UreqPortalTransport {
    fn new() -> Self {
        // `max_redirects(0)` returns a 3xx response; it never follows or raises it.
        let config = ureq::Agent::config_builder()
            .http_status_as_error(false)
            .max_redirects(0)
            .timeout_global(Some(Duration::from_secs(30)))
            .build();
        Self {
            agent: ureq::Agent::new_with_config(config),
        }
    }

    fn response(
        mut response: ureq::http::Response<ureq::Body>,
    ) -> Result<PortalResponse, PortalClientError> {
        let status = response.status().as_u16();
        let body = response
            .body_mut()
            .with_config()
            .limit(100 * 1024 * 1024)
            .lossy_utf8(true)
            .read_to_string()
            .map_err(|error| PortalClientError::Transport {
                message: error.to_string(),
            })?;
        Ok(PortalResponse { status, body })
    }
}

impl PortalTransport for UreqPortalTransport {
    fn request(
        &mut self,
        method: &str,
        url: &str,
        headers: &[(String, String)],
        body: RequestBody,
    ) -> Result<PortalResponse, PortalClientError> {
        let build = |content_type: Option<&str>| {
            let mut builder = ureq::http::Request::builder().method(method).uri(url);
            for (name, value) in headers {
                builder = builder.header(name, value);
            }
            if let Some(content_type) = content_type {
                builder = builder.header("Content-Type", content_type);
            }
            builder
        };
        match body {
            RequestBody::None => Self::response(
                self.agent
                    .run(build(None).body(()).map_err(transport_error)?)
                    .map_err(transport_error)?,
            ),
            RequestBody::Json(body) => Self::response(
                self.agent
                    .run(
                        build(Some("application/json"))
                            .body(body)
                            .map_err(transport_error)?,
                    )
                    .map_err(transport_error)?,
            ),
            RequestBody::Multipart(files) => {
                use ureq::unversioned::multipart::{Form, Part};
                let mut form = Form::new();
                for file in &files {
                    let mut part = Part::bytes(&file.bytes).file_name(&file.filename);
                    if let Some(content_type) = &file.content_type {
                        part = part.mime_str(content_type).map_err(transport_error)?;
                    }
                    form = form.part(&file.name, part);
                }
                // Agent::run consumes AsSendBody directly and, unlike
                // RequestBuilder::send, does not copy the body's inferred
                // content type onto the request. Preserve the generated
                // boundary explicitly for the portal's multipart parser.
                let content_type = format!("multipart/form-data; boundary={}", form.boundary());
                Self::response(
                    self.agent
                        .run(
                            build(Some(&content_type))
                                .body(form)
                                .map_err(transport_error)?,
                        )
                        .map_err(transport_error)?,
                )
            }
        }
    }
}

fn transport_error(error: impl std::fmt::Display) -> PortalClientError {
    PortalClientError::Transport {
        message: error.to_string(),
    }
}

pub(crate) trait ReadSeek: Read + Seek {}
impl<T: Read + Seek + ?Sized> ReadSeek for T {}

pub(crate) struct MultipartInput<'a> {
    pub(crate) name: String,
    pub(crate) filename: String,
    pub(crate) content_type: Option<String>,
    pub(crate) reader: &'a mut dyn ReadSeek,
}

#[derive(Clone)]
pub(crate) struct MultipartPart {
    pub(crate) name: String,
    pub(crate) filename: String,
    pub(crate) content_type: Option<String>,
    pub(crate) bytes: Vec<u8>,
}

/// A mutable, storage-rooted support portal client.
pub struct PortalClient {
    portal_url: String,
    storage_dir: PathBuf,
    anonymous: bool,
    handle: Option<String>,
    keypair: Option<Keypair>,
    access_token: Option<String>,
    tos: Option<String>,
    transport: Box<dyn PortalTransport>,
    runtime: Box<dyn PortalRuntime>,
}

impl PortalClient {
    /// Construct a client using the production clock, entropy, and HTTP transport.
    pub fn new(
        portal_url: impl Into<String>,
        storage_dir: impl Into<PathBuf>,
        handle: Option<String>,
        anonymous: bool,
    ) -> Result<Self, PortalClientError> {
        Self::new_with(
            portal_url,
            storage_dir,
            handle,
            anonymous,
            Box::new(UreqPortalTransport::new()),
            Box::new(ProductionRuntime),
        )
    }

    /// Construct a client using the support settings rooted at one journal.
    pub fn from_journal_settings(
        journal_root: &Path,
        portal_url: Option<&str>,
        anonymous: bool,
    ) -> Result<Self, PortalClientError> {
        let portal_url = portal_url
            .map(|url| url.trim_end_matches('/').to_owned())
            .unwrap_or_else(|| portal_url_from_settings(journal_root));
        Self::new(
            portal_url,
            journal_root.join("apps/support/portal"),
            None,
            anonymous,
        )
    }

    pub(crate) fn new_with(
        portal_url: impl Into<String>,
        storage_dir: impl Into<PathBuf>,
        mut handle: Option<String>,
        anonymous: bool,
        transport: Box<dyn PortalTransport>,
        mut runtime: Box<dyn PortalRuntime>,
    ) -> Result<Self, PortalClientError> {
        let storage_dir = storage_dir.into();
        fs::create_dir_all(&storage_dir).map_err(storage_error)?;
        if anonymous && handle.as_deref().is_none_or(str::is_empty) {
            let mut bytes = [0; 4];
            runtime.random_bytes(&mut bytes)?;
            handle = Some(format!("anon-{}", hex(&bytes)));
        }
        let mut client = Self {
            portal_url: portal_url.into().trim_end_matches('/').to_owned(),
            storage_dir,
            anonymous,
            handle,
            keypair: None,
            access_token: None,
            tos: None,
            transport,
            runtime,
        };
        client.load_state()?;
        Ok(client)
    }

    /// Memoize a generated client handle exactly as the Python property does.
    pub fn handle(&mut self) -> &str {
        if self.handle.as_deref().is_none_or(str::is_empty) {
            let hostname = native_hostname().to_lowercase();
            let hostname: String = hostname
                .replace('_', "-")
                .chars()
                .take(48)
                .filter(|character| character.is_alphanumeric() || matches!(character, '.' | '-'))
                .collect();
            self.handle = Some(format!("solstone-{}", hostname.trim_matches(['.', '-'])));
            if self.handle.as_deref() == Some("solstone-") {
                self.handle = Some("solstone-solstone".to_owned());
            }
        }
        self.handle.as_deref().expect("just initialized")
    }

    pub(crate) fn load_state(&mut self) -> Result<(), PortalClientError> {
        if self.anonymous {
            return Ok(());
        }
        let key_path = self.keypair_path();
        if key_path.is_file() {
            // Unlike token/TOS, malformed key material escapes construction.
            self.keypair = Some(Keypair::from_pem(
                &fs::read(key_path).map_err(storage_error)?,
            )?);
        }
        if let Ok(text) = fs::read_to_string(self.token_path()) {
            match serde_json::from_str::<Value>(&text) {
                Ok(Value::Object(value)) => {
                    self.access_token = value
                        .get("access_token")
                        .and_then(Value::as_str)
                        .map(str::to_owned);
                    if let Some(handle) = value.get("handle") {
                        self.handle = handle.as_str().map(str::to_owned);
                    }
                }
                Ok(_) => {
                    return Err(PortalClientError::State {
                        message: "token.json is not an object".to_owned(),
                    });
                }
                Err(_) => {}
            }
        }
        if let Ok(bytes) = fs::read(self.tos_path()) {
            self.tos =
                Some(
                    String::from_utf8(bytes).map_err(|error| PortalClientError::State {
                        message: error.to_string(),
                    })?,
                );
        }
        Ok(())
    }

    fn save_token(&self, access_token: &str) -> Result<(), PortalClientError> {
        // Deliberately no chmod: token.json remains at the process umask as in Python.
        if self.anonymous {
            return Ok(());
        }
        let text = json_ascii(&TokenFile {
            access_token,
            handle: self.handle.as_deref(),
        })
        .map_err(|error| PortalClientError::State {
            message: error.to_string(),
        })?;
        fs::write(self.token_path(), text).map_err(storage_error)
    }

    fn save_tos(&self, tos: &str) -> Result<(), PortalClientError> {
        if self.anonymous {
            return Ok(());
        }
        fs::write(self.tos_path(), tos).map_err(storage_error)
    }

    pub(crate) fn ensure_keypair(&mut self) -> Result<(), PortalClientError> {
        if self.keypair.is_some() {
            return Ok(());
        }
        let (keypair, pem) = match self.runtime.keypair_pem() {
            Some(pem) => (Keypair::from_pem(&pem)?, pem),
            None => Keypair::generate()?,
        };
        if !self.anonymous {
            save_keypair(&self.keypair_path(), &pem)?;
        }
        self.keypair = Some(keypair);
        Ok(())
    }

    pub(crate) fn principal(&mut self) -> Result<String, PortalClientError> {
        // This apparent derivation may create RSA-4096 state, matching the reference.
        if self.anonymous {
            return Ok("anonymous".to_owned());
        }
        self.ensure_keypair()?;
        Ok(format!(
            "jkt:{}",
            self.keypair
                .as_ref()
                .expect("ensure_keypair set it")
                .thumbprint
        ))
    }

    /// Return whether both durable identity components are currently present.
    pub fn is_registered(&self) -> bool {
        self.keypair.is_some() && self.access_token.is_some()
    }

    /// Return cached terms without issuing a request.
    pub fn cached_tos(&self) -> Option<&str> {
        self.tos.as_deref()
    }

    #[allow(
        dead_code,
        reason = "the private credential accessor remains reserved for in-crate protocol tests."
    )]
    pub(crate) fn access_token(&self) -> Option<&str> {
        self.access_token.as_deref()
    }

    pub fn fetch_tos(&mut self) -> Result<String, PortalClientError> {
        let url = format!("{}/tos", self.portal_url);
        let response = self.transport.request(
            "GET",
            &url,
            &[("Accept".to_owned(), "text/plain".to_owned())],
            RequestBody::None,
        )?;
        self.raise_for_status("GET", &url, &response)?;
        self.save_tos(&response.body)?;
        self.tos = Some(response.body.clone());
        Ok(response.body)
    }

    /// Register unconditionally. Only ensure_registered performs the reference guard.
    pub fn register(&mut self) -> Result<(), PortalClientError> {
        self.ensure_keypair()?;
        for attempt in 0..=3 {
            let tos = self.fetch_tos()?;
            let keypair = self.keypair.as_ref().expect("ensure_keypair set it");
            let access = create_access_token(
                &keypair.signer,
                &tos,
                &self.portal_url,
                &keypair.thumbprint,
                &self.runtime.uuid(),
                self.runtime.now(),
            )?;
            let dpop = create_dpop_proof(
                &keypair.signer,
                &keypair.jwk,
                "POST",
                &format!("{}/api/signup", self.portal_url),
                &self.runtime.uuid(),
                self.runtime.now(),
                None,
            )?;
            let body = json_ascii(&SignupBody {
                tos_signature: sign_tos(&keypair.signer, &tos)?,
                access_token: access.clone(),
                handle: self.handle().to_owned(),
            })
            .map_err(|error| PortalClientError::State {
                message: error.to_string(),
            })?;
            let url = format!("{}/api/signup", self.portal_url);
            let response = self.transport.request(
                "POST",
                &url,
                &[("DPoP".to_owned(), dpop)],
                RequestBody::Json(body),
            )?;
            if response.status == 409 {
                if attempt >= 3 {
                    return Err(PortalClientError::HandleCollision);
                }
                let mut random = [0; 4];
                self.runtime.random_bytes(&mut random)?;
                let current = self.handle.take().expect("handle was used above");
                // The Python regex strips any four lower alphanumeric chars, not one value.
                let base = if current.len() >= 5
                    && current.as_bytes()[current.len() - 5] == b'-'
                    && current[current.len() - 4..]
                        .chars()
                        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit())
                {
                    &current[..current.len() - 5]
                } else {
                    &current
                };
                self.handle = Some(format!("{base}-{}", suffix(&random)));
                continue;
            }
            self.raise_for_status("POST", &url, &response)?;
            let data: Value =
                serde_json::from_str(&response.body).map_err(|error| PortalClientError::State {
                    message: error.to_string(),
                })?;
            let received = data
                .get("access_token")
                .and_then(Value::as_str)
                .ok_or_else(|| PortalClientError::State {
                    message: "signup response has no access_token".to_owned(),
                })?
                .to_owned();
            if let Some(handle) = data.get("handle") {
                self.handle = handle.as_str().map(str::to_owned);
            }
            self.access_token = Some(received.clone());
            self.save_token(&received)?;
            return Ok(());
        }
        unreachable!("attempt loop returns after four collisions")
    }

    pub fn ensure_registered(&mut self) -> Result<(), PortalClientError> {
        if self.is_registered() {
            return Ok(());
        }
        self.register()
    }

    pub(crate) fn authed_request(
        &mut self,
        method: &str,
        path: &str,
        json_body: Option<&str>,
        params: Option<&[(String, String)]>,
        files: Option<&mut [MultipartInput<'_>]>,
        idempotency_key: Option<&str>,
    ) -> Result<PortalResponse, PortalClientError> {
        let mut options = AuthedRequestOptions {
            json_body: json_body.map(str::to_owned),
            params: params.map(ToOwned::to_owned),
            files,
            idempotency_key: idempotency_key.map(str::to_owned),
        };
        self.authed_request_inner(method, path, &mut options, true)
    }

    fn authed_request_inner(
        &mut self,
        method: &str,
        path: &str,
        options: &mut AuthedRequestOptions<'_, '_>,
        retry_on_tos: bool,
    ) -> Result<PortalResponse, PortalClientError> {
        let url = format!("{}{}", self.portal_url, path);
        let token = self
            .access_token
            .clone()
            .ok_or_else(|| PortalClientError::State {
                message: "access token is missing".to_owned(),
            })?;
        let keypair = self
            .keypair
            .as_ref()
            .ok_or_else(|| PortalClientError::State {
                message: "keypair is missing".to_owned(),
            })?;
        let dpop = create_dpop_proof(
            &keypair.signer,
            &keypair.jwk,
            method,
            &url,
            &self.runtime.uuid(),
            self.runtime.now(),
            Some(&token),
        )?;
        let mut headers = vec![
            ("Authorization".to_owned(), format!("DPoP {token}")),
            ("DPoP".to_owned(), dpop),
        ];
        if let Some(key) = &options.idempotency_key {
            headers.push(("Idempotency-Key".to_owned(), key.clone()));
        }
        let wire_url = append_params(&url, options.params.as_deref());
        let body = if let Some(body) = &options.json_body {
            RequestBody::Json(body.clone())
        } else if let Some(inputs) = options.files.as_deref_mut() {
            RequestBody::Multipart(rewind_files(inputs)?)
        } else {
            RequestBody::None
        };
        let response = self.transport.request(method, &wire_url, &headers, body)?;
        if response.status == 401 && retry_on_tos {
            let error = serde_json::from_str::<Value>(&response.body)
                .ok()
                .and_then(|body| body.get("error").and_then(Value::as_str).map(str::to_owned));
            if error.as_deref() == Some("tos_changed") {
                self.register()?;
                return self.authed_request_inner(method, path, options, false);
            }
        }
        Ok(response)
    }

    pub(crate) fn raise_for_status(
        &self,
        method: &str,
        url: &str,
        response: &PortalResponse,
    ) -> Result<(), PortalClientError> {
        if (200..300).contains(&response.status) {
            return Ok(());
        }
        let body: String = response.body.chars().take(500).collect();
        Err(PortalClientError::HttpStatus {
            message: format!("{method} {url} — {}: {body}", response.status),
        })
    }

    fn keypair_path(&self) -> PathBuf {
        self.storage_dir.join("keypair.pem")
    }
    fn token_path(&self) -> PathBuf {
        self.storage_dir.join("token.json")
    }
    fn tos_path(&self) -> PathBuf {
        self.storage_dir.join("tos.txt")
    }
}

#[cfg(unix)]
fn native_hostname() -> String {
    nix::unistd::gethostname()
        .expect("read local hostname")
        .to_string_lossy()
        .into_owned()
}

#[cfg(windows)]
fn native_hostname() -> String {
    std::env::var_os("COMPUTERNAME")
        .unwrap_or_else(|| "solstone".into())
        .to_string_lossy()
        .into_owned()
}

#[cfg(not(any(unix, windows)))]
fn native_hostname() -> String {
    "solstone".to_owned()
}

#[derive(Serialize)]
struct TokenFile<'a> {
    access_token: &'a str,
    handle: Option<&'a str>,
}

#[derive(Serialize)]
struct SignupBody {
    tos_signature: String,
    access_token: String,
    handle: String,
}

fn rewind_files(
    inputs: &mut [MultipartInput<'_>],
) -> Result<Vec<MultipartPart>, PortalClientError> {
    inputs
        .iter_mut()
        .map(|input| {
            input
                .reader
                .seek(SeekFrom::Start(0))
                .map_err(storage_error)?;
            let mut bytes = Vec::new();
            input
                .reader
                .read_to_end(&mut bytes)
                .map_err(storage_error)?;
            Ok(MultipartPart {
                name: input.name.clone(),
                filename: input.filename.clone(),
                content_type: input.content_type.clone(),
                bytes,
            })
        })
        .collect()
}

fn storage_error(error: std::io::Error) -> PortalClientError {
    PortalClientError::Storage {
        message: error.to_string(),
    }
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}
fn suffix(bytes: &[u8; 4]) -> String {
    const CHARS: &[u8] = b"abcdefghijklmnopqrstuvwxyz0123456789";
    bytes
        .iter()
        .map(|byte| char::from(CHARS[usize::from(*byte) % CHARS.len()]))
        .collect()
}

pub fn portal_url_from_settings(journal_root: &Path) -> String {
    portal_url_from_settings_with_env(
        journal_root,
        std::env::var("SOLSTONE_SUPPORT_URL").ok().as_deref(),
    )
}

pub(crate) fn portal_url_from_settings_with_env(
    journal_root: &Path,
    env_url: Option<&str>,
) -> String {
    if let Some(url) = env_url.filter(|url| !url.is_empty()) {
        return url.trim_end_matches('/').to_owned();
    }
    // The reference intentionally fails open for every config read/parse error.
    if let Ok(text) = fs::read_to_string(journal_root.join("config/config.json"))
        && let Ok(value) = serde_json::from_str::<Value>(&text)
        && let Some(url) = value
            .pointer("/support/portal_url")
            .and_then(Value::as_str)
            .filter(|url| !url.is_empty())
    {
        return url.trim_end_matches('/').to_owned();
    }
    DEFAULT_PORTAL_URL.to_owned()
}

pub fn is_enabled(journal_root: &Path) -> bool {
    let Ok(text) = fs::read_to_string(journal_root.join("config/config.json")) else {
        return true;
    };
    let Ok(value) = serde_json::from_str::<Value>(&text) else {
        return true;
    };
    value
        .pointer("/support/enabled")
        .map(python_truthy)
        .unwrap_or(true)
}

fn python_truthy(value: &Value) -> bool {
    match value {
        Value::Null | Value::Bool(false) => false,
        Value::Number(number) => number.as_f64().is_none_or(|number| number != 0.0),
        Value::String(text) => !text.is_empty(),
        Value::Array(values) => !values.is_empty(),
        Value::Object(values) => !values.is_empty(),
        Value::Bool(true) => true,
    }
}

fn append_params(url: &str, params: Option<&[(String, String)]>) -> String {
    let Some(params) = params.filter(|params| !params.is_empty()) else {
        return url.to_owned();
    };
    let query = params
        .iter()
        .map(|(key, value)| format!("{}={}", percent_encode(key), percent_encode(value)))
        .collect::<Vec<_>>()
        .join("&");
    let (base, fragment) = match url.split_once('#') {
        Some((base, fragment)) => (base, Some(fragment)),
        None => (url, None),
    };
    let separator = if base.contains('?') { "&" } else { "?" };
    match fragment {
        Some(fragment) => format!("{base}{separator}{query}#{fragment}"),
        None => format!("{base}{separator}{query}"),
    }
}

fn percent_encode(value: &str) -> String {
    value
        .bytes()
        .flat_map(|byte| match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                vec![char::from(byte)]
            }
            _ => format!("%{byte:02X}").chars().collect(),
        })
        .collect()
}

#[path = "operations_client.rs"]
mod operations_client;
pub use operations_client::PortalOperationError;
