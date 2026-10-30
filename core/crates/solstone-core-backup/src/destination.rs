// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use serde_json::{Map, Value};

use crate::{BackupError, Destination};

pub fn assemble_backend_env(destination: &Destination) -> Result<Map<String, Value>, BackupError> {
    let credentials = &destination.credentials;
    match destination.backend.as_str() {
        "s3" => {
            let mut env = Map::new();
            env.insert(
                "AWS_ACCESS_KEY_ID".to_owned(),
                Value::String(required(credentials, "access_key_id")?),
            );
            env.insert(
                "AWS_SECRET_ACCESS_KEY".to_owned(),
                Value::String(required(credentials, "secret_access_key")?),
            );
            #[cfg(target_os = "ios")]
            {
                env.insert(
                    "AWS_DEFAULT_REGION".to_owned(),
                    Value::String(ios_s3_region(&destination.repository, credentials)?),
                );
                // An operated destination's linked transport is create-only.
                if credentials.get("append_only").and_then(Value::as_str) == Some("1") {
                    env.insert(
                        "SOLSTONE_APPEND_ONLY".to_owned(),
                        Value::String("1".to_owned()),
                    );
                }
            }
            if let Some(token) = credentials
                .get("session_token")
                .and_then(Value::as_str)
                .filter(|value| !value.is_empty())
            {
                env.insert(
                    "AWS_SESSION_TOKEN".to_owned(),
                    Value::String(token.to_owned()),
                );
            }
            Ok(env)
        }
        "b2" => Ok(Map::from_iter([
            (
                "B2_ACCOUNT_ID".to_owned(),
                Value::String(required(credentials, "account_id")?),
            ),
            (
                "B2_ACCOUNT_KEY".to_owned(),
                Value::String(required(credentials, "account_key")?),
            ),
        ])),
        value => Err(BackupError::UnsupportedBackend(value.to_owned())),
    }
}

/// Whether an owner's own S3 or B2 destination names its storage service.
///
/// restic reads a repository without a backend prefix as a folder on this
/// computer, relative to the journal's working directory. That folder sits
/// inside the journal, so each backup would copy the repository into itself
/// while the storage credentials went unused. Other backends are not owner
/// destinations and are not judged here.
#[must_use]
pub fn repository_names_backend(destination: &Destination) -> bool {
    match destination.backend.as_str() {
        backend @ ("s3" | "b2") => destination
            .repository
            .strip_prefix(backend)
            .and_then(|rest| rest.strip_prefix(':'))
            .is_some_and(|location| !location.trim().is_empty()),
        _ => true,
    }
}

/// The iOS signer needs the signing region that desktop restic discovers on its own.
/// An owner-supplied region wins; otherwise use the endpoint's `s3.<region>.` or
/// `s3-<region>.` host label, else S3's default. A wrong region fails authentication.
#[cfg(any(target_os = "ios", test))]
fn ios_s3_region(
    repository: &str,
    credentials: &Map<String, Value>,
) -> Result<String, BackupError> {
    let valid = |region: &str| {
        !region.is_empty()
            && region.len() <= 64
            && region
                .bytes()
                .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
    };
    match credentials.get("region") {
        Some(Value::String(region)) if valid(region) => return Ok(region.clone()),
        Some(_) => return Err(BackupError::InvalidDestinationShape),
        None => {}
    }
    let location = repository.strip_prefix("s3:").unwrap_or(repository);
    let location = location
        .strip_prefix("https://")
        .or_else(|| location.strip_prefix("//"))
        .unwrap_or(location);
    let host = location.split('/').next().unwrap_or_default();
    let host = host.rsplit_once(':').map_or(host, |(name, _)| name);
    let labels: Vec<&str> = host.split('.').collect();
    let derived = match labels.as_slice() {
        ["s3", region, _, _, ..] => Some(*region),
        [first, _, _, ..] => first.strip_prefix("s3-"),
        _ => None,
    };
    Ok(derived
        .filter(|region| valid(region))
        .unwrap_or("us-east-1")
        .to_owned())
}

