// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Reference-compatible ticket, attachment, and knowledge-base operations.

use std::fs::{self, File};
use std::io::SeekFrom;
use std::path::Path;

use chrono::Utc;
use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};
use thiserror::Error;

use super::{
    MultipartInput, PortalClient, PortalClientError, PortalResponse, ReadSeek, json_ascii,
};
use crate::{Ledger, OperationError};

const FEEDBACK_SUBJECT: &str = "Feedback";
const CHUNK_SIZE: usize = 1024 * 1024;
const TOMBSTONE_FIELDS: [&str; 5] = [
    "ticket_id",
    "status",
    "closed_at",
    "close_scheduled_at",
    "reason_code",
];
const CONTENT_TYPES: [(&str, &str); 13] = [
    (".png", "image/png"),
    (".jpg", "image/jpeg"),
    (".jpeg", "image/jpeg"),
    (".gif", "image/gif"),
    (".webp", "image/webp"),
    (".svg", "image/svg+xml"),
    (".pdf", "application/pdf"),
    (".txt", "text/plain"),
    (".csv", "text/csv"),
    (".html", "text/html"),
    (".md", "text/markdown"),
    (".xml", "text/xml"),
    (".json", "application/json"),
];

/// Failure from an operation that may include either local ledger or portal work.
#[derive(Debug, Error)]
pub enum PortalOperationError {
    /// A local durable operation transition failed.
    #[error(transparent)]
    Operation(#[from] OperationError),
    /// Establishing or using the portal identity failed.
    #[error(transparent)]
    Portal(#[from] PortalClientError),
}

impl PortalClient {
    /// Maximum attachment size accepted by the reference client.
    pub const MAX_ATTACHMENT_SIZE: u64 = 10 * 1024 * 1024;

    /// Return the suffix-to-content-type table used for attachment detection.
    pub const fn allowed_content_types() -> &'static [(&'static str, &'static str)] {
        &CONTENT_TYPES
    }

    #[allow(
        clippy::too_many_arguments,
        reason = "the public operation mirrors the independently optional reference inputs"
    )]
    pub fn create_ticket(
        &mut self,
        product: &str,
        subject: &str,
        description: &str,
        severity: &str,
        category: Option<&str>,
        user_email: Option<&str>,
        user_context: Option<Value>,
        action_id: &str,
    ) -> Result<Value, PortalOperationError> {
        self.ensure_registered()?;
        let mut body = Map::from_iter([
            ("product".to_owned(), json!(product)),
            ("subject".to_owned(), json!(subject)),
            ("description".to_owned(), json!(description)),
            ("severity".to_owned(), json!(severity)),
        ]);
        if let Some(value) = category {
            body.insert("category".to_owned(), json!(value));
        }
        if let Some(value) = user_email {
            body.insert("user_email".to_owned(), json!(value));
        }
        if let Some(value) = user_context {
            body.insert("user_context".to_owned(), value);
        }
        let mut fields = body.clone();
        fields.insert("anonymous".to_owned(), json!(self.anonymous));
        self.dispatch_mutation(
            "POST",
            "/api/tickets",
            action_id,
            "create",
            &fields,
            0,
            Some(Value::Object(body)),
            None,
            true,
        )
    }

    pub fn submit_feedback(
        &mut self,
        body: &str,
        product: &str,
        user_email: Option<&str>,
        user_context: Option<Value>,
        action_id: &str,
    ) -> Result<Value, PortalOperationError> {
        self.ensure_registered()?;
        let mut payload = Map::from_iter([
            ("product".to_owned(), json!(product)),
            ("subject".to_owned(), json!(FEEDBACK_SUBJECT)),
            ("description".to_owned(), json!(body)),
            ("severity".to_owned(), json!("low")),
            ("category".to_owned(), json!("feedback")),
        ]);
        let mut fields = Map::from_iter([
            ("product".to_owned(), json!(product)),
            ("body".to_owned(), json!(body)),
            ("anonymous".to_owned(), json!(self.anonymous)),
        ]);
        if let Some(value) = user_email {
            payload.insert("user_email".to_owned(), json!(value));
            fields.insert("user_email".to_owned(), json!(value));
        }
        if let Some(value) = user_context {
            payload.insert("user_context".to_owned(), value.clone());
            fields.insert("user_context".to_owned(), value);
        }
        self.dispatch_mutation(
            "POST",
            "/api/tickets",
            action_id,
            "feedback",
            &fields,
            0,
            Some(Value::Object(payload)),
            None,
            true,
        )
    }

    pub fn reply_to_ticket(
        &mut self,
        ticket_id: i64,
        content: &str,
        action_id: &str,
    ) -> Result<Value, PortalOperationError> {
        self.ensure_registered()?;
        let fields = Map::from_iter([
            ("ticket_id".to_owned(), json!(ticket_id)),
            ("content".to_owned(), json!(content)),
        ]);
        self.dispatch_mutation(
            "POST",
            &format!("/api/tickets/{ticket_id}/messages"),
            action_id,
            "reply",
            &fields,
            0,
            Some(json!({"content": content})),
            None,
            true,
        )
    }

    pub fn attach_file(
        &mut self,
        ticket_id: i64,
        file_path: &Path,
        action_id: &str,
        index: u64,
        filename: Option<&str>,
        content_type: Option<&str>,
    ) -> Result<Value, PortalOperationError> {
        self.ensure_registered()?;
        if !file_path.is_file() {
            return Err(PortalClientError::Storage {
                message: format!("File not found: {}", file_path.display()),
            }
            .into());
        }
        let size = fs::metadata(file_path)
            .map_err(|error| PortalClientError::Storage {
                message: error.to_string(),
            })?
            .len();
        if size > Self::MAX_ATTACHMENT_SIZE {
            return Err(file_too_large(size).into());
        }
        let content_type = match content_type {
            Some(value) => value.to_owned(),
            None => content_type_for(file_path)?,
        };
        let filename = filename
            .unwrap_or_else(|| {
                file_path
                    .file_name()
                    .and_then(|name| name.to_str())
                    .unwrap_or_default()
            })
            .to_owned();
        let mut file = File::open(file_path).map_err(|error| PortalClientError::Storage {
            message: error.to_string(),
        })?;
        let (byte_size, content_sha256) = chunked_hash_and_rewind(&mut file)?;
        let fields = Map::from_iter([
            ("ticket_id".to_owned(), json!(ticket_id)),
            ("filename".to_owned(), json!(filename)),
            ("content_type".to_owned(), json!(content_type)),
            ("byte_size".to_owned(), json!(byte_size)),
            ("content_sha256".to_owned(), json!(content_sha256)),
        ]);
        let mut files = [MultipartInput {
            name: "file".to_owned(),
            filename,
            content_type: Some(content_type),
            reader: &mut file,
        }];
        self.dispatch_mutation(
            "POST",
            &format!("/api/tickets/{ticket_id}/attachments"),
            action_id,
            "attach",
            &fields,
            index,
            None,
            Some(&mut files),
            true,
        )
    }

    pub fn close_ticket(
        &mut self,
        ticket_id: i64,
        action_id: &str,
    ) -> Result<Value, PortalOperationError> {
        self.ensure_registered()?;
        let fields = Map::from_iter([("ticket_id".to_owned(), json!(ticket_id))]);
        Ok(project_tombstone(self.dispatch_mutation(
            "POST",
            &format!("/api/tickets/{ticket_id}/close"),
            action_id,
            "close",
            &fields,
            0,
            None,
            None,
            false,
        )?))
    }

    pub fn confirm_resolution(
        &mut self,
        ticket_id: i64,
        action_id: &str,
    ) -> Result<Value, PortalOperationError> {
        self.ensure_registered()?;
        let fields = Map::from_iter([("ticket_id".to_owned(), json!(ticket_id))]);
        Ok(project_tombstone(self.dispatch_mutation(
            "POST",
            &format!("/api/tickets/{ticket_id}/resolution/confirm"),
            action_id,
            "resolved",
            &fields,
            0,
            None,
            None,
            false,
        )?))
    }

    pub fn still_need_help(
        &mut self,
        ticket_id: i64,
        action_id: &str,
    ) -> Result<Value, PortalOperationError> {
        self.ensure_registered()?;
        let fields = Map::from_iter([("ticket_id".to_owned(), json!(ticket_id))]);
        self.dispatch_mutation(
            "POST",
            &format!("/api/tickets/{ticket_id}/resolution/still-need-help"),
            action_id,
            "still_need_help",
            &fields,
            0,
            None,
            None,
            false,
        )
    }

    pub fn list_tickets(
        &mut self,
        status: Option<&str>,
        product: Option<&str>,
        severity: Option<&str>,
    ) -> Result<Value, PortalOperationError> {
        self.ensure_registered()?;
        let mut params = Vec::new();
        if let Some(value) = status.filter(|value| !value.is_empty()) {
            params.push(("status".to_owned(), value.to_owned()));
        }
        if let Some(value) = product.filter(|value| !value.is_empty()) {
            params.push(("product".to_owned(), value.to_owned()));
        }
        if let Some(value) = severity.filter(|value| !value.is_empty()) {
            params.push(("severity".to_owned(), value.to_owned()));
        }
        let response =
            self.authed_request("GET", "/api/tickets", None, Some(&params), None, None)?;
        self.raise_operation_status("GET", "/api/tickets", &response)?;
        let data = parse_json(&response.body)?;
        Ok(match data {
            Value::Array(items) => Value::Array(
                items
                    .into_iter()
                    .map(|item| match item {
                        Value::Object(_) => project_ticket(item),
                        other => other,
                    })
                    .collect(),
            ),
            other => other,
        })
    }

    pub fn get_ticket(&mut self, ticket_id: i64) -> Result<Value, PortalOperationError> {
        self.ensure_registered()?;
        let path = format!("/api/tickets/{ticket_id}");
        let response = self.authed_request("GET", &path, None, None, None, None)?;
        self.raise_operation_status("GET", &path, &response)?;
        Ok(project_ticket(parse_json(&response.body)?))
    }

    pub fn list_closed_history(
        &mut self,
        cursor: Option<&str>,
    ) -> Result<Value, PortalOperationError> {
        self.ensure_registered()?;
        let params = cursor.map(|value| vec![("cursor".to_owned(), value.to_owned())]);
        let response = self.authed_request(
            "GET",
            "/api/tickets/closed",
            None,
            params.as_deref(),
            None,
            None,
        )?;
        self.raise_operation_status("GET", "/api/tickets/closed", &response)?;
        let data = object_json(&response.body, "closed history response must be an object")?;
        let tickets = data
            .get("tickets")
            .ok_or_else(|| state_error("closed history response has no tickets"))?;
        let next_cursor = data
            .get("next_cursor")
            .ok_or_else(|| state_error("closed history response has no next_cursor"))?;
        let tickets = tickets
            .as_array()
            .ok_or_else(|| state_error("closed history tickets are not an array"))?;
        Ok(
            json!({"tickets": tickets.iter().cloned().map(project_tombstone).collect::<Vec<_>>(), "next_cursor": next_cursor}),
        )
    }

    pub fn search_articles(&mut self, query: Option<&str>) -> Result<Value, PortalOperationError> {
        self.read_value(
            "/api/articles",
            query
                .filter(|value| !value.is_empty())
                .map(|value| vec![("q".to_owned(), value.to_owned())]),
        )
    }
    pub fn get_article(&mut self, slug: &str) -> Result<Value, PortalOperationError> {
        self.read_value(&format!("/api/articles/{slug}"), None)
    }
    pub fn list_announcements(&mut self) -> Result<Value, PortalOperationError> {
        self.read_value("/api/announcements", None)
    }

    pub fn acknowledge_operation(
        &mut self,
        remote_operation_id: &str,
    ) -> Result<bool, PortalOperationError> {
        self.ensure_registered()?;
        let body = json_ascii(&json!({"operation_id": remote_operation_id}))
            .map_err(|error| state_error(error.to_string()))?;
        let response = self.authed_request(
            "POST",
            "/api/idempotency/ack",
            Some(&body),
            None,
            None,
            None,
        )?;
        if (200..300).contains(&response.status) {
            if !response.body.is_empty() {
                let _ = object_json(
                    &response.body,
                    "support acknowledgement response must be an object",
                )?;
            }
            return Ok(true);
        }
        if response.status == 404 {
            return Ok(false);
        }
        self.raise_operation_status("POST", "/api/idempotency/ack", &response)?;
        unreachable!("raise_for_status returned on an error response")
    }

    pub fn drain_pending_acknowledgements(&mut self) -> Result<(), PortalOperationError> {
        let ledger = Ledger::new(self.storage_dir.clone());
        let records = ledger.list_pending_acknowledgements()?;
        for record in records {
            let Some(remote_operation_id) = record.remote_operation_id.as_deref() else {
                log::warn!(
                    "support acknowledgement missing operation id for {}",
                    record.child_action_id
                );
                continue;
            };
            match self.acknowledge_operation(remote_operation_id) {
                Ok(true) => {
                    let _ = ledger.mark_acknowledged(&record);
                }
                Ok(false) => log::debug!(
                    "support acknowledgement unavailable for {}",
                    record.child_action_id
                ),
                Err(error) => log::warn!(
                    "support acknowledgement failed for {}: {error}",
                    record.child_action_id
                ),
            }
        }
        Ok(())
    }

    fn read_value(
        &mut self,
        path: &str,
        params: Option<Vec<(String, String)>>,
    ) -> Result<Value, PortalOperationError> {
        self.ensure_registered()?;
        let response = self.authed_request("GET", path, None, params.as_deref(), None, None)?;
        self.raise_operation_status("GET", path, &response)?;
        Ok(parse_json(&response.body)?)
    }

    #[allow(
        clippy::too_many_arguments,
        reason = "the private dispatcher mirrors the public operation's independently optional inputs"
    )]
    fn dispatch_mutation(
        &mut self,
        method: &str,
        path: &str,
        action_id: &str,
        verb: &str,
        fields: &Map<String, Value>,
        index: u64,
        json_body: Option<Value>,
        files: Option<&mut [MultipartInput<'_>]>,
        project_ticket_result: bool,
    ) -> Result<Value, PortalOperationError> {
        let ledger = Ledger::new(self.storage_dir.clone());
        let now = Utc::now();
        let principal = self.principal()?;
        let mut record = ledger.begin_operation(action_id, verb, fields, &principal, index, now)?;
        if record.state == "pending" {
            record = ledger.mark_in_progress(&record, now)?;
        }
        let encoded = json_body
            .map(|body| json_ascii(&body).map_err(|error| state_error(error.to_string())))
            .transpose()?;
        let response = match self.authed_request(
            method,
            path,
            encoded.as_deref(),
            None,
            files,
            record.operation_key.as_deref(),
        ) {
            Ok(response) => response,
            Err(error @ PortalClientError::Transport { .. }) => {
                ledger.release_retryable_lease(&record, Utc::now())?;
                return Err(error.into());
            }
            Err(error) => return Err(error.into()),
        };
        if (200..300).contains(&response.status) {
            let data = match parse_json(&response.body) {
                Ok(Value::Object(value)) => Value::Object(value),
                Ok(_) => {
                    ledger.release_retryable_lease(&record, Utc::now())?;
                    return Err(
                        state_error("support portal mutation response must be an object").into(),
                    );
                }
                Err(error) => {
                    ledger.release_retryable_lease(&record, Utc::now())?;
                    return Err(error.into());
                }
            };
            ledger.mark_completed(&record, remote_operation_id(&data).as_deref(), Utc::now())?;
            return Ok(if project_ticket_result {
                project_ticket(data)
            } else {
                data
            });
        }
        if response.status >= 500 {
            ledger.release_retryable_lease(&record, Utc::now())?;
            self.raise_operation_status(method, path, &response)?;
            unreachable!();
        }
        let error = response_object(&response)
            .and_then(|body| body.get("error").and_then(Value::as_str).map(str::to_owned));
        match (response.status, error.as_deref()) {
            (409, Some("operation_in_progress")) => Err(OperationError::OperationInProgress.into()),
            (409, Some("idempotency_conflict")) => {
                ledger.mark_failed(&record, "idempotency_conflict", Utc::now())?;
                Err(OperationError::IdempotencyConflict.into())
            }
            (409, Some("invalid_state")) => {
                ledger.mark_failed(&record, "invalid_state", Utc::now())?;
                Err(OperationError::OperationInvalidState.into())
            }
            (410, Some("operation_retired")) => {
                ledger.mark_failed(&record, "operation_retired", Utc::now())?;
                Err(OperationError::OperationRetired.into())
            }
            (_, Some("operation_erased")) => {
                ledger.mark_failed(&record, "operation_erased", Utc::now())?;
                Err(OperationError::OperationErased.into())
            }
            (401, Some("tos_changed")) => {
                ledger.mark_failed(&record, "tos_changed", Utc::now())?;
                Err(OperationError::OperationTosChanged.into())
            }
            _ => {
                ledger.release_retryable_lease(&record, Utc::now())?;
                self.raise_operation_status(method, path, &response)?;
                unreachable!()
            }
        }
    }

    fn raise_operation_status(
        &self,
        method: &str,
        path: &str,
        response: &PortalResponse,
    ) -> Result<(), PortalClientError> {
        self.raise_for_status(method, &format!("{}{}", self.portal_url, path), response)
    }
}

