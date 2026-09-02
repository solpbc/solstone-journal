// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Describe-owned construction seam over the shared generate session client.

use std::env;
use std::path::{Path, PathBuf};
use std::time::Duration;

use solstone_core_generate::{
    GenerateRequest, SessionClient, SessionCloseError, SessionCompletion, SessionLaunchError,
    SessionReceiveError, SessionSubmitError,
};

pub const WIRE_PATH_ENV: &str = "SOLSTONE_DESCRIBE_GENERATE_WIRE";

pub trait DescribeSession {
    fn submit(&self, request: GenerateRequest) -> Result<(), SessionSubmitError>;
    fn recv_timeout(&self, timeout: Duration) -> Result<SessionCompletion, SessionReceiveError>;
    fn close(&self) -> Result<(), SessionCloseError>;
}

impl DescribeSession for SessionClient {
    fn submit(&self, request: GenerateRequest) -> Result<(), SessionSubmitError> {
        Self::submit(self, request)
    }
    fn recv_timeout(&self, timeout: Duration) -> Result<SessionCompletion, SessionReceiveError> {
        Self::recv_timeout(self, timeout)
    }
    fn close(&self) -> Result<(), SessionCloseError> {
        Self::close(self)
    }
}

pub trait DescribeSessionFactory {
    fn spawn(
        &self,
        max_in_flight: usize,
        explicit_journal: Option<&Path>,
    ) -> Result<Box<dyn DescribeSession>, SessionLaunchError>;
}

pub struct SystemSessionFactory;

impl SystemSessionFactory {
    fn client_at(path: PathBuf, explicit_journal: Option<&Path>) -> SessionClient {
        let client = SessionClient::at_path(path).with_prefix_arguments(["generate".into()]);
        match explicit_journal {
            Some(path) => client.with_session_journal(path.to_path_buf()),
            None => client,
        }
    }

    fn client(explicit_journal: Option<&Path>) -> Result<SessionClient, SessionLaunchError> {
        let path = match env::var_os(WIRE_PATH_ENV) {
            Some(path) => PathBuf::from(path),
            None => SessionClient::sibling_path()?,
        };
        Ok(Self::client_at(path, explicit_journal))
    }
}

impl DescribeSessionFactory for SystemSessionFactory {
    fn spawn(
        &self,
        max_in_flight: usize,
        explicit_journal: Option<&Path>,
    ) -> Result<Box<dyn DescribeSession>, SessionLaunchError> {
        Ok(Box::new(
            Self::client(explicit_journal)?.spawn(max_in_flight)?,
        ))
    }
}

#[cfg(all(test, not(feature = "full-tests")))]
mod tests {
    use super::*;

    fn expected_arguments(explicit_journal: Option<&Path>) -> Vec<std::ffi::OsString> {
        let session = &solstone_core_generate::contract()["framing"]["session"];
        let selector = session["selector"].as_str().unwrap();
        let concurrency_flag = session["concurrency"]["flag"].as_str().unwrap();
        let journal_flag = session["journal"]["flag"].as_str().unwrap();
        let mut arguments = vec![
            "generate".into(),
            selector.into(),
            concurrency_flag.into(),
            "3".into(),
        ];
        if let Some(path) = explicit_journal {
            arguments.push(journal_flag.into());
            arguments.push(path.as_os_str().to_owned());
        }
        arguments
    }

    #[test]
    fn client_at_plans_explicit_journal_without_changing_environment() {
        let journal = Path::new("/journal-a");
        let client = SystemSessionFactory::client_at(
            PathBuf::from("/nonexistent/solstone-core"),
            Some(journal),
        );

        assert_eq!(
            client.session_arguments(3).unwrap(),
            expected_arguments(Some(journal))
        );
    }

    #[test]
    fn client_at_without_explicit_journal_preserves_current_arguments() {
        let client =
            SystemSessionFactory::client_at(PathBuf::from("/nonexistent/solstone-core"), None);

        assert_eq!(
            client.session_arguments(3).unwrap(),
            expected_arguments(None)
        );
    }
}
