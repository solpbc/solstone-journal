# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

# Isolated operator build. Build tools are Unowned under the Windows process
# contract. On uncertain completion leave the fence, rules and original logs
# for operator reconciliation; never infer descendant cleanup from root exit.
[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)][ValidateRange(30,14400)][int]$OverallSeconds,
    [Parameter(Mandatory = $true)][string]$RepositoryRoot,
    [Parameter(Mandatory = $true)][string]$RfBundle,
    [Parameter(Mandatory = $true)][string]$GgmlBundle,
    [Parameter(Mandatory = $true)][string]$CmakeArchive,
    [Parameter(Mandatory = $true)][string]$RunRoot,
    [Parameter(Mandatory = $true)][string]$Model,
    [Parameter(Mandatory = $true)][string]$Image,
    [Parameter(Mandatory = $true)][ValidatePattern('^[0-9a-f]{64}$')][string]$ImageSha256,
    [Parameter(Mandatory = $true)][ValidatePattern('^[0-9a-f]{40}$')][string]$ExpectedProductCommit,
    [Parameter(Mandatory = $true)][ValidatePattern('^[0-9a-f]{64}$')][string]$ExpectedCargoLockSha256,
    [Parameter(Mandatory = $true)][string]$BuilderHost,
    [string]$GitRoot = 'C:\Program Files\Git'
)
Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
$ProgressPreference = 'SilentlyContinue'
$invocationClock = [Diagnostics.Stopwatch]::StartNew()
$RepositoryRoot = (Resolve-Path -LiteralPath $RepositoryRoot).ProviderPath
if (-not [IO.Path]::GetFullPath($PSScriptRoot).Equals(
    [IO.Path]::GetFullPath((Join-Path $RepositoryRoot 'core\distribution')), [StringComparison]::OrdinalIgnoreCase)) {
    throw 'execute the driver from the exact transferred product checkout'
}
. (Join-Path $PSScriptRoot 'rfdetr-windows-capture.ps1')
$utf8 = [Text.UTF8Encoding]::new($false, $true)
$fence = 'C:\ProgramData\solstone\journal-win-bootstrap.lock'
$token = [Guid]::NewGuid().ToString('N')
$fenceOwned = $false
$pending = $false
$exitCode = 1
$failures = [Collections.Generic.List[string]]::new()
$rules = [Collections.Generic.List[string]]::new()
$captures = [Collections.Generic.List[object]]::new()
$records = [Collections.Generic.List[object]]::new()
$environmentChanges = @{}
$nativeNames = @('cl','link','MSBuild','rustc','cargo','ninja','cmake','ctest','vpk','signtool',
    'rfdetr-cli','solstone-distribution','git','bash','sh')

