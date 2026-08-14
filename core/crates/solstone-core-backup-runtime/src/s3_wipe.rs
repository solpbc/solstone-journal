// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Minimal SigV4 S3 prefix wipe client for operated backup teardown.

use std::fmt;

use hmac::{Hmac, Mac};
use sha2::{Digest, Sha256};

use crate::hosted_runtime::{HttpError, HttpRequest, HttpTransport};

type HmacSha256 = Hmac<Sha256>;
pub const S3_WIPE_TIMEOUT_SECONDS: u64 = 60;
pub const DELETE_OBJECT_BATCH_SIZE: usize = 1000;
const EMPTY_SHA256: &str = "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";

#[derive(Clone, PartialEq, Eq)]
pub struct S3Credentials {
    pub endpoint: String,
    pub access_key_id: String,
    pub secret_access_key: String,
    pub session_token: String,
    pub region: String,
}
impl fmt::Debug for S3Credentials {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("S3Credentials")
            .field("endpoint", &self.endpoint)
            .field("access_key_id", &"<redacted>")
            .field("secret_access_key", &"<redacted>")
            .field("session_token", &"<redacted>")
            .field("region", &self.region)
            .finish()
    }
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WipeResult {
    pub status: String,
    pub reason_code: Option<String>,
}
fn failure(reason: impl Into<String>) -> WipeResult {
    WipeResult {
        status: "error".into(),
        reason_code: Some(reason.into()),
    }
}

fn percent(value: &str) -> String {
    value
        .bytes()
        .map(|byte| {
            if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'~') {
                char::from(byte).to_string()
            } else {
                format!("%{byte:02X}")
            }
        })
        .collect()
}
pub fn canonical_uri(bucket: &str, key: Option<&str>) -> String {
    let mut parts = vec![percent(bucket)];
    if let Some(key) = key {
        parts.extend(key.split('/').map(percent));
    }
    format!("/{}", parts.join("/"))
}
pub fn canonical_query(params: &[(String, String)]) -> String {
    let mut params = params.to_vec();
    params.sort();
    params
        .iter()
        .map(|(name, value)| format!("{}={}", percent(name), percent(value)))
        .collect::<Vec<_>>()
        .join("&")
}
pub fn normalize_header_value(value: &str) -> String {
    value.split_whitespace().collect::<Vec<_>>().join(" ")
}
pub fn signing_key(secret: &str, date: &str, region: &str) -> Vec<u8> {
    fn sign(key: &[u8], value: &str) -> Vec<u8> {
        let mut mac = HmacSha256::new_from_slice(key).expect("hmac key");
        mac.update(value.as_bytes());
        mac.finalize().into_bytes().to_vec()
    }
    let date = sign(format!("AWS4{secret}").as_bytes(), date);
    let region = sign(&date, region);
    let service = sign(&region, "s3");
    sign(&service, "aws4_request")
}
#[allow(clippy::too_many_arguments)] // SigV4's independently specified inputs are an API vector.
pub fn authorization_header(
    method: &str,
    uri: &str,
    query: &str,
    headers: &[(String, String)],
    payload_hash: &str,
    access: &str,
    secret: &str,
    region: &str,
    amz_date: &str,
) -> String {
    let mut lowered = headers
        .iter()
        .map(|(name, value)| (name.to_ascii_lowercase(), normalize_header_value(value)))
        .filter(|(name, _)| {
            matches!(
                name.as_str(),
                "content-md5"
                    | "host"
                    | "x-amz-content-sha256"
                    | "x-amz-date"
                    | "x-amz-security-token"
            )
        })
        .collect::<Vec<_>>();
    lowered.sort_by(|left, right| left.0.cmp(&right.0));
    let canonical_headers = lowered
        .iter()
        .map(|(name, value)| format!("{name}:{value}\n"))
        .collect::<String>();
    let signed = lowered
        .iter()
        .map(|(name, _)| name.as_str())
        .collect::<Vec<_>>()
        .join(";");
    let canonical =
        format!("{method}\n{uri}\n{query}\n{canonical_headers}\n{signed}\n{payload_hash}");
    let date = &amz_date[..8];
    let scope = format!("{date}/{region}/s3/aws4_request");
    let string_to_sign = format!(
        "AWS4-HMAC-SHA256\n{amz_date}\n{scope}\n{:x}",
        Sha256::digest(canonical.as_bytes())
    );
    let mut mac = HmacSha256::new_from_slice(&signing_key(secret, date, region)).expect("hmac key");
    mac.update(string_to_sign.as_bytes());
    let signature = format!("{:x}", mac.finalize().into_bytes());
    format!(
        "AWS4-HMAC-SHA256 Credential={access}/{scope}, SignedHeaders={signed}, Signature={signature}"
    )
}

