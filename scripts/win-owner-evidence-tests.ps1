# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

# Synthetic exporter controls only; these never supply ordinary-owner credit.
Set-StrictMode -Version Latest
$ErrorActionPreference='Stop'
$utf8=[Text.UTF8Encoding]::new($false,$true)
$root=Join-Path ([IO.Path]::GetTempPath()) ('journal-owner-export-'+[Guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Path $root | Out-Null
$nonce='synthetic-export-control'
$commit='0123456789012345678901234567890123456789'
$lock='0123456789012345678901234567890123456789012345678901234567890123'
$sid='S-1-5-21-123-456-789-1000'
$count=0
function Run-Control([string]$Name,[scriptblock]$Mutate,[bool]$WantPass) {
    $result=[ordered]@{schema='solstone.journal.win-owner-rail.result.v1';nonce=$nonce;selected_control='ordinary-owner';executed_control='ordinary-owner';payload_sha256=$lock;passed=$true;cargo_exit_code=0;ordinary_owner_marker=$true;ordinary_owner_refs_marker=$true;before=[ordered]@{owner_sid=$sid;session=1;elevated=$false;backup_privilege_present=$false;restore_privilege_present=$false};after=[ordered]@{owner_sid=$sid;session=1;elevated=$false;backup_privilege_present=$false;restore_privilege_present=$false}}
    $lease=[ordered]@{schema='solstone.journal.win-owner-rail.lease.v1';state='terminal-verified';nonce=$nonce;expected_commit=$commit;expected_cargo_lock_sha256=$lock;expected_owner_account='ordinary-owner';expected_owner_sid=$sid;expected_session=1;control_id='ordinary-owner';payload_sha256=$lock;result_path=(Join-Path $root "$Name-result.json")}
    & $Mutate $lease $result
    $leasePath=Join-Path $root "$Name-lease.json";$out=Join-Path $root "$Name-snapshot.json"
    [IO.File]::WriteAllText($lease.result_path,(ConvertTo-Json -InputObject $result -Depth 8),$utf8)
    [IO.File]::WriteAllText($leasePath,(ConvertTo-Json -InputObject $lease -Depth 8),$utf8)
    $passed=$false
    try {
        & (Join-Path $PSScriptRoot 'win-owner-evidence.ps1') -LeasePath $leasePath -OutputPath $out -ExpectedNonce $nonce -ExpectedCommit $commit -ExpectedLock $lock -ExpectedOwnerAccount 'ordinary-owner'
        $passed=$true
    } catch { if ($WantPass) { throw } }
    if ($passed -ne $WantPass) { throw "unexpected exporter outcome: $Name" }
    if ($WantPass) {
        $actual=Get-Content -LiteralPath $out -Raw | ConvertFrom-Json
        if ($actual.owner_sid -cne $sid -or $actual.elevated -isnot [bool] -or $actual.elevated -or
            $actual.ordinary_owner_marker -cne 'JOURNAL_WIN_CI_ORDINARY_OWNER_CONTROL=passed' -or
            $actual.ordinary_owner_refs_marker -cne 'JOURNAL_WIN_CI_ORDINARY_OWNER_REFS=passed') { throw 'snapshot did not adapt the actual owner result' }
    } elseif (Test-Path -LiteralPath $out) { throw "failed export wrote a snapshot: $Name" }
    $script:count++
}
try {
    Run-Control 'valid-before-after-bools' {param($l,$r)} $true
    Run-Control 'old-token-field-only' {param($l,$r) $r.token=$r.after;$r.Remove('after');$r.Remove('before')} $false
    Run-Control 'missing-after' {param($l,$r) $r.Remove('after')} $false
    Run-Control 'null-sid' {param($l,$r) $r.after.owner_sid=$null} $false
    Run-Control 'wrong-sid' {param($l,$r) $r.after.owner_sid='S-1-5-18'} $false
    Run-Control 'before-elevated' {param($l,$r) $r.before.elevated=$true} $false
    Run-Control 'after-elevated' {param($l,$r) $r.after.elevated=$true} $false
    Run-Control 'backup-privilege' {param($l,$r) $r.before.backup_privilege_present=$true} $false
    Run-Control 'string-boolean' {param($l,$r) $r.after.elevated='false'} $false
    Run-Control 'false-marker' {param($l,$r) $r.ordinary_owner_marker=$false} $false
    Run-Control 'false-refs-marker' {param($l,$r) $r.ordinary_owner_refs_marker=$false} $false
    Run-Control 'string-marker' {param($l,$r) $r.ordinary_owner_marker='true'} $false
    Run-Control 'failed-cargo' {param($l,$r) $r.cargo_exit_code=1} $false
    Run-Control 'wrong-nonce' {param($l,$r) $r.nonce='older-run'} $false
    Run-Control 'wrong-source' {param($l,$r) $l.expected_commit=('f'*40)} $false
    Run-Control 'wrong-session' {param($l,$r) $r.after.session=2} $false
    Run-Control 'string-session' {param($l,$r) $r.after.session='1'} $false
    Run-Control 'null-control' {param($l,$r) $l.control_id=$null;$r.selected_control=$null;$r.executed_control=$null} $false
    Run-Control 'unverified-lease' {param($l,$r) $l.state='launched'} $false
    Run-Control 'wrong-payload' {param($l,$r) $r.payload_sha256=('f'*64)} $false
    Write-Output "JOURNAL_WIN_CI_OWNER_SNAPSHOT_CONTROLS=passed count=$count"
} finally { Remove-Item -LiteralPath $root -Recurse -Force }
