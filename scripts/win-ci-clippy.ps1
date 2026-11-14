# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

# Lints every Windows workspace target with warnings denied, the way
# `make check-rust-clippy-full` lints Linux: once with default features and
# once with every classified package's suite features. Windows compiles
# different `cfg` branches, so code that is clean on Linux can still carry
# Windows-only lint; nothing else in this gate denies it.
#
# Runs inside win-ci.cmd after the CI runner is built, in its MSVC and staged
# FFmpeg toolchain environment. The excluded packages are the ONNX-linked ones
# `make ci` excludes.

param(
    [Parameter(Mandatory = $true)][string]$Runner
)

$ErrorActionPreference = 'Stop'

$excludes = @(
    '--exclude', 'solstone-core-speakers-analyze',
    '--exclude', 'solstone-core-speakers-onnx',
    '--exclude', 'solstone-core-vad-analyze'
)
$base = @('clippy', '--manifest-path', 'core\Cargo.toml', '--workspace') + $excludes + @('--all-targets', '--locked')

$classified = & $Runner classified
if ($LASTEXITCODE -ne 0) {
    Write-Output "ERROR: solstone-ci classified failed (exit $LASTEXITCODE)"
    exit 1
}
$features = @(
    foreach ($line in $classified) {
        $fields = -split $line
        if ($fields.Count -lt 2) { continue }
        foreach ($feature in ($fields[1] -split ',')) {
            if ($feature) { "$($fields[0])/$feature" }
        }
    }
)
if ($features.Count -eq 0) {
    Write-Output 'ERROR: solstone-ci classified listed no suite features'
    exit 1
}

$status = 0
Write-Output '=== cargo clippy --locked (every Windows target, default features) ==='
& cargo @base -- -D warnings
if ($LASTEXITCODE -ne 0) { $status = $LASTEXITCODE }
Write-Output '=== cargo clippy --locked (every Windows target, classified suite features) ==='
& cargo @base --features ($features -join ' ') -- -D warnings
if ($LASTEXITCODE -ne 0 -and $status -eq 0) { $status = $LASTEXITCODE }
if ($status -ne 0) {
    Write-Output "ERROR: Windows clippy found lint (exit $status)"
    exit $status
}
Write-Output 'JOURNAL_WIN_CI_CLIPPY=executed/pass'
exit 0
