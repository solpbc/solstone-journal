// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::collections::BTreeMap;
use std::fmt;
use std::path::Path;
use std::time::{Duration, Instant};

use serde_json::{Value, json};
use solstone_core_backup::archive_admission::ArchiveCancellation;
use solstone_core_backup::{
    Destination, HostedBinding, assemble_backend_env, get_backup_config, get_destination, get_keys,
    load_hosted_binding,
};

use crate::runner::{RunnerError, is_explicit_program_path};

pub const BROKER_TIMEOUT_SECONDS: u64 = 30;

#[derive(Clone, PartialEq, Eq)]
pub struct HttpRequest {
    pub method: String,
    pub url: String,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
    pub timeout: Duration,
}

impl fmt::Debug for HttpRequest {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("HttpRequest")
            .field("method", &self.method)
            .field("url", &self.url)
            .field("headers", &"<redacted>")
            .field("body", &"<redacted>")
            .field("timeout", &self.timeout)
            .finish()
    }
}
#[derive(Clone, PartialEq, Eq)]
pub struct HttpResponse {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}
impl fmt::Debug for HttpResponse {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("HttpResponse")
            .field("status", &self.status)
            .field("headers", &"<redacted>")
            .field("body", &"<redacted>")
            .finish()
    }
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HttpError {
    Timeout,
    Unreachable,
    Other,
}
pub trait HttpTransport {
    fn execute(&self, request: &HttpRequest) -> Result<HttpResponse, HttpError>;

    fn execute_cancellable(
        &self,
        _request: &HttpRequest,
        _cancellation: &ArchiveCancellation,
    ) -> Result<HttpResponse, HttpError> {
        Err(HttpError::Other)
    }
}
#[derive(Debug, Default)]
pub struct UreqHttpTransport;
impl HttpTransport for UreqHttpTransport {
    fn execute_cancellable(
        &self,
        request: &HttpRequest,
        cancellation: &ArchiveCancellation,
    ) -> Result<HttpResponse, HttpError> {
        #[cfg(any(target_os = "linux", target_os = "macos"))]
        {
            crate::controlled_http::execute(request, cancellation)
        }
        // Windows has no owned resolver yet: the request runs to its own timeout,
        // and cancellation is honoured on either side of it.
        #[cfg(not(any(target_os = "linux", target_os = "macos")))]
        {
            if cancellation.is_cancelled() {
                return Err(HttpError::Other);
            }
            let result = self.execute(request);
            if cancellation.is_cancelled() {
                Err(HttpError::Other)
            } else {
                result
            }
        }
    }

    fn execute(&self, request: &HttpRequest) -> Result<HttpResponse, HttpError> {
        let deadline = Instant::now()
            .checked_add(request.timeout)
            .ok_or(HttpError::Other)?;
        let remaining = || {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                Err(HttpError::Timeout)
            } else {
                Ok(remaining)
            }
        };
        remaining()?;
        let response = match request.method.as_str() {
            "POST" | "PUT" => {
                let mut builder = if request.method == "POST" {
                    ureq::post(&request.url)
                } else {
                    ureq::put(&request.url)
                };
                for (name, value) in &request.headers {
                    builder = builder.header(name, value);
                }
                builder
                    .config()
                    .timeout_global(Some(remaining()?))
                    .http_status_as_error(false)
                    .build()
                    .send(&request.body)
            }
            _ => {
                let mut builder = ureq::get(&request.url);
                for (name, value) in &request.headers {
                    builder = builder.header(name, value);
                }
                builder
                    .config()
                    .timeout_global(Some(remaining()?))
                    .http_status_as_error(false)
                    .build()
                    .call()
            }
        };
        remaining()?;
        let response = response.map_err(|error| {
            if request_timed_out(&error) {
                HttpError::Timeout
            } else {
                HttpError::Unreachable
            }
        })?;
        let status = response.status().as_u16();
        let headers = response
            .headers()
            .iter()
            .filter_map(|(name, value)| {
                value
                    .to_str()
                    .ok()
                    .map(|value| (name.to_string(), value.to_owned()))
            })
            .collect();
        let mut body = Vec::new();
        use std::io::Read;
        let mut reader = response.into_body().into_reader();
        let mut buffer = [0; 1024];
        loop {
            remaining()?;
            let read = reader.read(&mut buffer);
            remaining()?;
            match read {
                Ok(0) => break,
                Ok(length) => body.extend_from_slice(&buffer[..length]),
                Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(error) => return Err(body_read_error(&error)),
            }
        }
        remaining()?;
        Ok(HttpResponse {
            status,
            headers,
            body,
        })
    }
}

fn request_timed_out(error: &ureq::Error) -> bool {
    match error {
        ureq::Error::Timeout(_) => true,
        ureq::Error::Io(error) => io_timed_out(error),
        _ => false,
    }
}

fn io_timed_out(error: &std::io::Error) -> bool {
    matches!(
        error.kind(),
        std::io::ErrorKind::TimedOut | std::io::ErrorKind::WouldBlock
    )
}

