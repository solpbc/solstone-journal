// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! The Spaces calls the journal makes. As the sharer, on its own PDS: create
//! the space, add the person or group, and write or delete the activity
//! record. As a member: get a delegation token from its own PDS, exchange it
//! at the space authority for a space credential, and list the sharer's
//! records with that credential.
//!
//! Space references and record URIs are kept as the host returns them; the
//! only part read out of one is the authority's DID.

use std::fmt;
use std::time::Duration;

use serde_json::{Value, json};
use solstone_core_oauth::atproto::DpopKeyPair;
use solstone_core_oauth::atproto_client::{
    AtprotoClientError, XrpcAuth, XrpcBody, resolve_did_document, xrpc_call, xrpc_url,
};
use solstone_core_oauth::extract_oauth_error_code;
use solstone_core_oauth::ssrf::{HttpRequest, HttpResponse, OAuthHttpClient};

use crate::credential::{CredentialKey, CredentialKeyError};
use crate::{ACTIVITY_COLLECTION, SPACE_TYPE};

const TIMEOUT: Duration = Duration::from_secs(20);
const MAX_PAGE_BYTES: usize = 4 * 1024 * 1024;
const PAGE_LIMIT: &str = "100";
const MAX_PAGES: usize = 50;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SpaceError {
    Client(AtprotoClientError),
    Rejected { status: u16, error: Option<String> },
    Malformed(&'static str),
    BadSpaceRef,
    Key(CredentialKeyError),
    TooManyPages,
}

impl fmt::Display for SpaceError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Client(err) => err.fmt(formatter),
            Self::Rejected { status, error } => match error {
                Some(code) => write!(formatter, "host refused the call ({status}: {code})"),
                None => write!(formatter, "host refused the call ({status})"),
            },
            Self::Malformed(what) => write!(formatter, "host returned a malformed {what}"),
            Self::BadSpaceRef => formatter.write_str("not a space reference"),
            Self::Key(err) => err.fmt(formatter),
            Self::TooManyPages => formatter.write_str("space listing did not end"),
        }
    }
}

impl std::error::Error for SpaceError {}

impl From<AtprotoClientError> for SpaceError {
    fn from(err: AtprotoClientError) -> Self {
        Self::Client(err)
    }
}

/// The DID of the account that is a space's authority, read out of its
/// reference. Accepts the `at://<did>/…` form and the `at:space/<did>/…`
/// form alike.
pub fn space_authority(space: &str) -> Result<&str, SpaceError> {
    let start = space.find("did:").ok_or(SpaceError::BadSpaceRef)?;
    let rest = &space[start..];
    let end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    let did = &rest[..end];
    if solstone_core_oauth::atproto_client::is_atproto_did(did) {
        Ok(did)
    } else {
        Err(SpaceError::BadSpaceRef)
    }
}

/// A signed-in account on its own PDS.
pub struct Session<'a> {
    pub http: &'a dyn OAuthHttpClient,
    pub did: &'a str,
    pub pds: &'a str,
    pub access_token: &'a str,
    pub dpop_key: &'a DpopKeyPair,
    pub now_unix: u64,
}

impl Session<'_> {
    fn call(&self, nsid: &str, body: XrpcBody<'_>) -> Result<Value, SpaceError> {
        let (response, _) = xrpc_call(
            self.http,
            self.pds,
            nsid,
            body,
            &XrpcAuth {
                access_token: self.access_token,
                dpop_key: self.dpop_key,
                nonce: None,
            },
            self.now_unix,
            TIMEOUT,
            MAX_PAGE_BYTES,
        )?;
        json_body(&response)
    }
}

fn json_body(response: &HttpResponse) -> Result<Value, SpaceError> {
    if response.status != 200 {
        return Err(SpaceError::Rejected {
            status: response.status,
            error: extract_oauth_error_code(&response.body),
        });
    }
    if response.body.is_empty() {
        return Ok(Value::Null);
    }
    serde_json::from_slice(&response.body).map_err(|_| SpaceError::Malformed("response"))
}

