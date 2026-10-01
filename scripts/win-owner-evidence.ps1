# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

# Adapt the owner rail's actual before/after attestations and boolean markers
# into the registry gate's snapshot. Run after native `await` and before cleanup.
[CmdletBinding()]
param(
    [Parameter(Mandatory=$true)][string]$LeasePath,
    [Parameter(Mandatory=$true)][string]$OutputPath,
    [Parameter(Mandatory=$true)][string]$ExpectedNonce,
    [Parameter(Mandatory=$true)][ValidatePattern('^[0-9a-f]{40}$')][string]$ExpectedCommit,
    [Parameter(Mandatory=$true)][ValidatePattern('^[0-9a-f]{64}$')][string]$ExpectedLock,
    [Parameter(Mandatory=$true)][string]$ExpectedOwnerAccount
)
Set-StrictMode -Version Latest
$ErrorActionPreference='Stop'
$utf8=[Text.UTF8Encoding]::new($false,$true)
$lease=$utf8.GetString([IO.File]::ReadAllBytes($LeasePath)) | ConvertFrom-Json
$result=$utf8.GetString([IO.File]::ReadAllBytes($lease.result_path)) | ConvertFrom-Json
if ($lease.schema -cne 'solstone.journal.win-owner-rail.lease.v1' -or
    $result.schema -cne 'solstone.journal.win-owner-rail.result.v1' -or
    $lease.state -cne 'terminal-verified' -or
    [string]::IsNullOrWhiteSpace($ExpectedNonce) -or $lease.nonce -cne $ExpectedNonce -or $result.nonce -cne $ExpectedNonce -or
    $lease.expected_commit -cne $ExpectedCommit -or $lease.expected_cargo_lock_sha256 -cne $ExpectedLock -or
    $lease.expected_owner_account -cne $ExpectedOwnerAccount -or
    $lease.control_id -isnot [string] -or [string]::IsNullOrWhiteSpace($lease.control_id) -or
    $lease.payload_sha256 -cnotmatch '^[0-9a-f]{64}$' -or
    $result.selected_control -cne $lease.control_id -or $result.executed_control -cne $lease.control_id -or
    $result.payload_sha256 -cne $lease.payload_sha256) { throw 'owner result is not bound to this verified lease/source' }
foreach ($attestation in @($result.before,$result.after)) {
    if ($attestation.owner_sid -isnot [string] -or [string]::IsNullOrWhiteSpace($attestation.owner_sid) -or
        $attestation.owner_sid -cne $lease.expected_owner_sid -or $attestation.session -ne $lease.expected_session) { throw 'owner token identity differs' }
    if ($attestation.session -isnot [int] -and $attestation.session -isnot [long]) { throw 'integer owner session required' }
    foreach ($field in @('elevated','backup_privilege_present','restore_privilege_present')) {
        if ($attestation.$field -isnot [bool] -or $attestation.$field) { throw 'ordinary owner token required before and after execution' }
    }
}
foreach ($field in @('passed','ordinary_owner_marker','ordinary_owner_refs_marker')) {
    if ($result.$field -isnot [bool] -or -not $result.$field) { throw "owner result did not pass $field" }
}
if ($result.cargo_exit_code -isnot [int] -and $result.cargo_exit_code -isnot [long]) { throw 'integer cargo exit code required' }
if ($result.cargo_exit_code -ne 0) { throw 'owner cargo command failed' }
$snapshot=[ordered]@{
    schema='solstone.journal.win-owner-rail.evidence-snapshot.v1'
    lease_schema=$lease.schema;result_schema=$result.schema;nonce=$lease.nonce;result_nonce=$result.nonce
    expected_commit=$lease.expected_commit;expected_cargo_lock_sha256=$lease.expected_cargo_lock_sha256
    expected_owner_account=$lease.expected_owner_account;expected_owner_sid=$lease.expected_owner_sid
    passed=$result.passed;cargo_exit_code=$result.cargo_exit_code
    owner_sid=$result.after.owner_sid;elevated=$result.after.elevated
    ordinary_owner_marker='JOURNAL_WIN_CI_ORDINARY_OWNER_CONTROL=passed'
    ordinary_owner_refs_marker='JOURNAL_WIN_CI_ORDINARY_OWNER_REFS=passed'
}
[IO.File]::WriteAllText([IO.Path]::GetFullPath($OutputPath),(ConvertTo-Json -InputObject $snapshot -Depth 5),$utf8)
Write-Output "JOURNAL_WIN_CI_OWNER_SNAPSHOT=passed nonce=$ExpectedNonce"
