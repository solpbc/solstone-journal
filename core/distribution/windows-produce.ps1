# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

# Local, unsigned production only. Existing host fence and Unowned build tools;
# uncertain capture retains the fence and original evidence for reconciliation.
[CmdletBinding()]
param(
    [Parameter(Mandatory=$true)][ValidateSet('capture-controls','produce')][string]$Mode,
    [Parameter(Mandatory=$true)][ValidateRange(30,14400)][int]$OverallSeconds,
    [Parameter(Mandatory=$true)][string]$RecorderRepositoryRoot,
    [Parameter(Mandatory=$true)][string]$ProductRepositoryRoot,
    [Parameter(Mandatory=$true)][string]$RunRoot,
    [Parameter(Mandatory=$true)][string]$InputPaths,
    [Parameter(Mandatory=$true)][ValidatePattern('^[0-9a-f]{64}$')][string]$InputPathsSha256,
    [Parameter(Mandatory=$true)][string]$Msys2Archive,
    [Parameter(Mandatory=$true)][string]$MakeArchive,
    [Parameter(Mandatory=$true)][string]$NasmArchive,
    [Parameter(Mandatory=$true)][string]$LlvmArchive,
    [Parameter(Mandatory=$true)][ValidatePattern('^[0-9a-f]{40}$')][string]$ExpectedProductCommit,
    [Parameter(Mandatory=$true)][ValidatePattern('^[0-9a-f]{64}$')][string]$ExpectedCargoLockSha256,
    [string]$GitRoot='C:\Program Files\Git'
)
Set-StrictMode -Version Latest
$ErrorActionPreference='Stop'
$ProgressPreference='SilentlyContinue'
$invocationClock=[Diagnostics.Stopwatch]::StartNew()
$utf8=[Text.UTF8Encoding]::new($false,$true)
$fence='C:\ProgramData\solstone\journal-win-bootstrap.lock'
$token=[Guid]::NewGuid().ToString('N')
$fenceOwned=$false
$pending=$false
$exitCode=1
$captures=[Collections.Generic.List[object]]::new()
$failures=[Collections.Generic.List[string]]::new()
$rules=[Collections.Generic.List[string]]::new()
$environmentChanges=@{}
$nativeNames=@('cl','link','MSBuild','rustc','cargo','ninja','cmake','ctest','vpk','signtool',
    'rfdetr-cli','solstone-distribution','git','bash','sh','make','nasm','perl')

