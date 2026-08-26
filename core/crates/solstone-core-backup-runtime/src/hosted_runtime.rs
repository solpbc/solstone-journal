// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::collections::BTreeMap;
use std::fmt;
use std::path::Path;
use std::time::Duration;

use serde_json::{Value, json};
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
}
#[derive(Debug, Default)]
pub struct UreqHttpTransport;
impl HttpTransport for UreqHttpTransport {
    fn execute(&self, request: &HttpRequest) -> Result<HttpResponse, HttpError> {
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
                    .timeout_global(Some(request.timeout))
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
                    .timeout_global(Some(request.timeout))
                    .http_status_as_error(false)
                    .build()
                    .call()
            }
        }
        .map_err(|error| {
            if error.to_string().contains("timeout") {
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
        response
            .into_body()
            .into_reader()
            .read_to_end(&mut body)
            .map_err(|_| HttpError::Other)?;
        Ok(HttpResponse {
            status,
            headers,
            body,
        })
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
    let request = HttpRequest {
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
    };
    let response = transport
        .execute(&request)
        .map_err(|_| HostedCredsUnavailable {
            reason_code: "broker_unreachable",
        })?;
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
    Destination { repository:operated_repository(binding, credentials), backend:"s3".into(), credentials:serde_json::from_value(json!({"access_key_id":credentials.access_key_id,"secret_access_key":credentials.secret_access_key,"session_token":credentials.session_token})).expect("object") }
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
pub fn hosted_append_only_session(
    binding: &HostedBinding,
    credentials: &HostedCredentials,
    rclone: &Path,
) -> Result<HostedResticSession, RunnerError> {
    if !is_explicit_program_path(rclone) {
        return Err(RunnerError::BareProgram);
    }
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
        global_options: vec![
            "-o".into(),
            format!("rclone.program={}", rclone.display()),
            "-o".into(),
            "rclone.args=serve restic --stdio --append-only --config /dev/null".into(),
        ],
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
    use std::cell::RefCell;
    struct Fixture(Result<HttpResponse, HttpError>);
    impl HttpTransport for Fixture {
        fn execute(&self, _: &HttpRequest) -> Result<HttpResponse, HttpError> {
            self.0.clone()
        }
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
        assert!(
            session
                .global_options
                .contains(&"rclone.program=/fixture/bin/rclone".into())
        );
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
}
