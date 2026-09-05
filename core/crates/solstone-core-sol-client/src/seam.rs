// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::cell::RefCell;
use std::collections::{HashMap, HashSet, VecDeque};
use std::io::{Error, ErrorKind, Result as IoResult};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use crate::error::ClientError;
use crate::transport::{
    ApiRequest, HttpResponse, SseRequest, SseStream, UploadRequest, memory_sse_stream,
    ordered_query_pairs,
};

pub trait HttpTransport {
    fn request(&self, request: ApiRequest) -> Result<HttpResponse, ClientError>;
    fn upload(&self, request: UploadRequest) -> Result<HttpResponse, ClientError>;
    fn open_sse(&self, request: SseRequest) -> Result<SseStream, ClientError>;
}

pub trait Clock {
    fn now(&self) -> SystemTime;
    fn monotonic(&self) -> Duration;
    fn sleep(&self, duration: Duration);
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProcessOutput {
    pub status: i32,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
}

pub trait ProcessSpawner {
    fn run(&self, program: &str, args: &[String]) -> IoResult<ProcessOutput>;
}

pub trait BuildIdentityProvider {
    fn build_identity(&self, journal: &Path) -> Option<serde_json::Value>;
}

pub trait ClientItemIdProvider {
    fn client_item_id(&self) -> String;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NotificationSinkError {
    Unavailable,
}

pub trait NotificationSink {
    fn send_line(&self, line: &str) -> Result<(), NotificationSinkError>;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LinkJoinRelayErrorKind {
    HomeOffline,
    Unauthorized,
    Unpaid,
    UnknownInstance,
    PairWindowClosed,
    Overflow,
    Abnormal,
    UpgradeRejected,
    Stalled,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LinkJoinRelayControlEndpoint {
    EnrollDevice,
    TokenRefresh,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LinkJoinPairingErrorKind {
    Io,
    Tls,
    Crypto,
    Mux,
    Http,
    Json,
    PairLink,
    Pairing,
    PairResponseMissingHomeAttestation,
    Rejected {
        status: u16,
    },
    Relay(LinkJoinRelayErrorKind),
    RelayControlRejected {
        endpoint: LinkJoinRelayControlEndpoint,
        status: u16,
    },
    NoEndpoint,
    NotPaired,
    LocalOffset,
    RuntimeUnavailable,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LinkJoinPairingError {
    pub kind: LinkJoinPairingErrorKind,
}

impl LinkJoinPairingError {
    #[must_use]
    pub fn new(kind: LinkJoinPairingErrorKind) -> Self {
        Self { kind }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LinkJoinPairTarget {
    pub host: String,
    pub port: u16,
}

#[derive(Debug, Clone, PartialEq)]
pub struct LinkJoinDirectRequest {
    pub targets: Vec<LinkJoinPairTarget>,
    pub nonce_hex: String,
    pub ca_fp_prefix: Vec<u8>,
    pub device_label: String,
    pub additional_fields: serde_json::Map<String, serde_json::Value>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct LinkJoinRelayRequest {
    pub relay_origin: String,
    pub secret: Vec<u8>,
    pub ca_fp_spki: Vec<u8>,
    pub device_label: String,
    pub additional_fields: serde_json::Map<String, serde_json::Value>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct LinkJoinCredential {
    pub client_key_pem: String,
    pub client_cert_pem: String,
    pub ca_chain_pem: Vec<String>,
    pub ca_fingerprint: String,
    pub instance_id: String,
    pub home_label: String,
    pub home_attestation: Option<String>,
    pub local_endpoints: serde_json::Value,
    pub relay_device_token: Option<String>,
    pub relay_device_token_expires_at: Option<i64>,
}

pub trait LinkJoinPairingSeam: Send + Sync {
    fn pair_direct(
        &self,
        request: LinkJoinDirectRequest,
    ) -> Result<LinkJoinCredential, LinkJoinPairingError>;

    fn pair_relay(
        &self,
        request: LinkJoinRelayRequest,
    ) -> Result<LinkJoinCredential, LinkJoinPairingError>;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LinkServeEndpoint {
    pub host: String,
    pub port: u16,
}

#[derive(Debug, Clone, PartialEq)]
pub struct LinkServeBundle {
    pub private_key_pem: String,
    pub client_cert_pem: String,
    pub ca_chain_pem: Vec<String>,
    pub home_attestation: String,
    pub instance_id: String,
    pub home_label: String,
    pub endpoints: Vec<LinkServeEndpoint>,
    pub local_endpoints: serde_json::Value,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LinkServeCarrierPolicy {
    Direct,
    RelayPermitted,
    RelayOnly,
}

#[derive(Debug, Clone, PartialEq)]
pub struct LinkServeRequest {
    pub label: String,
    pub port: u16,
    pub policy: LinkServeCarrierPolicy,
    pub relay_origin: Option<String>,
    pub bundle: LinkServeBundle,
}

#[derive(Debug, Clone, PartialEq)]
pub struct LinkServeFailure {
    pub reason: String,
    pub detail: String,
    pub at: f64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct LinkServeStatusSnapshot {
    pub health: String,
    pub state: String,
    pub manager_alive: bool,
    pub connected_age_seconds: Option<f64>,
    pub last_connected_at: Option<f64>,
    pub last_failure: Option<LinkServeFailure>,
    pub next_retry_at: Option<f64>,
    pub reconnect_count: u64,
    pub active_requests: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LinkServeRelayErrorKind {
    HomeOffline,
    Unauthorized,
    Unpaid,
    UnknownInstance,
    PairWindowClosed,
    Overflow,
    Abnormal,
    UpgradeRejected,
    Stalled,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LinkServeRelayControlEndpoint {
    EnrollDevice,
    TokenRefresh,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LinkServeTransportErrorKind {
    Io,
    Tls,
    Crypto,
    Mux,
    Http,
    Json,
    PairLink,
    Pairing,
    Rejected {
        status: u16,
    },
    Relay(LinkServeRelayErrorKind),
    RelayControlRejected {
        endpoint: LinkServeRelayControlEndpoint,
        status: u16,
    },
    NoEndpoint,
    NotPaired,
    LocalOffset,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LinkServeErrorKind {
    InvalidBundle,
    Bind { port: u16, addr_in_use: bool },
    RuntimeUnavailable,
    BridgeCapability,
    Transport(LinkServeTransportErrorKind),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LinkServeError {
    pub kind: LinkServeErrorKind,
}

impl LinkServeError {
    #[must_use]
    pub fn new(kind: LinkServeErrorKind) -> Self {
        Self { kind }
    }
}

pub trait LinkServeSession: Send {
    fn bound_port(&self) -> u16;
    fn serve(
        self: Box<Self>,
        shutdown: &dyn crate::resident::ShutdownSignal,
    ) -> Result<(), LinkServeError>;
}

pub trait LinkServeRunner: Send + Sync {
    fn start(&self, request: LinkServeRequest)
    -> Result<Box<dyn LinkServeSession>, LinkServeError>;
}

pub trait FileProvider {
    fn read(&self, path: &Path) -> IoResult<Vec<u8>>;
    fn read_to_string(&self, path: &Path) -> std::io::Result<String>;
    fn exists(&self, path: &Path) -> bool;
    fn is_file(&self, path: &Path) -> bool;
    fn canonicalize(&self, path: &Path) -> std::io::Result<PathBuf>;
}

#[derive(Debug, Clone, PartialEq)]
pub enum ExpectedHttpCall {
    Request {
        expected: ApiRequest,
        result: Result<HttpResponse, ClientError>,
    },
    Upload {
        expected: UploadRequest,
        result: Result<HttpResponse, ClientError>,
    },
    Sse {
        expected: SseRequest,
        chunks: Vec<Vec<u8>>,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub enum RecordedHttpCall {
    Request {
        method: String,
        path: String,
        query: Vec<(String, String)>,
        json: Option<serde_json::Value>,
        headers: Vec<(String, String)>,
        timeout_policy: String,
    },
    Upload {
        path: String,
        files: Vec<RecordedMultipartFile>,
        data: Vec<(String, String)>,
        headers: Vec<(String, String)>,
        timeout_policy: String,
    },
    Sse {
        path: String,
        timeout_policy: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecordedMultipartFile {
    pub field_name: String,
    pub filename: String,
    pub content_type: Option<String>,
    pub length: usize,
}

#[derive(Debug, Default)]
pub struct ScriptedHttpTransport {
    calls: RefCell<VecDeque<ExpectedHttpCall>>,
    recorded: RefCell<Vec<RecordedHttpCall>>,
}

#[derive(Debug, Default)]
pub struct RecordingNotificationSink {
    recorded: RefCell<Vec<String>>,
    fail: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub enum ExpectedLinkJoinPairingCall {
    Direct {
        expected: LinkJoinDirectRequest,
        result: Result<LinkJoinCredential, LinkJoinPairingError>,
    },
    Relay {
        expected: LinkJoinRelayRequest,
        result: Result<LinkJoinCredential, LinkJoinPairingError>,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub enum RecordedLinkJoinPairingCall {
    Direct(LinkJoinDirectRequest),
    Relay(LinkJoinRelayRequest),
}

#[derive(Debug, Clone, PartialEq)]
pub struct ExpectedLinkServeSession {
    pub bound_port: u16,
    pub serve_result: Result<(), LinkServeError>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ExpectedLinkServeCall {
    pub expected: LinkServeRequest,
    pub result: Result<ExpectedLinkServeSession, LinkServeError>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct RecordedLinkServeCall {
    pub request: LinkServeRequest,
}

#[derive(Debug, Default)]
pub struct ScriptedLinkJoinPairingSeam {
    calls: Mutex<VecDeque<ExpectedLinkJoinPairingCall>>,
    recorded: Mutex<Vec<RecordedLinkJoinPairingCall>>,
}

#[derive(Debug, Default)]
pub struct ScriptedLinkServeRunner {
    calls: Mutex<VecDeque<ExpectedLinkServeCall>>,
    recorded: Mutex<Vec<RecordedLinkServeCall>>,
}

#[derive(Debug)]
struct ScriptedLinkServeSession {
    bound_port: u16,
    serve_result: Result<(), LinkServeError>,
}

impl ScriptedLinkJoinPairingSeam {
    #[must_use]
    pub fn new(calls: Vec<ExpectedLinkJoinPairingCall>) -> Self {
        Self {
            calls: Mutex::new(calls.into()),
            recorded: Mutex::new(Vec::new()),
        }
    }

    pub fn assert_done(&self) {
        assert!(
            self.calls.lock().expect("scripted calls lock").is_empty(),
            "scripted link pairing calls were not exhausted"
        );
    }

    #[must_use]
    pub fn recorded(&self) -> Vec<RecordedLinkJoinPairingCall> {
        self.recorded
            .lock()
            .expect("recorded link pairing lock")
            .clone()
    }
}

impl ScriptedLinkServeRunner {
    #[must_use]
    pub fn new(calls: Vec<ExpectedLinkServeCall>) -> Self {
        Self {
            calls: Mutex::new(calls.into()),
            recorded: Mutex::new(Vec::new()),
        }
    }

    pub fn assert_done(&self) {
        assert!(
            self.calls.lock().expect("scripted calls lock").is_empty(),
            "scripted link serve calls were not exhausted"
        );
    }

    #[must_use]
    pub fn recorded(&self) -> Vec<RecordedLinkServeCall> {
        self.recorded.lock().expect("link serve calls lock").clone()
    }
}

impl LinkServeRunner for ScriptedLinkServeRunner {
    fn start(
        &self,
        request: LinkServeRequest,
    ) -> Result<Box<dyn LinkServeSession>, LinkServeError> {
        self.recorded
            .lock()
            .expect("link serve calls lock")
            .push(RecordedLinkServeCall {
                request: request.clone(),
            });
        match self.calls.lock().expect("scripted calls lock").pop_front() {
            Some(ExpectedLinkServeCall { expected, result }) => {
                assert_eq!(request, expected);
                result.map(|session| {
                    Box::new(ScriptedLinkServeSession {
                        bound_port: session.bound_port,
                        serve_result: session.serve_result,
                    }) as Box<dyn LinkServeSession>
                })
            }
            other => panic!("expected link serve call, got {other:?}"),
        }
    }
}

impl LinkServeSession for ScriptedLinkServeSession {
    fn bound_port(&self) -> u16 {
        self.bound_port
    }

    fn serve(
        self: Box<Self>,
        _shutdown: &dyn crate::resident::ShutdownSignal,
    ) -> Result<(), LinkServeError> {
        self.serve_result
    }
}

impl RecordingNotificationSink {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    #[must_use]
    pub fn failing() -> Self {
        Self {
            recorded: RefCell::new(Vec::new()),
            fail: true,
        }
    }

    #[must_use]
    pub fn recorded(&self) -> Vec<String> {
        self.recorded.borrow().clone()
    }
}

impl NotificationSink for RecordingNotificationSink {
    fn send_line(&self, line: &str) -> Result<(), NotificationSinkError> {
        self.recorded.borrow_mut().push(line.to_string());
        if self.fail {
            return Err(NotificationSinkError::Unavailable);
        }
        Ok(())
    }
}

impl LinkJoinPairingSeam for ScriptedLinkJoinPairingSeam {
    fn pair_direct(
        &self,
        request: LinkJoinDirectRequest,
    ) -> Result<LinkJoinCredential, LinkJoinPairingError> {
        self.recorded
            .lock()
            .expect("recorded link pairing lock")
            .push(RecordedLinkJoinPairingCall::Direct(request.clone()));
        match self.calls.lock().expect("scripted calls lock").pop_front() {
            Some(ExpectedLinkJoinPairingCall::Direct { expected, result }) => {
                assert_eq!(request, expected);
                result
            }
            other => panic!("expected direct link pairing call, got {other:?}"),
        }
    }

    fn pair_relay(
        &self,
        request: LinkJoinRelayRequest,
    ) -> Result<LinkJoinCredential, LinkJoinPairingError> {
        self.recorded
            .lock()
            .expect("recorded link pairing lock")
            .push(RecordedLinkJoinPairingCall::Relay(request.clone()));
        match self.calls.lock().expect("scripted calls lock").pop_front() {
            Some(ExpectedLinkJoinPairingCall::Relay { expected, result }) => {
                assert_eq!(request, expected);
                result
            }
            other => panic!("expected relay link pairing call, got {other:?}"),
        }
    }
}

impl ScriptedHttpTransport {
    #[must_use]
    pub fn new(calls: Vec<ExpectedHttpCall>) -> Self {
        Self {
            calls: RefCell::new(calls.into()),
            recorded: RefCell::new(Vec::new()),
        }
    }

    pub fn assert_done(&self) {
        assert!(
            self.calls.borrow().is_empty(),
            "scripted HTTP calls were not exhausted"
        );
    }

    #[must_use]
    pub fn recorded(&self) -> Vec<RecordedHttpCall> {
        self.recorded.borrow().clone()
    }
}

impl HttpTransport for ScriptedHttpTransport {
    fn request(&self, request: ApiRequest) -> Result<HttpResponse, ClientError> {
        self.recorded.borrow_mut().push(RecordedHttpCall::Request {
            method: request.method.as_str().to_string(),
            path: request.path.clone(),
            query: ordered_query_pairs(&request.params),
            json: request.json.clone(),
            headers: request.headers.clone(),
            timeout_policy: request.policy.label().to_string(),
        });
        match self.calls.borrow_mut().pop_front() {
            Some(ExpectedHttpCall::Request { expected, result }) => {
                assert_eq!(request, expected);
                result
            }
            other => panic!("expected HTTP request call, got {other:?}"),
        }
    }

    fn upload(&self, request: UploadRequest) -> Result<HttpResponse, ClientError> {
        self.recorded.borrow_mut().push(RecordedHttpCall::Upload {
            path: request.path.clone(),
            files: request
                .files
                .iter()
                .map(|file| RecordedMultipartFile {
                    field_name: file.field_name.clone(),
                    filename: file.filename.clone(),
                    content_type: file.content_type.clone(),
                    length: file.body.len(),
                })
                .collect(),
            data: request
                .data
                .iter()
                .map(|field| (field.name.clone(), field.value.clone()))
                .collect(),
            headers: request.headers.clone(),
            timeout_policy: request.policy.label().to_string(),
        });
        match self.calls.borrow_mut().pop_front() {
            Some(ExpectedHttpCall::Upload { expected, result }) => {
                assert_eq!(request, expected);
                result
            }
            other => panic!("expected HTTP upload call, got {other:?}"),
        }
    }

    fn open_sse(&self, request: SseRequest) -> Result<SseStream, ClientError> {
        self.recorded.borrow_mut().push(RecordedHttpCall::Sse {
            path: request.path.clone(),
            timeout_policy: request.policy.label().to_string(),
        });
        match self.calls.borrow_mut().pop_front() {
            Some(ExpectedHttpCall::Sse { expected, chunks }) => {
                assert_eq!(request, expected);
                Ok(memory_sse_stream(chunks, request.policy))
            }
            other => panic!("expected HTTP SSE call, got {other:?}"),
        }
    }
}

#[derive(Debug)]
pub struct FakeClock {
    wall: RefCell<SystemTime>,
    monotonic: RefCell<Duration>,
}

impl FakeClock {
    #[must_use]
    pub fn new(wall: SystemTime) -> Self {
        Self {
            wall: RefCell::new(wall),
            monotonic: RefCell::new(Duration::ZERO),
        }
    }

    #[must_use]
    pub fn at_unix(seconds: u64) -> Self {
        Self::new(UNIX_EPOCH + Duration::from_secs(seconds))
    }

    pub fn advance(&self, duration: Duration) {
        *self.wall.borrow_mut() += duration;
        *self.monotonic.borrow_mut() += duration;
    }
}

impl Clock for FakeClock {
    fn now(&self) -> SystemTime {
        *self.wall.borrow()
    }

    fn monotonic(&self) -> Duration {
        *self.monotonic.borrow()
    }

    fn sleep(&self, duration: Duration) {
        self.advance(duration);
    }
}

#[derive(Debug, Default)]
pub struct FailingProcessSpawner;

impl ProcessSpawner for FailingProcessSpawner {
    fn run(&self, program: &str, args: &[String]) -> IoResult<ProcessOutput> {
        Err(Error::other(format!(
            "process spawning is disabled in native client tests: {program} {args:?}"
        )))
    }
}

#[derive(Debug, Clone, Default)]
pub struct FakeBuildIdentityProvider {
    value: Option<serde_json::Value>,
}

impl FakeBuildIdentityProvider {
    #[must_use]
    pub fn new(value: Option<serde_json::Value>) -> Self {
        Self { value }
    }
}

impl BuildIdentityProvider for FakeBuildIdentityProvider {
    fn build_identity(&self, _journal: &Path) -> Option<serde_json::Value> {
        self.value.clone()
    }
}

#[derive(Debug, Clone)]
pub struct FakeClientItemIdProvider {
    value: String,
}

impl FakeClientItemIdProvider {
    #[must_use]
    pub fn new(value: impl Into<String>) -> Self {
        Self {
            value: value.into(),
        }
    }
}

impl ClientItemIdProvider for FakeClientItemIdProvider {
    fn client_item_id(&self) -> String {
        self.value.clone()
    }
}

#[derive(Debug, Clone, Default)]
pub struct FixtureFileProvider {
    files: HashMap<PathBuf, Vec<u8>>,
    unreadable: HashSet<PathBuf>,
}

impl FixtureFileProvider {
    #[must_use]
    pub fn new(files: HashMap<PathBuf, Vec<u8>>) -> Self {
        Self {
            files,
            unreadable: HashSet::new(),
        }
    }

    pub fn insert(&mut self, path: impl Into<PathBuf>, body: impl Into<Vec<u8>>) {
        self.files.insert(path.into(), body.into());
    }

    pub fn mark_unreadable(&mut self, path: impl Into<PathBuf>) {
        self.unreadable.insert(path.into());
    }
}

impl FileProvider for FixtureFileProvider {
    fn read(&self, path: &Path) -> IoResult<Vec<u8>> {
        if self.unreadable.contains(path) {
            return Err(Error::new(
                ErrorKind::PermissionDenied,
                path.display().to_string(),
            ));
        }
        self.files
            .get(path)
            .cloned()
            .ok_or_else(|| Error::new(ErrorKind::NotFound, path.display().to_string()))
    }

    fn read_to_string(&self, path: &Path) -> IoResult<String> {
        String::from_utf8(self.read(path)?)
            .map_err(|error| Error::new(ErrorKind::InvalidData, error))
    }

    fn exists(&self, path: &Path) -> bool {
        self.files.contains_key(path)
    }

    fn is_file(&self, path: &Path) -> bool {
        self.exists(path)
    }

    fn canonicalize(&self, path: &Path) -> IoResult<PathBuf> {
        if self.exists(path) {
            Ok(path.to_path_buf())
        } else {
            Err(Error::new(ErrorKind::NotFound, path.display().to_string()))
        }
    }
}