fn required(credentials: &Map<String, Value>, key: &'static str) -> Result<String, BackupError> {
    credentials
        .get(key)
        .and_then(Value::as_str)
        .map(str::to_owned)
        .ok_or(BackupError::MissingCredential(key))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn owner_repositories_must_name_their_storage_service() {
        let destination = |backend: &str, repository: &str| Destination {
            repository: repository.into(),
            backend: backend.into(),
            credentials: Map::new(),
        };
        for (backend, repository) in [
            ("s3", "s3:s3.us-west-004.backblazeb2.com/bucket"),
            ("s3", "s3:https://s3.amazonaws.com/bucket/prefix"),
            ("b2", "b2:bucket:journal"),
            ("rclone", "rclone:spb:bucket/prefix"),
        ] {
            assert!(repository_names_backend(&destination(backend, repository)));
        }
        for (backend, repository) in [
            ("s3", "solstone-backup"),
            ("b2", "solstone-backup"),
            ("s3", "/Users/owner/backup"),
            ("s3", "b2:bucket:journal"),
            ("b2", "s3:host/bucket"),
            ("s3", "s3:"),
            ("b2", "b2: "),
            ("s3", "S3:host/bucket"),
        ] {
            assert!(
                !repository_names_backend(&destination(backend, repository)),
                "{backend} {repository}"
            );
        }
    }

    #[test]
    fn ios_region_prefers_owner_value_then_endpoint_then_default() {
        let explicit = serde_json::from_value(json!({"region":"eu-central-1"})).unwrap();
        assert_eq!(
            ios_s3_region("s3:s3.us-west-004.backblazeb2.com/b/p", &explicit).unwrap(),
            "eu-central-1"
        );
        for region in [
            json!(null),
            json!(""),
            json!("UPPER"),
            json!("a b"),
            json!("a\nb"),
            json!("a".repeat(65)),
        ] {
            let credentials = serde_json::from_value(json!({"region":region})).unwrap();
            assert!(ios_s3_region("s3:s3.us-west-004.backblazeb2.com/b", &credentials).is_err());
        }
        for (repository, region) in [
            (
                "s3:https://s3.us-west-004.backblazeb2.com/bucket/prefix",
                "us-west-004",
            ),
            ("s3:s3.eu-west-2.amazonaws.com/bucket", "eu-west-2"),
            ("s3://s3-ap-south-1.amazonaws.com/bucket", "ap-south-1"),
            (
                "s3:https://s3.us-east-2.wasabisys.com:443/bucket",
                "us-east-2",
            ),
            ("s3:s3.amazonaws.com/bucket", "us-east-1"),
            ("s3:https://minio.home.example:9000/bucket", "us-east-1"),
            ("s3:https://[2001:db8::1]:443/bucket", "us-east-1"),
        ] {
            assert_eq!(
                ios_s3_region(repository, &Map::new()).unwrap(),
                region,
                "{repository}"
            );
        }
    }

    #[test]
    fn s3_projects_exact_required_environment_without_session_token() {
        let s3 = Destination {
            repository: "repo".into(),
            backend: "s3".into(),
            credentials: serde_json::from_value(
                json!({"access_key_id":"a","secret_access_key":"s"}),
            )
            .unwrap(),
        };
        assert_eq!(
            assemble_backend_env(&s3).unwrap(),
            serde_json::from_value(json!({"AWS_ACCESS_KEY_ID":"a","AWS_SECRET_ACCESS_KEY":"s"}))
                .unwrap()
        );
    }

    #[test]
    fn s3_omits_blank_session_token_and_projects_populated_token() {
        for token in [json!(""), json!("token")] {
            let s3 = Destination {
                repository: "repo".into(),
                backend: "s3".into(),
                credentials: serde_json::from_value(
                    json!({"access_key_id":"a","secret_access_key":"s","session_token": token}),
                )
                .unwrap(),
            };
            let env = assemble_backend_env(&s3).unwrap();
            if token == json!("") {
                assert_eq!(env.get("AWS_SESSION_TOKEN"), None);
            } else {
                assert_eq!(
                    env.get("AWS_SESSION_TOKEN"),
                    Some(&Value::String("token".into()))
                );
            }
        }
    }

    #[test]
    fn b2_projects_exact_environment() {
        let b2 = Destination {
            repository: "repo".into(),
            backend: "b2".into(),
            credentials: serde_json::from_value(json!({"account_id":"a","account_key":"k"}))
                .unwrap(),
        };
        assert_eq!(
            assemble_backend_env(&b2).unwrap(),
            serde_json::from_value(json!({"B2_ACCOUNT_ID":"a","B2_ACCOUNT_KEY":"k"})).unwrap()
        );
    }

    #[test]
    fn rejects_missing_and_unknown_backend() {
        let missing = Destination {
            repository: "repo".into(),
            backend: "s3".into(),
            credentials: Map::new(),
        };
        let error = assemble_backend_env(&missing).unwrap_err();
        assert!(matches!(
            error,
            BackupError::MissingCredential("access_key_id")
        ));
        assert!(!error.to_string().contains("secret"));
        let unknown = Destination {
            repository: "repo".into(),
            backend: "wat".into(),
            credentials: Map::new(),
        };
        let error = assemble_backend_env(&unknown).unwrap_err();
        assert!(matches!(error, BackupError::UnsupportedBackend(_)));
        assert!(!error.to_string().contains("wat"));
    }
}