fn string_field(value: &Value, field: &'static str) -> Result<String, SpaceError> {
    value
        .get(field)
        .and_then(Value::as_str)
        .filter(|text| !text.is_empty())
        .map(ToOwned::to_owned)
        .ok_or(SpaceError::Malformed(field))
}

/// Create a space with the signed-in account as its authority. Only listed
/// members can read it; its member list is not published.
pub fn create_space(session: &Session<'_>) -> Result<String, SpaceError> {
    let input = json!({
        "spaceType": SPACE_TYPE,
        "policy": {"$type": "com.atproto.memberspace.policy"},
    });
    let output = session.call("com.atproto.space.createSpace", XrpcBody::Procedure(&input))?;
    string_field(&output, "space")
}

/// Give accounts read access to a space (and no write access).
pub fn add_readers(session: &Session<'_>, space: &str, dids: &[&str]) -> Result<(), SpaceError> {
    let members: Vec<Value> = dids
        .iter()
        .map(|did| json!({"did": did, "read": true, "write": false}))
        .collect();
    let input = json!({"space": space, "members": members});
    session.call(
        "com.atproto.memberspace.putMembers",
        XrpcBody::Procedure(&input),
    )?;
    Ok(())
}

/// What a write returned.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WrittenRecord {
    pub uri: String,
    pub cid: String,
}

/// Create or replace the activity record `rkey` in `space`.
pub fn put_activity(
    session: &Session<'_>,
    space: &str,
    rkey: &str,
    record: &Value,
) -> Result<WrittenRecord, SpaceError> {
    let input = json!({
        "space": space,
        "repo": session.did,
        "collection": ACTIVITY_COLLECTION,
        "rkey": rkey,
        "record": record,
    });
    let output = session.call("com.atproto.space.putRecord", XrpcBody::Procedure(&input))?;
    Ok(WrittenRecord {
        uri: string_field(&output, "uri")?,
        cid: string_field(&output, "cid")?,
    })
}

/// Delete the activity record `rkey` from `space`.
pub fn delete_activity(session: &Session<'_>, space: &str, rkey: &str) -> Result<(), SpaceError> {
    let input = json!({
        "space": space,
        "repo": session.did,
        "collection": ACTIVITY_COLLECTION,
        "rkey": rkey,
    });
    session.call(
        "com.atproto.space.deleteRecord",
        XrpcBody::Procedure(&input),
    )?;
    Ok(())
}

/// One activity record as the sharer's host lists it.
#[derive(Debug, Clone, PartialEq)]
pub struct RemoteRecord {
    pub rkey: String,
    pub uri: String,
    pub cid: String,
    pub value: Value,
}

/// Read every activity record the space's authority has written in it, as a
/// member of the space.
pub fn list_shared_activities(
    session: &Session<'_>,
    space: &str,
    plc_directory: &str,
) -> Result<Vec<RemoteRecord>, SpaceError> {
    let token_output = session.call(
        "com.atproto.space.getDelegationToken",
        XrpcBody::Query(&[("space", space)]),
    )?;
    let delegation_token = string_field(&token_output, "token")?;

    let authority = space_authority(space)?;
    let authority_pds = resolve_did_document(session.http, authority, plc_directory)?
        .pds
        .ok_or(SpaceError::Client(AtprotoClientError::NoPds))?;
    let key = CredentialKey::generate().map_err(SpaceError::Key)?;
    let authorize = json!({
        "space": space,
        "credentialKey": key.did_key(),
        "delegationToken": delegation_token,
    });
    let request = HttpRequest::post(
        xrpc_url(&authority_pds, "com.atproto.space.authorizeSpaceSync", &[]),
        "application/json",
        serde_json::to_vec(&authorize).map_err(|_| SpaceError::Malformed("request"))?,
    )
    .with_timeout(TIMEOUT);
    let response = session
        .http
        .send(&request)
        .map_err(|err| SpaceError::Client(AtprotoClientError::Fetch(err)))?;
    let credential = string_field(&json_body(&response)?, "credential")?;
    let headers = key
        .signed_headers(&credential, authority)
        .map_err(SpaceError::Key)?;

    let mut records = Vec::new();
    let mut cursor: Option<String> = None;
    for _ in 0..MAX_PAGES {
        let mut params = vec![
            ("space", space),
            ("repo", authority),
            ("collection", ACTIVITY_COLLECTION),
            ("limit", PAGE_LIMIT),
        ];
        if let Some(cursor) = cursor.as_deref() {
            params.push(("cursor", cursor));
        }
        let mut request = HttpRequest::get(xrpc_url(
            &authority_pds,
            "com.atproto.space.listRecords",
            &params,
        ))
        .with_timeout(TIMEOUT)
        .with_max_body_bytes(MAX_PAGE_BYTES);
        request.headers.extend(headers.iter().cloned());
        let response = session
            .http
            .send(&request)
            .map_err(|err| SpaceError::Client(AtprotoClientError::Fetch(err)))?;
        let page = json_body(&response)?;
        let items = page
            .get("records")
            .and_then(Value::as_array)
            .ok_or(SpaceError::Malformed("records"))?;
        for item in items {
            let uri = string_field(item, "uri")?;
            let rkey = uri
                .rsplit('/')
                .next()
                .filter(|rkey| !rkey.is_empty())
                .ok_or(SpaceError::Malformed("uri"))?
                .to_owned();
            records.push(RemoteRecord {
                rkey,
                uri,
                cid: string_field(item, "cid")?,
                value: item.get("value").cloned().unwrap_or(Value::Null),
            });
        }
        cursor = page
            .get("cursor")
            .and_then(Value::as_str)
            .filter(|c| !c.is_empty() && !items.is_empty())
            .map(ToOwned::to_owned);
        if cursor.is_none() {
            return Ok(records);
        }
    }
    Err(SpaceError::TooManyPages)
}