fn body_read_error(error: &std::io::Error) -> HttpError {
    // ureq preserves its global deadline as an inner Error::Timeout while
    // into_io labels the outer error Other. It can expire just before our
    // entry clock reaches its deadline; elapsed time is not its error type.
    if io_timed_out(error)
        || error
            .get_ref()
            .and_then(|inner| inner.downcast_ref::<ureq::Error>())
            .is_some_and(request_timed_out)
    {
        HttpError::Timeout
    } else {
        HttpError::Other
    }
}

#[derive(Clone, PartialEq, Eq)]
pub struct HostedCredentials {
    pub access_key_id: String,
    pub secret_access_key: String,
    pub session_token: String,
    pub endpoint: String,
    pub expires_at: String,
}
impl fmt::Debug for HostedCredentials {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("HostedCredentials")
            .field("access_key_id", &"<redacted>")
            .field("secret_access_key", &"<redacted>")
            .field("session_token", &"<redacted>")
            .field("endpoint", &self.endpoint)
            .finish()
    }
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HostedCredsUnavailable {
    pub reason_code: &'static str,
}
impl fmt::Display for HostedCredsUnavailable {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.reason_code)
    }
}
impl std::error::Error for HostedCredsUnavailable {}

pub fn fetch_hosted_credentials(
    transport: &dyn HttpTransport,
    binding: &HostedBinding,
    scope: &str,
    version: &str,
) -> Result<HostedCredentials, HostedCredsUnavailable> {
    let request = hosted_credentials_request(binding, scope, version);
    let response = transport
        .execute(&request)
        .map_err(|_| HostedCredsUnavailable {
            reason_code: "broker_unreachable",
        })?;
    decode_hosted_credentials(&response)
}

pub(crate) fn hosted_credentials_request(
    binding: &HostedBinding,
    scope: &str,
    version: &str,
) -> HttpRequest {
    HttpRequest {
        method: "POST".into(),
        url: format!(
            "{}/backup/credentials",
            binding.broker_endpoint.trim_end_matches('/')
        ),
        headers: vec![
            (
                "Authorization".into(),
                format!("Bearer {}", binding.broker_token),
            ),
            ("Content-Type".into(), "application/json".into()),
            ("User-Agent".into(), format!("solstone-backup/{version}")),
            ("Connection".into(), "close".into()),
        ],
        body: serde_json::to_vec(&json!({"scope":scope})).expect("serializable"),
        timeout: Duration::from_secs(BROKER_TIMEOUT_SECONDS),
    }
}

pub(crate) fn decode_hosted_credentials(
    response: &HttpResponse,
) -> Result<HostedCredentials, HostedCredsUnavailable> {
    let payload = serde_json::from_slice::<Value>(&response.body).ok();
    if response.status == 402 || needs_subscription(payload.as_ref()) {
        return Err(HostedCredsUnavailable {
            reason_code: "hosted_entitlement_inactive",
        });
    }
    if response.status == 401 {
        return Err(HostedCredsUnavailable {
            reason_code: if is_binding_superseded(payload.as_ref()) {
                "binding_superseded"
            } else {
                "binding_invalid"
            },
        });
    }
    if !(200..300).contains(&response.status) {
        return Err(HostedCredsUnavailable {
            reason_code: "broker_error",
        });
    }
    let Some(Value::Object(payload)) = payload else {
        return Err(HostedCredsUnavailable {
            reason_code: "broker_error",
        });
    };
    let field = |key: &str| {
        payload
            .get(key)
            .and_then(Value::as_str)
            .filter(|value| !value.trim().is_empty())
            .map(str::to_owned)
            .ok_or(HostedCredsUnavailable {
                reason_code: "broker_error",
            })
    };
    Ok(HostedCredentials {
        access_key_id: field("access_key_id")?,
        secret_access_key: field("secret_access_key")?,
        session_token: field("session_token")?,
        endpoint: field("endpoint")?,
        expires_at: field("expires_at")?,
    })
}
fn needs_subscription(payload: Option<&Value>) -> bool {
    let Some(Value::Object(payload)) = payload else {
        return false;
    };
    payload.get("needs_subscription") == Some(&Value::Bool(true))
        || ["error", "reason", "code", "status"]
            .iter()
            .any(|key| payload.get(*key) == Some(&Value::String("needs_subscription".into())))
}
fn is_binding_superseded(payload: Option<&Value>) -> bool {
    let Some(Value::Object(payload)) = payload else {
        return false;
    };
    payload.get("error") == Some(&Value::String("binding_superseded".into()))
}
pub fn operated_repository(binding: &HostedBinding, credentials: &HostedCredentials) -> String {
    format!(
        "s3:{}/{}/{}",
        credentials.endpoint.trim_end_matches('/'),
        binding.bucket,
        binding.prefix
    )
}
pub fn operated_destination(
    binding: &HostedBinding,
    credentials: &HostedCredentials,
) -> Destination {
    #[cfg(not(target_os = "ios"))]
    let fields = json!({"access_key_id":credentials.access_key_id,"secret_access_key":credentials.secret_access_key,"session_token":credentials.session_token});
    // iOS reaches operated storage through its linked transport, which signs
    // for R2's own region and keeps every non-lock write create-only, the
    // counterpart of desktop's append-only rclone transport.
    #[cfg(target_os = "ios")]
    let fields = json!({"access_key_id":credentials.access_key_id,"secret_access_key":credentials.secret_access_key,"session_token":credentials.session_token,"region":"auto","append_only":"1"});
    Destination {
        repository: operated_repository(binding, credentials),
        backend: "s3".into(),
        credentials: serde_json::from_value(fields).expect("object"),
    }
}
#[derive(Clone)]
pub struct HostedResticSession {
    pub destination: Destination,
    pub backend_env: BTreeMap<String, String>,
    pub global_options: Vec<String>,
}
impl fmt::Debug for HostedResticSession {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("HostedResticSession")
            .field("destination", &self.destination)
            .field("backend_env", &"<redacted>")
            .field("global_options", &self.global_options)
            .finish()
    }
}
pub fn hosted_session(
    binding: &HostedBinding,
    credentials: &HostedCredentials,
) -> Result<HostedResticSession, solstone_core_backup::BackupError> {
    let destination = operated_destination(binding, credentials);
    let backend_env = assemble_backend_env(&destination)?
        .into_iter()
        .filter_map(|(key, value)| value.as_str().map(|value| (key, value.to_owned())))
        .collect();
    Ok(HostedResticSession {
        destination,
        backend_env,
        global_options: vec![],
    })
}
fn quote_rclone_program(rclone: &Path) -> Result<String, RunnerError> {
    let path_str = rclone.to_str().ok_or(RunnerError::BareProgram)?;
    if path_str.contains('"') {
        return Err(RunnerError::BareProgram);
    }
    Ok(format!("rclone.program=\"{path_str}\""))
}

