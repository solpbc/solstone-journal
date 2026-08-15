// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Durable identity state and the reference-compatible registration client.

use std::fs;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

use serde::Serialize;
use serde_json::Value;

use crate::dpop::create_dpop_proof;
use crate::errors::PortalClientError;
use crate::keypair::{Keypair, save_keypair};
use crate::token::{create_access_token, sign_tos};

#[allow(
    dead_code,
    reason = "W1c resolves the production portal URL through this helper."
)]
const DEFAULT_PORTAL_URL: &str = "https://support.solstone.app";

/// The complete response needed for TOS classification, diagnostics, and text fetches.
#[derive(Debug)]
pub(crate) struct PortalResponse {
    pub(crate) status: u16,
    pub(crate) body: String,
}

#[allow(
    dead_code,
    reason = "W1c dispatches mutations through this private seam."
)]
pub(crate) trait PortalTransport {
    fn get(
        &mut self,
        url: &str,
        headers: &[(String, String)],
    ) -> Result<PortalResponse, PortalClientError>;
    fn post_json(
        &mut self,
        url: &str,
        headers: &[(String, String)],
        body: &str,
    ) -> Result<PortalResponse, PortalClientError>;
    fn post_multipart(
        &mut self,
        url: &str,
        headers: &[(String, String)],
        files: &[MultipartPart],
    ) -> Result<PortalResponse, PortalClientError>;
}

pub(crate) trait PortalRuntime {
    fn now(&mut self) -> i64;
    fn uuid(&mut self) -> String;
    fn random_bytes(&mut self, bytes: &mut [u8]) -> Result<(), PortalClientError>;
}

struct ProductionRuntime;

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
            .build();
        Self {
            agent: ureq::Agent::new_with_config(config),
        }
    }

    fn response(
        response: ureq::http::Response<ureq::Body>,
    ) -> Result<PortalResponse, PortalClientError> {
        let status = response.status().as_u16();
        let body = response.into_body().read_to_string().map_err(|error| {
            PortalClientError::Transport {
                message: error.to_string(),
            }
        })?;
        Ok(PortalResponse { status, body })
    }
}

impl PortalTransport for UreqPortalTransport {
    fn get(
        &mut self,
        url: &str,
        headers: &[(String, String)],
    ) -> Result<PortalResponse, PortalClientError> {
        let mut request = self.agent.get(url);
        for (name, value) in headers {
            request = request.header(name, value);
        }
        Self::response(request.call().map_err(transport_error)?)
    }

    fn post_json(
        &mut self,
        url: &str,
        headers: &[(String, String)],
        body: &str,
    ) -> Result<PortalResponse, PortalClientError> {
        let mut request = self
            .agent
            .post(url)
            .header("Content-Type", "application/json");
        for (name, value) in headers {
            request = request.header(name, value);
        }
        Self::response(request.send(body).map_err(transport_error)?)
    }

    fn post_multipart(
        &mut self,
        url: &str,
        headers: &[(String, String)],
        files: &[MultipartPart],
    ) -> Result<PortalResponse, PortalClientError> {
        use ureq::unversioned::multipart::{Form, Part};
        let mut form = Form::new();
        for file in files {
            let mut part = Part::bytes(&file.bytes).file_name(&file.filename);
            if let Some(content_type) = &file.content_type {
                part = part.mime_str(content_type).map_err(transport_error)?;
            }
            form = form.part(&file.name, part);
        }
        let mut request = self.agent.post(url);
        for (name, value) in headers {
            request = request.header(name, value);
        }
        Self::response(request.send(form).map_err(transport_error)?)
    }
}

fn transport_error(error: impl std::fmt::Display) -> PortalClientError {
    PortalClientError::Transport {
        message: error.to_string(),
    }
}

#[allow(dead_code, reason = "W1c supplies multipart attachment streams.")]
pub(crate) trait ReadSeek: Read + Seek {}
impl<T: Read + Seek + ?Sized> ReadSeek for T {}

#[allow(dead_code, reason = "W1c supplies multipart attachment streams.")]
pub(crate) struct MultipartInput<'a> {
    pub(crate) name: String,
    pub(crate) filename: String,
    pub(crate) content_type: Option<String>,
    pub(crate) reader: &'a mut dyn ReadSeek,
}

