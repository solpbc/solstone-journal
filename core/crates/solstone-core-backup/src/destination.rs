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