fn platform_rclone_no_config() -> &'static str {
    if cfg!(windows) { "NUL" } else { "/dev/null" }
}

pub fn hosted_append_only_session(
    binding: &HostedBinding,
    credentials: &HostedCredentials,
    rclone: &Path,
) -> Result<HostedResticSession, RunnerError> {
    if !is_explicit_program_path(rclone) {
        return Err(RunnerError::BareProgram);
    }
    let program_opt = quote_rclone_program(rclone)?;

    let args_opt = format!(
        "rclone.args=serve restic --stdio --append-only --config {}",
        platform_rclone_no_config()
    );

    Ok(HostedResticSession {
        destination: Destination {
            repository: format!("rclone:spb:{}/{}", binding.bucket, binding.prefix),
            backend: "rclone".into(),
            credentials: Default::default(),
        },
        backend_env: BTreeMap::from([
            ("RCLONE_CONFIG_SPB_TYPE".into(), "s3".into()),
            ("RCLONE_CONFIG_SPB_PROVIDER".into(), "Cloudflare".into()),
            ("RCLONE_CONFIG_SPB_ENV_AUTH".into(), "false".into()),
            (
                "RCLONE_CONFIG_SPB_ACCESS_KEY_ID".into(),
                credentials.access_key_id.clone(),
            ),
            (
                "RCLONE_CONFIG_SPB_SECRET_ACCESS_KEY".into(),
                credentials.secret_access_key.clone(),
            ),
            (
                "RCLONE_CONFIG_SPB_SESSION_TOKEN".into(),
                credentials.session_token.clone(),
            ),
            (
                "RCLONE_CONFIG_SPB_ENDPOINT".into(),
                credentials.endpoint.clone(),
            ),
            ("RCLONE_CONFIG_SPB_REGION".into(), "auto".into()),
            ("RCLONE_CONFIG_SPB_NO_CHECK_BUCKET".into(), "true".into()),
        ]),
        global_options: vec!["-o".into(), program_opt, "-o".into(), args_opt],
    })
}

