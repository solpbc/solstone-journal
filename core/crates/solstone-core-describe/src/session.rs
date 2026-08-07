// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Describe-owned construction seam over the shared generate session client.

use std::env;
use std::path::PathBuf;
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
    fn spawn(&self, max_in_flight: usize) -> Result<Box<dyn DescribeSession>, SessionLaunchError>;
}

pub struct SystemSessionFactory;

impl SystemSessionFactory {
    fn client() -> Result<SessionClient, SessionLaunchError> {
        match env::var_os(WIRE_PATH_ENV) {
            Some(path) => Ok(SessionClient::at_path(PathBuf::from(path))),
            None => SessionClient::sibling(),
        }
    }
}

impl DescribeSessionFactory for SystemSessionFactory {
    fn spawn(&self, max_in_flight: usize) -> Result<Box<dyn DescribeSession>, SessionLaunchError> {
        Ok(Box::new(Self::client()?.spawn(max_in_flight)?))
    }
}