fn parse_json(body: &str) -> Result<Value, PortalClientError> {
    serde_json::from_str(body).map_err(|error| state_error(error.to_string()))
}
fn object_json(body: &str, message: &str) -> Result<Map<String, Value>, PortalClientError> {
    match parse_json(body)? {
        Value::Object(object) => Ok(object),
        _ => Err(state_error(message)),
    }
}
fn response_object(response: &PortalResponse) -> Option<Map<String, Value>> {
    serde_json::from_str::<Value>(&response.body)
        .ok()
        .and_then(|value| value.as_object().cloned())
}
fn state_error(message: impl Into<String>) -> PortalClientError {
    PortalClientError::State {
        message: message.into(),
    }
}
fn remote_operation_id(data: &Value) -> Option<String> {
    let object = data.as_object()?;
    ["ticket_id", "message_id", "attachment_id", "id"]
        .into_iter()
        .find_map(|field| {
            object
                .get(field)
                .filter(|value| !value.is_null())
                .map(|value| match value {
                    Value::String(text) => text.clone(),
                    other => other.to_string(),
                })
        })
}
fn project_tombstone(data: Value) -> Value {
    let Some(object) = data.as_object() else {
        return data;
    };
    Value::Object(
        TOMBSTONE_FIELDS
            .into_iter()
            .filter_map(|field| {
                object
                    .get(field)
                    .cloned()
                    .map(|value| (field.to_owned(), value))
            })
            .collect(),
    )
}
fn project_ticket(data: Value) -> Value {
    matches!(
        data.get("status").and_then(Value::as_str),
        Some("closed" | "retired" | "erased")
    )
    .then(|| project_tombstone(data.clone()))
    .unwrap_or(data)
}
fn content_type_for(path: &Path) -> Result<String, PortalClientError> {
    let suffix = path
        .extension()
        .and_then(|suffix| suffix.to_str())
        .map(|suffix| format!(".{}", suffix.to_lowercase()))
        .unwrap_or_default();
    CONTENT_TYPES
        .iter()
        .find(|(extension, _)| *extension == suffix)
        .map(|(_, content_type)| (*content_type).to_owned())
        .ok_or_else(|| {
            let mut allowed = CONTENT_TYPES
                .iter()
                .map(|(extension, _)| *extension)
                .collect::<Vec<_>>();
            allowed.sort_unstable();
            state_error(format!(
                "Unsupported file type: {suffix}. Allowed: {}",
                allowed.join(", ")
            ))
        })
}
fn file_too_large(size: u64) -> PortalClientError {
    state_error(format!(
        "File too large: {:.1} MB (max {:.0} MB)",
        size as f64 / 1024.0 / 1024.0,
        PortalClient::MAX_ATTACHMENT_SIZE as f64 / 1024.0 / 1024.0
    ))
}

