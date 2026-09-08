# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

# Operator-run native controls. Requires the shared Windows slot. Uses only a
# fresh fixture directory; finite-lived fixture children have no owner state.
[CmdletBinding()]
param([Parameter(Mandatory = $true)][string]$FixtureRoot)
Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
. (Join-Path $PSScriptRoot 'rfdetr-windows-capture.ps1')
if (Test-Path -LiteralPath $FixtureRoot) { throw 'capture fixture must be fresh' }
New-Item -ItemType Directory -Path $FixtureRoot -ErrorAction Stop | Out-Null
$executable = Join-Path $FixtureRoot 'capture-child.exe'
$source = @'
using System;
using System.Diagnostics;
using System.IO;
using System.Reflection;
using System.Text;
using System.Threading;
public static class CaptureChild {
    public static int Main(string[] values) {
        if (values.Length == 1 && values[0] == "sleep") {
            Thread.Sleep(3000);
            return 9;
        }
        if (values.Length == 1 && values[0] == "held-output") {
            Process.Start(new ProcessStartInfo {
                FileName = Assembly.GetExecutingAssembly().Location,
                Arguments = "sleep", UseShellExecute = false
            }).Dispose();
            return 0;
        }
        if (values.Length > 0 && values[0] == "arguments") {
            foreach (string value in values) {
                byte[] bytes = Encoding.UTF8.GetBytes(value);
                Console.WriteLine(Convert.ToBase64String(bytes));
            }
            return 0;
        }
        byte[] stdout = {0, 1, 13, 10, 10, 255, 128, 65};
        byte[] stderr = {66, 13, 10, 0, 254};
        Console.OpenStandardOutput().Write(stdout, 0, stdout.Length);
        Console.OpenStandardError().Write(stderr, 0, stderr.Length);
        return 17;
    }
}
'@
$reports = [Collections.Generic.List[object]]::new()
$fixtureCaptures = [Collections.Generic.List[object]]::new()
$settlements = [Collections.Generic.List[object]]::new()
$failures = [Collections.Generic.List[string]]::new()
$controlExit = 1
function Capture([string]$Label, [string[]]$Arguments, [int]$Milliseconds) {
    $clock = [Diagnostics.Stopwatch]::StartNew()
    $result = Invoke-RfdetrCapture -Command $executable -Arguments $Arguments -Cwd $FixtureRoot `
        -StdoutPath (Join-Path $FixtureRoot "$Label.stdout") -StderrPath (Join-Path $FixtureRoot "$Label.stderr") `
        -TimeoutMilliseconds $Milliseconds
    $clock.Stop()
    $fixtureCaptures.Add([ordered]@{label=$Label;capture=$result})
    $result | Add-Member -NotePropertyName observed_return_ms -NotePropertyValue $clock.ElapsedMilliseconds
    $reports.Add([ordered]@{label=$Label;started=$result.started;completed=$result.completed;
        exit_code=$result.exit_code;elapsed_ms=$result.elapsed_ms;observed_return_ms=$result.observed_return_ms;error=$result.error})
    return $result
}
function Settle-Fixture($Result) {
    # Retry the retained original Process and its original I/O tasks. The
    # finite fixture exits itself; no PID reopening, taskkill or build Job.
    if (-not $Result.started) { throw 'fixture launch outcome is unknown; preserve evidence for reconciliation' }
    $clock = [Diagnostics.Stopwatch]::StartNew()
    if (-not $Result.process.WaitForExit(10000)) { throw 'finite capture fixture did not terminate' }
    $tasks = [Threading.Tasks.Task]::WhenAll([Threading.Tasks.Task[]]@($Result.stdout_task,$Result.stderr_task))
    $remaining = [Math]::Max(0, 10000 - [int]$clock.ElapsedMilliseconds)
    if (-not $tasks.Wait($remaining)) { throw 'finite capture fixture output did not settle' }
    $Result.stdout_file.Flush($true)
    $Result.stderr_file.Flush($true)
    $Result.stdout_file.Dispose()
    $Result.stderr_file.Dispose()
    $Result.process.Dispose()
}
try {
foreach ($name in @('rfdetr-windows-build.ps1','rfdetr-windows-capture.ps1','rfdetr-windows-capture-tests.ps1')) {
    $tokens = $null
    $parseErrors = $null
    [void][Management.Automation.Language.Parser]::ParseFile((Join-Path $PSScriptRoot $name), [ref]$tokens, [ref]$parseErrors)
    if ($parseErrors.Count -ne 0) { throw ($parseErrors | Out-String) }
}
[IO.File]::WriteAllText((Join-Path $FixtureRoot 'capture-child.cs'), $source, [Text.UTF8Encoding]::new($false))
Add-Type -TypeDefinition $source -OutputAssembly $executable -OutputType ConsoleApplication -ErrorAction Stop
$result = Capture 'bytes-and-exit' @() 10000
if (-not $result.completed -or $result.exit_code -ne 17 -or $result.observed_return_ms -gt 12000) {
    throw 'actual exit 17 or bounded observed return was not retained'
}
if ([Convert]::ToBase64String([IO.File]::ReadAllBytes((Join-Path $FixtureRoot 'bytes-and-exit.stdout'))) -cne 'AAENCgr/gEE=') {
    throw 'stdout original bytes changed'
}
if ([Convert]::ToBase64String([IO.File]::ReadAllBytes((Join-Path $FixtureRoot 'bytes-and-exit.stderr'))) -cne 'Qg0KAP4=') {
    throw 'stderr original bytes changed'
}
$arguments = @('arguments','','two words','C:\end\','quote"inside','slashes\\"quote',('snowman-'+[char]0x2603))
$result = Capture 'arguments' $arguments 10000
if (-not $result.completed -or $result.exit_code -ne 0 -or $result.observed_return_ms -gt 12000) { throw 'argument fixture failed' }
$received = [IO.File]::ReadAllLines((Join-Path $FixtureRoot 'arguments.stdout'))
if ($received.Count -ne $arguments.Count) { throw 'argument count changed' }
for ($ordinal=0; $ordinal -lt $arguments.Count; $ordinal++) {
    if ([Text.Encoding]::UTF8.GetString([Convert]::FromBase64String($received[$ordinal])) -cne $arguments[$ordinal]) {
        throw "argument $ordinal changed"
    }
}
$result = Capture 'native-deadline' @('sleep') 100
    if ($result.completed -or -not $result.started -or $null -ne $result.exit_code -or $result.observed_return_ms -gt 2000) {
        throw 'live-child deadline was misclassified or unbounded'
    }
$result = Capture 'held-output' @('held-output') 1000
    if ($result.completed -or $result.exit_code -ne 0 -or $result.observed_return_ms -gt 2500) {
        throw 'root exit with inherited output was misclassified or unbounded'
    }
    $controlExit = 0
} catch {
    $failures.Add($_.ToString())
    $failures.Add($_.ScriptStackTrace)
} finally {
    # Every prior capture participates, including an unexpected failure during
    # the first bytes/argument assertion. Report writing is never success-only.
    foreach ($entry in $fixtureCaptures) {
        $result = $entry.capture
        if ($result.completed -or -not $result.launch_attempted) { continue }
        try {
            Settle-Fixture $result
            $settlements.Add([ordered]@{label=$entry.label;settled=$true})
        } catch {
            $controlExit=1
            $failures.Add($_.ToString())
            $settlements.Add([ordered]@{label=$entry.label;settled=$false;error=$_.ToString()})
        }
    }
    $report = [ordered]@{exit_code=$controlExit;captures=@($reports.ToArray());settlements=@($settlements.ToArray());
        failures=@($failures.ToArray());scope='native fixture controls; synchronous startup/flush/dispose have no interruptible deadline';
        pending_reconciliation=(@($settlements | Where-Object { -not $_.settled }).Count -ne 0)}
    [IO.File]::WriteAllText((Join-Path $FixtureRoot 'capture-tests.json'),
        (ConvertTo-Json -InputObject $report -Depth 8), [Text.UTF8Encoding]::new($false))
}
if ($controlExit -eq 0) { Write-Output 'RFDETR_CAPTURE_CONTROLS_PASS bytes exit17 arguments native-deadline inherited-output' }
exit $controlExit
