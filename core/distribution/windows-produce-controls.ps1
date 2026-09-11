# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

# Native caller fault controls. Parse the complete producer, then execute only
# its exact capture functions and terminal block against private fixture fences.
# No canonical host fence, firewall, Cargo, package or owner state is touched.
[CmdletBinding()]
param([Parameter(Mandatory=$true)][string]$FixtureRoot)
Set-StrictMode -Version Latest
$ErrorActionPreference='Stop'
if (Test-Path -LiteralPath $FixtureRoot) { throw 'fresh fixture root required' }
New-Item -ItemType Directory -Path $FixtureRoot -ErrorAction Stop | Out-Null
$utf8=[Text.UTF8Encoding]::new($false,$true)
$driver=Join-Path $PSScriptRoot 'windows-produce.ps1'
$captureSource=Join-Path $PSScriptRoot 'rfdetr-windows-capture.ps1'
if ((Get-FileHash -LiteralPath $captureSource -Algorithm SHA256).Hash.ToLowerInvariant() -cne 'a4959a5aca1b346a915e87c4019e64169a153f27af2a724012eb01dca13fe605') { throw 'capture helper differs from reviewed bytes' }
. $captureSource
$allCaptures=[Collections.Generic.List[object]]::new()
$caseReports=[Collections.Generic.List[object]]::new()
$predicateReports=[Collections.Generic.List[object]]::new()
$budgetReports=[Collections.Generic.List[object]]::new()
$pathReports=[Collections.Generic.List[object]]::new()
$terminalFailures=[Collections.Generic.List[string]]::new()
$settlements=[Collections.Generic.List[object]]::new()
$fixtureFences=[Collections.Generic.List[object]]::new()
$controlExit=1
$source=@'
using System;
using System.Diagnostics;
using System.Reflection;
using System.Threading;
public static class ProducerControlChild {
    public static int Main(string[] args) {
        if (args[0] == "sleep") { Thread.Sleep(3000); return 9; }
        if (args[0] == "held") {
            Process.Start(new ProcessStartInfo {
                FileName=Assembly.GetExecutingAssembly().Location,
                Arguments="sleep", UseShellExecute=false
            }).Dispose();
            return 0;
        }
        byte[] bytes={0,255,13,10};
        Console.OpenStandardOutput().Write(bytes,0,bytes.Length);
        return args[0] == "exit2" ? 2 : 0;
    }
}
'@
function Settle-Capture($Result) {
    if (-not $Result.started) { throw 'unknown fixture launch; retain fixture for reconciliation' }
    $clock=[Diagnostics.Stopwatch]::StartNew()
    if (-not $Result.process.WaitForExit(10000)) { throw 'finite fixture root did not terminate' }
    $tasks=[Threading.Tasks.Task]::WhenAll([Threading.Tasks.Task[]]@($Result.stdout_task,$Result.stderr_task))
    $remaining=[Math]::Max(0,10000-[int]$clock.ElapsedMilliseconds)
    if (-not $tasks.Wait($remaining)) { throw 'finite fixture streams did not settle' }
    $Result.stdout_file.Flush($true); $Result.stderr_file.Flush($true)
    $Result.stdout_file.Dispose(); $Result.stderr_file.Dispose(); $Result.process.Dispose()
}
try {
    $tokens=$null; $errors=$null
    $ast=[Management.Automation.Language.Parser]::ParseFile($driver,[ref]$tokens,[ref]$errors)
    if ($errors.Count -ne 0) { throw ($errors | Out-String) }
    foreach ($file in @($PSCommandPath,$captureSource)) {
        [void][Management.Automation.Language.Parser]::ParseFile($file,[ref]$tokens,[ref]$errors)
        if ($errors.Count -ne 0) { throw ($errors | Out-String) }
    }
    foreach ($name in @('Require-PlainPath','Write-NewText','Write-NewJson','Digest','Invoke-Native','Test-ExactSingleTest')) {
        $functionNodes=@($ast.FindAll({ param($node) $node -is [Management.Automation.Language.FunctionDefinitionAst] -and $node.Name -ceq $name },$true))
        if ($functionNodes.Count -ne 1) { throw "expected one actual driver function: $name" }
        . ([ScriptBlock]::Create($functionNodes[0].Extent.Text))
    }
    Require-PlainPath 'C:\Program Files (x86)\Microsoft Visual Studio\vcvarsall.bat'
    foreach ($suffix in @('"','%','!','&','|','<','>','^','`','$',"'","`n")) {
        $refused=$false
        try { Require-PlainPath ('C:\bad'+$suffix+'path') } catch { $refused=$true }
        $pathReports.Add([ordered]@{suffix=$suffix;refused=$refused})
        if (-not $refused) { throw 'unsafe batch path admitted' }
    }
    $quotedRoot=Join-Path $FixtureRoot 'Program Files (x86)'
    New-Item -ItemType Directory -Path $quotedRoot -ErrorAction Stop | Out-Null
    $quotedChild=Join-Path $quotedRoot 'path control.cmd'
    $quotedBatch=Join-Path $FixtureRoot 'quoted-call.cmd'
    Write-NewText $quotedChild "@echo off`r`necho QUOTED_PATH_CONTROL`r`nexit /b 37`r`n"
    Write-NewText $quotedBatch "@echo off`r`ncall `"$quotedChild`" x64`r`nexit /b %errorlevel%`r`n"
    $quotedCapture=Invoke-RfdetrCapture -Command (Join-Path $env:SystemRoot 'System32\cmd.exe') `
        -Arguments @('/d','/s','/c',"`"$quotedBatch`"") -Cwd $FixtureRoot `
        -StdoutPath (Join-Path $FixtureRoot 'quoted-call.stdout') -StderrPath (Join-Path $FixtureRoot 'quoted-call.stderr') `
        -TimeoutMilliseconds 10000 -Environment @{} -CmdArgumentLine ('/d /s /c ""{0}""' -f $quotedBatch)
    $allCaptures.Add([ordered]@{name='quoted-path';capture=$quotedCapture})
    $pathReports.Add([ordered]@{path=$quotedChild;completed=$quotedCapture.completed;actual_exit=$quotedCapture.exit_code})
    if (-not $quotedCapture.completed -or $quotedCapture.exit_code -ne 37 -or
        [IO.File]::ReadAllText((Join-Path $FixtureRoot 'quoted-call.stdout')) -cne "QUOTED_PATH_CONTROL`r`n") {
        throw 'quoted parenthesized path did not preserve actual child execution'
    }
    $selector='fixture::exact_test'
    $named="test $selector ... ok"
    $summary='test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 263 filtered out; finished in 0.20s'
    $predicateCases=@(
        @{name='exact';text="$named`n`n$summary`n";expected=$true},
        @{name='duplicate-name';text="$named`n$named`n$summary`n";expected=$false},
        @{name='duplicate-summary';text="$named`n$summary`n$summary`n";expected=$false},
        @{name='wrong-name';text="test fixture::other ... ok`n$summary`n";expected=$false},
        @{name='zero-tests';text='test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 264 filtered out; finished in 0.00s';expected=$false},
        @{name='prefixed-name';text="prefix $named`n$summary`n";expected=$false},
        @{name='trailing-output';text="$named`n$summary`nlate output`n";expected=$false}
    )
    foreach ($case in $predicateCases) {
        $actual=Test-ExactSingleTest $case.text $selector
        $predicateReports.Add([ordered]@{name=$case.name;actual=$actual;expected=$case.expected})
        if ($actual -ne $case.expected) { throw "strict one-test predicate control failed: $($case.name)" }
    }
    # Exercise refusal before even resolving output paths or attempting a child.
    $OverallSeconds=60
    $invocationClock=[pscustomobject]@{ElapsedMilliseconds=[int64]60000}
    $captures=[Collections.Generic.List[object]]::new()
    $budgetError=$null
    try { Invoke-Native 'budget-expired' 'must-not-launch.exe' @() $FixtureRoot 120 }
    catch { $budgetError=$_.Exception.Message }
    $budgetReports.Add([ordered]@{name='expired-before-launch';error=$budgetError;captures=$captures.Count})
    if ($budgetError -cne 'total producer invocation budget exhausted before native launch' -or $captures.Count -ne 0) { throw 'expired total budget attempted a launch' }
    $tries=@($ast.EndBlock.Statements | Where-Object { $_ -is [Management.Automation.Language.TryStatementAst] })
    if ($tries.Count -ne 1 -or $null -eq $tries[0].Finally) { throw 'expected one actual top-level terminal block' }
    $text=$tries[0].Finally.Extent.Text
    $terminal=[ScriptBlock]::Create($text.Substring(1,$text.Length-2))
    # Explicit fault-injection boundaries: known-empty census and empty rule bag
    # cannot authorize fixture cleanup while the actual driver has pending=true.
    function Assert-NoNative([string]$Label) {
        $script:censusCalls++
        if ($script:failCensus) { throw 'injected census failure' }
    }
    function Remove-Denies { $script:denyCleanupCalls++ }
    $executable=Join-Path $FixtureRoot 'producer-control-child.exe'
    [IO.File]::WriteAllText((Join-Path $FixtureRoot 'producer-control-child.cs'),$source,$utf8)
    Add-Type -TypeDefinition $source -OutputAssembly $executable -OutputType ConsoleApplication -ErrorAction Stop
    $cases=@(
        @{name='success';argument='success';seconds=10;pending=$false;exit=0;writeFailure=$false;censusFailure=$false},
        @{name='producer-exit2';argument='exit2';seconds=10;pending=$true;exit=2;writeFailure=$false;censusFailure=$false},
        @{name='exit2-execution-write-failure';argument='exit2';seconds=10;pending=$true;exit=2;writeFailure=$true;censusFailure=$false},
        @{name='live-root-deadline';argument='sleep';seconds=1;pending=$true;exit=$null;writeFailure=$false;censusFailure=$false},
        @{name='root0-held-eof';argument='held';seconds=1;pending=$true;exit=0;writeFailure=$false;censusFailure=$false},
        @{name='total-budget-cap';argument='sleep';seconds=10;pending=$true;exit=$null;writeFailure=$false;censusFailure=$false},
        @{name='census-failure';argument='success';seconds=10;pending=$true;exit=0;writeFailure=$false;censusFailure=$true}
    )
    foreach ($case in $cases) {
        $caseRoot=Join-Path $FixtureRoot $case.name
        $reportRoot=Join-Path $caseRoot 'report'
        $logRoot=Join-Path $reportRoot 'logs'
        $fence=Join-Path $caseRoot 'fixture-fence'
        New-Item -ItemType Directory -Path $caseRoot,$reportRoot,$logRoot,$fence -ErrorAction Stop | Out-Null
        $token=[Guid]::NewGuid().ToString('N')
        Write-NewText (Join-Path $fence 'owner.token') $token
        $fixtureFences.Add([ordered]@{path=$fence;token=$token})
        $fenceOwned=$true; $pending=$false; $exitCode=0
        $captures=[Collections.Generic.List[object]]::new()
        $failures=[Collections.Generic.List[string]]::new()
        $rules=[Collections.Generic.List[string]]::new()
        $environmentChanges=@{}
        $Mode='fixture-controls'
        $OverallSeconds=60
        $invocationClock=[Diagnostics.Stopwatch]::StartNew()
        if ($case.name -ceq 'total-budget-cap') { $invocationClock=[pscustomobject]@{ElapsedMilliseconds=[int64]59000} }
        $InputPathsSha256='fixture-not-a-production-input'
        $ExpectedProductCommit='fixture-not-a-product-commit'
        $ExpectedCargoLockSha256='fixture-not-a-product-lock'
        $script:censusCalls=0; $script:denyCleanupCalls=0; $script:failCensus=$case.censusFailure
        if ($case.writeFailure) { Write-NewText (Join-Path $logRoot 'produce.execution.json') 'occupied-fixture-leaf' }
        $caught=$null
        $clock=[Diagnostics.Stopwatch]::StartNew()
        try {
            Invoke-Native 'produce' $executable @($case.argument) $caseRoot $case.seconds
        } catch {
            $caught=$_.ToString()
            $failures.Add($caught)
        } finally {
            $clock.Stop()
            foreach ($capture in $captures) { $allCaptures.Add([ordered]@{name=$case.name;capture=$capture}) }
            # Invoke the actual parsed terminal code even if capture/evidence
            # failed. This exercises the same pending predicate as production.
            . $terminal
        }
        if ($captures.Count -ne 1) { throw 'control lost actual capture owner' }
        $capture=$captures[0]
        $record=[ordered]@{name=$case.name;actual_child_exit=$capture.exit_code;capture_completed=$capture.completed;
            observed_return_ms=$clock.ElapsedMilliseconds;pending=$pending;fence_present=(Test-Path -LiteralPath $fence);
            census_calls=$script:censusCalls;deny_cleanup_calls=$script:denyCleanupCalls;error=$caught}
        $caseReports.Add($record)
        if ($capture.exit_code -ne $case.exit -or $pending -ne $case.pending -or $record.fence_present -ne $case.pending) { throw "control state mismatch: $($case.name)" }
        if ($case.pending -and -not $case.censusFailure -and ($script:censusCalls -ne 0 -or $script:denyCleanupCalls -ne 0)) { throw 'pending state reached cleanup' }
        if ($case.writeFailure -and [IO.File]::ReadAllText((Join-Path $logRoot 'produce.execution.json')) -cne 'occupied-fixture-leaf') { throw 'evidence collision replaced original bytes' }
        if (($case.seconds -eq 1 -or $case.name -ceq 'total-budget-cap') -and ($capture.completed -or $clock.ElapsedMilliseconds -gt 2500)) { throw 'deadline/held-EOF control did not return within its observed bound' }
        if ($case.name -ceq 'total-budget-cap') {
            $execution=[IO.File]::ReadAllText((Join-Path $logRoot 'produce.execution.json')) | ConvertFrom-Json
            $budgetReports.Add([ordered]@{name='remaining-budget-caps-step';wait_budget_ms=$execution.wait_budget_ms;requested_seconds=$case.seconds})
            if ($execution.wait_budget_ms -ne 1000) { throw 'step wait exceeded remaining invocation budget' }
        }
        if ($case.argument -eq 'success' -or $case.argument -eq 'exit2') {
            if (-not $capture.completed -or [Convert]::ToBase64String([IO.File]::ReadAllBytes((Join-Path $logRoot 'produce.stdout'))) -cne 'AP8NCg==') { throw 'original native bytes changed' }
        }
    }
    $controlExit=0
} catch {
    $terminalFailures.Add($_.ToString()); $terminalFailures.Add($_.ScriptStackTrace)
} finally {
    # Every actual capture participates, including those before an early failed
    # assertion. Retain original Process/tasks; never reopen a PID or kill it.
    foreach ($entry in $allCaptures) {
        $capture=$entry.capture
        if ($capture.completed -or -not $capture.launch_attempted) { continue }
        try {
            Settle-Capture $capture
            $settlements.Add([ordered]@{name=$entry.name;settled=$true})
        } catch {
            $controlExit=1; $terminalFailures.Add($_.ToString())
            $settlements.Add([ordered]@{name=$entry.name;settled=$false;error=$_.ToString()})
        }
    }
    $fixturePending=@($settlements | Where-Object { -not $_.settled }).Count -ne 0
    if (-not $fixturePending) {
        foreach ($entry in $fixtureFences) {
            if (Test-Path -LiteralPath $entry.path) {
                try {
                    if ([IO.File]::ReadAllText((Join-Path $entry.path 'owner.token')) -cne $entry.token) { throw 'fixture fence token changed' }
                    Remove-Item -LiteralPath $entry.path -Recurse -ErrorAction Stop
                } catch { $controlExit=1; $fixturePending=$true; $terminalFailures.Add($_.ToString()) }
            }
        }
    }
    $report=[ordered]@{actual_exit_code=$controlExit;cases=@($caseReports.ToArray());predicate_cases=@($predicateReports.ToArray());budget_cases=@($budgetReports.ToArray());path_cases=@($pathReports.ToArray());settlements=@($settlements.ToArray());
        failures=@($terminalFailures.ToArray());pending_fixture_reconciliation=$fixturePending;
        driver_sha256=(Get-FileHash -LiteralPath $driver -Algorithm SHA256).Hash.ToLowerInvariant();
        controls_sha256=(Get-FileHash -LiteralPath $PSCommandPath -Algorithm SHA256).Hash.ToLowerInvariant();
        capture_sha256=(Get-FileHash -LiteralPath $captureSource -Algorithm SHA256).Hash.ToLowerInvariant();
        scope='Actual native fixture exits/bytes and exact producer capture/finally AST. Census/rule removal injected against fixture fences only. No producer build, real host fence, firewall, package or whole-tree cleanup proof.'}
    [IO.File]::WriteAllText((Join-Path $FixtureRoot 'producer-controls.json'),(ConvertTo-Json -InputObject $report -Depth 12),$utf8)
}
if ($controlExit -eq 0) { Write-Output 'WINDOWS_PRODUCER_CALLER_CONTROLS_PASS' }
exit $controlExit
