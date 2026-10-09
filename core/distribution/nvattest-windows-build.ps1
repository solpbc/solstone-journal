# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc
#
# The native half of the controlled Windows NVIDIA GPU verifier build. It
# refuses inherited roots, verifies both transferred archives before
# extraction, builds the receipt recorder offline from this clean checkout,
# denies outbound network for every process except the operator's transport
# peer, runs the SDK's own offline build (sol/windows/build.ps1 at the pinned
# revision) and three of its refusal controls in a cleared environment, and
# persists the receipt through the recorder, which re-runs the producer's
# admission before writing anything. It does not sign, stage or publish.

[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)][string]$RepositoryRoot,
    [Parameter(Mandatory = $true)][string]$SourceArchive,
    [Parameter(Mandatory = $true)][ValidatePattern('^[0-9a-f]{64}$')][string]$SourceSha256,
    [Parameter(Mandatory = $true)][ValidateRange(1, [Int64]::MaxValue)][Int64]$SourceSize,
    [Parameter(Mandatory = $true)][string]$BundleArchive,
    [Parameter(Mandatory = $true)][ValidatePattern('^[0-9a-f]{64}$')][string]$BundleSha256,
    [Parameter(Mandatory = $true)][ValidateRange(1, [Int64]::MaxValue)][Int64]$BundleSize,
    [Parameter(Mandatory = $true)][ValidatePattern('^[0-9a-f]{64}$')][string]$ManifestSha256,
    [Parameter(Mandatory = $true)][string]$WorkRoot,
    [Parameter(Mandatory = $true)][string]$OutputRoot,
    [Parameter(Mandatory = $true)][string]$ReportRoot,
    [Parameter(Mandatory = $true)][ValidatePattern('^[0-9a-f]{40}$')][string]$ExpectedProductCommit,
    [Parameter(Mandatory = $true)][ValidatePattern('^[0-9a-f]{64}$')][string]$ExpectedCargoLockSha256,
    [Parameter(Mandatory = $true)][string]$BuilderHost,
    [Parameter(Mandatory = $true)][string]$TransportPeer,
    [ValidateRange(600, 7200)][int]$BuildSeconds = 7200
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
$ProgressPreference = 'SilentlyContinue'

# Committed pins. The recorder holds the same values in Rust and refuses any
# difference; these only let the driver refuse early and name its inputs.
$PinnedSdkRevision = '69a71c859ec02b6e5f10616b8b8a873c230941c9'
$PinnedSourceSha256 = '8a6f05954c8fe490acec8c1032926a72e31fb3f303493a7cb263bd5e63534623'
$PinnedSourceSize = 5201920
$PinnedBundleSha256 = '114e0135cbcec53dd104d330c76ffafd630c372f5c25d0e103d718514f34725d'
$PinnedBundleSize = 451512320
$PinnedManifestSha256 = 'bca2a8fa20094517599953f38a3708e79ab93106228c594df3c9a4e19836932d'
$CmakeArchiveName = 'cmake-3.31.12-windows-x86_64.zip'
$CmakeExeRelative = 'cmake-3.31.12-windows-x86_64\bin\cmake.exe'
$CorruptMember = 'openssl-3.6.5.tar.gz'
$RefusalManifestBoundary = 'offline manifest does not match the caller-bound digest'
$RefusalReuseBoundary = 'offline builds cannot reuse dependencies'
$RefusalCorruptBoundary = "missing or changed offline input: $CorruptMember"
$ProbePort = 443
$Ipv4Probe = '1.1.1.1'
$Ipv6Probe = '2606:4700:4700::1111'
# The complete SDK child environment; equal to NVATTEST_SDK_CHILD_ENVIRONMENT.
$ChildEnvironmentNames = [string[]]@('ComSpec', 'NUMBER_OF_PROCESSORS', 'NVAT_SOURCE_COMMIT', 'PATH',
    'PATHEXT', 'PROCESSOR_ARCHITECTURE', 'ProgramData', 'ProgramFiles', 'ProgramFiles(x86)',
    'ProgramW6432', 'RUSTC', 'SystemDrive', 'SystemRoot', 'TEMP', 'TMP', 'USERPROFILE', 'windir')

$RepositoryRoot = (Resolve-Path -LiteralPath $RepositoryRoot).ProviderPath
if (-not [IO.Path]::GetFullPath($PSScriptRoot).Equals(
    [IO.Path]::GetFullPath((Join-Path $RepositoryRoot 'core\distribution')), [StringComparison]::OrdinalIgnoreCase)) {
    throw 'execute the driver from the exact transferred product checkout'
}
. (Join-Path $PSScriptRoot 'rfdetr-windows-capture.ps1')
. (Join-Path $PSScriptRoot 'windows-rust-toolchain.ps1')

