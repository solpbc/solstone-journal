// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::io::{Cursor, Read};
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde_json::Value as JsonValue;

use crate::error::ClientError;
use crate::seam::{HttpTransport, LinkStatusProbe};

const API_CONNECT_SECONDS: u64 = 2;
const API_READ_SECONDS: u64 = 20;
const API_TOTAL_SECONDS: u64 = 30;
const UPLOAD_CONNECT_SECONDS: u64 = 2;
const UPLOAD_READ_SECONDS: u64 = 120;
const UPLOAD_TOTAL_SECONDS: u64 = 180;
const SSE_CONNECT_SECONDS: u64 = 10;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HttpMethod {
    Delete,
    Get,
    Post,
    Put,
}

impl HttpMethod {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            HttpMethod::Delete => "DELETE",
            HttpMethod::Get => "GET",
            HttpMethod::Post => "POST",
            HttpMethod::Put => "PUT",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TimeoutPolicy {
    Api,
    Upload,
    SseOpen,
}

impl TimeoutPolicy {
    #[must_use]
    pub fn spec(self) -> TimeoutSpec {
        match self {
            TimeoutPolicy::Api => TimeoutSpec {
                connect: Duration::from_secs(API_CONNECT_SECONDS),
                read: Some(Duration::from_secs(API_READ_SECONDS)),
                total: Some(Duration::from_secs(API_TOTAL_SECONDS)),
            },
            TimeoutPolicy::Upload => TimeoutSpec {
                connect: Duration::from_secs(UPLOAD_CONNECT_SECONDS),
                read: Some(Duration::from_secs(UPLOAD_READ_SECONDS)),
                total: Some(Duration::from_secs(UPLOAD_TOTAL_SECONDS)),
            },
            TimeoutPolicy::SseOpen => TimeoutSpec {
                connect: Duration::from_secs(SSE_CONNECT_SECONDS),
                read: None,
                total: None,
            },
        }
    }

    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            TimeoutPolicy::Api => "api",
            TimeoutPolicy::Upload => "upload",
            TimeoutPolicy::SseOpen => "sse-open",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TimeoutSpec {
    pub connect: Duration,
    pub read: Option<Duration>,
    pub total: Option<Duration>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum QueryValue {
    Single(String),
    Many(Vec<String>),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QueryParam {
    pub key: String,
    pub value: QueryValue,
}

impl QueryParam {
    #[must_use]
    pub fn single(key: impl Into<String>, value: impl Into<String>) -> Self {
        Self {
            key: key.into(),
            value: QueryValue::Single(value.into()),
        }
    }

    #[must_use]
    pub fn many(key: impl Into<String>, values: Vec<String>) -> Self {
        Self {
            key: key.into(),
            value: QueryValue::Many(values),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct ApiRequest {
    pub method: HttpMethod,
    pub path: String,
    pub params: Vec<QueryParam>,
    pub json: Option<JsonValue>,
    pub headers: Vec<(String, String)>,
    pub policy: TimeoutPolicy,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FormField {
    pub name: String,
    pub value: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MultipartFile {
    pub field_name: String,
    pub filename: String,
    pub content_type: Option<String>,
    pub body: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UploadRequest {
    pub path: String,
    pub files: Vec<MultipartFile>,
    pub data: Vec<FormField>,
    pub headers: Vec<(String, String)>,
    pub boundary: Option<String>,
    pub policy: TimeoutPolicy,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SseRequest {
    pub path: String,
    pub policy: TimeoutPolicy,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HttpResponse {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
    pub policy: TimeoutPolicy,
}

pub struct SseStream {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub body: Box<dyn Read + Send>,
    pub policy: TimeoutPolicy,
}

pub trait BoundaryGenerator: Send + Sync {
    fn boundary(&self) -> String;
}

#[derive(Debug, Default)]
pub struct SystemBoundaryGenerator;

impl BoundaryGenerator for SystemBoundaryGenerator {
    fn boundary(&self) -> String {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        format!("solstone-native-{nanos:x}")
    }
}

pub struct UreqHttpTransport {
    base_url: String,
    api_agent: ureq::Agent,
    upload_agent: ureq::Agent,
    sse_agent: ureq::Agent,
    boundary_generator: Arc<dyn BoundaryGenerator>,
}

impl UreqHttpTransport {
    #[must_use]
    pub fn new(port: i64) -> Self {
        Self::with_base_url(format!("http://localhost:{port}"))
    }

    #[must_use]
    pub fn with_base_url(base_url: impl Into<String>) -> Self {
        Self::with_boundary_generator(base_url, Arc::new(SystemBoundaryGenerator))
    }

    #[must_use]
    pub fn with_boundary_generator(
        base_url: impl Into<String>,
        boundary_generator: Arc<dyn BoundaryGenerator>,
    ) -> Self {
        Self {
            base_url: base_url.into(),
            api_agent: agent_for(TimeoutPolicy::Api),
            upload_agent: agent_for(TimeoutPolicy::Upload),
            sse_agent: agent_for(TimeoutPolicy::SseOpen),
            boundary_generator,
        }
    }

    fn agent(&self, policy: TimeoutPolicy) -> &ureq::Agent {
        match policy {
            TimeoutPolicy::Api => &self.api_agent,
            TimeoutPolicy::Upload => &self.upload_agent,
            TimeoutPolicy::SseOpen => &self.sse_agent,
        }
    }
}

#[derive(Debug, Clone, Copy, Default)]
pub struct UreqLinkStatusProbe;

impl LinkStatusProbe for UreqLinkStatusProbe {
    fn probe(&self, port: u16) -> Result<HttpResponse, ClientError> {
        let config = ureq::Agent::config_builder()
            .http_status_as_error(false)
            .max_redirects(0)
            .proxy(None)
            .timeout_global(Some(Duration::from_secs(5)))
            .build();
        let agent = ureq::Agent::new_with_config(config);
        let url = format!("http://127.0.0.1:{port}/_solstone/link/status");
        let response = agent
            .get(&url)
            .header("Cache-Control", "no-cache")
            .header("Pragma", "no-cache")
            .call()
            .map_err(|error| {
                map_ureq_error(
                    error,
                    TimeoutPolicy::Api,
                    HttpMethod::Get,
                    "/_solstone/link/status",
                )
            })?;
        collect_response(
            response,
            TimeoutPolicy::Api,
            HttpMethod::Get,
            "/_solstone/link/status",
        )
    }
}

impl HttpTransport for UreqHttpTransport {
    fn request(&self, request: ApiRequest) -> Result<HttpResponse, ClientError> {
        assert!(
            request.path.starts_with('/'),
            "convey path must start with '/'"
        );
        let url = build_url(&self.base_url, &request.path, &request.params);
        let policy = request.policy;
        let response = match request.method {
            HttpMethod::Get => {
                let builder = add_headers(self.agent(policy).get(&url), &request.headers);
                builder
                    .call()
                    .map_err(|error| map_ureq_error(error, policy, request.method, &request.path))?
            }
            HttpMethod::Delete => {
                let builder = add_headers(self.agent(policy).delete(&url), &request.headers);
                builder
                    .call()
                    .map_err(|error| map_ureq_error(error, policy, request.method, &request.path))?
            }
            HttpMethod::Post => {
                let builder = add_headers(self.agent(policy).post(&url), &request.headers);
                if let Some(value) = request.json.as_ref() {
                    builder
                        .header("Content-Type", "application/json")
                        .send(json_body(value)?)
                        .map_err(|error| {
                            map_ureq_error(error, policy, request.method, &request.path)
                        })?
                } else {
                    builder.send_empty().map_err(|error| {
                        map_ureq_error(error, policy, request.method, &request.path)
                    })?
                }
            }
            HttpMethod::Put => {
                let builder = add_headers(self.agent(policy).put(&url), &request.headers);
                if let Some(value) = request.json.as_ref() {
                    builder
                        .header("Content-Type", "application/json")
                        .send(json_body(value)?)
                        .map_err(|error| {
                            map_ureq_error(error, policy, request.method, &request.path)
                        })?
                } else {
                    builder.send_empty().map_err(|error| {
                        map_ureq_error(error, policy, request.method, &request.path)
                    })?
                }
            }
        };
        collect_response(response, policy, request.method, &request.path)
    }

    fn upload(&self, request: UploadRequest) -> Result<HttpResponse, ClientError> {
        assert!(
            request.path.starts_with('/'),
            "convey path must start with '/'"
        );
        let boundary = request
            .boundary
            .clone()
            .unwrap_or_else(|| self.boundary_generator.boundary());
        let body = build_multipart_body(&request.data, &request.files, &boundary);
        let content_type = format!("multipart/form-data; boundary={boundary}");
        let mut headers = request.headers.clone();
        headers.push(("Content-Type".to_string(), content_type));
        let url = build_url(&self.base_url, &request.path, &[]);
        let policy = request.policy;
        let builder = add_headers(self.agent(policy).post(&url), &headers);
        let response = builder
            .send(body)
            .map_err(|error| map_ureq_error(error, policy, HttpMethod::Post, &request.path))?;
        collect_response(response, policy, HttpMethod::Post, &request.path)
    }

    fn open_sse(&self, request: SseRequest) -> Result<SseStream, ClientError> {
        assert!(
            request.path.starts_with('/'),
            "convey path must start with '/'"
        );
        let url = build_url(&self.base_url, &request.path, &[]);
        let policy = request.policy;
        let response = self
            .agent(policy)
            .get(&url)
            .call()
            .map_err(|error| map_ureq_error(error, policy, HttpMethod::Get, &request.path))?;
        let status = response.status().as_u16();
        let headers = collect_headers(response.headers());
        Ok(SseStream {
            status,
            headers,
            body: Box::new(response.into_body().into_reader()),
            policy,
        })
    }
}

fn agent_for(policy: TimeoutPolicy) -> ureq::Agent {
    let spec = policy.spec();
    let config = ureq::Agent::config_builder()
        .http_status_as_error(false)
        .timeout_connect(Some(spec.connect))
        .timeout_recv_response(spec.read)
        .timeout_recv_body(spec.read)
        .timeout_global(spec.total)
        .build();
    ureq::Agent::new_with_config(config)
}

fn add_headers<B>(
    mut builder: ureq::RequestBuilder<B>,
    headers: &[(String, String)],
) -> ureq::RequestBuilder<B> {
    for (name, value) in headers {
        builder = builder.header(name, value);
    }
    builder
}

fn json_body(value: &JsonValue) -> Result<Vec<u8>, ClientError> {
    serde_json::to_vec(value).map_err(|error| {
        ClientError::unreachable(Some(format!("failed to encode JSON request: {error}")))
    })
}

fn collect_response(
    mut response: ureq::http::Response<ureq::Body>,
    policy: TimeoutPolicy,
    method: HttpMethod,
    path: &str,
) -> Result<HttpResponse, ClientError> {
    let status = response.status().as_u16();
    let headers = collect_headers(response.headers());
    let mut body = Vec::new();
    response
        .body_mut()
        .as_reader()
        .read_to_end(&mut body)
        .map_err(|error| map_read_error(error, policy, method, path))?;
    Ok(HttpResponse {
        status,
        headers,
        body,
        policy,
    })
}

fn collect_headers(headers: &ureq::http::HeaderMap) -> Vec<(String, String)> {
    headers
        .iter()
        .map(|(name, value)| {
            (
                name.as_str().to_string(),
                value.to_str().unwrap_or_default().to_string(),
            )
        })
        .collect()
}

fn map_read_error(
    error: std::io::Error,
    policy: TimeoutPolicy,
    method: HttpMethod,
    path: &str,
) -> ClientError {
    if error.kind() == std::io::ErrorKind::TimedOut {
        ClientError::timeout(Some(timeout_detail(policy, method, path)))
    } else {
        ClientError::unreachable(Some(error.to_string()))
    }
}

fn map_ureq_error(
    error: ureq::Error,
    policy: TimeoutPolicy,
    method: HttpMethod,
    path: &str,
) -> ClientError {
    match error {
        ureq::Error::Timeout(_) => ClientError::timeout(Some(timeout_detail(policy, method, path))),
        ureq::Error::Io(error) if error.kind() == std::io::ErrorKind::TimedOut => {
            ClientError::timeout(Some(timeout_detail(policy, method, path)))
        }
        other => ClientError::unreachable(Some(other.to_string())),
    }
}

fn timeout_detail(policy: TimeoutPolicy, method: HttpMethod, path: &str) -> String {
    let spec = policy.spec();
    format!(
        "{} {path} exceeded local convey timeout (connect={}s, read={}, total={})",
        method.as_str(),
        spec.connect.as_secs(),
        format_duration(spec.read),
        format_duration(spec.total)
    )
}

fn format_duration(duration: Option<Duration>) -> String {
    duration
        .map(|duration| format!("{}s", duration.as_secs()))
        .unwrap_or_else(|| "none".to_string())
}

#[must_use]
pub fn build_url(base_url: &str, path: &str, params: &[QueryParam]) -> String {
    assert!(path.starts_with('/'), "convey path must start with '/'");
    let mut url = format!("{}{}", base_url.trim_end_matches('/'), path);
    let query = encode_query(params);
    if !query.is_empty() {
        let separator = if url.contains('?') { '&' } else { '?' };
        url.push(separator);
        url.push_str(&query);
    }
    url
}

#[must_use]
pub fn ordered_query_pairs(params: &[QueryParam]) -> Vec<(String, String)> {
    let mut pairs = Vec::new();
    for param in params {
        match &param.value {
            QueryValue::Single(value) => pairs.push((param.key.clone(), value.clone())),
            QueryValue::Many(values) => {
                for value in values {
                    pairs.push((param.key.clone(), value.clone()));
                }
            }
        }
    }
    pairs
}

#[must_use]
pub fn encode_query(params: &[QueryParam]) -> String {
    encode_query_pairs(&ordered_query_pairs(params))
}

#[must_use]
pub fn encode_query_pairs(pairs: &[(String, String)]) -> String {
    pairs
        .iter()
        .map(|(key, value)| format!("{}={}", quote_plus(key), quote_plus(value)))
        .collect::<Vec<_>>()
        .join("&")
}

fn quote_plus(value: &str) -> String {
    let mut encoded = String::new();
    for byte in value.as_bytes() {
        match *byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'_' | b'.' | b'-' | b'~' => {
                encoded.push(*byte as char);
            }
            b' ' => encoded.push('+'),
            other => encoded.push_str(&format!("%{other:02X}")),
        }
    }
    encoded
}

#[must_use]
pub fn build_multipart_body(
    data: &[FormField],
    files: &[MultipartFile],
    boundary: &str,
) -> Vec<u8> {
    let mut body = Vec::new();
    for field in data {
        push_ascii(&mut body, "--");
        push_ascii(&mut body, boundary);
        push_ascii(&mut body, "\r\n");
        push_ascii(
            &mut body,
            &format!(
                "Content-Disposition: form-data; name=\"{}\"\r\n\r\n",
                escape_multipart_quoted(&field.name)
            ),
        );
        body.extend_from_slice(field.value.as_bytes());
        push_ascii(&mut body, "\r\n");
    }
    for file in files {
        push_ascii(&mut body, "--");
        push_ascii(&mut body, boundary);
        push_ascii(&mut body, "\r\n");
        push_ascii(
            &mut body,
            &format!(
                "Content-Disposition: form-data; name=\"{}\"; filename=\"{}\"\r\n",
                escape_multipart_quoted(&file.field_name),
                escape_multipart_quoted(&file.filename)
            ),
        );
        if let Some(content_type) = file.content_type.as_ref() {
            push_ascii(&mut body, &format!("Content-Type: {content_type}\r\n\r\n"));
        } else {
            push_ascii(&mut body, "\r\n");
        }
        body.extend_from_slice(&file.body);
        push_ascii(&mut body, "\r\n");
    }
    push_ascii(&mut body, "--");
    push_ascii(&mut body, boundary);
    push_ascii(&mut body, "--\r\n");
    body
}

fn push_ascii(output: &mut Vec<u8>, value: &str) {
    output.extend_from_slice(value.as_bytes());
}

fn escape_multipart_quoted(value: &str) -> String {
    value
        .replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('\r', "%0D")
        .replace('\n', "%0A")
}

#[must_use]
pub fn selected_http_client_surface() -> (&'static str, usize) {
    (
        "ureq",
        std::mem::size_of::<ureq::http::Response<ureq::Body>>()
            + std::mem::size_of::<ureq::Error>(),
    )
}

#[must_use]
pub fn memory_sse_stream(chunks: Vec<Vec<u8>>, policy: TimeoutPolicy) -> SseStream {
    let bytes = chunks.into_iter().flatten().collect::<Vec<_>>();
    SseStream {
        status: 200,
        headers: vec![],
        body: Box::new(Cursor::new(bytes)),
        policy,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io;

    #[test]
    fn query_encoding_preserves_order_and_repeats() {
        let params = vec![
            QueryParam::single("day", "20260723"),
            QueryParam::many("facet", vec!["work".to_string(), "personal".to_string()]),
            QueryParam::many(
                "include_hidden",
                vec!["muted items".to_string(), "done/items".to_string()],
            ),
        ];
        assert_eq!(
            ordered_query_pairs(&params),
            vec![
                ("day".to_string(), "20260723".to_string()),
                ("facet".to_string(), "work".to_string()),
                ("facet".to_string(), "personal".to_string()),
                ("include_hidden".to_string(), "muted items".to_string()),
                ("include_hidden".to_string(), "done/items".to_string()),
            ]
        );
        assert_eq!(
            encode_query(&params),
            "day=20260723&facet=work&facet=personal&include_hidden=muted+items&include_hidden=done%2Fitems"
        );
    }

    #[test]
    fn query_encoding_uses_quote_plus_utf8() {
        assert_eq!(
            encode_query(&[QueryParam::single("q", "café & tea")]),
            "q=caf%C3%A9+%26+tea"
        );
    }

    #[test]
    fn url_builder_matches_python_join_and_separator() {
        assert_eq!(
            build_url(
                "http://localhost:5015/",
                "/items?existing=1",
                &[QueryParam::single("next", "two words")]
            ),
            "http://localhost:5015/items?existing=1&next=two+words"
        );
    }

    #[test]
    fn multipart_body_is_deterministic() {
        let body = build_multipart_body(
            &[FormField {
                name: "description".to_string(),
                value: "hello".to_string(),
            }],
            &[MultipartFile {
                field_name: "file".to_string(),
                filename: "note.txt".to_string(),
                content_type: Some("text/plain".to_string()),
                body: b"abc".to_vec(),
            }],
            "BOUNDARY",
        );
        assert_eq!(
            String::from_utf8(body).expect("multipart utf-8 fixture"),
            "--BOUNDARY\r\nContent-Disposition: form-data; name=\"description\"\r\n\r\nhello\r\n--BOUNDARY\r\nContent-Disposition: form-data; name=\"file\"; filename=\"note.txt\"\r\nContent-Type: text/plain\r\n\r\nabc\r\n--BOUNDARY--\r\n"
        );
    }

    #[test]
    fn timeout_specs_are_pinned() {
        assert_eq!(TimeoutPolicy::Api.spec().connect, Duration::from_secs(2));
        assert_eq!(
            TimeoutPolicy::Upload.spec().read,
            Some(Duration::from_secs(120))
        );
        assert_eq!(TimeoutPolicy::SseOpen.spec().read, None);
    }

    #[test]
    fn api_timeout_detail_includes_method_path_and_limits() {
        assert_eq!(
            timeout_detail(TimeoutPolicy::Api, HttpMethod::Get, "/example/resource"),
            "GET /example/resource exceeded local convey timeout (connect=2s, read=20s, total=30s)"
        );
    }

    #[test]
    fn upload_timeout_detail_includes_method_path_and_limits() {
        assert_eq!(
            timeout_detail(TimeoutPolicy::Upload, HttpMethod::Post, "/example/upload"),
            "POST /example/upload exceeded local convey timeout (connect=2s, read=120s, total=180s)"
        );
    }

    #[test]
    fn read_timeout_error_preserves_request_target() {
        assert_eq!(
            map_read_error(
                io::Error::from(io::ErrorKind::TimedOut),
                TimeoutPolicy::Api,
                HttpMethod::Get,
                "/example/resource",
            ),
            ClientError::timeout(Some(
                "GET /example/resource exceeded local convey timeout (connect=2s, read=20s, total=30s)"
                    .to_string(),
            ))
        );
    }

    #[test]
    fn read_non_timeout_error_is_unreachable() {
        assert!(matches!(
            map_read_error(
                io::Error::from(io::ErrorKind::ConnectionReset),
                TimeoutPolicy::Api,
                HttpMethod::Get,
                "/example/resource",
            ),
            ClientError::Unreachable { detail: Some(_) }
        ));
    }

    #[test]
    fn ureq_connection_refused_is_unreachable() {
        assert!(matches!(
            map_ureq_error(
                ureq::Error::Io(io::Error::from(io::ErrorKind::ConnectionRefused)),
                TimeoutPolicy::Api,
                HttpMethod::Get,
                "/example/resource",
            ),
            ClientError::Unreachable { .. }
        ));
    }

    #[test]
    fn ureq_io_timeout_is_timeout() {
        assert!(matches!(
            map_ureq_error(
                ureq::Error::Io(io::Error::from(io::ErrorKind::TimedOut)),
                TimeoutPolicy::Api,
                HttpMethod::Get,
                "/example/resource",
            ),
            ClientError::Timeout { .. }
        ));
    }

    #[test]
    fn ureq_timeout_variant_is_timeout() {
        assert!(matches!(
            map_ureq_error(
                ureq::Error::Timeout(ureq::Timeout::RecvBody),
                TimeoutPolicy::Api,
                HttpMethod::Get,
                "/example/resource",
            ),
            ClientError::Timeout { .. }
        ));
    }
}