function Require-PlainPath([string]$Path) {
    if ($Path -notmatch '^[A-Za-z]:\\' -or $Path -match '["''%!&|<>^`$()\x00-\x1f]') {
        throw "plain absolute drive path required by the fixed build commands: $Path"
    }
}
function Require-File([string]$Path) {
    if (-not (Test-Path -LiteralPath $Path -PathType Leaf)) { throw "required file absent: $Path" }
    if ((Get-Item -LiteralPath $Path).Attributes -band [IO.FileAttributes]::ReparsePoint) { throw "reparse input refused: $Path" }
}
function Write-NewText([string]$Path,[string]$Text) {
    $bytes=$utf8.GetBytes($Text)
    $file=[IO.FileStream]::new($Path,[IO.FileMode]::CreateNew,[IO.FileAccess]::Write,[IO.FileShare]::Read)
    try { $file.Write($bytes,0,$bytes.Length); $file.Flush($true) } finally { $file.Dispose() }
}
function Write-NewJson([string]$Path,$Value) { Write-NewText $Path (ConvertTo-Json -InputObject $Value -Depth 15) }
function Digest([string]$Path) { (Get-FileHash -LiteralPath $Path -Algorithm SHA256).Hash.ToLowerInvariant() }
function Set-BuildEnvironment([string]$Name,[string]$Value) {
    if (-not $environmentChanges.ContainsKey($Name)) { $environmentChanges[$Name]=[Environment]::GetEnvironmentVariable($Name,'Process') }
    [Environment]::SetEnvironmentVariable($Name,$Value,'Process')
}
function Invoke-Native {
    param([string]$Label,[string]$Command,[string[]]$Arguments,[string]$Cwd,
        [int]$Seconds=120,[string]$CmdArgumentLine)
    if ($Label -notmatch '^[a-z0-9-]+$') { throw 'invalid capture label' }
    $remaining=[int64]$OverallSeconds*1000-$invocationClock.ElapsedMilliseconds
    if ($remaining -le 0) { throw 'total producer invocation budget exhausted before native launch' }
    $waitMilliseconds=[int][Math]::Min([int64]$Seconds*1000,$remaining)
    $stdout=Join-Path $logRoot "$Label.stdout"
    $stderr=Join-Path $logRoot "$Label.stderr"
    $capture=Invoke-RfdetrCapture -Command $Command -Arguments $Arguments -Cwd $Cwd `
        -StdoutPath $stdout -StderrPath $stderr -TimeoutMilliseconds $waitMilliseconds -Environment @{} -CmdArgumentLine $CmdArgumentLine
    $captures.Add($capture)
    # Latch before any evidence write. The original returned objects remain in
    # this driver until it exits; only the fence/evidence survives driver exit.
    if ($capture.launch_attempted -and -not $capture.completed) { $script:pending=$true }
    if ($Label -ceq 'host-fence-acquire' -and $capture.completed -and $capture.exit_code -eq 0) { $script:fenceOwned=$true }
    if ($Label -ceq 'produce' -and $capture.launch_attempted -and $capture.exit_code -ne 0) {
        # A failed inner capture may have closed its pipes without establishing
        # EOF. A known-name census cannot clear that uncertainty automatically.
        $script:pending=$true
    }
    Write-NewJson (Join-Path $logRoot "$Label.execution.json") ([ordered]@{
        label=$Label;command=$Command;arguments=$Arguments;cwd=$Cwd;launch_attempted=$capture.launch_attempted;
        started=$capture.started;pid=$capture.pid;actual_exit_code=$capture.exit_code;completed=$capture.completed;
        elapsed_ms=$capture.elapsed_ms;wait_budget_ms=$waitMilliseconds;invocation_elapsed_ms=$invocationClock.ElapsedMilliseconds;
        error=$capture.error;stdout=$stdout;stderr=$stderr
    })
    if (-not $capture.completed) { throw "$Label capture incomplete: $($capture.error)" }
    Write-NewJson (Join-Path $logRoot "$Label.streams.json") ([ordered]@{
        stdout_sha256=(Digest $stdout);stderr_sha256=(Digest $stderr);actual_exit_code=$capture.exit_code
    })
    if ($capture.exit_code -ne 0) { throw "$Label actual exit $($capture.exit_code)" }
}
function Test-ExactSingleTest([string]$Output,[string]$Selector) {
    $lines=@($Output -split "`r?`n" | Where-Object { $_.Length -ne 0 })
    $named=@($lines | Where-Object { $_ -ceq "test $Selector ... ok" })
    $allPasses=@($lines | Where-Object { $_ -cmatch '^test .+ \.\.\. ok$' })
    $summaries=@($lines | Where-Object { $_ -cmatch '^test result:' })
    return ($named.Count -eq 1 -and $allPasses.Count -eq 1 -and $summaries.Count -eq 1 -and
        $summaries[0] -cmatch '^test result: ok\. 1 passed; 0 failed; 0 ignored; 0 measured; [0-9]+ filtered out; finished in [0-9]+(?:\.[0-9]+)?s$' -and
        $lines[-1] -ceq $summaries[0])
}
function Log-Text([string]$Label) { [IO.File]::ReadAllText((Join-Path $logRoot "$Label.stdout"),$utf8) }
function Assert-NoNative([string]$Label) {
    $all=@(Get-Process -ErrorAction Stop)
    if (@($all | Where-Object { $_.Id -eq $PID }).Count -ne 1) { throw 'census failed its self control' }
    $active=@($all | Where-Object { $nativeNames -contains $_.ProcessName })
    Write-NewJson (Join-Path $reportRoot "$Label.json") @($active | ForEach-Object { [ordered]@{name=$_.ProcessName;pid=$_.Id} })
    if ($active.Count -ne 0) { throw "active build/release work at $Label; yield host" }
}
function Check-Repository([string]$Root,[string]$Label) {
    Invoke-Native "$Label-head" $git @('rev-parse','HEAD') $Root
    if ((Log-Text "$Label-head").Trim() -cne $ExpectedProductCommit) { throw "$Label source commit mismatch" }
    Invoke-Native "$Label-status" $git @('status','--porcelain=v1','--untracked-files=all','--ignore-submodules=none') $Root
    if ((Log-Text "$Label-status").Length -ne 0) { throw "$Label source is dirty" }
    if ((Digest (Join-Path $Root 'core\Cargo.lock')) -cne $ExpectedCargoLockSha256) { throw "$Label lock mismatch" }
}
function Add-Deny([string]$Program) {
    Require-File $Program
    $name="solstone-produce-$token-$($rules.Count)"
    $rules.Add($name)
    Write-NewJson (Join-Path $reportRoot "$name.json") ([ordered]@{name=$name;program=$Program})
    New-NetFirewallRule -Name $name -DisplayName $name -Direction Outbound -Action Block -Program $Program -Profile Any -Enabled True -ErrorAction Stop | Out-Null
    $rule=Get-NetFirewallRule -Name $name -PolicyStore ActiveStore -ErrorAction Stop
    if ($rule.Enabled -ne 'True' -or $rule.Direction -ne 'Outbound' -or $rule.Action -ne 'Block') { throw "deny rule inactive: $name" }
}
function Remove-Denies {
    foreach ($name in $rules) {
        $found=@(Get-NetFirewallRule -ErrorAction Stop | Where-Object { $_.Name -ceq $name })
        if ($found.Count -ne 0) { Remove-NetFirewallRule -Name $name -ErrorAction Stop }
        if (@(Get-NetFirewallRule -ErrorAction Stop | Where-Object { $_.Name -ceq $name }).Count -ne 0) { throw "deny removal unconfirmed: $name" }
    }
    $rules.Clear()
}
function Msys-Path([string]$Path) {
    Require-PlainPath $Path
    return '/'+$Path.Substring(0,1).ToLowerInvariant()+'/'+$Path.Substring(3).Replace('\','/')
}
function One-File([string]$Root,[string]$Name) {
    $files=@(Get-ChildItem -LiteralPath $Root -Filter $Name -File -Recurse)
    if ($files.Count -ne 1) { throw "expected one $Name under private tool root" }
    return $files[0].FullName
}

foreach ($path in @($RecorderRepositoryRoot,$ProductRepositoryRoot,$RunRoot,$InputPaths,$Msys2Archive,$MakeArchive,$NasmArchive,$LlvmArchive,$GitRoot)) { Require-PlainPath $path }
$RecorderRepositoryRoot=(Resolve-Path -LiteralPath $RecorderRepositoryRoot).ProviderPath
$ProductRepositoryRoot=(Resolve-Path -LiteralPath $ProductRepositoryRoot).ProviderPath
$RunRoot=[IO.Path]::GetFullPath($RunRoot)
$roots=@($RecorderRepositoryRoot.TrimEnd('\'),$ProductRepositoryRoot.TrimEnd('\'),$RunRoot.TrimEnd('\'))
for ($i=0;$i -lt $roots.Count;$i++) {
    for ($j=0;$j -lt $roots.Count;$j++) {
        if ($i -ne $j -and ($roots[$i].Equals($roots[$j],[StringComparison]::OrdinalIgnoreCase) -or $roots[$i].StartsWith($roots[$j]+'\',[StringComparison]::OrdinalIgnoreCase))) { throw 'recorder, product and run roots must be separate' }
    }
}
if (-not [IO.Path]::GetFullPath($PSScriptRoot).Equals([IO.Path]::GetFullPath((Join-Path $RecorderRepositoryRoot 'core\distribution')),[StringComparison]::OrdinalIgnoreCase)) { throw 'execute exact source-owned driver from recorder checkout' }
if ((Digest (Join-Path $PSScriptRoot 'rfdetr-windows-capture.ps1')) -cne 'a4959a5aca1b346a915e87c4019e64169a153f27af2a724012eb01dca13fe605') { throw 'capture helper differs from reviewed bytes' }
. (Join-Path $PSScriptRoot 'rfdetr-windows-capture.ps1')
if (Test-Path -LiteralPath $RunRoot) { throw 'fresh isolated run root required' }
New-Item -ItemType Directory -Path $RunRoot -ErrorAction Stop | Out-Null
$reportRoot=Join-Path $RunRoot 'report'
$logRoot=Join-Path $reportRoot 'logs'
New-Item -ItemType Directory -Path $reportRoot,$logRoot -ErrorAction Stop | Out-Null
try {
    foreach ($name in @('CL','_CL_','LINK','_LINK_','CFLAGS','CXXFLAGS','CPPFLAGS','LDFLAGS','FFMPEG_DIR',
        'CMAKE_TOOLCHAIN_FILE','CMAKE_GENERATOR','CMAKE_PREFIX_PATH','CARGO_TARGET_DIR','RUSTFLAGS','CARGO_ENCODED_RUSTFLAGS','RUSTC_WRAPPER','RUSTC_WORKSPACE_WRAPPER')) {
        if (-not [string]::IsNullOrEmpty([Environment]::GetEnvironmentVariable($name))) { throw "inherited build override refused: $name" }
    }
    Assert-NoNative 'host-before-fence'
    $cmd=Join-Path $env:SystemRoot 'System32\cmd.exe'
    Invoke-Native 'host-fence-acquire' $cmd @('/d','/s','/c',"mkdir `"$fence`"") $RunRoot 30 ('/d /s /c "mkdir "{0}""' -f $fence)
    Write-NewText (Join-Path $fence 'owner.token') $token
    Write-NewText (Join-Path $fence 'held.marker') ([DateTimeOffset]::UtcNow.ToUnixTimeSeconds().ToString())
    Assert-NoNative 'host-after-fence'
    foreach ($entry in @(Get-ChildItem Env:)) {
        if ($entry.Name.StartsWith('GIT_',[StringComparison]::OrdinalIgnoreCase)) { Set-BuildEnvironment $entry.Name $null }
    }
    Set-BuildEnvironment 'GIT_CONFIG_NOSYSTEM' '1'
    Set-BuildEnvironment 'GIT_CONFIG_GLOBAL' 'NUL'
    Set-BuildEnvironment 'GIT_NO_REPLACE_OBJECTS' '1'
    Set-BuildEnvironment 'GIT_TERMINAL_PROMPT' '0'
    Set-BuildEnvironment 'MSBUILDDISABLENODEREUSE' '1'
    Set-BuildEnvironment 'CARGO_BUILD_JOBS' '2'
    $git=Join-Path $GitRoot 'cmd\git.exe'
    Require-File $git
    Set-BuildEnvironment 'PATH' ((Split-Path -Parent $git)+';'+$env:PATH)
    Check-Repository $RecorderRepositoryRoot 'recorder-before'
    Check-Repository $ProductRepositoryRoot 'product-before'
    foreach ($root in @($RecorderRepositoryRoot,$ProductRepositoryRoot)) {
        if (Test-Path -LiteralPath (Join-Path $root 'core\target')) { throw 'both checkouts require fresh core/target' }
    }
    Require-File $InputPaths
    if ((Get-Item -LiteralPath $InputPaths).Length -gt 1048576 -or (Digest $InputPaths) -cne $InputPathsSha256) { throw 'local input path file changed' }
    $inputs=[IO.File]::ReadAllText($InputPaths,$utf8) | ConvertFrom-Json
    $ffmpeg=$inputs.ffmpeg_archive
    Require-PlainPath $ffmpeg
    foreach ($path in @($ffmpeg,$Msys2Archive,$MakeArchive,$NasmArchive,$LlvmArchive)) { Require-File $path }
    $vswhere=Join-Path ${env:ProgramFiles(x86)} 'Microsoft Visual Studio\Installer\vswhere.exe'
    Invoke-Native 'vswhere' $vswhere @('-latest','-products','*','-requires','Microsoft.VisualStudio.Component.VC.Tools.x86.x64','-property','installationPath') $RunRoot
    $vs=(Log-Text 'vswhere').Trim()
    if (-not $vs -or $vs.Contains("`n")) { throw 'expected one Visual Studio tool installation' }
    $vcvars=Join-Path $vs 'VC\Auxiliary\Build\vcvarsall.bat'
    Require-File $vcvars
    Require-PlainPath $vcvars
    $batch=Join-Path $RunRoot 'build-environment.cmd'
    Write-NewText $batch ("@echo off`r`ncall `"$vcvars`" x64`r`nif errorlevel 1 exit /b %errorlevel%`r`n"+
        "set PATH`r`nset INCLUDE`r`nset LIB`r`nset VCTools`r`nset WindowsSDK`r`nset VSINSTALLDIR`r`nset VCINSTALLDIR`r`nset VSCMD_ARG_TGT_ARCH`r`nexit /b 0`r`n")
    Invoke-Native 'build-environment' $cmd @('/d','/s','/c',"`"$batch`"") $RunRoot 60 ('/d /s /c ""{0}""' -f $batch)
    foreach ($line in (Log-Text 'build-environment') -split "`r?`n") {
        if ($line -match '^(PATH|PATHEXT|INCLUDE|LIB|LIBPATH|VCToolsInstallDir|VCToolsVersion|VCToolsRedistDir|WindowsSdkDir|WindowsSDKVersion|WindowsSDKLibVersion|WindowsSdkBinPath|WindowsSdkVerBinPath|VSINSTALLDIR|VCINSTALLDIR|VSCMD_ARG_TGT_ARCH)=(.*)$') { Set-BuildEnvironment $Matches[1] $Matches[2] }
    }
    $cargo=(Get-Command cargo.exe -CommandType Application -ErrorAction Stop).Source
    $rustc=(Get-Command rustc.exe -CommandType Application -ErrorAction Stop).Source
    $cl=(Get-Command cl.exe -CommandType Application -ErrorAction Stop).Source
    $link=(Get-Command link.exe -CommandType Application -ErrorAction Stop).Source
    $msbuild=Join-Path $vs 'MSBuild\Current\Bin\MSBuild.exe'
    foreach ($program in @($cargo,$rustc,$cl,$link,$msbuild,$git,(Join-Path $GitRoot 'mingw64\bin\git.exe'))) { Add-Deny $program }
    Invoke-Native 'build-recorder' $cargo @('build','--manifest-path','core\Cargo.toml','-p','solstone-core-distribution','--bin','solstone-distribution','--target','x86_64-pc-windows-msvc','--locked','--offline','-j','2') $RecorderRepositoryRoot 1800
    $recorder=Join-Path $RecorderRepositoryRoot 'core\target\x86_64-pc-windows-msvc\debug\solstone-distribution.exe'
    Require-File $recorder
    Add-Deny $recorder
    Write-NewJson (Join-Path $reportRoot 'recorder.json') ([ordered]@{path=$recorder;sha256=(Digest $recorder);product_commit=$ExpectedProductCommit;cargo_lock_sha256=$ExpectedCargoLockSha256})
    if ($Mode -eq 'capture-controls') {
        $selectors=@('live_capture_preserves_raw_bytes_and_actual_nonzero_exit',
            'live_capture_waits_for_inherited_writer_and_refuses_evidence_failure')
        $ordinal=0
        foreach ($selector in $selectors) {
            $label="capture-control-$ordinal"
            Invoke-Native $label $cargo @('test','--manifest-path','core\Cargo.toml','-p','solstone-core-distribution','--test','distribution_build_capture','--features','test-hooks','--target','x86_64-pc-windows-msvc','--locked','--offline',$selector,'--','--exact','--nocapture','--test-threads=1') $RecorderRepositoryRoot 1800
            $output=Log-Text $label
            if (-not (Test-ExactSingleTest $output $selector)) { throw 'exact one-test capture predicate failed' }
            $ordinal++
        }
    } else {
        Invoke-Native 'verify-tool-inputs' $recorder @('ffmpeg-windows','verify-inputs','--source-archive',$ffmpeg,'--msys2-archive',$Msys2Archive,'--make-archive',$MakeArchive,'--nasm-archive',$NasmArchive,'--llvm-archive',$LlvmArchive) $ProductRepositoryRoot 300
        $toolsRoot=Join-Path $RunRoot 'tools'
        New-Item -ItemType Directory -Path $toolsRoot -ErrorAction Stop | Out-Null
        $systemTar=Join-Path $env:SystemRoot 'System32\tar.exe'
        Invoke-Native 'msys-extract' $systemTar @('-xJf',$Msys2Archive,'-C',$toolsRoot) $RunRoot 600
        $msysRoot=Join-Path $toolsRoot 'msys64'
        $msysBin=Join-Path $msysRoot 'usr\bin'
        $bash=Join-Path $msysBin 'bash.exe'
        $makeExtract="'/usr/bin/zstd' -d -c '$(Msys-Path $MakeArchive)' | '/usr/bin/tar' -x -C '$(Msys-Path $msysRoot)'"
        Invoke-Native 'make-extract' $bash @('--noprofile','--norc','-o','pipefail','-c',$makeExtract) $RunRoot 600
        $nasmRoot=Join-Path $toolsRoot 'nasm'
        New-Item -ItemType Directory -Path $nasmRoot -ErrorAction Stop | Out-Null
        Invoke-Native 'nasm-extract' $systemTar @('-xf',$NasmArchive,'-C',$nasmRoot) $RunRoot 120
        Invoke-Native 'llvm-extract' $systemTar @('-xJf',$LlvmArchive,'-C',$toolsRoot) $RunRoot 600
        $nasm=One-File $nasmRoot 'nasm.exe'
        $libclang=One-File $toolsRoot 'libclang.dll'
        $sh=Join-Path $msysBin 'sh.exe'
        $make=Join-Path $msysBin 'make.exe'
        foreach ($program in @($bash,$sh,$make,(Join-Path $msysBin 'perl.exe'),$nasm)) { Add-Deny $program }
        Set-BuildEnvironment 'PATH' ((Split-Path -Parent $cl)+';'+(Split-Path -Parent $nasm)+';'+(Split-Path -Parent $git)+';'+$env:PATH+';'+$msysBin)
        Set-BuildEnvironment 'LIBCLANG_PATH' (Split-Path -Parent $libclang)
        Set-BuildEnvironment 'CARGO_TARGET_X86_64_PC_WINDOWS_MSVC_LINKER' $link
        Set-BuildEnvironment 'CC' 'cl'
        Set-BuildEnvironment 'FFMPEG_MARCH' ''
        Set-BuildEnvironment 'FFMPEG_MTUNE' ''
        Set-BuildEnvironment 'SOLSTONE_DISTRIBUTION_OFFLINE' '1'
        $expected=[ordered]@{'cargo.exe'=$cargo;'git.exe'=$git;'cl.exe'=$cl;'link.exe'=$link;'sh.exe'=$sh;'make.exe'=$make;'nasm.exe'=$nasm}
        foreach ($name in $expected.Keys) {
            $actual=(Get-Command $name -CommandType Application -ErrorAction Stop).Source
            if (-not $actual.Equals($expected[$name],[StringComparison]::OrdinalIgnoreCase)) { throw "unexpected first PATH resolution: $name" }
        }
        Write-NewJson (Join-Path $reportRoot 'toolchain.json') @(@($expected.Values)+@($bash,$libclang) | ForEach-Object { [ordered]@{path=$_;sha256=(Digest $_)} })
        Invoke-Native 'shell-probe' $sh @('-c','printf FFMPEG_SH_READY') $RunRoot
        if ((Log-Text 'shell-probe') -cne 'FFMPEG_SH_READY') { throw 'carried shell probe differs' }
        if ((Digest $InputPaths) -cne $InputPathsSha256) { throw 'local input path file changed before production' }
        $destination=Join-Path $RunRoot 'payload'
        $cargoLogs=Join-Path $RunRoot 'cargo-product'
        Invoke-Native 'produce' $recorder @('produce','windows-x86_64',$destination,'--inputs',$InputPaths,'--logs',$cargoLogs) $ProductRepositoryRoot 7200
        $outcome=Log-Text 'produce' | ConvertFrom-Json
        if ($outcome.signed -ne $false -or $outcome.durability_proven -ne $false -or $outcome.source.commit -cne $ExpectedProductCommit -or $outcome.source.lock_sha256 -cne $ExpectedCargoLockSha256) { throw 'producer outcome differs from requested unsigned source' }
        Write-NewJson (Join-Path $reportRoot 'payload.json') $outcome
    }
    Check-Repository $RecorderRepositoryRoot 'recorder-after'
    Check-Repository $ProductRepositoryRoot 'product-after'
    $exitCode=0
} catch {
    $failures.Add($_.ToString()); $failures.Add($_.ScriptStackTrace)
} finally {
    if ($fenceOwned -and -not $pending) {
        try { Assert-NoNative 'host-terminal' } catch { $pending=$true; $failures.Add($_.ToString()) }
    }
    if ($fenceOwned -and -not $pending) {
        try {
            Remove-Denies
            if ([IO.File]::ReadAllText((Join-Path $fence 'owner.token')) -cne $token) { throw 'fence ownership changed' }
            Remove-Item -LiteralPath $fence -Recurse -ErrorAction Stop
            if (Test-Path -LiteralPath $fence) { throw 'fence removal unconfirmed' }
            $fenceOwned=$false
        } catch { $pending=$true; $failures.Add($_.ToString()) }
    }
    if ($pending -or $failures.Count -ne 0) { $exitCode=1 }
    foreach ($name in $environmentChanges.Keys) { [Environment]::SetEnvironmentVariable($name,$environmentChanges[$name],'Process') }
    Write-NewJson (Join-Path $reportRoot 'execution.json') ([ordered]@{actual_exit_code=$exitCode;mode=$Mode;pending_reconciliation=$pending;
        fence_retained=$fenceOwned;fence_present=(Test-Path -LiteralPath $fence);fence=$fence;owner_token=$token;firewall_rules_retained=@($rules.ToArray());
        captures=@($captures | ForEach-Object { [ordered]@{pid=$_.pid;started=$_.started;actual_exit_code=$_.exit_code;completed=$_.completed;elapsed_ms=$_.elapsed_ms;error=$_.error} });
        failures=@($failures.ToArray());process_ownership='Unowned';whole_tree_quiescence_proven=$false;input_paths_sha256=$InputPathsSha256;
        overall_budget_seconds=$OverallSeconds;invocation_elapsed_ms=$invocationClock.ElapsedMilliseconds;
        product_commit=$ExpectedProductCommit;cargo_lock_sha256=$ExpectedCargoLockSha256;network_scope='offline Cargo and explicit per-program deny rules; no machine-wide network-denial claim'})
    Write-NewText (Join-Path $reportRoot 'exit.txt') $exitCode.ToString()
}
Write-Output "WINDOWS_PRODUCER_TERMINAL exit=$exitCode pending=$pending report=$reportRoot"
exit $exitCode