$utf8 = [Text.UTF8Encoding]::new($false, $true)
# Native tools write in whatever code page they choose; read their bytes leniently.
$lenient = [Text.UTF8Encoding]::new($false, $false)
$token = [Guid]::NewGuid().ToString('N')
$rules = [Collections.Generic.List[string]]::new()
$rulesAdded = 0
$rulesRemaining = -1
$failures = [Collections.Generic.List[string]]::new()
$captures = [Collections.Generic.List[object]]::new()
$environmentChanges = @{}
$childEnvironment = $null
$nativeExit = $null
$exitCode = 1

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
function Set-BuildEnvironment([string]$Name, [string]$Value) {
    if (-not $environmentChanges.ContainsKey($Name)) {
        $environmentChanges[$Name] = [Environment]::GetEnvironmentVariable($Name, 'Process')
    }
    [Environment]::SetEnvironmentVariable($Name, $Value, 'Process')
}
function Invoke-Native {
    param([string]$Label, [string]$Command, [string[]]$Arguments, [string]$Cwd,
        [int]$Seconds = 120, [hashtable]$Environment = @{}, [string]$CmdArgumentLine, [switch]$AllowFailure)
    if ($Label -notmatch '^[a-z0-9-]+$') { throw 'invalid native evidence label' }
    $stdout = Join-Path $logRoot "$Label.stdout"
    $stderr = Join-Path $logRoot "$Label.stderr"
    # Arguments are quoted for CreateProcess by the shared capture helper, and
    # both streams are kept as the child's raw bytes.
    $capture = Invoke-RfdetrCapture -Command $Command -Arguments $Arguments -Cwd $Cwd `
        -StdoutPath $stdout -StderrPath $stderr -TimeoutMilliseconds ($Seconds * 1000) `
        -Environment $Environment -CmdArgumentLine $CmdArgumentLine
    $captures.Add($capture)
    Write-NewJson (Join-Path $logRoot "$Label.execution.json") ([ordered]@{label=$Label; argv=@($Command)+$Arguments; cwd=$Cwd;
        launch_attempted=$capture.launch_attempted; started=$capture.started; pid=$capture.pid;
        exit_code=$capture.exit_code; completed=$capture.completed; elapsed_ms=$capture.elapsed_ms;
        error=$capture.error; stdout_sha256=(Digest $stdout); stderr_sha256=(Digest $stderr)})
    if (-not $capture.completed) { throw "$Label capture incomplete: $($capture.error)" }
    # Nothing is returned: callers such as the toolchain selection must not
    # collect exit codes into their own output. The code is kept here instead.
    $script:nativeExit = [int]$capture.exit_code
    if ($capture.exit_code -ne 0 -and -not $AllowFailure) { throw "$Label exited $($capture.exit_code)" }
}
function Log-Text([string]$Label) { $lenient.GetString([IO.File]::ReadAllBytes((Join-Path $logRoot "$Label.stdout"))) }
function Log-Combined([string]$Label) {
    (Log-Text $Label) + "`n" + $lenient.GetString([IO.File]::ReadAllBytes((Join-Path $logRoot "$Label.stderr")))
}
function Add-NamedRule([string]$Name, [hashtable]$Rule) {
    # Register intent before creation so an uncertain result has a cleanup name.
    $rules.Add($Name)
    Write-NewJson (Join-Path $reportRoot "$Name.json") ([ordered]@{name=$Name; rule=$Rule})
    New-NetFirewallRule -Name $Name -DisplayName $Name -Direction Outbound -Action Block `
        -Profile Any -Enabled True -ErrorAction Stop @Rule | Out-Null
    $active = Get-NetFirewallRule -Name $Name -PolicyStore ActiveStore -ErrorAction Stop
    if ($active.Enabled -ne 'True' -or $active.Direction -ne 'Outbound' -or $active.Action -ne 'Block') {
        throw "network deny is not active: $Name"
    }
    $script:rulesAdded++
}
# Used by the Rust toolchain selection for the rustup program it invokes.
function Add-Deny([string]$Program) {
    Require-File $Program
    Add-NamedRule "solstone-nvattest-build-$token-$($rules.Count)" @{Program=$Program}
}
function Add-RemoteDeny([string[]]$Ranges) {
    foreach ($range in $Ranges) {
        Add-NamedRule "solstone-nvattest-build-$token-$($rules.Count)" @{RemoteAddress=$range}
    }
}
function Count-RemainingRules {
    $remaining = 0
    foreach ($name in $rules) {
        if (@(Get-NetFirewallRule -Name $name -ErrorAction SilentlyContinue).Count -ne 0) { $remaining++ }
    }
    return $remaining
}
function Remove-Rules {
    foreach ($name in $rules) {
        if (@(Get-NetFirewallRule -Name $name -ErrorAction SilentlyContinue).Count -ne 0) {
            Remove-NetFirewallRule -Name $name -ErrorAction Stop
        }
    }
    $script:rulesRemaining = Count-RemainingRules
    return $script:rulesRemaining
}
function Test-TcpProbe([string]$Address, [int]$TimeoutMs = 5000) {
    $client = $null
    try {
        # A guest with the protocol disabled throws here; that is unreachable too.
        $ip = [Net.IPAddress]::Parse($Address)
        $client = [Net.Sockets.TcpClient]::new($ip.AddressFamily)
        $pending = $client.BeginConnect($ip, $ProbePort, $null, $null)
        if (-not $pending.AsyncWaitHandle.WaitOne($TimeoutMs, $false)) { return $false }
        $client.EndConnect($pending)
        return $true
    } catch {
        return $false
    } finally {
        if ($null -ne $client) { $client.Dispose() }
    }
}
function Stop-DescendantProcesses {
    # A child that outlived its deadline must not outlive the network denial.
    $all = @(Get-CimInstance Win32_Process)
    $parents = @{}
    foreach ($process in $all) { $parents[[int]$process.ProcessId] = [int]$process.ParentProcessId }
    $stopped = 0
    foreach ($process in $all) {
        $id = [int]$process.ProcessId
        $cursor = $parents[$id]
        $seen = 0
        while ($null -ne $cursor -and $cursor -ne 0 -and $seen -lt 64) {
            if ($cursor -eq $PID) {
                Stop-Process -Id $id -Force -ErrorAction SilentlyContinue
                $stopped++
                break
            }
            $cursor = $parents[$cursor]
            $seen++
        }
    }
    return $stopped
}
function Step-Address([byte[]]$Bytes, [int]$Delta) {
    $copy = [byte[]]$Bytes.Clone()
    for ($i = $copy.Length - 1; $i -ge 0; $i--) {
        $next = [int]$copy[$i] + $Delta
        if ($next -ge 0 -and $next -le 255) { $copy[$i] = [byte]$next; return $copy }
        if ($Delta -lt 0) { $copy[$i] = 255 } else { $copy[$i] = 0 }
    }
    throw 'address step overflow'
}
# Every address of the peer's family except the peer, and the whole other family.
function Get-ComplementRanges([Net.IPAddress]$Peer) {
    $bytes = $Peer.GetAddressBytes()
    $low = [Net.IPAddress]::new([byte[]]::new($bytes.Length))
    $high = [Net.IPAddress]::new([byte[]](@(255) * $bytes.Length))
    $ranges = [Collections.Generic.List[string]]::new()
    if (-not $Peer.Equals($low)) { $ranges.Add("$low-$([Net.IPAddress]::new((Step-Address $bytes -1)))") }
    if (-not $Peer.Equals($high)) { $ranges.Add("$([Net.IPAddress]::new((Step-Address $bytes 1)))-$high") }
    if ($Peer.AddressFamily -eq [Net.Sockets.AddressFamily]::InterNetwork) {
        $ranges.Add('::-ffff:ffff:ffff:ffff:ffff:ffff:ffff:ffff')
    } else {
        $ranges.Add('0.0.0.0-255.255.255.255')
    }
    return $ranges.ToArray()
}
function Check-Product([string]$Phase) {
    Invoke-Native "product-head-$Phase" $git @('rev-parse', 'HEAD') $RepositoryRoot | Out-Null
    if ((Log-Text "product-head-$Phase").Trim() -cne $ExpectedProductCommit) { throw 'product source commit mismatch' }
    Invoke-Native "product-status-$Phase" $git @('status', '--porcelain=v1', '--untracked-files=all', '--ignore-submodules=none') $RepositoryRoot | Out-Null
    if ((Log-Text "product-status-$Phase").Length -ne 0) { throw 'product checkout is dirty' }
    if ((Digest (Join-Path $RepositoryRoot 'core\Cargo.lock')) -cne $ExpectedCargoLockSha256) { throw 'product lockfile mismatch' }
}
# Run one child with exactly the committed environment and nothing inherited,
# then restore this process's own environment.
function Invoke-CleanChild {
    param([string]$Label, [string]$Command, [string[]]$Arguments, [string]$Cwd, [int]$Seconds = 120, [switch]$AllowFailure)
    $saved = [Environment]::GetEnvironmentVariables('Process')
    try {
        foreach ($name in @($saved.Keys)) { [Environment]::SetEnvironmentVariable([string]$name, $null, 'Process') }
        foreach ($name in $childEnvironment.Keys) { [Environment]::SetEnvironmentVariable($name, $childEnvironment[$name], 'Process') }
        $actual = [string[]]@([Environment]::GetEnvironmentVariables('Process').Keys | ForEach-Object { [string]$_ })
        [Array]::Sort($actual, [StringComparer]::Ordinal)
        if (($actual -join "`n") -cne ($ChildEnvironmentNames -join "`n")) {
            throw "SDK child environment is not the committed allowlist: $($actual -join ',')"
        }
        Invoke-Native $Label $Command $Arguments $Cwd $Seconds -AllowFailure:$AllowFailure
    } finally {
        foreach ($name in @([Environment]::GetEnvironmentVariables('Process').Keys)) {
            [Environment]::SetEnvironmentVariable([string]$name, $null, 'Process')
        }
        foreach ($name in $saved.Keys) { [Environment]::SetEnvironmentVariable([string]$name, [string]$saved[$name], 'Process') }
    }
}
function Get-SdkArguments([string]$Root, [string]$Digest, [switch]$Reuse) {
    $arguments = @('-NoProfile', '-NonInteractive', '-ExecutionPolicy', 'Bypass', '-File', $sdkScript,
        '-Root', $Root, '-OfflineBundle', $bundleRoot, '-OfflineManifestSha256', $Digest)
    if ($Reuse) { $arguments += '-ReuseDependencies' }
    return $arguments
}
function Require-Refusal([string]$Label, [string[]]$Arguments, [string]$Boundary) {
    Invoke-CleanChild $Label $powershell $Arguments $WorkRoot 600 -AllowFailure
    $code = $script:nativeExit
    if ($code -eq 0 -or -not (Log-Combined $Label).Contains($Boundary)) {
        throw "SDK refusal control $Label did not refuse at: $Boundary"
    }
    return [ordered]@{exit_code=$code; boundary=$Boundary}
}

# Fresh, disjoint roots, all outside the transferred checkout.
$roots = @()
foreach ($path in @($WorkRoot, $OutputRoot, $ReportRoot)) {
    $full = [IO.Path]::GetFullPath($path).TrimEnd('\')
    if (Test-Path -LiteralPath $full) { throw "root already exists and will not be reused: $full" }
    $roots += $full
}
$WorkRoot, $OutputRoot, $ReportRoot = $roots
$separate = @($roots) + @($RepositoryRoot.TrimEnd('\'))
for ($i = 0; $i -lt $separate.Count; $i++) {
    for ($j = 0; $j -lt $separate.Count; $j++) {
        if ($i -ne $j -and ($separate[$i].Equals($separate[$j], [StringComparison]::OrdinalIgnoreCase) -or
            $separate[$i].StartsWith($separate[$j] + '\', [StringComparison]::OrdinalIgnoreCase))) {
            throw 'work, output, report roots and the checkout must be separate'
        }
    }
}
New-Item -ItemType Directory -Path $WorkRoot, $OutputRoot, $ReportRoot -ErrorAction Stop | Out-Null
$logRoot = Join-Path $ReportRoot 'logs'
New-Item -ItemType Directory -Path $logRoot -ErrorAction Stop | Out-Null

try {
    foreach ($name in @('CL', '_CL_', 'LINK', '_LINK_', 'CFLAGS', 'CXXFLAGS', 'CPPFLAGS', 'LDFLAGS',
        'CMAKE_TOOLCHAIN_FILE', 'CMAKE_GENERATOR', 'CMAKE_PREFIX_PATH', 'CARGO_TARGET_DIR', 'CARGO_BUILD_TARGET_DIR',
        'RUSTFLAGS', 'CARGO_ENCODED_RUSTFLAGS', 'CARGO_HOME_OVERRIDE')) {
        if (-not [string]::IsNullOrEmpty([Environment]::GetEnvironmentVariable($name))) { throw "inherited build override refused: $name" }
    }
    if ($SourceSha256 -cne $PinnedSourceSha256 -or $SourceSize -ne $PinnedSourceSize -or
        $BundleSha256 -cne $PinnedBundleSha256 -or $BundleSize -ne $PinnedBundleSize -or
        $ManifestSha256 -cne $PinnedManifestSha256) {
        throw 'caller-bound archive or manifest identity differs from the committed pins'
    }
    $peer = $null
    if (-not [Net.IPAddress]::TryParse($TransportPeer, [ref]$peer) -or $peer.ToString() -in @($Ipv4Probe, $Ipv6Probe)) {
        throw 'TransportPeer must be one IP address other than the network probes'
    }
    foreach ($archive in @(@($SourceArchive, $SourceSize, $SourceSha256), @($BundleArchive, $BundleSize, $BundleSha256))) {
        Require-File $archive[0]
        if ((Get-Item -LiteralPath $archive[0]).Length -ne $archive[1] -or (Digest $archive[0]) -cne $archive[2]) {
            throw "archive size or SHA-256 differs from its pin: $($archive[0])"
        }
    }

    $git = (Get-Command git.exe -CommandType Application -ErrorAction Stop).Source
    foreach ($entry in @(Get-ChildItem Env:)) {
        if ($entry.Name.StartsWith('GIT_', [StringComparison]::OrdinalIgnoreCase)) { Set-BuildEnvironment $entry.Name $null }
    }
    Set-BuildEnvironment 'GIT_CONFIG_NOSYSTEM' '1'
    Set-BuildEnvironment 'GIT_CONFIG_GLOBAL' 'NUL'
    Set-BuildEnvironment 'GIT_TERMINAL_PROMPT' '0'
    Check-Product 'before'

    $systemRoot = [Environment]::GetEnvironmentVariable('SystemRoot', 'Process')
    if ([string]::IsNullOrEmpty($systemRoot) -or -not [IO.Path]::IsPathRooted($systemRoot)) { throw 'SystemRoot is not a rooted path' }
    $cmd = Join-Path $systemRoot 'System32\cmd.exe'
    $tar = Join-Path $systemRoot 'System32\tar.exe'
    $powershell = Join-Path $PSHOME 'powershell.exe'
    foreach ($tool in @($cmd, $tar, $powershell)) { Require-File $tool }
    $vswhere = Join-Path ${env:ProgramFiles(x86)} 'Microsoft Visual Studio\Installer\vswhere.exe'
    Require-File $vswhere
    Invoke-Native 'vswhere' $vswhere @('-latest', '-products', '*', '-requires', 'Microsoft.VisualStudio.Component.VC.Tools.x86.x64', '-property', 'installationPath') $WorkRoot | Out-Null
    $vs = (Log-Text 'vswhere').Trim()
    if (-not $vs -or $vs.Contains("`n")) { throw 'Visual Studio selection is not one installation' }
    $vcvarsall = Join-Path $vs 'VC\Auxiliary\Build\vcvarsall.bat'
    Require-File $vcvarsall
    if ($vcvarsall -match '["%!&|<>^\r\n]') { throw "path cannot be passed to the fixed cmd batch: $vcvarsall" }

    $rustTools = Select-WindowsRustToolchain $RepositoryRoot $ReportRoot
    $rustBin = Split-Path -Parent $rustTools.rustc

    # Compiler environment for the recorder build only, captured in a child;
    # this process never imports it, so the SDK children cannot inherit it.
    $environmentBatch = Join-Path $WorkRoot 'recorder-environment.cmd'
    Write-NewText $environmentBatch ("@echo off`r`ncall `"$vcvarsall`" x64`r`nif errorlevel 1 exit /b %errorlevel%`r`n" +
        "set PATH`r`nset INCLUDE`r`nset LIB`r`nset VCTools`r`nset WindowsSDK`r`nset VSINSTALLDIR`r`nset VCINSTALLDIR`r`nexit /b 0`r`n")
    Invoke-Native 'recorder-environment' $cmd @() $WorkRoot 120 @{} ('/d /s /c ""{0}""' -f $environmentBatch) | Out-Null
    $recorderEnvironment = @{}
    foreach ($line in (Log-Text 'recorder-environment') -split "`r?`n") {
        if ($line -match '^(PATH|INCLUDE|LIB|LIBPATH|VCToolsInstallDir|VCToolsVersion|VCToolsRedistDir|WindowsSdkDir|WindowsSDKVersion|WindowsSDKLibVersion|VSINSTALLDIR|VCINSTALLDIR)=(.*)$') {
            $recorderEnvironment[$Matches[1]] = $Matches[2]
        }
    }
    if (-not $recorderEnvironment.ContainsKey('PATH') -or -not $recorderEnvironment.ContainsKey('LIB')) { throw 'compiler environment capture is incomplete' }

    # Network denial for every process: positive controls, then the
    # complement of the transport peer, then negative controls.
    # IPv4 must connect before denial. A guest with no IPv6 route records that
    # instead; IPv6 is denied either way and must refuse afterwards.
    if (-not (Test-TcpProbe $Ipv4Probe)) { throw "network positive control could not connect before denial: $Ipv4Probe" }
    $ipv6Positive = if (Test-TcpProbe $Ipv6Probe) { 'connected' } else { 'unreachable' }
    Add-RemoteDeny (Get-ComplementRanges $peer)
    foreach ($probe in @($Ipv4Probe, $Ipv6Probe)) {
        if (Test-TcpProbe $probe 3000) { throw "network negative control connected under denial: $probe" }
    }
    Write-Output "NVATTEST_WINDOWS_NETWORK_DENY=all-programs-except-$peer rules=$rulesAdded"

    # The recorder, built offline from this checkout into the work root.
    $recorderTarget = Join-Path $WorkRoot 'recorder-target'
    Invoke-Native 'recorder-build' $rustTools.cargo @('build', '--manifest-path', 'core\Cargo.toml', '-p', 'solstone-core-distribution',
        '--bin', 'solstone-distribution', '--target', 'x86_64-pc-windows-msvc', '--target-dir', $recorderTarget,
        '--locked', '--offline', '-j', '2') $RepositoryRoot 1800 $recorderEnvironment | Out-Null
    $recorder = Join-Path $recorderTarget 'x86_64-pc-windows-msvc\debug\solstone-distribution.exe'
    Require-File $recorder
    Invoke-Native 'verify-inputs' $recorder @('nvattest-windows', 'verify-inputs', '--source-archive', $SourceArchive,
        '--bundle-archive', $BundleArchive) $RepositoryRoot 600 | Out-Null

    # Extract each verified archive once. The bundle directory is what every
    # SDK invocation receives; the SDK re-verifies it against the manifest.
    $sourceRoot = Join-Path $WorkRoot 'source'
    $bundleRoot = Join-Path $WorkRoot 'offline-inputs'
    New-Item -ItemType Directory -Path $sourceRoot, $bundleRoot -ErrorAction Stop | Out-Null
    Invoke-Native 'source-extract' $tar @('-xf', $SourceArchive, '-C', $sourceRoot) $WorkRoot 600 | Out-Null
    Invoke-Native 'bundle-extract' $tar @('-xf', $BundleArchive, '-C', $bundleRoot) $WorkRoot 1800 | Out-Null
    $sdkScript = Join-Path $sourceRoot 'sol\windows\build.ps1'
    Require-File $sdkScript
    $manifestPath = Join-Path $bundleRoot 'offline-manifest.json'
    Require-File $manifestPath
    if ((Digest $manifestPath) -cne $PinnedManifestSha256) { throw 'extracted offline manifest differs from its pin' }
    $manifest = [IO.File]::ReadAllText($manifestPath, $utf8) | ConvertFrom-Json
    $members = @{}
    foreach ($item in $manifest.files) {
        $file = Join-Path $bundleRoot ($item.path.Replace('/', '\'))
        Require-File $file
        if ((Get-Item -LiteralPath $file).Length -ne $item.size -or (Digest $file) -cne $item.sha256) {
            throw "extracted bundle member differs from the manifest: $($item.path)"
        }
        $members[$item.path] = $item
    }
    $extracted = @(Get-ChildItem -LiteralPath $bundleRoot -Recurse -File -Force)
    if ($extracted.Count -ne $members.Count + 1) { throw 'extracted bundle has members the manifest does not list' }
    if (-not $members.ContainsKey($CorruptMember) -or -not $members.ContainsKey($CmakeArchiveName)) {
        throw 'bundle manifest lacks a member the controls require'
    }

    # The SDK children's exact environment. Its user profile is fresh, so the
    # SDK's own Cargo bin prepend names an empty directory and Path resolution
    # reaches the selected toolchain, which RUSTC also names.
    $childProfile = Join-Path $WorkRoot 'child-profile'
    $childTemp = Join-Path $WorkRoot 'child-temp'
    New-Item -ItemType Directory -Path $childProfile, $childTemp -ErrorAction Stop | Out-Null
    $childEnvironment = [ordered]@{}
    foreach ($name in @('NUMBER_OF_PROCESSORS', 'PROCESSOR_ARCHITECTURE', 'ProgramData', 'ProgramFiles', 'ProgramFiles(x86)',
        'ProgramW6432', 'SystemDrive')) {
        $value = [Environment]::GetEnvironmentVariable($name, 'Process')
        if ([string]::IsNullOrEmpty($value)) { throw "host environment lacks $name" }
        $childEnvironment[$name] = $value
    }
    $childEnvironment['ComSpec'] = $cmd
    $childEnvironment['NVAT_SOURCE_COMMIT'] = $PinnedSdkRevision
    $childEnvironment['PATH'] = "$rustBin;$systemRoot\System32;$systemRoot;$systemRoot\System32\Wbem;$systemRoot\System32\WindowsPowerShell\v1.0"
    $childEnvironment['PATHEXT'] = '.COM;.EXE;.BAT;.CMD'
    $childEnvironment['RUSTC'] = $rustTools.rustc
    $childEnvironment['SystemRoot'] = $systemRoot
    $childEnvironment['TEMP'] = $childTemp
    $childEnvironment['TMP'] = $childTemp
    $childEnvironment['USERPROFILE'] = $childProfile
    $childEnvironment['windir'] = $systemRoot

    # Tool census, resolved exactly as the SDK resolves its tools: in the
    # cleared environment, after vcvars64 and the Cargo bin prepend.
    $probeScript = Join-Path $WorkRoot 'census-probe.ps1'
    $probeOut = Join-Path $WorkRoot 'census-probe.json'
    Write-NewText $probeScript @'
param([Parameter(Mandatory = $true)][string]$Out)
$ErrorActionPreference = 'Stop'
$vswhere = Join-Path ${env:ProgramFiles(x86)} 'Microsoft Visual Studio\Installer\vswhere.exe'
$vs = & $vswhere -latest -products '*' -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 -property installationPath
if (-not $vs) { throw 'Visual Studio C++ x64 build tools not found' }
$vcvars = Join-Path $vs 'VC\Auxiliary\Build\vcvars64.bat'
cmd.exe /c "`"$vcvars`" >nul && set" | ForEach-Object {
    if ($_ -match '^([^=]+)=(.*)$') { Set-Item -Path "env:$($Matches[1])" -Value $Matches[2] }
}
$env:Path = "$env:USERPROFILE\.cargo\bin;$env:Path"
function Resolve-Tool([string]$Name) { @(Get-Command $Name -CommandType Application -ErrorAction Stop)[0].Source }
$msbuild = @(Get-Command 'MSBuild.exe' -CommandType Application -ErrorAction SilentlyContinue)
$result = [ordered]@{
    vs_installation = [string]$vs
    vc_tools_version = $env:VCToolsVersion
    windows_sdk_version = $env:WindowsSDKVersion
    visual_studio_version = $env:VisualStudioVersion
    rustc = (Resolve-Tool 'rustc.exe')
    rustc_version = [string](& rustc -V)
    cargo = (Resolve-Tool 'cargo.exe')
    cargo_version = [string](& cargo -V)
    cl = (Resolve-Tool 'cl.exe')
    link = (Resolve-Tool 'link.exe')
    nmake = (Resolve-Tool 'nmake.exe')
    dumpbin = (Resolve-Tool 'dumpbin.exe')
    msbuild = $(if ($msbuild.Count -gt 0) { $msbuild[0].Source } else { $null })
}
[IO.File]::WriteAllText($Out, (ConvertTo-Json -InputObject $result -Depth 4), [Text.UTF8Encoding]::new($false))
'@
    Invoke-CleanChild 'census-probe' $powershell @('-NoProfile', '-NonInteractive', '-ExecutionPolicy', 'Bypass',
        '-File', $probeScript, '-Out', $probeOut) $WorkRoot 300 | Out-Null
    $probe = [IO.File]::ReadAllText($probeOut, $utf8) | ConvertFrom-Json
    foreach ($tool in @('rustc', 'cargo')) {
        if (-not ([string]$probe.$tool).Equals($rustTools[$tool], [StringComparison]::OrdinalIgnoreCase)) {
            throw "the SDK environment resolves a different $tool than the selected toolchain: $($probe.$tool)"
        }
    }
    $windowsSdk = [string]$probe.windows_sdk_version
    if (-not $windowsSdk.EndsWith('\') -or $windowsSdk.EndsWith('\\')) { throw "WindowsSDKVersion must end with one backslash: $windowsSdk" }
    if ([string]::IsNullOrEmpty([string]$probe.vc_tools_version)) { throw 'VCToolsVersion is absent from the SDK environment' }
    $msbuildPath = [string]$probe.msbuild
    if ([string]::IsNullOrEmpty($msbuildPath)) { $msbuildPath = Join-Path $vs 'MSBuild\Current\Bin\amd64\MSBuild.exe' }

    $cmakeRoot = Join-Path $WorkRoot 'tools-cmake'
    New-Item -ItemType Directory -Path $cmakeRoot -ErrorAction Stop | Out-Null
    Invoke-Native 'cmake-extract' $tar @('-xf', (Join-Path $bundleRoot $CmakeArchiveName), '-C', $cmakeRoot) $WorkRoot 600 | Out-Null
    $cmake = Join-Path $cmakeRoot $CmakeExeRelative
    Require-File $cmake
    Invoke-CleanChild 'cmake-version' $cmake @('--version') $WorkRoot 60 | Out-Null
    $cmakeVersion = ((Log-Text 'cmake-version') -split "`r?`n")[0].Trim()

    function Census-Tool([string]$Path, [string]$Version) {
        Require-File $Path
        if ([string]::IsNullOrWhiteSpace($Version)) { $Version = (Get-Item -LiteralPath $Path).VersionInfo.FileVersion }
        return [ordered]@{version=$Version; path=$Path; sha256=(Digest $Path)}
    }
    $rustcVersion = ([string]$probe.rustc_version).Trim()
    $cargoVersion = ([string]$probe.cargo_version).Trim()
    $census = [ordered]@{
        schema = 'solstone.nvattest-windows-tool-census.v1'
        rustc = (Census-Tool ([string]$probe.rustc) $rustcVersion)
        cargo = (Census-Tool ([string]$probe.cargo) $cargoVersion)
        cmake = (Census-Tool $cmake $cmakeVersion)
        cl = (Census-Tool ([string]$probe.cl) '')
        link = (Census-Tool ([string]$probe.link) '')
        nmake = (Census-Tool ([string]$probe.nmake) '')
        msbuild = (Census-Tool $msbuildPath '')
        msvc = [ordered]@{vc_tools_version = [string]$probe.vc_tools_version}
        windows_sdk = [ordered]@{version = $windowsSdk}
        vs = [ordered]@{product_version = [string]$probe.visual_studio_version}
    }
    $censusPath = Join-Path $ReportRoot 'tool-census.json'
    Write-NewJson $censusPath $census

    # Three refusal controls through the SDK's real entry, each from a fresh root.
    $refusalManifest = Require-Refusal 'refusal-manifest-digest' (Get-SdkArguments (Join-Path $WorkRoot 'refusal-manifest') ('0' * 64)) $RefusalManifestBoundary
    $refusalReuse = Require-Refusal 'refusal-reuse-dependencies' (Get-SdkArguments (Join-Path $WorkRoot 'refusal-reuse') $PinnedManifestSha256 -Reuse) $RefusalReuseBoundary
    $changed = Join-Path $bundleRoot $CorruptMember
    $stream = [IO.File]::Open($changed, [IO.FileMode]::Open, [IO.FileAccess]::ReadWrite, [IO.FileShare]::None)
    try { $originalByte = $stream.ReadByte(); $stream.Position = 0; $stream.WriteByte([byte]($originalByte -bxor 1)) } finally { $stream.Dispose() }
    try {
        $refusalCorrupt = Require-Refusal 'refusal-changed-input' (Get-SdkArguments (Join-Path $WorkRoot 'refusal-changed') $PinnedManifestSha256) $RefusalCorruptBoundary
    } finally {
        $stream = [IO.File]::Open($changed, [IO.FileMode]::Open, [IO.FileAccess]::Write, [IO.FileShare]::None)
        try { $stream.WriteByte([byte]$originalByte) } finally { $stream.Dispose() }
    }
    if ((Get-Item -LiteralPath $changed).Length -ne $members[$CorruptMember].size -or
        (Digest $changed) -cne $members[$CorruptMember].sha256) {
        throw "changed-input control did not restore $CorruptMember to its manifest digest"
    }

    # The one real build, from a fresh root.
    $realRoot = Join-Path $WorkRoot 'build'
    $realArguments = Get-SdkArguments $realRoot $PinnedManifestSha256
    Invoke-CleanChild 'sdk-build' $powershell $realArguments $WorkRoot $BuildSeconds | Out-Null
    $dist = Join-Path $realRoot 'dist\nvattest'
    $report = Join-Path $realRoot 'build-report.json'
    $staged = [ordered]@{
        'bin\nvattest.exe' = $null; 'bin\msvcp140.dll' = $null; 'bin\vcruntime140.dll' = $null
        'bin\vcruntime140_1.dll' = $null; 'LICENSE' = $null; 'share\ca\ca-bundle.pem' = $null
    }
    foreach ($relative in @($staged.Keys)) {
        $source = Join-Path $dist $relative
        Require-File $source
        $staged[$relative] = $source
    }
    Require-File $report
    $sdkCmake = Join-Path $realRoot $CmakeExeRelative
    Require-File $sdkCmake
    if ((Digest $sdkCmake) -cne $census.cmake.sha256) { throw 'the SDK build ran a different cmake.exe than the census hashed' }

    $dumpbinRoot = Join-Path $ReportRoot 'dumpbin'
    New-Item -ItemType Directory -Path $dumpbinRoot -ErrorAction Stop | Out-Null
    foreach ($name in @('nvattest.exe', 'msvcp140.dll', 'vcruntime140.dll', 'vcruntime140_1.dll')) {
        $label = 'dumpbin-' + ($name -replace '[^a-z0-9]', '-')
        Invoke-Native $label ([string]$probe.dumpbin) @('/nologo', '/dependents', (Join-Path $dist "bin\$name")) $WorkRoot 120 $recorderEnvironment | Out-Null
        [IO.File]::Copy((Join-Path $logRoot "$label.stdout"), (Join-Path $dumpbinRoot "$name.dependents.txt"), $false)
    }

    foreach ($relative in $staged.Keys) {
        $target = Join-Path $OutputRoot $relative
        New-Item -ItemType Directory -Path (Split-Path -Parent $target) -Force | Out-Null
        [IO.File]::Copy($staged[$relative], $target, $false)
    }
    [IO.File]::Copy($report, (Join-Path $ReportRoot 'build-report.json'), $false)
    Check-Product 'after'

    if ((Remove-Rules) -ne 0) { throw 'network-rules: firewall rules remained after removal' }
    $controls = [ordered]@{
        schema = 'solstone.nvattest-windows-driver-controls.v1'
        invocation = [ordered]@{
            offline = $true; bundle_path = $bundleRoot; manifest_sha256 = $PinnedManifestSha256
            source_commit = $PinnedSdkRevision; environment = @($ChildEnvironmentNames)
            argv = @(@($powershell) + $realArguments)
        }
        refusals = [ordered]@{manifest_digest = $refusalManifest; reuse_dependencies = $refusalReuse; corrupt_member = $refusalCorrupt}
        network = [ordered]@{
            transport_peer = $peer.ToString()
            ipv4 = [ordered]@{target = "${Ipv4Probe}:$ProbePort"; positive_control = 'connected'; negative_control = 'refused'}
            ipv6 = [ordered]@{target = "[$Ipv6Probe]:$ProbePort"; positive_control = $ipv6Positive; negative_control = 'refused'}
            rules_added = $rulesAdded; rules_remaining = $rulesRemaining
        }
    }
    $controlsPath = Join-Path $ReportRoot 'driver-controls.json'
    Write-NewJson $controlsPath $controls

    $validationLines = [Collections.Generic.List[string]]::new()
    foreach ($line in @('schema=solstone.nvattest-windows-validation.v1', "product_commit=$ExpectedProductCommit",
        "cargo_lock_sha256=$ExpectedCargoLockSha256", "sdk_revision=$PinnedSdkRevision",
        "source_archive_sha256=$SourceSha256", "source_archive_size=$SourceSize",
        "bundle_archive_sha256=$BundleSha256", "bundle_archive_size=$BundleSize", "manifest_sha256=$PinnedManifestSha256",
        "recorder_sha256=$(Digest $recorder)", "census_sha256=$(Digest $censusPath)", "controls_sha256=$(Digest $controlsPath)",
        "report_sha256=$(Digest $report)", "sdk_cmake_sha256=$(Digest $sdkCmake)",
        "network=denied-for-all-programs-except-$peer", "firewall_rules_added=$rulesAdded", "firewall_rules_remaining=$rulesRemaining")) {
        $validationLines.Add($line)
    }
    foreach ($name in $ChildEnvironmentNames) { $validationLines.Add("child_environment.$name=$($childEnvironment[$name])") }
    foreach ($label in @('census-probe', 'cmake-version', 'refusal-manifest-digest', 'refusal-reuse-dependencies',
        'refusal-changed-input', 'sdk-build')) {
        $validationLines.Add("log.$label.stdout_sha256=$(Digest (Join-Path $logRoot "$label.stdout"))")
        $validationLines.Add("log.$label.stderr_sha256=$(Digest (Join-Path $logRoot "$label.stderr"))")
    }
    foreach ($relative in $staged.Keys) {
        $validationLines.Add("output.$($relative.Replace('\', '/'))_sha256=$(Digest (Join-Path $OutputRoot $relative))")
    }
    $validation = Join-Path $ReportRoot 'nvattest-build-validation.log'
    Write-NewText $validation (($validationLines -join "`n") + "`n")

    $evidence = Join-Path $ReportRoot 'nvattest-build-evidence.json'
    $receipt = Join-Path $ReportRoot 'nvattest-build-receipt.json'
    Invoke-Native 'record' $recorder @('nvattest-windows', 'record', '--repo', $RepositoryRoot,
        '--source-archive', $SourceArchive, '--bundle-archive', $BundleArchive, '--output-root', $OutputRoot,
        '--report', $report, '--census', $censusPath, '--controls', $controlsPath, '--dumpbin-dir', $dumpbinRoot,
        '--validation', $validation, '--evidence', $evidence, '--receipt', $receipt,
        '--product-commit', $ExpectedProductCommit, '--cargo-lock-sha256', $ExpectedCargoLockSha256,
        '--builder-host', $BuilderHost, '--toolchain', "MSVC $($probe.vc_tools_version); $rustcVersion; $cmakeVersion") $RepositoryRoot 900 | Out-Null
    Invoke-Native 'verify' $recorder @('nvattest-windows', 'verify', '--receipt', $receipt, '--evidence', $evidence,
        '--validation', $validation, '--output-root', $OutputRoot) $RepositoryRoot 300 | Out-Null
    Write-Output "NVATTEST_WINDOWS_BUILD_OK output=$OutputRoot receipt=$receipt evidence=$evidence validation=$validation"
    $exitCode = 0
} catch {
    $failures.Add($_.ToString())
    $failures.Add($_.ScriptStackTrace)
} finally {
    $stoppedChildren = 0
    try {
        $stoppedChildren = Stop-DescendantProcesses
        if ($stoppedChildren -ne 0) { $failures.Add("stopped $stoppedChildren child processes still running before the denial was lifted") }
    } catch {
        $failures.Add("child process census failed before the denial was lifted: $_")
    }
    try {
        if ($rules.Count -ne 0 -and $rulesRemaining -ne 0) { Remove-Rules | Out-Null }
    } catch {
        $failures.Add("firewall rule removal failed: $_")
    }
    try { $rulesRemaining = Count-RemainingRules } catch { $failures.Add("firewall rule census failed: $_"); $rulesRemaining = -1 }
    if ($rulesRemaining -ne 0 -or $failures.Count -ne 0) { $exitCode = 1 }
    foreach ($name in $environmentChanges.Keys) { [Environment]::SetEnvironmentVariable($name, $environmentChanges[$name], 'Process') }
    Write-NewJson (Join-Path $ReportRoot 'execution.json') ([ordered]@{exit_code=$exitCode; failures=@($failures.ToArray());
        firewall_rules_added=$rulesAdded; firewall_rules_remaining=$rulesRemaining; children_stopped_before_lifting_denial=$stoppedChildren; firewall_rules=@($rules.ToArray());
        captured_processes=@($captures | ForEach-Object {
            [ordered]@{pid=$_.pid; started=$_.started; exit_code=$_.exit_code; completed=$_.completed; elapsed_ms=$_.elapsed_ms; error=$_.error}
        });
        product_commit=$ExpectedProductCommit; cargo_lock_sha256=$ExpectedCargoLockSha256})
}
Write-Output "NVATTEST_WINDOWS_TERMINAL exit=$exitCode firewall_rules_remaining=$rulesRemaining report=$ReportRoot"
exit $exitCode