#[cfg(test)]
mod tests {
    use std::collections::VecDeque;
    use std::sync::Mutex;

    use solstone_core_oauth::ssrf::SsrfFetchError;

    use super::*;

    const ALICE: &str = "did:plc:aliceaaaaaaaaaaaaaaaaaaa";
    const BOB: &str = "did:plc:bobbbbbbbbbbbbbbbbbbbbbb";

    struct Routes {
        routes: Mutex<VecDeque<(String, u16, Value)>>,
        requests: Mutex<Vec<HttpRequest>>,
    }

    impl Routes {
        fn new(routes: Vec<(&str, u16, Value)>) -> Self {
            Self {
                routes: Mutex::new(
                    routes
                        .into_iter()
                        .map(|(p, s, b)| (p.to_owned(), s, b))
                        .collect(),
                ),
                requests: Mutex::new(Vec::new()),
            }
        }
    }

    impl OAuthHttpClient for Routes {
        fn send(&self, request: &HttpRequest) -> Result<HttpResponse, SsrfFetchError> {
            self.requests.lock().unwrap().push(request.clone());
            let mut routes = self.routes.lock().unwrap();
            let index = routes
                .iter()
                .position(|(prefix, _, _)| request.url.starts_with(prefix.as_str()))
                .ok_or(SsrfFetchError::Connect)?;
            let (_, status, body) = routes.remove(index).unwrap();
            Ok(HttpResponse {
                status,
                headers: Vec::new(),
                body: serde_json::to_vec(&body).unwrap(),
            })
        }
    }

    #[test]
    fn the_authority_is_read_from_either_reference_form() {
        let old = format!("at://{ALICE}/space/{SPACE_TYPE}/3lbxyz");
        let new = format!("at:space/{ALICE}/{SPACE_TYPE}/3lbxyz");
        assert_eq!(space_authority(&old).unwrap(), ALICE);
        assert_eq!(space_authority(&new).unwrap(), ALICE);
        assert_eq!(
            space_authority("at://example.com/x"),
            Err(SpaceError::BadSpaceRef)
        );
    }