fn xml_values(body: &[u8], parent: &str, field: &str) -> Vec<String> {
    let text = String::from_utf8_lossy(body);
    let mut values = Vec::new();
    let mut rest = text.as_ref();
    let open = format!("<{parent}");
    let close = format!("</{parent}>");
    while let Some(start) = rest.find(&open) {
        let part = &rest[start..];
        let Some(end) = part.find(&close) else { break };
        let chunk = &part[..end];
        let field_open = format!("<{field}>");
        let field_close = format!("</{field}>");
        if let Some(value_start) = chunk.find(&field_open) {
            let value = &chunk[value_start + field_open.len()..];
            if let Some(value_end) = value.find(&field_close) {
                values.push(value[..value_end].to_owned());
            }
        }
        rest = &part[end + close.len()..];
    }
    values
}
fn xml_field(body: &[u8], field: &str) -> Option<String> {
    let text = String::from_utf8_lossy(body);
    let open = format!("<{field}>");
    let close = format!("</{field}>");
    let start = text.find(&open)? + open.len();
    let rest = &text[start..];
    Some(rest[..rest.find(&close)?].to_owned())
}
fn true_field(body: &[u8], field: &str) -> bool {
    xml_field(body, field).is_some_and(|value| value.eq_ignore_ascii_case("true"))
}
fn delete_body(keys: &[String]) -> Vec<u8> {
    let mut body = String::from("<Delete>");
    for key in keys {
        body.push_str("<Object><Key>");
        body.push_str(key);
        body.push_str("</Key></Object>");
    }
    body.push_str("</Delete>");
    body.into_bytes()
}
fn http_reason(status: u16) -> &'static str {
    if matches!(status, 401 | 403) {
        "auth_failed"
    } else if matches!(status, 408 | 504) {
        "timeout"
    } else {
        "failed"
    }
}

