// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Test-only doubles and constructors. Available under `cfg(test)` and the
//! `test-support` feature; not a general HTTP mocking framework.

use std::collections::{BTreeMap, VecDeque};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use crate::client::{
    PortalClient, PortalResponse as CratePortalResponse, PortalRuntime, PortalTransport,
    ProductionRuntime, RequestBody,
};
use crate::errors::PortalClientError;

/// Dummy origin used by [`RoutePortal::client`].
pub const DUMMY_PORTAL_URL: &str = "https://portal.example";

/// Status and body a fake transport should return.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PortalResponse {
    pub status: u16,
    pub body: String,
}

impl From<PortalResponse> for CratePortalResponse {
    fn from(response: PortalResponse) -> Self {
        Self {
            status: response.status,
            body: response.body,
        }
    }
}

/// One recorded request observed by [`StubTransport`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RequestLog {
    pub method: String,
    pub path: String,
    pub headers: Vec<(String, String)>,
    pub body: Option<String>,
}

/// One multipart part captured before ureq encodes a `Form`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MultipartCapture {
    pub name: String,
    pub filename: String,
    pub content_type: Option<String>,
    pub bytes: Vec<u8>,
}

/// Sequential in-process transport used by portal `--lib` tests.
pub struct StubTransport {
    base: String,
    replies: VecDeque<CratePortalResponse>,
    log: Arc<Mutex<Vec<RequestLog>>>,
    pub multipart_bodies: Arc<Mutex<Vec<Vec<MultipartCapture>>>>,
}

impl StubTransport {
    pub fn new(
        base: impl Into<String>,
        replies: Vec<PortalResponse>,
    ) -> (Self, Arc<Mutex<Vec<RequestLog>>>) {
        let log = Arc::new(Mutex::new(Vec::new()));
        (
            Self {
                base: base.into(),
                replies: replies.into_iter().map(CratePortalResponse::from).collect(),
                log: log.clone(),
                multipart_bodies: Arc::new(Mutex::new(Vec::new())),
            },
            log,
        )
    }

    fn reply(
        &mut self,
        method: &str,
        url: &str,
        headers: &[(String, String)],
        body: Option<String>,
    ) -> Result<CratePortalResponse, PortalClientError> {
        let path = url.strip_prefix(&self.base).unwrap_or(url).to_owned();
        self.log.lock().expect("log lock").push(RequestLog {
            method: method.to_owned(),
            path,
            headers: headers.to_vec(),
            body,
        });
        self.replies
            .pop_front()
            .ok_or_else(|| PortalClientError::Transport {
                message: "fake has no response".to_owned(),
            })
    }
}

impl PortalTransport for StubTransport {
    fn request(
        &mut self,
        method: &str,
        url: &str,
        headers: &[(String, String)],
        body: RequestBody,
    ) -> Result<CratePortalResponse, PortalClientError> {
        match body {
            RequestBody::None => self.reply(method, url, headers, None),
            RequestBody::Json(body) => self.reply(method, url, headers, Some(body)),
            RequestBody::Multipart(files) => {
                self.multipart_bodies.lock().expect("body lock").push(
                    files
                        .iter()
                        .map(|part| MultipartCapture {
                            name: part.name.clone(),
                            filename: part.filename.clone(),
                            content_type: part.content_type.clone(),
                            bytes: part.bytes.clone(),
                        })
                        .collect(),
                );
                self.reply(method, url, headers, None)
            }
        }
    }
}

/// One recorded request observed by [`RoutePortal`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RouteRequest {
    pub method: String,
    pub path: String,
    pub query: Option<String>,
    pub idempotency_key: Option<String>,
    pub had_authorization: bool,
    pub had_dpop: bool,
    pub body: Option<Vec<u8>>,
    pub multipart: Option<Vec<MultipartCapture>>,
}

/// Fixed reply served by [`RoutePortal`] for one method/path pair.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RouteReply {
    pub status: u16,
    pub body: String,
    pub content_type: String,
}

type RouteReplyOverrides = BTreeMap<(String, String), VecDeque<RouteReply>>;

/// In-process route-table double for corpus replay. Not a socket.
pub struct RoutePortal {
    routes: Arc<BTreeMap<(String, String), RouteReply>>,
    overrides: Arc<Mutex<RouteReplyOverrides>>,
    log: Arc<Mutex<Vec<RouteRequest>>>,
    bodies: Arc<Mutex<Vec<Vec<u8>>>>,
}

impl Clone for RoutePortal {
    fn clone(&self) -> Self {
        self.share()
    }
}

impl RoutePortal {
    pub fn new(routes: BTreeMap<(String, String), RouteReply>) -> Self {
        Self {
            routes: Arc::new(routes),
            overrides: Arc::new(Mutex::new(BTreeMap::new())),
            log: Arc::new(Mutex::new(Vec::new())),
            bodies: Arc::new(Mutex::new(Vec::new())),
        }
    }

    pub fn share(&self) -> Self {
        Self {
            routes: self.routes.clone(),
            overrides: self.overrides.clone(),
            log: self.log.clone(),
            bodies: self.bodies.clone(),
        }
    }