/// Adapt a seekable input to the reference's bounded 1 MiB snapshot hash operation.
pub(crate) fn chunked_hash_and_rewind(
    reader: &mut dyn ReadSeek,
) -> Result<(u64, String), PortalClientError> {
    let mut digest = Sha256::new();
    let mut total = 0_u64;
    let mut buffer = vec![0_u8; CHUNK_SIZE];
    loop {
        let read = reader
            .read(&mut buffer)
            .map_err(|error| PortalClientError::Storage {
                message: error.to_string(),
            })?;
        if read == 0 {
            break;
        }
        total += u64::try_from(read).expect("usize fits u64");
        if total > PortalClient::MAX_ATTACHMENT_SIZE {
            return Err(file_too_large(total));
        }
        digest.update(&buffer[..read]);
    }
    reader
        .seek(SeekFrom::Start(0))
        .map_err(|error| PortalClientError::Storage {
            message: error.to_string(),
        })?;
    let digest = digest.finalize();
    Ok((
        total,
        digest.iter().map(|byte| format!("{byte:02x}")).collect(),
    ))
}

#[cfg(test)]
mod tests {
    use std::collections::VecDeque;
    use std::fs;
    use std::io::{Cursor, Read, Seek, SeekFrom};
    use std::path::Path;
    use std::sync::{Arc, Mutex};