    #[test]
    fn the_sharer_creates_a_members_only_space_and_writes_the_record() {
        let http = Routes::new(vec![
            (
                "https://pds.alice.test/xrpc/com.atproto.space.createSpace",
                200,
                json!({"space": "at://S"}),
            ),
            (
                "https://pds.alice.test/xrpc/com.atproto.memberspace.putMembers",
                200,
                json!({}),
            ),
            (
                "https://pds.alice.test/xrpc/com.atproto.space.putRecord",
                200,
                json!({"uri": "at://S/a/r1", "cid": "bafy1"}),
            ),
            (
                "https://pds.alice.test/xrpc/com.atproto.space.deleteRecord",
                200,
                json!({}),
            ),
        ]);
        let key = DpopKeyPair::generate().unwrap();
        let session = Session {
            http: &http,
            did: ALICE,
            pds: "https://pds.alice.test",
            access_token: "t",
            dpop_key: &key,
            now_unix: 1,
        };
        let space = create_space(&session).unwrap();
        add_readers(&session, &space, &[BOB]).unwrap();
        let written = put_activity(
            &session,
            &space,
            "r1",
            &json!({"$type": ACTIVITY_COLLECTION}),
        )
        .unwrap();
        assert_eq!(written.cid, "bafy1");
        delete_activity(&session, &space, "r1").unwrap();

        let requests = http.requests.lock().unwrap();
        let body = |i: usize| -> Value {
            serde_json::from_slice(requests[i].body.as_ref().unwrap()).unwrap()
        };
        assert_eq!(body(0)["policy"]["$type"], "com.atproto.memberspace.policy");
        assert!(body(0)["policy"].get("publicRead").is_none());
        assert_eq!(
            body(1)["members"][0],
            json!({"did": BOB, "read": true, "write": false})
        );
        assert_eq!(body(2)["repo"], ALICE);
        assert_eq!(body(2)["collection"], ACTIVITY_COLLECTION);
        assert_eq!(body(3)["rkey"], "r1");
    }

    #[test]
    fn a_member_reads_the_sharers_records_with_a_signed_credential() {
        let space = format!("at://{ALICE}/space/{SPACE_TYPE}/s1");
        let http = Routes::new(vec![
            (
                "https://pds.bob.test/xrpc/com.atproto.space.getDelegationToken",
                200,
                json!({"token": "deleg"}),
            ),
            (
                "https://plc.test/did:plc:alice",
                200,
                json!({
                    "id": ALICE,
                    "service": [{"id": "#atproto_pds", "type": "AtprotoPersonalDataServer", "serviceEndpoint": "https://pds.alice.test"}]
                }),
            ),
            (
                "https://pds.alice.test/xrpc/com.atproto.space.authorizeSpaceSync",
                200,
                json!({"credential": "cred"}),
            ),
            (
                "https://pds.alice.test/xrpc/com.atproto.space.listRecords",
                200,
                json!({
                    "cursor": "c1",
                    "records": [{"uri": format!("{space}/{ALICE}/{ACTIVITY_COLLECTION}/r1"), "cid": "bafy1", "value": {"title": "one"}}]
                }),
            ),
            (
                "https://pds.alice.test/xrpc/com.atproto.space.listRecords",
                200,
                json!({"records": []}),
            ),
        ]);
        let key = DpopKeyPair::generate().unwrap();
        let session = Session {
            http: &http,
            did: BOB,
            pds: "https://pds.bob.test",
            access_token: "t",
            dpop_key: &key,
            now_unix: 1,
        };
        let records = list_shared_activities(&session, &space, "https://plc.test").unwrap();
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].rkey, "r1");
        assert_eq!(records[0].value["title"], "one");

        let requests = http.requests.lock().unwrap();
        let authorize: Value = serde_json::from_slice(requests[2].body.as_ref().unwrap()).unwrap();
        assert_eq!(authorize["delegationToken"], "deleg");
        assert!(
            authorize["credentialKey"]
                .as_str()
                .unwrap()
                .starts_with("did:key:zDn")
        );
        assert!(
            !requests[2]
                .headers
                .iter()
                .any(|(k, _)| k == "Authorization"),
            "the authority call carries no OAuth token"
        );
        let list = &requests[3];
        assert!(
            list.headers
                .iter()
                .any(|(k, v)| k == "Authorization" && v == "Atproto-Space-Credential cred")
        );
        assert!(
            list.headers
                .iter()
                .any(|(k, v)| k == "Atproto-Space-Audience" && v == ALICE)
        );
        assert!(!list.headers.iter().any(|(k, _)| k == "DPoP"));
        assert!(requests[4].url.contains("cursor=c1"));
    }
}