struct Client<'a> {
    transport: &'a dyn HttpTransport,
    credentials: &'a S3Credentials,
    amz_date: String,
}
impl Client<'_> {
    fn request(
        &self,
        method: &str,
        bucket: &str,
        key: Option<&str>,
        query: Vec<(String, String)>,
        body: Vec<u8>,
        extra: Vec<(String, String)>,
    ) -> Result<Vec<u8>, WipeResult> {
        let base = self.credentials.endpoint.trim_end_matches('/');
        let uri = canonical_uri(bucket, key);
        let query_text = canonical_query(&query);
        let url = if query_text.is_empty() {
            format!("{base}{uri}")
        } else {
            format!("{base}{uri}?{query_text}")
        };
        let host = base
            .trim_start_matches("https://")
            .trim_start_matches("http://")
            .split('/')
            .next()
            .unwrap_or_default();
        let hash = if body.is_empty() {
            EMPTY_SHA256.to_owned()
        } else {
            format!("{:x}", Sha256::digest(&body))
        };
        let mut headers = vec![
            ("Host".into(), host.into()),
            ("x-amz-content-sha256".into(), hash.clone()),
            ("x-amz-date".into(), self.amz_date.clone()),
            (
                "x-amz-security-token".into(),
                self.credentials.session_token.clone(),
            ),
            ("Connection".into(), "close".into()),
        ];
        headers.extend(extra);
        let authorization = authorization_header(
            method,
            &uri,
            &query_text,
            &headers,
            &hash,
            &self.credentials.access_key_id,
            &self.credentials.secret_access_key,
            &self.credentials.region,
            &self.amz_date,
        );
        headers.push(("Authorization".into(), authorization));
        let response = self
            .transport
            .execute(&HttpRequest {
                method: method.into(),
                url,
                headers,
                body,
                timeout: std::time::Duration::from_secs(S3_WIPE_TIMEOUT_SECONDS),
            })
            .map_err(|error| {
                failure(match error {
                    HttpError::Timeout => "timeout",
                    HttpError::Unreachable => "unreachable",
                    HttpError::Other => "failed",
                })
            })?;
        if !(200..300).contains(&response.status) {
            return Err(failure(http_reason(response.status)));
        }
        Ok(response.body)
    }
    fn list_objects(&self, bucket: &str, prefix: &str) -> Result<Vec<String>, WipeResult> {
        let mut keys = Vec::new();
        let mut continuation = None;
        loop {
            let mut query = vec![
                ("list-type".into(), "2".into()),
                ("prefix".into(), prefix.into()),
            ];
            if let Some(token) = continuation.take() {
                query.push(("continuation-token".into(), token));
            }
            let body = self.request("GET", bucket, None, query, vec![], vec![])?;
            let page = xml_values(&body, "Contents", "Key");
            let content_records = String::from_utf8_lossy(&body).matches("<Contents").count();
            if page.len() != content_records {
                return Err(failure("failed"));
            }
            keys.extend(page.into_iter().filter(|key| key.starts_with(prefix)));
            if !true_field(&body, "IsTruncated") {
                return Ok(keys);
            }
            continuation = xml_field(&body, "NextContinuationToken");
            if continuation.is_none() {
                return Err(failure("failed"));
            }
        }
    }
    fn delete(&self, bucket: &str, keys: &[String]) -> Result<(), WipeResult> {
        let body = delete_body(keys);
        let md5 = base64(&md5::compute(&body).0);
        let response = self.request(
            "POST",
            bucket,
            None,
            vec![("delete".into(), "".into())],
            body,
            vec![("Content-MD5".into(), md5)],
        )?;
        for code in xml_values(&response, "Error", "Code") {
            if code != "NoSuchKey" {
                return Err(failure("failed"));
            }
        }
        Ok(())
    }
    fn uploads(&self, bucket: &str, prefix: &str) -> Result<Vec<(String, String)>, WipeResult> {
        let mut uploads = Vec::new();
        let (mut key_marker, mut upload_marker) = (None, None);
        loop {
            let mut query = vec![
                ("uploads".into(), "".into()),
                ("prefix".into(), prefix.into()),
            ];
            if let Some(marker) = key_marker.take() {
                query.push(("key-marker".into(), marker));
            }
            if let Some(marker) = upload_marker.take() {
                query.push(("upload-id-marker".into(), marker));
            }
            let body = self.request("GET", bucket, None, query, vec![], vec![])?;
            let keys = xml_values(&body, "Upload", "Key");
            let ids = xml_values(&body, "Upload", "UploadId");
            if keys.len() != ids.len() {
                return Err(failure("failed"));
            }
            uploads.extend(
                keys.into_iter()
                    .zip(ids)
                    .filter(|(key, _)| key.starts_with(prefix)),
            );
            if !true_field(&body, "IsTruncated") {
                return Ok(uploads);
            }
            key_marker = xml_field(&body, "NextKeyMarker");
            upload_marker = xml_field(&body, "NextUploadIdMarker");
            if key_marker.is_none() || upload_marker.is_none() {
                return Err(failure("failed"));
            }
        }
    }
    fn abort(&self, bucket: &str, key: &str, upload_id: &str) -> Result<(), WipeResult> {
        self.request(
            "DELETE",
            bucket,
            Some(key),
            vec![("uploadId".into(), upload_id.into())],
            vec![],
            vec![],
        )
        .map(|_| ())
    }
}
fn base64(bytes: &[u8]) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::new();
    for chunk in bytes.chunks(3) {
        let value = (u32::from(chunk[0]) << 16)
            | (u32::from(*chunk.get(1).unwrap_or(&0)) << 8)
            | u32::from(*chunk.get(2).unwrap_or(&0));
        out.push(TABLE[((value >> 18) & 63) as usize] as char);
        out.push(TABLE[((value >> 12) & 63) as usize] as char);
        out.push(if chunk.len() > 1 {
            TABLE[((value >> 6) & 63) as usize] as char
        } else {
            '='
        });
        out.push(if chunk.len() > 2 {
            TABLE[(value & 63) as usize] as char
        } else {
            '='
        });
    }
    out
}