pub enum RuntimeResolution {
    Skip,
    Byo {
        destination: Destination,
        keys: solstone_core_backup::BackupKeys,
    },
    Operated {
        binding: HostedBinding,
        credentials: HostedCredentials,
    },
}
pub fn resolve_runtime(
    transport: &dyn HttpTransport,
    journal: &Path,
    scope: &str,
    version: &str,
) -> Result<RuntimeResolution, HostedCredsUnavailable> {
    let config = get_backup_config(journal).map_err(|_| HostedCredsUnavailable {
        reason_code: "broker_error",
    })?;
    if config.get("enabled") != Some(&Value::Bool(true)) {
        return Ok(RuntimeResolution::Skip);
    }
    let Some(keys) = get_keys(journal).map_err(|_| HostedCredsUnavailable {
        reason_code: "broker_error",
    })?
    else {
        return Ok(RuntimeResolution::Skip);
    };
    if config.get("mode") == Some(&Value::String("operated".into())) {
        let Some(binding) = load_hosted_binding(journal) else {
            return Ok(RuntimeResolution::Skip);
        };
        let credentials = fetch_hosted_credentials(transport, &binding, scope, version)?;
        return Ok(RuntimeResolution::Operated {
            binding,
            credentials,
        });
    }
    let Some(destination) = get_destination(journal).map_err(|_| HostedCredsUnavailable {
        reason_code: "broker_error",
    })?
    else {
        return Ok(RuntimeResolution::Skip);
    };
    Ok(RuntimeResolution::Byo { destination, keys })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn typed_timeouts_survive_the_body_io_wrapper() {
        use std::io::{Error, ErrorKind};
        let timeout = ureq::Error::Timeout(ureq::Timeout::Global).into_io();
        assert_eq!(timeout.kind(), ErrorKind::Other);
        assert_eq!(body_read_error(&timeout), HttpError::Timeout);
        for kind in [ErrorKind::TimedOut, ErrorKind::WouldBlock] {
            assert_eq!(body_read_error(&Error::from(kind)), HttpError::Timeout);
            assert!(request_timed_out(&ureq::Error::Io(Error::from(kind))));
        }
        for kind in [
            ErrorKind::UnexpectedEof,
            ErrorKind::ConnectionReset,
            ErrorKind::Other,
        ] {
            let error = Error::new(kind, "not a timeout, despite this diagnostic");
            assert_eq!(body_read_error(&error), HttpError::Other);
            assert!(!request_timed_out(&ureq::Error::Io(error)));
        }
    }
    use std::cell::RefCell;
    struct Fixture(Result<HttpResponse, HttpError>);
    impl HttpTransport for Fixture {
        fn execute(&self, _: &HttpRequest) -> Result<HttpResponse, HttpError> {
            self.0.clone()
        }
    }
    #[test]
    fn cancellable_default_refuses_without_ordinary_execution() {
        struct CountingTransport(std::cell::Cell<usize>);
        impl HttpTransport for CountingTransport {
            fn execute(&self, _: &HttpRequest) -> Result<HttpResponse, HttpError> {
                self.0.set(self.0.get() + 1);
                Err(HttpError::Unreachable)
            }
        }
        let transport = CountingTransport(std::cell::Cell::new(0));
        let request = HttpRequest {
            method: "POST".into(),
            url: "http://example.invalid".into(),
            headers: vec![],
            body: vec![],
            timeout: Duration::from_secs(1),
        };
        let cancellation = ArchiveCancellation::new();
        let dynamic: &dyn HttpTransport = &transport;
        assert_eq!(
            dynamic.execute_cancellable(&request, &cancellation),
            Err(HttpError::Other)
        );
        cancellation.clone().cancel();
        assert_eq!(
            dynamic.execute_cancellable(&request, &cancellation),
            Err(HttpError::Other)
        );
        assert_eq!(transport.0.get(), 0);
    }
    fn binding() -> HostedBinding {
        HostedBinding {
            broker_endpoint: "https://broker".into(),
            account_id: "account".into(),
            instance_id: "instance".into(),
            bucket: "bucket".into(),
            prefix: "prefix".into(),
            broker_token: "TOKEN".into(),
        }
    }
    fn credentials() -> HostedCredentials {
        HostedCredentials {
            access_key_id: "ACCESS".into(),
            secret_access_key: "SECRET".into(),
            session_token: "SESSION".into(),
            endpoint: "https://s3.example".into(),
            expires_at: "tomorrow".into(),
        }
    }
    #[test]
    fn append_only_session_refuses_bare_rclone_program() {
        assert!(matches!(
            hosted_append_only_session(&binding(), &credentials(), Path::new("rclone")),
            Err(RunnerError::BareProgram)
        ));
        let session = hosted_append_only_session(
            &binding(),
            &credentials(),
            Path::new("/fixture/bin/rclone"),
        )
        .unwrap();
        #[cfg(not(windows))]
        {
            assert!(
                session
                    .global_options
                    .contains(&"rclone.program=\"/fixture/bin/rclone\"".into())
            );
            assert!(session.global_options.contains(
                &"rclone.args=serve restic --stdio --append-only --config /dev/null".into()
            ));
        }
        #[cfg(windows)]
        {
            assert!(
                session
                    .global_options
                    .contains(&"rclone.program=\"/fixture/bin/rclone\"".into())
            );
            assert!(
                session.global_options.contains(
                    &"rclone.args=serve restic --stdio --append-only --config NUL".into()
                )
            );
        }
        assert!(
            !session
                .global_options
                .iter()
                .any(|option| option == "rclone.program=rclone")
        );
    }
    #[test]
    fn debug_redacts_credentials() {
        let credentials = HostedCredentials {
            access_key_id: "ACCESS".into(),
            secret_access_key: "SECRET".into(),
            session_token: "TOKEN".into(),
            endpoint: "endpoint".into(),
            expires_at: "date".into(),
        };
        let rendered = format!("{credentials:?}");
        for secret in ["ACCESS", "SECRET", "TOKEN"] {
            assert!(!rendered.contains(secret));
        }
    }

    #[test]
    fn debug_redacts_http_and_hosted_session_secrets() {
        let request = HttpRequest {
            method: "POST".into(),
            url: "https://broker".into(),
            headers: vec![("Authorization".into(), "Bearer REQUEST_TOKEN".into())],
            body: b"REQUEST_BODY_SECRET".to_vec(),
            timeout: Duration::from_secs(1),
        };
        let response = HttpResponse {
            status: 200,
            headers: vec![("x-session".into(), "RESPONSE_HEADER_SECRET".into())],
            body: b"RESPONSE_BODY_SECRET".to_vec(),
        };
        let session = HostedResticSession {
            destination: Destination {
                repository: "s3:bucket/prefix".into(),
                backend: "s3".into(),
                credentials: Default::default(),
            },
            backend_env: BTreeMap::from([("AWS_SECRET_ACCESS_KEY".into(), "ENV_SECRET".into())]),
            global_options: vec![],
        };

        let rendered = format!("{request:?}\n{response:?}\n{session:?}");
        for secret in [
            "REQUEST_TOKEN",
            "REQUEST_BODY_SECRET",
            "RESPONSE_HEADER_SECRET",
            "RESPONSE_BODY_SECRET",
            "ENV_SECRET",
        ] {
            assert!(!rendered.contains(secret));
        }
    }
    #[test]
    fn broker_maps_subscription_and_keeps_token_out_of_error() {
        for response in [
            HttpResponse {
                status: 402,
                headers: vec![],
                body: vec![],
            },
            HttpResponse {
                status: 200,
                headers: vec![],
                body: br#"{"needs_subscription":true}"#.to_vec(),
            },
        ] {
            let transport = Fixture(Ok(response));
            let error =
                fetch_hosted_credentials(&transport, &binding(), "backup", "1").unwrap_err();
            assert_eq!(error.reason_code, "hosted_entitlement_inactive");
            assert!(!error.to_string().contains("TOKEN"));
        }
    }

    #[test]
    fn broker_maps_superseded_binding_and_keeps_token_out_of_error() {
        let transport = Fixture(Ok(HttpResponse {
            status: 401,
            headers: vec![],
            body: br#"{"error":"binding_superseded"}"#.to_vec(),
        }));
        let error = fetch_hosted_credentials(&transport, &binding(), "backup", "1").unwrap_err();
        assert_eq!(error.reason_code, "binding_superseded");
        assert!(!error.to_string().contains("TOKEN"));
    }

    #[test]
    fn broker_maps_unrecognized_unauthorized_bodies_to_invalid_binding() {
        for body in [
            br#"{"error":"invalid_token"}"#.to_vec(),
            vec![],
            b"not-json".to_vec(),
        ] {
            let transport = Fixture(Ok(HttpResponse {
                status: 401,
                headers: vec![],
                body,
            }));
            let error =
                fetch_hosted_credentials(&transport, &binding(), "backup", "1").unwrap_err();
            assert_eq!(error.reason_code, "binding_invalid");
            assert!(!error.to_string().contains("TOKEN"));
        }
    }

    #[test]
    fn broker_maps_non_authorization_http_failure_to_broker_error() {
        let transport = Fixture(Ok(HttpResponse {
            status: 500,
            headers: vec![],
            body: vec![],
        }));
        let error = fetch_hosted_credentials(&transport, &binding(), "backup", "1").unwrap_err();
        assert_eq!(error.reason_code, "broker_error");
        assert!(!error.to_string().contains("TOKEN"));
    }

    struct CountingTransport {
        response: Result<HttpResponse, HttpError>,
        calls: RefCell<Vec<HttpRequest>>,
    }
    impl HttpTransport for CountingTransport {
        fn execute(&self, request: &HttpRequest) -> Result<HttpResponse, HttpError> {
            self.calls.borrow_mut().push(request.clone());
            self.response.clone()
        }
    }
    fn credentials_response() -> HttpResponse {
        HttpResponse { status:200, headers:vec![], body:serde_json::to_vec(&json!({"access_key_id":"ACCESS","secret_access_key":"SECRET","session_token":"SESSION","endpoint":"https://s3.example","expires_at":"tomorrow"})).unwrap() }
    }

    #[test]
    fn broker_request_has_reference_auth_shape_and_validates_failure_classes() {
        let transport = CountingTransport {
            response: Ok(credentials_response()),
            calls: RefCell::new(vec![]),
        };
        let credentials =
            fetch_hosted_credentials(&transport, &binding(), "backup", "1.0").unwrap();
        assert_eq!(credentials.endpoint, "https://s3.example");
        let request = transport.calls.borrow().first().unwrap().clone();
        assert_eq!(request.method, "POST");
        assert_eq!(request.url, "https://broker/backup/credentials");
        assert_eq!(request.timeout, Duration::from_secs(BROKER_TIMEOUT_SECONDS));
        assert!(
            request
                .headers
                .iter()
                .any(|(name, value)| name == "Authorization" && value == "Bearer TOKEN")
        );
        for (response, expected_reason_code) in [
            (Err(HttpError::Timeout), "broker_unreachable"),
            (
                Ok(HttpResponse {
                    status: 200,
                    headers: vec![],
                    body: b"not-json".to_vec(),
                }),
                "broker_error",
            ),
        ] {
            let error = fetch_hosted_credentials(
                &CountingTransport {
                    response,
                    calls: RefCell::new(vec![]),
                },
                &binding(),
                "backup",
                "1",
            )
            .unwrap_err();
            assert_eq!(error.reason_code, expected_reason_code);
            assert!(!error.to_string().contains("TOKEN"));
        }
    }

    #[test]
    fn quote_rclone_program_quotes_and_rejects_embedded_quotes() {
        assert_eq!(
            quote_rclone_program(Path::new(r"C:\Program Files\solstone\bin\rclone.exe")).unwrap(),
            r#"rclone.program="C:\Program Files\solstone\bin\rclone.exe""#
        );
        assert_eq!(
            quote_rclone_program(Path::new("/opt/solstone app/lib/solstone-rclone/rclone"))
                .unwrap(),
            r#"rclone.program="/opt/solstone app/lib/solstone-rclone/rclone""#
        );
        assert!(matches!(
            quote_rclone_program(Path::new(r#"C:\injected"payload\rclone.exe"#)),
            Err(RunnerError::BareProgram)
        ));
        assert!(matches!(
            quote_rclone_program(Path::new(r#"/injected"payload/rclone"#)),
            Err(RunnerError::BareProgram)
        ));
    }

    fn split_shell_strings(input: &str) -> Result<Vec<String>, String> {
        let mut words = Vec::new();
        let mut current = String::new();
        let mut in_quotes = false;
        let mut in_word = false;
        let mut chars = input.chars().peekable();

        while let Some(c) = chars.next() {
            if in_quotes {
                if c == '"' {
                    in_quotes = false;
                } else if c == '\\' {
                    if let Some(next) = chars.next() {
                        current.push(next);
                    } else {
                        return Err("trailing escape inside quote".to_string());
                    }
                } else {
                    current.push(c);
                }
            } else if c == '\\' {
                if let Some(next) = chars.next() {
                    current.push(next);
                    in_word = true;
                } else {
                    return Err("trailing escape".to_string());
                }
            } else if c == '"' {
                in_quotes = true;
                in_word = true;
            } else if c == ' ' || c == '\t' || c == '\n' {
                if in_word {
                    words.push(std::mem::take(&mut current));
                    in_word = false;
                }
            } else {
                current.push(c);
                in_word = true;
            }
        }

        if in_quotes {
            return Err("unclosed quote".to_string());
        }
        if in_word {
            words.push(current);
        }
        Ok(words)
    }

    #[test]
    fn split_shell_strings_oracle_semantics_and_acceptance() {
        // Oracle edge cases
        assert_eq!(
            split_shell_strings("a b\tc\nd").unwrap(),
            vec!["a", "b", "c", "d"]
        );
        assert_eq!(
            split_shell_strings(r#"hello "world with spaces" test"#).unwrap(),
            vec!["hello", "world with spaces", "test"]
        );
        assert_eq!(
            split_shell_strings("escaped\\ space and\\\tab").unwrap(),
            vec!["escaped space", "and\tab"]
        );
        assert!(split_shell_strings(r#""unclosed quote"#).is_err());
        assert!(split_shell_strings(r#"trailing escape\"#).is_err());

        // Acceptance: a package root containing a space produces an rclone.program= value
        // that the oracle splits into exactly one argument, equal to the verified rclone member path.
        let member_path = Path::new("/opt/solstone package root/lib/solstone-rclone/rclone");
        let program_opt = quote_rclone_program(member_path).unwrap();
        let raw_val = program_opt.strip_prefix("rclone.program=").unwrap();
        let split = split_shell_strings(raw_val).expect("splits cleanly");
        assert_eq!(split.len(), 1);
        assert_eq!(split[0], member_path.to_str().unwrap());

        // A path containing `"` refuses and does not spawn.
        let injected = Path::new(r#"/opt/solstone"bad/lib/solstone-rclone/rclone"#);
        assert!(matches!(
            quote_rclone_program(injected),
            Err(RunnerError::BareProgram)
        ));
    }

    #[test]
    fn platform_rclone_no_config_matches_platform() {
        if cfg!(windows) {
            assert_eq!(platform_rclone_no_config(), "NUL");
        } else {
            assert_eq!(platform_rclone_no_config(), "/dev/null");
        }
    }
}

#[cfg(all(test, feature = "full-tests"))]
mod body_deadline_tests {
    use super::*;
    use std::io::{self, Read, Write};
    use std::net::{TcpListener, TcpStream};
    use std::thread::{self, JoinHandle};

    struct ResponsePlan {
        status: u16,
        body: Vec<u8>,
        declared_length: Option<usize>,
        header_delay: Duration,
        body_delay: Duration,
        chunk_size: usize,
        chunk_delay: Duration,
    }

    impl ResponsePlan {
        fn body(body: Vec<u8>) -> Self {
            Self {
                status: 200,
                body,
                declared_length: None,
                header_delay: Duration::ZERO,
                body_delay: Duration::ZERO,
                chunk_size: usize::MAX,
                chunk_delay: Duration::ZERO,
            }
        }
    }

    #[derive(Debug)]
    struct ObservedRequest {
        method: String,
        headers: BTreeMap<String, String>,
        body: Vec<u8>,
    }

    struct Server(Option<JoinHandle<io::Result<ObservedRequest>>>);

    impl Server {
        fn finish(mut self) -> ObservedRequest {
            self.0.take().unwrap().join().unwrap().unwrap()
        }
    }

    impl Drop for Server {
        fn drop(&mut self) {
            if let Some(handle) = self.0.take() {
                let _ = handle.join();
            }
        }
    }

    fn read_request(stream: &mut TcpStream) -> io::Result<ObservedRequest> {
        let mut raw = Vec::new();
        while !raw.ends_with(b"\r\n\r\n") {
            let mut byte = [0];
            stream.read_exact(&mut byte)?;
            raw.push(byte[0]);
            if raw.len() > 16 * 1024 {
                return Err(io::Error::other("synthetic request headers too large"));
            }
        }
        let text = String::from_utf8(raw).map_err(io::Error::other)?;
        let mut lines = text.lines();
        let method = lines
            .next()
            .and_then(|line| line.split_whitespace().next())
            .ok_or_else(|| io::Error::other("synthetic request method missing"))?
            .to_owned();
        let headers: BTreeMap<_, _> = lines
            .filter_map(|line| line.split_once(':'))
            .map(|(name, value)| (name.to_ascii_lowercase(), value.trim().to_owned()))
            .collect();
        let length = headers
            .get("content-length")
            .map(|value| value.parse::<usize>().map_err(io::Error::other))
            .transpose()?
            .unwrap_or(0);
        let mut body = vec![0; length];
        stream.read_exact(&mut body)?;
        Ok(ObservedRequest {
            method,
            headers,
            body,
        })
    }

    fn exchange(
        method: &str,
        request_body: Vec<u8>,
        timeout: Duration,
        plan: ResponsePlan,
    ) -> (Result<HttpResponse, HttpError>, ObservedRequest) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        listener.set_nonblocking(true).unwrap();
        let server = Server(Some(thread::spawn(move || {
            let accept_deadline = Instant::now() + Duration::from_secs(3);
            let mut stream = loop {
                match listener.accept() {
                    Ok((stream, _)) => break stream,
                    Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                        if Instant::now() >= accept_deadline {
                            return Err(io::Error::new(io::ErrorKind::TimedOut, "no test request"));
                        }
                        thread::sleep(Duration::from_millis(2));
                    }
                    Err(error) => return Err(error),
                }
            };
            stream.set_read_timeout(Some(Duration::from_secs(2)))?;
            stream.set_write_timeout(Some(Duration::from_secs(2)))?;
            let observed = read_request(&mut stream)?;
            thread::sleep(plan.header_delay);
            let header = format!(
                "HTTP/1.1 {} Control\r\nContent-Length: {}\r\nX-Body-Control: exact\r\nConnection: close\r\n\r\n",
                plan.status,
                plan.declared_length.unwrap_or(plan.body.len())
            );
            stream.write_all(header.as_bytes())?;
            thread::sleep(plan.body_delay);
            let chunks = plan.body.chunks(plan.chunk_size);
            let count = chunks.len();
            for (index, chunk) in chunks.enumerate() {
                if stream.write_all(chunk).is_err() {
                    break;
                }
                if index + 1 < count {
                    thread::sleep(plan.chunk_delay);
                }
            }
            Ok(observed)
        })));
        let request = HttpRequest {
            method: method.into(),
            url: format!("http://{address}/body-control"),
            headers: vec![
                ("X-Request-Control".into(), "selected".into()),
                ("Authorization".into(), "Bearer synthetic-token".into()),
                ("Connection".into(), "close".into()),
            ],
            body: request_body,
            timeout,
        };
        let started = Instant::now();
        let response = UreqHttpTransport.execute(&request);
        let elapsed = started.elapsed();
        let observed = server.finish();
        eprintln!("body control {method}: budget {timeout:?}, elapsed {elapsed:?}, {response:?}");
        (response, observed)
    }

    fn nonuniform_body(length: usize) -> Vec<u8> {
        (0..length)
            .map(|index| ((index * 37) % 251) as u8)
            .collect()
    }

    #[test]
    fn exact_multiread_body_and_request_fields_survive_get_and_post() {
        let body = nonuniform_body(5_123);
        for method in ["GET", "POST"] {
            let sent = if method == "POST" {
                nonuniform_body(1_319)
            } else {
                vec![]
            };
            let (response, observed) = exchange(
                method,
                sent.clone(),
                Duration::from_secs(2),
                ResponsePlan::body(body.clone()),
            );
            assert_eq!(observed.method, method);
            assert_eq!(observed.body, sent);
            assert_eq!(observed.headers["x-request-control"], "selected");
            assert_eq!(observed.headers["authorization"], "Bearer synthetic-token");
            let response = response.unwrap();
            assert_eq!(response.status, 200);
            assert_eq!(response.body, body);
            assert!(
                response
                    .headers
                    .contains(&("x-body-control".into(), "exact".into()))
            );
        }
    }

    #[test]
    fn empty_and_unauthorized_responses_keep_http_semantics() {
        for (status, body) in [(204, vec![]), (401, b"denied".to_vec())] {
            let mut plan = ResponsePlan::body(body.clone());
            plan.status = status;
            let (response, observed) = exchange("GET", vec![], Duration::from_secs(2), plan);
            assert_eq!(observed.method, "GET");
            let response = response.unwrap();
            assert_eq!(response.status, status);
            assert_eq!(response.body, body);
        }
    }

    #[test]
    fn stalled_body_is_timeout() {
        let mut plan = ResponsePlan::body(vec![1]);
        plan.body_delay = Duration::from_millis(350);
        let (response, _) = exchange("POST", vec![], Duration::from_millis(120), plan);
        assert_eq!(response, Err(HttpError::Timeout));
    }

    #[test]
    fn continuing_body_cannot_succeed_after_deadline() {
        let mut plan = ResponsePlan::body(nonuniform_body(2_048));
        plan.chunk_size = 64;
        plan.chunk_delay = Duration::from_millis(10);
        let (response, _) = exchange("POST", vec![], Duration::from_millis(120), plan);
        assert_eq!(response, Err(HttpError::Timeout));
    }

    #[test]
    fn prompt_truncation_is_other_without_partial_success() {
        let mut plan = ResponsePlan::body(vec![1, 2, 3, 4]);
        plan.declared_length = Some(128);
        let (response, _) = exchange("GET", vec![], Duration::from_secs(2), plan);
        assert_eq!(response, Err(HttpError::Other));
    }

    #[test]
    fn header_and_body_share_entry_budget() {
        let body = nonuniform_body(2_048);
        let mut prompt = ResponsePlan::body(body.clone());
        prompt.chunk_size = 128;
        prompt.chunk_delay = Duration::from_millis(20);
        let (response, _) = exchange("GET", vec![], Duration::from_millis(500), prompt);
        assert_eq!(response.unwrap().body, body);

        let mut delayed = ResponsePlan::body(body);
        delayed.chunk_size = 128;
        delayed.chunk_delay = Duration::from_millis(20);
        delayed.header_delay = Duration::from_millis(300);
        let (response, _) = exchange("GET", vec![], Duration::from_millis(500), delayed);
        assert_eq!(response, Err(HttpError::Timeout));
    }

    #[test]
    fn zero_budget_issues_no_http_request() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let request = HttpRequest {
            method: "GET".into(),
            url: format!("http://{}/zero", listener.local_addr().unwrap()),
            headers: vec![],
            body: vec![],
            timeout: Duration::ZERO,
        };
        assert_eq!(UreqHttpTransport.execute(&request), Err(HttpError::Timeout));
        assert_eq!(
            listener.accept().unwrap_err().kind(),
            io::ErrorKind::WouldBlock
        );
    }
}

/// The backup page's hosted setup and restore poll through `execute_cancellable`.
/// It once refused before any request on every platform but Linux. Where the
/// owned resolver runs, it needs the installed package and the packaged teardown
/// control covers it.
#[cfg(all(test, feature = "full-tests"))]
#[cfg(not(any(target_os = "linux", target_os = "macos")))]
mod transport_tests {
    use super::*;
    use std::io::{Read, Write};

    #[test]
    fn cancellable_requests_reach_the_network_without_an_owned_resolver() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let request = HttpRequest {
            method: "GET".into(),
            url: format!("http://{}/poll", listener.local_addr().unwrap()),
            headers: vec![("Connection".into(), "close".into())],
            body: vec![],
            timeout: Duration::from_secs(10),
        };
        let server = std::thread::spawn(move || {
            let (mut peer, _) = listener.accept().unwrap();
            let mut head = Vec::new();
            let mut byte = [0];
            while !head.ends_with(b"\r\n\r\n") {
                peer.read_exact(&mut byte).unwrap();
                head.push(byte[0]);
            }
            peer.write_all(b"HTTP/1.1 204 No Content\r\nConnection: close\r\n\r\n")
                .unwrap();
        });
        let response = UreqHttpTransport
            .execute_cancellable(&request, &ArchiveCancellation::new())
            .unwrap();
        server.join().unwrap();
        assert_eq!(response.status, 204);
        let cancelled = ArchiveCancellation::new();
        cancelled.cancel();
        assert_eq!(
            UreqHttpTransport.execute_cancellable(&request, &cancelled),
            Err(HttpError::Other)
        );
    }
}
