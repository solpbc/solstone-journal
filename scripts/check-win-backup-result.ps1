# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc
param(
    [Parameter(Mandatory = $true)][string]$LogPath,
    [Parameter(Mandatory = $true)]
    [ValidateSet('backup_native_round_trip', 'process::backup_native_job_cleanup')]
    [string]$TestName,
    [Parameter(Mandatory = $true)][int]$TestExitCode
)

$ErrorActionPreference = 'Stop'
try {
    if ($TestExitCode -ne 0) { throw "Cargo test exited $TestExitCode" }
    $log = Get-Item -LiteralPath $LogPath
    if ($log.PSIsContainer -or $log.Length -gt 16MB) { throw 'Invalid or oversized backup test log' }
    $text = [IO.File]::ReadAllText($log.FullName)
    $selected = [regex]::Escape($TestName)
    # The outer --show-output harness prints its result without test-output interleaving.
    if ([regex]::Matches($text, '(?m)^test ' + $selected + ' \.\.\. ok\r?$').Count -lt 1) {
        throw "Missing executed pass for $TestName"
    }
    if ($text -match '(?m)^test result: FAILED\.' -or
        $text -notmatch 'test result: ok\. 1 passed; 0 failed; 0 ignored; 0 measured; [0-9]+ filtered out; finished in [^\r\n]+\r?\n(?:\r?\n)*\z') {
        throw 'Missing terminal one-test successful result or a nested harness failed'
    }
    $prefixNames = @($selected)
    if ($TestName -eq 'backup_native_round_trip') {
        $receipt = 'JOURNAL_WIN_CI_BACKUP_ROUND_TRIP'
        $markers = @(
            'NATIVE_BACKUP_PACKAGE_TOOLS_NO_DOWNLOAD_OK',
            'NATIVE_BACKUP_BYO_RECOVERY_OK',
            'NATIVE_BACKUP_UNSUPPORTED_ROOT_NO_WRITES_OK',
            'NATIVE_BACKUP_REPLACED_ROOT_NO_WRITES_OK',
            'NATIVE_BACKUP_SELECTIVE_REPARSE_NO_WRITES_OK',
            'NATIVE_BACKUP_OFFLOAD_PRUNE_RESTORE_OK',
            'NATIVE_BACKUP_RCLONE_PATH_NUL_OK',
            'NATIVE_BACKUP_RCLONE_TIMEOUT_OK',
            'NATIVE_BACKUP_UNVERIFIED_NO_MARK_OR_REMOVAL_OK',
            'NATIVE_BACKUP_POISONED_MISSING_PAYLOAD_REFUSED_OK'
        )
    } else {
        $receipt = 'JOURNAL_WIN_CI_BACKUP_JOB_CLEANUP'
        $markers = @(
            'NATIVE_BACKUP_JOB_TIMEOUT_ROOT_DESCENDANT_CLEANUP_OK',
            'NATIVE_BACKUP_JOB_PARENT_DEATH_ROOT_DESCENDANT_CLEANUP_OK'
        )
        # The deliberately killed driver/helper may leave an unfinished libtest prefix.
        $prefixNames += [regex]::Escape('process::backup_native_job_driver')
        $prefixNames += [regex]::Escape('process::backup_native_job_helper')
    }
    $prefix = '^(?:test (?:' + ($prefixNames -join '|') + ') \.\.\. )*'
    $lines = @($text -split '\r?\n' | ForEach-Object { [regex]::Replace($_, $prefix, '') })
    foreach ($marker in $markers) {
        if (@($lines | Where-Object { $_ -ceq $marker }).Count -ne 1) {
            throw "Expected exactly one $marker"
        }
    }
    Write-Output ($receipt + '=executed/pass')
    exit 0
} catch {
    [Console]::Error.WriteLine('ERROR: backup native result: ' + $_.Exception.Message)
    exit 1
}
