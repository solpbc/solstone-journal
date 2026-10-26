// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use super::*;
use crate::hosted_runtime::{decode_hosted_credentials, hosted_credentials_request};
use solstone_core_backup::ios_repository as repository;

/// Normal broker credential result, retained only in this invocation. No
/// serialized recovery record can construct it or substitute storage credentials.
pub struct IosOperatedPreparation {
    config_sha256: String,
    destination: repository::Destination,
    credentials: crate::s3_wipe::S3Credentials,
}

impl IosOperatedPreparation {
    pub fn destination(&self) -> &repository::Destination {
        &self.destination
    }
}

impl TeardownOperation {
    pub fn prepare_ios_operated(
        &mut self,
        admission: &CarrierTeardownAdmission,
        reader: &mut TeardownReader<'_>,
        version: &str,
    ) -> Result<IosOperatedPreparation, TeardownRemoteRefusal> {
        if self.report.phase != RemoteTeardownPhase::NotStarted {
            return Err(TeardownRemoteRefusal::AlreadyAttempted);
        }
        self.remaining().map_err(TeardownRemoteRefusal::Operation)?;
        admission
            .check(reader)
            .map_err(TeardownRemoteRefusal::Admission)?;
        let binding = admission
            .configuration()
            .binding()
            .ok_or(TeardownRemoteRefusal::BindingUnavailable)?;
        self.report.phase = RemoteTeardownPhase::Broker;
        self.report.broker_attempted = true;
        let response = self.request(hosted_credentials_request(binding, "maintenance", version))?;
        admission
            .revalidate()
            .map_err(TeardownRemoteRefusal::Admission)?;
        let credentials =
            decode_hosted_credentials(&response).map_err(TeardownRemoteRefusal::Broker)?;
        let expiry = chrono::DateTime::parse_from_rfc3339(&credentials.expires_at)
            .map_err(|_| TeardownRemoteRefusal::Timestamp)?
            .timestamp();
        self.budget.cap_credential_expiry(expiry);
        self.remaining().map_err(TeardownRemoteRefusal::Operation)?;
        let configured = crate::hosted_runtime::operated_destination(binding, &credentials);
        let env = solstone_core_backup::assemble_backend_env(&configured)
            .map_err(|_| TeardownRemoteRefusal::BindingUnavailable)?;
        let env = env
            .into_iter()
            .map(|(k, v)| {
                Ok((
                    k,
                    Some(
                        v.as_str()
                            .ok_or(TeardownRemoteRefusal::BindingUnavailable)?
                            .to_owned(),
                    ),
                ))
            })
            .collect::<Result<std::collections::BTreeMap<_, _>, TeardownRemoteRefusal>>()?;
        let repository::Repository::S3 { destination, .. } =
            repository::parse(&configured.repository, Some(&env))
                .map_err(|_| TeardownRemoteRefusal::BindingUnavailable)?
        else {
            return Err(TeardownRemoteRefusal::BindingUnavailable);
        };
        admission
            .check(reader)
            .map_err(TeardownRemoteRefusal::Admission)?;
        Ok(IosOperatedPreparation {
            config_sha256: admission.configuration().sha256().to_owned(),
            destination,
            credentials: crate::s3_wipe::S3Credentials {
                endpoint: credentials.endpoint,
                access_key_id: credentials.access_key_id,
                secret_access_key: credentials.secret_access_key,
                session_token: credentials.session_token,
                region: "auto".into(),
            },
        })
    }

    pub fn wipe_ios_operated(
        &mut self,
        admission: &CarrierTeardownAdmission,
        reader: &mut TeardownReader<'_>,
        preparation: IosOperatedPreparation,
    ) -> Result<(), TeardownRemoteRefusal> {
        if self.report.phase != RemoteTeardownPhase::Broker
            || preparation.config_sha256 != admission.configuration().sha256()
        {
            return Err(TeardownRemoteRefusal::AlreadyAttempted);
        }
        self.remaining().map_err(TeardownRemoteRefusal::Operation)?;
        admission
            .check(reader)
            .map_err(TeardownRemoteRefusal::Admission)?;
        crate::s3_wipe::owned::wipe(self, admission, reader, &preparation.credentials)?;
        self.report.phase = RemoteTeardownPhase::Wiped;
        Ok(())
    }

    /// The coordinator calls this only after the typed native prune completed
    /// and its exact completed record was locked and re-observed.
    pub fn record_ios_prune_completion(&mut self, admitted_snapshot_count: u32) {
        self.report.phase = RemoteTeardownPhase::SnapshotsPruned;
        self.report.admitted_snapshot_count = Some(admitted_snapshot_count);
    }
}