#[derive(Clone)]
#[allow(dead_code, reason = "W1c supplies multipart attachment streams.")]
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
        if anonymous && handle.is_none() {
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
        if self.handle.is_none() {
            let hostname = nix::unistd::gethostname()
                .expect("read local hostname")
                .to_string_lossy()
                .to_lowercase();
            let hostname: String = hostname
                .replace('_', "-")
                .chars()
                .take(48)
                .filter(|character| {
                    character.is_ascii_alphanumeric() || matches!(character, '.' | '-')
                })
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
        match fs::read_to_string(self.token_path()) {
            Ok(text) => match serde_json::from_str::<Value>(&text) {
                Ok(Value::Object(value)) => {
                    self.access_token = value
                        .get("access_token")
                        .and_then(Value::as_str)
                        .map(str::to_owned);
                    self.handle = value
                        .get("handle")
                        .and_then(Value::as_str)
                        .map(str::to_owned)
                        .or(self.handle.take());
                }
                Ok(_) => {
                    return Err(PortalClientError::State {
                        message: "token.json is not an object".to_owned(),
                    });
                }
                Err(_) => {}
            },
            Err(_) => {}
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
        let text = serde_json::to_string(&TokenFile {
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
        let (keypair, pem) = Keypair::generate()?;
        if !self.anonymous {
            save_keypair(&self.keypair_path(), &pem)?;
        }
        self.keypair = Some(keypair);
        Ok(())
    }

    #[allow(dead_code, reason = "W1c derives its durable ledger principal here.")]
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
        reason = "W1c forwards the cached credential to its request adapter."
    )]
    pub(crate) fn access_token(&self) -> Option<&str> {
        self.access_token.as_deref()
    }

    pub fn fetch_tos(&mut self) -> Result<String, PortalClientError> {
        let url = format!("{}/tos", self.portal_url);
        let response = self
            .transport
            .get(&url, &[("Accept".to_owned(), "text/plain".to_owned())])?;
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
            let body = serde_json::to_string(&SignupBody {
                tos_signature: sign_tos(&keypair.signer, &tos)?,
                access_token: access.clone(),
                handle: self.handle().to_owned(),
            })
            .map_err(|error| PortalClientError::State {
                message: error.to_string(),
            })?;
            let url = format!("{}/api/signup", self.portal_url);
            let response = self
                .transport
                .post_json(&url, &[("DPoP".to_owned(), dpop)], &body)?;
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
            self.handle = data
                .get("handle")
                .and_then(Value::as_str)
                .map(str::to_owned)
                .or(self.handle.take());
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

    #[allow(
        dead_code,
        reason = "W1c dispatches authenticated reads and mutations here."
    )]
    pub(crate) fn authed_request(
        &mut self,
        method: &str,
        url: &str,
        json_body: Option<&str>,
        mut files: Option<&mut [MultipartInput<'_>]>,
    ) -> Result<PortalResponse, PortalClientError> {
        self.authed_request_inner(method, url, json_body, &mut files, true)
    }

    #[allow(
        dead_code,
        reason = "W1c dispatches authenticated reads and mutations here."
    )]
    fn authed_request_inner(
        &mut self,
        method: &str,
        url: &str,
        json_body: Option<&str>,
        files: &mut Option<&mut [MultipartInput<'_>]>,
        retry_on_tos: bool,
    ) -> Result<PortalResponse, PortalClientError> {
        let url = if url.starts_with('/') {
            format!("{}{}", self.portal_url, url)
        } else {
            url.to_owned()
        };
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
        let headers = vec![
            ("Authorization".to_owned(), format!("DPoP {token}")),
            ("DPoP".to_owned(), dpop),
        ];
        let response = if let Some(body) = json_body {
            self.transport.post_json(&url, &headers, body)?
        } else if let Some(inputs) = files.as_deref_mut() {
            let parts = rewind_files(inputs)?;
            self.transport.post_multipart(&url, &headers, &parts)?
        } else {
            self.transport.get(&url, &headers)?
        };
        if response.status == 401 && retry_on_tos {
            let error = serde_json::from_str::<Value>(&response.body)
                .ok()
                .and_then(|body| body.get("error").and_then(Value::as_str).map(str::to_owned));
            if error.as_deref() == Some("tos_changed") {
                self.register()?;
                return self.authed_request_inner(method, &url, json_body, files, false);
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

#[allow(
    dead_code,
    reason = "W1c retries multipart uploads through this rewind path."
)]
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

#[allow(
    dead_code,
    reason = "W1c reads portal configuration at its journal boundary."
)]
pub(crate) fn portal_url_from_settings(journal_root: &Path) -> String {
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
        && let Some(url) = value.pointer("/support/portal_url").and_then(Value::as_str)
    {
        return url.trim_end_matches('/').to_owned();
    }
    DEFAULT_PORTAL_URL.to_owned()
}

#[allow(dead_code, reason = "W1c checks this portal configuration gate.")]
pub(crate) fn is_enabled(journal_root: &Path) -> bool {
    let Ok(text) = fs::read_to_string(journal_root.join("config/config.json")) else {
        return true;
    };
    let Ok(value) = serde_json::from_str::<Value>(&text) else {
        return true;
    };
    value
        .pointer("/support/enabled")
        .and_then(Value::as_bool)
        .unwrap_or(true)
}