/// Delete only objects whose returned key remains inside `prefix`; batches are max 1000.
pub fn wipe_prefix(
    transport: &dyn HttpTransport,
    credentials: &S3Credentials,
    bucket: &str,
    prefix: &str,
    amz_date: &str,
) -> WipeResult {
    if amz_date.len() != 16 {
        return failure("failed");
    }
    let client = Client {
        transport,
        credentials,
        amz_date: amz_date.into(),
    };
    let keys = match client.list_objects(bucket, prefix) {
        Ok(keys) => keys,
        Err(result) => return result,
    };
    for batch in keys.chunks(DELETE_OBJECT_BATCH_SIZE) {
        if let Err(result) = client.delete(bucket, batch) {
            return result;
        }
    }
    let uploads = match client.uploads(bucket, prefix) {
        Ok(uploads) => uploads,
        Err(result) => return result,
    };
    for (key, upload_id) in uploads {
        if let Err(result) = client.abort(bucket, &key, &upload_id) {
            return result;
        }
    }
    WipeResult {
        status: "ok".into(),
        reason_code: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;
    use std::collections::VecDeque;

    struct Fixture {
        responses: RefCell<VecDeque<Result<crate::hosted_runtime::HttpResponse, HttpError>>>,
        requests: RefCell<Vec<HttpRequest>>,
    }
    impl HttpTransport for Fixture {
        fn execute(
            &self,
            request: &HttpRequest,
        ) -> Result<crate::hosted_runtime::HttpResponse, HttpError> {
            self.requests.borrow_mut().push(request.clone());
            self.responses.borrow_mut().pop_front().expect("response")
        }
    }
    fn credentials() -> S3Credentials {
        S3Credentials {
            endpoint: "https://s3.example".into(),
            access_key_id: "ACCESS".into(),
            secret_access_key: "SECRET".into(),
            session_token: "TOKEN".into(),
            region: "us-east-1".into(),
        }
    }
    fn response(body: &str) -> Result<crate::hosted_runtime::HttpResponse, HttpError> {
        Ok(crate::hosted_runtime::HttpResponse {
            status: 200,
            headers: vec![],
            body: body.as_bytes().to_vec(),
        })
    }

    #[test]
    fn canonical_values_preserve_slashes_and_sort_query() {
        assert_eq!(canonical_uri("bucket", Some("a/b c")), "/bucket/a/b%20c");
        assert_eq!(
            canonical_query(&[("b".into(), "x y".into()), ("a".into(), "".into())]),
            "a=&b=x%20y"
        );
        assert_eq!(normalize_header_value(" a\t b \n c "), "a b c");
    }
    #[test]
    fn paging_batches_and_keeps_sibling_prefixes() {
        let fixture = Fixture {
            responses: RefCell::new(VecDeque::from([
                response(
                    "<ListBucketResult><Contents><Key>a/b/one</Key></Contents><Contents><Key>a/bc/keep</Key></Contents><IsTruncated>true</IsTruncated><NextContinuationToken>next</NextContinuationToken></ListBucketResult>",
                ),
                response(
                    "<ListBucketResult><Contents><Key>a/b/two</Key></Contents><IsTruncated>false</IsTruncated></ListBucketResult>",
                ),
                response("<DeleteResult/>"),
                response(
                    "<ListMultipartUploadsResult><IsTruncated>false</IsTruncated></ListMultipartUploadsResult>",
                ),
            ])),
            requests: RefCell::new(vec![]),
        };
        assert_eq!(
            wipe_prefix(
                &fixture,
                &credentials(),
                "bucket",
                "a/b/",
                "20150830T123600Z"
            )
            .status,
            "ok"
        );
        let requests = fixture.requests.borrow();
        assert_eq!(requests.len(), 4);
        let delete = String::from_utf8_lossy(&requests[2].body);
        assert!(delete.contains("a/b/one") && delete.contains("a/b/two"));
        assert!(!delete.contains("a/bc/keep"));
        assert!(requests[1].url.contains("continuation-token=next"));
    }
    #[test]
    fn batch_delete_error_and_transport_timeout_remain_errors() {
        let failed = Fixture {
            responses: RefCell::new(VecDeque::from([
                response(
                    "<ListBucketResult><Contents><Key>a/b/one</Key></Contents><IsTruncated>false</IsTruncated></ListBucketResult>",
                ),
                response("<DeleteResult><Error><Code>AccessDenied</Code></Error></DeleteResult>"),
            ])),
            requests: RefCell::new(vec![]),
        };
        assert_eq!(
            wipe_prefix(
                &failed,
                &credentials(),
                "bucket",
                "a/b/",
                "20150830T123600Z"
            )
            .reason_code,
            Some("failed".into())
        );
        let timeout = Fixture {
            responses: RefCell::new(VecDeque::from([Err(HttpError::Timeout)])),
            requests: RefCell::new(vec![]),
        };
        assert_eq!(
            wipe_prefix(
                &timeout,
                &credentials(),
                "bucket",
                "a/b/",
                "20150830T123600Z"
            )
            .reason_code,
            Some("timeout".into())
        );
    }
    #[test]
    fn delete_batches_at_the_published_thousand_object_boundary() {
        let mut page = String::from("<ListBucketResult>");
        for index in 0..=DELETE_OBJECT_BATCH_SIZE {
            page.push_str(&format!("<Contents><Key>a/b/{index}</Key></Contents>"));
        }
        page.push_str("<IsTruncated>false</IsTruncated></ListBucketResult>");
        let fixture = Fixture {
            responses: RefCell::new(VecDeque::from([
                response(&page),
                response("<DeleteResult/>"),
                response("<DeleteResult/>"),
                response(
                    "<ListMultipartUploadsResult><IsTruncated>false</IsTruncated></ListMultipartUploadsResult>",
                ),
            ])),
            requests: RefCell::new(vec![]),
        };

        assert_eq!(
            wipe_prefix(
                &fixture,
                &credentials(),
                "bucket",
                "a/b/",
                "20150830T123600Z"
            )
            .status,
            "ok"
        );
        let requests = fixture.requests.borrow();
        let deletes = requests
            .iter()
            .filter(|request| request.method == "POST")
            .collect::<Vec<_>>();
        assert_eq!(deletes.len(), 2);
        assert_eq!(
            String::from_utf8_lossy(&deletes[0].body)
                .matches("<Object><Key>")
                .count(),
            DELETE_OBJECT_BATCH_SIZE
        );
        assert_eq!(
            String::from_utf8_lossy(&deletes[1].body)
                .matches("<Object><Key>")
                .count(),
            1
        );
    }
    #[test]
    fn malformed_list_xml_fails_closed_before_delete() {
        let fixture = Fixture {
            responses: RefCell::new(VecDeque::from([response(
                "<ListBucketResult><Contents><Key>a/b/one</Key></Contents><Contents><IsTruncated>false</IsTruncated></ListBucketResult>",
            )])),
            requests: RefCell::new(vec![]),
        };

        assert_eq!(
            wipe_prefix(
                &fixture,
                &credentials(),
                "bucket",
                "a/b/",
                "20150830T123600Z"
            )
            .reason_code,
            Some("failed".into())
        );
        assert_eq!(fixture.requests.borrow().len(), 1);
    }
    #[test]
    fn debug_redacts_s3_values() {
        let text = format!("{:?}", credentials());
        assert!(!text.contains("ACCESS") && !text.contains("SECRET") && !text.contains("TOKEN"));
    }
}
