# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc
param(
    [Parameter(Mandatory = $true)][string]$LogPath,
    [Parameter(Mandatory = $true)][string]$TestName,
    [Parameter(Mandatory = $true)][int]$TestExitCode,
    [string]$Marker = ''
)

$ErrorActionPreference = 'Stop'
$stream = $null
$reader = $null
try {
    if ($TestExitCode -ne 0) { throw "Cargo test exited $TestExitCode" }
    if ([string]::IsNullOrWhiteSpace($TestName) -or $TestName -match '[\r\n]') {
        throw 'An exact single-line test name is required'
    }
    # Refuse a log still held by a writer. This observes the file only; it does
    # not establish process-tree cleanup or authorize releasing a host fence.
    $stream = [IO.FileStream]::new($LogPath, [IO.FileMode]::Open,
        [IO.FileAccess]::Read, [IO.FileShare]::Read)
    if ($stream.Length -gt 16MB) { throw 'Oversized test log' }
    $reader = [IO.StreamReader]::new($stream, [Text.UTF8Encoding]::new($false, $true), $false)
    $text = $reader.ReadToEnd()
    $named = [regex]::Matches($text, '(?m)^test ([^\r\n]+) \.\.\. (ok|FAILED|ignored)[^\r\n]*\r?$')
    if ($named.Count -ne 1 -or $named[0].Groups[1].Value -cne $TestName -or
        $named[0].Groups[2].Value -cne 'ok' -or
        $named[0].Value.TrimEnd([char]13) -cne ('test ' + $TestName + ' ... ok')) {
        throw 'Expected exactly one named executed pass'
    }
    $summaries = [regex]::Matches($text, '(?m)^test result: [^\r\n]*\r?$')
    $terminal = '^test result: ok\. 1 passed; 0 failed; 0 ignored; 0 measured; [0-9]+ filtered out; finished in [^\r\n]+\r?$'
    if ($summaries.Count -ne 1 -or $summaries[0].Value -cnotmatch $terminal -or
        $text -cnotmatch ('(?m)' + $terminal + '(?:\r?\n)*\z')) {
        throw 'Expected exactly one terminal one-test summary'
    }
    if ($Marker) {
        if ($Marker -notmatch '^[A-Z][A-Z0-9_]*=[^\r\n]+$') { throw 'Invalid receipt marker' }
        $key = [regex]::Escape($Marker.Split('=')[0])
        $markers = [regex]::Matches($text, '(?m)^' + $key + '=[^\r\n]*\r?$')
        if ($markers.Count -ne 1 -or $markers[0].Value.TrimEnd([char]13) -cne $Marker) {
            throw 'Expected exactly one source receipt marker'
        }
    }
    Write-Output ('JOURNAL_WIN_CI_EXACT_TEST=' + $TestName + '/pass')
    exit 0
} catch {
    [Console]::Error.WriteLine('ERROR: exact Windows test result: ' + $_.Exception.Message)
    exit 1
} finally {
    if ($null -ne $reader) { $reader.Dispose() }
    elseif ($null -ne $stream) { $stream.Dispose() }
}