function Write-NewText([string]$Path, [string]$Text) {
    $bytes = $utf8.GetBytes($Text)
    $file = [IO.FileStream]::new($Path, [IO.FileMode]::CreateNew, [IO.FileAccess]::Write, [IO.FileShare]::Read)
    try { $file.Write($bytes, 0, $bytes.Length); $file.Flush($true) } finally { $file.Dispose() }
}
function Write-NewJson([string]$Path, $Value) {
    Write-NewText $Path (ConvertTo-Json -InputObject $Value -Depth 15)
}
function Digest([string]$Path) { (Get-FileHash -LiteralPath $Path -Algorithm SHA256).Hash.ToLowerInvariant() }
function Require-File([string]$Path) {
    if (-not (Test-Path -LiteralPath $Path -PathType Leaf)) { throw "required file absent: $Path" }
    if ((Get-Item -LiteralPath $Path).Attributes -band [IO.FileAttributes]::ReparsePoint) { throw "reparse file refused: $Path" }
}
function Require-CmdPath([string]$Path) {
    if ($Path -match '["%!&|<>^\r\n]') { throw "path cannot be passed to the fixed cmd batch: $Path" }
}
function Set-BuildEnvironment([string]$Name, [string]$Value) {
    if (-not $environmentChanges.ContainsKey($Name)) {
        $environmentChanges[$Name] = [Environment]::GetEnvironmentVariable($Name, 'Process')
    }
    [Environment]::SetEnvironmentVariable($Name, $Value, 'Process')
}
function Invoke-Native {
    param([string]$Label, [string]$Command, [string[]]$Arguments, [string]$Cwd,
        [int]$Seconds = 120, [hashtable]$Environment = @{}, [string]$CmdArgumentLine)
    if ($Label -notmatch '^[a-z0-9-]+$') { throw 'invalid native evidence label' }
    $remaining = [int64]$OverallSeconds * 1000 - $invocationClock.ElapsedMilliseconds
    if ($remaining -le 0) { throw 'total RF invocation budget exhausted before native launch' }
    $waitMilliseconds = [int][Math]::Min([int64]$Seconds * 1000, $remaining)
    $stdout = Join-Path $logRoot "$Label.stdout"
    $stderr = Join-Path $logRoot "$Label.stderr"
    $capture = Invoke-RfdetrCapture -Command $Command -Arguments $Arguments -Cwd $Cwd `
        -StdoutPath $stdout -StderrPath $stderr -TimeoutMilliseconds $waitMilliseconds `
        -Environment $Environment -CmdArgumentLine $CmdArgumentLine
    $captures.Add($capture)
    # Preserve ownership before any evidence write can itself fail.
    if ($capture.launch_attempted -and -not $capture.completed) { $script:pending = $true }
    if ($Label -ceq 'host-fence-acquire' -and $capture.completed -and $capture.exit_code -eq 0) {
        $script:fenceOwned = $true
    }
    $detail = [ordered]@{label=$Label; argv=@($Command)+$Arguments; cwd=$Cwd;
        launch_attempted=$capture.launch_attempted; started=$capture.started; pid=$capture.pid;
        exit_code=$capture.exit_code; completed=$capture.completed; elapsed_ms=$capture.elapsed_ms;
        wait_budget_ms=$waitMilliseconds; invocation_elapsed_ms=$invocationClock.ElapsedMilliseconds;
        error=$capture.error; stdout_path="logs/$Label.stdout"; stderr_path="logs/$Label.stderr"}
    Write-NewJson (Join-Path $logRoot "$Label.execution.json") $detail
    if (-not $capture.completed) {
        throw "$Label capture incomplete: $($capture.error)"
    }
    $records.Add([ordered]@{label=$Label; argv=@($Command)+$Arguments; cwd=$Cwd;
        exit_code=$capture.exit_code; stdout_path="logs/$Label.stdout"; stderr_path="logs/$Label.stderr"})
    if ($capture.exit_code -ne 0) { throw "$Label exited $($capture.exit_code)" }
    if ((Get-Item -LiteralPath $stdout).Length -gt 67108864 -or (Get-Item -LiteralPath $stderr).Length -gt 67108864) {
        throw "$Label exceeds the recorder stream admission limit; original bytes retained"
    }
}
function Log-Text([string]$Label) { [IO.File]::ReadAllText((Join-Path $logRoot "$Label.stdout"), $utf8) }
function Assert-NoNative([string]$Label) {
    # A finite, read-only census of the release/build tools that serialize this
    # host. This is not a process-tree owner and does not reopen/kill these PIDs.
    $all = @(Get-Process -ErrorAction Stop)
    if (@($all | Where-Object { $_.Id -eq $PID }).Count -ne 1) { throw 'process census did not find its own driver' }
    $active = @($all | Where-Object { $nativeNames -contains $_.ProcessName })
    Write-NewJson (Join-Path $reportRoot "$Label.json") @($active | ForEach-Object {
        [ordered]@{name=$_.ProcessName; pid=$_.Id}
    })
    if ($active.Count -ne 0) { throw "native/release tool activity at $Label; yield the host" }
}
function Add-Deny([string]$Program) {
    Require-File $Program
    $name = "solstone-rfdetr-build-$token-$($rules.Count)"
    # Register intent before creation so an uncertain creation result also has
    # a durable cleanup name. Only these request-owned names may be removed.
    $rules.Add($name)
    Write-NewJson (Join-Path $reportRoot "$name.json") ([ordered]@{name=$name;program=$Program})
    New-NetFirewallRule -Name $name -DisplayName $name -Direction Outbound -Action Block `
        -Program $Program -Profile Any -Enabled True -ErrorAction Stop | Out-Null
    $rule = Get-NetFirewallRule -Name $name -PolicyStore ActiveStore -ErrorAction Stop
    if ($rule.Enabled -ne 'True' -or $rule.Direction -ne 'Outbound' -or $rule.Action -ne 'Block') {
        throw "network deny is not active: $name"
    }
}
function Remove-Denies {
    foreach ($name in $rules) {
        $existing = @(Get-NetFirewallRule -ErrorAction Stop | Where-Object { $_.Name -ceq $name })
        if ($existing.Count -ne 0) { Remove-NetFirewallRule -Name $name -ErrorAction Stop }
        if (@(Get-NetFirewallRule -ErrorAction Stop | Where-Object { $_.Name -ceq $name }).Count -ne 0) {
            throw "network deny removal unconfirmed: $name"
        }
    }
    $rules.Clear()
}
function Check-Product([string]$Phase) {
    Invoke-Native "product-head-$Phase" $git @('rev-parse','HEAD') $RepositoryRoot
    if ((Log-Text "product-head-$Phase").Trim() -cne $ExpectedProductCommit) { throw 'product source commit mismatch' }
    Invoke-Native "product-status-$Phase" $git @('status','--porcelain=v1','--untracked-files=all','--ignore-submodules=none') $RepositoryRoot
    if ((Log-Text "product-status-$Phase").Length -ne 0) { throw 'product checkout is dirty' }
    if ((Digest (Join-Path $RepositoryRoot 'core\Cargo.lock')) -cne $ExpectedCargoLockSha256) { throw 'product lockfile mismatch' }
}

# All mutation roots are fresh, outside the transferred clean product checkout.
$RunRoot = [IO.Path]::GetFullPath($RunRoot)
if (Test-Path -LiteralPath $RunRoot) { throw 'run root already exists; use a fresh isolated path' }
if ($RunRoot.StartsWith($RepositoryRoot.TrimEnd('\')+'\', [StringComparison]::OrdinalIgnoreCase)) {
    throw 'run root must be outside the source checkout'
}
New-Item -ItemType Directory -Path $RunRoot -ErrorAction Stop | Out-Null
$reportRoot = Join-Path $RunRoot 'report'
$logRoot = Join-Path $reportRoot 'logs'
New-Item -ItemType Directory -Path $reportRoot,$logRoot -ErrorAction Stop | Out-Null
try {
    foreach ($name in @('CL','_CL_','LINK','_LINK_','CFLAGS','CXXFLAGS','CPPFLAGS','LDFLAGS',
        'CMAKE_TOOLCHAIN_FILE','CMAKE_GENERATOR','CMAKE_GENERATOR_INSTANCE','CMAKE_GENERATOR_PLATFORM',
        'CMAKE_GENERATOR_TOOLSET','CMAKE_PREFIX_PATH','CARGO_TARGET_DIR','RUSTFLAGS','CARGO_ENCODED_RUSTFLAGS')) {
        if (-not [string]::IsNullOrEmpty([Environment]::GetEnvironmentVariable($name))) { throw "inherited build override refused: $name" }
    }
    Assert-NoNative 'host-before-fence'
    Require-CmdPath $fence
    $cmd = Join-Path $env:SystemRoot 'System32\cmd.exe'
    Invoke-Native 'host-fence-acquire' $cmd @('/d','/s','/c',"mkdir `"$fence`"") $RunRoot 30 @{} `
        ('/d /s /c "mkdir "{0}""' -f $fence)
    $fenceOwned = $true
    Write-NewText (Join-Path $fence 'owner.token') $token
    Write-NewText (Join-Path $fence 'held.marker') ([DateTimeOffset]::UtcNow.ToUnixTimeSeconds().ToString())
    Assert-NoNative 'host-after-fence'
    foreach ($path in @($RfBundle,$GgmlBundle,$CmakeArchive,$Model,$Image)) { Require-File $path }
    $git = Join-Path $GitRoot 'cmd\git.exe'
    Require-File $git
    foreach ($entry in @(Get-ChildItem Env:)) {
        if ($entry.Name.StartsWith('GIT_', [StringComparison]::OrdinalIgnoreCase)) { Set-BuildEnvironment $entry.Name $null }
    }
    Set-BuildEnvironment 'GIT_CONFIG_NOSYSTEM' '1'
    Set-BuildEnvironment 'GIT_CONFIG_GLOBAL' 'NUL'
    Set-BuildEnvironment 'GIT_NO_REPLACE_OBJECTS' '1'
    Set-BuildEnvironment 'GIT_TERMINAL_PROMPT' '0'
    Set-BuildEnvironment 'MSBUILDDISABLENODEREUSE' '1'
    Set-BuildEnvironment 'CARGO_BUILD_JOBS' '2'
    Check-Product 'before'
    if (Test-Path -LiteralPath (Join-Path $RepositoryRoot 'core\target')) {
        throw 'recorder requires a fresh checkout/core/target; existing build output is not restamped'
    }
    $vswhere = Join-Path ${env:ProgramFiles(x86)} 'Microsoft Visual Studio\Installer\vswhere.exe'
    Require-File $vswhere
    Invoke-Native 'vswhere' $vswhere @('-latest','-products','*','-requires','Microsoft.VisualStudio.Component.VC.Tools.x86.x64','-property','installationPath') $RunRoot
    $vs = (Log-Text 'vswhere').Trim()
    if (-not $vs -or $vs.Contains("`n")) { throw 'Visual Studio selection is not one installation' }
    $vcvars = Join-Path $vs 'VC\Auxiliary\Build\vcvarsall.bat'
    Require-File $vcvars
    Require-CmdPath $vcvars
    $environmentBatch = Join-Path $RunRoot 'build-environment.cmd'
    Require-CmdPath $environmentBatch
    # Retain compiler setup only. Dumping the whole inherited environment would
    # put unrelated operator credentials into a distributable evidence packet.
    Write-NewText $environmentBatch ("@echo off`r`ncall `"$vcvars`" x64`r`nif errorlevel 1 exit /b %errorlevel%`r`n" +
        "set PATH`r`nset INCLUDE`r`nset LIB`r`nset VCTools`r`nset WindowsSDK`r`nset VSINSTALLDIR`r`nset VCINSTALLDIR`r`nset VSCMD_ARG_TGT_ARCH`r`nexit /b 0`r`n")
    Invoke-Native 'build-environment' $cmd @('/d','/s','/c',"`"$environmentBatch`"") $RunRoot 60 @{} `
        ('/d /s /c ""{0}""' -f $environmentBatch)
    foreach ($line in (Log-Text 'build-environment') -split "`r?`n") {
        if ($line -match '^(PATH|PATHEXT|INCLUDE|LIB|LIBPATH|VCToolsInstallDir|VCToolsVersion|VCToolsRedistDir|WindowsSdkDir|WindowsSDKVersion|WindowsSDKLibVersion|WindowsSdkBinPath|WindowsSdkVerBinPath|VSINSTALLDIR|VCINSTALLDIR|VSCMD_ARG_TGT_ARCH)=(.*)$') {
            Set-BuildEnvironment $Matches[1] $Matches[2]
        }
    }
    $cargo = (Get-Command cargo.exe -CommandType Application -ErrorAction Stop).Source
    $rustc = (Get-Command rustc.exe -CommandType Application -ErrorAction Stop).Source
    $cl = (Get-Command cl.exe -CommandType Application -ErrorAction Stop).Source
    $link = (Get-Command link.exe -CommandType Application -ErrorAction Stop).Source
    $msbuild = Join-Path $vs 'MSBuild\Current\Bin\MSBuild.exe'
    $bash = Join-Path $GitRoot 'bin\bash.exe'
    Require-File $bash
    Set-BuildEnvironment 'PATH' ((Join-Path $GitRoot 'bin')+';'+$env:PATH)
    foreach ($program in @($git,$cargo,$rustc,$cl,$link,$msbuild,$bash,
        (Join-Path $GitRoot 'mingw64\bin\git.exe'),(Join-Path $GitRoot 'usr\bin\bash.exe'),
        (Join-Path $GitRoot 'usr\bin\sh.exe'))) { Add-Deny $program }
    Invoke-Native 'cargo-build-recorder' $cargo @('build','--manifest-path','core\Cargo.toml','-p','solstone-core-distribution',
        '--bin','solstone-distribution','--target','x86_64-pc-windows-msvc','--locked','--offline','-j','2') $RepositoryRoot 1800
    $recorder = Join-Path $RepositoryRoot 'core\target\x86_64-pc-windows-msvc\debug\solstone-distribution.exe'
    Require-File $recorder
    Add-Deny $recorder
    Invoke-Native 'build-plan' $recorder @('rfdetr-windows','plan') $RepositoryRoot
    $plan = Log-Text 'build-plan' | ConvertFrom-Json
    if ((Digest $Model) -cne $plan.model_sha256 -or (Digest $Image) -cne $ImageSha256) { throw 'real-image/model input digest mismatch' }
    Invoke-Native 'verify-inputs' $recorder @('rfdetr-windows','verify-inputs','--rf-bundle',$RfBundle,
        '--ggml-bundle',$GgmlBundle,'--cmake-archive',$CmakeArchive) $RepositoryRoot 300
    $source = Join-Path $RunRoot 'source'
    $expectedSource = Join-Path $RunRoot 'expected-source'
    $expectedGgml = Join-Path $expectedSource 'third_party\ggml'
    Invoke-Native 'rf-clone' $git @('-c','core.autocrlf=false','clone','--no-checkout',$RfBundle,$source) $RunRoot
    Invoke-Native 'rf-checkout' $git @('-c','core.autocrlf=false','checkout','--detach',$plan.rf_commit) $source
    $ggml = Join-Path $source 'third_party\ggml'
    Invoke-Native 'ggml-clone' $git @('-c','core.autocrlf=false','clone','--no-checkout',$GgmlBundle,$ggml) $RunRoot
    Invoke-Native 'ggml-checkout' $git @('-c','core.autocrlf=false','checkout','--detach',$plan.ggml_commit) $ggml
    Invoke-Native 'expected-rf-clone' $git @('-c','core.autocrlf=false','clone','--no-checkout',$RfBundle,$expectedSource) $RunRoot
    Invoke-Native 'expected-rf-checkout' $git @('-c','core.autocrlf=false','checkout','--detach',$plan.rf_commit) $expectedSource
    Invoke-Native 'expected-ggml-clone' $git @('-c','core.autocrlf=false','clone','--no-checkout',$GgmlBundle,$expectedGgml) $RunRoot
    Invoke-Native 'expected-ggml-checkout' $git @('-c','core.autocrlf=false','checkout','--detach',$plan.ggml_commit) $expectedGgml
    $patches = @(Get-ChildItem -LiteralPath (Join-Path $source 'third_party\ggml-patches') -Filter '*.patch' -File | Sort-Object Name)
    $patchInputs = [Collections.Generic.List[object]]::new()
    foreach ($patch in $patches) {
        Require-File $patch.FullName
        $patchInputs.Add([ordered]@{path=('third_party/ggml-patches/'+$patch.Name);size=$patch.Length;sha256=(Digest $patch.FullName)})
    }
    $patchScript = Join-Path $source 'scripts\apply_ggml_patches.sh'
    Require-File $patchScript
    Write-NewJson (Join-Path $reportRoot 'source-patch-inputs.json') ([ordered]@{
        script=[ordered]@{path='scripts/apply_ggml_patches.sh';size=(Get-Item -LiteralPath $patchScript).Length;sha256=(Digest $patchScript)};
        ordered_patches=@($patchInputs.ToArray())})
    # Execute the admitted script unchanged in its own fresh RF+ggml checkout.
    # A hand-reimplementation of its apply/skip/locking behavior is not evidence.
    $expectedScript = (Join-Path $expectedSource 'scripts\apply_ggml_patches.sh').Replace('\','/')
    Invoke-Native 'expected-source-patches' $bash @($expectedScript) $expectedSource
    Invoke-Native 'ggml-expected-diff' $git @('diff','--binary','--full-index','HEAD') $expectedGgml
    $cmakeRoot = Join-Path $RunRoot 'cmake'
    Add-Type -AssemblyName System.IO.Compression.FileSystem
    [IO.Compression.ZipFile]::ExtractToDirectory($CmakeArchive, $cmakeRoot)
    $cmakes = @(Get-ChildItem -LiteralPath $cmakeRoot -Filter cmake.exe -Recurse -File)
    if ($cmakes.Count -ne 1) { throw 'CMake archive executable census mismatch' }
    $cmake = $cmakes[0].FullName
    Add-Deny $cmake
    $build = Join-Path $RunRoot 'build'
    Invoke-Native 'cmake-configure' $cmake (@('-S',$source,'-B',$build,'-G','Visual Studio 17 2022','-A','x64') + $plan.configuration.flags) $source 600
    Invoke-Native 'cmake-build' $cmake @('--build',$build,'--config','Release','--target','rfdetr-cli','--parallel','2') $source 3600
    Invoke-Native 'rf-tracked-after' $git @('diff','--exit-code','--ignore-submodules=all','HEAD') $source
    Invoke-Native 'rf-untracked-after' $git @('ls-files','--others','--exclude-standard') $source
    $untracked = @((Log-Text 'rf-untracked-after') -split "`r?`n" | Where-Object { $_ })
    if (@($untracked | Where-Object { $_ -cne 'third_party/.ggml-patch.lock' }).Count -ne 0) { throw 'unexpected RF source files after build' }
    foreach ($member in $untracked) {
        $generated = Join-Path $source 'third_party\.ggml-patch.lock'
        Require-File $generated
        if ((Get-Item -LiteralPath $generated).Length -ne 0) { throw 'source patch lock is not the script-created empty file' }
    }
    Invoke-Native 'ggml-actual-diff' $git @('diff','--binary','--full-index','HEAD') $ggml
    if ((Digest (Join-Path $logRoot 'ggml-actual-diff.stdout')) -cne (Digest (Join-Path $logRoot 'ggml-expected-diff.stdout'))) {
        throw 'ggml build changes do not match the admitted source-owned patches'
    }
    Invoke-Native 'ggml-untracked-after' $git @('ls-files','--others','--exclude-standard') $ggml
    if ((Log-Text 'ggml-untracked-after').Length -ne 0) { throw 'unexpected ggml source files after build' }
    $cache = Join-Path $build 'CMakeCache.txt'
    $projects = @(Get-ChildItem -LiteralPath $build -Filter rfdetr-cli.vcxproj -Recurse -File)
    $executables = @(Get-ChildItem -LiteralPath $build -Filter rfdetr-cli.exe -Recurse -File)
    if ($projects.Count -ne 1 -or $executables.Count -ne 1) { throw 'RF output/project census mismatch' }
    Require-File $cache
    $outputRoot = Join-Path $RunRoot 'output'
    $outputBin = Join-Path $outputRoot 'bin'
    New-Item -ItemType Directory -Path $outputBin -ErrorAction Stop | Out-Null
    $executable = Join-Path $outputBin 'rfdetr-cli.exe'
    [IO.File]::Copy($executables[0].FullName, $executable, $false)
    Add-Deny $executable
    $detections = Join-Path $reportRoot 'detections.json'
    Invoke-Native 'real-image' $executable @('detect','--model',$Model,'--input',$Image,'--threshold','0.55',
        '--threads','2','--output',$detections) $outputBin 120 @{'PATH'=(Join-Path $env:SystemRoot 'System32')+';'+$env:SystemRoot}
    Require-File $detections
    $detected = [IO.File]::ReadAllText($detections, $utf8) | ConvertFrom-Json
    if (@($detected.detections | Where-Object { $_.class_name -eq 'person' -and $_.score -ge 0.55 }).Count -eq 0) {
        throw 'real-image inference did not detect the expected person'
    }
    Check-Product 'after'
    Assert-NoNative 'host-after-build'
    Remove-Denies
    Write-NewJson (Join-Path $reportRoot 'subprocess-evidence.json') @($records.ToArray())
    $validation = Join-Path $reportRoot 'rfdetr-build-validation.json'
    Write-NewJson $validation ([ordered]@{product_commit=$ExpectedProductCommit;cargo_lock_sha256=$ExpectedCargoLockSha256;
        rf_bundle_sha256=(Digest $RfBundle);ggml_bundle_sha256=(Digest $GgmlBundle);cmake_archive_sha256=(Digest $CmakeArchive);
        model_sha256=(Digest $Model);image_sha256=(Digest $Image);detections_sha256=(Digest $detections);
        ggml_patch_diff_sha256=(Digest (Join-Path $logRoot 'ggml-actual-diff.stdout'));
        source_patch_inputs_sha256=(Digest (Join-Path $reportRoot 'source-patch-inputs.json'));
        generated_untracked_exclusions=@($untracked);
        process_ownership='Unowned build tools';capture='native root exit and original stdout/stderr EOF';
        cleanup='known build/release tool census empty; request firewall rules removed';whole_tree_quiescence_proven=$false;
        proof='controlled source build and standalone inference; installed payload proof remains separate'})
    $receipt = Join-Path $reportRoot 'rfdetr-build-receipt.json'
    $evidence = Join-Path $reportRoot 'rfdetr-build-evidence.json'
    $subprocessEvidence = Join-Path $reportRoot 'subprocess-evidence.json'
    Invoke-Native 'record' $recorder @('rfdetr-windows','record','--rf-bundle',$RfBundle,'--ggml-bundle',$GgmlBundle,
        '--cmake-archive',$CmakeArchive,'--cmake-cache',$cache,'--build-option-evidence',$projects[0].FullName,
        '--subprocess-evidence',$subprocessEvidence,'--output-root',$outputRoot,'--evidence',$evidence,'--receipt',$receipt,
        '--validation',$validation,'--product-commit',$ExpectedProductCommit,'--cargo-lock-sha256',$ExpectedCargoLockSha256,
        '--builder-host',$BuilderHost,'--toolchain',("CMake $($plan.cmake.version); MSVC " + (Get-Item -LiteralPath $cl).VersionInfo.ProductVersion)) $RepositoryRoot 300
    Invoke-Native 'verify' $recorder @('rfdetr-windows','verify','--receipt',$receipt,'--output-root',$outputRoot,
        '--evidence',$evidence,'--cmake-cache',$cache,'--build-option-evidence',$projects[0].FullName,
        '--subprocess-evidence',$subprocessEvidence) $RepositoryRoot 300
    $exitCode = 0
} catch {
    $failures.Add($_.ToString())
    $failures.Add($_.ScriptStackTrace)
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
            $fenceOwned = $false
        } catch { $pending=$true; $failures.Add($_.ToString()) }
    }
    if ($pending -or $failures.Count -ne 0) { $exitCode=1 }
    foreach ($name in $environmentChanges.Keys) { [Environment]::SetEnvironmentVariable($name, $environmentChanges[$name], 'Process') }
    Write-NewJson (Join-Path $reportRoot 'execution.json') ([ordered]@{exit_code=$exitCode;pending_reconciliation=$pending;
        overall_budget_seconds=$OverallSeconds;invocation_elapsed_ms=$invocationClock.ElapsedMilliseconds;
        timed_scope='native child/root and stream EOF waits share one budget; synchronous Start, filesystem, cmdlets, Flush and Dispose are outside interruptible waits; finite SSH transport required';
        fence_retained=$fenceOwned;fence=$fence;owner_token=$token;firewall_rules_retained=@($rules.ToArray());
        fence_present=(Test-Path -LiteralPath $fence);captured_processes=@($captures | ForEach-Object {
            [ordered]@{launch_attempted=$_.launch_attempted;started=$_.started;pid=$_.pid;exit_code=$_.exit_code;
                completed=$_.completed;elapsed_ms=$_.elapsed_ms;error=$_.error}
        });
        failures=@($failures.ToArray());process_ownership='Unowned';whole_tree_quiescence_proven=$false;
        incomplete_logs_are_original_prefixes=$pending;product_commit=$ExpectedProductCommit;cargo_lock_sha256=$ExpectedCargoLockSha256})
    Write-NewText (Join-Path $reportRoot 'exit.txt') $exitCode.ToString()
}
Write-Output "RFDETR_WINDOWS_TERMINAL exit=$exitCode pending=$pending report=$reportRoot"
exit $exitCode