    use tempfile::TempDir;

    use super::*;
    use crate::client::{
        PortalResponse as CratePortalResponse, PortalRuntime, PortalTransport, RequestBody,
    };
    use crate::test_support::{PortalResponse, StubTransport};

    const FIXED_TEST_KEYPAIR_PEM: &[u8] =
        include_bytes!("../../../fixtures/support_portal_golden_nonproduction/keypair.pem");

    struct TestRuntime;
    impl PortalRuntime for TestRuntime {
        fn now(&mut self) -> i64 {
            1_767_225_600
        }
        fn uuid(&mut self) -> String {
            "00000000-0000-4000-8000-000000000000".to_owned()
        }
        fn random_bytes(&mut self, bytes: &mut [u8]) -> Result<(), PortalClientError> {
            bytes.copy_from_slice(&[0, 1, 2, 3]);
            Ok(())
        }
        fn keypair_pem(&mut self) -> Option<Vec<u8>> {
            Some(FIXED_TEST_KEYPAIR_PEM.to_vec())
        }
    }

    fn reply(status: u16, body: &str) -> PortalResponse {
        PortalResponse {
            status,
            body: body.to_owned(),
        }
    }
    fn client(dir: &Path, mut responses: Vec<PortalResponse>) -> PortalClient {
        let mut startup = vec![
            reply(200, "terms"),
            reply(200, r#"{"access_token":"token"}"#),
        ];
        startup.append(&mut responses);
        let (transport, _) = StubTransport::new("https://portal.example", startup);
        PortalClient::new_with(
            "https://portal.example",
            dir,
            Some("test-abcd".to_owned()),
            false,
            Box::new(transport),
            Box::new(TestRuntime),
        )
        .unwrap()
    }

    fn ticket_fields() -> Map<String, Value> {
        Map::from_iter([("ticket_id".to_owned(), json!(7))])
    }

    fn mutate(client: &mut PortalClient, action_id: &str) -> Result<Value, PortalOperationError> {
        client.close_ticket(7, action_id)
    }

    fn operation_record_path(storage_dir: &Path) -> std::path::PathBuf {
        fs::read_dir(storage_dir.join("operations"))
            .expect("operations directory")
            .map(|entry| entry.expect("operation entry").path())
            .find(|path| {
                path.extension()
                    .is_some_and(|extension| extension == "json")
            })
            .expect("operation record")
    }

    struct SnapshotTransport {
        replies: VecDeque<CratePortalResponse>,
        storage_dir: std::path::PathBuf,
        snapshot: Arc<Mutex<Option<Vec<u8>>>>,
    }

    impl PortalTransport for SnapshotTransport {
        fn request(
            &mut self,
            _method: &str,
            _url: &str,
            _headers: &[(String, String)],
            _body: RequestBody,
        ) -> Result<CratePortalResponse, PortalClientError> {
            if self.replies.len() == 1 {
                *self.snapshot.lock().expect("snapshot lock") = Some(
                    fs::read(operation_record_path(&self.storage_dir))
                        .expect("read in-progress operation record"),
                );
            }
            self.replies
                .pop_front()
                .ok_or_else(|| PortalClientError::Transport {
                    message: "fake has no response".to_owned(),
                })
        }
    }

    #[test]
    fn arm_01_success_completes_and_next_begin_succeeds() {
        let dir = TempDir::new().unwrap();
        let mut client = client(
            dir.path(),
            vec![reply(
                201,
                r#"{"ticket_id":7,"status":"closed","private":true}"#,
            )],
        );
        assert_eq!(
            mutate(&mut client, "success").unwrap(),
            json!({"ticket_id": 7, "status": "closed"})
        );
        let ledger = Ledger::new(dir.path());
        let principal = client.principal().unwrap();
        assert!(
            ledger
                .begin_operation(
                    "success",
                    "close",
                    &ticket_fields(),
                    &principal,
                    0,
                    Utc::now()
                )
                .is_ok()
        );
    }

    #[test]
    fn arm_02_transport_releases_lease_and_next_begin_succeeds() {
        let dir = TempDir::new().unwrap();
        let (transport, _) = StubTransport::new(
            "https://portal.example",
            vec![
                reply(200, "terms"),
                reply(200, r#"{"access_token":"token"}"#),
            ],
        );
        let mut client = PortalClient::new_with(
            "https://portal.example",
            dir.path(),
            Some("test-abcd".to_owned()),
            false,
            Box::new(transport),
            Box::new(TestRuntime),
        )
        .unwrap();
        assert!(matches!(
            mutate(&mut client, "transport"),
            Err(PortalOperationError::Portal(
                PortalClientError::Transport { .. }
            ))
        ));
        let ledger = Ledger::new(dir.path());
        let principal = client.principal().unwrap();
        assert!(
            ledger
                .begin_operation(
                    "transport",
                    "close",
                    &ticket_fields(),
                    &principal,
                    0,
                    Utc::now()
                )
                .is_ok()
        );
    }

    #[test]
    fn arm_03_malformed_success_releases_lease_and_next_begin_succeeds() {
        let dir = TempDir::new().unwrap();
        let mut client = client(dir.path(), vec![reply(200, "not json")]);
        assert!(matches!(
            mutate(&mut client, "malformed"),
            Err(PortalOperationError::Portal(
                PortalClientError::State { .. }
            ))
        ));
        let ledger = Ledger::new(dir.path());
        let principal = client.principal().unwrap();
        assert!(
            ledger
                .begin_operation(
                    "malformed",
                    "close",
                    &ticket_fields(),
                    &principal,
                    0,
                    Utc::now()
                )
                .is_ok()
        );
    }

    #[test]
    fn arm_04_non_object_success_releases_lease_and_next_begin_succeeds() {
        let dir = TempDir::new().unwrap();
        let mut client = client(dir.path(), vec![reply(200, "[]")]);
        assert!(matches!(
            mutate(&mut client, "array"),
            Err(PortalOperationError::Portal(
                PortalClientError::State { .. }
            ))
        ));
        let ledger = Ledger::new(dir.path());
        let principal = client.principal().unwrap();
        assert!(
            ledger
                .begin_operation(
                    "array",
                    "close",
                    &ticket_fields(),
                    &principal,
                    0,
                    Utc::now()
                )
                .is_ok()
        );
    }

    #[test]
    fn arm_05_server_error_releases_lease_and_next_begin_succeeds() {
        let dir = TempDir::new().unwrap();
        let mut client = client(dir.path(), vec![reply(500, "broken")]);
        assert!(matches!(
            mutate(&mut client, "server"),
            Err(PortalOperationError::Portal(
                PortalClientError::HttpStatus { .. }
            ))
        ));
        let ledger = Ledger::new(dir.path());
        let principal = client.principal().unwrap();
        assert!(
            ledger
                .begin_operation(
                    "server",
                    "close",
                    &ticket_fields(),
                    &principal,
                    0,
                    Utc::now()
                )
                .is_ok()
        );
    }

    #[test]
    fn arm_06_in_progress_does_not_touch_ledger_after_marking() {
        let dir = TempDir::new().unwrap();
        let mut client = client(
            dir.path(),
            vec![reply(409, r#"{"error":"operation_in_progress"}"#)],
        );
        assert!(matches!(
            mutate(&mut client, "progress"),
            Err(PortalOperationError::Operation(
                OperationError::OperationInProgress
            ))
        ));
        let ledger = Ledger::new(dir.path());
        let principal = client.principal().unwrap();
        assert!(matches!(
            ledger.begin_operation(
                "progress",
                "close",
                &ticket_fields(),
                &principal,
                0,
                Utc::now()
            ),
            Err(OperationError::OperationInProgress)
        ));
    }

    #[test]
    fn arm_06_preserves_record_content_after_marking_in_progress() {
        let dir = TempDir::new().unwrap();
        let snapshot = Arc::new(Mutex::new(None));
        let transport = SnapshotTransport {
            replies: VecDeque::from([
                CratePortalResponse {
                    status: 200,
                    body: "terms".to_owned(),
                },
                CratePortalResponse {
                    status: 200,
                    body: r#"{"access_token":"token"}"#.to_owned(),
                },
                CratePortalResponse {
                    status: 409,
                    body: r#"{"error":"operation_in_progress"}"#.to_owned(),
                },
            ]),
            storage_dir: dir.path().to_owned(),
            snapshot: snapshot.clone(),
        };
        let mut client = PortalClient::new_with(
            "https://portal.example",
            dir.path(),
            Some("test-abcd".to_owned()),
            false,
            Box::new(transport),
            Box::new(TestRuntime),
        )
        .unwrap();
        // The transport observes the file immediately after dispatch marks it in progress.
        assert!(matches!(
            mutate(&mut client, "content"),
            Err(PortalOperationError::Operation(
                OperationError::OperationInProgress
            ))
        ));
        let before = snapshot
            .lock()
            .expect("snapshot lock")
            .clone()
            .expect("snapshot after mark_in_progress");
        let after = fs::read(operation_record_path(dir.path())).expect("record after dispatch");
        assert_eq!(before, after);
    }

    #[test]
    fn recovered_in_progress_record_skips_the_pending_transition() {
        let dir = TempDir::new().unwrap();
        let mut client = client(
            dir.path(),
            vec![
                reply(500, "temporary failure"),
                reply(201, r#"{"ticket_id":7,"status":"closed"}"#),
            ],
        );
        assert!(matches!(
            mutate(&mut client, "recover"),
            Err(PortalOperationError::Portal(
                PortalClientError::HttpStatus { .. }
            ))
        ));
        let path = operation_record_path(dir.path());
        let released: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        assert_eq!(released["state"], "in_progress");
        assert!(released["lease_expires_at"].is_string());
        assert!(mutate(&mut client, "recover").is_ok());
        let completed: Value = serde_json::from_slice(&fs::read(path).unwrap()).unwrap();
        assert_eq!(completed["state"], "completed");
    }

    #[test]
    fn arm_07_idempotency_conflict_fails_and_next_begin_raises() {
        let dir = TempDir::new().unwrap();
        let mut client = client(
            dir.path(),
            vec![reply(409, r#"{"error":"idempotency_conflict"}"#)],
        );
        assert!(matches!(
            mutate(&mut client, "conflict"),
            Err(PortalOperationError::Operation(
                OperationError::IdempotencyConflict
            ))
        ));
        let ledger = Ledger::new(dir.path());
        let principal = client.principal().unwrap();
        assert!(matches!(
            ledger.begin_operation(
                "conflict",
                "close",
                &ticket_fields(),
                &principal,
                0,
                Utc::now()
            ),
            Err(OperationError::IdempotencyConflict)
        ));
    }

    #[test]
    fn arm_08_invalid_state_fails_and_next_begin_raises() {
        let dir = TempDir::new().unwrap();
        let mut client = client(dir.path(), vec![reply(409, r#"{"error":"invalid_state"}"#)]);
        assert!(matches!(
            mutate(&mut client, "state"),
            Err(PortalOperationError::Operation(
                OperationError::OperationInvalidState
            ))
        ));
        let ledger = Ledger::new(dir.path());
        let principal = client.principal().unwrap();
        assert!(matches!(
            ledger.begin_operation(
                "state",
                "close",
                &ticket_fields(),
                &principal,
                0,
                Utc::now()
            ),
            Err(OperationError::OperationInvalidState)
        ));
    }

    #[test]
    fn arm_09_retired_fails_and_next_begin_raises() {
        let dir = TempDir::new().unwrap();
        let mut client = client(
            dir.path(),
            vec![reply(410, r#"{"error":"operation_retired"}"#)],
        );
        assert!(matches!(
            mutate(&mut client, "retired"),
            Err(PortalOperationError::Operation(
                OperationError::OperationRetired
            ))
        ));
        let ledger = Ledger::new(dir.path());
        let principal = client.principal().unwrap();
        assert!(matches!(
            ledger.begin_operation(
                "retired",
                "close",
                &ticket_fields(),
                &principal,
                0,
                Utc::now()
            ),
            Err(OperationError::OperationRetired)
        ));
    }

    #[test]
    fn arm_10_erased_matches_any_status_before_catch_all() {
        let dir = TempDir::new().unwrap();
        let mut client = client(
            dir.path(),
            vec![reply(400, r#"{"error":"operation_erased"}"#)],
        );
        assert!(matches!(
            mutate(&mut client, "erased"),
            Err(PortalOperationError::Operation(
                OperationError::OperationErased
            ))
        ));
        let ledger = Ledger::new(dir.path());
        let principal = client.principal().unwrap();
        assert!(matches!(
            ledger.begin_operation(
                "erased",
                "close",
                &ticket_fields(),
                &principal,
                0,
                Utc::now()
            ),
            Err(OperationError::OperationErased)
        ));
    }

    #[test]
    fn arm_11_repeated_tos_changed_fails_and_next_begin_raises() {
        let dir = TempDir::new().unwrap();
        let mut client = client(
            dir.path(),
            vec![
                reply(401, r#"{"error":"tos_changed"}"#),
                reply(200, "terms-two"),
                reply(200, r#"{"access_token":"token-two"}"#),
                reply(401, r#"{"error":"tos_changed"}"#),
            ],
        );
        assert!(matches!(
            mutate(&mut client, "tos"),
            Err(PortalOperationError::Operation(
                OperationError::OperationTosChanged
            ))
        ));
        let ledger = Ledger::new(dir.path());
        let principal = client.principal().unwrap();
        assert!(matches!(
            ledger.begin_operation("tos", "close", &ticket_fields(), &principal, 0, Utc::now()),
            Err(OperationError::OperationTosChanged)
        ));
    }

    #[test]
    fn arm_12_catch_all_releases_for_400_404_and_unknown_409() {
        for (name, response) in [
            ("four", reply(400, "bad")),
            ("missing", reply(404, "missing")),
            ("unknown", reply(409, r#"{"error":"other"}"#)),
        ] {
            let dir = TempDir::new().unwrap();
            let mut client = client(dir.path(), vec![response]);
            assert!(matches!(
                mutate(&mut client, name),
                Err(PortalOperationError::Portal(
                    PortalClientError::HttpStatus { .. }
                ))
            ));
            let ledger = Ledger::new(dir.path());
            let principal = client.principal().unwrap();
            assert!(
                ledger
                    .begin_operation(name, "close", &ticket_fields(), &principal, 0, Utc::now())
                    .is_ok()
            );
        }
    }

    #[test]
    fn projections_and_reads_follow_their_distinct_reference_paths() {
        let dir = TempDir::new().unwrap();
        let body = r#"{"ticket_id":7,"status":"closed","closed_at":"now","extra":"kept?"}"#;
        let mut portal = client(dir.path(), vec![reply(200, body), reply(200, body)]);
        assert_eq!(
            portal.close_ticket(7, "close").unwrap(),
            json!({"ticket_id":7,"status":"closed","closed_at":"now"})
        );
        assert_eq!(
            portal.confirm_resolution(7, "confirm").unwrap(),
            json!({"ticket_id":7,"status":"closed","closed_at":"now"})
        );
        let dir = TempDir::new().unwrap();
        let mut portal = client(
            dir.path(),
            vec![reply(
                200,
                r#"{"ticket_id":7,"status":"open","extra":true}"#,
            )],
        );
        assert_eq!(
            portal.still_need_help(7, "need").unwrap(),
            json!({"ticket_id":7,"status":"open","extra":true})
        );
    }

    #[test]
    fn attachment_hash_rewinds_and_enforces_the_streamed_limit() {
        let mut reader = Cursor::new(b"abc".to_vec());
        let (size, hash) = chunked_hash_and_rewind(&mut reader).unwrap();
        assert_eq!(
            (size, hash.as_str()),
            (
                3,
                "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
            )
        );
        assert_eq!(reader.position(), 0);
        let mut reader = ExpandingReader {
            bytes: vec![1; PortalClient::MAX_ATTACHMENT_SIZE as usize + 1],
            position: 0,
        };
        assert!(chunked_hash_and_rewind(&mut reader).is_err());
    }

    struct ExpandingReader {
        bytes: Vec<u8>,
        position: usize,
    }
    impl Read for ExpandingReader {
        fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
            let remaining = &self.bytes[self.position..];
            let count = remaining.len().min(buffer.len());
            buffer[..count].copy_from_slice(&remaining[..count]);
            self.position += count;
            Ok(count)
        }
    }
    impl Seek for ExpandingReader {
        fn seek(&mut self, from: SeekFrom) -> std::io::Result<u64> {
            match from {
                SeekFrom::Start(0) => {
                    self.position = 0;
                    Ok(0)
                }
                _ => Err(std::io::Error::other("unsupported")),
            }
        }
    }

    #[test]
    fn acknowledgement_and_empty_drain_match_the_reference() {
        let dir = TempDir::new().unwrap();
        let mut client = client(dir.path(), vec![reply(404, "")]);
        assert!(!client.acknowledge_operation("remote").unwrap());
        let dir = TempDir::new().unwrap();
        let (transport, log) = StubTransport::new("https://portal.example", Vec::new());
        let mut client = PortalClient::new_with(
            "https://portal.example",
            dir.path(),
            Some("test-abcd".to_owned()),
            false,
            Box::new(transport),
            Box::new(TestRuntime),
        )
        .unwrap();
        client.drain_pending_acknowledgements().unwrap();
        assert!(log.lock().unwrap().is_empty());
        assert!(!dir.path().join("keypair.pem").exists());
    }

    #[test]
    fn remote_operation_id_uses_reference_priority_and_absence() {
        assert_eq!(
            remote_operation_id(
                &json!({"ticket_id": 7, "message_id": 8, "attachment_id": 9, "id": 10})
            ),
            Some("7".to_owned())
        );
        assert_eq!(
            remote_operation_id(&json!({"message_id": 8, "attachment_id": 9, "id": 10})),
            Some("8".to_owned())
        );
        assert_eq!(
            remote_operation_id(&json!({"attachment_id": 9, "id": 10})),
            Some("9".to_owned())
        );
        assert_eq!(
            remote_operation_id(&json!({"id": 10})),
            Some("10".to_owned())
        );
        assert_eq!(
            remote_operation_id(&json!({"ticket_id": null, "id": null})),
            None
        );
    }

    #[test]
    fn ticket_and_collection_reads_apply_only_the_required_projection() {
        let dir = TempDir::new().unwrap();
        let mut portal = client(
            dir.path(),
            vec![
                reply(200, r#"{"ticket_id":7,"status":"closed","extra":true}"#),
                reply(200, r#"{"ticket_id":8,"status":"open","extra":true}"#),
                reply(
                    200,
                    r#"[{"ticket_id":7,"status":"closed","extra":true}, 3]"#,
                ),
                reply(200, r#"[{"title":"article","extra":true}]"#),
                reply(200, r#"{"slug":"article","extra":true}"#),
                reply(200, r#"[{"title":"announcement","extra":true}]"#),
            ],
        );
        assert_eq!(
            portal.get_ticket(7).unwrap(),
            json!({"ticket_id":7,"status":"closed"})
        );
        assert_eq!(
            portal.get_ticket(8).unwrap(),
            json!({"ticket_id":8,"status":"open","extra":true})
        );
        assert_eq!(
            portal.list_tickets(None, None, None).unwrap(),
            json!([{"ticket_id":7,"status":"closed"}, 3])
        );
        assert_eq!(
            portal.search_articles(None).unwrap(),
            json!([{"title":"article","extra":true}])
        );
        assert_eq!(
            portal.get_article("article").unwrap(),
            json!({"slug":"article","extra":true})
        );
        assert_eq!(
            portal.list_announcements().unwrap(),
            json!([{"title":"announcement","extra":true}])
        );
    }

    #[test]
    fn closed_history_requires_both_reference_keys() {
        let dir = TempDir::new().unwrap();
        let mut portal = client(dir.path(), vec![reply(200, r#"{"tickets":[]}"#)]);
        assert!(matches!(
            portal.list_closed_history(None),
            Err(PortalOperationError::Portal(
                PortalClientError::State { .. }
            ))
        ));
    }

    #[test]
    fn every_read_registers_before_its_request() {
        let dir = TempDir::new().unwrap();
        let (transport, log) = StubTransport::new(
            "https://portal.example",
            vec![
                reply(200, "terms"),
                reply(200, r#"{"access_token":"token"}"#),
                reply(200, "[]"),
            ],
        );
        let mut portal = PortalClient::new_with(
            "https://portal.example",
            dir.path(),
            Some("test-abcd".to_owned()),
            false,
            Box::new(transport),
            Box::new(TestRuntime),
        )
        .unwrap();
        assert_eq!(portal.search_articles(None).unwrap(), json!([]));
        let log = log.lock().unwrap();
        assert_eq!(log[0].path, "/tos");
        assert_eq!(log[1].path, "/api/signup");
        assert_eq!(log[2].path, "/api/articles");
    }

    #[test]
    fn attachment_suffix_refusal_lists_the_sorted_allowed_set() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("note.exe");
        fs::write(&path, "x").unwrap();
        let mut portal = client(dir.path(), Vec::new());
        let error = portal
            .attach_file(7, &path, "suffix", 0, None, None)
            .unwrap_err()
            .to_string();
        assert!(error.contains(
            ".csv, .gif, .html, .jpeg, .jpg, .json, .md, .pdf, .png, .svg, .txt, .webp, .xml"
        ));
    }

    #[test]
    fn attachment_stat_limit_rejects_before_an_attachment_request() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("oversize.txt");
        let file = fs::File::create(&path).expect("create sparse attachment");
        file.set_len(PortalClient::MAX_ATTACHMENT_SIZE + 1)
            .expect("set sparse attachment length");
        assert_eq!(
            fs::metadata(&path).expect("attachment metadata").len(),
            PortalClient::MAX_ATTACHMENT_SIZE + 1
        );
        let (transport, log) = StubTransport::new(
            "https://portal.example",
            vec![
                reply(200, "terms"),
                reply(200, r#"{"access_token":"token"}"#),
            ],
        );
        let mut client = PortalClient::new_with(
            "https://portal.example",
            dir.path(),
            Some("test-abcd".to_owned()),
            false,
            Box::new(transport),
            Box::new(TestRuntime),
        )
        .unwrap();
        client.register().expect("pre-register client");
        let requests_before = log.lock().expect("log lock").len();
        assert!(matches!(
            client.attach_file(7, &path, "oversize", 0, None, None),
            Err(PortalOperationError::Portal(
                PortalClientError::State { .. }
            ))
        ));
        let log = log.lock().expect("log lock");
        assert_eq!(log.len(), requests_before);
        assert!(
            log.iter()
                .all(|request| request.path != "/api/tickets/7/attachments")
        );
    }

    #[test]
    fn create_ticket_uses_ticket_projection_not_tombstone_projection() {
        let dir = TempDir::new().unwrap();
        let mut portal = client(
            dir.path(),
            vec![reply(
                201,
                r#"{"ticket_id":7,"status":"open","private":"kept"}"#,
            )],
        );
        assert_eq!(
            portal
                .create_ticket(
                    "solstone",
                    "subject",
                    "description",
                    "medium",
                    None,
                    None,
                    None,
                    "create",
                )
                .unwrap(),
            json!({"ticket_id":7,"status":"open","private":"kept"})
        );
    }

    #[test]
    fn drain_attempts_each_pending_remote_id_and_acknowledges_only_successes() {
        let dir = TempDir::new().unwrap();
        let mut portal = client(
            dir.path(),
            vec![reply(200, ""), reply(404, ""), reply(200, "")],
        );
        let principal = portal.principal().unwrap();
        let ledger = Ledger::new(dir.path());
        for (action_id, remote_operation_id) in
            [("first", "one"), ("second", "two"), ("third", "three")]
        {
            let fields = Map::from_iter([("ticket_id".to_owned(), json!(7))]);
            let record = ledger
                .begin_operation(action_id, "close", &fields, &principal, 0, Utc::now())
                .unwrap();
            let record = ledger.mark_in_progress(&record, Utc::now()).unwrap();
            ledger
                .mark_completed(&record, Some(remote_operation_id), Utc::now())
                .unwrap();
        }
        portal.drain_pending_acknowledgements().unwrap();
        let pending = ledger.list_pending_acknowledgements().unwrap();
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].remote_operation_id.as_deref(), Some("two"));
    }
}