    pub fn client(
        &self,
        storage_dir: impl Into<PathBuf>,
        handle: Option<String>,
        anonymous: bool,
    ) -> Result<PortalClient, PortalClientError> {
        client(
            DUMMY_PORTAL_URL,
            storage_dir,
            handle,
            anonymous,
            self.share(),
        )
    }

    pub fn log(&self) -> Vec<RouteRequest> {
        self.log.lock().expect("log lock").clone()
    }

    pub fn clear_log(&self) {
        self.log.lock().expect("log lock").clear();
    }

    pub fn bodies(&self) -> Vec<Vec<u8>> {
        self.bodies.lock().expect("body lock").clone()
    }

    pub fn clear_bodies(&self) {
        self.bodies.lock().expect("body lock").clear();
    }

    pub fn override_route(&self, method: &str, path: &str, replies: Vec<RouteReply>) {
        self.overrides
            .lock()
            .expect("override lock")
            .insert((method.to_owned(), path.to_owned()), replies.into());
    }
}

const TEST_KEYPAIR_PEM: &[u8] =
    include_bytes!("../../../fixtures/support_portal_golden_nonproduction/keypair.pem");

struct FixedKeypairRuntime(ProductionRuntime);

impl PortalRuntime for FixedKeypairRuntime {
    fn now(&mut self) -> i64 {
        self.0.now()
    }
    fn uuid(&mut self) -> String {
        self.0.uuid()
    }
    fn random_bytes(&mut self, bytes: &mut [u8]) -> Result<(), PortalClientError> {
        self.0.random_bytes(bytes)
    }
    fn keypair_pem(&mut self) -> Option<Vec<u8>> {
        Some(TEST_KEYPAIR_PEM.to_vec())
    }
}

/// Construct a client with production runtime and an injected [`RoutePortal`].
pub fn client(
    portal_url: impl Into<String>,
    storage_dir: impl Into<PathBuf>,
    handle: Option<String>,
    anonymous: bool,
    transport: RoutePortal,
) -> Result<PortalClient, PortalClientError> {
    PortalClient::new_with(
        portal_url,
        storage_dir,
        handle,
        anonymous,
        Box::new(transport),
        Box::new(FixedKeypairRuntime(ProductionRuntime)),
    )
}

/// Issue a raw authenticated GET and return the response without raising for
/// non-2xx status. Exposed for `tests/support_portal_transport.rs`, which
/// cannot see `PortalClient::authed_request` (`pub(crate)`) across the
/// integration-test crate boundary.
pub fn authed_get(
    client: &mut PortalClient,
    path: &str,
) -> Result<PortalResponse, PortalClientError> {
    let response = client.authed_request("GET", path, None, None, None, None)?;
    Ok(PortalResponse {
        status: response.status,
        body: response.body,
    })
}

impl PortalTransport for RoutePortal {
    fn request(
        &mut self,
        method: &str,
        url: &str,
        headers: &[(String, String)],
        body: RequestBody,
    ) -> Result<CratePortalResponse, PortalClientError> {
        let remainder = url.strip_prefix(DUMMY_PORTAL_URL).unwrap_or(url);
        let (path, query) = remainder
            .split_once('?')
            .map_or((remainder.to_owned(), None), |(path, query)| {
                (path.to_owned(), Some(query.to_owned()))
            });
        let (json_body, multipart) = match body {
            RequestBody::None => (None, None),
            RequestBody::Json(body) => (Some(body.into_bytes()), None),
            RequestBody::Multipart(files) => (
                None,
                Some(
                    files
                        .iter()
                        .map(|part| MultipartCapture {
                            name: part.name.clone(),
                            filename: part.filename.clone(),
                            content_type: part.content_type.clone(),
                            bytes: part.bytes.clone(),
                        })
                        .collect(),
                ),
            ),
        };
        self.bodies
            .lock()
            .expect("body lock")
            .push(json_body.clone().unwrap_or_default());
        self.log.lock().expect("log lock").push(RouteRequest {
            method: method.to_owned(),
            path: path.clone(),
            query,
            idempotency_key: header_value(headers, "idempotency-key"),
            had_authorization: header_present(headers, "authorization"),
            had_dpop: header_present(headers, "dpop"),
            body: json_body,
            multipart,
        });
        let key = (method.to_owned(), path);
        let reply = self
            .overrides
            .lock()
            .expect("override lock")
            .get_mut(&key)
            .and_then(VecDeque::pop_front)
            .or_else(|| self.routes.get(&key).cloned())
            .unwrap_or_else(|| RouteReply {
                status: 404,
                body: r#"{"error":"not_found"}"#.to_owned(),
                content_type: "application/json".to_owned(),
            });
        Ok(CratePortalResponse {
            status: reply.status,
            body: reply.body,
        })
    }
}

fn header_present(headers: &[(String, String)], name: &str) -> bool {
    header_value(headers, name).is_some()
}

fn header_value(headers: &[(String, String)], name: &str) -> Option<String> {
    headers.iter().find_map(|(header, value)| {
        header
            .eq_ignore_ascii_case(name)
            .then(|| value.to_owned())
            .filter(|value| !value.is_empty())
    })
}
