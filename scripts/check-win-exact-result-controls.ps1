# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc
param([Parameter(Mandatory = $true)][string]$Root)
Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
. (Join-Path $PSScriptRoot '..\core\distribution\rfdetr-windows-capture.ps1')
$parser = Join-Path $PSScriptRoot 'check-win-exact-result.ps1'
if (Test-Path -LiteralPath $Root) { throw 'Fresh control root required' }
New-Item -ItemType Directory -Path $Root -ErrorAction Stop | Out-Null
$utf8 = [Text.UTF8Encoding]::new($false)
$clock = [Diagnostics.Stopwatch]::StartNew()
$captures = [Collections.Generic.List[object]]::new()
$cases = [Collections.Generic.List[object]]::new()
$pending = $true
$failure = $null
$passed = $false
$held = $null
$good = "running 1 test`ntest module::receipt ... ok`n`nsuccesses:`n`n---- module::receipt stdout ----`nJOURNAL_WIN_CONTROL=PASS`n`nsuccesses:`n    module::receipt`n`ntest result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 3 filtered out; finished in 0.01s`n"
function Run-Case([string]$Name, [string]$Text, [int]$CargoExit, [int]$ExpectedExit, [bool]$HoldWriter = $false) {
    $path = Join-Path $Root ($Name + '.input.log')
    [IO.File]::WriteAllText($path, $Text, $utf8)
    $writer = $null
    try {
        if ($HoldWriter) {
            $writer = [IO.FileStream]::new($path, [IO.FileMode]::Open,
                [IO.FileAccess]::Write, [IO.FileShare]::ReadWrite)
        }
        $remaining = [Math]::Min(30000, 300000 - [int]$clock.ElapsedMilliseconds)
        if ($remaining -le 0) { throw 'Control launch budget exhausted' }
        $stdout = Join-Path $Root ($Name + '.stdout')
        $stderr = Join-Path $Root ($Name + '.stderr')
        $arguments = @('-NoLogo', '-NoProfile', '-NonInteractive', '-ExecutionPolicy', 'Bypass', '-File', $parser,
            '-LogPath', $path, '-TestName', 'module::receipt', '-TestExitCode', [string]$CargoExit,
            '-Marker', 'JOURNAL_WIN_CONTROL=PASS')
        $result = Invoke-RfdetrCapture -Command (Join-Path $env:SystemRoot 'System32\WindowsPowerShell\v1.0\powershell.exe') `
            -Arguments $arguments -Cwd $Root -StdoutPath $stdout -StderrPath $stderr -TimeoutMilliseconds $remaining
        $captures.Add($result)
        # Persist the actual child result before reading or hashing its output.
        [IO.File]::WriteAllText((Join-Path $Root ($Name + '.exit.json')),
            ([ordered]@{pid=$result.pid;actual_exit=$result.exit_code;completed=$result.completed;error=$result.error} | ConvertTo-Json), $utf8)
        if (-not $result.completed) { throw 'Unsettled parser capture' }
        # The negative held the writer through the parser's terminal result.
        # Release it before Get-FileHash independently opens the input file.
        if ($null -ne $writer) { $writer.Dispose(); $writer = $null }
        $out = [IO.File]::ReadAllText($stdout)
        $err = [IO.File]::ReadAllText($stderr)
        $ok = $result.exit_code -eq $ExpectedExit
        if ($ExpectedExit -eq 0) {
            $ok = $ok -and $out.TrimEnd([char]13, [char]10) -ceq 'JOURNAL_WIN_CI_EXACT_TEST=module::receipt/pass' -and $err.Length -eq 0
        } else {
            $ok = $ok -and $out.Length -eq 0 -and $err.StartsWith('ERROR: exact Windows test result: ', [StringComparison]::Ordinal)
        }
        $cases.Add([ordered]@{name=$Name;passed=$ok;expected_exit=$ExpectedExit;actual_exit=$result.exit_code;
            input_sha256=(Get-FileHash -LiteralPath $path -Algorithm SHA256).Hash.ToLowerInvariant();
            stdout_sha256=(Get-FileHash -LiteralPath $stdout -Algorithm SHA256).Hash.ToLowerInvariant();
            stderr_sha256=(Get-FileHash -LiteralPath $stderr -Algorithm SHA256).Hash.ToLowerInvariant()})
        if (-not $ok) { throw ('Parser control failed: ' + $Name) }
    } finally {
        if ($null -ne $writer) { $writer.Dispose() }
    }
}
try {
    Run-Case 'positive-lf' $good 0 0
    Run-Case 'positive-crlf' ($good.Replace("`n", "`r`n")) 0 0
    Run-Case 'cargo-nonzero' $good 17 1
    Run-Case 'empty' '' 0 1
    Run-Case 'wrong-name' ($good.Replace('test module::receipt ... ok', 'test module::other ... ok')) 0 1
    Run-Case 'duplicate-pass' ($good.Replace('test module::receipt ... ok', "test module::receipt ... ok`ntest module::receipt ... ok")) 0 1
    Run-Case 'zero-tests' ($good.Replace('1 passed', '0 passed')) 0 1
    Run-Case 'ignored-test' ($good.Replace('0 ignored', '1 ignored')) 0 1
    Run-Case 'failed-test' ($good.Replace('... ok', '... FAILED')) 0 1
    Run-Case 'wrong-marker' ($good.Replace('JOURNAL_WIN_CONTROL=PASS', 'JOURNAL_WIN_CONTROL=FAIL')) 0 1
    Run-Case 'missing-marker' ($good.Replace('JOURNAL_WIN_CONTROL=PASS', '')) 0 1
    Run-Case 'duplicate-marker' ($good.Replace('JOURNAL_WIN_CONTROL=PASS', "JOURNAL_WIN_CONTROL=PASS`nJOURNAL_WIN_CONTROL=PASS")) 0 1
    Run-Case 'duplicate-summary' ($good + "test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 3 filtered out; finished in 0.01s`n") 0 1
    Run-Case 'nonterminal-summary' ($good + "unexpected trailer`n") 0 1
    Run-Case 'held-writer' $good 0 1 $true
    Run-Case 'writer-released' $good 0 0
    $passed = $true
} catch {
    $failure = $_.Exception.ToString()
} finally {
    # A settled logical refusal is distinct from unfinished process/EOF capture.
    $pending = @($captures | Where-Object { -not $_.completed }).Count -ne 0
    $records = @($captures | ForEach-Object {
        [ordered]@{pid=$_.pid;actual_exit=$_.exit_code;completed=$_.completed;error=$_.error}
    })
    [IO.File]::WriteAllText((Join-Path $Root 'report.json'),
        ([ordered]@{passed=$passed;pending=$pending;failure=$failure;cases=@($cases.ToArray());captures=$records} | ConvertTo-Json -Depth 8), $utf8)
}
if (-not $passed -or $pending) { exit 1 }
Write-Output 'JOURNAL_WIN_CI_EXACT_PARSER_CONTROLS=PASS'
exit 0
